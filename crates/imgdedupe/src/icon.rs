//! The window's icon, drawn rather than stored.
//!
//! A picture with a second one behind it, a step down and to the right: two of
//! a thing, which is what the program is about. It fills the icon corner to
//! corner and stands on nothing, because a title bar draws it sixteen points
//! across and anything that is a mark inside a card is a card at that size.
//!
//! Drawn here because a picture kept as a file would have to be found at run
//! time or embedded and decoded, and this is two rounded squares, a circle and
//! a five point line. Everything is laid out in a square of `EDGE` points and
//! drawn at four times that, then averaged down, which is what gives the
//! rounded corners and the sloped hills a clean edge at any size.

/// The icon's own grid. Every measurement below is in these units.
const EDGE: f32 = 64.0;

/// Drawn this many times larger and averaged down.
const OVER: u32 = 4;

/// The picture in front, and what is drawn inside it.
const PICTURE: [u8; 3] = [0x3c, 0x7f, 0xb1];
const INSIDE: [u8; 3] = [0xff, 0xff, 0xff];

/// The one behind, which is an outline and nothing else.
const BEHIND: [u8; 3] = [0x7a, 0x8a, 0x99];

/// The two pictures: the same square twice, the second one down and right by
/// enough to be seen behind the first and no more.
const FRAME: f32 = 50.0;
const FRAME_ROUND: f32 = 6.3;
const FRONT_AT: (f32, f32) = (2.0, 2.0);
const STEP: f32 = 8.0;
const OUTLINE: f32 = 4.0;

/// The sun in the picture in front, and where the hills in it stand.
const SUN_AT: (f32, f32) = (17.0, 17.0);
const SUN: f32 = 5.9;
const HILLS: [(f32, f32); 5] =
    [(5.5, 43.1), (19.9, 27.0), (29.7, 36.8), (37.8, 29.7), (48.4, 43.1)];

/// The icon at `EDGE` points a side, as the rows of pixels a window wants.
pub fn window_icon() -> egui::IconData {
    let side = EDGE as u32;
    let big = side * OVER;
    let mut over = vec![[0u8; 4]; (big * big) as usize];

    for y in 0..big {
        for x in 0..big {
            // In the icon's own units, at the middle of this pixel.
            let at = ((x as f32 + 0.5) / OVER as f32, (y as f32 + 0.5) / OVER as f32);
            if let Some(colour) = colour_at(at) {
                over[(y * big + x) as usize] = [colour[0], colour[1], colour[2], 0xff];
            }
        }
    }

    // Averaged down, transparency and all, so an edge that covered half its
    // pixels ends up half there.
    let mut pixels = Vec::with_capacity((side * side * 4) as usize);
    for y in 0..side {
        for x in 0..side {
            let mut sum = [0u32; 4];
            for step_y in 0..OVER {
                for step_x in 0..OVER {
                    let at = ((y * OVER + step_y) * big + x * OVER + step_x) as usize;
                    for (part, value) in over[at].iter().enumerate() {
                        sum[part] += u32::from(*value);
                    }
                }
            }
            let taken = (OVER * OVER) as u32;
            for part in sum {
                pixels.push((part / taken) as u8);
            }
        }
    }
    egui::IconData { rgba: pixels, width: side, height: side }
}

/// What is at one point of the icon, or nothing where the icon is not.
fn colour_at(at: (f32, f32)) -> Option<[u8; 3]> {
    // The one behind is an outline: inside its edge and outside the room that
    // edge leaves. The one in front is drawn over it, so where they overlap
    // there is only the front one.
    let back = (FRONT_AT.0 + STEP, FRONT_AT.1 + STEP);
    let mut colour = None;
    if inside_rounded(at, back, (FRAME, FRAME), FRAME_ROUND)
        && !inside_rounded(
            at,
            (back.0 + OUTLINE, back.1 + OUTLINE),
            (FRAME - OUTLINE * 2.0, FRAME - OUTLINE * 2.0),
            (FRAME_ROUND - OUTLINE).max(0.5),
        )
    {
        colour = Some(BEHIND);
    }

    if inside_rounded(at, FRONT_AT, (FRAME, FRAME), FRAME_ROUND) {
        colour = Some(if in_the_sun(at) || on_the_hills(at) { INSIDE } else { PICTURE });
    }
    colour
}

/// Whether a point is inside a rectangle with rounded corners.
fn inside_rounded(at: (f32, f32), min: (f32, f32), size: (f32, f32), round: f32) -> bool {
    let (x, y) = (at.0 - min.0, at.1 - min.1);
    if x < 0.0 || y < 0.0 || x > size.0 || y > size.1 {
        return false;
    }
    // How far into the corner's square this is. Outside every corner square the
    // answer is already yes.
    let across = (round - x).max(x - (size.0 - round)).max(0.0);
    let down = (round - y).max(y - (size.1 - round)).max(0.0);
    across * across + down * down <= round * round
}

fn in_the_sun(at: (f32, f32)) -> bool {
    let (x, y) = (at.0 - SUN_AT.0, at.1 - SUN_AT.1);
    x * x + y * y <= SUN * SUN
}

/// The hills under it: a peak, a dip, a smaller peak, standing on the bottom of
/// the picture. Counted by how many of the shape's sides a line drawn out to the
/// right of the point crosses, which is odd inside it and even outside.
fn on_the_hills(at: (f32, f32)) -> bool {
    let mut inside = false;
    let mut previous = HILLS[HILLS.len() - 1];
    for corner in HILLS {
        let (one, other) = (corner, previous);
        if (one.1 > at.1) != (other.1 > at.1) {
            let across = (other.0 - one.0) * (at.1 - one.1) / (other.1 - one.1) + one.0;
            if at.0 < across {
                inside = !inside;
            }
        }
        previous = corner;
    }
    inside
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(icon: &egui::IconData, x: u32, y: u32) -> [u8; 4] {
        let start = ((y * icon.width + x) * 4) as usize;
        [icon.rgba[start], icon.rgba[start + 1], icon.rgba[start + 2], icon.rgba[start + 3]]
    }

    fn solid(colour: [u8; 3]) -> [u8; 4] {
        [colour[0], colour[1], colour[2], 0xff]
    }

    /// The icon is the size a window is told it is, and every row of it is
    /// there. A picture that says one size and holds another is not shown at
    /// all: the window quietly keeps whatever it had.
    #[test]
    fn the_icon_holds_the_pixels_it_says_it_does() {
        let icon = window_icon();
        assert_eq!(icon.width, EDGE as u32);
        assert_eq!(icon.height, EDGE as u32);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }

    /// It is one picture over another and nothing else: no card behind them, so
    /// what a title bar draws at sixteen points across is the two pictures
    /// rather than a white square with a mark in the middle of it.
    #[test]
    fn the_icon_is_one_picture_over_another() {
        let icon = window_icon();

        assert_eq!(at(&icon, 0, 0)[3], 0, "the corner of the icon is not clear");
        assert_eq!(at(&icon, 62, 6)[3], 0, "there is something behind the pictures");
        // The picture in front: its sky, its sun, and its hills.
        assert_eq!(at(&icon, 8, 8), solid(PICTURE), "the front picture has no sky");
        assert_eq!(at(&icon, 17, 17), solid(INSIDE), "the sun is not in the picture");
        assert_eq!(at(&icon, 20, 35), solid(INSIDE), "the hills are not in the picture");
        // The one behind: its edge, and nothing at all inside what that edge
        // encloses, because the picture in front is standing in it.
        assert_eq!(at(&icon, 58, 50), solid(BEHIND), "the one behind has no edge");
        assert_eq!(at(&icon, 54, 20)[3], 0, "the one behind is not an outline");
    }
}
