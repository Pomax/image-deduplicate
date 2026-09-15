use eframe::egui;

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

/// The band the buttons along the bottom of a set sit on.
pub const SET_BUTTON_BAND_BACKGROUND_COLOUR: egui::Color32 =
    egui::Color32::from_rgb(0xe8, 0xe8, 0xe8);
