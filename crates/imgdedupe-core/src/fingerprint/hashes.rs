use super::*;

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
pub(super) fn grid(small: &RgbImage) -> Vec<f32> {
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
pub(super) fn variant_hashes(grid: &[f32]) -> [Hash; VARIANTS] {
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
