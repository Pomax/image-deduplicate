use super::*;
use imgdedupe_core::matching::Member;

fn member(id: i64, path: &str, auto_keep: bool, size: i64) -> Member {
    Member {
        file_id: id,
        rel_path: path.to_string(),
        width: 800,
        height: 600,
        format: "jpeg".to_string(),
        channels: 3,
        size_bytes: size,
        mtime_seconds: 1,
        auto_keep,
    }
}

fn sets() -> Vec<DuplicateSet> {
    vec![DuplicateSet {
        set_id: 1,
        members: vec![
            member(1, "big.jpg", true, 500),
            member(2, "small, odd.jpg", false, 100),
        ],
    }]
}

#[test]
fn the_json_report_names_the_keeper() {
    let text = report_json(&sets());
    let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
    assert_eq!(parsed[0]["recoverable_bytes"], 100);
    assert_eq!(parsed[0]["members"][0]["keep"], true);
    assert_eq!(parsed[0]["members"][1]["keep"], false);
}

#[test]
fn the_csv_report_has_a_header_and_quotes_commas() {
    let text = report_csv(&sets());
    let lines: Vec<&str> = text.lines().collect();
    assert!(lines[0].starts_with("set_id,keep,path"));
    assert!(lines[1].contains("keep,big.jpg"));
    assert!(lines[2].contains("\"small, odd.jpg\""), "{}", lines[2]);
}

#[test]
fn the_plan_covers_everything_but_the_keeper() {
    let plan = plan_from(&sets());
    assert_eq!(plan.files(), 1);
    assert_eq!(plan.bytes(), 100);
    assert!(describe(&plan).contains("small, odd.jpg"));
}

/// The removed files are gone from the index because they are gone from those
/// paths. The one that was kept is still there and must still be indexed, or
/// the next pass has to read it again for no reason.
#[test]
fn cleaning_up_forgets_the_removed_files_and_only_those() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("index.sqlite");
    let index = imgdedupe_core::index::Index::start();
    index.open(&db_path).expect("an index");
    for path in ["big.jpg", "small, odd.jpg", "elsewhere.jpg"] {
        index.upsert(vec![row(path)], 1).expect("insert");
    }

    let dropped = forget(&index, &[String::from("small, odd.jpg")]).expect("forget");
    assert_eq!(dropped, 1);

    // Read the file, not what the manager holds: the rows have to be gone
    // from the folder's index, not only from this run.
    index.synced().expect("wait for the file");
    let conn = imgdedupe_core::db::open_and_migrate(&db_path).expect("reopen");
    let mut left: Vec<String> = conn
        .prepare("SELECT rel_path FROM files")
        .expect("prepare")
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<Result<_, _>>()
        .expect("rows");
    left.sort();
    assert_eq!(
        left,
        vec!["big.jpg".to_string(), "elsewhere.jpg".to_string()]
    );
}

/// One picture's worth of index, enough to have a path in the folder.
fn row(rel_path: &str) -> imgdedupe_core::db::Record {
    use imgdedupe_core::fingerprint::{Fingerprint, HASH_BYTES, VARIANTS};
    imgdedupe_core::db::Record {
        rel_path: rel_path.to_string(),
        size_bytes: 1,
        mtime_seconds: 1,
        width: 10,
        height: 10,
        format: imgdedupe_core::format::Format::Jpeg,
        channels: 3,
        fingerprint: Fingerprint {
            dct_hashes: [[0u8; HASH_BYTES]; VARIANTS],
            ring_stats: vec![0u8; 4],
        },
        corners: Vec::new(),
    }
}

#[test]
fn a_cleanup_that_removed_nothing_touches_no_rows() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("index.sqlite");
    let index = imgdedupe_core::index::Index::start();
    index.open(&db_path).expect("an index");
    assert_eq!(forget(&index, &[]).expect("forget"), 0);
}

/// Neither flag means the window opens, and no flag is the help flag.
#[test]
fn no_flags_asks_for_neither_report_nor_clean() {
    let args = Args::try_parse_from(["imgdedupe"]).expect("parse");
    assert!(args.report.is_none());
    assert!(args.clean.is_none());
    assert!(Args::try_parse_from(["imgdedupe", "--help"]).is_err());
}

#[test]
fn report_and_clean_each_take_the_folder_and_cannot_be_combined() {
    let args = Args::try_parse_from(["imgdedupe", "--report", "/photos"]).expect("parse");
    assert_eq!(args.report.as_deref(), Some(Path::new("/photos")));

    let args = Args::try_parse_from(["imgdedupe", "--clean", "/photos"]).expect("parse");
    assert_eq!(args.clean.as_deref(), Some(Path::new("/photos")));

    assert!(
        Args::try_parse_from(["imgdedupe", "--report", "/a", "--clean", "/a"]).is_err(),
        "asking for both at once has no meaning"
    );
}
