//! What a folder's own index says about how it is scanned, searched and
//! reviewed.
//!
//! These are choices about a folder rather than about the program: a folder
//! scanned with its subfolders is scanned that way again, a folder reviewed one
//! picture at a time is reviewed that way again, and a folder that is searched
//! one folder at a time keeps being. They live in the index's `meta` table
//! beside the schema version.
//!
//! Read in one place, so the names of the keys are written once. Nothing here
//! decides anything: the window takes these and applies them.

use std::path::Path;

use anyhow::Result;
use imgdedupe_core::db::{self, Connection};

/// Every choice an index holds, or nothing where the index has never been asked
/// about one. Nothing is a folder that has not said, which is not the same as a
/// folder that said no.
#[derive(Debug, Default, Clone)]
pub struct Notes {
    pub recurse: Option<bool>,
    pub disposal: Option<String>,
    pub move_dir: Option<String>,
    pub multi_select: Option<bool>,
    pub match_whole_frame: Option<bool>,
    pub match_corners: Option<bool>,
    pub within_a_folder: Option<bool>,
    pub auto_rescan: Option<bool>,
}

/// The keys the window writes and reads them under.
pub const RECURSE: &str = "recurse";
pub const DISPOSAL: &str = "disposal";
pub const MOVE_DIR: &str = "move_dir";
pub const MULTI_SELECT: &str = "multi_select";
pub const MATCH_WHOLE_FRAME: &str = "match_whole_frame";
pub const MATCH_CORNERS: &str = "match_corners";
pub const WITHIN_A_FOLDER: &str = "within_a_folder";
pub const AUTO_RESCAN: &str = "auto_rescan";

/// How a yes and a no are written.
pub fn mark(on: bool) -> &'static str {
    if on {
        "1"
    } else {
        "0"
    }
}

/// Everything the index has to say, off a connection that is already open.
pub fn read(conn: &Connection) -> Notes {
    let value = |key: &str| db::get_meta(conn, key).ok().flatten();
    let yes_or_no = |key: &str| value(key).map(|held| held == "1");
    Notes {
        recurse: yes_or_no(RECURSE),
        disposal: value(DISPOSAL),
        move_dir: value(MOVE_DIR),
        multi_select: yes_or_no(MULTI_SELECT),
        match_whole_frame: yes_or_no(MATCH_WHOLE_FRAME),
        match_corners: yes_or_no(MATCH_CORNERS),
        within_a_folder: yes_or_no(WITHIN_A_FOLDER),
        auto_rescan: yes_or_no(AUTO_RESCAN),
    }
}

/// The same, for an index nothing has opened yet. Called on a thread of its own:
/// the folder can be on another machine, and this is a file opened across the
/// network before anything has been drawn.
pub fn of_folder(db_path: &Path) -> Result<Notes> {
    let conn = db::open_for_notes(db_path)?;
    let notes = read(&conn);
    let _ = conn.close();
    Ok(notes)
}
