use std::path::{Path, PathBuf};

use anyhow::Result;
use imgdedupe_core::db;

/// Open an index for reading. A pass is the only writer, so nothing here needs
/// write access.
pub fn open_index(db_path: &Path) -> Result<db::Connection> {
    if !db_path.exists() {
        anyhow::bail!("no index at {}. Scan the folder first.", db_path.display());
    }
    Ok(db::open_read_only(db_path)?)
}

/// Where the index for a folder lives unless it was put somewhere else.
pub fn default_db_path(root: &Path) -> PathBuf {
    root.join(db::INDEX_FILENAME)
}

#[cfg(test)]
mod tests {
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
}
