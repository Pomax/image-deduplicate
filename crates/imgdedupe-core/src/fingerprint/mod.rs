use image::{imageops, GrayImage, RgbImage};

use crate::decode::Decoded;

mod colour;
mod distance;
mod hashes;

use self::colour::*;
pub use self::distance::*;
use self::hashes::*;

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

#[cfg(test)]
#[path = "../tests/fingerprint.rs"]
mod tests;
