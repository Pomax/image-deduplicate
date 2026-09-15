/// The window's title bar.
pub const WINDOW_TITLE_TEMPLATE: &str = "imgdedupe {version}";

/// Under the scan page's counters when a pass over the folder is over.
pub const SCAN_FINISHED_TEMPLATE: &str =
    "indexed {indexed}, removed {removed}, failed {failed}, in {seconds}s";

/// Above the lamps once a folder has been opened.
pub const LOADED_FOLDER_TEMPLATE: &str = "Loaded {folder}";

/// A lamp that has lit, with how long into the pass it lit.
pub const LAMP_LINE_TEMPLATE: &str = "{label}  {milliseconds} ms";

/// Above the progress bars while the folder is still being listed.
pub const LISTING_FOLDER_TEMPLATE: &str = "listing the folder: {count}";

/// The heading over the list of files a pass could not read.
pub const FILES_COULD_NOT_BE_READ_TEMPLATE: &str = "{count} files could not be read";

/// One line in the list of files a pass could not read.
pub const FAILED_FILE_LINE_TEMPLATE: &str = "{path}: {message}";

/// On the cleanup page's bar while files are going.
pub const CLEANUP_PROGRESS_TEMPLATE: &str = "{doing} {done} of {total}";

/// On the cleanup page's button.
pub const CLEANUP_BUTTON_TEMPLATE: &str = "{verb} {count} files";

/// A size in megabytes.
pub const MEGABYTES_TEMPLATE: &str = "{megabytes} MB";

/// A file a cleanup could not remove, with what the system said.
pub const CLEANUP_FAILED_FILE_LINE_TEMPLATE: &str = "{path}  {reason}";

/// The part of the cleanup result about the index, when it is kept.
pub const DROPPED_FROM_INDEX_TEMPLATE: &str = "{count} dropped from the index";

/// What a cleanup did, once it is over.
pub const CLEANUP_RESULT_TEMPLATE: &str =
    "removed {removed} files, freed {megabytes} MB, {failed} failed, {index}";

/// Under the preview: the picture's size, format and file size.
pub const PREVIEW_DETAILS_TEMPLATE: &str = "{width}x{height}  {format}  {megabytes} MB";

/// Under a thumbnail: the picture's size.
pub const THUMBNAIL_DIMENSIONS_TEMPLATE: &str = "{width}x{height}";

/// Under a thumbnail: the format and file size.
pub const THUMBNAIL_FORMAT_AND_SIZE_TEMPLATE: &str = "{format}  {megabytes} MB";

/// In the review toolbar: how many files are marked to go.
pub const TO_REMOVE_TEMPLATE: &str = "{count} to remove";

/// In the review toolbar: how much space those files take.
pub const TO_RECLAIM_TEMPLATE: &str = "{megabytes} MB to reclaim";

/// On a progress bar: its label and how full it is.
pub const PROGRESS_BAR_TEMPLATE: &str = "{label} {percent}%";

/// A number and the word for what it counts.
pub const COUNTED_TEMPLATE: &str = "{count} {noun}";

/// When a file was last written.
pub const FILE_DATE_TEMPLATE: &str = "{year}-{month}-{day} {hour}:{minute}";
