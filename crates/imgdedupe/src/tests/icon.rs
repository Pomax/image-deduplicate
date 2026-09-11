use super::*;

fn at(icon: &egui::IconData, x: u32, y: u32) -> [u8; 4] {
    let start = ((y * icon.width + x) * 4) as usize;
    [
        icon.rgba[start],
        icon.rgba[start + 1],
        icon.rgba[start + 2],
        icon.rgba[start + 3],
    ]
}

fn solid(colour: [u8; 3]) -> [u8; 4] {
    [colour[0], colour[1], colour[2], 0xff]
}

/// The icon is the size a window is told it is, and every row of it is there. A
/// picture that says one size and holds another is not shown at all: the window
/// quietly keeps whatever it had.
#[test]
fn the_icon_holds_the_pixels_it_says_it_does() {
    let icon = window_icon();
    assert_eq!(icon.width, SIDE);
    assert_eq!(icon.height, SIDE);
    assert_eq!(icon.rgba.len(), (icon.width * icon.height * 4) as usize);
}

/// It is one picture over another and nothing else: no card behind them, so what
/// a title bar draws at sixteen points across is the two pictures rather than a
/// white square with a mark in the middle of it.
#[cfg(not(target_os = "macos"))]
#[test]
fn the_icon_is_one_picture_over_another() {
    let icon = window_icon();

    assert_eq!(at(&icon, 0, 0)[3], 0, "the corner of the icon is not clear");
    assert_eq!(
        at(&icon, 62, 6)[3],
        0,
        "there is something behind the pictures"
    );
    // The picture in front: its sky, its sun, and its hills.
    assert_eq!(
        at(&icon, 12, 12),
        solid(PICTURE),
        "the front picture has no sky"
    );
    assert_eq!(
        at(&icon, 22, 22),
        solid(INSIDE),
        "the sun is not in the picture"
    );
    assert_eq!(
        at(&icon, 25, 40),
        solid(INSIDE),
        "the hills are not in the picture"
    );
    // The one behind: its edge, and nothing at all inside what that edge
    // encloses, because the picture in front is standing in it.
    assert_eq!(
        at(&icon, 60, 50),
        solid(BEHIND),
        "the one behind has no edge"
    );
    assert_eq!(at(&icon, 58, 20)[3], 0, "the one behind is not an outline");
}

/// The first and last row, and the first and last column, holding a colour.
#[cfg(not(target_os = "macos"))]
fn bounds(icon: &egui::IconData, wanted: [u8; 3]) -> (u32, u32, u32, u32) {
    let (mut top, mut bottom, mut left, mut right) = (u32::MAX, 0, u32::MAX, 0);
    for y in 0..icon.height {
        for x in 0..icon.width {
            if at(icon, x, y) == solid(wanted) {
                top = top.min(y);
                bottom = bottom.max(y);
                left = left.min(x);
                right = right.max(x);
            }
        }
    }
    assert!(top != u32::MAX, "the icon holds none of that colour");
    (top, bottom, left, right)
}

/// The picture in front is what the icon lines up on: centred on the canvas,
/// with the one behind stepping out below it and to the right. Lining up on the
/// two of them together puts the picture in front up and to the left of the
/// middle, which is what a taskbar button shows.
#[cfg(not(target_os = "macos"))]
#[test]
fn the_picture_in_front_is_centred_and_the_one_behind_steps_off_it() {
    let icon = window_icon();
    let (top, bottom, left, right) = bounds(&icon, PICTURE);
    let under = SIDE - 1 - bottom;
    let beside = SIDE - 1 - right;
    assert!(
        top.abs_diff(under) <= 1,
        "the picture in front is {top} from the top and {under} from the bottom"
    );
    assert!(
        left.abs_diff(beside) <= 1,
        "the picture in front is {left} from the left and {beside} from the right"
    );

    let (_, behind_bottom, _, behind_right) = bounds(&icon, BEHIND);
    assert!(
        behind_bottom > bottom,
        "the one behind does not step below the picture"
    );
    assert!(
        behind_right > right,
        "the one behind does not step right of the picture"
    );
    assert!(
        behind_bottom < SIDE - 1,
        "the one behind runs off the bottom of the canvas"
    );
    assert!(
        behind_right < SIDE - 1,
        "the one behind runs off the side of the canvas"
    );
}

/// On macOS the mark stands on a card the shape and size the dock draws every
/// other icon at: clear at the corners of the canvas, clear along the margin the
/// card is inset by, and the card's own colour from there in.
///
/// An icon that fills its canvas is the fault this catches. It has no corner to
/// round, so it is square where every icon beside it is not, and it is wider
/// than they are because the dock insets them and not it.
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
                let (x, y) = if across_a_row {
                    (other, along)
                } else {
                    (along, other)
                };
                at(&icon, x, y) == solid(wanted)
            })
        };
        let rows: Vec<u32> = (0..SIDE).filter(|&n| has(n, true)).collect();
        let columns: Vec<u32> = (0..SIDE).filter(|&n| has(n, false)).collect();
        (
            (
                *rows.first().expect("no rows"),
                *rows.last().expect("no rows"),
            ),
            (
                *columns.first().expect("no columns"),
                *columns.last().expect("no columns"),
            ),
        )
    };

    // The picture in front is the box the dock draws every icon in: the dock's
    // own margin around it, and its edge the dock's own width.
    let (rows, columns) = reach(PICTURE);
    let margin = pixel(FRONT_ON_CANVAS);
    let far = pixel(FRONT_ON_CANVAS + CARD);
    for (side, wanted, what) in [
        (rows.0, margin, "top"),
        (columns.0, margin, "left"),
        (rows.1, far, "bottom"),
        (columns.1, far, "right"),
    ] {
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
    assert_eq!(
        at(&icon, 0, 0)[3],
        0,
        "the corner of the canvas is not clear"
    );
    assert_eq!(
        at(&icon, margin, margin)[3],
        0,
        "the corner of the picture is square"
    );
}
