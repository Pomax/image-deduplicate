use super::*;

/// Describe the neighbourhood as 256 yes-or-no answers: for each of 256 fixed
/// pairs of places in the patch, is the first brighter than the second.
///
/// The pairs are turned by the corner's own angle before they are read, so the
/// same neighbourhood gives the same answers however the picture is turned.
pub(super) fn describe(picture: &GrayImage, x: u32, y: u32, angle: f32) -> [u8; DESCRIPTOR_BYTES] {
    let (width, height) = picture.dimensions();
    let (sin, cos) = angle.sin_cos();
    let mut out = [0u8; DESCRIPTOR_BYTES];

    for (index, (ax, ay, bx, by)) in PATTERN.iter().enumerate() {
        let turn = |dx: i32, dy: i32| {
            let (dx, dy) = (dx as f32, dy as f32);
            let rx = (dx * cos - dy * sin).round() as i32;
            let ry = (dx * sin + dy * cos).round() as i32;
            (
                (x as i32 + rx).clamp(0, width as i32 - 1) as u32,
                (y as i32 + ry).clamp(0, height as i32 - 1) as u32,
            )
        };
        let (first_x, first_y) = turn(*ax, *ay);
        let (second_x, second_y) = turn(*bx, *by);
        let first = picture.get_pixel(first_x, first_y).0[0];
        let second = picture.get_pixel(second_x, second_y).0[0];
        if first > second {
            out[index / 8] |= 1 << (index % 8);
        }
    }
    out
}

/// The 256 pairs of places the description compares, laid out once and the same
/// for every picture this program ever fingerprints.
///
/// They are drawn from a fixed sequence rather than written out, so the table is
/// the sequence and not a page of numbers. Changing either changes every
/// fingerprint, which is what the fingerprint version is for.
static PATTERN: std::sync::LazyLock<[(i32, i32, i32, i32); DESCRIPTOR_BITS]> =
    std::sync::LazyLock::new(|| {
        let mut state = 0x2545_F491_4F6C_DD1Du64;
        let mut next = || {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            // Inside the patch, and away from its very edge so a turn of the
            // pattern keeps it inside.
            (state % (PATCH as u64 * 2 - 5)) as i32 - PATCH + 2
        };
        std::array::from_fn(|_| (next(), next(), next(), next()))
    });

/// How different two descriptions are, in bits.
pub fn distance(one: &[u8; DESCRIPTOR_BYTES], other: &[u8; DESCRIPTOR_BYTES]) -> u32 {
    one.iter()
        .zip(other.iter())
        .map(|(a, b)| (a ^ b).count_ones())
        .sum()
}
