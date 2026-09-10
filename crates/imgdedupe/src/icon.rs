//! The window's icon, drawn rather than stored.
//!
//! A picture with a second one behind it, a step down and to the right: two of
//! a thing, which is what the program is about. It fills the icon corner to
//! corner and stands on nothing, because a title bar draws it sixteen points
//! across and anything that is a mark inside a card is a card at that size.
//!
//! macOS is the other way round. The dock draws every icon as a card of its own,
//! a rounded square inset in its canvas with the mark inside it, and one that
//! fills its canvas instead sits among them with no shape and the wrong size. So
//! there the same mark is drawn on that card, to the proportions the dock uses.
//!
//! Drawn here because a picture kept as a file would have to be found at run
//! time or embedded and decoded, and this is two rounded squares, a circle and
//! a five point line. Everything is laid out in a square of `EDGE` points and
//! drawn at four times that, then averaged down, which is what gives the
//! rounded corners and the sloped hills a clean edge at any size.

/// The icon's own grid. Every measurement below is in these units.
const EDGE: f32 = 64.0;

/// The icon in pixels a side.
///
/// A title bar draws it sixteen points across and a bitmap the size of the grid
/// is more than that needs. The dock draws it as large as the screen it is on,
/// beside icons made at a thousand and twenty-four, and one made at sixty-four
/// is soft where none of them are.
#[cfg(target_os = "macos")]
const SIDE: u32 = 512;
#[cfg(not(target_os = "macos"))]
const SIDE: u32 = EDGE as u32;

/// Drawn this many times larger and averaged down. Fewer steps where the icon
/// itself is large, because the edges have pixels of their own to land on.
#[cfg(target_os = "macos")]
const OVER: u32 = 2;
#[cfg(not(target_os = "macos"))]
const OVER: u32 = 4;

/// The picture in front, and what is drawn inside it.
const PICTURE: [u8; 3] = [0x3c, 0x7f, 0xb1];
const INSIDE: [u8; 3] = [0xff, 0xff, 0xff];

/// The one behind, which is an outline and nothing else.
const BEHIND: [u8; 3] = [0x7a, 0x8a, 0x99];

/// The two pictures: the same square twice, the second one down and right by
/// enough to be seen behind the first and no more.
const FRAME: f32 = 50.0;
/// The corner the dock rounds its icons to, as a share of the picture's own
/// edge, so the picture in front reads as a card at any size.
#[cfg(target_os = "macos")]
const FRAME_ROUND: f32 = FRAME * 185.4 / 824.0;
#[cfg(not(target_os = "macos"))]
const FRAME_ROUND: f32 = 6.3;
const FRONT_AT: (f32, f32) = (2.0, 2.0);
/// How far the one behind steps out from the one in front.
///
/// On macOS that step is the overhang: the picture in front is the size the dock
/// draws every icon, so the one behind can only go outside it, into the margin
/// the dock leaves around them. `OVERHANG` of that margin, in canvas units,
/// undone by `SCALE` because this is measured in the mark's own.
#[cfg(target_os = "macos")]
const STEP: f32 = OVERHANG / SCALE;
#[cfg(not(target_os = "macos"))]
const STEP: f32 = 8.0;
const OUTLINE: f32 = 4.0;

/// The box the dock draws an icon in, in the proportions macOS uses: of a canvas
/// 1024 across, a square 824 wide, centred. The two pictures together fill it,
/// and nothing is drawn behind them.
#[cfg(target_os = "macos")]
const CARD: f32 = EDGE * 824.0 / 1024.0;

/// What the mark is drawn at for the picture in front to come out `CARD` wide,
/// and where that picture's own corner then sits.
#[cfg(target_os = "macos")]
const SCALE: f32 = CARD / FRAME;
#[cfg(target_os = "macos")]
const FRONT_ON_CANVAS: f32 = (EDGE - CARD) / 2.0;

/// How far past the picture in front the one behind reaches, out of the margin
/// the dock leaves, which is `FRONT_ON_CANVAS` wide. Short of all of it, so the
/// overhang does not touch the edge of the canvas.
#[cfg(target_os = "macos")]
const OVERHANG: f32 = FRONT_ON_CANVAS * 0.86;

/// The sun in the picture in front, and where the hills in it stand.
const SUN_AT: (f32, f32) = (17.0, 17.0);
const SUN: f32 = 5.9;
const HILLS: [(f32, f32); 5] =
    [(5.5, 43.1), (19.9, 27.0), (29.7, 36.8), (37.8, 29.7), (48.4, 43.1)];

/// The icon at `EDGE` points a side, as the rows of pixels a window wants.
pub fn window_icon() -> egui::IconData {
    let side = SIDE;
    let big = side * OVER;
    let mut over = vec![[0u8; 4]; (big * big) as usize];

    // The icon's own units per sample of the oversampled picture.
    let step = EDGE / big as f32;
    for y in 0..big {
        for x in 0..big {
            // In the icon's own units, at the middle of this sample.
            let at = ((x as f32 + 0.5) * step, (y as f32 + 0.5) * step);
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
///
/// The mark fills the canvas: there is nothing behind it.
#[cfg(not(target_os = "macos"))]
fn colour_at(at: (f32, f32)) -> Option<[u8; 3]> {
    mark_at(at)
}

/// What is at one point of the icon, or nothing where the icon is not.
///
/// The same mark, drawn large enough that the picture in front comes out the
/// size the dock draws every icon, sitting where the dock puts them. The one
/// behind steps out past it into the margin.
#[cfg(target_os = "macos")]
fn colour_at(at: (f32, f32)) -> Option<[u8; 3]> {
    let into_mark = (
        (at.0 - FRONT_ON_CANVAS) / SCALE + FRONT_AT.0,
        (at.1 - FRONT_ON_CANVAS) / SCALE + FRONT_AT.1,
    );
    mark_at(into_mark)
}

/// The mark itself, in a square of `EDGE` points, or nothing where it is not.
fn mark_at(at: (f32, f32)) -> Option<[u8; 3]> {
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
        assert_eq!(icon.width, SIDE);
        assert_eq!(icon.height, SIDE);
        assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
    }

    /// It is one picture over another and nothing else: no card behind them, so
    /// what a title bar draws at sixteen points across is the two pictures
    /// rather than a white square with a mark in the middle of it.
    #[cfg(not(target_os = "macos"))]
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

    /// On macOS the mark stands on a card the shape and size the dock draws
    /// every other icon at: clear at the corners of the canvas, clear along the
    /// margin the card is inset by, and the card's own colour from there in.
    ///
    /// An icon that fills its canvas is the fault this catches. It has no corner
    /// to round, so it is square where every icon beside it is not, and it is
    /// wider than they are because the dock insets them and not it.
    #[cfg(target_os = "macos")]
    #[test]
    fn the_picture_in_front_is_the_size_the_dock_draws_and_the_other_overhangs() {
        let icon = window_icon();
        // A point of the icon's own grid, in pixels of the picture.
        let pixel = |unit: f32| (unit * SIDE as f32 / EDGE) as u32;

        // Where a colour reaches, in rows and in columns.
        let reach = |wanted: [u8; 3]| {
            let has = |along: u32, across_a_row: bool| {
                (0..SIDE).any(|other| {
                    let (x, y) = if across_a_row { (other, along) } else { (along, other) };
                    at(&icon, x, y) == solid(wanted)
                })
            };
            let rows: Vec<u32> = (0..SIDE).filter(|&n| has(n, true)).collect();
            let columns: Vec<u32> = (0..SIDE).filter(|&n| has(n, false)).collect();
            (
                (*rows.first().expect("no rows"), *rows.last().expect("no rows")),
                (*columns.first().expect("no columns"), *columns.last().expect("no columns")),
            )
        };

        // The picture in front is the box the dock draws every icon in: the
        // dock's own margin around it, and its edge the dock's own width.
        let (rows, columns) = reach(PICTURE);
        let margin = pixel(FRONT_ON_CANVAS);
        let far = pixel(FRONT_ON_CANVAS + CARD);
        for (side, wanted, what) in
            [(rows.0, margin, "top"), (columns.0, margin, "left"), (rows.1, far, "bottom"), (columns.1, far, "right")]
        {
            assert!(
                side.abs_diff(wanted) <= 2,
                "the {what} of the picture in front is at {side} where the dock draws {wanted}"
            );
        }

        // And the one behind steps out past it, without reaching the canvas.
        let (behind_rows, behind_columns) = reach(BEHIND);
        assert!(
            behind_rows.1 > rows.1 && behind_columns.1 > columns.1,
            "the one behind does not overhang: it reaches {behind_rows:?} against {rows:?}"
        );
        assert!(behind_rows.1 < SIDE - 1, "the overhang runs off the canvas");
        assert_eq!(at(&icon, 0, 0)[3], 0, "the corner of the canvas is not clear");
        assert_eq!(at(&icon, margin, margin)[3], 0, "the corner of the picture is square");
    }
}
