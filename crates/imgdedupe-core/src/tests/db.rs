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
        mtime_seconds: 999,
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
    conn.pragma_update(None, "foreign_keys", "ON")
        .expect("pragma");
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
        conn.query_row("SELECT id FROM files WHERE rel_path = ?1", [path], |row| {
            row.get(0)
        })
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

    let files: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    let images: i64 = conn
        .query_row("SELECT count(*) FROM images", [], |r| r.get(0))
        .unwrap();
    let prints: i64 = conn
        .query_row("SELECT count(*) FROM fingerprints", [], |r| r.get(0))
        .unwrap();
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
    assert_eq!(
        delete_paths(&tx, &["a.jpg".to_string()]).expect("delete"),
        1
    );
    tx.commit().expect("commit");

    let files: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    let images: i64 = conn
        .query_row("SELECT count(*) FROM images", [], |r| r.get(0))
        .unwrap();
    let prints: i64 = conn
        .query_row("SELECT count(*) FROM fingerprints", [], |r| r.get(0))
        .unwrap();
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
    assert_eq!(entry.mtime_seconds, 999);
    assert_eq!(entry.fingerprint_version, fingerprint::FINGERPRINT_VERSION);
}

#[test]
fn a_file_without_fingerprints_reads_as_stale() {
    let conn = memory_db();
    conn.execute(
        "INSERT INTO files(rel_path, size_bytes, mtime_seconds, last_scanned_at)
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
    assert!(
        columns.iter().any(|name| name == "corners"),
        "not migrated: {columns:?}"
    );
    let rows: i64 = file
        .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
        .expect("count");
    assert_eq!(rows, 1, "the row was lost");
}

/// An index from a build that kept stamps in nanoseconds comes back holding
/// whole seconds, and the nanosecond column is gone.
#[test]
fn an_index_written_in_nanoseconds_comes_back_in_seconds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("index.sqlite");
    make_an_old_index(&path, "mtime_ns", 1_700_000_000_123_456_789);

    let conn = open_and_migrate(&path).expect("open");
    drop(conn);

    // The file itself, not the copy that was handed back.
    let file = Connection::open(&path).expect("open the file");
    let columns = column_names(&file, "files");
    assert!(
        columns.iter().any(|name| name == "mtime_seconds"),
        "no column: {columns:?}"
    );
    assert!(
        !columns.iter().any(|name| name == "mtime_ns"),
        "mtime_ns left: {columns:?}"
    );
    let stamp: i64 = file
        .query_row("SELECT mtime_seconds FROM files", [], |row| row.get(0))
        .expect("the row");
    assert_eq!(stamp, 1_700_000_000, "the stamp was not carried across");
    drop(file);

    let conn = open_and_migrate(&path).expect("reopen");
    let known = load_known(&conn).expect("load");
    assert_eq!(known["a.jpg"].mtime_seconds, 1_700_000_000);
}

/// An index from the build that kept stamps in milliseconds comes back
/// holding whole seconds, and the millisecond column is gone.
///
/// Milliseconds are as wrong as nanoseconds here: Windows writes the sub-second
/// part and macOS reports none, so every file read as changed on the machine
/// that did not write the index.
#[test]
fn an_index_written_in_milliseconds_comes_back_in_seconds() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("index.sqlite");
    make_an_old_index(&path, "mtime_ms", 1_700_000_000_810);

    let conn = open_and_migrate(&path).expect("open");
    let known = load_known(&conn).expect("load");
    assert_eq!(
        known["a.jpg"].mtime_seconds, 1_700_000_000,
        "the sub-second part was kept, so the file still reads as changed"
    );
    drop(conn);

    let file = Connection::open(&path).expect("open the file");
    let columns = column_names(&file, "files");
    assert!(
        !columns.iter().any(|name| name == "mtime_ms"),
        "mtime_ms left: {columns:?}"
    );
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
            "ALTER TABLE files ADD COLUMN mtime_seconds INTEGER NOT NULL DEFAULT 0;
             UPDATE files SET mtime_seconds = 1700000000;",
        )
        .expect("half of it");
    }

    let conn = open_and_migrate(&path).expect("open");
    let known = load_known(&conn).expect("load");
    assert_eq!(
        known["a.jpg"].mtime_seconds, 1_700_000_000,
        "the stamp was divided a second time"
    );
    drop(conn);

    let file = Connection::open(&path).expect("open the file");
    let columns = column_names(&file, "files");
    assert!(
        !columns.iter().any(|name| name == "mtime_ns"),
        "mtime_ns left: {columns:?}"
    );
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
        conn.execute_batch("ALTER TABLE files ADD COLUMN mtime_seconds INTEGER NOT NULL DEFAULT 0")
            .expect("the column and nothing else");
    }

    let conn = open_and_migrate(&path).expect("open");
    let known = load_known(&conn).expect("load");
    assert_eq!(
        known["a.jpg"].mtime_seconds, 1_700_000_000,
        "the stamp was left at nought"
    );
    drop(conn);

    let file = Connection::open(&path).expect("open the file");
    let columns = column_names(&file, "files");
    assert!(
        !columns.iter().any(|name| name == "mtime_ns"),
        "mtime_ns left: {columns:?}"
    );
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

/// A folder with no index gets one written, and opening that one again
/// leaves the file alone.
///
/// The file is what the folder is on the far side of a network mount, and
/// writing it back costs the whole index across it. An index already in the
/// current shape has nothing to carry across, so nothing is written.
#[test]
fn an_index_already_in_the_current_shape_is_not_written_again() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("index.sqlite");
    drop(open_and_migrate(&path).expect("create"));
    assert!(path.is_file(), "the index was never written");

    let written = std::fs::metadata(&path)
        .expect("the file")
        .modified()
        .expect("a stamp");
    // Coarse clocks: without this a second write inside the same tick reads
    // as no write at all.
    std::thread::sleep(std::time::Duration::from_millis(1100));

    drop(open_and_migrate(&path).expect("reopen"));
    let after = std::fs::metadata(&path)
        .expect("the file")
        .modified()
        .expect("a stamp");
    assert_eq!(
        written, after,
        "the index was written back with nothing to change"
    );
}

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
            "INSERT INTO files (id, rel_path, size_bytes, mtime_seconds, last_scanned_at)
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
            "INSERT INTO files (id, rel_path, size_bytes, mtime_seconds, last_scanned_at)
             VALUES (1, 'a.jpg', 1, 1, 1), (2, 'b.jpg', 1, 1, 1)",
        )
        .expect("files");
        ignore(&conn, &[(1, 2)]).expect("ignore");
        assert_eq!(ignored(&conn).expect("read").len(), 1);

        conn.execute_batch("DELETE FROM files WHERE id = 2")
            .expect("delete");
        assert!(
            ignored(&conn).expect("read").is_empty(),
            "the pair outlived its picture"
        );
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
mod looked_at {
    use super::*;

    fn looked(path: &str, size: i64, mtime: i64) -> Looked {
        Looked {
            rel_path: path.to_string(),
            size_bytes: size,
            mtime_seconds: mtime,
        }
    }

    /// A picture as the index holds one, so a file can be indexed and then not,
    /// and the other way about.
    fn a_record(path: &str, seed: u64, size: i64) -> Record {
        let mut hash = [0u8; fingerprint::HASH_BYTES];
        hash[..8].copy_from_slice(&seed.to_le_bytes());
        Record {
            rel_path: path.to_string(),
            size_bytes: size,
            mtime_seconds: 900,
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
        assert_eq!((entry.size_bytes, entry.mtime_seconds), (13, 700));
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
        assert!(
            !entry.not_a_picture,
            "the row still says it is not a picture"
        );
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
        assert_eq!(
            entry.fingerprint_version, -1,
            "the fingerprints outlived the picture"
        );
        let pictures: i64 = conn
            .query_row("SELECT count(*) FROM images", [], |row| row.get(0))
            .expect("count");
        assert_eq!(pictures, 0, "the index still says it is a picture");
    }
}

/// A review is written down as it happens, and taken away when it is over. Its
/// two tables are made where they are written, so a folder nobody has reviewed
/// has neither and reads as one nobody has reviewed.
mod reviewing {
    use super::*;

    fn three_files(conn: &Connection) {
        conn.execute_batch(
            "INSERT INTO files (id, rel_path, size_bytes, mtime_seconds, last_scanned_at)
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
        assert!(
            !table(&conn, "keep"),
            "a fresh index was given a marks table"
        );
        assert!(
            !table(&conn, "duplicate_sets"),
            "a fresh index was given a sets table"
        );
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
        assert!(
            table(&conn, "keep"),
            "a review began with nowhere to mark a picture"
        );
        assert!(
            table(&conn, "duplicate_sets"),
            "a review began with nowhere to put its sets"
        );
        assert!(
            kept(&conn).expect("read").is_empty(),
            "something was marked by nobody"
        );
        assert!(
            stored_sets(&conn).expect("read").is_empty(),
            "sets appeared out of nothing"
        );

        // And beginning one on a folder that already has one leaves what is there.
        keep_these(&conn, &[1]).expect("write");
        begin_review(&conn).expect("begin again");
        assert_eq!(
            kept(&conn).expect("read"),
            vec![1],
            "reopening a review lost its marks"
        );
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
        assert_eq!(
            held,
            vec![1, 2, 3],
            "marking one picture disturbed the others"
        );

        unkeep_these(&conn, &[1]).expect("write");
        let mut held = kept(&conn).expect("read");
        held.sort();
        assert_eq!(
            held,
            vec![2, 3],
            "unmarking one picture disturbed the others"
        );

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

        conn.execute("DELETE FROM files WHERE id = 1", [])
            .expect("delete");
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
        assert_eq!(
            stored_sets(&conn).expect("read"),
            vec![(3, vec![1, 3]), (2, vec![1, 2])]
        );
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

        conn.execute("DELETE FROM files WHERE id = 2", [])
            .expect("delete");
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
        assert!(
            !table(&conn, "duplicate_sets"),
            "the sets table outlived the review"
        );
        assert!(
            table(&conn, "ignore"),
            "the ignored pairs went with the review"
        );
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
