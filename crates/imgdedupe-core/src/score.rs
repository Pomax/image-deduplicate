use crate::format::Format;

/// Filename and path fragments that mark a file as a copy of another one.
const COPY_MARKERS: [&str; 8] = [
    " - copy",
    " copy",
    "copy of ",
    "-copy",
    "_copy",
    "(1)",
    "(2)",
    "(3)",
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
const TOTAL_LOWER_BUDGET: f64 =
    LOSSLESS_WEIGHT + BYTES_PER_PIXEL_PENALTY + COLOUR_WEIGHT + ALPHA_WEIGHT
        + MAX_MARKER_PENALTY + MAX_LENGTH_PENALTY;

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
mod tests {
    use super::*;

    fn score(width: u32, height: u32, format: Format, channels: u8, size: i64, path: &str) -> f64 {
        keep_score(width, height, format, channels, size, path)
    }

    #[test]
    fn more_pixels_wins() {
        let big = score(4000, 3000, Format::Jpeg, 3, 4_000_000, "a.jpg");
        let small = score(1000, 750, Format::Jpeg, 3, 250_000, "b.jpg");
        assert!(big > small, "{big} !> {small}");
    }

    #[test]
    fn resolution_outweighs_everything_below_it() {
        // Twice the pixels, and worst on every other term.
        let big = score(2000, 1500, Format::Jpeg, 1, 1, "downloads/photo - copy (1)_2.jpg");
        let small = score(1414, 1060, Format::Png, 4, 4_000_000, "photo.png");
        assert!(big > small, "{big} !> {small}");
    }

    #[test]
    fn lossless_wins_at_equal_resolution() {
        let png = score(1000, 1000, Format::Png, 3, 500_000, "a.png");
        let jpeg = score(1000, 1000, Format::Jpeg, 3, 500_000, "a.jpg");
        assert!(png > jpeg, "{png} !> {jpeg}");
    }

    #[test]
    fn the_smaller_file_wins_at_the_same_resolution() {
        let fat = score(1000, 1000, Format::Jpeg, 3, 2_000_000, "a.jpg");
        let thin = score(1000, 1000, Format::Jpeg, 3, 100_000, "b.jpg");
        assert!(thin > fat, "{thin} !> {fat}");
    }

    /// What the review list showed: three copies of one picture at 3000x4000,
    /// two of them 7.1 MB and one 1.2 MB. The small one is the keeper.
    #[test]
    fn the_small_copy_of_three_identical_pictures_is_the_keeper() {
        let small = score(3000, 4000, Format::Jpeg, 3, 1_200_000, "jwgormmw79hb1.jpeg");
        for other in ["karw5eaoqmpd1.jpeg", "o09d3ef8fybd1.jpeg"] {
            let big = score(3000, 4000, Format::Jpeg, 3, 7_100_000, other);
            assert!(small > big, "{other}: {small} !> {big}");
        }
    }

    #[test]
    fn colour_wins_over_grayscale() {
        let colour = score(800, 600, Format::Png, 3, 100_000, "a.png");
        let gray = score(800, 600, Format::Png, 1, 100_000, "b.png");
        assert!(colour > gray, "{colour} !> {gray}");
    }

    #[test]
    fn alpha_wins_over_flattened() {
        let alpha = score(800, 600, Format::Png, 4, 100_000, "a.png");
        let flat = score(800, 600, Format::Png, 3, 100_000, "b.png");
        assert!(alpha > flat, "{alpha} !> {flat}");
    }

    #[test]
    fn a_copy_marker_loses_to_a_clean_name() {
        let clean = score(800, 600, Format::Jpeg, 3, 100_000, "holiday.jpg");
        for marked in [
            "holiday - Copy.jpg",
            "holiday (1).jpg",
            "Copy of holiday.jpg",
            "holiday_1.jpg",
            "holiday-2.jpg",
            "~holiday.jpg",
        ] {
            let scored = score(800, 600, Format::Jpeg, 3, 100_000, marked);
            assert!(clean > scored, "{marked}: {clean} !> {scored}");
        }
    }

    #[test]
    fn a_copy_folder_loses_to_the_same_file_elsewhere() {
        let original = score(800, 600, Format::Jpeg, 3, 100_000, "photos/holiday.jpg");
        let downloaded = score(800, 600, Format::Jpeg, 3, 100_000, "downloads/holiday.jpg");
        assert!(original > downloaded, "{original} !> {downloaded}");
    }

    #[test]
    fn the_path_penalty_cannot_beat_a_resolution_doubling() {
        let marked_big = score(1600, 1200, Format::Jpeg, 3, 100_000, "downloads/a - copy (1)_2.jpg");
        let clean_small = score(1131, 848, Format::Jpeg, 3, 100_000, "a.jpg");
        assert!(marked_big > clean_small, "{marked_big} !> {clean_small}");
    }

    #[test]
    fn everything_below_resolution_together_is_worth_less_than_one_doubling() {
        // The budget that makes the priority order hold rather than merely
        // describe the intent. If a term is added or reweighted, this is what
        // catches it overrunning.
        assert!(TOTAL_LOWER_BUDGET < 1.0, "lower terms total {TOTAL_LOWER_BUDGET}");
    }

    #[test]
    fn a_shorter_path_wins_all_else_equal() {
        let short = score(800, 600, Format::Jpeg, 3, 100_000, "a.jpg");
        let long = score(800, 600, Format::Jpeg, 3, 100_000, "a/very/deeply/nested/place/a.jpg");
        assert!(short > long, "{short} !> {long}");
    }

    #[test]
    fn scoring_is_deterministic() {
        let once = score(800, 600, Format::Jpeg, 3, 100_000, "a.jpg");
        let twice = score(800, 600, Format::Jpeg, 3, 100_000, "a.jpg");
        assert_eq!(once, twice);
    }

    #[test]
    fn a_number_that_is_part_of_the_name_is_not_a_copy_marker() {
        assert!(!ends_with_copy_number("img2024"));
        assert!(ends_with_copy_number("img_2"));
        assert!(ends_with_copy_number("img-12"));
    }
}
