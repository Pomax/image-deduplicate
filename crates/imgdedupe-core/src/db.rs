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
/// A review is not in here. What it marks to keep and the sets it is a review of
/// are one sitting's worth of work: they are written when there is one and taken
/// away when it is over, so their tables are made where they are written and are
/// no part of what an index is. See `KEEP_TABLE` and `DUPLICATE_SETS_TABLE`.
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
    last_scanned_at INTEGER NOT NULL,
    not_a_picture   INTEGER NOT NULL DEFAULT 0
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
        ("files", "not_a_picture", "INTEGER NOT NULL DEFAULT 0"),
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
    /// The pass read this file and it is not a picture: not one of the formats,
    /// or animated, or it could not be read. There is nothing else in the index
    /// about it, and looking again would find what it found.
    pub not_a_picture: bool,
}

/// Every path the pass has been through, in one query. A row whose fingerprints
/// are missing reads as version -1 so it is always treated as stale.
pub fn load_known(conn: &Connection) -> Result<HashMap<String, Known>> {
    let mut statement = conn.prepare(
        "SELECT f.rel_path, f.id, f.size_bytes, f.mtime_ms, COALESCE(p.fingerprint_version, -1),
                f.not_a_picture
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
                not_a_picture: row.get::<_, i64>(5)? != 0,
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
    // A file that was not a picture and is one now loses the flag with the same
    // statement that records what it is.
    tx.execute(
        "INSERT INTO files(rel_path, size_bytes, mtime_ms, last_scanned_at, not_a_picture)
         VALUES (?1, ?2, ?3, ?4, 0)
         ON CONFLICT(rel_path) DO UPDATE SET
             size_bytes = excluded.size_bytes,
             mtime_ms = excluded.mtime_ms,
             last_scanned_at = excluded.last_scanned_at,
             not_a_picture = 0",
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

/// A review is starting: make the two tables it is written in.
///
/// Where the window goes to the review page and somebody can start marking
/// pictures, not where the first mark happens to be made. From that moment the
/// folder has a review, and a review with nothing marked in it is a review with
/// nothing marked, not a folder that has no such thing.
///
/// The pairs somebody said are not copies of each other are not made here. They
/// are not one sitting's work — they are a decision about the pictures — so they
/// are part of the index itself and a cleanup never touches them.
pub fn begin_review(conn: &Connection) -> Result<()> {
    conn.execute(DUPLICATE_SETS_TABLE, [])?;
    conn.execute(KEEP_TABLE, [])?;
    Ok(())
}

/// Write down that the pass read this file and it is not a picture.
///
/// The row says the path, the size and the timestamp and nothing else: whether it
/// was not one of the formats, or animated, or could not be read, the answer to
/// the next comparison is the same, and looking again would find what it found.
///
/// Whatever the index held about it as a picture goes, because it is not one. The
/// cascade takes those rows with the ones written here.
pub fn not_a_picture(tx: &Transaction<'_>, looked_at: &Looked, scanned_at: i64) -> Result<()> {
    tx.execute(
        "INSERT INTO files(rel_path, size_bytes, mtime_ms, last_scanned_at, not_a_picture)
         VALUES (?1, ?2, ?3, ?4, 1)
         ON CONFLICT(rel_path) DO UPDATE SET
             size_bytes = excluded.size_bytes,
             mtime_ms = excluded.mtime_ms,
             last_scanned_at = excluded.last_scanned_at,
             not_a_picture = 1",
        params![looked_at.rel_path, looked_at.size_bytes, looked_at.mtime_ms, scanned_at],
    )?;
    let file_id: i64 = tx.query_row(
        "SELECT id FROM files WHERE rel_path = ?1",
        params![looked_at.rel_path],
        |row| row.get(0),
    )?;
    tx.execute("DELETE FROM images WHERE file_id = ?1", params![file_id])?;
    tx.execute("DELETE FROM fingerprints WHERE file_id = ?1", params![file_id])?;
    Ok(())
}

/// A file the pass read and did not index, as the index records it.
#[derive(Debug, Clone)]
pub struct Looked {
    pub rel_path: String,
    pub size_bytes: i64,
    pub mtime_ms: i64,
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

/// The pictures a review marks to keep, made where they are first written.
///
/// The rows go when the files do, for the reason the ignored pairs' do: a mark
/// on a file that is gone is not a mark.
const KEEP_TABLE: &str = "CREATE TABLE IF NOT EXISTS keep (
    file_id INTEGER PRIMARY KEY REFERENCES files(id) ON DELETE CASCADE
)";

/// Mark these pictures to keep. What a person just did, and no more than that.
///
/// One statement per picture and nothing else touched. Marking one picture is one
/// row written: on a folder on another machine, every statement is a journal
/// written and deleted beside the index, so a write that redid the whole review
/// on every click cost as many of those as the review had marks.
pub fn keep_these(conn: &Connection, file_ids: &[i64]) -> Result<()> {
    if file_ids.is_empty() {
        return Ok(());
    }
    let mut statement =
        conn.prepare_cached("INSERT OR IGNORE INTO keep (file_id) VALUES (?1)")?;
    for file_id in file_ids {
        statement.execute(params![file_id])?;
    }
    Ok(())
}

/// Take the mark off these pictures. The other half of the same thing.
pub fn unkeep_these(conn: &Connection, file_ids: &[i64]) -> Result<()> {
    if file_ids.is_empty() {
        return Ok(());
    }
    let mut statement = conn.prepare_cached("DELETE FROM keep WHERE file_id = ?1")?;
    for file_id in file_ids {
        statement.execute(params![file_id])?;
    }
    Ok(())
}

/// Take the marks away. A review that has been carried out is over, and the
/// files it was about are not there any more.
pub fn clear_keep(conn: &Connection) -> Result<()> {
    conn.execute("DROP TABLE IF EXISTS keep", [])?;
    Ok(())
}

/// Every picture marked to keep.
///
/// No table to read is not a failure: it is a folder nobody has reviewed yet.
pub fn kept(conn: &Connection) -> Result<Vec<i64>> {
    let table = conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'keep'");
    let known = match table {
        Ok(mut statement) => statement.exists([])?,
        Err(_) => false,
    };
    if !known {
        return Ok(Vec::new());
    }
    let mut statement = conn.prepare("SELECT file_id FROM keep")?;
    let rows = statement.query_map([], |row| row.get(0))?;
    Ok(rows.filter_map(Result::ok).collect())
}

/// The sets a search found, made where they are first written.
///
/// A row per picture per set. `at` is the set's place in the list the review
/// shows: read back in another order it is another list, and not where the
/// person left off. The pictures inside a set need no such column, because they
/// are ordered by timestamp and sorting them again gives the same order. The
/// rows go when the files do, because a set that has lost a picture is not that
/// set.
const DUPLICATE_SETS_TABLE: &str = "CREATE TABLE IF NOT EXISTS duplicate_sets (
    set_id  INTEGER NOT NULL,
    file_id INTEGER NOT NULL REFERENCES files(id) ON DELETE CASCADE,
    at      INTEGER NOT NULL,
    PRIMARY KEY (set_id, file_id)
)";

/// Write down the sets a search found, in the order the review shows them.
///
/// All of them, every time, for the reason the marks are written that way: this
/// is what the search said, and there is no such thing as half of it.
pub fn store_sets(conn: &Connection, sets: &[(i64, Vec<i64>)]) -> Result<()> {
    conn.execute("DELETE FROM duplicate_sets", [])?;
    let mut statement = conn.prepare_cached(
        "INSERT OR IGNORE INTO duplicate_sets (set_id, file_id, at) VALUES (?1, ?2, ?3)",
    )?;
    for (at, (set_id, file_ids)) in sets.iter().enumerate() {
        for file_id in file_ids {
            statement.execute(params![set_id, file_id, at as i64])?;
        }
    }
    Ok(())
}

/// The sets a search found, in the order they were shown in.
///
/// An index written before there was such a thing has no table to read, which is
/// not a failure: it is a folder that has never been searched.
pub fn stored_sets(conn: &Connection) -> Result<Vec<(i64, Vec<i64>)>> {
    let table = conn
        .prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'duplicate_sets'");
    let known = match table {
        Ok(mut statement) => statement.exists([])?,
        Err(_) => false,
    };
    if !known {
        return Ok(Vec::new());
    }
    let mut statement =
        conn.prepare("SELECT set_id, file_id FROM duplicate_sets ORDER BY at, set_id, file_id")?;
    let rows = statement.query_map([], |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)))?;
    let mut sets: Vec<(i64, Vec<i64>)> = Vec::new();
    for (set_id, file_id) in rows.filter_map(Result::ok) {
        match sets.last_mut() {
            Some((last, members)) if *last == set_id => members.push(file_id),
            _ => sets.push((set_id, vec![file_id])),
        }
    }
    Ok(sets)
}

/// Take the stored sets away. No table is a folder that has never been searched,
/// and the next search that finds any makes it again.
pub fn clear_sets(conn: &Connection) -> Result<()> {
    conn.execute("DROP TABLE IF EXISTS duplicate_sets", [])?;
    Ok(())
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

/// What the pass looked at and did not index. The row says the file has been
/// read and there is nothing in the index about it as a picture, which is what
/// stops the next pass reading it and the next comparison calling it new.
#[cfg(test)]
mod looked_at {
    use super::*;

    fn looked(path: &str, size: i64, mtime: i64) -> Looked {
        Looked { rel_path: path.to_string(), size_bytes: size, mtime_ms: mtime }
    }

    /// A picture as the index holds one, so a file can be indexed and then not,
    /// and the other way about.
    fn a_record(path: &str, seed: u64, size: i64) -> Record {
        let mut hash = [0u8; fingerprint::HASH_BYTES];
        hash[..8].copy_from_slice(&seed.to_le_bytes());
        Record {
            rel_path: path.to_string(),
            size_bytes: size,
            mtime_ms: 900,
            width: 800,
            height: 600,
            format: Format::Jpeg,
            channels: 3,
            fingerprint: crate::fingerprint::Fingerprint {
                dct_hashes: [hash, hash, hash, hash, hash, hash, hash, hash],
                ring_stats: vec![1, 2, 3, 4],
            },
            corners: Vec::new(),
        }
    }

    /// The row comes back saying what it is: known, at that size and timestamp,
    /// and not a picture.
    #[test]
    fn a_file_that_is_not_a_picture_is_written_down_as_looked_at() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        let tx = conn.transaction().expect("transaction");
        not_a_picture(&tx, &looked("notes.png", 13, 700), 1).expect("write");
        tx.commit().expect("commit");

        let known = load_known(&conn).expect("read");
        let entry = known.get("notes.png").expect("the row");
        assert!(entry.not_a_picture, "the row does not say it was looked at");
        assert_eq!((entry.size_bytes, entry.mtime_ms), (13, 700));
    }

    /// A file that was not a picture and is one now is indexed in the ordinary
    /// way, and the row stops saying otherwise.
    #[test]
    fn a_file_that_became_a_picture_stops_being_marked_as_not_one() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        let tx = conn.transaction().expect("transaction");
        not_a_picture(&tx, &looked("a.png", 13, 700), 1).expect("write");
        tx.commit().expect("commit");

        let record = a_record("a.png", 0x1234, 900);
        let tx = conn.transaction().expect("transaction");
        upsert(&tx, &record, 2).expect("upsert");
        tx.commit().expect("commit");

        let known = load_known(&conn).expect("read");
        let entry = known.get("a.png").expect("the row");
        assert!(!entry.not_a_picture, "the row still says it is not a picture");
        assert_eq!(entry.fingerprint_version, fingerprint::FINGERPRINT_VERSION);
    }

    /// And the other way: a picture replaced by something that is not one loses
    /// what the index held about it as a picture.
    #[test]
    fn a_picture_that_stopped_being_one_loses_what_was_held_about_it() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        let record = a_record("a.png", 0x1234, 900);
        let tx = conn.transaction().expect("transaction");
        upsert(&tx, &record, 1).expect("upsert");
        tx.commit().expect("commit");

        let tx = conn.transaction().expect("transaction");
        not_a_picture(&tx, &looked("a.png", 13, 700), 2).expect("write");
        tx.commit().expect("commit");

        let known = load_known(&conn).expect("read");
        let entry = known.get("a.png").expect("the row");
        assert!(entry.not_a_picture);
        assert_eq!(entry.fingerprint_version, -1, "the fingerprints outlived the picture");
        let pictures: i64 = conn
            .query_row("SELECT count(*) FROM images", [], |row| row.get(0))
            .expect("count");
        assert_eq!(pictures, 0, "the index still says it is a picture");
    }
}

/// A review is written down as it happens, and taken away when it is over. Its
/// two tables are made where they are written, so a folder nobody has reviewed
/// has neither and reads as one nobody has reviewed.
#[cfg(test)]
mod reviewing {
    use super::*;

    fn three_files(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO files (id, rel_path, size_bytes, mtime_ms, last_scanned_at)
             VALUES (1, 'a.jpg', 1, 1, 1), (2, 'b.jpg', 1, 1, 1), (3, 'c.jpg', 1, 1, 1)",
        )
        .expect("files");
    }

    fn table(conn: &Connection, name: &str) -> bool {
        conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1")
            .expect("ask")
            .exists([name])
            .expect("ask")
    }

    /// A fresh index holds neither table. Reading a review out of one is not a
    /// failure: it is a folder nobody has reviewed.
    #[test]
    fn an_index_nobody_has_reviewed_has_no_marks_and_no_sets() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        assert!(!table(&conn, "keep"), "a fresh index was given a marks table");
        assert!(!table(&conn, "duplicate_sets"), "a fresh index was given a sets table");
        assert!(kept(&conn).expect("read").is_empty());
        assert!(stored_sets(&conn).expect("read").is_empty());
    }

    /// A review is both tables, and they are made where a review begins: sets
    /// being built into the review page, which is the one way anybody reaches it.
    /// Not by the first mark, so a review nobody has marked anything in yet is a
    /// review with nothing marked and not a folder that has no such thing.
    #[test]
    fn a_review_beginning_makes_the_tables_it_is_written_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        three_files(&conn);

        begin_review(&conn).expect("begin");
        assert!(table(&conn, "keep"), "a review began with nowhere to mark a picture");
        assert!(table(&conn, "duplicate_sets"), "a review began with nowhere to put its sets");
        assert!(kept(&conn).expect("read").is_empty(), "something was marked by nobody");
        assert!(stored_sets(&conn).expect("read").is_empty(), "sets appeared out of nothing");

        // And beginning one on a folder that already has one leaves what is there.
        keep_these(&conn, &[1]).expect("write");
        begin_review(&conn).expect("begin again");
        assert_eq!(kept(&conn).expect("read"), vec![1], "reopening a review lost its marks");
    }

    /// A mark going on is one row written and a mark coming off is one row
    /// deleted. Nothing else in the review is touched, which is what keeps a
    /// click on a picture to a single statement.
    #[test]
    fn a_mark_written_is_a_mark_read_back_and_the_rest_are_left_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        three_files(&conn);
        begin_review(&conn).expect("begin");

        keep_these(&conn, &[1, 3]).expect("write");
        let mut held = kept(&conn).expect("read");
        held.sort();
        assert_eq!(held, vec![1, 3]);

        keep_these(&conn, &[2]).expect("write");
        let mut held = kept(&conn).expect("read");
        held.sort();
        assert_eq!(held, vec![1, 2, 3], "marking one picture disturbed the others");

        unkeep_these(&conn, &[1]).expect("write");
        let mut held = kept(&conn).expect("read");
        held.sort();
        assert_eq!(held, vec![2, 3], "unmarking one picture disturbed the others");

        // Saying it twice says it once, and taking off what is not on is nothing.
        keep_these(&conn, &[2]).expect("write");
        unkeep_these(&conn, &[1]).expect("write");
        let mut held = kept(&conn).expect("read");
        held.sort();
        assert_eq!(held, vec![2, 3]);
    }

    /// A mark on a file that is gone is not a mark, so the rows go when the
    /// files do.
    #[test]
    fn a_mark_goes_when_its_picture_does() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        three_files(&conn);
        begin_review(&conn).expect("begin");
        keep_these(&conn, &[1, 2]).expect("write");

        conn.execute("DELETE FROM files WHERE id = 1", []).expect("delete");
        assert_eq!(kept(&conn).expect("read"), vec![2]);
    }

    /// The review is a list and somebody left off partway down it, so the sets
    /// come back in the order they were shown in.
    #[test]
    fn the_sets_come_back_in_the_order_they_were_stored_in() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        three_files(&conn);
        begin_review(&conn).expect("begin");

        store_sets(&conn, &[(3, vec![3, 1]), (2, vec![2, 1])]).expect("write");
        assert_eq!(stored_sets(&conn).expect("read"), vec![(3, vec![1, 3]), (2, vec![1, 2])]);
    }

    /// A set that has lost a picture is not that set, so its rows go with the
    /// file. What is left of it is what comes back.
    #[test]
    fn a_stored_set_loses_the_pictures_its_files_lost() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        three_files(&conn);
        begin_review(&conn).expect("begin");
        store_sets(&conn, &[(1, vec![1, 2, 3])]).expect("write");

        conn.execute("DELETE FROM files WHERE id = 2", []).expect("delete");
        assert_eq!(stored_sets(&conn).expect("read"), vec![(1, vec![1, 3])]);
    }

    /// The end of a review takes the review away and leaves everything that is
    /// not the review alone.
    ///
    /// The pairs somebody said are not copies of each other are not part of one
    /// sitting: they are a decision about those pictures that holds for as long
    /// as the pictures do. A cleanup drops the marks and the sets and never
    /// touches them.
    #[test]
    fn a_cleanup_leaves_the_ignored_pairs_where_they_are() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        three_files(&conn);
        ignore(&conn, &[(1, 2)]).expect("ignore");
        begin_review(&conn).expect("begin");
        keep_these(&conn, &[1]).expect("write");
        store_sets(&conn, &[(1, vec![1, 2])]).expect("write");

        clear_keep(&conn).expect("clear the marks");
        clear_sets(&conn).expect("clear the sets");

        assert!(!table(&conn, "keep"), "the marks table outlived the review");
        assert!(!table(&conn, "duplicate_sets"), "the sets table outlived the review");
        assert!(table(&conn, "ignore"), "the ignored pairs went with the review");
        assert_eq!(
            ignored(&conn).expect("read"),
            vec![(1, 2)],
            "a pair somebody said is not a pair of copies was forgotten by a cleanup"
        );
    }

    /// The end of a review takes both tables away, and the next review makes
    /// them again.
    #[test]
    fn a_review_that_is_over_leaves_neither_table() {
        let dir = tempfile::tempdir().expect("tempdir");
        let conn = open_and_migrate(&dir.path().join("index.sqlite")).expect("open");
        three_files(&conn);
        begin_review(&conn).expect("begin");
        keep_these(&conn, &[1]).expect("write");
        store_sets(&conn, &[(1, vec![1, 2])]).expect("write");

        clear_keep(&conn).expect("clear");
        clear_sets(&conn).expect("clear");
        assert!(kept(&conn).expect("read").is_empty());
        assert!(stored_sets(&conn).expect("read").is_empty());

        // And the next review makes them again.
        begin_review(&conn).expect("begin");
        keep_these(&conn, &[2]).expect("write");
        store_sets(&conn, &[(2, vec![2, 3])]).expect("write");
        assert_eq!(kept(&conn).expect("read"), vec![2]);
        assert_eq!(stored_sets(&conn).expect("read"), vec![(2, vec![2, 3])]);
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
