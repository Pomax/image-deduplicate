//! A feature fingerprint: what is in the picture, and where, rather than what
//! the whole frame averages out to.
//!
//! The perceptual hash beside this one squashes the entire frame onto one square
//! and describes that. It is exact about a picture that fills the frame the same
//! way and blind to a crop, because cropping stretches a different region over
//! the same square and every number changes at once.
//!
//! This finds corners in the picture, describes the neighbourhood of each one,
//! and stores those. A crop keeps the corners that are still in it, and they are
//! described the same way whatever else was cut off, so two pictures are the same
//! picture when enough of their corners agree and agree in the same arrangement.
//!
//! Every part of it looks at one picture on its own. Nothing is learned from a
//! folder, nothing is trained, and the fingerprint of a file does not depend on
//! what else was scanned with it.

use image::{imageops, GrayImage};

/// Long edge the picture is decoded to before corners are looked for.
///
/// The hash needs 64 pixels and takes 128; corners need enough pixels to have a
/// neighbourhood worth describing. Half of a 512 pixel picture is still 256,
/// which holds up.
pub const FEATURE_EDGE: u32 = 512;

/// Corners kept per picture.
///
/// A landscape has thousands and the strongest of them spread over the frame is
/// what a crop keeps some of. How many to keep is a trade against the size of
/// the index: a third of a picture is a third of its corners, and of those only
/// the ones the crop also picked as its own strongest are found again, so a
/// budget of a hundred leaves a crop and its original agreeing on ten-odd
/// corners, which is inside the noise. Three hundred puts it comfortably clear
/// and costs eleven kilobytes a picture.
pub const KEYPOINTS: usize = 320;

/// Bits in one corner's description.
pub const DESCRIPTOR_BITS: usize = 256;
pub const DESCRIPTOR_BYTES: usize = DESCRIPTOR_BITS / 8;

/// Levels of the pyramid, each this much smaller than the one before it. A crop
/// enlarged to the same size as the original shows its corners at a different
/// size, and the level they are found on absorbs that.
///
/// The steps are close together on purpose: whatever the size difference between
/// two pictures, some level of one is within half a step of some level of the
/// other, and half of a small step is a small error. At 1.2 the worst case is a
/// tenth, which the descriptions below shrug off; at 1.4 it is a fifth, which
/// measured as the difference between finding a crop and missing it.
const LEVELS: usize = 8;
const LEVEL_SCALE: f32 = 1.2;

/// How much brighter or darker than the middle the arc has to be.
const FAST_THRESHOLD: i16 = 18;
/// Pixels of the sixteen around a corner that have to agree.
const FAST_ARC: usize = 9;
/// Radius of the patch a description is sampled from.
const PATCH: i32 = 15;

/// One corner: where it is in the picture, and what it looks like.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Keypoint {
    /// Where the corner sits in the decoded picture, in pixels.
    pub x: u16,
    pub y: u16,
    pub descriptor: [u8; DESCRIPTOR_BYTES],
}

/// Bytes one stored corner occupies: where it is, then what it looks like.
pub const KEYPOINT_BYTES: usize = 4 + DESCRIPTOR_BYTES;

/// Find the corners of a picture and describe them.
pub fn features(picture: &GrayImage) -> Vec<Keypoint> {
    let mut found: Vec<(u32, Keypoint)> = Vec::new();
    let mut level = picture.clone();
    let mut scale = 1.0f32;

    for step in 0..LEVELS {
        if level.width() < 2 * PATCH as u32 || level.height() < 2 * PATCH as u32 {
            break;
        }
        // Corners are found on the picture as it is, and described from a
        // smoothed copy of it. A description is 256 comparisons of one pixel
        // against another, and single pixels differ between two copies of a
        // picture for reasons that have nothing to do with what is in it:
        // compression, resampling, the sensor. Smoothing first is what makes the
        // answers the same for both.
        let smoothed = smooth(&level);
        for (x, y, score) in corners(&level) {
            let Some(angle) = orientation(&level, x, y) else {
                continue;
            };
            let descriptor = describe(&smoothed, x, y, angle);
            let point = Keypoint {
                x: (x as f32 * scale).round() as u16,
                y: (y as f32 * scale).round() as u16,
                descriptor,
            };
            found.push((score, point));
        }
        if step + 1 == LEVELS {
            break;
        }
        let (width, height) = (
            (level.width() as f32 / LEVEL_SCALE) as u32,
            (level.height() as f32 / LEVEL_SCALE) as u32,
        );
        if width < 2 * PATCH as u32 || height < 2 * PATCH as u32 {
            break;
        }
        level = imageops::resize(&level, width, height, imageops::FilterType::Triangle);
        scale *= LEVEL_SCALE;
    }

    strongest(found, picture.width(), picture.height())
}

/// Average each pixel with the eight around it, twice, which is close enough to
/// a gaussian for this and costs four passes over the picture.
fn smooth(picture: &GrayImage) -> GrayImage {
    let once = box_blur(picture);
    box_blur(&once)
}

fn box_blur(picture: &GrayImage) -> GrayImage {
    let (width, height) = picture.dimensions();
    let across = GrayImage::from_fn(width, height, |x, y| {
        let left = picture.get_pixel(x.saturating_sub(1), y).0[0] as u16;
        let middle = picture.get_pixel(x, y).0[0] as u16;
        let right = picture.get_pixel((x + 1).min(width - 1), y).0[0] as u16;
        image::Luma([((left + middle + right) / 3) as u8])
    });
    GrayImage::from_fn(width, height, |x, y| {
        let up = across.get_pixel(x, y.saturating_sub(1)).0[0] as u16;
        let middle = across.get_pixel(x, y).0[0] as u16;
        let down = across.get_pixel(x, (y + 1).min(height - 1)).0[0] as u16;
        image::Luma([((up + middle + down) / 3) as u8])
    })
}

/// The strongest corners, spread over the picture.
///
/// Taking the strongest outright gives every one of them to whichever part of the
/// picture has the most texture, and a crop of any other part matches nothing.
/// The frame is divided into cells and each cell keeps its own best.
fn strongest(mut found: Vec<(u32, Keypoint)>, width: u32, height: u32) -> Vec<Keypoint> {
    const CELLS: u32 = 8;
    found.sort_by(|a, b| b.0.cmp(&a.0));
    let (cell_w, cell_h) = ((width / CELLS).max(1), (height / CELLS).max(1));
    let per_cell = KEYPOINTS / (CELLS * CELLS) as usize + 1;

    let mut taken = std::collections::HashMap::<(u32, u32), usize>::new();
    let mut out = Vec::with_capacity(KEYPOINTS);
    for (_, point) in &found {
        if out.len() >= KEYPOINTS {
            break;
        }
        let cell = (point.x as u32 / cell_w, point.y as u32 / cell_h);
        let count = taken.entry(cell).or_insert(0);
        if *count >= per_cell {
            continue;
        }
        *count += 1;
        out.push(*point);
    }
    // A picture with texture in one corner and flat sky everywhere else fills its
    // cells and stops short of the budget. The rest of the strongest fill it.
    if out.len() < KEYPOINTS {
        for (_, point) in &found {
            if out.len() >= KEYPOINTS {
                break;
            }
            if !out.contains(point) {
                out.push(*point);
            }
        }
    }
    out
}

/// The sixteen pixels of the circle a corner is decided by, in order around it.
const CIRCLE: [(i32, i32); 16] = [
    (0, -3),
    (1, -3),
    (2, -2),
    (3, -1),
    (3, 0),
    (3, 1),
    (2, 2),
    (1, 3),
    (0, 3),
    (-1, 3),
    (-2, 2),
    (-3, 1),
    (-3, 0),
    (-3, -1),
    (-2, -2),
    (-1, -3),
];

/// Corners by the FAST test: a pixel is one when nine of the sixteen around it,
/// in a row, are all brighter than it or all darker than it.
///
/// The score is how much brighter or darker, summed, which is what orders one
/// corner against another.
fn corners(picture: &GrayImage) -> Vec<(u32, u32, u32)> {
    let (width, height) = (picture.dimensions().0 as i32, picture.dimensions().1 as i32);
    let mut out = Vec::new();
    let at = |x: i32, y: i32| -> i16 { picture.get_pixel(x as u32, y as u32).0[0] as i16 };

    for y in PATCH..height - PATCH {
        for x in PATCH..width - PATCH {
            let middle = at(x, y);
            let ring: [i16; 16] = std::array::from_fn(|index| {
                let (dx, dy) = CIRCLE[index];
                at(x + dx, y + dy)
            });

            // The quick rejection the test is built around: four pixels at the
            // compass points, of which three have to agree before the rest are
            // worth looking at.
            let bright = |value: i16| value > middle + FAST_THRESHOLD;
            let dark = |value: i16| value < middle - FAST_THRESHOLD;
            let compass = [ring[0], ring[4], ring[8], ring[12]];
            if compass.iter().filter(|value| bright(**value)).count() < 3
                && compass.iter().filter(|value| dark(**value)).count() < 3
            {
                continue;
            }
            if !(arc(&ring, bright) || arc(&ring, dark)) {
                continue;
            }
            let score: u32 = ring.iter().map(|value| (value - middle).unsigned_abs() as u32).sum();
            out.push((x as u32, y as u32, score));
        }
    }
    suppress(out, picture.width())
}

/// Whether nine of the sixteen in a row satisfy the test.
fn arc(ring: &[i16; 16], test: impl Fn(i16) -> bool) -> bool {
    let mut run = 0;
    for index in 0..16 + FAST_ARC {
        if test(ring[index % 16]) {
            run += 1;
            if run >= FAST_ARC {
                return true;
            }
        } else {
            run = 0;
        }
    }
    false
}

/// One corner out of every cluster of them: the strongest, with everything within
/// three pixels of it dropped. Without this an edge produces a corner per pixel
/// along it and they crowd out the rest of the picture.
fn suppress(found: Vec<(u32, u32, u32)>, width: u32) -> Vec<(u32, u32, u32)> {
    let mut best: std::collections::HashMap<(u32, u32), (u32, u32, u32)> =
        std::collections::HashMap::new();
    for (x, y, score) in found {
        let cell = (x / 4, y / 4);
        match best.get(&cell) {
            Some((_, _, held)) if *held >= score => {}
            _ => {
                best.insert(cell, (x, y, score));
            }
        }
    }
    let mut out: Vec<(u32, u32, u32)> = best.into_values().collect();
    // A stable order, so the same picture gives the same fingerprint twice.
    out.sort_by_key(|(x, y, _)| *y * width + *x);
    out
}

/// Which way the corner faces, from where its weight sits: the angle from the
/// middle of the patch to the patch's centre of intensity. A picture and the same
/// picture turned round give the same description because the pattern below is
/// turned with it.
fn orientation(picture: &GrayImage, x: u32, y: u32) -> Option<f32> {
    let (width, height) = picture.dimensions();
    if x < PATCH as u32 || y < PATCH as u32 {
        return None;
    }
    if x + PATCH as u32 >= width || y + PATCH as u32 >= height {
        return None;
    }
    let (mut moment_x, mut moment_y) = (0i64, 0i64);
    for dy in -PATCH..=PATCH {
        for dx in -PATCH..=PATCH {
            if dx * dx + dy * dy > PATCH * PATCH {
                continue;
            }
            let value =
                picture.get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32).0[0] as i64;
            moment_x += dx as i64 * value;
            moment_y += dy as i64 * value;
        }
    }
    Some((moment_y as f32).atan2(moment_x as f32))
}

/// Describe the neighbourhood as 256 yes-or-no answers: for each of 256 fixed
/// pairs of places in the patch, is the first brighter than the second.
///
/// The pairs are turned by the corner's own angle before they are read, so the
/// same neighbourhood gives the same answers however the picture is turned.
fn describe(picture: &GrayImage, x: u32, y: u32, angle: f32) -> [u8; DESCRIPTOR_BYTES] {
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
    one.iter().zip(other.iter()).map(|(a, b)| (a ^ b).count_ones()).sum()
}

/// The most bits two descriptions may differ in and still be the same corner.
/// A quarter of them: measured, corners of the same place in two copies of a
/// picture land under 60 and corners of different places sit around 128, which
/// is what two unrelated strings of bits do.
const SAME_CORNER: u32 = 64;

/// How much better the best match has to be than the second best, in tenths.
/// A corner that matches two places equally well matches neither: repeated
/// texture produces exactly that, and it is the usual way a match is wrong.
const CLEARLY_BETTER: u32 = 8;

/// Pixels a corner may sit away from where the arrangement says it should be.
const ROOM: f32 = 6.0;

/// Corners that have to agree, in the same arrangement, before two pictures are
/// the same picture.
///
/// Measured on photographs: 190 pairs of unrelated ones never got past ten and
/// mostly sat at zero, a picture and a sixty percent crop of it agreed on
/// thirty-seven, and frames of the same scene seconds apart on sixty to two
/// hundred. Sixteen sits between the two with room either side.
pub const AGREEING_CORNERS: u32 = 16;

/// How many corners of one picture are corners of the other, in the same
/// arrangement.
///
/// Corners are paired by description alone, which pairs some of them wrongly:
/// two windows of the same building look identical. What is left is geometry.
/// One scale, one rotation and one shift take the whole of a picture onto the
/// whole of a copy of it, or onto the part of it a crop kept, so a pair of
/// matches proposes such an arrangement and every other match votes on it. The
/// arrangement with the most votes is the answer, and the votes are the number
/// returned. Wrong pairs vote for nothing in particular and are left out.
pub fn agreement(one: &[Keypoint], other: &[Keypoint]) -> u32 {
    let pairs = paired(one, other);
    if pairs.len() < AGREEING_CORNERS as usize {
        return 0;
    }

    let mut best = 0;
    let mut state = 0x9E37_79B9_7F4A_7C15u64;
    let mut random = move || {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        state
    };

    for _ in 0..ATTEMPTS {
        let first = (random() as usize) % pairs.len();
        let second = (random() as usize) % pairs.len();
        if first == second {
            continue;
        }
        let Some(arrangement) = Arrangement::through(pairs[first], pairs[second]) else {
            continue;
        };
        let votes = pairs.iter().filter(|pair| arrangement.holds(**pair)).count() as u32;
        best = best.max(votes);
        // Nothing is going to beat every pair agreeing.
        if best as usize == pairs.len() {
            break;
        }
    }
    best
}

/// Arrangements tried. Two matches out of a hundred-odd, drawn at random: a
/// hundred draws finds one that is right long before it runs out.
const ATTEMPTS: usize = 200;

/// One corner of each picture, paired because they describe the same thing.
type Pair = ((f32, f32), (f32, f32));

fn paired(one: &[Keypoint], other: &[Keypoint]) -> Vec<Pair> {
    let mut out = Vec::new();
    for point in one {
        let (mut best, mut second) = (u32::MAX, u32::MAX);
        let mut at = None;
        for candidate in other {
            let bits = distance(&point.descriptor, &candidate.descriptor);
            if bits < best {
                second = best;
                best = bits;
                at = Some(candidate);
            } else if bits < second {
                second = bits;
            }
        }
        let Some(found) = at else {
            continue;
        };
        if best > SAME_CORNER || best * 10 > second * CLEARLY_BETTER {
            continue;
        }
        out.push((
            (point.x as f32, point.y as f32),
            (found.x as f32, found.y as f32),
        ));
    }
    out
}

/// A scale, a turn and a shift: what takes one picture onto another.
#[derive(Debug, Clone, Copy)]
struct Arrangement {
    /// The scale and the turn together, as one complex number.
    turn: (f32, f32),
    shift: (f32, f32),
}

impl Arrangement {
    /// The arrangement two pairs of corners propose. `None` when they propose
    /// nothing: two corners in the same place say nothing about scale or turn,
    /// and a scale far from one is not a crop of anything.
    fn through(first: Pair, second: Pair) -> Option<Arrangement> {
        let from = (second.0 .0 - first.0 .0, second.0 .1 - first.0 .1);
        let to = (second.1 .0 - first.1 .0, second.1 .1 - first.1 .1);
        let length = from.0 * from.0 + from.1 * from.1;
        if length < 16.0 {
            return None;
        }
        // to / from, as complex numbers: the scale and the turn in one.
        let turn = (
            (to.0 * from.0 + to.1 * from.1) / length,
            (to.1 * from.0 - to.0 * from.1) / length,
        );
        let scale = (turn.0 * turn.0 + turn.1 * turn.1).sqrt();
        if !(0.2..=5.0).contains(&scale) {
            return None;
        }
        let placed = apply(turn, first.0);
        Some(Arrangement { turn, shift: (first.1 .0 - placed.0, first.1 .1 - placed.1) })
    }

    fn holds(&self, pair: Pair) -> bool {
        let placed = apply(self.turn, pair.0);
        let (dx, dy) = (placed.0 + self.shift.0 - pair.1 .0, placed.1 + self.shift.1 - pair.1 .1);
        dx * dx + dy * dy <= ROOM * ROOM
    }
}

fn apply(turn: (f32, f32), point: (f32, f32)) -> (f32, f32) {
    (turn.0 * point.0 - turn.1 * point.1, turn.1 * point.0 + turn.0 * point.1)
}

/// Pack corners for the index: position then description, one after another.
pub fn pack(points: &[Keypoint]) -> Vec<u8> {
    let mut out = Vec::with_capacity(points.len() * KEYPOINT_BYTES);
    for point in points {
        out.extend_from_slice(&point.x.to_le_bytes());
        out.extend_from_slice(&point.y.to_le_bytes());
        out.extend_from_slice(&point.descriptor);
    }
    out
}

pub fn unpack(bytes: &[u8]) -> Vec<Keypoint> {
    bytes
        .chunks_exact(KEYPOINT_BYTES)
        .map(|chunk| Keypoint {
            x: u16::from_le_bytes([chunk[0], chunk[1]]),
            y: u16::from_le_bytes([chunk[2], chunk[3]]),
            descriptor: chunk[4..].try_into().unwrap_or([0; DESCRIPTOR_BYTES]),
        })
        .collect()
}

#[cfg(test)]
mod tests {
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
}
