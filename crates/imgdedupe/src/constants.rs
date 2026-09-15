/// The one text size. There is no second size: headings, small print and button
/// labels are all this.
pub const FONT_SIZE: f32 = 16.0;

/// Room either side of a button's words.
pub const BUTTON_HORIZONTAL_PADDING: f32 = 12.0;

/// Room above and below a button's words.
pub const BUTTON_VERTICAL_PADDING: f32 = 7.0;

/// Space between widgets laid out side by side.
pub const WIDGET_HORIZONTAL_SPACING: f32 = 9.0;

/// Space between widgets laid out one above the other.
pub const WIDGET_VERTICAL_SPACING: f32 = 7.0;

/// The least height of anything that can be clicked or typed in.
pub const CONTROL_MIN_HEIGHT: f32 = 26.0;

/// The width of the ring drawn for a step there was nothing to do for.
pub const LAMP_SKIPPED_RING_WIDTH: f32 = 1.5;

/// The radius of a scan page lamp.
pub const LAMP_DOT_RADIUS: f32 = 5.0;

/// Width of the strip a scrollbar sits in, and of the handle that fills it.
pub const SCROLLBAR_STRIP_WIDTH: f32 = 12.0;

/// Spacing used between the sections of a view, so they are consistent.
pub const SECTION_SPACING_GAP: f32 = 14.0;

/// Width of the line `Frame::group` draws around its contents, on each side.
pub const FRAME_GROUP_BORDER_WIDTH: f32 = 1.0;

/// The margin `Frame::group` keeps inside its line, on each side.
pub const FRAME_GROUP_INNER_MARGIN: f32 = 6.0;

/// The widest a thumbnail in a set may be drawn.
pub const THUMBNAIL_MAX_WIDTH: f32 = 156.0;

/// The tallest a thumbnail in a set may be drawn. What the strip of them is tall
/// is worked out from this and the font, in `tile_strip_height`.
pub const THUMBNAIL_MAX_HEIGHT: f32 = 118.0;

/// Space kept clear around a thumbnail for what is drawn around it: the keeper's
/// border, and the ring outside that for the one the preview is showing. The ring
/// sits 3 out from the border and is 3 wide, so it reaches 4.5 past it. Without
/// this the ring is drawn outside the tile and the neighbour clips it.
pub const THUMBNAIL_CLEARANCE: f32 = 6.0;

/// The margin between a thumbnail and its border, on each side.
pub const THUMBNAIL_INNER_MARGIN: f32 = 2.0;

/// Room above and below the buttons inside the band along the bottom of a set,
/// so the line along the top of the band stands clear of the buttons instead of
/// being drawn along their top edge. The space between the buttons is the space
/// the row lays them out with.
pub const SET_BUTTON_BAND_VERTICAL_PADDING: f32 = 2.0;

/// How far in from the left edge of the box the row of buttons starts.
pub const SET_BUTTON_BAND_LEFT_INSET: f32 = 5.0;

/// What the page keeps at its edges, and what a list keeps between what is in it
/// and the scrollbar down its right: the same, so a box in a list stops as far
/// from the bar as the list stops from the edge of the window.
///
/// The review keeps this itself rather than through the panel it is drawn in, so
/// the panels in it can run the width of the window and draw their lines across
/// all of it.
pub const CONTENT_MARGIN: f32 = 16.0;

/// Kept clear at the right of the folder row for the button that lists the
/// folders scanned before, so a long path stops short of it.
pub const PREVIOUS_BUTTON_WIDTH: f32 = 84.0;

/// The review toolbar's one row. The checkbox, the counts and the button are
/// laid out over the same rectangle, which has to be as tall as the tallest of
/// them: the button.
pub const TOOLBAR_ROW_HEIGHT: f32 = 28.0;

/// Between the buttons at the left of the review toolbar.
pub const TOOLBAR_BUTTON_GAP: f32 = 14.0;

/// The cleanup button at the right of it, which is a fixed width so the counts
/// know how much of the row is left for them.
pub const CLEANUP_BUTTON_WIDTH: f32 = 120.0;

/// Room either side of a small button's words. Four of these sit under the
/// slider as presets and are read at a glance, so they are no bigger than the
/// words in them.
pub const SMALL_BUTTON_HORIZONTAL_PADDING: f32 = 6.0;

/// Room above and below a small button's words.
pub const SMALL_BUTTON_VERTICAL_PADDING: f32 = 2.0;

/// The box holding the percentage beside the slider. Wide enough for the widest
/// value the scale reaches, so the number never changes the width of anything.
pub const SENSITIVITY_PERCENTAGE_BOX_WIDTH: f32 = 56.0;

/// Kept between the writing under the thumbnails and the strip's own scrollbar,
/// which sits on the band of buttons below it. Enough to be seen: the bar reads
/// as another line of the writing when the two touch.
pub const SET_TEXT_TO_SCROLLBAR_GAP: f32 = 6.0;

/// Kept between one set and the next.
pub const SET_BOX_VERTICAL_GAP: f32 = 12.0;

/// How much of a set nobody calls a set of copies is drawn: the thumbnails and
/// every line of writing under them, but not the row of buttons, which is how it
/// stops being ignored.
pub const IGNORED_SET_OPACITY: f32 = 0.25;

/// The line the box is drawn with, on each side.
pub const SET_BOX_BORDER_WIDTH: f32 = 1.0;

/// What a box keeps between its edge and the thumbnails in it. The band of
/// buttons keeps none: it is the bottom of the box.
pub const SET_BOX_INNER_PADDING: f32 = 6.0;

/// Width of the window the first time it opens, before it has a size of its own to
/// remember.
pub const WINDOW_DEFAULT_WIDTH: f32 = 1100.0;

/// Height of the window the first time it opens. Tall enough for the scan page's
/// own content without scrolling: the three boxes, the progress box, and one line
/// per step of a pass.
pub const WINDOW_DEFAULT_HEIGHT: f32 = 860.0;

/// The narrowest the window can be made.
pub const WINDOW_MIN_WIDTH: f32 = 700.0;

/// The shortest the window can be made.
pub const WINDOW_MIN_HEIGHT: f32 = 780.0;

/// Space above and below the tabs along the top of the window, and above and below
/// an error along the bottom of it.
pub const WINDOW_BAR_VERTICAL_PADDING: f32 = 6.0;

/// Space between a page's content and the top and bottom of the window. The sides
/// keep `CONTENT_MARGIN`.
pub const CONTENT_VERTICAL_MARGIN: f32 = 12.0;

/// How often the window redraws while indexing, a search or a cleanup is running,
/// so its progress shows without the mouse having to move.
pub const WORK_PROGRESS_REPAINT_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(100);

/// How often the window redraws while waiting for a folder's index to arrive.
pub const INDEX_ARRIVAL_REPAINT_INTERVAL: std::time::Duration =
    std::time::Duration::from_millis(50);

/// Space above and below the content of a panel inside a page, and inside the
/// previous session question.
pub const PANEL_VERTICAL_PADDING: f32 = 4.0;

/// Space between a section's title and its box.
pub const SECTION_TITLE_GAP: f32 = 4.0;

/// Space between the groups of controls inside a section.
pub const SECTION_ROW_GAP: f32 = 6.0;

/// Width kept for a scan page lamp, which is wider than the lamp so the text beside
/// it does not touch it.
pub const LAMP_SLOT_WIDTH: f32 = LAMP_DOT_RADIUS * 3.0;

/// How far one step of the scan page's lamp list scrolls, in control heights.
pub const LAMP_LIST_SCROLL_STEP_CONTROL_HEIGHTS: f32 = 3.0;

/// Width of the sensitivity slider.
pub const SENSITIVITY_SLIDER_WIDTH: f32 = 300.0;

/// Least width of the "Scan" button.
pub const SCAN_BUTTON_WIDTH: f32 = 90.0;

/// Least width of the "Cancel" button.
pub const CANCEL_BUTTON_WIDTH: f32 = 80.0;

/// Width of the "Find duplicates" button.
pub const FIND_DUPLICATES_BUTTON_WIDTH: f32 = 178.0;

/// Least height of the buttons in the scan page's "Run" box.
pub const RUN_BUTTON_HEIGHT: f32 = 30.0;

/// Space between one progress bar and the next.
pub const PROGRESS_BAR_GAP: f32 = 4.0;

/// Space between the columns of counts under the progress bars.
pub const SCAN_COUNTS_COLUMN_GAP: f32 = 24.0;

/// Space between the rows of counts under the progress bars.
pub const SCAN_COUNTS_ROW_GAP: f32 = 4.0;

/// The tallest the list of files that could not be read grows before it scrolls.
pub const FAILED_FILES_LIST_MAX_HEIGHT: f32 = 160.0;

/// Width of the cleanup page's button, and of the progress bar that takes its place
/// while the cleanup runs.
pub const CLEANUP_PAGE_BUTTON_WIDTH: f32 = 210.0;

/// Least height of the cleanup page's button.
pub const CLEANUP_PAGE_BUTTON_HEIGHT: f32 = 28.0;

/// Width of the panel down the left of the cleanup page.
pub const CLEANUP_SETTINGS_PANEL_WIDTH: f32 = 320.0;

/// Space between the columns of the cleanup page's summary.
pub const CLEANUP_SUMMARY_COLUMN_GAP: f32 = 16.0;

/// Space between the rows of the cleanup page's summary.
pub const CLEANUP_SUMMARY_ROW_GAP: f32 = 4.0;

/// Space between the choice of where files go and the note under it.
pub const DESTINATION_NOTE_GAP: f32 = 4.0;

/// Width of the field holding the folder files are moved to.
pub const MOVE_FOLDER_FIELD_WIDTH: f32 = 190.0;

/// Space between the heading over the cleanup page's list of files and the list.
pub const FILE_LIST_HEADING_GAP: f32 = 4.0;

/// Width of the review page's preview pane before anybody has resized it, as a
/// fraction of the page.
pub const PREVIEW_PANE_DEFAULT_WIDTH_FRACTION: f32 = 0.42;

/// The narrowest the preview pane can be made.
pub const PREVIEW_PANE_MIN_WIDTH: f32 = 260.0;

/// Space between the file's name and its picture in the preview pane.
pub const PREVIEW_PICTURE_TOP_GAP: f32 = 4.0;

/// How much of the preview pane's height the picture takes, as a fraction.
pub const PREVIEW_PICTURE_HEIGHT_FRACTION: f32 = 0.62;

/// Space between the picture in the preview pane and what the file says about
/// itself.
pub const PREVIEW_METADATA_TOP_GAP: f32 = 6.0;

/// How much of the preview pane's width the names in the metadata list take, as a
/// fraction.
pub const METADATA_NAME_COLUMN_WIDTH_FRACTION: f32 = 0.38;

/// The shortest the metadata list is made, in lines.
pub const METADATA_LIST_MIN_HEIGHT_LINES: f32 = 3.0;

/// How far one step of the metadata list scrolls, in lines.
pub const METADATA_LIST_SCROLL_STEP_LINES: f32 = 3.0;

/// Corner radius of the band behind a heading in the metadata list.
pub const METADATA_HEADING_CORNER_RADIUS: f32 = 2.0;

/// Distance from the left of that band to the heading's text.
pub const METADATA_HEADING_LEFT_INSET: f32 = 6.0;

/// How much of the window a picture opened to fill it may take, as a fraction.
pub const FULL_WINDOW_PICTURE_SIZE_FRACTION: f32 = 0.98;

/// Width of the line along the top of the band of buttons at the bottom of a set.
pub const SET_BUTTON_BAND_TOP_LINE_WIDTH: f32 = 1.0;

/// Width of the border round a thumbnail that is being kept.
pub const KEPT_THUMBNAIL_BORDER_WIDTH: f32 = 3.0;

/// Width of the border round a thumbnail that is not being kept.
pub const THUMBNAIL_BORDER_WIDTH: f32 = 1.0;

/// How far inside a thumbnail's border the ring for the one the preview is showing
/// is drawn.
pub const SHOWN_THUMBNAIL_RING_INSET: f32 = 3.0;

/// Corner radius of that ring.
pub const SHOWN_THUMBNAIL_RING_CORNER_RADIUS: f32 = 2.0;

/// Width of that ring's line.
pub const SHOWN_THUMBNAIL_RING_WIDTH: f32 = 3.0;

/// Space between the previous session question and its buttons.
pub const PREVIOUS_SESSION_BUTTONS_GAP: f32 = 10.0;

/// Width of the line round a scrollbar's strip.
pub const SCROLLBAR_OUTLINE_WIDTH: f32 = 1.0;

/// Corner radius of a scrollbar's handle.
pub const SCROLLBAR_HANDLE_CORNER_RADIUS: f32 = 2.0;

/// How far a scrollbar arrow reaches from the middle of its button, as a fraction
/// of the button's size.
pub const SCROLLBAR_ARROW_REACH_FRACTION: f32 = 0.26;

/// Whether "Include subfolders" starts ticked.
pub const INCLUDE_SUBFOLDERS_DEFAULT: bool = false;

/// Whether "Only match within folders" starts ticked.
pub const ONLY_MATCH_WITHIN_FOLDERS_DEFAULT: bool = false;

/// Whether "Automatically rescan when opening this index" starts ticked.
pub const AUTOMATICALLY_RESCAN_DEFAULT: bool = false;

/// Whether "Automatically mark to keep" starts ticked.
pub const AUTOMATICALLY_MARK_TO_KEEP_DEFAULT: bool = false;

/// Whether "Match whole pictures" starts ticked.
pub const MATCH_WHOLE_PICTURES_DEFAULT: bool = true;

/// Whether "Match partials" starts ticked.
pub const MATCH_PARTIALS_DEFAULT: bool = true;

/// Whether "Match colour with grayscale" starts ticked.
pub const MATCH_COLOUR_WITH_GRAYSCALE_DEFAULT: bool = false;

/// The folder files are moved to before one has been chosen.
pub const MOVE_FOLDER_DEFAULT: &str = "";

/// The tab for the scan page.
pub const SCAN_TAB_LABEL: &str = "1  Scan";

/// The tab for the review page.
pub const REVIEW_TAB_LABEL: &str = "2  Review";

/// The tab for the cleanup page.
pub const CLEANUP_TAB_LABEL: &str = "3  Clean up";

/// The button that closes an error along the bottom of the window.
pub const DISMISS_ERROR_BUTTON_LABEL: &str = "dismiss";

/// Title of the scan page's folder box.
pub const FOLDER_SECTION_TITLE: &str = "Folder";

/// The button that opens the folder picker.
pub const CHOOSE_FOLDER_BUTTON_LABEL: &str = "Choose folder";

/// Written in the folder box when no folder is chosen.
pub const NO_FOLDER_CHOSEN_TEXT: &str = "none chosen";

/// The button that lists the folders scanned before.
pub const PREVIOUS_FOLDERS_BUTTON_LABEL: &str = "previous";

/// The last line of that list, which empties it.
pub const CLEAR_PREVIOUS_FOLDERS_LABEL: &str = "clear previous locations";

/// The subfolders checkbox.
pub const INCLUDE_SUBFOLDERS_LABEL: &str = "Include subfolders";

/// The checkbox for matching each subfolder on its own.
pub const ONLY_MATCH_WITHIN_FOLDERS_LABEL: &str = "Only match within folders";

/// The checkbox for keeping an index.
pub const SAVE_INDEX_LABEL: &str = "Save an index database for this folder";

/// The checkbox for rescanning when a folder is opened.
pub const AUTOMATICALLY_RESCAN_LABEL: &str = "Automatically rescan when opening this index";

/// The checkbox for marking at the end of a pass.
pub const AUTOMATICALLY_MARK_TO_KEEP_LABEL: &str = "Automatically mark to keep";

/// Title of the scan page's matching box.
pub const MATCHING_SECTION_TITLE: &str = "What counts as a duplicate";

/// The text beside the sensitivity slider.
pub const SENSITIVITY_SLIDER_LABEL: &str = "difference allowed";

/// What follows the number on the sensitivity slider.
pub const SENSITIVITY_SLIDER_SUFFIX: &str = " %";

/// Written before the preset buttons.
pub const SENSITIVITY_PRESETS_LABEL: &str = "presets:";

/// The checkbox for matching whole pictures.
pub const MATCH_WHOLE_PICTURES_LABEL: &str = "Match whole pictures";

/// The checkbox for matching part of a picture.
pub const MATCH_PARTIALS_LABEL: &str = "Match partials";

/// The checkbox for matching colour against grayscale.
pub const MATCH_COLOUR_WITH_GRAYSCALE_LABEL: &str = "Match colour with grayscale";

/// Title of the scan page's run box.
pub const RUN_SECTION_TITLE: &str = "Run";

/// The button that starts a pass.
pub const SCAN_BUTTON_LABEL: &str = "Scan";

/// The button that stops a pass or a search.
pub const CANCEL_BUTTON_LABEL: &str = "Cancel";

/// The button that starts a search.
pub const FIND_DUPLICATES_BUTTON_LABEL: &str = "Find duplicates";

/// Title of the scan page's progress box.
pub const PROGRESS_SECTION_TITLE: &str = "Progress";

/// The bar for reading the folder.
pub const SCANNING_PROGRESS_LABEL: &str = "Scanning for files";

/// The bar for writing the index.
pub const INDEXING_PROGRESS_LABEL: &str = "Indexing files";

/// The bar for the search.
pub const FINDING_DUPLICATES_PROGRESS_LABEL: &str = "Finding duplicates";

/// One file, in a count.
pub const FILE_WORD: &str = "file";

/// More than one file, in a count.
pub const FILES_WORD: &str = "files";

/// The count of files in the folder.
pub const FOUND_COUNTER_LABEL: &str = "found";

/// The count of files this pass read.
pub const NEW_COUNTER_LABEL: &str = "new";

/// The count of files the index already had.
pub const UNCHANGED_COUNTER_LABEL: &str = "unchanged";

/// The count of files gone from the folder.
pub const REMOVED_COUNTER_LABEL: &str = "removed";

/// The count of files that could not be read.
pub const FAILED_TO_READ_COUNTER_LABEL: &str = "failed to read";

/// The count of files read each second.
pub const PER_SECOND_COUNTER_LABEL: &str = "per second";

/// The lamp line when no folder is open.
pub const NO_FOLDER_OPEN_TEXT: &str = "No folder open";

/// What a pass over an empty folder says.
pub const NO_IMAGES_FOUND_TEXT: &str = "No images found in this folder";

/// What a cancelled pass or search says.
pub const CANCELLED_TEXT: &str = "cancelled";

/// What a search over an empty folder says.
pub const NO_FILES_IN_FOLDER_TEXT: &str = "No files in this folder";

/// What a search that found nothing says.
pub const NO_DUPLICATES_FOUND_TEXT: &str = "No duplicates found for current settings";

/// Title of the window asking about a saved review.
pub const PREVIOUS_SESSION_TITLE: &str = "Previous session";

/// The question in that window.
pub const PREVIOUS_SESSION_QUESTION: &str =
    "Previous session found but folder content has changed. Load previous session or rescan?";

/// The button that opens the saved review.
pub const FINISH_PREVIOUS_SESSION_BUTTON_LABEL: &str = "Finish previous session";

/// The button that scans the folder again instead.
pub const RESCAN_BUTTON_LABEL: &str = "Rescan";

/// Written on the review page when no folder is chosen.
pub const REVIEW_NO_FOLDER_TEXT: &str = "no folder chosen";

/// The toolbar button that takes every mark off.
pub const UNMARK_ALL_BUTTON_LABEL: &str = "unmark all";

/// The toolbar button that marks everything.
pub const MARK_ALL_BUTTON_LABEL: &str = "mark all";

/// The toolbar button that picks the keepers.
pub const AUTO_MARK_BUTTON_LABEL: &str = "auto-mark to keep";

/// The button that goes to the cleanup page.
pub const CLEAN_UP_BUTTON_LABEL: &str = "Clean up";

/// Written in the preview pane before a picture is clicked.
pub const PREVIEW_EMPTY_TEXT: &str = "click a picture to see it here";

/// The preview pane's button that keeps the picture on show.
pub const KEEP_THIS_ONE_BUTTON_LABEL: &str = "Keep this one";

/// Written while a picture or what a file says about itself is being read.
pub const READING_TEXT: &str = "reading...";

/// Written when a file says nothing about itself.
pub const NO_METADATA_TEXT: &str = "this file says nothing about itself";

/// A set's button that keeps every picture.
pub const KEEP_ALL_BUTTON_LABEL: &str = "keep all";

/// A set's button that keeps no picture.
pub const KEEP_NONE_BUTTON_LABEL: &str = "keep none";

/// A set's button that ignores it.
pub const IGNORE_BUTTON_LABEL: &str = "ignore";

/// That button once the set is ignored.
pub const IGNORED_BUTTON_LABEL: &str = "ignored";

/// Written over a thumbnail that is being kept.
pub const KEEP_MARK_TEXT: &str = "KEEP";

/// Written where a thumbnail will be while it is read.
pub const THUMBNAIL_LOADING_TEXT: &str = "...";

/// One set, in a count.
pub const SET_WORD: &str = "set";

/// More than one set, in a count.
pub const SETS_WORD: &str = "sets";

/// One duplicate, in a count.
pub const DUPLICATE_WORD: &str = "duplicate";

/// More than one duplicate, in a count.
pub const DUPLICATES_WORD: &str = "duplicates";

/// On the cleanup page's bar while files are moved.
pub const MOVING_WORD: &str = "moving";

/// On the cleanup page's bar while files are removed.
pub const REMOVING_WORD: &str = "removing";

/// On the cleanup page's bar while the index is tidied.
pub const TIDYING_INDEX_TEXT: &str = "tidying the index";

/// Title of the cleanup page's summary.
pub const WHAT_WILL_HAPPEN_SECTION_TITLE: &str = "What will happen";

/// The summary line for the number of sets.
pub const SETS_SUMMARY_LABEL: &str = "Sets";

/// The summary line for the number of files moved.
pub const FILES_MOVED_SUMMARY_LABEL: &str = "Files moved";

/// The summary line for the number of files removed.
pub const FILES_REMOVED_SUMMARY_LABEL: &str = "Files removed";

/// The summary line for the space freed.
pub const SPACE_FREED_SUMMARY_LABEL: &str = "Space freed";

/// Title of the cleanup page's destination box.
pub const WHERE_THEY_GO_SECTION_TITLE: &str = "Where they go";

/// The hint in the field for the folder files are moved to.
pub const MOVE_FOLDER_FIELD_HINT: &str = "folder";

/// The button that opens the folder picker for that field.
pub const CHOOSE_MOVE_FOLDER_BUTTON_LABEL: &str = "choose";

/// Heading over the list of files that will be moved.
pub const FILES_TO_MOVE_HEADING: &str = "Files that will be moved";

/// Heading over the list of files that will be removed.
pub const FILES_TO_REMOVE_HEADING: &str = "Files that will be removed";

/// The end of a cleanup's result when the index was not kept.
pub const INDEX_DELETED_TEXT: &str = "the index was deleted";

/// What the scan page says after a cleanup removed everything it planned to.
pub const CLEANUP_DONE_TEXT: &str = "cleanup done.";

/// The choice for sending files to the recycle bin.
pub const RECYCLE_BIN_LABEL: &str = "Recycle bin";

/// The choice for moving files to a folder.
pub const MOVE_TO_FOLDER_LABEL: &str = "Move to a folder";

/// The choice for deleting files outright.
pub const DELETE_PERMANENTLY_LABEL: &str = "Delete permanently";

/// The note under the recycle bin choice.
pub const RECYCLE_BIN_NOTE: &str = "Recoverable from the recycle bin.";

/// The note under the move choice.
pub const MOVE_TO_FOLDER_NOTE: &str = "Keeps the folder structure, so the files can be put back.";

/// The note under the delete choice.
pub const DELETE_PERMANENTLY_NOTE: &str = "This cannot be undone.";

/// The first word on the cleanup button when files are removed.
pub const REMOVE_VERB: &str = "Remove";

/// The first word on the cleanup button when files are moved.
pub const MOVE_VERB: &str = "Move";

/// The scan page lamp for looking for an index file in the folder.
pub const CHECKED_FOR_INDEX_FILE_LAMP_LABEL: &str = "Checked for sqlite file in this folder";

/// The scan page lamp for starting to open the index for writing.
pub const STARTED_OPENING_THE_INDEX_FOR_WRITING_LAMP_LABEL: &str =
    "Started opening the index for writing";

/// The scan page lamp for finishing opening the index for writing.
pub const FINISHED_OPENING_THE_INDEX_FOR_WRITING_LAMP_LABEL: &str =
    "Finished opening the index for writing";

/// The scan page lamp for starting to read the index's settings.
pub const STARTED_READING_THE_INDEX_SETTINGS_LAMP_LABEL: &str =
    "Started reading the index's own settings";

/// The scan page lamp for finishing reading the index's settings.
pub const FINISHED_READING_THE_INDEX_SETTINGS_LAMP_LABEL: &str =
    "Finished reading the index's own settings";

/// The scan page lamp for starting to count the files.
pub const STARTED_LOOKING_FOR_THE_TOTAL_LAMP_LABEL: &str =
    "Started looking for total number of files";

/// The scan page lamp for having counted the files.
pub const FOUND_THE_TOTAL_LAMP_LABEL: &str = "Found total number of files";

/// The scan page lamp for having listed the folder.
pub const LISTED_THE_FOLDER_LAMP_LABEL: &str = "Retrieved full file list in the folder";

/// The scan page lamp for having loaded the index into memory.
pub const LOADED_INDEX_INTO_MEMORY_LAMP_LABEL: &str =
    "Loaded sqlite file into memory and constructed in-memory index";

/// The scan page lamp for having compared the folder with the index.
pub const CROSS_REFERENCED_WITH_THE_INDEX_LAMP_LABEL: &str =
    "Cross referenced file list in folder with index from memory";

/// The scan page lamp for having counted new, unchanged and removed files.
pub const COUNTED_WHAT_CHANGED_LAMP_LABEL: &str =
    "Finished finding number of new, unchanged, and removed files";

/// The scan page lamp for starting to read new files.
pub const STARTED_READING_NEW_FILES_LAMP_LABEL: &str =
    "Starting individual file reads for any new file not in the index yet";

/// The scan page lamp for finishing reading new files.
pub const FINISHED_READING_NEW_FILES_LAMP_LABEL: &str =
    "Finished individual file reads for any new file not in the index yet";

/// The scan page lamp for starting to index new files.
pub const STARTED_INDEXING_NEW_FILES_LAMP_LABEL: &str =
    "Starting indexing for new files not in the index yet";

/// The scan page lamp for finishing indexing new files.
pub const FINISHED_INDEXING_NEW_FILES_LAMP_LABEL: &str =
    "Finished indexing for new files not in the index yet";

/// The scan page lamp for starting to build the in-memory index.
pub const STARTED_BUILDING_THE_MEMORY_INDEX_LAMP_LABEL: &str =
    "Starting index conversion to in-memory datastructure";

/// The scan page lamp for finishing building the in-memory index.
pub const FINISHED_BUILDING_THE_MEMORY_INDEX_LAMP_LABEL: &str =
    "Finished converting index to in-memory datastructure";

/// The scan page lamp for starting to find duplicates.
pub const STARTED_FINDING_DUPLICATES_LAMP_LABEL: &str =
    "Started duplication computation given current settings";

/// The scan page lamp for finishing finding duplicates.
pub const FINISHED_FINDING_DUPLICATES_LAMP_LABEL: &str = "Finished duplication computation";
