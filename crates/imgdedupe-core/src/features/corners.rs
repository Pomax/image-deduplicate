use super::*;

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
pub(super) fn corners(picture: &GrayImage) -> Vec<(u32, u32, u32)> {
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
            let score: u32 = ring
                .iter()
                .map(|value| (value - middle).unsigned_abs() as u32)
                .sum();
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
pub(super) fn orientation(picture: &GrayImage, x: u32, y: u32) -> Option<f32> {
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
            let value = picture
                .get_pixel((x as i32 + dx) as u32, (y as i32 + dy) as u32)
                .0[0] as i64;
            moment_x += dx as i64 * value;
            moment_y += dy as i64 * value;
        }
    }
    Some((moment_y as f32).atan2(moment_x as f32))
}
