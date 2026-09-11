use image::{imageops, GrayImage, RgbImage};

use crate::decode::Decoded;

/// Bumping this re-runs fingerprinting for every row computed by an older version,
/// and leaves `files` and `images` alone.
///
/// 2 is the version that records the picture's corners beside the hash, which is
/// what finds a crop of a picture. A row without them was written before that and
/// is read again.
pub const FINGERPRINT_VERSION: i64 = 2;

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
    /// The image as it sits, then the seven for its rotations and mirrors, in a
    /// fixed order.
    pub dct_hashes: [Hash; VARIANTS],
    pub ring_stats: Vec<u8>,
}

pub fn fingerprint(decoded: &Decoded) -> Fingerprint {
    Fingerprint {
        dct_hashes: variant_hashes(&grid(&decoded.small)),
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
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
}

/// A hash as machine words. The search compares millions of pairs and does it a
/// word at a time rather than a byte at a time.
pub const HASH_WORDS: usize = HASH_BYTES / 8;

pub type Words = [u64; HASH_WORDS];

pub fn words(hash: &Hash) -> Words {
    let mut out = [0u64; HASH_WORDS];
    for (slot, chunk) in out.iter_mut().zip(hash.chunks_exact(8)) {
        *slot = u64::from_le_bytes(chunk.try_into().expect("eight bytes"));
    }
    out
}

pub fn hamming_words(a: &Words, b: &Words) -> u32 {
    let mut total = 0;
    for index in 0..HASH_WORDS {
        total += (a[index] ^ b[index]).count_ones();
    }
    total
}

/// `hamming_any` on words: one side's eight variants against the other's indexed
/// one, stopping as soon as a variant is inside `limit`.
pub fn hamming_any_words(variants: &[Words; VARIANTS], other: &Words, limit: u32) -> u32 {
    let mut best = u32::MAX;
    for hash in variants {
        let distance = hamming_words(hash, other);
        if distance <= limit {
            return distance;
        }
        best = best.min(distance);
    }
    best
}

/// A stored ring signature as the floats `ring_distance` would weigh, so the
/// weighting is paid once per image rather than once per comparison. An empty
/// vector stands for a signature that cannot be compared.
pub fn ring_weighted(bytes: &[u8]) -> Vec<f32> {
    if bytes.len() % 4 != 0 {
        return Vec::new();
    }
    bytes
        .chunks_exact(4)
        .enumerate()
        .map(|(index, chunk)| {
            let value = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
            value * ring_weight(index)
        })
        .collect()
}

/// `ring_distance` over what `ring_weighted` produced.
pub fn ring_distance_weighted(a: &[f32], b: &[f32]) -> f32 {
    if a.is_empty() || a.len() != b.len() {
        return f32::MAX;
    }
    let mut total = 0.0f32;
    for (x, y) in a.iter().zip(b.iter()) {
        let delta = x - y;
        total += delta * delta;
    }
    (total / (RINGS * RING_VALUES) as f32).sqrt()
}

/// The colour axes count for more than lightness: a recompression moves
/// lightness far more than it moves hue.
fn ring_weight(index: usize) -> f32 {
    match index % RING_VALUES {
        1 | 2 => 4.0,
        _ => 1.0,
    }
}

/// How far apart two images are, allowing either to be a rotation or a mirror of
/// the other. One side contributes all eight of its hashes and the other only the
/// one it was indexed under, which is enough: if B is a symmetry of A then one of
/// A's eight is B's.
pub fn hamming_any(hashes: &[Hash; VARIANTS], other: &Hash) -> u32 {
    hashes
        .iter()
        .map(|hash| hamming(hash, other))
        .min()
        .unwrap_or(u32::MAX)
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
        let mut moved = if variant & 4 != 0 {
            transpose_block(&block)
        } else {
            block
        };
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
            let alpha = if k == 0 {
                (1.0 / n).sqrt()
            } else {
                (2.0 / n).sqrt()
            };
            for sample in 0..GRID {
                let angle =
                    std::f32::consts::PI * (2.0 * sample as f32 + 1.0) * k as f32 / (2.0 * n);
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
            (
                mean_l as f32,
                (sums[ring][1] / n) as f32,
                (sums[ring][2] / n) as f32,
                variance.sqrt() as f32,
            )
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
        let delta = (va - vb) * ring_weight(index);
        total += delta * delta;
    }
    (total / (RINGS * RING_VALUES) as f32).sqrt()
}

/// sRGB to Oklab. The transfer function is a 256-entry table because there are
/// only 256 possible inputs; the three cube roots that follow are what this costs
/// and are why it is called once per pixel and not once per statistic.
fn oklab(rgb: [u8; 3]) -> (f32, f32, f32) {
    let linear = linear_table();
    let (r, g, b) = (
        linear[rgb[0] as usize],
        linear[rgb[1] as usize],
        linear[rgb[2] as usize],
    );

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
            *slot = if c <= 0.04045 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            };
        }
        table
    })
}

#[cfg(test)]
#[path = "tests/fingerprint.rs"]
mod tests;
