use super::*;

pub fn hamming(a: &Hash, b: &Hash) -> u32 {
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x ^ y).count_ones())
        .sum()
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
