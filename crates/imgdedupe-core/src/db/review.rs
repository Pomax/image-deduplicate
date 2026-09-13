use super::*;

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
pub(super) const KEEP_TABLE: &str = "CREATE TABLE IF NOT EXISTS keep (
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
pub(super) const DUPLICATE_SETS_TABLE: &str = "CREATE TABLE IF NOT EXISTS duplicate_sets (
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
