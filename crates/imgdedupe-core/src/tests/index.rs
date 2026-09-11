use super::*;
use crate::db::Record;
use crate::fingerprint::{Fingerprint, HASH_BYTES, VARIANTS};

/// One picture's worth of index.
fn row(rel_path: &str, size_bytes: i64) -> Record {
    Record {
        rel_path: rel_path.to_string(),
        size_bytes,
        mtime_seconds: 1,
        width: 10,
        height: 10,
        format: crate::format::Format::Jpeg,
        channels: 3,
        fingerprint: Fingerprint {
            dct_hashes: [[0u8; HASH_BYTES]; VARIANTS],
            ring_stats: vec![0u8; 4],
        },
        corners: Vec::new(),
    }
}

/// The index as it is on disk. Nothing but this reads the file.
fn on_disk(path: &Path) -> Connection {
    db::open_and_migrate(path).expect("read the index file")
}

fn temp() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("imgdedupe.sqlite");
    (dir, path)
}

/// A read and a write are both answered, and what comes back is an answer
/// rather than a connection: the handle has no way of giving one out.
///
/// An ask made before any folder is held says so, rather than answering with
/// nothing, which is the empty database the old code substituted.
#[test]
fn nothing_but_the_manager_holds_the_index() {
    let (_dir, path) = temp();
    let index = Index::start();

    assert!(
        index.open_index_path().is_none(),
        "a manager that was told nothing holds something"
    );
    let too_soon = index.known().expect_err("a read before a folder was given");
    assert!(
        too_soon.to_string().contains("no folder is open"),
        "{too_soon}"
    );

    index.open(&path).expect("hold");
    assert_eq!(index.open_index_path().as_deref(), Some(path.as_path()));

    index.upsert(vec![row("a.jpg", 10)], 1).expect("a write");
    let known = index.known().expect("a read");
    assert_eq!(known.len(), 1, "the write was not there to be read back");
    assert!(known.contains_key("a.jpg"));
}

/// Letting go answers only once the file holds what the manager did.
#[test]
fn letting_go_of_a_folder_waits_for_the_file_to_catch_up() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("hold");
    index
        .upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1)
        .expect("write");
    index.set_meta("recurse", "1").expect("write");

    index.close().expect("let go");
    assert!(
        index.open_index_path().is_none(),
        "the folder was not let go of"
    );

    let conn = on_disk(&path);
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
        .expect("count");
    assert_eq!(
        rows, 2,
        "the file was behind what the manager had been holding"
    );
    assert_eq!(
        db::get_meta(&conn, "recurse").expect("meta").as_deref(),
        Some("1")
    );
}

/// A pass that indexed nothing still leaves the file in step.
///
/// This is what the old code got wrong: a pass that found no work skipped
/// writing the index out, and the migration made on the way in went with it,
/// so the next run migrated the same file again.
#[test]
fn a_scan_that_indexed_nothing_still_leaves_the_file_in_step() {
    let (_dir, path) = temp();

    // A folder whose index was made by a build that knew nothing of corners
    // and kept its stamps in nanoseconds.
    let older = rusqlite::Connection::open(&path).expect("make an older index");
    older
        .execute_batch(
            "CREATE TABLE files (
               id INTEGER PRIMARY KEY,
               rel_path TEXT NOT NULL UNIQUE,
               size_bytes INTEGER NOT NULL,
               mtime_ns INTEGER NOT NULL,
               last_scanned_at INTEGER NOT NULL
             );
             CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
             INSERT INTO files(rel_path, size_bytes, mtime_ns, last_scanned_at)
             VALUES ('a.jpg', 1, 1700000000123456789, 1);",
        )
        .expect("an older shape");
    drop(older);

    let index = Index::start();
    index.open(&path).expect("hold");
    // Nothing at all happens to it: no pass, no row, no setting.
    index.close().expect("let go");

    let conn = on_disk(&path);
    assert_eq!(
        db::get_meta(&conn, "schema_version")
            .expect("meta")
            .as_deref(),
        Some(db::SCHEMA_VERSION.to_string().as_str()),
        "the migration the manager made was thrown away"
    );
    let corners: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('fingerprints') WHERE name = 'corners'",
            [],
            |row| row.get(0),
        )
        .expect("looking for the column");
    assert_eq!(
        corners, 1,
        "the file was not left in the shape this build reads"
    );

    // And the other thing an index that old needs: the stamp carried into
    // whole seconds and the nanosecond column gone.
    let known = db::load_known(&conn).expect("read the file back");
    assert_eq!(
        known["a.jpg"].mtime_seconds, 1_700_000_000,
        "the stamp was not carried across"
    );
    let nanos: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('files') WHERE name = 'mtime_ns'",
            [],
            |row| row.get(0),
        )
        .expect("looking for the column");
    assert_eq!(nanos, 0, "the nanosecond column is still there");
}

/// One of every kind of change, then let go: all of them are in the file.
#[test]
fn every_change_reaches_the_file() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("hold");

    index
        .upsert(
            vec![row("a.jpg", 10), row("b.jpg", 20), row("gone.jpg", 30)],
            1,
        )
        .expect("upsert");
    index.set_meta("disposal", "delete").expect("set_meta");
    let ids: Vec<i64> = {
        let known = index.known().expect("known");
        let mut ids: Vec<i64> = ["a.jpg", "b.jpg"]
            .iter()
            .map(|path| known[*path].id)
            .collect();
        ids.sort();
        ids
    };
    index.ignore(&[db::pair(ids[0], ids[1])]).expect("ignore");
    assert_eq!(
        index
            .delete_paths(vec![String::from("gone.jpg")])
            .expect("delete"),
        1
    );
    index.compact().expect("compact");
    index.set_meta("to be forgotten", "here").expect("set_meta");
    index.forget_meta("to be forgotten").expect("forget_meta");

    index.close().expect("let go");

    let conn = on_disk(&path);
    let paths: Vec<String> = conn
        .prepare("SELECT rel_path FROM files ORDER BY rel_path")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .map(Result::unwrap)
        .collect();
    assert_eq!(
        paths,
        vec!["a.jpg".to_string(), "b.jpg".to_string()],
        "the rows are not right"
    );
    assert_eq!(
        db::get_meta(&conn, "disposal").expect("meta").as_deref(),
        Some("delete")
    );
    assert_eq!(db::get_meta(&conn, "to be forgotten").expect("meta"), None);
    assert_eq!(db::ignored(&conn).expect("ignored"), vec![(ids[0], ids[1])]);
}

/// A change is on the disk once the writing has been waited for, without the
/// index being closed first.
#[test]
fn a_change_is_in_the_file_once_the_writing_is_waited_for() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("open");

    index.set_meta("disposal", "delete").expect("set_meta");
    index.synced().expect("wait for the disk");

    let conn = rusqlite::Connection::open(&path).expect("read the file");
    assert_eq!(
        db::get_meta(&conn, "disposal").expect("meta").as_deref(),
        Some("delete")
    );
}

/// A change is written into the file. It used to be written beside it and
/// renamed over it, which left a different file each time.
#[test]
fn a_change_does_not_replace_the_file() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("open");
    index.synced().expect("wait for the disk");
    let was = std::fs::metadata(&path)
        .expect("the index")
        .created()
        .expect("when it was made");

    index.upsert(vec![row("a.jpg", 10)], 1).expect("upsert");
    index.set_meta("disposal", "delete").expect("set_meta");
    index.synced().expect("wait for the disk");

    let now = std::fs::metadata(&path)
        .expect("the index")
        .created()
        .expect("when it was made");
    assert_eq!(was, now, "the index was replaced rather than written to");
}

/// The rows that follow a file are the file's: they go when it does, on the
/// disk as well as in memory. This is what the writer's own connection needs
/// `foreign_keys` on for.
#[test]
fn deleting_a_file_takes_its_ignored_pairs_out_of_the_file() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("open");
    index
        .upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1)
        .expect("upsert");
    let ids: Vec<i64> = {
        let known = index.known().expect("known");
        let mut ids: Vec<i64> = ["a.jpg", "b.jpg"]
            .iter()
            .map(|path| known[*path].id)
            .collect();
        ids.sort();
        ids
    };
    index.ignore(&[db::pair(ids[0], ids[1])]).expect("ignore");
    index.synced().expect("wait for the disk");

    index
        .delete_paths(vec![String::from("a.jpg")])
        .expect("delete");
    index.synced().expect("wait for the disk");

    let conn = rusqlite::Connection::open(&path).expect("read the file");
    assert!(
        db::ignored(&conn).expect("ignored").is_empty(),
        "the pair outlived its picture in the file"
    );
}

/// A review is written down as it is made, so what one window wrote is what
/// the next one reading the file finds: the marks, and the sets they are
/// marks in.
#[test]
fn a_review_written_by_one_window_is_in_the_file_for_the_next() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("open");
    index
        .upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1)
        .expect("upsert");
    let known = index.known().expect("known");
    let (a, b) = (known["a.jpg"].id, known["b.jpg"].id);

    index.begin_review().expect("begin the review");
    index.keep_these(&[a]).expect("mark");
    index.store_sets(&[(a, vec![a, b])]).expect("store");
    index.synced().expect("wait for the disk");

    let conn = rusqlite::Connection::open(&path).expect("read the file");
    assert_eq!(
        db::kept(&conn).expect("marks"),
        vec![a],
        "the mark is not in the file"
    );
    assert_eq!(
        db::stored_sets(&conn).expect("sets"),
        vec![(a, vec![a, b])],
        "the set is not in the file"
    );
}

/// The end of a review takes it out of the file as well, both tables, so the
/// next window opens a folder nobody has reviewed.
#[test]
fn a_review_that_is_over_is_out_of_the_file_too() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("open");
    index
        .upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1)
        .expect("upsert");
    let known = index.known().expect("known");
    let (a, b) = (known["a.jpg"].id, known["b.jpg"].id);
    index.begin_review().expect("begin the review");
    index.keep_these(&[a]).expect("mark");
    index.store_sets(&[(a, vec![a, b])]).expect("store");
    index.synced().expect("wait for the disk");

    index.clear_keep().expect("clear the marks");
    index.clear_sets().expect("clear the sets");
    index.synced().expect("wait for the disk");

    let conn = rusqlite::Connection::open(&path).expect("read the file");
    assert!(
        db::kept(&conn).expect("marks").is_empty(),
        "the marks outlived the review"
    );
    assert!(
        db::stored_sets(&conn).expect("sets").is_empty(),
        "the sets outlived the review"
    );
}

/// A window writes a review before it has a folder open: a mark is made on
/// the sets in front of somebody, and the manager may be between folders. A
/// change the copy in memory could not make is not sent to the file, and the
/// writer is not left holding trouble to report.
#[test]
fn a_review_written_with_no_index_open_is_refused_and_leaves_the_writer_clean() {
    let index = Index::start();
    assert!(
        index.keep_these(&[1]).is_err(),
        "a mark was taken with no index open"
    );
    assert!(
        index.unkeep_these(&[1]).is_err(),
        "a mark was taken off the same way"
    );
    assert!(
        index.store_sets(&[(1, vec![1, 2])]).is_err(),
        "sets were taken the same way"
    );
    assert!(index.clear_keep().is_err());
    assert!(index.clear_sets().is_err());
    index
        .synced()
        .expect("the writer was handed work it had nowhere to do");
}

/// Tidying replaces the index file with the tidied database from memory, so
/// everything written before it has to be in that file afterwards: both what
/// had already reached the disk and what was still on its way there.
#[test]
fn tidying_the_index_keeps_everything_written_before_it() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("open");
    index
        .upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1)
        .expect("upsert");
    index.synced().expect("wait for the disk");
    let known = index.known().expect("known");
    let (a, b) = (known["a.jpg"].id, known["b.jpg"].id);
    index.begin_review().expect("begin the review");
    // Written and not waited for: this is still on the writer's queue when
    // the tidying starts.
    index.keep_these(&[a]).expect("mark");
    index.ignore(&[db::pair(a, b)]).expect("ignore");

    index.compact().expect("tidy the index");

    let conn = rusqlite::Connection::open(&path).expect("read the file");
    assert_eq!(
        db::kept(&conn).expect("marks"),
        vec![a],
        "a mark was lost by the tidying"
    );
    assert_eq!(
        db::ignored(&conn).expect("pairs"),
        vec![db::pair(a, b)],
        "an ignored pair was lost by the tidying"
    );
    assert_eq!(
        db::load_known(&conn).expect("known").len(),
        2,
        "the pictures were lost"
    );

    // And the index is still being written to afterwards.
    index.keep_these(&[b]).expect("mark again");
    index.synced().expect("wait for the disk");
    let conn = rusqlite::Connection::open(&path).expect("read the file");
    let mut held = db::kept(&conn).expect("marks");
    held.sort();
    assert_eq!(
        held,
        vec![a, b],
        "the file stopped taking changes after the tidying"
    );
}

/// Giving back the space of deleted rows is asked for to make the file
/// smaller, so it has to make the file smaller.
#[test]
fn compacting_makes_the_file_smaller() {
    let (_dir, path) = temp();
    let index = Index::start();
    index.open(&path).expect("open");
    let rows: Vec<Record> = (0..400)
        .map(|at| row(&format!("{at}.jpg"), at as i64))
        .collect();
    index.upsert(rows, 1).expect("upsert");
    index.synced().expect("wait for the disk");

    let paths: Vec<String> = (0..400).map(|at| format!("{at}.jpg")).collect();
    index.delete_paths(paths).expect("delete");
    index.synced().expect("wait for the disk");
    let full = std::fs::metadata(&path).expect("the index").len();

    index.compact().expect("compact");
    let after = std::fs::metadata(&path).expect("the index").len();
    assert!(
        after < full,
        "the file was {full} bytes and is {after} after compacting"
    );
}

/// The two ways an index cannot be read. Each is refused, each leaves the
/// file exactly as it was, and no index is left open.
#[test]
fn a_broken_index_stops_the_manager_and_writes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");

    // Not a database at all.
    let rubbish = dir.path().join("rubbish.sqlite");
    std::fs::write(&rubbish, b"not a database, just some bytes").expect("fixture");

    // A shape this build cannot bring to the current one: an index written
    // under another schema version.
    let ahead = dir.path().join("ahead.sqlite");
    drop(db::open_and_migrate(&ahead).expect("make one"));
    let conn = rusqlite::Connection::open(&ahead).expect("open it");
    db::set_meta(&conn, "schema_version", "99").expect("move it on");
    drop(conn);

    for (path, why) in [
        (&rubbish, "not a database"),
        (&ahead, "another schema version"),
    ] {
        let was = std::fs::read(path).expect("read what is there");
        let index = Index::start();
        let refused = index
            .open(path)
            .expect_err("this was not refused")
            .to_string();
        assert!(!refused.is_empty(), "{why} was refused without saying why");
        assert!(
            index.open_index_path().is_none(),
            "{why} left an index open"
        );
        assert_eq!(
            std::fs::read(path).expect("read"),
            was,
            "{why} was written over: {refused}"
        );
    }
}

/// Nothing outside the manager opens a database.
///
/// This is the rule all of this is for, and the only thing that keeps it true
/// once it is true. `db.rs` holds the one opener and `index.rs` is the
/// manager that calls it; a connection made anywhere else is a second owner
/// of the file.
#[test]
fn no_connection_is_made_outside_the_manager() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("the crates folder");
    let allowed = ["db.rs", "index.rs"];
    let ways = ["Connection::open", "open_in_memory", "open_with_flags"];

    let mut found = Vec::new();
    let mut looked_at = 0;
    for file in every_source_file(crates) {
        let name = file
            .file_name()
            .and_then(|it| it.to_str())
            .unwrap_or_default();
        if allowed.contains(&name) {
            continue;
        }
        // A test may forge a file on disk for the manager to open; nothing the
        // program does may open one. Tests live in a `tests` folder.
        if file.components().any(|part| part.as_os_str() == "tests") {
            continue;
        }
        let text = std::fs::read_to_string(&file).expect("read a source file");
        looked_at += 1;
        let program = match text.find("#[cfg(test)]") {
            Some(at) => &text[..at],
            None => &text[..],
        };
        for (number, line) in program.lines().enumerate() {
            if ways.iter().any(|way| line.contains(way)) {
                found.push(format!(
                    "{}:{}: {}",
                    file.display(),
                    number + 1,
                    line.trim()
                ));
            }
        }
    }
    assert!(
        looked_at > 10,
        "the source of the two crates was not found to read"
    );
    assert!(
        found.is_empty(),
        "a database is opened outside the manager:\n{}",
        found.join("\n")
    );
}

/// Every `.rs` file under the two crates.
fn every_source_file(from: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut queue = vec![from.to_path_buf()];
    while let Some(dir) = queue.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.filter_map(std::result::Result::ok) {
            let path = entry.path();
            if path.is_dir() {
                if path.file_name().is_some_and(|it| it == "target") {
                    continue;
                }
                queue.push(path);
            } else if path.extension().is_some_and(|it| it == "rs") {
                out.push(path);
            }
        }
    }
    out
}
