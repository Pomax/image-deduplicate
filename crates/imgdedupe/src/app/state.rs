use super::*;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum View {
    Scan,
    Review,
    Cleanup,
}

/// What the thread removing files sends back.
pub(super) enum Removal {
    Progress(usize),
    /// The files are gone and the index is being brought up to date. On a large
    /// folder that is the slowest part of a cleanup, so it says so.
    Tidying,
    /// The outcome, and how many rows the index lost.
    Done(Box<cleanup::Outcome>, usize),
    Failed(String),
}

/// What the thread searching for duplicates sends back.
pub(super) enum Found {
    /// Where the search has got to. It used to send nothing until it was
    /// finished, so the window sat on whatever the pass had last said for the
    /// whole of it.
    Progress(matching::Progress),
    Sets(Vec<DuplicateSet>),
    Cancelled,
    Failed(String),
}

/// What the thread that opens a folder's index sends back.
///
/// A folder that has been scanned before is a folder whose pictures are already
/// known, so opening it reads them into memory whether or not a pass is going to
/// run: the search then costs the comparing and nothing else, and pressing Find
/// duplicates does not first sit through a file being read over a network.
pub(super) enum Opened {
    /// What the folder was set to the last time it was open.
    Notes(crate::notes::Notes),
    /// How far the reading has got.
    Reading(matching::Progress),
    /// Whether the folder holds an index. Looked for on a thread, not while
    /// drawing.
    Found(bool),
    /// The pairs somebody said are not copies of each other, read off the same
    /// connection as the pictures. They are part of what a folder's index says
    /// about it, so they arrive with it and are held in memory from then on.
    Ignored(Vec<(i64, i64)>),
    /// The pictures a review marked to keep, whenever that review was. Read off
    /// the same connection as the pairs, and held until there are sets to hang
    /// them on.
    Kept(Vec<i64>),
    /// The sets the last search of this folder found, as file ids in the order
    /// they were shown in. Held until the pictures arrive, which is what they are
    /// built from.
    Sets(Vec<(i64, Vec<i64>)>),
    /// The pictures, as the search wants them.
    Index(std::sync::Arc<Vec<matching::Image>>),
    Failed(String),
}

/// Why a folder with a saved review is being asked about rather than opened on
/// it.
///
/// One reason, and it is the only one there can be: a file was added, removed or
/// written since the review was saved, so the sets in it are not certainly the
/// sets of what is there now. Nothing having changed is not a question: the
/// review stands and is opened. A folder set to rescan itself is not a question
/// either: with something to bring up to date it is brought up to date, which is
/// what the box says, and with nothing to bring up to date the pass has no work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Question {
    TheFolderChanged,
}

impl Question {
    pub(super) fn wording(self) -> &'static str {
        match self {
            Question::TheFolderChanged => PREVIOUS_SESSION_QUESTION,
        }
    }
}

/// Every pair of pictures in a set, lower file id first, which is how a pair is
/// written down and how it is looked up.
pub(super) fn pairs_of(set: &DuplicateSet) -> impl Iterator<Item = (i64, i64)> + '_ {
    set.members.iter().enumerate().flat_map(|(at, one)| {
        set.members[at + 1..]
            .iter()
            .map(|other| db::pair(one.file_id, other.file_id))
    })
}

/// How one of the steps on the scan page went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Went {
    Happened,
    Waiting,
    Skipped,
}

/// Which way a cursor key moves the preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Direction {
    Forward,
    Back,
    NextSet,
    PreviousSet,
}

/// The place a cursor key moves to, given how many pictures each set on screen
/// holds. Nothing at either end of the list, rather than wrapping round.
///
/// Left and right run through the whole list, crossing into the next or previous
/// set at its edges. Up and down move a set at a time and stay at the same place
/// within it, or at the last picture when the set they arrive at is shorter.
pub(super) fn step(
    counts: &[usize],
    at: (usize, usize),
    direction: Direction,
) -> Option<(usize, usize)> {
    let (set, member) = at;
    if counts.get(set).copied().unwrap_or(0) == 0 {
        return None;
    }
    match direction {
        Direction::Forward => {
            if member + 1 < counts[set] {
                Some((set, member + 1))
            } else {
                let next = set + 1;
                (counts.get(next)? > &0).then_some((next, 0))
            }
        }
        Direction::Back => {
            if member > 0 {
                Some((set, member - 1))
            } else {
                let previous = set.checked_sub(1)?;
                (counts[previous] > 0).then(|| (previous, counts[previous] - 1))
            }
        }
        Direction::NextSet => {
            let next = set + 1;
            let count = *counts.get(next)?;
            (count > 0).then(|| (next, member.min(count - 1)))
        }
        Direction::PreviousSet => {
            let previous = set.checked_sub(1)?;
            let count = counts[previous];
            (count > 0).then(|| (previous, member.min(count - 1)))
        }
    }
}

/// What a set is keeping. A set with no entry at all has been marked with
/// nothing, which is a set nobody has reached: it keeps everything it has.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Keep {
    One(i64),
    /// More than one picture. Never one and never none: one picture is `One` and
    /// no picture is no entry at all.
    Several(Vec<i64>),
}

impl Keep {
    pub(super) fn keeps(&self, file_id: i64) -> bool {
        match self {
            Keep::One(kept) => *kept == file_id,
            Keep::Several(kept) => kept.contains(&file_id),
        }
    }

    /// The pictures marked, for the cleanup to read.
    pub(super) fn marked(&self) -> Vec<i64> {
        match self {
            Keep::One(kept) => vec![*kept],
            Keep::Several(kept) => kept.clone(),
        }
    }
}

/// Whether a set that is keeping this is keeping that picture.
pub(super) fn keeps(keeping: Option<&Keep>, file_id: i64) -> bool {
    keeping.is_some_and(|keep| keep.keeps(file_id))
}

/// What a set keeps once one more picture is marked, and once one is unmarked.
/// A set that ends up marked with nothing has no entry, which is what `None`
/// says.
pub(super) fn marked(keeping: Option<&Keep>, file_id: i64) -> Option<Keep> {
    let mut kept = keeping.map(Keep::marked).unwrap_or_default();
    kept.push(file_id);
    as_keep(kept)
}

pub(super) fn unmarked(keeping: Option<&Keep>, file_id: i64) -> Option<Keep> {
    let mut kept = keeping.map(Keep::marked).unwrap_or_default();
    kept.retain(|id| *id != file_id);
    as_keep(kept)
}

pub(super) fn as_keep(kept: Vec<i64>) -> Option<Keep> {
    match kept.len() {
        0 => None,
        1 => Some(Keep::One(kept[0])),
        _ => Some(Keep::Several(kept)),
    }
}

/// Where removed files go. Held as one value rather than three booleans, so the
/// three choices cannot all appear selected at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Destination {
    Trash,
    MoveTo,
    Delete,
}

impl Destination {
    pub(super) fn label(self) -> &'static str {
        match self {
            Destination::Trash => RECYCLE_BIN_LABEL,
            Destination::MoveTo => MOVE_TO_FOLDER_LABEL,
            Destination::Delete => DELETE_PERMANENTLY_LABEL,
        }
    }

    pub(super) fn note(self) -> &'static str {
        match self {
            Destination::Trash => RECYCLE_BIN_NOTE,
            Destination::MoveTo => MOVE_TO_FOLDER_NOTE,
            Destination::Delete => DELETE_PERMANENTLY_NOTE,
        }
    }

    pub(super) fn name(self) -> &'static str {
        match self {
            Destination::Trash => "trash",
            Destination::MoveTo => "move",
            Destination::Delete => "delete",
        }
    }

    /// What the button that carries this out does, in the words for it. Moving a
    /// file to another folder is not removing it.
    pub(super) fn verb(self) -> &'static str {
        match self {
            Destination::Trash | Destination::Delete => REMOVE_VERB,
            Destination::MoveTo => MOVE_VERB,
        }
    }

    pub(super) fn from_name(name: &str) -> Option<Self> {
        match name {
            "trash" => Some(Destination::Trash),
            "move" => Some(Destination::MoveTo),
            "delete" => Some(Destination::Delete),
            _ => None,
        }
    }
}

/// What one of the buttons under a set does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum SetAction {
    KeepAll,
    KeepNone,
    Ignore,
    Unignore,
}
