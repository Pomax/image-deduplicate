use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, OpenFlags, Transaction};

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
/// `fingerprints.corners` holds the picture's corners and what each one looks
/// like, and is empty for a picture with nothing corner-shaped in it: a flat
/// sky, or one out of focus.
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
    mtime_ns        INTEGER NOT NULL,
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

DROP TABLE IF EXISTS phash_bands;

CREATE VIEW IF NOT EXISTS indexed_images AS
SELECT f.id, f.rel_path, f.size_bytes, f.mtime_ns,
       i.width, i.height, i.format, i.channels,
       p.dct_hashes, p.ring_stats, p.corners
FROM files f
JOIN images i       ON i.file_id = f.id
JOIN fingerprints p ON p.file_id = f.id;
";

/// Open for writing, creating the schema if it is not there. Only the indexer
/// does this: it is the single writer.
pub fn open(path: &Path) -> Result<Connection> {
    // The index is worked on in memory and written out as one file when the pass
    // is done. Every statement a pass runs against a database on another machine
    // is a round trip: creating it, the write-ahead log, the shared memory file
    // that a log needs and that a network filesystem cannot properly provide, the
    // schema, and every insert. In memory they are all free, and what reaches the
    // network is one sequential write of a file that is a few megabytes.
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let conn = match std::fs::read(path) {
        Ok(bytes) => {
            let existing = into_memory(bytes, path, false)?;
            crate::log_line!(
                "    read {} bytes of index: {:.2}s",
                std::fs::metadata(path).map(|it| it.len()).unwrap_or(0),
                at.elapsed().as_secs_f64()
            );
            existing
        }
        // No index yet, or none that can be read. Either way this pass builds one.
        Err(_) => Connection::open_in_memory().context("opening an index in memory")?,
    };
    conn.pragma_update(None, "foreign_keys", "ON")?;
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    conn.execute_batch(SCHEMA).context("applying the schema")?;
    drop_dead_columns(&conn)?;
    add_new_columns(&conn)?;
    crate::log_line!("    schema: {:.2}s", at.elapsed().as_secs_f64());

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
    Ok(conn)
}

/// Take the columns nothing reads out of an index written by an older build.
///
/// A file already on disk keeps whatever columns it was made with, and the ones
/// that are `NOT NULL` would refuse every insert that no longer names them.
fn drop_dead_columns(conn: &Connection) -> Result<()> {
    let dead = [("files", "bytes_hash"), ("fingerprints", "dct_hash")];
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
    let mut statement = conn.prepare("PRAGMA table_info(fingerprints)")?;
    let present = statement
        .query_map([], |row| row.get::<_, String>(1))?
        .filter_map(Result::ok)
        .any(|name| name == "corners");
    drop(statement);
    if present {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE fingerprints ADD COLUMN corners BLOB NOT NULL DEFAULT x''")
        .context("adding the corners column")?;
    // The view was made without it and would go on reading the old shape.
    conn.execute_batch("DROP VIEW IF EXISTS indexed_images")?;
    conn.execute_batch(SCHEMA).context("rebuilding the schema")?;
    Ok(())
}

/// Give back the space the deleted rows were using. Everything still indexed
/// stays exactly as it is.
///
/// Without this the file only ever grows: a delete leaves its pages inside the
/// file for the next write to use, and nothing ever gives them back.
///
/// The index is copied, the copy is cleaned, and the copy takes its place. The
/// original is not opened for writing at any point, so a failure at any step
/// leaves it exactly as it was and costs nothing but the copy.
///
/// Nothing may be holding the index open when this runs.
pub fn compact(path: &Path) -> Result<()> {
    let scratch = path.with_extension("compacting");
    let scratch_log = log_beside(&scratch);
    let log = log_beside(path);

    clear(&scratch)?;
    clear(&scratch_log)?;

    std::fs::copy(path, &scratch)
        .with_context(|| format!("copying {} to clean up", path.display()))?;
    // The tail of the index lives in the log beside it, so the copy is not a copy
    // of everything without it.
    if log.exists() {
        std::fs::copy(&log, &scratch_log)
            .with_context(|| format!("copying {}", log.display()))?;
    }

    {
        let conn = Connection::open(&scratch)
            .with_context(|| format!("opening {}", scratch.display()))?;
        conn.execute_batch("VACUUM").context("cleaning up the copy of the index")?;
        conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
            .context("folding the copy's write-ahead log into it")?;
    }
    clear(&scratch_log)?;

    // Only now, with a finished copy on disk, does the original go. Its log has
    // to go with it: it describes the original's pages, and SQLite would apply it
    // to the file taking its place, which was never written for it. Everything it
    // held is in the copy.
    clear(path)?;
    clear(&log)?;
    clear(&index_beside(path))?;

    std::fs::rename(&scratch, path)
        .with_context(|| format!("putting the cleaned index at {}", path.display()))?;
    Ok(())
}

/// The write-ahead log SQLite keeps beside an index, and the shared-memory file
/// that goes with it.
fn log_beside(path: &Path) -> std::path::PathBuf {
    with_suffix(path, "-wal")
}

fn index_beside(path: &Path) -> std::path::PathBuf {
    with_suffix(path, "-shm")
}

fn with_suffix(path: &Path, suffix: &str) -> std::path::PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    std::path::PathBuf::from(name)
}

fn clear(path: &Path) -> Result<()> {
    if path.exists() {
        std::fs::remove_file(path).with_context(|| format!("clearing {}", path.display()))?;
    }
    Ok(())
}

/// Open without the ability to write. The GUI holds one of these open while the
/// indexer runs, which WAL allows.
pub fn open_read_only(path: &Path) -> Result<Connection> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .with_context(|| format!("opening index at {} for reading", path.display()))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// Read the whole index in one go and open that, in memory.
///
/// SQLite reads a database in pages, as a query asks for them. That is right for
/// a file on this machine and wrong for one that is not: every page is its own
/// round trip, and a join across two tables re-fetches pages the cache was too
/// small to keep. The file is small. Reading it once, in order, costs one
/// transfer at whatever the link does, and every query after that is against
/// memory.
///
/// The caller must be sure no write-ahead log is outstanding, because this reads
/// the database file and nothing beside it. `checkpoint` is what makes that true
/// while a writer is open.
pub fn open_snapshot(path: &Path) -> Result<Connection> {
    let bytes = std::fs::read(path)
        .with_context(|| format!("reading the index at {}", path.display()))?;
    into_memory(bytes, path, true)
}

/// Hand a database's bytes to SQLite as a database in memory.
///
/// `read_only` decides whether it can then be written to. A writable one grows in
/// memory as rows are added and is written back out with `write_out`.
fn into_memory(mut bytes: Vec<u8>, path: &Path, read_only: bool) -> Result<Connection> {
    if !can_be_read_in_memory(&mut bytes, path) {
        // Something is in the log that the file does not have. Read it the slow
        // way rather than read it wrong.
        return open_read_only(path);
    }
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
    conn.deserialize(rusqlite::DatabaseName::Main, data, read_only)
        .with_context(|| format!("reading {} as a database", path.display()))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
}

/// Write a database that has been worked on in memory out to its file.
///
/// One sequential write of the whole thing, which is what a few megabytes going
/// to another machine should cost. It is written beside the real file and then
/// moved onto it, so a run that dies half way through leaves the old index rather
/// than half of a new one.
pub fn write_out(conn: &Connection, path: &Path) -> Result<usize> {
    let data = conn
        .serialize(rusqlite::DatabaseName::Main)
        .context("taking the index out of memory")?;
    let bytes: &[u8] = &data;
    let beside = path.with_extension("writing");
    std::fs::write(&beside, bytes)
        .with_context(|| format!("writing the index to {}", beside.display()))?;
    std::fs::rename(&beside, path)
        .with_context(|| format!("moving the index onto {}", path.display()))?;
    Ok(bytes.len())
}

/// Whether these bytes can be handed to SQLite as an in-memory database, making
/// them so if the only thing in the way is the journal mode.
///
/// A database in memory cannot have a write-ahead log, so SQLite refuses an image
/// whose header says the file is in WAL mode, which is what a file with a pass
/// open on it says. Bytes 18 and 19 are the write and read format versions, 1 for
/// a rollback journal and 2 for WAL, and turning them back to 1 is the whole of
/// the difference **once the log has been folded in**.
///
/// So this only does that when no log is left holding anything. A `-wal` longer
/// than its own 32 byte header has frames the database file does not, and reading
/// the file without them answers with rows that have been superseded.
fn can_be_read_in_memory(bytes: &mut [u8], path: &Path) -> bool {
    const WRITE_VERSION: usize = 18;
    const READ_VERSION: usize = 19;
    const WAL: u8 = 2;
    const ROLLBACK: u8 = 1;
    /// A `-wal` holding nothing is this long: the header and no frames.
    const EMPTY_WAL: u64 = 32;

    if bytes.len() <= READ_VERSION {
        return false;
    }
    if bytes[WRITE_VERSION] != WAL && bytes[READ_VERSION] != WAL {
        return true;
    }
    let log = path.with_file_name(format!(
        "{}-wal",
        path.file_name().and_then(|name| name.to_str()).unwrap_or_default()
    ));
    if std::fs::metadata(&log).map(|it| it.len()).unwrap_or(0) > EMPTY_WAL {
        return false;
    }
    bytes[WRITE_VERSION] = ROLLBACK;
    bytes[READ_VERSION] = ROLLBACK;
    true
}

/// Fold the write-ahead log back into the database file, so a copy of that file
/// is the whole of what has been written.
pub fn checkpoint(conn: &Connection) -> Result<()> {
    conn.query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .context("checkpointing the index")?;
    Ok(())
}

/// Close a writing connection and leave one file behind.
///
/// A database in WAL mode keeps a `-wal` beside it holding everything written
/// since the last checkpoint, and a `-shm` that every connection maps to find
/// its way around that log. Folding the log back in and taking the database out
/// of WAL mode is what removes both; deleting them by hand throws away whatever
/// the log still holds.
pub fn close(conn: Connection, path: &Path) -> Result<()> {
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    // The byte count is only there to be logged, but the write itself is not.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    let written = write_out(&conn, path)?;
    crate::log_line!(
        "write the index out: {:.2}s, {written} bytes",
        at.elapsed().as_secs_f64()
    );
    drop(conn);
    // A write-ahead log and its shared memory file, left by a version that kept
    // the index open across the network. Nothing writes them now and a stale one
    // beside the file would be read as newer than it.
    for suffix in ["-wal", "-shm"] {
        let stale = path.with_file_name(format!(
            "{}{suffix}",
            path.file_name().and_then(|name| name.to_str()).unwrap_or_default()
        ));
        let _ = std::fs::remove_file(stale);
    }
    Ok(())
}

pub fn set_meta(conn: &Connection, key: &str, value: &str) -> Result<()> {
    conn.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

/// Open the index for writing what the review left behind, without touching
/// anything a scan owns.
pub fn open_for_notes(path: &Path) -> Result<Connection> {
    let conn = Connection::open(path)
        .with_context(|| format!("opening index at {} to write to", path.display()))?;
    conn.pragma_update(None, "foreign_keys", "ON")?;
    Ok(conn)
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
    pub mtime_ns: i64,
    pub fingerprint_version: i64,
}

/// Every indexed path, in one query. A row whose fingerprints are missing reads
/// as version -1 so it is always treated as stale.
pub fn load_known(conn: &Connection) -> Result<HashMap<String, Known>> {
    let mut statement = conn.prepare(
        "SELECT f.rel_path, f.id, f.size_bytes, f.mtime_ns, COALESCE(p.fingerprint_version, -1)
         FROM files f LEFT JOIN fingerprints p ON p.file_id = f.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Known {
                id: row.get(1)?,
                size_bytes: row.get(2)?,
                mtime_ns: row.get(3)?,
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
    pub mtime_ns: i64,
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
        "INSERT INTO files(rel_path, size_bytes, mtime_ns, last_scanned_at)
         VALUES (?1, ?2, ?3, ?4)
         ON CONFLICT(rel_path) DO UPDATE SET
             size_bytes = excluded.size_bytes,
             mtime_ns = excluded.mtime_ns,
             last_scanned_at = excluded.last_scanned_at",
        params![record.rel_path, record.size_bytes, record.mtime_ns, scanned_at],
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
            mtime_ns: 999,
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
        assert_eq!(entry.mtime_ns, 999);
        assert_eq!(entry.fingerprint_version, fingerprint::FINGERPRINT_VERSION);
    }

    #[test]
    fn a_file_without_fingerprints_reads_as_stale() {
        let conn = memory_db();
        conn.execute(
            "INSERT INTO files(rel_path, size_bytes, mtime_ns, last_scanned_at)
             VALUES ('orphan.png', 1, 1, 1)",
            [],
        )
        .expect("insert");
        let known = load_known(&conn).expect("load");
        assert_eq!(known["orphan.png"].fingerprint_version, -1);
    }

    #[test]
    fn reopening_an_index_from_another_schema_version_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        {
            let conn = open(&path).expect("create");
            set_meta(&conn, "schema_version", "99").expect("bump");
        }
        let err = open(&path).expect_err("should refuse");
        assert!(err.to_string().contains("schema version 99"), "{err}");
    }

    fn fill(path: &Path, count: usize) {
        let mut conn = open(path).expect("create");
        let tx = conn.transaction().expect("begin");
        for index in 0..count {
            tx.execute(
                "INSERT INTO files(rel_path, size_bytes, mtime_ns, last_scanned_at)
                 VALUES (?1, 1, 1, 1)",
                [format!("a-fairly-long-path-name/{index}.jpeg")],
            )
            .expect("insert");
        }
        tx.commit().expect("commit");
    }

    fn count_files(path: &Path) -> i64 {
        open_read_only(path)
            .expect("open")
            .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
            .expect("count")
    }

    /// Deleting rows leaves their pages inside the file for the next write to
    /// use, so an index that is scanned and cleaned up over and over only grows.
    /// Compacting gives that space back, and everything still indexed stays.
    #[test]
    fn compacting_gives_back_the_space_of_deleted_rows_and_keeps_the_rest() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        fill(&path, 4000);
        compact(&path).expect("compact");
        let full = std::fs::metadata(&path).expect("stat").len();

        {
            let mut conn = open(&path).expect("open");
            let tx = conn.transaction().expect("begin");
            let paths: Vec<String> =
                (0..3000).map(|index| format!("a-fairly-long-path-name/{index}.jpeg")).collect();
            assert_eq!(delete_paths(&tx, &paths).expect("delete"), 3000);
            tx.commit().expect("commit");
        }
        compact(&path).expect("compact");

        let after = std::fs::metadata(&path).expect("stat").len();
        assert!(after < full, "the file did not shrink: {full} then {after}");
        assert_eq!(count_files(&path), 1000, "compacting took live rows with it");
    }

    /// The original is copied and the copy is cleaned. Nothing writes to the
    /// original, so a failure at any step leaves it exactly as it was.
    #[test]
    fn compacting_leaves_the_original_alone_when_it_cannot_finish() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        fill(&path, 100);
        let before = std::fs::read(&path).expect("read");

        // A directory where the copy has to be written.
        std::fs::create_dir(path.with_extension("compacting")).expect("mkdir");
        compact(&path).expect_err("should not have finished");

        assert_eq!(std::fs::read(&path).expect("read"), before, "the index was written to");
        assert_eq!(count_files(&path), 100);
    }

    #[test]
    fn compacting_clears_what_a_failed_attempt_left_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        fill(&path, 100);

        std::fs::write(path.with_extension("compacting"), b"not a database").expect("leftover");
        compact(&path).expect("compact");
        assert_eq!(count_files(&path), 100);
    }

    /// The rows written since the last checkpoint are in the log beside the
    /// index, not in the index. Compacting has to carry them over, and must not
    /// leave the old log where it would be applied to the file that replaces it.
    #[test]
    fn compacting_keeps_the_rows_that_are_still_only_in_the_write_ahead_log() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        fill(&path, 20);

        {
            let conn = open(&path).expect("open");
            conn.execute(
                "INSERT INTO files(rel_path, size_bytes, mtime_ns, last_scanned_at)
                 VALUES ('written-late.jpeg', 1, 1, 1)",
                [],
            )
            .expect("insert");
            let log = log_beside(&path);
            assert!(log.exists(), "the fixture never wrote a log");
            assert!(std::fs::metadata(&log).expect("stat").len() > 0);
        }

        compact(&path).expect("compact");

        assert!(!log_beside(&path).exists(), "the old log was left beside the new index");
        assert_eq!(count_files(&path), 21, "the rows in the log were lost");
    }

    /// An index that has been closed is one file. The write-ahead log and the
    /// shared-memory file beside it are what a database in WAL mode keeps while
    /// it is open, and both go when it is closed.
    #[test]
    fn closing_an_index_leaves_one_file_behind() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");

        let mut conn = open(&path).expect("open");
        let tx = conn.transaction().expect("tx");
        upsert(&tx, &record("a.jpg", 1), 1).expect("insert");
        tx.commit().expect("commit");
        assert!(log_beside(&path).exists(), "WAL mode wrote no log to close");

        close(conn, &path).expect("close");
        assert!(path.is_file());
        assert!(!log_beside(&path).exists(), "the write-ahead log was left behind");
        assert!(!index_beside(&path).exists(), "the shared-memory file was left behind");

        // And what was written is in the file it was closed into.
        let conn = open_read_only(&path).expect("reopen");
        let rows: i64 =
            conn.query_row("SELECT count(*) FROM files", [], |row| row.get(0)).expect("count");
        assert_eq!(rows, 1, "closing the index lost what it held");
    }

    #[test]
    fn a_fresh_index_records_its_schema_version() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("index.sqlite");
        let conn = open(&path).expect("create");
        assert_eq!(
            get_meta(&conn, "schema_version").expect("meta"),
            Some(SCHEMA_VERSION.to_string())
        );
    }
}
