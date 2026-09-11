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
/// `files.mtime_seconds` and `files.last_scanned_at` are both whole seconds
/// since the epoch. Whole seconds because that is the finest a network mount
/// reports the same way on every platform: Windows hands back the file system's
/// own sub-second ticks and macOS hands back none, so anything finer makes an
/// index written on one machine read as changed on the other.
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
    mtime_seconds   INTEGER NOT NULL,
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
SELECT f.id, f.rel_path, f.size_bytes, f.mtime_seconds,
       i.width, i.height, i.format, i.channels,
       p.dct_hashes, p.ring_stats, p.corners
FROM files f
JOIN images i       ON i.file_id = f.id
JOIN fingerprints p ON p.file_id = f.id;
";

/// Open a folder's index: read it in, bring the copy to the current shape, and
/// put it back only if that changed anything.
///
/// Only the manager calls this, and nothing else opens the index.
///
/// The copy is what is migrated, not the file. Every statement against a
/// database on another machine is a round trip, and `DROP COLUMN` has SQLite
/// rewrite the whole table, which on a hundred-megabyte index across a mount is
/// a quarter of a minute. In memory those statements are free, and what crosses
/// the network is one sequential read and, when there was something to change,
/// one finished file written once. Nothing can be handed a connection in an
/// older shape either way: the copy is brought up to date before it is returned.
pub fn open_and_migrate(path: &Path) -> Result<Connection> {
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    // A folder with no index yet starts from an empty database, which the
    // migration then gives the current shape and which is written out below.
    let bytes = std::fs::read(path).unwrap_or_default();
    crate::log_line!(
        "    read {} bytes of index: {:.2}s",
        bytes.len(),
        at.elapsed().as_secs_f64()
    );

    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let conn = if bytes.is_empty() {
        Connection::open_in_memory().context("opening an index in memory")?
    } else {
        into_memory(bytes, path)?
    };
    conn.pragma_update(None, "foreign_keys", "ON")?;
    crate::log_line!("    hand it to sqlite: {:.2}s", at.elapsed().as_secs_f64());

    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let changed = migrate_the_copy(&conn)?;
    crate::log_line!("    migrate the copy: {:.2}s", at.elapsed().as_secs_f64());

    if changed {
        #[cfg(feature = "logging")]
        let at = std::time::Instant::now();
        put_the_index_back(&conn, path)?;
        crate::log_line!("    write it back: {:.2}s", at.elapsed().as_secs_f64());
    }
    Ok(conn)
}

/// Write the whole index over the folder's file, in one copy.
///
/// Written out by SQLite to this machine's own disk first. Done straight onto
/// the folder's file it would be the whole index across the network in page-sized
/// writes with a journal beside it.
fn put_the_index_back(conn: &Connection, path: &Path) -> Result<()> {
    static MIGRATIONS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let local = std::env::temp_dir().join(format!(
        "imgdedupe-migrating-{}-{}.sqlite",
        std::process::id(),
        MIGRATIONS.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&local);
    conn.execute("VACUUM INTO ?1", [local.to_string_lossy().as_ref()])
        .with_context(|| format!("writing the migrated index to {}", local.display()))?;
    // If this fails the folder still has the index it had.
    let put_back = std::fs::copy(&local, path)
        .with_context(|| format!("putting the migrated index back at {}", path.display()));
    let _ = std::fs::remove_file(&local);
    put_back?;
    Ok(())
}

/// Bring the copy to the current shape: the schema, the columns an older build
/// lacks, and the version it is written under. Says whether anything changed,
/// which is what decides if the folder's file has to be written again.
///
/// This happens before anything reads a row.
fn migrate_the_copy(conn: &Connection) -> Result<bool> {
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let fresh = !has_table(conn, "files")?;
    conn.execute_batch(SCHEMA).context("applying the schema")?;
    crate::log_line!("      apply the schema: {:.2}s", at.elapsed().as_secs_f64());
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let added = add_new_columns(conn)?;
    let carried = carry_the_stamps_across(conn)?;
    let dropped = drop_dead_columns(conn)?;
    crate::log_line!("      the columns: {:.2}s", at.elapsed().as_secs_f64());

    let existing: Option<i64> = conn
        .query_row(
            "SELECT value FROM meta WHERE key = 'schema_version'",
            [],
            |row| row.get::<_, String>(0),
        )
        .ok()
        .and_then(|value| value.parse().ok());
    let stamped = match existing {
        Some(version) if version != SCHEMA_VERSION => {
            anyhow::bail!(
                "index was written by schema version {version}, this build speaks {SCHEMA_VERSION}"
            );
        }
        Some(_) => false,
        None => {
            set_meta(conn, "schema_version", &SCHEMA_VERSION.to_string())?;
            true
        }
    };
    Ok(fresh || added || carried || dropped || stamped)
}

/// Whether the database has a table.
fn has_table(conn: &Connection, table: &str) -> Result<bool> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        params![table],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

/// Where the index is written before it is moved onto itself.
/// Take the columns nothing reads out of an index written by an older build.
///
/// A file already on disk keeps whatever columns it was made with, and the ones
/// that are `NOT NULL` would refuse every insert that no longer names them.
fn drop_dead_columns(conn: &Connection) -> Result<bool> {
    let dead = [
        ("files", "bytes_hash"),
        ("fingerprints", "dct_hash"),
        ("files", "mtime_ns"),
        ("files", "mtime_ms"),
    ];
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
        return Ok(false);
    }
    // The view names them, and a column a view reads cannot be dropped.
    conn.execute_batch("DROP VIEW IF EXISTS indexed_images")?;
    conn.execute_batch("DROP INDEX IF EXISTS files_bytes_hash")?;
    for (table, column) in found {
        conn.execute_batch(&format!("ALTER TABLE {table} DROP COLUMN {column}"))
            .with_context(|| format!("dropping {table}.{column}"))?;
    }
    conn.execute_batch(SCHEMA)
        .context("rebuilding the schema")?;
    Ok(true)
}

/// Add the columns a newer build writes to an index made by an older one.
///
/// `CREATE TABLE IF NOT EXISTS` leaves a table that already exists exactly as it
/// was, so a file from an older build keeps the shape it was made with. The rows
/// in it are re-fingerprinted anyway, because the fingerprint version moved, and
/// they need somewhere to be written to.
fn add_new_columns(conn: &Connection) -> Result<bool> {
    let wanted = [
        ("fingerprints", "corners", "BLOB NOT NULL DEFAULT x''"),
        ("files", "mtime_seconds", "INTEGER NOT NULL DEFAULT 0"),
        ("files", "not_a_picture", "INTEGER NOT NULL DEFAULT 0"),
    ];
    let mut added = false;
    for (table, column, declaration) in wanted {
        if has_column(conn, table, column)? {
            continue;
        }
        conn.execute_batch(&format!(
            "ALTER TABLE {table} ADD COLUMN {column} {declaration}"
        ))
        .with_context(|| format!("adding {table}.{column}"))?;
        added = true;
    }
    if !added {
        return Ok(false);
    }
    // The view was made without them and would go on reading the old shape.
    conn.execute_batch("DROP VIEW IF EXISTS indexed_images")?;
    conn.execute_batch(SCHEMA)
        .context("rebuilding the schema")?;
    Ok(true)
}

/// Fill `files.mtime_seconds` from the nanoseconds or milliseconds an older
/// build wrote. Runs between the column being added and the old ones being
/// dropped.
fn carry_the_stamps_across(conn: &Connection) -> Result<bool> {
    let mut carried = false;
    if has_column(conn, "files", "mtime_ns")? {
        conn.execute_batch("UPDATE files SET mtime_seconds = mtime_ns / 1000000000")
            .context("carrying the modification times across from nanoseconds")?;
        carried = true;
    }
    if has_column(conn, "files", "mtime_ms")? {
        conn.execute_batch("UPDATE files SET mtime_seconds = mtime_ms / 1000")
            .context("carrying the modification times across from milliseconds")?;
        carried = true;
    }
    Ok(carried)
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
    pub mtime_seconds: i64,
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
        "SELECT f.rel_path, f.id, f.size_bytes, f.mtime_seconds, COALESCE(p.fingerprint_version, -1),
                f.not_a_picture
         FROM files f LEFT JOIN fingerprints p ON p.file_id = f.id",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            Known {
                id: row.get(1)?,
                size_bytes: row.get(2)?,
                mtime_seconds: row.get(3)?,
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
    pub mtime_seconds: i64,
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
        "INSERT INTO files(rel_path, size_bytes, mtime_seconds, last_scanned_at, not_a_picture)
         VALUES (?1, ?2, ?3, ?4, 0)
         ON CONFLICT(rel_path) DO UPDATE SET
             size_bytes = excluded.size_bytes,
             mtime_seconds = excluded.mtime_seconds,
             last_scanned_at = excluded.last_scanned_at,
             not_a_picture = 0",
        params![
            record.rel_path,
            record.size_bytes,
            record.mtime_seconds,
            scanned_at
        ],
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
/// are not one sitting's work, but a decision about the pictures, so they
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
        "INSERT INTO files(rel_path, size_bytes, mtime_seconds, last_scanned_at, not_a_picture)
         VALUES (?1, ?2, ?3, ?4, 1)
         ON CONFLICT(rel_path) DO UPDATE SET
             size_bytes = excluded.size_bytes,
             mtime_seconds = excluded.mtime_seconds,
             last_scanned_at = excluded.last_scanned_at,
             not_a_picture = 1",
        params![
            looked_at.rel_path,
            looked_at.size_bytes,
            looked_at.mtime_seconds,
            scanned_at
        ],
    )?;
    let file_id: i64 = tx.query_row(
        "SELECT id FROM files WHERE rel_path = ?1",
        params![looked_at.rel_path],
        |row| row.get(0),
    )?;
    tx.execute("DELETE FROM images WHERE file_id = ?1", params![file_id])?;
    tx.execute(
        "DELETE FROM fingerprints WHERE file_id = ?1",
        params![file_id],
    )?;
    Ok(())
}

/// A file the pass read and did not index, as the index records it.
#[derive(Debug, Clone)]
pub struct Looked {
    pub rel_path: String,
    pub size_bytes: i64,
    pub mtime_seconds: i64,
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
    let table =
        conn.prepare("SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = 'ignore'");
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
    let mut statement = conn.prepare_cached("INSERT OR IGNORE INTO keep (file_id) VALUES (?1)")?;
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
#[path = "tests/db.rs"]
mod tests;
