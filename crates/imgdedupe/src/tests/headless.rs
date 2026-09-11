use super::*;

#[test]
fn opening_a_missing_index_says_to_scan_the_folder() {
    let dir = tempfile::tempdir().expect("tempdir");
    let err = open_index(&dir.path().join("nothing.sqlite")).expect_err("should fail");
    assert!(err.to_string().contains("Scan the folder"), "{err}");
}

#[test]
fn the_default_index_sits_in_the_scanned_folder() {
    let path = default_db_path(Path::new("/photos"));
    assert!(path.ends_with(db::INDEX_FILENAME));
}
