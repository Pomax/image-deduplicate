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

use imgdedupe_core::index::Index;

/// Every choice an index holds, or nothing where the index has never been asked
/// about one. Nothing is a folder that has not said, which is not the same as a
/// folder that said no.
#[derive(Debug, Default, Clone)]
pub struct Notes {
    pub recurse: Option<bool>,
    pub disposal: Option<String>,
    pub move_dir: Option<String>,
    pub auto_mark: Option<bool>,
    pub match_whole_frame: Option<bool>,
    pub match_corners: Option<bool>,
    pub within_a_folder: Option<bool>,
    pub auto_rescan: Option<bool>,
    pub sensitivity: Option<f64>,
    pub ignore_colour: Option<bool>,
}

/// The settings a search ran under, which is what its sets are the answer to.
/// Sets found under one of these are not the sets another gives.
///
/// This is what got used, not what somebody moved a control to. A slider moved
/// and left alone has changed nothing, and the index goes on holding the
/// settings the sets in it were found with.
#[derive(Debug, Default, Clone, Copy, PartialEq)]
pub struct Search {
    pub sensitivity: f64,
    pub whole_frame: bool,
    pub corners: bool,
    pub ignore_colour: bool,
    pub within_a_folder: bool,
}

/// The keys the window writes and reads them under.
pub const RECURSE: &str = "recurse";
pub const DISPOSAL: &str = "disposal";
pub const MOVE_DIR: &str = "move_dir";
pub const AUTO_MARK: &str = "auto_mark";
pub const MATCH_WHOLE_FRAME: &str = "match_whole_frame";
pub const MATCH_CORNERS: &str = "match_corners";
pub const WITHIN_A_FOLDER: &str = "within_a_folder";
pub const AUTO_RESCAN: &str = "auto_rescan";
pub const SENSITIVITY: &str = "sensitivity";
pub const IGNORE_COLOUR: &str = "ignore_colour";

/// How a yes and a no are written.
pub fn mark(on: bool) -> &'static str {
    if on {
        "1"
    } else {
        "0"
    }
}

/// Everything the index has to say, asked of the manager holding it.
pub fn read(index: &Index) -> Notes {
    let value = |key: &str| index.meta(key).ok().flatten();
    let yes_or_no = |key: &str| value(key).map(|held| held == "1");
    let number = |key: &str| value(key).and_then(|held| held.parse::<f64>().ok());
    Notes {
        recurse: yes_or_no(RECURSE),
        disposal: value(DISPOSAL),
        move_dir: value(MOVE_DIR),
        auto_mark: yes_or_no(AUTO_MARK),
        match_whole_frame: yes_or_no(MATCH_WHOLE_FRAME),
        match_corners: yes_or_no(MATCH_CORNERS),
        within_a_folder: yes_or_no(WITHIN_A_FOLDER),
        auto_rescan: yes_or_no(AUTO_RESCAN),
        sensitivity: number(SENSITIVITY),
        ignore_colour: yes_or_no(IGNORE_COLOUR),
    }
}

/// Write down the settings a search ran under. Called where one has run, and
/// nowhere else.
pub fn ran_under(index: &Index, search: &Search) -> anyhow::Result<()> {
    index.set_meta(SENSITIVITY, &search.sensitivity.to_string())?;
    index.set_meta(MATCH_WHOLE_FRAME, mark(search.whole_frame))?;
    index.set_meta(MATCH_CORNERS, mark(search.corners))?;
    index.set_meta(IGNORE_COLOUR, mark(search.ignore_colour))?;
    index.set_meta(WITHIN_A_FOLDER, mark(search.within_a_folder))
}


