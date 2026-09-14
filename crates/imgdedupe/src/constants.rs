use eframe::egui;

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
