use super::*;

/// Two images are the same picture only if they have the same shape, allowing for
/// a 90 degree rotation having swapped the axes.
pub(super) fn aspect_ok(w1: f64, h1: f64, w2: f64, h2: f64) -> bool {
    if w1 <= 0.0 || h1 <= 0.0 || w2 <= 0.0 || h2 <= 0.0 {
        return false;
    }
    let a = w1.max(h1) / w1.min(h1);
    let b = w2.max(h2) / w2.min(h2);
    (a - b).abs() / a.max(b) < 0.06
}

/// Whether a candidate pair really is the same picture. `a` is the one with the
/// lower file id, which is the side that contributes all eight of its variants.
pub(super) fn is_match(a: &Image, b: &Image, thresholds: Thresholds) -> bool {
    // A search that only matches inside a folder compares nothing across two of
    // them, however alike the two pictures are.
    if thresholds.within_a_folder && folder_of(&a.rel_path) != folder_of(&b.rel_path) {
        return false;
    }
    (thresholds.whole_frame && whole_frame_match(a, b, thresholds))
        || (thresholds.corners && same_picture_inside(a, b))
}

/// The two pictures are the same picture filling the frame the same way: a
/// resize, a recompression, a rotation. Cheap, and it is most of what a folder
/// of duplicates holds.
fn whole_frame_match(a: &Image, b: &Image, thresholds: Thresholds) -> bool {
    if !aspect_ok(
        a.width as f64,
        a.height as f64,
        b.width as f64,
        b.height as f64,
    ) {
        return false;
    }
    let distance = fingerprint::hamming_any_words(&a.variants, &b.variants[0], thresholds.max_bits);
    if distance > thresholds.max_bits {
        return false;
    }
    thresholds.ignore_colour
        || fingerprint::ring_distance_weighted(&a.ring, &b.ring) <= thresholds.max_ring
}

/// One picture is the other, or part of it.
///
/// Enough of the corners describe the same things, arranged the same way, which
/// a crop of a picture keeps and a different picture of the same subject does
/// not. This is what the whole-frame hash cannot see: crop a picture and every
/// number in that hash changes at once, while its corners stay where they were.
fn same_picture_inside(a: &Image, b: &Image) -> bool {
    features::agreement(&a.corners, &b.corners) >= features::AGREEING_CORNERS
}
