use super::*;

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
