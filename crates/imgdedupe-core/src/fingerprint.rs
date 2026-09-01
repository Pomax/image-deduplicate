use image::{imageops, GrayImage, RgbImage};

use crate::decode::Decoded;

/// Bumping this re-runs fingerprinting for every row computed by an older version,
/// and leaves `files` and `images` alone.
pub const FINGERPRINT_VERSION: i64 = 1;

/// Side of the square the perceptual hash is computed on.
const GRID: usize = 64;
/// Side of the low-frequency DCT block the hash bits come from. A quarter of the
/// grid, so the frequencies the hash sees do not change with the grid size.
const BLOCK: usize = 16;
/// The block minus the DC term.
pub const HASH_BITS: usize = BLOCK * BLOCK - 1;
/// Bytes one hash occupies.
pub const HASH_BYTES: usize = (BLOCK * BLOCK) / 8;

/// A perceptual hash. Long enough that the bands below can be wide enough to be
/// selective; see `BAND_BITS`.
pub type Hash = [u8; HASH_BYTES];

/// Concentric rings sampled inside the inscribed circle.
const RINGS: usize = 12;
/// Mean L, mean a, mean b and the standard deviation of L, per ring.
const RING_VALUES: usize = 4;

/// The eight symmetries of the square: four rotations, each with and without a mirror.
pub const VARIANTS: usize = 8;

pub struct Fingerprint {
    /// The hash of the image as it sits. Everything is indexed against this one.
    pub dct_hash: Hash,
    /// That hash and the seven for its rotations and mirrors, in a fixed order.
    pub dct_hashes: [Hash; VARIANTS],
    pub ring_stats: Vec<u8>,
}

pub fn fingerprint(decoded: &Decoded) -> Fingerprint {
    let dct_hashes = variant_hashes(&grid(&decoded.small));
    Fingerprint {
        dct_hash: dct_hashes[0],
        dct_hashes,
        ring_stats: ring_stats(&decoded.small),
    }
}

/// Pack the eight hashes for storage.
pub fn pack_hashes(hashes: &[Hash; VARIANTS]) -> Vec<u8> {
    hashes.iter().flatten().copied().collect()
}

pub fn unpack_hashes(bytes: &[u8]) -> Option<[Hash; VARIANTS]> {
    if bytes.len() != VARIANTS * HASH_BYTES {
        return None;
    }
    let mut out = [[0u8; HASH_BYTES]; VARIANTS];
    for (slot, chunk) in out.iter_mut().zip(bytes.chunks_exact(HASH_BYTES)) {
        *slot = chunk.try_into().ok()?;
    }
    Some(out)
}

pub fn unpack_hash(bytes: &[u8]) -> Option<Hash> {
    bytes.try_into().ok()
}

pub fn hamming(a: &Hash, b: &Hash) -> u32 {
    a.iter().zip(b.iter()).map(|(x, y)| (x ^ y).count_ones()).sum()
}

/// How far apart two images are, allowing either to be a rotation or a mirror of
/// the other. One side contributes all eight of its hashes and the other only the
/// one it was indexed under, which is enough: if B is a symmetry of A then one of
/// A's eight is B's.
pub fn hamming_any(hashes: &[Hash; VARIANTS], other: &Hash) -> u32 {
    hashes.iter().map(|hash| hamming(hash, other)).min().unwrap_or(u32::MAX)
}

/// Bits per band.
///
/// This is the number that decides whether the candidate lookup is a lookup or a
/// scan. A band with `w` bits has `2^w` buckets, and with `n` indexed hashes each
/// bucket holds `n / 2^w`. At 8 bits and tens of thousands of images that is
/// hundreds per bucket and the join degenerates into comparing everything to
/// everything: measured at 27,500 files it produced around 190 million candidate
/// pairs and did not finish. At 16 bits the same corpus averages under four per
/// bucket. The band width has to sit above `log2(n)`, and 16 bits covers folders
/// into the hundreds of thousands.
pub const BAND_BITS: usize = 16;
/// Bands the hash divides into. Two hashes differing in at most `BANDS - 1` bits
/// must agree exactly on at least one band, which is what makes the candidate
/// lookup an indexed equality join.
pub const BANDS: usize = HASH_BITS.div_ceil(BAND_BITS);

pub fn bands(hash: &Hash) -> [u16; BANDS] {
    let mut out = [0u16; BANDS];
    for (index, slot) in out.iter_mut().enumerate() {
        let byte = index * (BAND_BITS / 8);
        *slot = u16::from_le_bytes([hash[byte], hash[byte + 1]]);
    }
    out
}

/// Rotate the image to landscape before reducing it to a square grid.
///
/// Reducing 4000x3000 to 32x32 scales the axes by different factors, and the same
/// picture rotated 90 degrees gets those factors the other way round, so the two
/// grids are not rotations of each other and no comparison of them finds anything.
/// Making every image landscape first gives a rotated pair the same factors.
/// Reduce to a fixed square of luma samples.
///
/// Resampling onto a square of a fixed size is what makes the hash both scale and
/// aspect independent, and rotation passes straight through it: resampling a
/// rotated image onto the square gives the rotation of the resampled square.
/// There is no orientation to correct for beforehand.
fn grid(small: &RgbImage) -> Vec<f32> {
    let gray = to_luma(small);
    let square = imageops::thumbnail(&gray, GRID as u32, GRID as u32);
    square.pixels().map(|p| p.0[0] as f32).collect()
}

/// Rec. 709 luma, which is what the tools that produce a grayscale copy use, so
/// a colourised copy and its grayscale original reduce to nearly the same grid.
fn to_luma(image: &RgbImage) -> GrayImage {
    GrayImage::from_fn(image.width(), image.height(), |x, y| {
        let p = image.get_pixel(x, y).0;
        let luma = 0.2126 * p[0] as f32 + 0.7152 * p[1] as f32 + 0.0722 * p[2] as f32;
        image::Luma([luma.round().clamp(0.0, 255.0) as u8])
    })
}

/// Hash the image and each of its seven rotations and mirrors.
///
/// Rotation invariance could instead be a canonical orientation, one hash per
/// image and an eighth of the index. Every way of choosing that orientation is a
/// hard decision on a continuous quantity, whether it is the numerically smallest
/// of the eight hashes or a comparison of low-frequency coefficients. Resizing an
/// image by a non-integral factor perturbs those quantities, so the decision
/// flips, and a flipped decision does not move the hash by a few bits: it
/// replaces it. Storing all eight removes the decision, and a rotated pair then
/// matches exactly rather than usually.
///
/// It costs one DCT, not eight: the symmetries of the image are sign changes and
/// a transpose on the coefficients.
fn variant_hashes(grid: &[f32]) -> [Hash; VARIANTS] {
    let block = dct_low_block(grid);
    let mut out = [[0u8; HASH_BYTES]; VARIANTS];
    for (variant, slot) in out.iter_mut().enumerate() {
        let mut moved = if variant & 4 != 0 { transpose_block(&block) } else { block };
        if variant & 1 != 0 {
            mirror_horizontal(&mut moved);
        }
        if variant & 2 != 0 {
            mirror_vertical(&mut moved);
        }
        *slot = hash_block(&moved);
    }
    out
}

fn transpose_block(block: &[f32; BLOCK * BLOCK]) -> [f32; BLOCK * BLOCK] {
    let mut out = [0.0f32; BLOCK * BLOCK];
    for u in 0..BLOCK {
        for v in 0..BLOCK {
            out[u * BLOCK + v] = block[v * BLOCK + u];
        }
    }
    out
}

fn mirror_horizontal(block: &mut [f32; BLOCK * BLOCK]) {
    for u in 0..BLOCK {
        for v in (1..BLOCK).step_by(2) {
            block[u * BLOCK + v] = -block[u * BLOCK + v];
        }
    }
}

fn mirror_vertical(block: &mut [f32; BLOCK * BLOCK]) {
    for u in (1..BLOCK).step_by(2) {
        for v in 0..BLOCK {
            block[u * BLOCK + v] = -block[u * BLOCK + v];
        }
    }
}

fn hash_block(block: &[f32; BLOCK * BLOCK]) -> Hash {
    // The DC term carries overall brightness, not structure, so it is not hashed
    // and does not sit in the median.
    let mut coefficients: Vec<f32> = block[1..].to_vec();
    let median = median_of(&mut coefficients);

    let mut hash = [0u8; HASH_BYTES];
    for (bit, value) in block[1..].iter().enumerate() {
        if *value > median {
            hash[bit / 8] |= 1 << (bit % 8);
        }
    }
    hash
}

fn median_of(values: &mut [f32]) -> f32 {
    values.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let mid = values.len() / 2;
    if values.len() % 2 == 0 {
        (values[mid - 1] + values[mid]) / 2.0
    } else {
        values[mid]
    }
}

/// Only the top-left 8x8 of the DCT is ever read, so only that is computed: the
/// row pass produces 8 columns and the column pass 8 rows, not 32 of each.
fn dct_low_block(grid: &[f32]) -> [f32; BLOCK * BLOCK] {
    let basis = cosine_basis();

    let mut rows = [0.0f32; GRID * BLOCK];
    for y in 0..GRID {
        for v in 0..BLOCK {
            let mut sum = 0.0;
            for x in 0..GRID {
                sum += grid[y * GRID + x] * basis[v * GRID + x];
            }
            rows[y * BLOCK + v] = sum;
        }
    }

    let mut out = [0.0f32; BLOCK * BLOCK];
    for u in 0..BLOCK {
        for v in 0..BLOCK {
            let mut sum = 0.0;
            for y in 0..GRID {
                sum += rows[y * BLOCK + v] * basis[u * GRID + y];
            }
            out[u * BLOCK + v] = sum;
        }
    }
    out
}

/// `basis[k * GRID + n]` is the normalised DCT-II coefficient for frequency k at
/// sample n. Eight frequencies over thirty-two samples, built once.
fn cosine_basis() -> &'static [f32; BLOCK * GRID] {
    use std::sync::OnceLock;
    static BASIS: OnceLock<[f32; BLOCK * GRID]> = OnceLock::new();
    BASIS.get_or_init(|| {
        let mut table = [0.0f32; BLOCK * GRID];
        let n = GRID as f32;
        for k in 0..BLOCK {
            let alpha = if k == 0 { (1.0 / n).sqrt() } else { (2.0 / n).sqrt() };
            for sample in 0..GRID {
                let angle = std::f32::consts::PI * (2.0 * sample as f32 + 1.0) * k as f32 / (2.0 * n);
                table[k * GRID + sample] = alpha * angle.cos();
            }
        }
        table
    })
}

/// Rings are rotation invariant only inside the largest circle that fits in the
/// image. Past that radius a ring is clipped by the long edges, and a 90 degree
/// rotation clips it on the other axis, so the two disagree. Sampling stops at
/// `min(w, h) / 2` and the corners are simply not looked at.
fn ring_stats(small: &RgbImage) -> Vec<u8> {
    let (w, h) = (small.width() as f32, small.height() as f32);
    let (cx, cy) = ((w - 1.0) / 2.0, (h - 1.0) / 2.0);
    let radius = (w.min(h) / 2.0).max(1.0);

    let mut sums = [[0.0f64; 4]; RINGS];
    let mut counts = [0u32; RINGS];

    for (x, y, pixel) in small.enumerate_pixels() {
        let dx = x as f32 - cx;
        let dy = y as f32 - cy;
        let distance = (dx * dx + dy * dy).sqrt();
        if distance > radius {
            continue;
        }
        let ring = ((distance / radius) * RINGS as f32) as usize;
        let ring = ring.min(RINGS - 1);
        let (l, a, b) = oklab(pixel.0);
        sums[ring][0] += l as f64;
        sums[ring][1] += a as f64;
        sums[ring][2] += b as f64;
        sums[ring][3] += (l * l) as f64;
        counts[ring] += 1;
    }

    let mut out = Vec::with_capacity(RINGS * RING_VALUES * 4);
    for ring in 0..RINGS {
        let (mean_l, mean_a, mean_b, sd_l) = if counts[ring] == 0 {
            (0.0, 0.0, 0.0, 0.0)
        } else {
            let n = counts[ring] as f64;
            let mean_l = sums[ring][0] / n;
            let variance = (sums[ring][3] / n - mean_l * mean_l).max(0.0);
            (mean_l as f32, (sums[ring][1] / n) as f32, (sums[ring][2] / n) as f32, variance.sqrt() as f32)
        };
        for value in [mean_l, mean_a, mean_b, sd_l] {
            out.extend_from_slice(&value.to_le_bytes());
        }
    }
    out
}

/// Euclidean distance between two ring signatures, with the colour axes weighted
/// up because a recompression moves lightness far more than it moves hue.
pub fn ring_distance(a: &[u8], b: &[u8]) -> f32 {
    if a.len() != b.len() || a.len() % 4 != 0 {
        return f32::MAX;
    }
    let mut total = 0.0f32;
    for (index, (pa, pb)) in a.chunks_exact(4).zip(b.chunks_exact(4)).enumerate() {
        let va = f32::from_le_bytes([pa[0], pa[1], pa[2], pa[3]]);
        let vb = f32::from_le_bytes([pb[0], pb[1], pb[2], pb[3]]);
        let weight = match index % RING_VALUES {
            0 => 1.0,
            1 | 2 => 4.0,
            _ => 1.0,
        };
        let delta = (va - vb) * weight;
        total += delta * delta;
    }
    (total / (RINGS * RING_VALUES) as f32).sqrt()
}

/// sRGB to Oklab. The transfer function is a 256-entry table because there are
/// only 256 possible inputs; the three cube roots that follow are what this costs
/// and are why it is called once per pixel and not once per statistic.
fn oklab(rgb: [u8; 3]) -> (f32, f32, f32) {
    let linear = linear_table();
    let (r, g, b) = (linear[rgb[0] as usize], linear[rgb[1] as usize], linear[rgb[2] as usize]);

    let l = 0.4122214708 * r + 0.5363325363 * g + 0.0514459929 * b;
    let m = 0.2119034982 * r + 0.6806995451 * g + 0.1073969566 * b;
    let s = 0.0883024619 * r + 0.2817188376 * g + 0.6299787005 * b;

    let l = l.cbrt();
    let m = m.cbrt();
    let s = s.cbrt();

    (
        0.2104542553 * l + 0.7936177850 * m - 0.0040720468 * s,
        1.9779984951 * l - 2.4285922050 * m + 0.4505937099 * s,
        0.0259040371 * l + 0.7827717662 * m - 0.8086757660 * s,
    )
}

fn linear_table() -> &'static [f32; 256] {
    use std::sync::OnceLock;
    static TABLE: OnceLock<[f32; 256]> = OnceLock::new();
    TABLE.get_or_init(|| {
        let mut table = [0.0f32; 256];
        for (value, slot) in table.iter_mut().enumerate() {
            let c = value as f32 / 255.0;
            *slot = if c <= 0.04045 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) };
        }
        table
    })
}

#[cfg(test)]
mod tests {
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
        fingerprint(&decoded_from(image)).dct_hash
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
            a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).sum::<f32>() / (GRID * GRID) as f32
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
        let apart = hamming_any(&hashes_of(&image), &fingerprint(&decoded).dct_hash);
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
        assert!(distance < 0.01, "rotation moved the ring signature by {distance}");
    }

    #[test]
    fn the_ring_signature_survives_rescaling() {
        let image = scene(320, 200);
        let smaller = imageops::thumbnail(&image, 160, 100);
        let distance = ring_distance(&ring_stats(&image), &ring_stats(&smaller));
        assert!(distance < 0.02, "rescaling moved the ring signature by {distance}");
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
                let shared = bands(&base).iter().zip(bands(&other).iter()).any(|(a, b)| a == b);
                assert!(shared, "flipping bits {first} and {second} left no shared band");
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
        assert!(ring_distance(&red, &grey) > 0.05, "a flat red read as a flat grey");
    }

    #[test]
    fn ring_distance_rejects_mismatched_signatures() {
        assert_eq!(ring_distance(&[0, 0, 0, 0], &[0, 0]), f32::MAX);
    }
}
