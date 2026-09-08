use std::path::{Path, PathBuf};

#[cfg(any(debug_assertions, test))]
use anyhow::Result;
use imgdedupe_core::db;

/// Start a manager and give it a folder's index to hold.
///
/// Only the development build's command line opens an index this way. The window
/// has a manager of its own for as long as it is running.
#[cfg(any(debug_assertions, test))]
pub fn open_index(db_path: &Path) -> Result<imgdedupe_core::index::Index> {
    if !db_path.exists() {
        anyhow::bail!("no index at {}. Scan the folder first.", db_path.display());
    }
    let index = imgdedupe_core::index::Index::start();
    index.open(db_path)?;
    Ok(index)
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
