/// The widest setting offered. Unrelated pictures were measured above 25 percent
/// apart, so the top of this range reports them as duplicates. That is the
/// point of it: what is a duplicate is the person's to decide, and the review
/// step is where they decide it.
pub const SENSITIVITY_SLIDER_MAX_PERCENT: f64 = 50.0;

/// What the window starts on, and what it goes back to for a folder it has not
/// been set for.
pub const SENSITIVITY_SLIDER_DEFAULT_PERCENT: f64 = 15.0;

/// The named points on the scale. Everything between them is reachable too: a
/// preset is a place on the slider, not a separate setting.
pub const SENSITIVITY_SLIDER_PRESETS: [(&str, f64); 4] = [
    // Re-encodes and resizes of the same picture.
    ("close", 5.0),
    // Heavier edits, crops and rotations.
    ("balanced", SENSITIVITY_SLIDER_DEFAULT_PERCENT),
    // Pictures of the same thing, and some that are not.
    ("wide", 30.0),
    // Everything, including pictures with nothing to do with each other.
    ("yolo", SENSITIVITY_SLIDER_MAX_PERCENT),
];

/// Whether a threshold starts out skipping the colour check.
pub const IGNORE_COLOUR_DEFAULT: bool = false;

/// Whether a threshold starts out looking for pictures that fill the frame the
/// same way.
pub const WHOLE_FRAME_DEFAULT: bool = true;

/// Whether a threshold starts out looking for one picture inside another.
pub const CORNERS_DEFAULT: bool = true;

/// Whether a threshold starts out comparing only pictures in the same folder.
pub const WITHIN_A_FOLDER_DEFAULT: bool = false;
