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
#[path = "tests/headless.rs"]
mod tests;
