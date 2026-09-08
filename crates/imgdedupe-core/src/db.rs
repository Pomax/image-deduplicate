use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Transaction};

/// Re-exported so nothing above this layer needs its own SQLite dependency.
pub use rusqlite::Connection;

use crate::fingerprint::{self, Fingerprint};
use crate::format::Format;

pub const SCHEMA_VERSION: i64 = 1;

/// Default name of the index inside the scanned folder.
pub const INDEX_FILENAME: &str = "imgdedupe.sqlite";

/// Every statement that defines the index. Applied on open and reused by tests
/// that want the same shape in memory.
///
/// `meta` holds facts about the index itself: the schema version, the last scan,
/// how far down the folder it reaches, and where a cleanup sends what it
/// removes. Which folder it is about is the folder the file sits in, so that is
/// not written down anywhere.
///
/// `files.mtime_ms` is milliseconds since the epoch; `files.last_scanned_at` is
/// seconds.
///
/// `fingerprints.corners` holds the picture's corners and what each one looks
/// like, and is empty for a picture with nothing corner-shaped in it: a flat
/// sky, or one out of focus.
///
/// `ignore` holds pairs of pictures that are not to be treated as copies of each
/// other, lower id first. A set every pair of which is in there is a set the
/// review shows and the cleanup steps over. The rows go when either file does,
/// which is what the foreign keys are for: a pair that has lost a side is not a
/// pair.
///
/// `phash_bands` was a table of 128 rows per picture that the search now works
/// out as it loads, so any file still carrying one is relieved of it.
///
/// None of this is written into the statements below. SQLite stores their text
/// as typed and hands it back to the parser on every open, so a comment in there
/// is data in the file rather than a note to whoever reads this, and dropping a
/// column rewrites that text around it.
pub const SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS meta (
    key   TEXT PRIMARY KEY,
    value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS files (
    id              INTEGER PRIMARY KEY,
    rel_path        TEXT    NOT NULL UNIQUE,
    size_bytes      INTEGER NOT NULL,
    mtime_ms        INTEGER NOT NULL,
    last_scanned_at INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS images (
    file_id  INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    width    INTEGER NOT NULL,
    height   INTEGER NOT NULL,
    format   TEXT    NOT NULL,
    channels INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS fingerprints (
    file_id             INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE,
    fingerprint_version INTEGER NOT NULL,
    dct_hashes          BLOB    NOT NULL,
    ring_stats          BLOB    NOT NULL,
    corners             BLOB    NOT NULL DEFAULT x''
);

CREATE TABLE IF NOT EXISTS ignore (
    lower  INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    higher INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    PRIMARY KEY (lower, higher)
);

DROP TABLE IF EXISTS phash_bands;

CREATE VIEW IF NOT EXISTS indexed_images AS
SELECT f.id, f.rel_path, f.size_bytes, f.mtime_ms,
       i.width, i.height, i.format, i.channels,
       p.dct_hashes, p.ring_stats, p.corners
FROM files f
JOIN images i       ON i.file_id = f.id
JOIN fingerprints p ON p.file_id = f.id;
";

/// Open a folder's index: bring the file to the current shape, then read it in.
///
/// Only the manager calls this, and nothing else opens the index.
pub fn open_and_migrate(path: &Path) -> Result<Connection> {
    // The file is brought to the current shape first, on disk, so no reader can
    // be handed a connection to an index that is still in an older one.
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    migrate_the_file(path)?;
    crate::log_line!("    migrate the file: {:.2}s", at.elapsed().as_secs_f64());

    // Then read in one go. Every statement against a database on another machine
    // is a round trip; in memory they are free, and what reaches the network is
    // one sequential read of a file that is a few megabytes.
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading the index at {}", path.display()))?;
    crate::log_line!("    read {} bytes of index: {:.2}s", bytes.len(), at.elapsed().as_secs_f64());
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let conn = into_memory(bytes, path)?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    crate::log_line!("    hand it to sqlite: {:.2}s", at.elapsed().as_secs_f64());
    Ok(conn)
}

/// Bring the file itself to the current shape: the schema, the columns an older
/// build lacks, and the version it is written under.
///
/// This happens before anything reads a row.
fn migrate_the_file(path: &Path) -> Result<()> {
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let conn = Connection::open(path)
        .with_context(|| format!("opening the index at {}", path.display()))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    crate::log_line!("      open the file: {:.2}s", at.elapsed().as_secs_f64());
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    conn.execute_batch(SCHEMA).context("applying the schema")?;
    crate::log_line!("      apply the schema: {:.2}s", at.elapsed().as_secs_f64());
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    add_new_columns(&conn)?;
    carry_the_stamps_across(&conn)?;
    drop_dead_columns(&conn)?;
    crate::log_line!("      the columns: {:.2}s", at.elapsed().as_secs_f64());

    let existing: Option<i64> = conn
        .query_row("SELECT value FROM meta WHERE key = 'schema_version'", [], |row| {
            row.get::<_, String>(0)
        })
        .ok()
        .and_then(|value| value.parse().ok());
    match existing {
        Some(version) if version != SCHEMA_VERSION => {
            anyhow::bail!("index was written by schema version {version}, this build speaks {SCHEMA_VERSION}");
        }
        Some(_) => {}
        None => set_meta(&conn, "schema_version", &SCHEMA_VERSION.to_string())?,
    }
    drop(conn);
    Ok(())
}

/// Where the index is written before it is moved onto itself.
/// Take the columns nothing reads out of an index written by an older build.
///
/// A file already on disk keeps whatever columns it was made with, and the ones
/// that are `NOT NULL` would refuse every insert that no longer names them.
fn drop_dead_columns(conn: &Connection) -> Result<()> {
    let dead =
        [("files", "bytes_hash"), ("fingerprints", "dct_hash"), ("files", "mtime_ns")];
    let mut found = Vec::new();
    for (table, column) in dead {
        let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
        let present = statement
            .query_map([], |row| row.get::<_, String>(1))?
            .filter_map(Result::ok)
            .any(|name| name == column);
        if present {
            found.push((table, column));
        }
    }
    if found.is_empty() {
        return Ok(());
    }
    // The view names them, and a column a view reads cannot be dropped.
    conn.execute_batch("DROP VIEW IF EXISTS indexed_images")?;
    conn.execute_batch("DROP INDEX IF EXISTS files_bytes_hash")?;
    for (table, column) in found {
        conn.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {column}"))
            .with_context(|| format!("dropping {table}.{column}"))?;
    }
    conn.execute_batch(SCHEMA).context("rebuilding the schema")?;
    Ok(())
}

/// Add the columns a newer build writes to an index made by an older one.
///
/// `CREATE TABLE IF NOT EXISTS` leaves a table that already exists exactly as it
/// was, so a file from an older build keeps the shape it was made with. The rows
/// in it are re-fingerprinted anyway, because the fingerprint version moved, and
/// they need somewhere to be written to.
fn add_new_columns(conn: &Connection) -> Result<()> {
    let wanted = [
        ("fingerprints", "corners", "BLOB NOT NULL DEFAULT x''"),
        ("files", "mtime_ms", "INTEGER NOT NULL DEFAULT 0"),
    ];
    let mut added = false;
    for (table, column, declaration) in wanted {
        if has_column(conn, table, column)? {
            continue;
        }
        conn.execute_batch(&format!("ALTER TABLE {table} ADD COLUMN {column} {declaration}"))
            .with_context(|| format!("adding {table}.{column}"))?;
        added = true;
    }
    if !added {
        return Ok(());
    }
    // The view was made without them and would go on reading the old shape.
    conn.execute_batch("DROP VIEW IF EXISTS indexed_images")?;
    conn.execute_batch(SCHEMA).context("rebuilding the schema")?;
    Ok(())
}

/// Fill `files.mtime_ms` from the nanoseconds an older build wrote. Runs between
/// the column being added and `mtime_ns` being dropped.
fn carry_the_stamps_across(conn: &Connection) -> Result<()> {
    if !has_column(conn, "files", "mtime_ns")? {
        return Ok(());
    }
    conn.execute_batch("UPDATE files SET mtime_ms = mtime_ns / 1000000")
        .context("carrying the modification times across")?;
    Ok(())
}

/// Whether a table has a column.
fn has_column(conn: &Connection, table: &str, column: &str) -> Result<bool> {
    let mut statement = conn.prepare(&format!("PRAGMA table_info({table})"))?;
    let present = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == column);
    Ok(present)
}






/// Hand a database's bytes to SQLite as a database in memory. What comes back
/// grows as rows are added, and the manager puts it back on disk.
fn into_memory(bytes: Vec<u8>, path: &Path) -> Result<Connection> {
    let size = bytes.len();

    // SQLite takes ownership of this and frees it with its own allocator, so it
    // has to come from that allocator.
    let held = unsafe { rusqlite::ffi::sqlite3_malloc64(size as u64) }.cast::<u8>();
    let Some(held) = std::ptr::NonNull::new(held) else {
        anyhow::bail!("no room for a {size} byte copy of {}", path.display());
    };
    // Safety: `held` is `size` bytes from SQLite's allocator and `bytes` is that
    // long, so the two do not overlap and neither is short.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), held.as_ptr(), size) };
    // Safety: allocated by `sqlite3_malloc64` immediately above, as required.
    let data = unsafe { rusqlite::serialize::OwnedData::from_raw_nonnull(held, size) };

    let mut conn = Connection::open_in_memory().context("opening an index in memory")?;
    conn.deserialize(rusqlite::DatabaseName::Main, data, false)
        .with_context(|| format!("reading {} as a database", path.display()))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}





pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}


/// Take a setting out, leaving a folder that has never been asked about it,
/// which is not the same as one that answered no.
pub fn forget_meta(conn: &Connection, key: &str) -> Result<()> {
    conn.execute("DELETE FROM meta WHERE key = ?1", params![key])?;
    Ok(())
}

pub fn get_meta(conn: &Connection, key: &str) -> Result<Option<String>> {
    let mut statement = conn.prepare("SELECT value FROM meta WHERE key = ?1")?;
    let mut rows = statement.query(params![key])?;
    Ok(match rows.next()? {
        Some(row) => Some(row.get(0)?),
        None => None,
    })
}

/// What the incremental diff needs to know about a path already in the index.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Known {
    pub id: i64,
    pub size_bytes: i64,
    pub mtime_ms: i64,
    pub fingerprint_version: i64,
}

/// Every indexed path, in one query. A row whose fingerprints are missing reads
/// as version -1 so it is always treated as stale.
pub fn load_known(conn: &Connection) -> Result<HashMap<String, Known>> {
    let mut statement = conn.prepare(
        "SELECT f.rel_path, f.id, f.size_bytes, f.mtime_ms, COALESCE(p.fingerprint_version, -1)
         FROM files f LEFT JOIN fingerprints p ON p.file_id = f.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Known {
                id: row.get(1)?,
                size_bytes: row.get(2)?,
                mtime_ms: row.get(3)?,
                fingerprint_version: row.get(4)?,
            },
        ))
    })?;
    let mut out = HashMap::new();
    for row in rows {
        let (path, known) = row?;
        out.insert(path, known);
    }
    Ok(out)
}

/// Everything the index records about one image file.
pub struct Record {
    pub rel_path: String,
    pub size_bytes: i64,
    pub mtime_ms: i64,
    pub width: u32,
    pub height: u32,
    pub format: Format,
    pub channels: u8,
    pub fingerprint: Fingerprint,
    /// The picture's corners, packed. Empty when it has none.
    pub corners: Vec<u8>,
}

/// Write one image into every table it belongs in. Callers batch these inside a
/// transaction, which is what makes a killed run leave a consistent index.
pub fn upsert(tx: &Transaction<'_>, record: &Record, scanned_at: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO files(rel_path, size_bytes, mtime_ms, last_scanned_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(rel_path) DO UPDATE SET
             size_bytes = excluded.size_bytes,
             mtime_ms = excluded.mtime_ms,
             last_scanned_at = excluded.last_scanned_at",
        params![record.rel_path, record.size_bytes, record.mtime_ms, scanned_at],
    )?;
    let file_id: i64 = tx.query_row(
        "SELECT id FROM files WHERE rel_path = ?1",
        params![record.rel_path],
        |row| row.get(0),
    )?;

    tx.execute(
        "INSERT INTO images(file_id, width, height, format, channels)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(file_id) DO UPDATE SET
             width = excluded.width,
             height = excluded.height,
             format = excluded.format,
             channels = excluded.channels",
        params![
            file_id,
            record.width as i64,
            record.height as i64,
            record.format.as_str(),
            record.channels as i64
        ],
    )?;

    tx.execute(
        "INSERT INTO fingerprints(file_id, fingerprint_version, dct_hashes, ring_stats, corners)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(file_id) DO UPDATE SET
             fingerprint_version = excluded.fingerprint_version,
             dct_hashes = excluded.dct_hashes,
             ring_stats = excluded.ring_stats,
             corners = excluded.corners",
        params![
            file_id,
            fingerprint::FINGERPRINT_VERSION,
            fingerprint::pack_hashes(&record.fingerprint.dct_hashes),
            record.fingerprint.ring_stats,
            record.corners
        ],
    )?;

    Ok(())
}

/// Remove paths that are no longer on disk. The cascade clears the derived tables.
pub fn delete_paths(tx: &Transaction<'_>, paths: &[String]) -> Result<usize> {
    let mut statement = tx.prepare_cached("DELETE FROM files WHERE rel_path = ?1")?;
    let mut removed = 0;
    for path in paths {
        removed += statement.execute(params![path])?;
    }
    Ok(removed)
}

/// One pair of pictures that are not copies of each other, lower id first, which
/// is how they are stored and how they are asked for.
pub fn pair(one: i64, other: i64) -> (i64, i64) {
    (one.min(other), one.max(other))
}

/// Write down that these pairs are not copies of each other. Saying it twice is
/// saying it once: the pair is the key.
pub fn ignore(conn: &Connection, pairs: &[(i64, i64)]) -> Result<()> {
    let mut statement =
        conn.prepare_cached("INSERT OR IGNORE INTO ignore (lower, higher) VALUES (?1, ?2)")?;
    for (one, other) in pairs {
        let (lower, higher) = pair(*one, *other);
        statement.execute(params![lower, higher])?;
    }
    Ok(())
}

/// Take those pairs back: they are copies of each other after all. A pair that
/// was never written down is nothing to take back.
pub fn unignore(conn: &Connection, pairs: &[(i64, i64)]) -> Result<()> {
    let mut statement =
        conn.prepare_cached("DELETE FROM ignore WHERE lower = ?1 AND higher = ?2")?;
    for (one, other) in pairs {
        let (lower, higher) = pair(*one, *other);
        statement.execute(params![lower, higher])?;
    }
    Ok(())
}

/// Every pair that has been ignored.
///
/// An index written before there was such a thing has no table to read, which is
/// not a failure: it is a folder where nothing has been ignored.
pub fn ignored(conn: &Connection) -> Result<Vec<(i64, i64)>> {
    let table = conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ignore'");
    let known = match table {
        Ok(mut statement) => statement.exists([])?,
        Err(_) => false,
    };
    if !known {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare("SELECT lower, higher FROM ignore")?;
    let rows = statement.query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?;
    Ok(rows.filter_map(Result::ok).collect())
}

#[cfg(test)]
mod ignoring {
    use super::*;

    /// A pair is written down once whichever way round it is given, and comes
    /// back the same way. Saying it twice says it once.
    #[test]
    fn a_pair_is_the_same_pair_either_way_round() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        let conn = open_and_migrate(&path).expect("open");
        conn.execute_batch(
            "INSERT INTO files (id, rel_path, size_bytes, mtime_ms, last_scanned_at)
             VALUES (7, 'a.jpg', 1, 1, 1), (9, 'b.jpg', 1, 1, 1), (11, 'c.jpg', 1, 1, 1)",
        )
        .expect("files");

        ignore(&conn, &[(9, 7), (7, 9), (11, 7)]).expect("ignore");
        let mut held = ignored(&conn).expect("read");
        held.sort();
        assert_eq!(held, vec![(7, 9), (7, 11)]);
    }

    /// A file that is gone takes its pairs with it: a pair with one side left is
    /// not a pair.
    #[test]
    fn a_pair_goes_when_either_of_its_pictures_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        let conn = open_and_migrate(&path).expect("open");
        conn.execute_batch(
            "INSERT INTO files (id, rel_path, size_bytes, mtime_ms, last_scanned_at)
             VALUES (1, 'a.jpg', 1, 1, 1), (2, 'b.jpg', 1, 1, 1)",
        )
        .expect("files");
        ignore(&conn, &[(1, 2)]).expect("ignore");
        assert_eq!(ignored(&conn).expect("read").len(), 1);

        conn.execute_batch("DELETE FROM files WHERE id = 2").expect("delete");
        assert!(ignored(&conn).expect("read").is_empty(), "the pair outlived its picture");
    }

    /// An index written before there was such a table has nothing ignored,
    /// which is an answer rather than a failure.
    #[test]
    fn an_index_without_the_table_has_nothing_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        let conn = open_and_migrate(&path).expect("open");
        conn.execute_batch("DROP TABLE ignore").expect("drop");
        assert!(ignored(&conn).expect("read").is_empty());
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fingerprint::Fingerprint;

    /// A hash whose first bytes carry the seed, so two seeds differ in a known
    /// number of bits rather than in whatever the encoding happens to give.
    fn hash_seeded(seed: u64) -> fingerprint::Hash {
        let mut out = [0u8; fingerprint::HASH_BYTES];
        out[..8].copy_from_slice(&seed.to_le_bytes());
        out
    }

    fn record(path: &str, seed: u64) -> Record {
        let hash = hash_seeded(seed);
        Record {
            rel_path: path.to_string(),
            size_bytes: 1234,
            mtime_ms: 999,
            width: 800,
            height: 600,
            format: Format::Jpeg,
            channels: 3,
            fingerprint: Fingerprint {
                dct_hashes: [hash, hash, hash, hash, hash, hash, hash, hash],
                ring_stats: vec![1, 2, 3, 4],
            },
            corners: Vec::new(),
        }
    }

    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "foreign_keys", "ON").expect("pragma");
        conn.execute_batch(SCHEMA).expect("schema");
        conn
    }

    /// A file's id is not a lasting name for it. Deleting the last rows and
    /// indexing again hands those ids to different files, so anything outside the
    /// index that remembers a file by its id is remembering the wrong file.
    #[test]
    fn a_file_id_is_reused_after_the_rows_above_it_are_deleted() {
        let mut conn = memory_db();
        let tx = conn.transaction().expect("tx");
        for path in ["a.jpg", "b.jpg", "c.jpg"] {
            upsert(&tx, &record(path, 1), 1).expect("insert");
        }
        tx.commit().expect("commit");

        let id_of = |conn: &Connection, path: &str| -> i64 {
            conn.query_row("SELECT id FROM files WHERE rel_path = ?1", [path], |row| row.get(0))
                .expect("id")
        };
        let was = id_of(&conn, "c.jpg");

        let tx = conn.transaction().expect("tx");
        delete_paths(&tx, &[String::from("c.jpg")]).expect("delete");
        tx.commit().expect("commit");

        let tx = conn.transaction().expect("tx");
        upsert(&tx, &record("something else.jpg", 1), 1).expect("insert");
        tx.commit().expect("commit");

        assert_eq!(
            id_of(&conn, "something else.jpg"),
            was,
            "the id was not reused, so this check is no longer measuring anything"
        );
    }

    #[test]
    fn upsert_writes_every_table_and_is_idempotent() {
        let mut conn = memory_db();
        let tx = conn.transaction().expect("tx");
        upsert(&tx, &record("a.jpg", 0x1122_3344_5566_7788), 1).expect("insert");
        upsert(&tx, &record("a.jpg", 0x1122_3344_5566_7788), 2).expect("re-insert");
        tx.commit().expect("commit");

        let files: i64 = conn.query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap();
        let images: i64 = conn.query_row("SELECT count(*) FROM images", [], |r| r.get(0)).unwrap();
        let prints: i64 =
            conn.query_row("SELECT count(*) FROM fingerprints", [], |r| r.get(0)).unwrap();
        assert_eq!((files, images, prints), (1, 1, 1));
    }

    #[test]
    fn deleting_a_file_cascades_to_the_derived_tables() {
        let mut conn = memory_db();
        let tx = conn.transaction().expect("tx");
        upsert(&tx, &record("a.jpg", 1), 1).expect("insert");
        upsert(&tx, &record("b.jpg", 2), 1).expect("insert");
        tx.commit().expect("commit");

        let tx = conn.transaction().expect("tx");
        assert_eq!(delete_paths(&tx, &["a.jpg".to_string()]).expect("delete"), 1);
        tx.commit().expect("commit");

        let files: i64 = conn.query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap();
        let images: i64 = conn.query_row("SELECT count(*) FROM images", [], |r| r.get(0)).unwrap();
        let prints: i64 =
            conn.query_row("SELECT count(*) FROM fingerprints", [], |r| r.get(0)).unwrap();
        assert_eq!((files, images, prints), (1, 1, 1));
    }

    #[test]
    fn load_known_reports_what_the_diff_needs() {
        let mut conn = memory_db();
        let tx = conn.transaction().expect("tx");
        upsert(&tx, &record("a.jpg", 1), 1).expect("insert");
        tx.commit().expect("commit");

        let known = load_known(&conn).expect("load");
        let entry = known.get("a.jpg").expect("path present");
        assert_eq!(entry.size_bytes, 1234);
        assert_eq!(entry.mtime_ms, 999);
        assert_eq!(entry.fingerprint_version, fingerprint::FINGERPRINT_VERSION);
    }

    #[test]
    fn a_file_without_fingerprints_reads_as_stale() {
        let conn = memory_db();
        conn.execute(
            "INSERT INTO files(rel_path, size_bytes, mtime_ms, last_scanned_at)
             VALUES ('orphan.png', 1, 1, 1)",
            [],
        )
        .expect("insert");
        let known = load_known(&conn).expect("load");
        assert_eq!(known["orphan.png"].fingerprint_version, -1);
    }

    /// An index written by a schema version this build does not speak is one it
    /// cannot bring to the current shape, so it is refused rather than read.
    #[test]
    fn an_index_from_another_schema_version_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        {
            let conn = open_and_migrate(&path).expect("create");
            drop(conn);
            let file = Connection::open(&path).expect("open the file");
            set_meta(&file, "schema_version", "99").expect("bump");
        }
        let err = open_and_migrate(&path).expect_err("should refuse");
        assert!(err.to_string().contains("schema version 99"), "{err}");
    }

    /// The schema and the columns an older build lacks are applied to the file,
    /// not to a copy of it, so nothing can be handed an index in an older shape.
    #[test]
    fn an_index_in_an_older_shape_is_migrated_on_disk_when_it_is_opened() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        {
            let conn = Connection::open(&path).expect("create");
            conn.execute_batch(
                "CREATE TABLE files (
                     id              INTEGER PRIMARY KEY,
                     rel_path        TEXT    NOT NULL UNIQUE,
                     size_bytes      INTEGER NOT NULL,
                     mtime_ns        INTEGER NOT NULL,
                     last_scanned_at INTEGER NOT NULL
                 );
                 CREATE TABLE fingerprints (
                     file_id             INTEGER PRIMARY KEY,
                     fingerprint_version INTEGER NOT NULL,
                     dct_hashes          BLOB    NOT NULL,
                     ring_stats          BLOB    NOT NULL
                 );
                 INSERT INTO files(rel_path, size_bytes, mtime_ns, last_scanned_at)
                 VALUES ('a.jpg', 1234, 5, 1);",
            )
            .expect("an older index");
        }

        let conn = open_and_migrate(&path).expect("open");
        drop(conn);

        // The file itself, not the copy that was handed back.
        let file = Connection::open(&path).expect("open the file");
        let columns: Vec<String> = file
            .prepare("PRAGMA table_info(fingerprints)")
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert!(columns.iter().any(|name| name == "corners"), "not migrated: {columns:?}");
        let rows: i64 =
            file.query_row("SELECT count(*) FROM files", [], |row| row.get(0)).expect("count");
        assert_eq!(rows, 1, "the row was lost");
    }

    /// An index from a build that kept stamps in nanoseconds comes back holding
    /// milliseconds, and the nanosecond column is gone.
    #[test]
    fn an_index_written_in_nanoseconds_comes_back_in_milliseconds() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        make_an_old_index(&path, "mtime_ns", 1_700_000_000_123_456_789);

        let conn = open_and_migrate(&path).expect("open");
        drop(conn);

        // The file itself, not the copy that was handed back.
        let file = Connection::open(&path).expect("open the file");
        let columns = column_names(&file, "files");
        assert!(columns.iter().any(|name| name == "mtime_ms"), "no mtime_ms: {columns:?}");
        assert!(!columns.iter().any(|name| name == "mtime_ns"), "mtime_ns left: {columns:?}");
        let stamp: i64 =
            file.query_row("SELECT mtime_ms FROM files", [], |row| row.get(0)).expect("the row");
        assert_eq!(stamp, 1_700_000_000_123, "the stamp was not carried across");
        drop(file);

        let conn = open_and_migrate(&path).expect("reopen");
        let known = load_known(&conn).expect("load");
        assert_eq!(known["a.jpg"].mtime_ms, 1_700_000_000_123);
    }

    /// A run killed after the stamps were carried across and before the old
    /// column was dropped. The next open finishes the job rather than dividing
    /// what it already divided.
    #[test]
    fn a_half_converted_index_is_finished_rather_than_converted_twice() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        make_an_old_index(&path, "mtime_ns", 1_700_000_000_123_456_789);
        {
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch(
                "ALTER TABLE files ADD COLUMN mtime_ms INTEGER NOT NULL DEFAULT 0;
                 UPDATE files SET mtime_ms = 1700000000123;",
            )
            .expect("half of it");
        }

        let conn = open_and_migrate(&path).expect("open");
        let known = load_known(&conn).expect("load");
        assert_eq!(
            known["a.jpg"].mtime_ms, 1_700_000_000_123,
            "the stamp was divided a second time"
        );
        drop(conn);

        let file = Connection::open(&path).expect("open the file");
        let columns = column_names(&file, "files");
        assert!(!columns.iter().any(|name| name == "mtime_ns"), "mtime_ns left: {columns:?}");
    }

    /// A run killed after the column was added and before the stamps were
    /// carried across. The next open carries them.
    #[test]
    fn an_index_given_the_new_column_but_no_values_is_converted() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        make_an_old_index(&path, "mtime_ns", 1_700_000_000_123_456_789);
        {
            let conn = Connection::open(&path).expect("open");
            conn.execute_batch("ALTER TABLE files ADD COLUMN mtime_ms INTEGER NOT NULL DEFAULT 0")
                .expect("the column and nothing else");
        }

        let conn = open_and_migrate(&path).expect("open");
        let known = load_known(&conn).expect("load");
        assert_eq!(known["a.jpg"].mtime_ms, 1_700_000_000_123, "the stamp was left at nought");
        drop(conn);

        let file = Connection::open(&path).expect("open the file");
        let columns = column_names(&file, "files");
        assert!(!columns.iter().any(|name| name == "mtime_ns"), "mtime_ns left: {columns:?}");
    }

    /// An index in the shape an older build left it, with one row.
    fn make_an_old_index(path: &Path, column: &str, stamp: i64) {
        let conn = Connection::open(path).expect("create");
        conn.execute_batch(&format!(
            "CREATE TABLE files (
                 id              INTEGER PRIMARY KEY,
                 rel_path        TEXT    NOT NULL UNIQUE,
                 size_bytes      INTEGER NOT NULL,
                 {column}        INTEGER NOT NULL,
                 last_scanned_at INTEGER NOT NULL
             );
             INSERT INTO files(rel_path, size_bytes, {column}, last_scanned_at)
             VALUES ('a.jpg', 1234, {stamp}, 1);"
        ))
        .expect("an older index");
    }

    fn column_names(conn: &Connection, table: &str) -> Vec<String> {
        conn.prepare(&format!("PRAGMA table_info({table})"))
            .unwrap()
            .query_map([], |row| row.get(1))
            .unwrap()
            .map(Result::unwrap)
            .collect()
    }

    #[test]
    fn a_fresh_index_records_its_schema_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        let conn = open_and_migrate(&path).expect("create");
        assert_eq!(
            get_meta(&conn, "schema_version").expect("meta"),
            Some(SCHEMA_VERSION.to_string())
        );
    }
}
