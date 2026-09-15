use eframe::egui;

/// The colour an error is written in.
pub fn error_message_text_colour(visuals: &egui::Visuals) -> egui::Color32 {
    visuals.error_fg_color
}

/// The fill of the "Clean up" button, and of the cleanup page's button when what
/// it does can be undone.
pub fn cleanup_button_fill_colour(visuals: &egui::Visuals) -> egui::Color32 {
    visuals.selection.bg_fill
}

/// The fill of the cleanup page's button when what it does is permanent, on a
/// dark theme.
pub const PERMANENT_DELETE_BUTTON_FILL_DARK_COLOUR: egui::Color32 =
    egui::Color32::from_rgb(150, 50, 50);

/// The fill of the cleanup page's button when what it does is permanent, on a
/// light theme.
pub const PERMANENT_DELETE_BUTTON_FILL_LIGHT_COLOUR: egui::Color32 =
    egui::Color32::from_rgb(240, 170, 170);

/// The fill of the cleanup page's button when what it does is permanent.
pub fn permanent_delete_button_fill_colour(visuals: &egui::Visuals) -> egui::Color32 {
    if visuals.dark_mode {
        PERMANENT_DELETE_BUTTON_FILL_DARK_COLOUR
    } else {
        PERMANENT_DELETE_BUTTON_FILL_LIGHT_COLOUR
    }
}

/// The words on those buttons.
pub fn cleanup_button_text_colour(visuals: &egui::Visuals) -> egui::Color32 {
    visuals.selection.stroke.color
}

/// The border round a thumbnail that is being kept, and the "KEEP" over it, on a
/// dark theme.
pub const KEPT_THUMBNAIL_MARK_DARK_COLOUR: egui::Color32 = egui::Color32::from_rgb(90, 180, 110);

/// The border round a thumbnail that is being kept, and the "KEEP" over it, on a
/// light theme.
pub const KEPT_THUMBNAIL_MARK_LIGHT_COLOUR: egui::Color32 = egui::Color32::from_rgb(40, 130, 60);

/// The border round a thumbnail that is being kept, and the "KEEP" over it.
pub fn kept_thumbnail_mark_colour(visuals: &egui::Visuals) -> egui::Color32 {
    if visuals.dark_mode {
        KEPT_THUMBNAIL_MARK_DARK_COLOUR
    } else {
        KEPT_THUMBNAIL_MARK_LIGHT_COLOUR
    }
}

/// What covers the window behind a picture opened to fill it.
pub const FULL_WINDOW_PICTURE_BACKDROP_COLOUR: egui::Color32 = egui::Color32::from_black_alpha(240);

/// A scan page lamp for a step that has not happened yet.
pub const LAMP_WAITING_COLOUR: egui::Color32 = egui::Color32::from_rgb(196, 62, 54);

/// A scan page lamp for a step that has happened.
pub const LAMP_DONE_COLOUR: egui::Color32 = egui::Color32::from_rgb(58, 160, 78);

/// The ring for a step there was nothing to do for.
pub const LAMP_SKIPPED_RING_COLOUR: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);

/// The band the buttons along the bottom of a set sit on.
pub fn set_button_band_background_colour(visuals: &egui::Visuals) -> egui::Color32 {
    visuals.faint_bg_color
}
