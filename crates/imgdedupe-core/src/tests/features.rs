use super::*;

/// A picture with enough going on in it to have corners: squares of varying
/// brightness, which is corners at every join.
fn picture(width: u32, height: u32, seed: u32) -> GrayImage {
    GrayImage::from_fn(width, height, |x, y| {
        let cell = (x / 17).wrapping_mul(7) ^ (y / 13).wrapping_mul(11).wrapping_add(seed);
        let shade = ((cell.wrapping_mul(2654435761) >> 24) & 0xFF) as u8;
        image::Luma([shade])
    })
}

#[test]
fn a_picture_gives_corners_and_the_same_ones_twice() {
    let one = features(&picture(512, 384, 0));
    let again = features(&picture(512, 384, 0));
    assert!(one.len() > 40, "only {} corners were found", one.len());
    assert_eq!(one, again, "the same picture gave a different fingerprint");
}

#[test]
fn a_flat_picture_has_no_corners() {
    let flat = GrayImage::from_pixel(300, 200, image::Luma([128]));
    assert!(features(&flat).is_empty());
}

#[test]
fn corners_are_spread_over_the_picture_rather_than_bunched() {
    let found = features(&picture(512, 384, 3));
    let left = found.iter().filter(|point| point.x < 256).count();
    let right = found.len() - left;
    assert!(
        left > found.len() / 6 && right > found.len() / 6,
        "the corners are all on one side: {left} left, {right} right"
    );
}

#[test]
fn corners_of_the_same_place_are_described_the_same_way() {
    let whole = picture(512, 384, 5);
    let corner = features(&whole);
    // The same picture, cut in half. Whatever survives the cut should be
    // described as it was before it.
    let cut = imageops::crop_imm(&whole, 0, 0, 256, 384).to_image();
    let inside = features(&cut);

    let mut matched = 0;
    for point in &inside {
        let best = corner
            .iter()
            .filter(|other| {
                (other.x as i32 - point.x as i32).abs() < 4
                    && (other.y as i32 - point.y as i32).abs() < 4
            })
            .map(|other| distance(&other.descriptor, &point.descriptor))
            .min();
        if best.is_some_and(|bits| bits < 64) {
            matched += 1;
        }
    }
    assert!(
        matched >= 10,
        "only {matched} of {} corners in the cut were described as they were in the whole",
        inside.len()
    );
}

#[test]
fn corners_survive_being_packed_and_read_back() {
    let found = features(&picture(400, 300, 9));
    assert_eq!(unpack(&pack(&found)), found);
    assert!(pack(&found).len() % KEYPOINT_BYTES == 0);
}

/// Corners spread over one picture whose best match in the other is all the
/// same corner. Line art does this: a dozen places described almost the same
/// way, and one place in the other picture that each of them is nearest to.
/// A dozen pairs that all end in the same place is not two pictures arranged
/// the same way, and it is how unrelated drawings were reaching the
/// threshold: sixteen corners agreeing on an arrangement that put every one
/// of them on two places.
#[test]
fn corners_that_all_match_the_same_corner_do_not_agree_on_anything() {
    let mut one = Vec::new();
    for which in 0..40u16 {
        one.push(Keypoint {
            x: which * 13,
            y: which * 7,
            descriptor: std::array::from_fn(|byte| {
                // Alike enough to pair, different enough to be corners of
                // different places.
                if byte == 31 {
                    which as u8
                } else {
                    0x5A
                }
            }),
        });
    }
    let other = vec![Keypoint {
        x: 100,
        y: 100,
        descriptor: std::array::from_fn(|byte| if byte == 31 { 0 } else { 0x5A }),
    }];

    assert_eq!(
        paired(&one, &other).len(),
        1,
        "more than one corner paired with the one"
    );
    assert_eq!(
        agreement(&one, &other),
        0,
        "corners agreed on an arrangement onto one place"
    );
}
