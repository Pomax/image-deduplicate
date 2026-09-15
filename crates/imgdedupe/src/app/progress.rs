use super::*;

/// What the scan screen is showing.
#[derive(Debug, Default, Clone)]
pub(super) struct ScanState {
    pub(super) total: u64,
    pub(super) done: u64,
    /// Whether the reading has begun, is going, or is over, and the same for the
    /// writing. A bar is a fraction only while the work it measures is running:
    /// before that it is empty and after it is full, whatever numbers are lying
    /// about from the listing or from the index.
    pub(super) reading: Stage,
    pub(super) writing: Stage,
    /// Pictures turned into a record, and how many of the folder are expected to
    /// become one.
    pub(super) indexed: u64,
    pub(super) to_index: u64,
    pub(super) per_sec: u64,
    pub(super) unchanged: u64,
    pub(super) removed: u64,
    /// Read, and not a picture this build indexes. Not a failure and not work
    /// that produced anything, so it is not counted as either.
    pub(super) ignored: u64,
    pub(super) failures: Vec<(String, String)>,
    pub(super) finished: Option<String>,
    /// Files the listing has found, while it is still listing. There is no total
    /// to measure it against until the listing is over, so this is a count, and
    /// it is the only thing there is to show for the part of a pass that used to
    /// show nothing at all.
    pub(super) listing: Option<u64>,
}

/// Where one stage of a pass has got to.
///
/// A count out of a total is worth drawing while the stage producing them is the
/// one running, and at no other time. Before it starts there is nothing to
/// measure and the bar is empty; once it is over everything it was going to do
/// is done and the bar is full.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(super) enum Stage {
    #[default]
    Waiting,
    Running,
    Over,
}

impl Stage {
    /// Under way, unless it is already over: a report that arrives after the
    /// stage that sent it finished does not start it again.
    pub(super) fn begun(self) -> Stage {
        match self {
            Stage::Over => Stage::Over,
            _ => Stage::Running,
        }
    }
}

/// What the search is doing, kept entirely apart from what the pass did.
///
/// The two have nothing to say about each other. When the pass has read and
/// indexed a folder its numbers are the answer and they stay on screen until a
/// new scan; the search reports its own work underneath them.
#[derive(Debug, Default, Clone)]
pub(super) struct SearchState {
    /// The stage, or nothing when no search is running.
    pub(super) stage: Option<&'static str>,
    /// Pictures read out of the index, of how many it holds. Zero for the total
    /// means it has not counted them yet.
    pub(super) loaded: u64,
    pub(super) to_load: u64,
    /// Whether this search is reading the index at all. A search that follows a
    /// pass is not: the pass built what it searches while it was scanning, and
    /// there is nothing left to read.
    pub(super) reads_the_index: bool,
    /// Pictures the shortlist has looked up, of how many there are.
    pub(super) shortlisted: u64,
    pub(super) to_shortlist: u64,
    /// Pairs compared, of how many the shortlist produced.
    pub(super) compared: u64,
    pub(super) pairs: u64,
    /// Set when the search is over, so the bar stays full afterwards rather than
    /// emptying because nothing is reporting any more.
    pub(super) done: bool,
}

impl SearchState {
    /// The search as one number out of one number.
    ///
    /// Stages of very different lengths, none of which knows its own size until
    /// it starts, so they share one bar an equal part each: reading the index,
    /// drawing up the shortlist, then comparing what it produced. Measured on a
    /// folder of ten thousand pictures those take four, eight and thirteen
    /// seconds, so an equal part each runs a little fast at the start and a
    /// little slow at the end, and never stands still.
    ///
    /// A search straight after a pass does no reading: the pass built what it
    /// searches. Giving that a part of its own would open the bar at a third for
    /// work that nothing is going to do, so the bar is the parts that are going
    /// to happen and no others.
    pub(super) fn progress(&self) -> (u64, u64) {
        const PART: u64 = 1000;
        let reading = if self.reads_the_index { 1 } else { 0 };
        let whole = PART * (reading + 2);
        if self.pairs > 0 || self.compared > 0 {
            return (
                PART * (reading + 1) + fraction(self.compared, self.pairs, PART),
                whole,
            );
        }
        if self.to_shortlist > 0 {
            return (
                PART * reading + fraction(self.shortlisted, self.to_shortlist, PART),
                whole,
            );
        }
        if reading == 0 {
            // The count of what is in memory arrives before the work starts.
            // It is not progress: nothing has been done with it yet.
            return (0, whole);
        }
        (fraction(self.loaded, self.to_load, PART), whole)
    }
}

/// `part` of `whole`, scaled onto `out_of`. Nothing of nothing is nothing.
pub(super) fn fraction(part: u64, whole: u64, out_of: u64) -> u64 {
    if whole == 0 {
        return 0;
    }
    (part.min(whole) * out_of) / whole
}

impl ScanState {
    /// Pictures this pass actually read. What was skipped over as not an image,
    /// and what could not be read at all, are their own numbers.
    /// Pictures this pass read. What was left alone was not read, and files that
    /// are not pictures or would not open are counted on their own.
    pub(super) fn found(&self) -> u64 {
        self.done
            .saturating_sub(self.unchanged)
            .saturating_sub(self.ignored)
            .saturating_sub(self.failures.len() as u64)
    }
}

/// The things a pass goes through, each with a lamp on the scan page. Red until
/// the thing happens, green after.
///
/// The order here is the order they are drawn in, which is the order they were
/// asked for and not the order they happen in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(super) enum Lamp {
    CheckedForIndexFile,
    StartedReadingTheIndexSettings,
    FinishedReadingTheIndexSettings,
    StartedOpeningTheIndexForWriting,
    FinishedOpeningTheIndexForWriting,
    StartedLookingForTheTotal,
    FoundTheTotal,
    LoadedIndexIntoMemory,
    ListedTheFolder,
    CrossReferencedWithTheIndex,
    CountedWhatChanged,
    StartedReadingNewFiles,
    FinishedReadingNewFiles,
    StartedIndexingNewFiles,
    FinishedIndexingNewFiles,
    StartedBuildingTheMemoryIndex,
    FinishedBuildingTheMemoryIndex,
    StartedFindingDuplicates,
    FinishedFindingDuplicates,
}

/// In the order a pass goes through them, which is also the order they read in.
///
/// The index is opened for writing first and its settings are read off that same
/// connection, so the two about opening come before the two about the settings.
///
/// One thing sits out of its running order: on a folder where nothing has
/// changed, the conversion happens as soon as the counting says there is nothing
/// to index, so those two lamps turn before the four about reading and indexing,
/// which are skipped rather than run.
pub(super) const LAMPS: [(Lamp, &str); 19] = [
    (Lamp::CheckedForIndexFile, CHECKED_FOR_INDEX_FILE_LAMP_LABEL),
    (
        Lamp::StartedOpeningTheIndexForWriting,
        STARTED_OPENING_THE_INDEX_FOR_WRITING_LAMP_LABEL,
    ),
    (
        Lamp::FinishedOpeningTheIndexForWriting,
        FINISHED_OPENING_THE_INDEX_FOR_WRITING_LAMP_LABEL,
    ),
    (
        Lamp::StartedReadingTheIndexSettings,
        STARTED_READING_THE_INDEX_SETTINGS_LAMP_LABEL,
    ),
    (
        Lamp::FinishedReadingTheIndexSettings,
        FINISHED_READING_THE_INDEX_SETTINGS_LAMP_LABEL,
    ),
    (
        Lamp::StartedLookingForTheTotal,
        STARTED_LOOKING_FOR_THE_TOTAL_LAMP_LABEL,
    ),
    (Lamp::FoundTheTotal, FOUND_THE_TOTAL_LAMP_LABEL),
    (Lamp::ListedTheFolder, LISTED_THE_FOLDER_LAMP_LABEL),
    (
        Lamp::LoadedIndexIntoMemory,
        LOADED_INDEX_INTO_MEMORY_LAMP_LABEL,
    ),
    (
        Lamp::CrossReferencedWithTheIndex,
        CROSS_REFERENCED_WITH_THE_INDEX_LAMP_LABEL,
    ),
    (Lamp::CountedWhatChanged, COUNTED_WHAT_CHANGED_LAMP_LABEL),
    (
        Lamp::StartedReadingNewFiles,
        STARTED_READING_NEW_FILES_LAMP_LABEL,
    ),
    (
        Lamp::FinishedReadingNewFiles,
        FINISHED_READING_NEW_FILES_LAMP_LABEL,
    ),
    (
        Lamp::StartedIndexingNewFiles,
        STARTED_INDEXING_NEW_FILES_LAMP_LABEL,
    ),
    (
        Lamp::FinishedIndexingNewFiles,
        FINISHED_INDEXING_NEW_FILES_LAMP_LABEL,
    ),
    (
        Lamp::StartedBuildingTheMemoryIndex,
        STARTED_BUILDING_THE_MEMORY_INDEX_LAMP_LABEL,
    ),
    (
        Lamp::FinishedBuildingTheMemoryIndex,
        FINISHED_BUILDING_THE_MEMORY_INDEX_LAMP_LABEL,
    ),
    (
        Lamp::StartedFindingDuplicates,
        STARTED_FINDING_DUPLICATES_LAMP_LABEL,
    ),
    (
        Lamp::FinishedFindingDuplicates,
        FINISHED_FINDING_DUPLICATES_LAMP_LABEL,
    ),
];

impl From<scan::Step> for Lamp {
    fn from(step: scan::Step) -> Self {
        match step {
            scan::Step::StartedReadingTheIndexSettings => Lamp::StartedReadingTheIndexSettings,
            scan::Step::FinishedReadingTheIndexSettings => Lamp::FinishedReadingTheIndexSettings,
            scan::Step::StartedOpeningTheIndexForWriting => Lamp::StartedOpeningTheIndexForWriting,
            scan::Step::FinishedOpeningTheIndexForWriting => {
                Lamp::FinishedOpeningTheIndexForWriting
            }
            scan::Step::StartedConvertingTheIndex => Lamp::StartedBuildingTheMemoryIndex,
            scan::Step::FinishedConvertingTheIndex => Lamp::FinishedBuildingTheMemoryIndex,
            scan::Step::StartedLookingForTheTotal => Lamp::StartedLookingForTheTotal,
            scan::Step::FoundTheTotal => Lamp::FoundTheTotal,
            scan::Step::LoadedIndexIntoMemory => Lamp::LoadedIndexIntoMemory,
            scan::Step::ListedTheFolder => Lamp::ListedTheFolder,
            scan::Step::CrossReferencedWithTheIndex => Lamp::CrossReferencedWithTheIndex,
            scan::Step::CountedWhatChanged => Lamp::CountedWhatChanged,
            scan::Step::StartedReadingNewFiles => Lamp::StartedReadingNewFiles,
            scan::Step::FinishedReadingNewFiles => Lamp::FinishedReadingNewFiles,
            scan::Step::StartedIndexingNewFiles => Lamp::StartedIndexingNewFiles,
            scan::Step::FinishedIndexingNewFiles => Lamp::FinishedIndexingNewFiles,
        }
    }
}
