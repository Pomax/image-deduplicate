use eframe::egui;

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

/// The colour an error is written in.
pub const ERROR_MESSAGE_TEXT_COLOUR: egui::Color32 = egui::Color32::from_rgb(200, 80, 80);

/// The fill of the "Clean up" button, and of the cleanup page's button when what
/// it does can be undone.
pub const CLEANUP_BUTTON_FILL_COLOUR: egui::Color32 = egui::Color32::from_rgb(60, 110, 180);

/// The fill of the cleanup page's button when what it does is permanent.
pub const PERMANENT_DELETE_BUTTON_FILL_COLOUR: egui::Color32 = egui::Color32::from_rgb(150, 50, 50);

/// The words on those buttons.
pub const CLEANUP_BUTTON_TEXT_COLOUR: egui::Color32 = egui::Color32::WHITE;

/// The border round a thumbnail that is being kept, and the "KEEP" over it.
pub const KEPT_THUMBNAIL_MARK_COLOUR: egui::Color32 = egui::Color32::from_rgb(90, 180, 110);

/// What covers the window behind a picture opened to fill it.
pub const FULL_WINDOW_PICTURE_BACKDROP_COLOUR: egui::Color32 = egui::Color32::from_black_alpha(240);

/// A scan page lamp for a step that has not happened yet.
pub const LAMP_WAITING_COLOUR: egui::Color32 = egui::Color32::from_rgb(196, 62, 54);

/// A scan page lamp for a step that has happened.
pub const LAMP_DONE_COLOUR: egui::Color32 = egui::Color32::from_rgb(58, 160, 78);

/// The ring for a step there was nothing to do for.
pub const LAMP_SKIPPED_RING_COLOUR: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);

/// The width of that ring's line.
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

/// The band those buttons sit on.
pub const SET_BUTTON_BAND_BACKGROUND_COLOUR: egui::Color32 =
    egui::Color32::from_rgb(0xe8, 0xe8, 0xe8);

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
