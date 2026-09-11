use super::*;
use crate::decode::{decode_at_most, Decoded};
use crate::format::Format;
use image::DynamicImage;

/// A stand-in for a photograph: hard edges, blocks at several scales and no
/// symmetry in either axis.
///
/// A smooth gradient will not do. Its low-frequency coefficients are all near
/// zero and therefore all near the median, so half the hash bits are decided
/// by rounding and any resampling flips them. Real pictures have energy
/// spread across the block, which is what makes the threshold meaningful.
fn scene(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        let fx = x as f32 / width as f32;
        let fy = y as f32 / height as f32;

        let mut value = 40.0;
        if fx > 0.15 && fx < 0.55 && fy > 0.10 && fy < 0.45 {
            value = 210.0;
        }
        if fx > 0.60 && fy > 0.55 {
            value = 130.0;
        }
        if (fx - 0.30).powi(2) + (fy - 0.72).powi(2) < 0.02 {
            value = 245.0;
        }
        if fx + fy * 0.5 > 1.05 {
            value = 20.0;
        }
        // A coarse checker, large enough to survive the reduction to 32x32.
        if fx > 0.62 && fy < 0.40 {
            let cell = ((fx * 9.0) as u32 + (fy * 6.0) as u32) % 2;
            value = if cell == 0 { 200.0 } else { 60.0 };
        }

        let tint = (fx * 60.0) as u8;
        image::Rgb([
            (value + tint as f32).min(255.0) as u8,
            value as u8,
            (value * 0.6) as u8,
        ])
    })
}

fn decoded_from(image: &RgbImage) -> Decoded {
    Decoded {
        width: image.width(),
        height: image.height(),
        channels: 3,
        small: image.clone(),
    }
}

fn hash_of(image: &RgbImage) -> Hash {
    fingerprint(&decoded_from(image)).dct_hashes[0]
}

fn hashes_of(image: &RgbImage) -> [Hash; VARIANTS] {
    fingerprint(&decoded_from(image)).dct_hashes
}

/// How the matching query compares two images: one side's whole set against
/// the other side's indexed hash.
fn distance(a: &RgbImage, b: &RgbImage) -> u32 {
    hamming_any(&hashes_of(a), &hash_of(b))
}

/// Tolerances are a share of the hash, not a count, so changing the hash
/// length does not silently change what these tests demand.
fn within(percent: f64) -> u32 {
    (HASH_BITS as f64 * percent / 100.0) as u32
}

/// What a resize, a recompression or a rotation is allowed to move the hash.
const SAME_PICTURE: f64 = 12.0;

#[test]
fn a_rotated_non_square_image_matches_the_original() {
    let image = scene(160, 100);
    for (name, rotated) in [
        ("90", imageops::rotate90(&image)),
        ("180", imageops::rotate180(&image)),
        ("270", imageops::rotate270(&image)),
    ] {
        let apart = distance(&image, &rotated);
        assert!(
            apart <= within(SAME_PICTURE),
            "{name} degree rotation was {apart} of {HASH_BITS} bits away"
        );
    }
}

#[test]
fn a_mirrored_non_square_image_matches_the_original() {
    let image = scene(160, 100);
    for (name, mirrored) in [
        ("horizontal", imageops::flip_horizontal(&image)),
        ("vertical", imageops::flip_vertical(&image)),
    ] {
        let apart = distance(&image, &mirrored);
        assert!(
            apart <= within(SAME_PICTURE),
            "{name} mirror was {apart} of {HASH_BITS} bits away"
        );
    }
}

#[test]
fn a_rotated_copy_that_was_also_resized_still_matches() {
    // The case a canonical orientation gets wrong: resampling by a
    // non-integral factor is what flips a canonicalisation decision.
    let image = scene(320, 200);
    let rotated = imageops::rotate90(&imageops::thumbnail(&image, 213, 133));
    let apart = distance(&image, &rotated);
    assert!(
        apart <= within(SAME_PICTURE),
        "a rotated resize was {apart} of {HASH_BITS} bits away"
    );
}

#[test]
fn an_unrelated_image_is_far_from_every_variant() {
    let a = scene(200, 150);
    let b = RgbImage::from_fn(200, 150, |x, y| {
        image::Rgb([((x * 7 + y * 3) % 256) as u8, ((y * 11) % 256) as u8, 40])
    });
    let apart = distance(&a, &b);
    assert!(
        apart > within(25.0),
        "unrelated pictures were only {apart} of {HASH_BITS} bits apart"
    );
}

#[test]
fn the_variant_hashes_of_a_symmetry_are_a_permutation_of_the_originals() {
    // This is why comparing one side's eight against the other side's one
    // works, and it is the property the whole scheme rests on.
    let base = grid(&scene(160, 100));
    let original = variant_hashes(&base);
    for variant in 0..8u8 {
        let moved = variant_hashes(&symmetry_of(&base, variant));
        let mut sorted_original = original;
        let mut sorted_moved = moved;
        sorted_original.sort_unstable();
        sorted_moved.sort_unstable();
        assert_eq!(sorted_original, sorted_moved, "symmetry {variant}");
    }
}

#[test]
fn hashes_round_trip_through_storage() {
    let hashes = hashes_of(&scene(64, 48));
    assert_eq!(unpack_hashes(&pack_hashes(&hashes)), Some(hashes));
    assert_eq!(unpack_hashes(&[0, 1, 2]), None);
}

#[test]
fn resampling_onto_a_fixed_square_carries_rotation_through() {
    // Why there is no orientation correction before the grid: resampling a
    // rotated image onto the square gives the rotation of the resampled
    // square. Checked directly on the grids, so a change to the resampler
    // that broke it would show up here rather than as a matching miss.
    let image = scene(240, 60);
    let straight = grid(&image);
    let rotated = grid(&imageops::rotate90(&image));

    let mut turned = vec![0.0f32; GRID * GRID];
    for y in 0..GRID {
        for x in 0..GRID {
            turned[y * GRID + x] = straight[(GRID - 1 - x) * GRID + y];
        }
    }

    let mean_difference = |a: &[f32], b: &[f32]| -> f32 {
        a.iter()
            .zip(b.iter())
            .map(|(x, y)| (x - y).abs())
            .sum::<f32>()
            / (GRID * GRID) as f32
    };

    // Resampling at 4:1 is coarse on the short axis, so the two do not agree
    // exactly. The bound that matters is relative: what is left has to be far
    // smaller than the difference against an unrelated picture.
    let carried = mean_difference(&rotated, &turned);
    let unrelated = mean_difference(&rotated, &grid(&scene(97, 61)));
    assert!(
        carried * 4.0 < unrelated,
        "rotation did not survive the resample: {carried} against {unrelated} for an unrelated image"
    );
}

#[test]
fn a_strongly_rectangular_image_matches_its_rotation() {
    let image = scene(240, 60);
    let apart = distance(&image, &imageops::rotate90(&image));
    assert!(
        apart <= within(SAME_PICTURE),
        "a 4:1 rotation was {apart} of {HASH_BITS} bits away"
    );
}

/// Bit 0 mirrors x, bit 1 mirrors y, bit 2 transposes: all eight symmetries
/// of the square, applied to a grid rather than to its coefficients.
fn symmetry_of(grid: &[f32], variant: u8) -> Vec<f32> {
    let mut out = vec![0.0; GRID * GRID];
    for y in 0..GRID {
        for x in 0..GRID {
            let (mut sx, mut sy) = (x, y);
            if variant & 1 != 0 {
                sx = GRID - 1 - sx;
            }
            if variant & 2 != 0 {
                sy = GRID - 1 - sy;
            }
            if variant & 4 != 0 {
                std::mem::swap(&mut sx, &mut sy);
            }
            out[y * GRID + x] = grid[sy * GRID + sx];
        }
    }
    out
}

#[test]
fn a_square_image_still_matches_its_rotations() {
    let image = scene(128, 128);
    assert_eq!(distance(&image, &imageops::rotate90(&image)), 0);
    assert_eq!(distance(&image, &imageops::rotate180(&image)), 0);
    assert_eq!(distance(&image, &imageops::flip_horizontal(&image)), 0);
}

#[test]
fn a_resized_copy_hashes_close_to_the_original() {
    let image = scene(320, 200);
    for (width, height) in [(160, 100), (213, 133), (96, 60)] {
        let resized = imageops::thumbnail(&image, width, height);
        let apart = distance(&image, &resized);
        assert!(
            apart <= within(SAME_PICTURE),
            "{width}x{height} moved the hash by {apart} of {HASH_BITS} bits"
        );
    }
}

#[test]
fn a_recompressed_copy_hashes_close_to_the_original() {
    let image = scene(320, 200);
    let mut buffer = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut buffer, image::ImageFormat::Jpeg)
        .expect("encode");
    let decoded = decode_at_most(Format::Jpeg, &buffer.into_inner(), 320).expect("decode");
    let apart = hamming_any(&hashes_of(&image), &fingerprint(&decoded).dct_hashes[0]);
    assert!(
        apart <= within(SAME_PICTURE),
        "recompression moved the hash by {apart} of {HASH_BITS} bits"
    );
}

#[test]
fn a_grayscale_copy_hashes_close_to_the_colour_original() {
    // The hash is computed on luma, so colourising barely moves it. Tools do
    // not all use the same luma weights, so a few bits of drift is expected
    // and the threshold covers it. The ring signature is what separates the
    // two, and only when the setting asks it to.
    let image = scene(160, 100);
    let gray = DynamicImage::ImageRgb8(image.clone()).into_luma8();
    let gray_rgb = DynamicImage::ImageLuma8(gray).to_rgb8();
    let apart = distance(&image, &gray_rgb);
    assert!(
        apart <= within(SAME_PICTURE),
        "grayscale moved the hash by {apart} of {HASH_BITS} bits"
    );
}

#[test]
fn the_ring_signature_separates_colour_from_grayscale() {
    let image = scene(160, 100);
    let gray = DynamicImage::ImageRgb8(image.clone()).into_luma8();
    let gray_rgb = DynamicImage::ImageLuma8(gray).to_rgb8();
    let colour = ring_stats(&image);
    let mono = ring_stats(&gray_rgb);
    assert!(ring_distance(&colour, &mono) > ring_distance(&colour, &colour));
    assert_eq!(ring_distance(&colour, &colour), 0.0);
}

#[test]
fn the_ring_signature_survives_rotation() {
    let image = scene(160, 100);
    let rotated = imageops::rotate90(&image);
    let distance = ring_distance(&ring_stats(&image), &ring_stats(&rotated));
    assert!(
        distance < 0.01,
        "rotation moved the ring signature by {distance}"
    );
}

#[test]
fn the_ring_signature_survives_rescaling() {
    let image = scene(320, 200);
    let smaller = imageops::thumbnail(&image, 160, 100);
    let distance = ring_distance(&ring_stats(&image), &ring_stats(&smaller));
    assert!(
        distance < 0.02,
        "rescaling moved the ring signature by {distance}"
    );
}

fn flip(hash: &Hash, bit: usize) -> Hash {
    let mut out = *hash;
    out[bit / 8] ^= 1 << (bit % 8);
    out
}

#[test]
fn bands_reassemble_into_the_hash() {
    let hash = hash_of(&scene(96, 72));
    let parts = bands(&hash);
    let mut rebuilt = [0u8; HASH_BYTES];
    for (index, value) in parts.iter().enumerate() {
        rebuilt[index * 2..index * 2 + 2].copy_from_slice(&value.to_le_bytes());
    }
    assert_eq!(rebuilt, hash);
}

#[test]
fn hashes_within_the_band_bound_share_a_band() {
    // The pigeonhole argument the candidate query rests on: differing in at
    // most BANDS - 1 bits leaves at least one band untouched.
    let base = hash_of(&scene(96, 72));
    for first in (0..HASH_BITS).step_by(3) {
        for second in (0..HASH_BITS).step_by(7) {
            let other = flip(&flip(&base, first), second);
            if hamming(&base, &other) > (BANDS - 1) as u32 {
                continue;
            }
            let shared = bands(&base)
                .iter()
                .zip(bands(&other).iter())
                .any(|(a, b)| a == b);
            assert!(
                shared,
                "flipping bits {first} and {second} left no shared band"
            );
        }
    }
}

#[test]
fn a_band_is_wide_enough_to_be_selective() {
    // The measured reason the first version did not finish: an 8-bit band has
    // 256 buckets, and tens of thousands of images put hundreds in each, which
    // turns the candidate lookup into a scan.
    assert!(
        BAND_BITS >= 16,
        "a {BAND_BITS}-bit band gives only {} buckets",
        1usize << BAND_BITS
    );
    assert_eq!(BANDS * BAND_BITS, HASH_BITS + 1);
}

#[test]
fn the_hash_leaves_the_unused_bit_clear() {
    let hash = hash_of(&scene(64, 64));
    let top = hash[HASH_BYTES - 1] >> 7;
    assert_eq!(top, 0, "hash used more than {HASH_BITS} bits");
}

#[test]
fn the_ring_signature_records_a_colour_for_a_flat_image() {
    // Grey has no chroma and a saturated colour does, which is the difference
    // the confirmation step turns on.
    let red = ring_stats(&RgbImage::from_pixel(64, 64, image::Rgb([200, 30, 30])));
    let grey = ring_stats(&RgbImage::from_pixel(64, 64, image::Rgb([128, 128, 128])));
    assert!(
        ring_distance(&red, &grey) > 0.05,
        "a flat red read as a flat grey"
    );
}

#[test]
fn ring_distance_rejects_mismatched_signatures() {
    assert_eq!(ring_distance(&[0, 0, 0, 0], &[0, 0]), f32::MAX);
}

/// The search compares in words and against pre-weighted floats. Both have to
/// give the same answers as the byte versions they replace, or a folder gets
/// a different set of duplicates depending on which one ran.
#[test]
fn comparing_in_words_gives_what_comparing_in_bytes_gives() {
    let mut a = [0u8; HASH_BYTES];
    let mut b = [0u8; HASH_BYTES];
    for index in 0..HASH_BYTES {
        a[index] = (index as u8).wrapping_mul(37);
        b[index] = (index as u8).wrapping_mul(91) ^ 0x5A;
    }
    assert_eq!(hamming_words(&words(&a), &words(&b)), hamming(&a, &b));
    assert_eq!(hamming_words(&words(&a), &words(&a)), 0);

    let variants = [a, b, a, b, a, b, a, b];
    let word_variants = [
        words(&a),
        words(&b),
        words(&a),
        words(&b),
        words(&a),
        words(&b),
        words(&a),
        words(&b),
    ];
    assert_eq!(
        hamming_any_words(&word_variants, &words(&b), 0),
        hamming_any(&variants, &b)
    );
}

/// Stopping early is only allowed to change how long it takes, never what it
/// reports: inside the limit the distance is exact, outside it the smallest.
#[test]
fn stopping_early_still_reports_a_distance_the_threshold_can_judge() {
    let zero = [0u8; HASH_BYTES];
    let mut one_bit = zero;
    one_bit[0] = 1;
    let mut far = [0xFFu8; HASH_BYTES];
    far[0] = 0xFE;

    let variants = [far, far, far, one_bit, far, far, far, far];
    let word_variants = variants.map(|hash| words(&hash));

    assert_eq!(hamming_any_words(&word_variants, &words(&zero), 4), 1);
    assert!(
        hamming_any_words(&word_variants, &words(&zero), 0) <= 1,
        "the nearest variant was not reported when nothing was inside the limit"
    );
}

#[test]
fn a_pre_weighted_signature_measures_the_same_distance() {
    let red = ring_stats(&RgbImage::from_pixel(64, 64, image::Rgb([200, 30, 30])));
    let grey = ring_stats(&RgbImage::from_pixel(64, 64, image::Rgb([128, 128, 128])));
    let plain = ring_distance(&red, &grey);
    let weighted = ring_distance_weighted(&ring_weighted(&red), &ring_weighted(&grey));
    assert!(
        (plain - weighted).abs() < 1e-5,
        "bytes gave {plain} and weighted floats gave {weighted}"
    );
    assert_eq!(ring_distance_weighted(&ring_weighted(&red), &[]), f32::MAX);
}
