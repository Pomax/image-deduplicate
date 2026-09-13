use super::*;

/// Bring the copy to the current shape: the schema, the columns an older build
/// lacks, and the version it is written under. Says whether anything changed,
/// which is what decides if the folder's file has to be written again.
///
/// This happens before anything reads a row.
pub(super) fn migrate_the_copy(conn: &Connection) -> Result<bool> {
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
