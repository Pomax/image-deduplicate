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
    let big = score(
        2000,
        1500,
        Format::Jpeg,
        1,
        1,
        "downloads/photo - copy (1)_2.jpg",
    );
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

/// What the review list showed: three copies of one picture at 3000x4000, two of
/// them 7.1 MB and one 1.2 MB. The small one is the keeper.
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
    let marked_big = score(
        1600,
        1200,
        Format::Jpeg,
        3,
        100_000,
        "downloads/a - copy (1)_2.jpg",
    );
    let clean_small = score(1131, 848, Format::Jpeg, 3, 100_000, "a.jpg");
    assert!(marked_big > clean_small, "{marked_big} !> {clean_small}");
}

#[test]
fn everything_below_resolution_together_is_worth_less_than_one_doubling() {
    // The budget that makes the priority order hold rather than merely describe
    // the intent. If a term is added or reweighted, this is what catches it
    // overrunning.
    assert!(
        TOTAL_LOWER_BUDGET < 1.0,
        "lower terms total {TOTAL_LOWER_BUDGET}"
    );
}

#[test]
fn a_shorter_path_wins_all_else_equal() {
    let short = score(800, 600, Format::Jpeg, 3, 100_000, "a.jpg");
    let long = score(
        800,
        600,
        Format::Jpeg,
        3,
        100_000,
        "a/very/deeply/nested/place/a.jpg",
    );
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
