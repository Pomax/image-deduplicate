use std::collections::HashMap;
use std::path::Path;

use anyhow::{Context, Result};
use rusqlite::{params, Transaction};

/// Re-exported so nothing above this layer needs its own SQLite dependency.
pub use rusqlite::Connection;

use crate::fingerprint::{self, Fingerprint};
use crate::format::Format;

mod files;
mod meta;
mod migrate;
mod review;

pub use self::files::*;
pub use self::meta::*;
use self::migrate::*;
pub use self::review::*;

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

#[cfg(test)]
#[path = "../tests/db.rs"]
mod tests;
