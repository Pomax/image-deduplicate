use crate::format::Format;

/// Filename and path fragments that mark a file as a copy of another one.
const COPY_MARKERS: [&str; 8] = [
    " - copy", " copy", "copy of ", "-copy", "_copy", "(1)", "(2)", "(3)",
];

/// Folder names that suggest a file arrived rather than originated.
const COPY_FOLDERS: [&str; 3] = ["/copy/", "/new folder", "/downloads/"];

/// How much better one candidate in a duplicate set is than another. Higher wins.
///
/// Resolution is the first criterion, and a doubling of pixel count is worth 1.0.
/// Every term below it is budgeted so that all of them together come to less than
/// that, which is what makes the priority order hold rather than merely describe
/// the intent.
const LOSSLESS_WEIGHT: f64 = 0.30;
const BYTES_PER_PIXEL_PENALTY: f64 = 0.20;
const COLOUR_WEIGHT: f64 = 0.15;
const ALPHA_WEIGHT: f64 = 0.08;
const MARKER_WEIGHT: f64 = 0.05;
const MAX_MARKER_PENALTY: f64 = 0.15;
const MAX_LENGTH_PENALTY: f64 = 0.05;

pub fn keep_score(
    width: u32,
    height: u32,
    format: Format,
    channels: u8,
    size_bytes: i64,
    rel_path: &str,
) -> f64 {
    let pixels = (width as f64 * height as f64).max(1.0);
    let mut score = pixels.log2();

    if !format.is_lossy() {
        score += LOSSLESS_WEIGHT;
    }

    // Everything in a set is the same picture, so at the same resolution the
    // extra bytes buy nothing and the smaller file is the one to keep.
    let bytes_per_pixel = (size_bytes.max(0) as f64) / pixels;
    score -= (bytes_per_pixel.min(8.0) / 8.0) * BYTES_PER_PIXEL_PENALTY;

    if channels >= 3 {
        score += COLOUR_WEIGHT;
    }
    if channels == 2 || channels == 4 {
        score += ALPHA_WEIGHT;
    }

    score += path_penalty(rel_path);
    score
}

/// Everything below resolution, at its most generous, against one doubling.
#[cfg(test)]
const TOTAL_LOWER_BUDGET: f64 = LOSSLESS_WEIGHT
    + BYTES_PER_PIXEL_PENALTY
    + COLOUR_WEIGHT
    + ALPHA_WEIGHT
    + MAX_MARKER_PENALTY
    + MAX_LENGTH_PENALTY;

/// Copies carry marks. This is capped so it can break a near-tie but can never
/// outweigh a difference in resolution.
fn path_penalty(rel_path: &str) -> f64 {
    let lower = rel_path.to_ascii_lowercase();
    let name = lower.rsplit('/').next().unwrap_or(&lower);
    let stem = name.rsplit_once('.').map(|(stem, _)| stem).unwrap_or(name);

    let mut markers = 0;
    for marker in COPY_MARKERS {
        if stem.contains(marker) {
            markers += 1;
        }
    }
    if stem.starts_with('~') {
        markers += 1;
    }
    if ends_with_copy_number(stem) {
        markers += 1;
    }
    for folder in COPY_FOLDERS {
        if lower.contains(folder) {
            markers += 1;
        }
    }

    let marker_penalty = (markers as f64 * MARKER_WEIGHT).min(MAX_MARKER_PENALTY);
    let length_penalty = (rel_path.chars().count() as f64 * 0.0005).min(MAX_LENGTH_PENALTY);
    -(marker_penalty + length_penalty)
}

/// A trailing `_1` or `-2`, which is what a save-again produces.
fn ends_with_copy_number(stem: &str) -> bool {
    let Some(last) = stem.chars().last() else {
        return false;
    };
    if !last.is_ascii_digit() {
        return false;
    }
    let trimmed = stem.trim_end_matches(|c: char| c.is_ascii_digit());
    trimmed.ends_with('_') || trimmed.ends_with('-')
}

#[cfg(test)]
#[path = "tests/score.rs"]
mod tests;
