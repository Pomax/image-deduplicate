use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use eframe::egui;
use imgdedupe_core::cleanup::{self, Disposal, Plan};
use imgdedupe_core::db;
use imgdedupe_core::matching::{self, DuplicateSet, Thresholds};
use imgdedupe_core::scan;
use imgdedupe_core::runlog;

use crate::headless;
use crate::indexer::{self, Run, Update};
use crate::thumbs::{self, Thumbnails};

pub fn launch() -> Result<()> {
    #[cfg(target_os = "linux")]
    crate::mesa::quieten();
    let result = start_window();
    #[cfg(feature = "logging")]
    if let Err(err) = &result {
        // Without this a failure to open the window is invisible: a windowed
        // process has no console for the message to go to.
        runlog::log_line!("the window could not be opened: {err:#}");
    }
    result
}

fn start_window() -> Result<()> {
    let saved = crate::settings::Settings::load();
    let mut viewport = egui::ViewportBuilder::default()
        // Tall enough for the scan page's own content without scrolling: the three
        // boxes, the progress box, and one line per step of a pass. That last list
        // is what sets the height, and a window that cannot show all of it hides
        // exactly the part someone is watching when they want to know what is
        // taking so long.
        .with_inner_size([1100.0, 860.0])
        .with_min_inner_size([700.0, 780.0])
        .with_title("imgdedupe")
        .with_icon(crate::icon::window_icon());
    if let Some(window) = saved.window {
        viewport = viewport
            .with_inner_size([window.width, window.height])
            .with_position([window.x, window.y])
            .with_maximized(window.maximized);
    }
    let options = eframe::NativeOptions { viewport, ..Default::default() };
    eframe::run_native(
        "imgdedupe",
        options,
        Box::new(|cc| {
            crate::fonts::install(&cc.egui_ctx);
            install_style(&cc.egui_ctx);
            Ok(Box::new(App::from_settings(saved)))
        }),
    )
    .map_err(|err| anyhow::anyhow!("{err}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum View {
    Scan,
    Review,
    Cleanup,
}

/// What the thread removing files sends back.
enum Removal {
    Progress(usize),
    /// The files are gone and the index is being brought up to date. On a large
    /// folder that is the slowest part of a cleanup, so it says so.
    Tidying,
    /// The outcome, and how many rows the index lost.
    Done(Box<cleanup::Outcome>, usize),
    Failed(String),
}

/// What the thread searching for duplicates sends back.
enum Found {
    /// Where the search has got to. It used to send nothing until it was
    /// finished, so the window sat on whatever the pass had last said for the
    /// whole of it.
    Progress(matching::Progress),
    Sets(Vec<DuplicateSet>),
    Cancelled,
    Failed(String),
}

/// What the thread that opens a folder's index sends back.
///
/// A folder that has been scanned before is a folder whose pictures are already
/// known, so opening it reads them into memory whether or not a pass is going to
/// run: the search then costs the comparing and nothing else, and pressing Find
/// duplicates does not first sit through a file being read over a network.
enum Opened {
    /// What the folder was set to the last time it was open.
    Notes(crate::notes::Notes),
    /// How far the reading has got.
    Reading(matching::Progress),
    /// The pairs somebody said are not copies of each other, read off the same
    /// connection as the pictures. They are part of what a folder's index says
    /// about it, so they arrive with it and are held in memory from then on.
    Ignored(Vec<(i64, i64)>),
    /// The pictures, as the search wants them.
    Index(std::sync::Arc<Vec<matching::Image>>),
    Failed(String),
}

/// Every pair of pictures in a set, lower file id first, which is how a pair is
/// written down and how it is looked up.
fn pairs_of(set: &DuplicateSet) -> impl Iterator<Item = (i64, i64)> + '_ {
    set.members.iter().enumerate().flat_map(|(at, one)| {
        set.members[at + 1..].iter().map(|other| db::pair(one.file_id, other.file_id))
    })
}

/// How one of the steps on the scan page went.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Went {
    Happened,
    Waiting,
    Skipped,
}

/// Which way a cursor key moves the preview.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Direction {
    Forward,
    Back,
    NextSet,
    PreviousSet,
}

/// The place a cursor key moves to, given how many pictures each set on screen
/// holds. Nothing at either end of the list, rather than wrapping round.
///
/// Left and right run through the whole list, crossing into the next or previous
/// set at its edges. Up and down move a set at a time and stay at the same place
/// within it, or at the last picture when the set they arrive at is shorter.
fn step(counts: &[usize], at: (usize, usize), direction: Direction) -> Option<(usize, usize)> {
    let (set, member) = at;
    if counts.get(set).copied().unwrap_or(0) == 0 {
        return None;
    }
    match direction {
        Direction::Forward => {
            if member + 1 < counts[set] {
                Some((set, member + 1))
            } else {
                let next = set + 1;
                (counts.get(next)? > &0).then_some((next, 0))
            }
        }
        Direction::Back => {
            if member > 0 {
                Some((set, member - 1))
            } else {
                let previous = set.checked_sub(1)?;
                (counts[previous] > 0).then(|| (previous, counts[previous] - 1))
            }
        }
        Direction::NextSet => {
            let next = set + 1;
            let count = *counts.get(next)?;
            (count > 0).then(|| (next, member.min(count - 1)))
        }
        Direction::PreviousSet => {
            let previous = set.checked_sub(1)?;
            let count = counts[previous];
            (count > 0).then(|| (previous, member.min(count - 1)))
        }
    }
}

/// Where the list has to be scrolled to for one row to sit in the middle of what
/// is on screen, or nothing when it is already there.
///
/// The row walked to is always brought to the middle, so the rows on either side
/// of it are in sight: walking forward shows what is coming and walking back
/// shows what has been passed. The ends of the list are the only exception, since
/// there is nothing beyond them to scroll into view.
///
/// The rows the list is not drawing cannot be asked to scroll themselves into
/// view, so the place of the one wanted is worked out from its number: every row
/// is the same height, and they are laid out one after another.
fn scroll_to_show(
    row: usize,
    rows: usize,
    row_height: f32,
    spacing: f32,
    offset: f32,
    viewport: f32,
) -> Option<f32> {
    let pitch = row_height + spacing;
    let content = (rows as f32 * pitch - spacing).max(0.0);
    let middle = row as f32 * pitch + row_height / 2.0 - viewport / 2.0;
    let wanted = middle.clamp(0.0, (content - viewport).max(0.0));
    ((wanted - offset).abs() > 0.5).then_some(wanted)
}

/// What a set is keeping. A set with no entry at all is keeping nothing, and
/// every picture in it goes: what is marked is what is kept.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Keep {
    One(i64),
    /// More than one picture, which is what marking a second one does while
    /// "allow multi-select" is on. Never one and never none: one picture is `One`
    /// and no picture is no entry at all.
    Several(Vec<i64>),
    All,
}

impl Keep {
    fn keeps(&self, file_id: i64) -> bool {
        match self {
            Keep::One(kept) => *kept == file_id,
            Keep::Several(kept) => kept.contains(&file_id),
            Keep::All => true,
        }
    }
}

/// Whether a set that is keeping this is keeping that picture.
fn keeps(keeping: Option<&Keep>, file_id: i64) -> bool {
    keeping.is_some_and(|keep| keep.keeps(file_id))
}

/// What a set keeps once one more picture is marked, and once one is unmarked.
/// A set that ends up keeping nothing has no entry, which is what `None` says.
fn marked(keeping: Option<&Keep>, members: &[i64], file_id: i64) -> Option<Keep> {
    let mut kept = spelled_out(keeping, members);
    kept.push(file_id);
    as_keep(kept)
}

fn unmarked(keeping: Option<&Keep>, members: &[i64], file_id: i64) -> Option<Keep> {
    let mut kept = spelled_out(keeping, members);
    kept.retain(|id| *id != file_id);
    as_keep(kept)
}

/// Every picture a set keeps, by name. `All` is the set itself, so it is written
/// out here before one of them can be taken off it.
fn spelled_out(keeping: Option<&Keep>, members: &[i64]) -> Vec<i64> {
    match keeping {
        Some(Keep::One(kept)) => vec![*kept],
        Some(Keep::Several(kept)) => kept.clone(),
        Some(Keep::All) => members.to_vec(),
        None => Vec::new(),
    }
}

fn as_keep(kept: Vec<i64>) -> Option<Keep> {
    match kept.len() {
        0 => None,
        1 => Some(Keep::One(kept[0])),
        _ => Some(Keep::Several(kept)),
    }
}

/// Where removed files go. Held as one value rather than three booleans, so the
/// three choices cannot all appear selected at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    Trash,
    MoveTo,
    Delete,
}

impl Destination {
    fn label(self) -> &'static str {
        match self {
            Destination::Trash => "Recycle bin",
            Destination::MoveTo => "Move to a folder",
            Destination::Delete => "Delete permanently",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Destination::Trash => "Recoverable from the recycle bin.",
            Destination::MoveTo => "Keeps the folder structure, so the files can be put back.",
            Destination::Delete => "This cannot be undone.",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Destination::Trash => "trash",
            Destination::MoveTo => "move",
            Destination::Delete => "delete",
        }
    }

    /// What the button that carries this out does, in the words for it. Moving a
    /// file to another folder is not removing it.
    fn verb(self) -> &'static str {
        match self {
            Destination::Trash | Destination::Delete => "Remove",
            Destination::MoveTo => "Move",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "trash" => Some(Destination::Trash),
            "move" => Some(Destination::MoveTo),
            "delete" => Some(Destination::Delete),
            _ => None,
        }
    }
}

/// Width of the strip a scroll bar sits in, and of the handle that fills it.
const SCROLL_BAR: f32 = 12.0;

/// When a file was last written, as `YYYY-MM-DD HH:MM`, in UTC.
///
/// The stamp is nanoseconds since the epoch, which is what the file system
/// reports and what the index stores. Nothing here reads a date out of the
/// picture's own metadata: most of the formats this reads do not carry one.
fn file_date(mtime_ns: i64) -> String {
    let seconds = mtime_ns.div_euclid(1_000_000_000);
    let (days, rest) = (seconds.div_euclid(86_400), seconds.rem_euclid(86_400));
    let (year, month, day) = civil_from_days(days);
    format!("{year:04}-{month:02}-{day:02} {:02}:{:02}", rest / 3600, (rest % 3600) / 60)
}

/// Days since 1970-01-01 to a calendar date, by Howard Hinnant's method: the year
/// is shifted to start in March so a leap day falls at the end of it and every
/// four hundred years is one cycle of a fixed length.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let shifted = days + 719_468;
    let era = shifted.div_euclid(146_097);
    let day_of_era = shifted.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let year = year_of_era + era * 400;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted_month = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted_month + 2) / 5 + 1) as u32;
    let month = if shifted_month < 10 { shifted_month + 3 } else { shifted_month - 9 } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The triangle on a scroll bar's end button, pointing at the end it scrolls to.
fn arrow(painter: &egui::Painter, button: egui::Rect, towards: f32, down: bool, ink: egui::Color32) {
    let middle = button.center();
    let reach = button.width().min(button.height()) * 0.26;
    let along = |amount: f32| {
        if down {
            egui::vec2(0.0, amount)
        } else {
            egui::vec2(amount, 0.0)
        }
    };
    let across = |amount: f32| {
        if down {
            egui::vec2(amount, 0.0)
        } else {
            egui::vec2(0.0, amount)
        }
    };
    painter.add(egui::Shape::convex_polygon(
        vec![
            middle + along(reach * towards),
            middle + along(-reach * towards) + across(-reach),
            middle + along(-reach * towards) + across(reach),
        ],
        ink,
        egui::Stroke::NONE,
    ));
}

/// Show something that scrolls, with the bar this application draws rather than
/// the toolkit's: a strip taken out of the space, a button at each end of it, and
/// a handle between them.
///
/// `step` is how far one click of an end button moves. Gives back what was shown,
/// how far along it is, and how much of it is on screen.
fn scrolled<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    down: bool,
    step: f32,
    area: egui::ScrollArea,
    show: impl FnOnce(egui::ScrollArea, &mut egui::Ui) -> egui::scroll_area::ScrollAreaOutput<R>,
) -> (R, f32, f32) {
    let room = ui.available_rect_before_wrap();
    let (content_rect, strip) = if down {
        (
            // A gap before the bar as well as the bar itself, so what is in the
            // list stops as far from it as the list stops from the window's edge
            // rather than running up against it.
            room.with_max_x(room.right() - SCROLL_BAR - PAGE_MARGIN),
            egui::Rect::from_min_max(egui::pos2(room.right() - SCROLL_BAR, room.top()), room.max),
        )
    } else {
        (
            room.with_max_y(room.bottom() - SCROLL_BAR),
            egui::Rect::from_min_max(egui::pos2(room.left(), room.bottom() - SCROLL_BAR), room.max),
        )
    };

    let pending: Option<f32> = ui.data_mut(|data| data.remove_temp(id));
    let mut area = area.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
    if let Some(offset) = pending {
        area = if down {
            area.vertical_scroll_offset(offset)
        } else {
            area.horizontal_scroll_offset(offset)
        };
    }

    let output = ui
        .allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            // Nothing inside is drawn outside: a list is a window onto its
            // rows, and a row that came out wider than the room it was given
            // paints over whatever is beside the list rather than being cut off
            // at its edge. Cut at the bar rather than at the content, because a
            // line drawn round something is drawn half on either side of its
            // edge, and a clip along that edge takes half the line with it.
            // The whole of the room the list was given, up to the bar: a box in
            // it is drawn with a line round it, and a line sits half on either
            // side of the edge it marks, so a clip along the content itself
            // takes the top off the first box and the bottom off the last.
            let cut = if down {
                egui::Rect::from_min_max(
                    egui::pos2(room.left(), room.top()),
                    egui::pos2(strip.left(), room.bottom()),
                )
            } else {
                egui::Rect::from_min_max(
                    egui::pos2(room.left(), room.top()),
                    egui::pos2(room.right(), strip.top()),
                )
            };
            ui.set_clip_rect(cut.intersect(ui.clip_rect()));
            show(area, ui)
        })
        .inner;

    // Alongside what it scrolls, and nailed to it. The bar marks the room the
    // list is drawn in, which is the same rectangle from one frame to the next:
    // it does not move because the list moved under it. That rectangle is the
    // one the area was given, not the one the rows happen to be at, which is
    // exactly what scrolling changes.
    let strip = if down {
        egui::Rect::from_min_max(
            egui::pos2(strip.left(), content_rect.top()),
            egui::pos2(strip.right(), content_rect.bottom()),
        )
    } else {
        egui::Rect::from_min_max(
            egui::pos2(content_rect.left(), strip.top()),
            egui::pos2(content_rect.right(), strip.bottom()),
        )
    };

    let axis = usize::from(down);
    let content = output.content_size[axis];
    let viewport = output.inner_rect.size()[axis];
    let offset = output.state.offset[axis];
    if let Some(wanted) = paint_scroll_bar(ui, strip, down, step, content, viewport, offset) {
        ui.data_mut(|data| data.insert_temp(id, wanted));
        ui.ctx().request_repaint();
    }
    (output.inner, offset, viewport)
}

/// Draw the bar for one scrolling area, and say where a click or a drag on it
/// wants that area to be.
fn paint_scroll_bar(
    ui: &egui::Ui,
    strip: egui::Rect,
    down: bool,
    step: f32,
    content: f32,
    viewport: f32,
    offset: f32,
) -> Option<f32> {
    if viewport <= 0.0 || content <= viewport + 0.5 {
        return None;
    }
    let furthest = content - viewport;
    let thickness = if down { strip.width() } else { strip.height() };
    let length = if down { strip.height() } else { strip.width() };

    let end = |from_start: f32| {
        if down {
            egui::Rect::from_min_size(
                egui::pos2(strip.left(), strip.top() + from_start),
                egui::vec2(thickness, thickness),
            )
        } else {
            egui::Rect::from_min_size(
                egui::pos2(strip.left() + from_start, strip.top()),
                egui::vec2(thickness, thickness),
            )
        }
    };
    let first = end(0.0);
    let last = end(length - thickness);
    let track_length = (length - thickness * 2.0).max(1.0);

    let ink = ui.visuals().text_color();
    let painter = ui.painter();
    painter.rect_filled(strip, 0.0, ui.visuals().extreme_bg_color);
    // Outlined on every side, so the bar reads as a part of the window rather
    // than as a strip of another colour left on the side of it, and so it ends
    // somewhere rather than running off the edge. Half a point in, because a
    // line is drawn either side of where it is put and the outer half of it
    // would be clipped away.
    painter.rect_stroke(
        strip.shrink(0.5),
        0.0,
        egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    arrow(painter, first, -1.0, down, ink);
    arrow(painter, last, 1.0, down, ink);

    let handle_length = (track_length * viewport / content).max(thickness * 2.0).min(track_length);
    let travel = track_length - handle_length;
    let at = (offset / furthest).clamp(0.0, 1.0);
    let handle_start = thickness + at * travel;
    let handle = if down {
        egui::Rect::from_min_size(
            egui::pos2(strip.left(), strip.top() + handle_start),
            egui::vec2(thickness, handle_length),
        )
    } else {
        egui::Rect::from_min_size(
            egui::pos2(strip.left() + handle_start, strip.top()),
            egui::vec2(handle_length, thickness),
        )
    };
    painter.rect_filled(handle, 2.0, ink);

    let base = ui.id().with(("scroll bar", strip.left() as i32, strip.top() as i32));
    let stepped = |by: f32| Some((offset + by).clamp(0.0, furthest));
    if ui.interact(first, base.with("first"), egui::Sense::click()).clicked() {
        return stepped(-step);
    }
    if ui.interact(last, base.with("last"), egui::Sense::click()).clicked() {
        return stepped(step);
    }

    let track = ui.interact(strip, base.with("track"), egui::Sense::click_and_drag());
    let grip = base.with("grip");

    // Nothing is being pressed, so there is no grip to hold on to. Clearing it
    // here is what makes the next press take a fresh one.
    let Some(pointer) = track.interact_pointer_pos() else {
        ui.data_mut(|data| data.remove_temp::<f32>(grip));
        return None;
    };
    let along = if down { pointer.y - strip.top() } else { pointer.x - strip.left() };

    // Where on the handle the pointer went down, kept for as long as it is held.
    // Taken on the first frame of the press: a drag has not started yet then, and
    // a click is not reported until the button comes back up, so waiting for
    // either of those is what made the handle jump under the pointer.
    let held = ui.data_mut(|data| data.get_temp::<f32>(grip)).unwrap_or_else(|| {
        let handle_start = thickness + (offset / furthest).clamp(0.0, 1.0) * travel;
        let on_handle = along >= handle_start && along <= handle_start + handle_length;
        // A press on the track outside the handle has nowhere to hold, so it
        // jumps, and the handle arrives centred on the pointer.
        let from_start =
            if on_handle { along - handle_start } else { handle_length / 2.0 };
        ui.data_mut(|data| data.insert_temp(grip, from_start));
        from_start
    });

    let wanted = ((along - thickness - held) / travel.max(1.0)).clamp(0.0, 1.0);
    Some(wanted * furthest)
}

/// How the window looks and what it lets the pointer do.
///
/// Scrolling gets a strip of `SCROLL_BAR` points at the edge of anything that
/// scrolls, with the handle filling it when there is something to scroll.
/// `solid` is the preset that takes that space rather than floating over the
/// content. Its handle is drawn in the widget background colour, which is pale
/// grey on a near white track and comes out invisible; the foreground colour is
/// what makes a handle that can be seen.
fn install_style(ctx: &egui::Context) {
    // Both of them. `style_mut` changes the theme in use at the time, and the
    // window is handed the machine's theme after this runs, which would leave the
    // other one as egui ships it.
    ctx.all_styles_mut(|style| {
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = SCROLL_BAR;
        style.spacing.scroll.bar_inner_margin = 0.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        style.spacing.scroll.foreground_color = true;
        // Nothing here is a text field. Labels are what the window says, not
        // something to drag a cursor through.
        style.interaction.selectable_labels = false;
    });
}

/// Spacing used between the sections of a view, so they are consistent.
const SECTION_GAP: f32 = 14.0;

/// Height of the boxes on the scan row. Fixed, so the row is flush and nothing
/// the pointer does can change it. Sized to the tallest of the three, which is
/// the matching box at four rows.

/// Border and inner margin `Frame::group` adds around its contents, so the
/// arithmetic below is about outer widths.
const FRAME_EXTRA: f32 = 14.0;



/// The largest a picture in a set may be drawn. What the strip of them is tall
/// is worked out from this and the font, in `tile_strip_height`.
const TILE: egui::Vec2 = egui::vec2(156.0, 118.0);

/// Space kept clear around a picture for what is drawn around it: the keeper's
/// border, and the ring outside that for the one the preview is showing. The ring
/// sits 3 out from the border and is 3 wide, so it reaches 4.5 past it. Without
/// this the ring is drawn outside the tile and the neighbour clips it.
const TILE_RING: f32 = 6.0;

/// The picture's border and the margin inside it, on both sides.
const TILE_BORDER: f32 = 4.0;

/// What a picture of these proportions comes out as inside `TILE`.
fn fitted(width: u32, height: u32) -> egui::Vec2 {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (TILE.x / width).min(TILE.y / height);
    egui::vec2(width * scale, height * scale)
}

/// How wide one tile is: its own picture with room for what is drawn around it.
/// A portrait beside a landscape is a portrait's width, so a strip has no gaps in
/// it where a narrow picture was given a wide picture's column.
fn tile_width(member: &imgdedupe_core::matching::Member) -> f32 {
    fitted(member.width, member.height).x.max(1.0) + TILE_BORDER + TILE_RING * 2.0
}

/// Room round the buttons along the bottom of a set. None: the band is the
/// buttons, and the space between them is the space the row lays them out with.
const BUTTON_ROW_GAP: f32 = 0.0;

/// The band those buttons sit on.
const BUTTON_ROW_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xe8, 0xe8);

/// The line along the top of that band, darker than the line round the box so
/// the band reads as the foot of the set rather than as another edge of it.
const BUTTON_ROW_EDGE: egui::Color32 = egui::Color32::from_rgb(0x22, 0x22, 0x22);

/// What the page keeps at its edges, and what a list keeps between what is in it
/// and the scroll bar down its right: the same, so a box in a list stops as far
/// from the bar as the list stops from the edge of the window.
///
/// The review keeps this itself rather than through the panel it is drawn in, so
/// the panels in it can run the width of the window and draw their lines across
/// all of it.
const PAGE_MARGIN: f32 = 16.0;

/// What one of the buttons under a set does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SetAction {
    KeepAll,
    KeepNone,
    Ignore,
    Unignore,
}

/// Kept clear at the right of the folder row for the button that lists the
/// folders scanned before, so a long path stops short of it.
const PREVIOUS_ROOM: f32 = 84.0;

/// What the review has found and what a cleanup would do about it, as one line:
/// how many sets, how many pictures in them are not being kept, and what those
/// come to. The last part is the least of it and is drawn as such.
fn count_line(
    ui: &egui::Ui,
    sets: usize,
    duplicates: usize,
    going: usize,
    reclaimable: i64,
) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let strong = egui::TextFormat {
        font_id: font.clone(),
        color: ui.style().visuals.strong_text_color(),
        ..Default::default()
    };
    let weak = egui::TextFormat {
        font_id: font,
        color: ui.style().visuals.weak_text_color(),
        ..Default::default()
    };
    let gap = ui.spacing().item_spacing.x;

    let mut line = egui::text::LayoutJob::default();
    line.append(&counted(sets as u64, "set", "sets"), 0.0, strong.clone());
    line.append(&counted(duplicates as u64, "duplicate", "duplicates"), gap, strong.clone());
    line.append(&format!("{going} to remove"), gap, strong);
    line.append(&format!("{:.1} MB to reclaim", reclaimable as f64 / 1e6), gap, weak);
    line
}

/// The review toolbar's one row. The checkbox, the counts and the button are
/// laid out over the same rectangle, which has to be as tall as the tallest of
/// them: the button.
const TOOLBAR_HEIGHT: f32 = 28.0;

/// Room around a preset's name. Four of these sit under the slider and are read
/// at a glance, so they are no bigger than the words in them.
const PRESET_PADDING: egui::Vec2 = egui::vec2(6.0, 2.0);

/// The box holding the percentage beside the slider. Wide enough for the widest
/// value the scale reaches, so the number never changes the width of anything.
const VALUE_WIDTH: f32 = 56.0;

/// Whether the slider is sitting on a preset, which is what draws that one as
/// pressed. The slider carries one decimal place, so anything closer than half
/// of that is the same setting.
fn on_preset(sensitivity: f64, percent: f64) -> bool {
    (sensitivity - percent).abs() < 0.05
}

/// How many pictures are copies of another: every picture in a set, less the one
/// each set keeps.
fn duplicate_count(sets: &[DuplicateSet]) -> usize {
    let pictures: usize = sets.iter().map(|set| set.members.len()).sum();
    pictures - sets.len()
}

/// What a set row takes. The list places the rows it is not drawing by this, and
/// a row is built to exactly it, so a row can never be a few points out and shift
/// the content under a scroll that is already running.
fn set_row_height(ui: &egui::Ui) -> f32 {
    // The room kept above the pictures, the strip, the scroll bar under it, the
    // row of buttons under that, the line drawn round the lot, and the space to
    // the next box. The list places the rows it is not drawing by this number,
    // so the space between boxes has to be part of it.
    BOX_PADDING
        + tile_strip_height(ui)
        + SCROLL_BAR
        + button_row_height(ui)
        + 2.0 * BOX_EDGE
        + BETWEEN_BOXES
}

/// Kept between one set and the next.
const BETWEEN_BOXES: f32 = 12.0;

/// How much of a set nobody calls a set of copies is drawn: the pictures and
/// every line of writing under them, but not the row of buttons, which is how it
/// stops being ignored.
const IGNORED_OPACITY: f32 = 0.25;

/// The line the box is drawn with, on each side.
const BOX_EDGE: f32 = 1.0;

/// What a box keeps between its edge and the pictures in it. The band of
/// buttons keeps none: it is the bottom of the box.
const BOX_PADDING: f32 = 6.0;

/// The row of buttons along the bottom of a set, with the space above it.
fn button_row_height(ui: &egui::Ui) -> f32 {
    // What the buttons themselves come out as, with the same small space above
    // and below them. They are drawn with the presets' padding, not the style's
    // own, and a band built to the style's is half as tall again as it needs.
    2.0 * BUTTON_ROW_GAP
        + ui.text_style_height(&egui::TextStyle::Button)
        + 2.0 * PRESET_PADDING.y
}

/// What one tile in a set takes from top to bottom, which is what the strip of
/// them is tall.
///
/// Worked out from the style rather than kept as a number, because every part of
/// it is a style value: the text under a picture is four lines of whatever the
/// window's font measures, and a number written down here for one font is dead
/// space or a clipped line in another.
fn tile_strip_height(ui: &egui::Ui) -> f32 {
    let gap = ui.spacing().item_spacing.y;
    // Room kept clear for the ring, then the picture at its largest inside the
    // border drawn round it.
    let picture = TILE_RING + TILE.y + TILE_BORDER;
    // Under it, five rows with a gap above each: the one that says KEEP, which
    // is kept clear whether or not it says it, and the four lines of text.
    let under = 5.0 * gap + ui.spacing().interact_size.y
        + 4.0 * ui.text_style_height(&egui::TextStyle::Body);
    picture + under
}

/// Box widths for a row: each one its measured content, plus an equal share of
/// whatever is left over, so the row fills the width and no box is padded more
/// than any other.
fn share_row_width(available: f32, content: &[f32], gap: f32) -> Vec<f32> {
    let boxes = content.len() as f32;
    let used: f32 = content.iter().map(|width| width + FRAME_EXTRA).sum();
    let spare = ((available - used - gap * (boxes - 1.0)) / boxes).max(0.0);
    content.iter().map(|width| width + spare).collect()
}

/// A titled box. Every group of related controls goes in one, so the window reads
/// as parts rather than as one column of widgets.
fn section(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width() - FRAME_EXTRA;
    sized_section(ui, title, egui::vec2(width, 0.0), contents);
}

/// The same at a fixed height, for boxes standing side by side.
///
/// The height is a constant and not measured. Measuring the boxes and feeding the
/// tallest back in on the next frame ratchets: the measurement includes the height
/// that was just imposed, so it can only grow, and hovering a widget was enough to
/// make the whole row taller and keep it there.
/// Draws the box and returns the width its contents actually wanted.
///
/// The contents are laid out in a child that is never told how wide to be, so the
/// measurement is of the content and not of the box. That is what stops it
/// feeding back on itself: the box is always at least as wide as its contents, so
/// nothing wraps, so the measurement does not change when the box grows.
fn sized_section(
    ui: &mut egui::Ui,
    title: &str,
    size: egui::Vec2,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::Vec2 {
    let mut content = egui::Vec2::ZERO;
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(size.x);
            ui.set_min_height(size.y);
            content = ui.scope(contents).response.rect.size();
        });
    });
    content
}

/// A number with its name under it, for a row of them.
///
/// Neither line wraps. The column is as wide as the number, which is narrower
/// than the word under it, so wrapping breaks the word in half.
fn counter(ui: &mut egui::Ui, name: &str, value: u64) {
    ui.vertical(|ui| {
        ui.add(unwrapped(egui::RichText::new(value.to_string()).strong()));
        ui.add(unwrapped(egui::RichText::new(name).weak()));
    });
}

/// A bar with its label inside it, filled up to `fraction`.
///
/// At zero nothing is filled. `egui::ProgressBar` widens its fill to the corner
/// radius so that the rounding has something to round, which draws a bubble on a
/// bar that has made no progress at all.
fn progress_bar(ui: &mut egui::Ui, label: &str, fraction: f32, width: f32) -> egui::Response {
    let fraction = fraction.clamp(0.0, 1.0);
    let height = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.style().visuals.clone();
    let rounding = height / 2.0;
    let painter = ui.painter();
    painter.rect(
        rect,
        rounding,
        visuals.extreme_bg_color,
        egui::Stroke::NONE,
    );
    if fraction > 0.0 {
        let filled = (rect.width() * fraction).max(2.0 * rounding);
        painter.rect(
            egui::Rect::from_min_size(rect.min, egui::vec2(filled, height)),
            rounding,
            visuals.selection.bg_fill,
            egui::Stroke::NONE,
        );
    }
    let galley = egui::WidgetText::from(format!("{label} {:.0}%", fraction * 100.0)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let at = rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0)
        + egui::vec2(ui.spacing().item_spacing.x, 0.0);
    let ink = visuals
        .override_text_color
        .unwrap_or(visuals.selection.stroke.color);
    ui.painter().with_clip_rect(rect).galley(at, galley, ink);
    response
}

/// A number and the word for it, which is not the same word when there is one of
/// them.
fn counted(how_many: u64, one: &str, more: &str) -> String {
    format!("{how_many} {}", if how_many == 1 { one } else { more })
}

/// A line of text cut to the room it is given.
///
/// `egui::Label` puts the whole string in a tooltip whenever it has to cut one,
/// and nothing in this window explains itself by being hovered over. Painting the
/// text rather than adding a widget leaves nothing to hover over.
fn clipped_line(ui: &mut egui::Ui, text: egui::RichText) {
    let room = ui.available_width();
    clipped_line_in(ui, text, room);
}

/// The same, in the room given rather than in whatever is left.
fn clipped_line_in(ui: &mut egui::Ui, text: egui::RichText, room: f32) {
    let room = room.max(0.0);
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        room,
        egui::TextStyle::Body,
    );
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    let ink = ui.visuals().text_color();
    ui.painter().with_clip_rect(rect).galley(rect.min, galley, ink);
}

/// A label that takes the width it needs rather than breaking to fit.
fn unwrapped(text: egui::RichText) -> egui::Label {
    egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend)
}

/// What the scan screen is showing.
#[derive(Debug, Default, Clone)]
struct ScanState {
    total: u64,
    done: u64,
    /// Whether the reading has begun, is going, or is over, and the same for the
    /// writing. A bar is a fraction only while the work it measures is running:
    /// before that it is empty and after it is full, whatever numbers are lying
    /// about from the listing or from the index.
    reading: Stage,
    writing: Stage,
    /// Pictures turned into a record, and how many of the folder are expected to
    /// become one.
    indexed: u64,
    to_index: u64,
    per_sec: u64,
    unchanged: u64,
    removed: u64,
    /// Read, and not a picture this build indexes. Not a failure and not work
    /// that produced anything, so it is not counted as either.
    ignored: u64,
    failures: Vec<(String, String)>,
    finished: Option<String>,
    /// Files the listing has found, while it is still listing. There is no total
    /// to measure it against until the listing is over, so this is a count, and
    /// it is the only thing there is to show for the part of a pass that used to
    /// show nothing at all.
    listing: Option<u64>,
}

/// Where one stage of a pass has got to.
///
/// A count out of a total is worth drawing while the stage producing them is the
/// one running, and at no other time. Before it starts there is nothing to
/// measure and the bar is empty; once it is over everything it was going to do
/// is done and the bar is full.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
enum Stage {
    #[default]
    Waiting,
    Running,
    Over,
}

impl Stage {
    /// Under way, unless it is already over: a report that arrives after the
    /// stage that sent it finished does not start it again.
    fn begun(self) -> Stage {
        match self {
            Stage::Over => Stage::Over,
            _ => Stage::Running,
        }
    }
}

/// What the search is doing, kept entirely apart from what the pass did.
///
/// The two have nothing to say about each other. When the pass has read and
/// indexed a folder its numbers are the answer and they stay on screen until a
/// new scan; the search reports its own work underneath them.
#[derive(Debug, Default, Clone)]
struct SearchState {
    /// The stage, or nothing when no search is running.
    stage: Option<&'static str>,
    /// Pictures read out of the index, of how many it holds. Zero for the total
    /// means it has not counted them yet.
    loaded: u64,
    to_load: u64,
    /// Whether this search is reading the index at all. A search that follows a
    /// pass is not: the pass built what it searches while it was scanning, and
    /// there is nothing left to read.
    reads_the_index: bool,
    /// Pictures the shortlist has looked up, of how many there are.
    shortlisted: u64,
    to_shortlist: u64,
    /// Pairs compared, of how many the shortlist produced.
    compared: u64,
    pairs: u64,
    /// Set when the search is over, so the bar stays full afterwards rather than
    /// emptying because nothing is reporting any more.
    done: bool,
}

impl SearchState {
    /// The search as one number out of one number.
    ///
    /// Stages of very different lengths, none of which knows its own size until
    /// it starts, so they share one bar an equal part each: reading the index,
    /// drawing up the shortlist, then comparing what it produced. Measured on a
    /// folder of ten thousand pictures those take four, eight and thirteen
    /// seconds, so an equal part each runs a little fast at the start and a
    /// little slow at the end, and never stands still.
    ///
    /// A search straight after a pass does no reading: the pass built what it
    /// searches. Giving that a part of its own would open the bar at a third for
    /// work that nothing is going to do, so the bar is the parts that are going
    /// to happen and no others.
    fn progress(&self) -> (u64, u64) {
        const PART: u64 = 1000;
        let reading = if self.reads_the_index { 1 } else { 0 };
        let whole = PART * (reading + 2);
        if self.pairs > 0 || self.compared > 0 {
            return (PART * (reading + 1) + fraction(self.compared, self.pairs, PART), whole);
        }
        if self.to_shortlist > 0 {
            return (PART * reading + fraction(self.shortlisted, self.to_shortlist, PART), whole);
        }
        if reading == 0 {
            // The count of what is in memory arrives before the work starts.
            // It is not progress: nothing has been done with it yet.
            return (0, whole);
        }
        (fraction(self.loaded, self.to_load, PART), whole)
    }
}

/// `part` of `whole`, scaled onto `out_of`. Nothing of nothing is nothing.
fn fraction(part: u64, whole: u64, out_of: u64) -> u64 {
    if whole == 0 {
        return 0;
    }
    (part.min(whole) * out_of) / whole
}

impl ScanState {
    /// Pictures this pass actually read. What was skipped over as not an image,
    /// and what could not be read at all, are their own numbers.
    /// Pictures this pass read. What was left alone was not read, and files that
    /// are not pictures or would not open are counted on their own.
    fn found(&self) -> u64 {
        self.done
            .saturating_sub(self.unchanged)
            .saturating_sub(self.ignored)
            .saturating_sub(self.failures.len() as u64)
    }
}

/// The things a pass goes through, each with a lamp on the scan page. Red until
/// the thing happens, green after.
///
/// The order here is the order they are drawn in, which is the order they were
/// asked for and not the order they happen in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Lamp {
    CheckedForIndexFile,
    StartedReadingTheIndexSettings,
    FinishedReadingTheIndexSettings,
    StartedOpeningTheIndexForWriting,
    FinishedOpeningTheIndexForWriting,
    StartedLookingForTheTotal,
    FoundTheTotal,
    LoadedIndexIntoMemory,
    ListedTheFolder,
    CrossReferencedWithTheIndex,
    CountedWhatChanged,
    StartedReadingNewFiles,
    FinishedReadingNewFiles,
    StartedIndexingNewFiles,
    FinishedIndexingNewFiles,
    StartedBuildingTheMemoryIndex,
    FinishedBuildingTheMemoryIndex,
    StartedFindingDuplicates,
    FinishedFindingDuplicates,
}

/// In the order a pass goes through them, which is also the order they read in.
///
/// The index is opened for writing first and its settings are read off that same
/// connection, so the two about opening come before the two about the settings.
///
/// One thing sits out of its running order: on a folder where nothing has
/// changed, the conversion happens as soon as the counting says there is nothing
/// to index, so those two lamps turn before the four about reading and indexing,
/// which are skipped rather than run.
const LAMPS: [(Lamp, &str); 19] = [
    (Lamp::CheckedForIndexFile, "Checked for sqlite file in this folder"),
    (Lamp::StartedOpeningTheIndexForWriting, "Started opening the index for writing"),
    (Lamp::FinishedOpeningTheIndexForWriting, "Finished opening the index for writing"),
    (
        Lamp::StartedReadingTheIndexSettings,
        "Started reading the index's own settings",
    ),
    (
        Lamp::FinishedReadingTheIndexSettings,
        "Finished reading the index's own settings",
    ),
    (Lamp::StartedLookingForTheTotal, "Started looking for total number of files"),
    (Lamp::FoundTheTotal, "Found total number of files"),
    (Lamp::ListedTheFolder, "Retrieved full file list in the folder"),
    (
        Lamp::LoadedIndexIntoMemory,
        "Loaded sqlite file into memory and constructed in-memory index",
    ),
    (
        Lamp::CrossReferencedWithTheIndex,
        "Cross referenced file list in folder with index from memory",
    ),
    (
        Lamp::CountedWhatChanged,
        "Finished finding number of new, unchanged, and removed files",
    ),
    (
        Lamp::StartedReadingNewFiles,
        "Starting individual file reads for any new file not in the index yet",
    ),
    (
        Lamp::FinishedReadingNewFiles,
        "Finished individual file reads for any new file not in the index yet",
    ),
    (
        Lamp::StartedIndexingNewFiles,
        "Starting indexing for new files not in the index yet",
    ),
    (
        Lamp::FinishedIndexingNewFiles,
        "Finished indexing for new files not in the index yet",
    ),
    (
        Lamp::StartedBuildingTheMemoryIndex,
        "Starting index conversion to in-memory datastructure",
    ),
    (
        Lamp::FinishedBuildingTheMemoryIndex,
        "Finished converting index to in-memory datastructure",
    ),
    (
        Lamp::StartedFindingDuplicates,
        "Started duplication computation given current settings",
    ),
    (Lamp::FinishedFindingDuplicates, "Finished duplication computation"),
];

impl From<scan::Step> for Lamp {
    fn from(step: scan::Step) -> Self {
        match step {
            scan::Step::StartedReadingTheIndexSettings => Lamp::StartedReadingTheIndexSettings,
            scan::Step::FinishedReadingTheIndexSettings => Lamp::FinishedReadingTheIndexSettings,
            scan::Step::StartedOpeningTheIndexForWriting => {
                Lamp::StartedOpeningTheIndexForWriting
            }
            scan::Step::FinishedOpeningTheIndexForWriting => {
                Lamp::FinishedOpeningTheIndexForWriting
            }
            scan::Step::StartedConvertingTheIndex => Lamp::StartedBuildingTheMemoryIndex,
            scan::Step::FinishedConvertingTheIndex => Lamp::FinishedBuildingTheMemoryIndex,
            scan::Step::StartedLookingForTheTotal => Lamp::StartedLookingForTheTotal,
            scan::Step::FoundTheTotal => Lamp::FoundTheTotal,
            scan::Step::LoadedIndexIntoMemory => Lamp::LoadedIndexIntoMemory,
            scan::Step::ListedTheFolder => Lamp::ListedTheFolder,
            scan::Step::CrossReferencedWithTheIndex => Lamp::CrossReferencedWithTheIndex,
            scan::Step::CountedWhatChanged => Lamp::CountedWhatChanged,
            scan::Step::StartedReadingNewFiles => Lamp::StartedReadingNewFiles,
            scan::Step::FinishedReadingNewFiles => Lamp::FinishedReadingNewFiles,
            scan::Step::StartedIndexingNewFiles => Lamp::StartedIndexingNewFiles,
            scan::Step::FinishedIndexingNewFiles => Lamp::FinishedIndexingNewFiles,
        }
    }
}

pub struct App {
    view: View,
    folder: Option<PathBuf>,
    /// The folder whose letters the window has already made sure it can draw.
    covered: Option<PathBuf>,
    db_path: Option<PathBuf>,
    /// The "Save an index database for this folder" box. Ticked when the folder
    /// is opened with an index file in it, unticked when it is opened without
    /// one; unticking it deletes the index, and a cleanup on a folder with it
    /// unticked deletes the index when it is done. It is not saved: the folder
    /// itself says whether it has an index.
    keep_index: bool,
    /// Folders that have been scanned, alphabetically. Choosing a folder does not
    /// put one here; scanning it does.
    previous: Vec<PathBuf>,
    recurse: bool,
    ignore_colour: bool,
    /// The ways of matching that are switched on. Both to begin with, and both
    /// kept in the folder's own index: a folder searched one way is searched
    /// that way again when it is opened.
    match_whole_frame: bool,
    match_corners: bool,
    /// Whether a folder scanned with its subfolders is searched one folder at a
    /// time. Off to begin with, and kept in the folder's own index.
    within_a_folder: bool,
    /// Pairs of pictures said not to be copies of each other, as the folder's
    /// index holds them. A set every pair of which is in here is left alone.
    ignored: std::collections::HashSet<(i64, i64)>,
    /// Whether opening this folder starts a pass by itself. Off to begin with,
    /// and kept in the index, which is also the thing it depends on: a folder
    /// with no index has nothing to run on opening.
    auto_rescan: bool,
    /// The index being read, on a thread of its own. The folder can be on
    /// another machine, and this is a file opened across the network before
    /// anything has been drawn.
    asking: Option<std::sync::mpsc::Receiver<Opened>>,
    /// Whether this folder's index has already said what it was set to. It says
    /// so once, when the folder is opened; a pass over the folder says it again,
    /// and by then the boxes are whoever pressed Scan's business, not the
    /// index's.
    noted: bool,
    /// How far apart two pictures may be and still count as the same one, as a
    /// share of the hash. The presets set this; the slider overrides them.
    sensitivity: f64,

    running: Option<Run>,
    scan: ScanState,
    /// The search for duplicates, while it is running.
    searching: Option<std::sync::mpsc::Receiver<Found>>,
    /// Set to stop the search. It is looked at between the pieces of the work.
    search_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,

    sets: Vec<DuplicateSet>,
    /// The keep marks per set, held here and not in the index: a review session
    /// is not a fact about a file on disk.
    keep: HashMap<i64, Keep>,
    /// Whether marking a picture adds to what its set keeps instead of taking
    /// the place of what is marked. A fact about reviewing this folder, so it is
    /// kept in the folder's index beside where a cleanup sends what it removes.
    multi_select: bool,

    destination: Destination,
    move_dir: String,
    /// The removal, while it is running, and how far through it is.
    removing: Option<std::sync::mpsc::Receiver<Removal>>,
    removed_so_far: usize,
    to_remove: usize,
    /// The files are gone and the index is being rewritten without them.
    tidying: bool,
    /// Files the last cleanup could not remove, and what the system said. Shown
    /// on the cleanup page so another destination can be tried.
    cleanup_failures: Vec<(String, String)>,
    cleanup_result: Option<String>,

    thumbs: Thumbnails,
    /// The file that was clicked, and the one whose picture is on screen. They
    /// differ while the clicked one is being read: the pane keeps drawing what it
    /// has until the new picture can replace it in one go, rather than blanking.
    selected: Option<i64>,
    showing: Option<i64>,
    /// A set row is the same height every time, but that height comes from the
    /// style and the text, so it is taken from the first row drawn and used to
    /// A row the cursor keys moved to that the list may not be showing, and what
    /// the list was scrolled to and how tall it was on the last frame.
    scroll_to: Option<usize>,
    /// The picture filling the window, put there by a click on the preview. A
    /// click anywhere or the escape key puts it back.
    filling_the_window: Option<i64>,
    /// What the file the preview is showing says about itself, and the reading
    /// of it, which happens off this thread.
    metadata: crate::metadata::Metadata,
    /// Set when the cursor keys move the preview, and cleared by the set holding
    /// it scrolling sideways far enough to show it. A set wider than the window
    /// is most of a review, and walking off the end of one used to move the
    /// selection to a picture that was not on screen.
    show_selected: bool,
    list_offset: f32,
    list_viewport: f32,
    /// A folder that already has an index is brought up to date on sight, which
    /// cannot happen until the window exists.
    scan_on_open: bool,
    /// Where the window is and how wide the preview pane is, read every frame and
    /// written out when the window closes. Writing on every change would rewrite
    /// the settings file throughout a drag.
    window: Option<crate::settings::Window>,
    preview_width: Option<f32>,

    error: Option<String>,
    /// What each box on the scan row measured last frame, so this frame can share
    /// the leftover width between them.
    scan_content: Vec<f32>,
    /// How tall the scan row's three boxes were last frame, so they end level
    /// with each other without any of them being a fixed height.
    scan_row: f32,
    /// What the search is doing, when one is running.
    search: SearchState,
    /// The index in the form the search works on, once it has been read.
    ///
    /// Nothing in it changes while the folder does not, so it is read out of the
    /// database once and kept. Moving the sensitivity and looking again costs the
    /// comparing and no storage at all. Dropped when the folder is changed or a
    /// pass rewrites the index, because then it is describing something else.
    images: Option<std::sync::Arc<Vec<matching::Image>>>,
    /// Which of the lamps on the scan page are green, and how many milliseconds
    /// into the run each one turned. The times are what say where the wait
    /// actually is.
    lit: HashMap<Lamp, u128>,
    /// What the lamp times are measured from: the start of this run.
    ///
    /// A folder that already has an index is scanned the moment the window opens,
    /// so for that run the start of the run is the start of the application and
    /// the window's own setup is part of the wait. A folder without one waits for
    /// the Scan button, and its clock starts there. Every run after that, whether
    /// it follows a cancel or not, starts its own.
    started: std::time::Instant,
}

impl Default for App {
    fn default() -> Self {
        App::from_settings(crate::settings::Settings::load())
    }
}

impl App {
    /// Build from a given set of settings rather than from whatever is on this
    /// machine, so tests do not depend on what the last real run left behind.
    fn from_settings(saved: crate::settings::Settings) -> Self {
        let db_path = saved.folder.as_deref().map(headless::default_db_path);
        // Look in the remembered folder for an index file. Whether one is there
        // decides whether the folder counts as remembered and whether it is
        // scanned on sight; the settings file is not asked.
        let has_index = db_path.as_deref().is_some_and(Path::is_file);
        // Whether the folder was looked in at all, which is what the first lamp
        // reports. A window opened with no folder has looked in nothing.
        let db_path_checked = db_path.is_some();
        App {
            view: View::Scan,
            folder: saved.folder,
            covered: None,
            db_path,
            keep_index: has_index,
            previous: crate::settings::sorted(&saved.previous),
            recurse: saved.recurse,
            ignore_colour: saved.ignore_colour,
            match_whole_frame: true,
            match_corners: true,
            within_a_folder: false,
            ignored: std::collections::HashSet::new(),
            auto_rescan: false,
            asking: None,
            noted: false,
            // What counts as a duplicate is a decision about the pictures in
            // front of the person making it, so every run starts on the default
            // rather than on whatever the last one was left at.
            sensitivity: matching::DEFAULT_SENSITIVITY,
            running: None,
            scan: ScanState::default(),
            searching: None,
            search_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sets: Vec::new(),
            keep: HashMap::new(),
            multi_select: false,
            destination: Destination::Trash,
            move_dir: String::new(),
            removing: None,
            removed_so_far: 0,
            to_remove: 0,
            tidying: false,
            cleanup_failures: Vec::new(),
            cleanup_result: None,
            thumbs: Thumbnails::new(),
            selected: None,
            showing: None,
            scroll_to: None,
            filling_the_window: None,
            metadata: crate::metadata::Metadata::default(),
            show_selected: false,
            list_offset: 0.0,
            list_viewport: 0.0,
            // The folder the window opened on is asked what it says about
            // itself once the window is up, which is what decides whether it is
            // also scanned. Nothing here can do that: it would be a file opened
            // across the network before a single frame had been drawn.
            scan_on_open: has_index,
            window: saved.window,
            preview_width: saved.preview_width,
            error: None,
            lit: {
                let mut lit = HashMap::new();
                // Looking in the folder for an index is what decided `has_index`
                // a few lines above, so this one is already true.
                if db_path_checked {
                    lit.insert(Lamp::CheckedForIndexFile, 0);
                }
                lit
            },
            search: SearchState::default(),
            images: None,
            started: std::time::Instant::now(),
            scan_content: vec![0.0; 3],
            scan_row: 0.0,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.note_window(ctx);
        // The folder is on screen from the moment it is chosen, and it can be in
        // any script. Asked once per folder, not once per frame.
        if self.covered != self.folder {
            self.covered.clone_from(&self.folder);
            if let Some(folder) = &self.folder {
                crate::fonts::cover(ctx, &folder.display().to_string());
            }
        }
        self.open_what_was_left_open();
        self.hear_the_index(ctx);
        self.take_dropped_folder(ctx);
        self.pump_indexer(ctx);
        self.pump_search(ctx);
        self.pump_cleanup(ctx);
        self.thumbs.collect(ctx);

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = self.have_sets();
                // A review holding nothing but sets somebody has said are not
                // copies has nothing for a cleanup to do, so there is nowhere
                // for that tab to go.
                let anything_to_clean = self.sets.iter().any(|set| !self.is_ignored(set));
                let tabs = [
                    (View::Scan, "1  Scan", true),
                    (View::Review, "2  Review", ready),
                    (View::Cleanup, "3  Clean up", ready && anything_to_clean),
                ];
                for (view, label, enabled) in tabs {
                    let selected = self.view == view;
                    let response = ui.add_enabled(
                        enabled,
                        egui::SelectableLabel::new(selected, label),
                    );
                    if response.clicked() {
                        self.view = view;
                    }
                }
            });
            ui.add_space(6.0);
        });

        if let Some(error) = self.error.clone() {
            egui::TopBottomPanel::bottom("error").show(ctx, |ui| {
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.colored_label(egui::Color32::from_rgb(200, 80, 80), error);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("dismiss").clicked() {
                            self.error = None;
                        }
                    });
                });
                ui.add_space(6.0);
            });
        }

        // The margin belongs to the panel, not to a frame drawn inside it. A frame
        // inside takes its bottom margin out of nothing: the content is given the
        // full height and then pushed down, so the last of it falls off the
        // bottom edge instead of ending there.
        let margin = match self.view {
            // The review's own toolbar is a panel across the page, and a panel
            // inside a margin draws its line inside that margin, which is a rule
            // that stops short of the one above it. The page keeps no margin
            // here and the parts of the review keep their own.
            View::Cleanup | View::Review => egui::Margin::ZERO,
            _ => egui::Margin::symmetric(16.0, 12.0),
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(margin))
            .show(ctx, |ui| match self.view {
                View::Scan => self.scan_view(ui),
                View::Review => self.review_view(ui),
                View::Cleanup => self.cleanup_view(ui),
            });

        self.filling_the_window(ctx);

        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.remember();
        // The indexer is a separate process. Left running it holds the index
        // open, and the next run of the application cannot write to it.
        if let Some(run) = self.running.as_mut() {
            run.cancel();
        }
        self.running = None;
    }
}

impl App {
    /// Turn a lamp green, noting how far into the run it happened. The first time
    /// only: a step that is reported twice keeps the time it first reached.
    fn light(&mut self, lamp: Lamp) {
        let at = self.started.elapsed().as_millis();
        self.lit.entry(lamp).or_insert(at);
    }

    /// Show a problem and put it in the log, so a report of one has something
    /// behind it.
    fn fail(&mut self, message: &str) {
        runlog::log_line!("ERROR {message}");
        self.error = Some(message.to_string());
    }

    /// Take whatever the pass has reported since the last frame.
    fn pump_indexer(&mut self, _ctx: &egui::Context) {
        let Some(run) = self.running.as_mut() else {
            return;
        };
        let waiting: Vec<Update> = run.updates.try_iter().collect();
        for update in waiting {
            match update {
                Update::Reached(step) => {
                    // The pass says which stage it is in. The bars follow that
                    // rather than guessing from numbers that came from a stage
                    // that is over.
                    match step {
                        scan::Step::StartedReadingNewFiles => self.scan.reading = Stage::Running,
                        scan::Step::FinishedReadingNewFiles => self.scan.reading = Stage::Over,
                        scan::Step::StartedIndexingNewFiles => self.scan.writing = Stage::Running,
                        scan::Step::FinishedIndexingNewFiles => self.scan.writing = Stage::Over,
                        _ => {}
                    }
                    self.light(step.into());
                }
                Update::Images(images) => self.images = Some(images),
                // A pass reads the index's settings off the connection it just
                // opened, and they are worth having for a folder that has not
                // been asked yet. A folder that has been asked already answered:
                // what the boxes say now is what the person at the window set
                // them to, and a pass does not put them back.
                Update::Settings(notes) => {
                    if !self.noted {
                        self.take_notes(notes);
                    }
                }
                Update::Walking { found, of } => {
                    // The listing has a line of its own above the bars. It is not
                    // the reading and does not move the reading's bar: nothing
                    // has been read while the folder is still being listed.
                    self.scan.listing = Some(found);
                    if let Some(total) = of {
                        self.scan.total = total;
                    }
                }
                Update::Start { total: 0 } => {
                    // Nothing in the folder. Everything lit up to here was the
                    // pass finding that out, and leaving those green claims a
                    // scan happened. It did not: there was nothing to scan.
                    self.lit.clear();
                    self.scan = ScanState {
                        finished: Some(String::from("No images found in this folder")),
                        ..ScanState::default()
                    };
                    self.search = SearchState::default();
                }
                Update::Start { total } => {
                    // The listing is over, so its line comes off the screen and
                    // the reading has a total to be measured against. Nothing has
                    // been read yet, and the bar says so until the first file is.
                    self.scan.total = total;
                    self.scan.done = 0;
                    self.scan.listing = None;
                    self.scan.indexed = 0;
                    self.scan.to_index = 0;
                }
                Update::Progress { done, per_sec, unchanged, removed, ignored } => {
                    // These come off the reading, so the reading is under way
                    // whether or not its step arrived first.
                    self.scan.reading = self.scan.reading.begun();
                    self.scan.done = done;
                    self.scan.per_sec = per_sec;
                    self.scan.unchanged = unchanged;
                    self.scan.removed = removed;
                    self.scan.ignored = ignored;
                }
                Update::Indexed { done, total, read, unchanged, ignored } => {
                    // Reading is over when everything in the folder has been read
                    // or was already in the index, which for a folder that has
                    // not changed is true before a single file is opened. The
                    // same for the writing.
                    self.scan.reading =
                        if read >= total { Stage::Over } else { self.scan.reading.begun() };
                    self.scan.writing =
                        if done >= total { Stage::Over } else { self.scan.writing.begun() };
                    self.scan.indexed = done;
                    self.scan.to_index = total;
                    // The counters under the bars come from the read side, and
                    // they move whichever bar it was that moved.
                    self.scan.done = read;
                    self.scan.unchanged = unchanged;
                    self.scan.ignored = ignored;
                }
                Update::Failed { path, message } => self.scan.failures.push((path, message)),
                Update::Done { indexed, removed, failed, elapsed_ms } => {
                    // The pass is over. Everything in the folder that was going
                    // to be read has been read and everything that was going to
                    // be indexed is in the index, including a folder where that
                    // was nothing at all: an index that already holds every file
                    // in the folder is a folder fully read and fully indexed, and
                    // no work has to run to make that true.
                    if self.scan.total > 0 {
                        self.scan.reading = Stage::Over;
                        self.scan.writing = Stage::Over;
                    }
                    self.scan.finished = Some(format!(
                        "indexed {indexed}, removed {removed}, failed {failed}, in {:.1}s",
                        elapsed_ms as f64 / 1000.0
                    ));
                }
                Update::Finished { cancelled, error } => {
                    self.running = None;
                    match error {
                        Some(message) => self.error = Some(message),
                        None if cancelled => {
                            self.scan.finished = Some(String::from("cancelled"))
                        }
                        // Nothing was read, so there is nothing to look through.
                        // Searching an empty folder lights two more lamps and
                        // fills a bar for work that cannot have happened.
                        None if self.scan.total == 0 => {}
                        None => self.load_sets(),
                    }
                    return;
                }
            }
        }
    }

    fn scan_view(&mut self, ui: &mut egui::Ui) {
        let step = ui.spacing().interact_size.y * 3.0;
        scrolled(
            ui,
            egui::Id::new("scan view"),
            true,
            step,
            egui::ScrollArea::vertical().auto_shrink([false, false]),
            |area, ui| {
                area.show(ui, |ui| {
                    // Everything on one row: the groups and the buttons are all
                    // short and stacking them full width leaves most of the
                    // window empty.
                    let widths =
                        share_row_width(ui.available_width(), &self.scan_content, SECTION_GAP);
                    let mut measured = self.scan_content.clone();
                    // The three boxes end level with each other, at the height of
                    // whichever holds the most. Nothing is a fixed height, so
                    // taking a control out takes its space with it.
                    let mut tallest = 0.0_f32;
                    ui.horizontal_top(|ui| {
                        let folder = self.folder_section(ui, widths[0]);
                        ui.add_space(SECTION_GAP);
                        let matching = self.matching_section(ui, widths[1]);
                        ui.add_space(SECTION_GAP);
                        let run = self.run_section(ui, widths[2]);
                        measured = vec![folder.x, matching.x, run.x];
                        tallest = folder.y.max(matching.y).max(run.y);
                    });
                    self.scan_content = measured;
                    self.scan_row = tallest;
                    self.progress_section(ui);
                    self.lamps(ui);
                })
            },
        );
    }

    /// One lamp per thing a pass goes through: red until it happens, green after,
    /// with the milliseconds since the application started at the end of the
    /// line. The gaps between those numbers are where the wait is.
    fn lamps(&mut self, ui: &mut egui::Ui) {
        const RED: egui::Color32 = egui::Color32::from_rgb(196, 62, 54);
        const GREEN: egui::Color32 = egui::Color32::from_rgb(58, 160, 78);
        const DOT: f32 = 5.0;

        const GREY: egui::Color32 = egui::Color32::from_rgb(150, 150, 150);

        ui.add_space(SECTION_GAP);
        let dot = |ui: &mut egui::Ui, state: Went| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(DOT * 3.0, ui.spacing().interact_size.y),
                egui::Sense::hover(),
            );
            match state {
                Went::Happened => ui.painter().circle_filled(rect.center(), DOT, GREEN),
                Went::Waiting => ui.painter().circle_filled(rect.center(), DOT, RED),
                // Nothing to do rather than not done yet: an empty ring, so a
                // pass that had no new files to read does not read as a pass
                // that failed to read them.
                Went::Skipped => ui.painter().circle_stroke(
                    rect.center(),
                    DOT,
                    egui::Stroke::new(1.5_f32, GREY),
                ),
            }
        };

        // The folder this is all about, before the things done to it. No time
        // against it: opening a folder is choosing one, not work that took a
        // while.
        ui.horizontal(|ui| {
            dot(ui, if self.folder.is_some() { Went::Happened } else { Went::Waiting });
            match &self.folder {
                Some(folder) => {
                    clipped_line(ui, egui::RichText::new(format!("Loaded {}", folder.display())))
                }
                None => {
                    ui.label(egui::RichText::new("No folder open").weak());
                }
            }
        });

        for (lamp, label) in LAMPS {
            let at = self.lit.get(&lamp).copied();
            ui.horizontal(|ui| {
                dot(ui, self.how_it_went(lamp));
                match at {
                    Some(at) => ui.label(format!("{label}  {at} ms")),
                    None => ui.label(egui::RichText::new(label).weak()),
                };
            });
        }
    }

    /// Whether a step happened, is still to happen, or was passed over.
    ///
    /// A pass over a folder where nothing has changed reads no files and indexes
    /// none, so the four steps for that never happen. They were not missed: there
    /// was nothing in them to do, and once the pass is past indexing that is
    /// what an unlit one of them means.
    fn how_it_went(&self, lamp: Lamp) -> Went {
        if self.lit.contains_key(&lamp) {
            return Went::Happened;
        }
        let of_the_files = matches!(
            lamp,
            Lamp::StartedReadingNewFiles
                | Lamp::FinishedReadingNewFiles
                | Lamp::StartedIndexingNewFiles
                | Lamp::FinishedIndexingNewFiles
        );
        if of_the_files && self.scan.writing == Stage::Over {
            return Went::Skipped;
        }
        Went::Waiting
    }

    fn folder_section(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Vec2 {
        let busy = self.busy();
        sized_section(ui, "Folder", egui::vec2(width, self.scan_row), |ui| {
            let inner = ui.max_rect();
            let row = ui
                .horizontal(|ui| {
                    if ui.add_enabled(!busy, egui::Button::new("Choose folder")).clicked() {
                        if let Some(folder) = crate::folder_picker::pick(self.folder.as_deref()) {
                            self.open_folder(folder);
                        }
                    }
                    // The previous button sits over the right end of this row, so
                    // the path stops before it rather than running under it.
                    let reserved = if self.previous.is_empty() { 0.0 } else { PREVIOUS_ROOM };
                    match &self.folder {
                        Some(folder) => clipped_line_in(
                            ui,
                            egui::RichText::new(folder.display().to_string()).strong(),
                            ui.available_width() - reserved,
                        ),
                        None => {
                            ui.label(egui::RichText::new("none chosen").weak());
                        }
                    };
                })
                .response
                .rect;
            // Against the right edge of the box, in a space of its own. Laying it
            // out with the row would count it as content, and the box is sized
            // from what its content measures.
            let strip = egui::Rect::from_min_max(
                egui::pos2(inner.left(), row.top()),
                egui::pos2(inner.right(), row.bottom()),
            );
            let mut against_the_edge = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(strip)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            self.previous_folders(&mut against_the_edge, busy);
            ui.add_space(6.0);
            let subfolders = ui.add_enabled(
                !busy,
                egui::Checkbox::new(&mut self.recurse, "Include subfolders"),
            );
            // Between the two, because it is about what the subfolders above it
            // mean: with it on, each of them is searched on its own and a
            // picture filed in two of them is two pictures. With no subfolders
            // there is only one folder, so it says nothing and cannot be ticked.
            let apart = ui.add_enabled(
                !busy && self.recurse,
                egui::Checkbox::new(&mut self.within_a_folder, "Only match within folders"),
            );
            let remember = ui.add_enabled(
                !busy,
                egui::Checkbox::new(
                    &mut self.keep_index,
                    "Save an index database for this folder",
                ),
            );
            // Under the index box and about it: what runs on opening is the
            // pass that brings the index up to date, so with no index kept there
            // is nothing to run and nothing to tick.
            let on_opening = ui.add_enabled(
                !busy && self.keep_index,
                egui::Checkbox::new(
                    &mut self.auto_rescan,
                    "Automatically rescan when opening this index",
                ),
            );
            if remember.changed() && !self.keep_index {
                // The index is what remembering a folder amounts to, so taking
                // the tick off takes the index with it. On its own thread: this
                // is up to three files removed, and when the folder is on another
                // machine that is three round trips the window would otherwise
                // sit through with the pointer as a spinning wheel. Nothing here
                // waits on the answer, and the box is already unticked.
                if let Some(db_path) = self.db_path.clone() {
                    std::thread::spawn(move || {
                        discard_index(Some(&db_path));
                    });
                }
                self.thumbs.forget();
                self.sets.clear();
                self.keep.clear();
                self.selected = None;
                self.showing = None;
                self.scan = ScanState::default();
            }
            // A folder worth keeping an index for is a folder worth bringing up
            // to date on sight, so saying yes to the one says yes to the other.
            // It can be turned off again; what it cannot be is on without an
            // index to rescan.
            if remember.changed() && self.keep_index {
                self.auto_rescan = true;
            }
            // A box that has just lost what it depends on comes off, and is
            // written out that way rather than left ticked in the index for the
            // next run to read back.
            let depended_on = subfolders.changed() || remember.changed();
            if depended_on {
                self.settle_the_boxes();
            }
            if apart.changed() || on_opening.changed() || depended_on {
                self.remember_ways_of_matching();
            }
            if subfolders.changed() || remember.changed() || apart.changed() {
                self.remember();
            }
        })
    }

    /// The folders scanned before, to go back to one of them without finding it
    /// in the file browser again. Nothing is offered until something has been
    /// scanned.
    fn previous_folders(&mut self, ui: &mut egui::Ui, busy: bool) {
        if self.previous.is_empty() {
            return;
        }
        let mut chosen = None;
        let mut forget = false;
        let button = ui.add_enabled(!busy, egui::Button::new("previous"));
        let list = egui::Id::new("previous folders");
        if button.clicked() {
            ui.memory_mut(|memory| memory.toggle_popup(list));
        }
        // A popup rather than part of the row: it is there for as long as it
        // takes to pick something, and it lies over the window rather than
        // making room in it.
        egui::popup_below_widget(
            ui,
            list,
            &button,
            egui::PopupCloseBehavior::CloseOnClick,
            |ui| {
                // Each folder on one line. The popup is as wide as the longest
                // one rather than wrapping paths into paragraphs.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                for folder in &self.previous {
                    let here = self.folder.as_deref() == Some(folder.as_path());
                    if ui.selectable_label(here, folder.display().to_string()).clicked() {
                        chosen = Some(folder.clone());
                    }
                }
                ui.separator();
                if ui.selectable_label(false, "clear previous locations").clicked() {
                    forget = true;
                }
            },
        );
        if forget {
            self.previous.clear();
            self.remember();
        }
        if let Some(folder) = chosen {
            self.open_folder(folder);
        }
    }

    /// Take down where the window is. A maximized window reports the size it
    /// covers the screen with, which is not what it should open at when it is
    /// restored, so that measurement is left as it was.
    fn note_window(&mut self, ctx: &egui::Context) {
        let (outer, inner, maximized) = ctx.input(|input| {
            let viewport = input.viewport();
            (viewport.outer_rect, viewport.inner_rect, viewport.maximized.unwrap_or(false))
        });
        if maximized {
            if let Some(window) = self.window.as_mut() {
                window.maximized = true;
            }
            return;
        }
        let (Some(outer), Some(inner)) = (outer, inner) else {
            return;
        };
        self.window = Some(crate::settings::Window {
            x: outer.min.x,
            y: outer.min.y,
            width: inner.width(),
            height: inner.height(),
            maximized: false,
        });
    }

    /// Take a folder dropped on the window: open it and scan it, which the search
    /// for duplicates follows on its own once the pass is over.
    ///
    /// Anything dropped that is not a folder is ignored, and so is a drop while
    /// there is work running, which the buttons are disabled for as well.
    fn take_dropped_folder(&mut self, ctx: &egui::Context) {
        if self.busy() {
            return;
        }
        let dropped = ctx.input(|input| {
            input
                .raw
                .dropped_files
                .iter()
                .filter_map(|file| file.path.clone())
                .find(|path| path.is_dir())
        });
        let Some(folder) = dropped else {
            return;
        };
        // Wherever the drop landed, this is a new folder at step one.
        self.view = View::Scan;
        self.open_folder(folder);
        // A folder that already had an index is scanning by now.
        if self.running.is_none() {
            self.start_scan();
        }
    }

    /// A folder that has been indexed before is brought up to date on sight. One
    /// that has not waits for the Scan button.
    fn open_folder(&mut self, folder: PathBuf) {
        let db_path = headless::default_db_path(&folder);
        let has_index = db_path.is_file();
        let elsewhere = self.folder.as_deref() != Some(folder.as_path());
        // What was read into memory describes the folder it was read from.
        if elsewhere {
            self.images = None;
        }
        self.db_path = Some(db_path);
        self.folder = Some(folder);
        // Everything about the last folder was about its pictures. A different
        // folder starts where a first look at a folder starts, and then takes
        // back whatever its own index has a record of.
        if elsewhere {
            self.sensitivity = matching::DEFAULT_SENSITIVITY;
            self.ignore_colour = false;
            self.recurse = false;
            // Both ways of matching, until this folder's index says otherwise,
            // and the whole folder at once rather than one folder at a time.
            self.match_whole_frame = true;
            self.match_corners = true;
            self.within_a_folder = false;
            // Whether opening a folder runs a pass is that folder's own answer,
            // and a folder that has not been asked yet has not said yes.
            self.auto_rescan = false;
            // A folder that has an index arrives with the box already ticked.
            self.keep_index = has_index;
            self.destination = Destination::Trash;
            self.move_dir = String::new();
        }
        self.sets.clear();
        self.keep.clear();
        self.selected = None;
        self.showing = None;
        // Counters, bars and whatever the last search said were about the folder
        // before this one, and a folder with no index does not start a pass that
        // would clear them.
        self.scan = ScanState::default();
        self.error = None;
        self.thumbs.forget();
        // What this folder's index says about itself, including whether opening
        // it is meant to start a pass and which of its sets are not sets of
        // copies. Asked off the thread that draws, and answered in a later frame.
        self.noted = false;
        self.remember();
        self.ask_the_index();
    }

    /// The folder the window opened on, asked about itself once, on the first
    /// frame. That folder is never chosen: it is set from what was saved before
    /// anything is drawn, so this is the only place it is ever asked.
    ///
    /// What its index says about itself, including whether opening it starts a
    /// pass, is read on a thread of its own: reading it here opened the index
    /// across the network and held the window for as long as that took, before
    /// anything had been drawn.
    fn open_what_was_left_open(&mut self) {
        if !self.scan_on_open {
            return;
        }
        self.scan_on_open = false;
        self.ask_the_index();
    }

    /// Ask the folder's index what it says about itself, on a thread of its
    /// own. A folder with no index says nothing, and the window keeps the
    /// answers a folder gives until another folder gives its own.
    fn ask_the_index(&mut self) {
        self.asking = None;
        // Whatever the last folder said is not true of this one. What this one
        // says arrives with its index.
        self.ignored.clear();
        let Some(db_path) = self.db_path.clone() else {
            return;
        };
        if !db_path.is_file() {
            return;
        }
        // Reading the index is the first thing done to a folder, so the clock
        // the lamps are timed against starts with it, the way it starts again
        // at the press of the Scan button.
        self.started = std::time::Instant::now();
        self.lit.clear();
        self.lit.insert(Lamp::CheckedForIndexFile, 0);
        let (send, receive) = std::sync::mpsc::channel();
        self.asking = Some(receive);
        std::thread::spawn(move || {
            if let Ok(notes) = crate::notes::of_folder(&db_path) {
                let _ = send.send(Opened::Notes(notes));
            }
            // And the index itself. Having it in memory is what a folder that
            // has been scanned before is for: the pictures can be searched
            // without reading anything again, whether or not a pass runs.
            let read = imgdedupe_core::db::open_snapshot(&db_path).and_then(|conn| {
                let never = std::sync::atomic::AtomicBool::new(false);
                let report = |progress| {
                    let _ = send.send(Opened::Reading(progress));
                };
                // Off the same connection as the pictures: the pairs are part of
                // what the index holds about this folder, and after this they
                // are in memory and nothing asks the index about them again.
                let _ = send.send(Opened::Ignored(imgdedupe_core::db::ignored(&conn)?));
                let images = matching::load_images(&conn, &never, &report)?;
                let _ = conn.close();
                Ok(images)
            });
            match read {
                Ok(Some(images)) => {
                    let _ = send.send(Opened::Index(std::sync::Arc::new(images)));
                }
                Ok(None) => {}
                Err(err) => {
                    let _ = send.send(Opened::Failed(format!("{err:#}")));
                }
            }
        });
    }

    /// Take what the index says as it arrives: what the folder was set to, then
    /// the pictures themselves. A pass is started on top of that only when the
    /// folder asked to be rescanned on opening.
    fn hear_the_index(&mut self, ctx: &egui::Context) {
        let Some(asking) = &self.asking else {
            return;
        };
        let mut arrived = Vec::new();
        let mut over = false;
        loop {
            match asking.try_recv() {
                Ok(said) => arrived.push(said),
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    over = true;
                    break;
                }
            }
        }
        if arrived.is_empty() && !over {
            return;
        }
        for said in arrived {
            match said {
                Opened::Notes(notes) => {
                    self.take_notes(notes);
                    // A folder that asks to be brought up to date on sight is
                    // started here rather than when its pictures arrive: the
                    // pass reads the index itself, and there is nothing to be
                    // gained by waiting for a second copy of it.
                    if self.auto_rescan && !self.busy() {
                        self.start_scan();
                    }
                }
                Opened::Reading(progress) => self.note_search_progress(progress),
                Opened::Ignored(pairs) => self.ignored.extend(pairs),
                Opened::Index(images) => {
                    self.light(Lamp::LoadedIndexIntoMemory);
                    // A pass that started in the meantime is reading the same
                    // folder and will hand over its own, newer copy.
                    if self.running.is_none() {
                        self.images = Some(images);
                    }
                    self.search = SearchState::default();
                }
                Opened::Failed(err) => self.error = Some(err),
            }
        }
        if over {
            self.asking = None;
        }
        ctx.request_repaint();
    }

    /// What a search says while it runs, and what the reading of an index says
    /// on the way to one. The same bar draws both: reading an index on opening
    /// a folder is the same work a search would otherwise have done itself.
    fn note_search_progress(&mut self, progress: matching::Progress) {
        match progress {
            matching::Progress::Loading { done, total } => {
                self.light(Lamp::StartedBuildingTheMemoryIndex);
                self.search.stage = Some("reading the index");
                self.search.reads_the_index = true;
                self.search.loaded = done;
                self.search.to_load = total;
            }
            matching::Progress::Loaded { images } => {
                self.light(Lamp::FinishedBuildingTheMemoryIndex);
                self.search.stage = Some("comparing");
                self.search.loaded = images;
                self.search.to_load = self.search.to_load.max(images);
            }
            matching::Progress::Shortlisting { done, total } => {
                self.search.stage = Some("shortlisting");
                self.search.shortlisted = done;
                self.search.to_shortlist = total;
            }
            matching::Progress::Comparing { done, total } => {
                self.search.stage = Some("comparing");
                self.search.compared = done;
                self.search.pairs = total;
            }
            matching::Progress::Grouping => {
                self.search.stage = Some("grouping");
                self.search.compared = self.search.pairs;
            }
        }
    }

    /// Write everything the window was left set to, so the next run opens the
    /// same way.
    fn remember(&self) {
        self.settings().save();
    }

    /// What the next run would be started from. The sensitivity is not in it: it
    /// is a decision about the pictures on screen, not a preference.
    fn settings(&self) -> crate::settings::Settings {
        crate::settings::Settings {
            folder: self.folder.clone(),
            previous: self.previous.clone(),
            recurse: self.recurse,
            ignore_colour: self.ignore_colour,
            window: self.window,
            preview_width: self.preview_width,
        }
    }

    fn matching_section(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Vec2 {
        let busy = self.busy();
        let row = self.scan_row;
        sized_section(
            ui,
            "What counts as a duplicate",
            egui::vec2(width, row),
            |ui| {
            ui.spacing_mut().slider_width = 300.0;
            // The number beside the slider is drawn in a box of this width, and
            // the box would otherwise size to the digits in it. The row's boxes
            // are shared out by what their contents measure, so 5.0 and 30.0
            // would each want a different share and move all three.
            ui.spacing_mut().interact_size.x = VALUE_WIDTH;
            let mut changed = ui
                .add_enabled(
                    !busy,
                    egui::Slider::new(&mut self.sensitivity, 0.5..=matching::MAX_SENSITIVITY)
                        .suffix(" %")
                        .fixed_decimals(1)
                        .text("difference allowed"),
                )
                .changed();
            ui.horizontal(|ui| {
                ui.label("presets:");
                ui.spacing_mut().button_padding = PRESET_PADDING;
                for (name, percent) in matching::PRESETS {
                    let here = on_preset(self.sensitivity, percent);
                    // The one the slider is on is drawn as pressed, so the row
                    // says where the setting is as well as where it can go.
                    let button = egui::Button::new(name).selected(here);
                    if ui.add_enabled(!busy, button).clicked() {
                        self.sensitivity = percent;
                        changed = true;
                    }
                }
            });
            ui.add_space(6.0);
            // The two ways of matching, either of which can be left out. The
            // first is nearly free and finds resizes, recompressions and
            // rotations; the second is most of what a search costs and is what
            // finds a crop. They come before the colour box, which is a change
            // to how the first of them decides rather than a way of its own.
            let ways = ui
                .add_enabled(
                    !busy,
                    egui::Checkbox::new(&mut self.match_whole_frame, "Match whole pictures"),
                )
                .changed()
                | ui.add_enabled(
                    !busy,
                    egui::Checkbox::new(&mut self.match_corners, "Match partials"),
                )
                .changed();
            changed |= ui
                .add_enabled(
                    !busy,
                    egui::Checkbox::new(
                        &mut self.ignore_colour,
                        "Match colour with grayscale",
                    ),
                )
                .changed();
            if ways {
                self.remember_ways_of_matching();
            }
            if changed || ways {
                self.remember();
            }
            },
        )
    }

    fn run_section(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Vec2 {
        // Cancel covers the indexing and the search, which are the two a person
        // waits through. It does not cover a removal: files are going, and
        // stopping halfway leaves a job half done with nothing said about it.
        let stoppable = self.running.is_some() || self.searching.is_some();
        let busy = self.busy();
        let have_folder = self.folder.is_some();

        sized_section(ui, "Run", egui::vec2(width, self.scan_row), |ui| {
            ui.horizontal(|ui| {
                let start = ui.add_enabled(
                    !busy && have_folder,
                    egui::Button::new(egui::RichText::new("Scan").strong())
                        .min_size(egui::vec2(90.0, 30.0)),
                );
                if start.clicked() {
                    self.start_scan();
                }
                if ui
                    .add_enabled(
                        stoppable,
                        egui::Button::new("Cancel").min_size(egui::vec2(80.0, 30.0)),
                    )
                    .clicked()
                {
                    self.cancel_work();
                }
            });
            // A button's label sits where the layout puts it, and a row's layout
            // starts at the left, so the width `min_size` adds all lands on the
            // right of the text.
            let wide = egui::vec2(178.0, 30.0);
            let found = ui
                .allocate_ui_with_layout(
                    wide,
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.add_enabled(
                            !busy && self.db_path.is_some(),
                            egui::Button::new("Find duplicates").min_size(wide),
                        )
                    },
                )
                .inner;
            if found.clicked() {
                // Looking for duplicates in a folder that has not been read is
                // looking at nothing. The pass comes first and searches when it
                // is done, which is the same thing that happens when a folder
                // with an index is opened.
                if self.images.is_some() {
                    self.load_sets();
                } else {
                    self.start_scan();
                }
            }
        })
    }

    fn progress_section(&mut self, ui: &mut egui::Ui) {
        let running = self.running.is_some();
        let searching = self.searching.is_some();
        if !running && !searching && self.scan.total == 0 && self.scan.finished.is_none() {
            return;
        }

        ui.add_space(SECTION_GAP);
        section(ui, "Progress", |ui| {
            // Before the folder has been listed there is no total, so there is no
            // fraction and the bars have nothing to show. The count is what there
            // is, and it is the difference between a window that is working and a
            // window that looks stopped.
            if let Some(found) = self.scan.listing {
                ui.label(format!("listing the folder: {}", counted(found, "file", "files")));
                ui.add_space(6.0);
            }
            let width = ui.available_width();
            // A bar is a fraction only while the stage it measures is the one
            // running. Before that there is nothing to divide and it is empty;
            // after it, everything that stage was going to do is done and it is
            // full. A stage that never ran leaves its bar empty, which is what
            // happened: nothing.
            let bar = |ui: &mut egui::Ui, label: &str, stage: Stage, count: u64, out_of: u64| {
                let fraction = match stage {
                    Stage::Waiting => 0.0,
                    Stage::Running if out_of == 0 => 0.0,
                    Stage::Running => count as f32 / out_of as f32,
                    Stage::Over => 1.0,
                };
                progress_bar(ui, label, fraction, width);
            };
            bar(ui, "Scanning for files", self.scan.reading, self.scan.done, self.scan.total);
            ui.add_space(4.0);
            bar(ui, "Indexing files", self.scan.writing, self.scan.indexed, self.scan.to_index);
            ui.add_space(4.0);
            // The search's own bar, under the pass's two and driven by nothing
            // they touch. It is two stages of very different lengths, so it runs
            // over both: reading the index fills the first part of it, comparing
            // the pairs that reading produced fills the rest.
            let (duplicates, of) = self.search.progress();
            let searching = match (self.search.done, self.search.stage.is_some()) {
                (true, _) => Stage::Over,
                (false, true) => Stage::Running,
                (false, false) => Stage::Waiting,
            };
            bar(ui, "Finding duplicates", searching, duplicates, of);
            ui.add_space(6.0);
            // How many files the folder holds. The listing is what produces that
            // number, so before it is over the count it has reached so far is
            // what there is.
            let in_folder = if self.scan.total > 0 {
                self.scan.total
            } else {
                self.scan.listing.unwrap_or(0)
            };
            egui::Grid::new("scan counts")
                .num_columns(6)
                .spacing([24.0, 4.0])
                .show(ui, |ui| {
                    counter(ui, "found", in_folder);
                    // Files this pass has read, which are the ones the index did
                    // not already have. This was labelled "found", which is the
                    // folder's count, not this.
                    counter(ui, "new", self.scan.found());
                    counter(ui, "unchanged", self.scan.unchanged);
                    counter(ui, "removed", self.scan.removed);
                    counter(ui, "failed to read", self.scan.failures.len() as u64);
                    counter(ui, "per second", self.scan.per_sec);
                    ui.end_row();
                });
            if let Some(finished) = &self.scan.finished {
                ui.add_space(6.0);
                ui.label(egui::RichText::new(finished).strong());
            }
            if !self.scan.failures.is_empty() {
                ui.add_space(6.0);
                egui::CollapsingHeader::new(format!(
                    "{} files could not be read",
                    self.scan.failures.len()
                ))
                .show(ui, |ui| {
                    let line = ui.text_style_height(&egui::TextStyle::Body);
                    ui.set_max_height(160.0);
                    scrolled(
                        ui,
                        egui::Id::new("scan failures"),
                        true,
                        line,
                        egui::ScrollArea::vertical().max_height(160.0),
                        |area, ui| {
                            area.show(ui, |ui| {
                                for (path, message) in &self.scan.failures {
                                    ui.label(format!("{path}: {message}"));
                                }
                            })
                        },
                    );
                });
            }
        });
    }

    fn start_scan(&mut self) {
        let (Some(folder), Some(db_path)) = (self.folder.clone(), self.db_path.clone()) else {
            return;
        };
        // The clock the lamps are timed against starts here, at the press of the
        // button: what the numbers beside them answer is how long this run has
        // been going, not how long the window has been open.
        self.started = std::time::Instant::now();
        self.lit.clear();

        self.scan = ScanState::default();
        // The duplicates bar is drawn from this, and it is a different set of
        // numbers from the pass's. Leaving it alone left the last run's finished
        // bar full while a new run started underneath it.
        self.search = SearchState::default();
        self.error = None;
        // A pass rewrites the index, so what was read out of it before describes
        // a folder that no longer exists in that form, and so does everything
        // that came out of it: the sets, what was marked to keep in them, what is
        // selected, and the pictures loaded for them.
        self.images = None;
        self.sets.clear();
        self.keep.clear();
        self.selected = None;
        self.showing = None;
        self.thumbs.forget();

        // Look for an index every time a pass starts, not only when the folder is
        // opened. One may have been put there since, by hand or by a copy of the
        // folder, and finding it is the difference between reading nine thousand
        // files and reading none of them. A folder that has one keeps it, which
        // is what the checkbox means.
        let has_index = db_path.is_file();
        self.light(Lamp::CheckedForIndexFile);
        if has_index {
            self.keep_index = true;
        }
        // A folder counts as one worth offering again once it has been scanned.
        // Choosing one and thinking better of it does not put it in the list.
        if !self.previous.contains(&folder) {
            self.previous.push(folder.clone());
            self.previous = crate::settings::sorted(&self.previous);
            self.remember();
        }
        match indexer::start(&folder, &db_path, self.recurse) {
            Ok(run) => self.running = Some(run),
            Err(err) => self.fail(&format!("{err:#}")),
        }
    }

    /// Start the search for duplicates. It runs on its own thread: it is several
    /// seconds of SQLite on a large folder, and doing it here would stop the
    /// window painting, so nothing could be shown about it while it happened.
    /// Search the index that is in memory.
    ///
    /// The database is not a source for this. It exists so that opening a folder
    /// that has been indexed before is fast, and the pass converts it to the form
    /// the search works on as part of loading it. If there is no such structure
    /// there is nothing to search, and the caller runs a pass first.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn load_sets(&mut self) {
        let Some(db_path) = self.db_path.clone() else {
            return;
        };
        let Some(held) = self.images.clone() else {
            return;
        };
        if self.searching.is_some() {
            return;
        }
        self.light(Lamp::StartedFindingDuplicates);
        self.search = SearchState { stage: Some("starting"), ..SearchState::default() };
        let mut thresholds = Thresholds::at(self.sensitivity);
        thresholds.ignore_colour = self.ignore_colour;
        thresholds.whole_frame = self.match_whole_frame;
        thresholds.corners = self.match_corners;
        thresholds.within_a_folder = self.within_a_folder;
        runlog::log_line!(
            "matching {} at {:.1}% ({} bits), ignore_colour {}, whole frame {}, corners {}, \
             within a folder {}",
            db_path.display(),
            self.sensitivity,
            thresholds.max_bits,
            thresholds.ignore_colour,
            thresholds.whole_frame,
            thresholds.corners,
            thresholds.within_a_folder
        );

        // What the last pass ended with says nothing about this one.
        self.scan.finished = None;
        self.error = None;

        let (send, receive) = std::sync::mpsc::channel::<Found>();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let asked = std::sync::Arc::clone(&stop);
        let telling = send.clone();
        std::thread::spawn(move || {
            let report = |progress| {
                let _ = telling.send(Found::Progress(progress));
            };
            let result = matching::find_sets_in(&held, thresholds, &asked, &report);
            let _ = send.send(match result {
                Ok(Some(sets)) => Found::Sets(sets),
                Ok(None) => Found::Cancelled,
                Err(err) => Found::Failed(format!("{err:#}")),
            });
        });
        self.search_cancel = stop;
        self.searching = Some(receive);
    }

    /// Take what the search has sent. Called once per frame.
    fn pump_search(&mut self, ctx: &egui::Context) {
        let Some(receive) = &self.searching else {
            return;
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let waiting: Vec<Found> = receive.try_iter().collect();
        let mut done = false;
        for found in waiting {
            match found {
                Found::Progress(progress) => self.note_search_progress(progress),
                Found::Sets(sets) => {
                    self.light(Lamp::FinishedFindingDuplicates);
                    self.search.stage = Some("finished");
                    self.search.done = true;
                    self.accept_sets(sets);
                    // The names about to go on screen. If any of them is in a
                    // script the bundled face does not have, this is where the
                    // machine gets asked for one that does.
                    let names: String = self
                        .sets
                        .iter()
                        .flat_map(|set| set.members.iter())
                        .map(|member| member.rel_path.as_str())
                        .collect();
                    crate::fonts::cover(ctx, &names);
                    done = true;
                }
                Found::Cancelled => {
                    runlog::log_line!("the search was cancelled");
                    self.scan.finished = Some(String::from("cancelled"));
                    done = true;
                }
                Found::Failed(message) => {
                    self.fail(&message);
                    done = true;
                }
            }
        }
        if done {
            self.searching = None;
        }
    }

    fn accept_sets(&mut self, sets: Vec<DuplicateSet>) {
        runlog::log_line!("found {} duplicate sets", sets.len());
        self.keep.clear();
        self.selected = None;
        self.showing = None;
        // The pictures were read against the index as it stood for the last
        // result. This one names them by the same numbers and can mean other
        // files by them.
        self.thumbs.forget();

        // Nothing was found, so there is nothing to review. Say so and stay here.
        if sets.is_empty() {
            self.sets.clear();
            // Nothing to compare and nothing found are different answers. A
            // folder with no pictures in it was never searched, and saying no
            // duplicates were found in it claims otherwise.
            self.scan.finished = Some(String::from(if self.scan.total == 0 {
                "No files in this folder"
            } else {
                "No duplicates found for current settings"
            }));
            return;
        }

        for set in &sets {
            if let Some(member) = set.members.iter().find(|member| member.auto_keep) {
                self.keep.insert(set.set_id, Keep::One(member.file_id));
            }
        }
        self.sets = sets;
        self.preselect_first_keeper();
        if let Some(root) = self.folder.clone() {
            // The picture the pane opens on is asked for before the thumbnails,
            // so it is not queued behind every one of them and the pane has
            // something in it when the view arrives.
            if let Some(opening) = self.selected.and_then(|file_id| {
                self.sets
                    .iter()
                    .flat_map(|set| set.members.iter())
                    .find(|member| member.file_id == file_id)
                    .map(|member| (member.file_id, member.rel_path.clone()))
            }) {
                self.thumbs.prime(
                    &root,
                    std::iter::once((opening.0, opening.1.as_str())),
                    thumbs::LARGE_EDGE,
                );
            }
            self.thumbs.prime(
                &root,
                self.sets.iter().flat_map(|set| {
                    set.members.iter().map(|member| (member.file_id, member.rel_path.as_str()))
                }),
                thumbs::THUMB_EDGE,
            );
        }
        self.view = View::Review;
    }

    fn review_view(&mut self, ui: &mut egui::Ui) {
        let Some(root) = self.folder.clone() else {
            ui.label("no folder chosen");
            return;
        };

        let visible: Vec<usize> = (0..self.sets.len()).collect();
        let duplicates = duplicate_count(&self.sets);
        let (going, reclaimable) = self.selected_for_removal();

        egui::TopBottomPanel::top("review toolbar").show_inside(ui, |ui| {
            ui.add_space(4.0);
            // Three lots that share one row: the checkbox against the left edge,
            // the counts in the middle of the window, and the button against the
            // right edge. They are laid out over the same rectangle, each with
            // the layout that puts it where it belongs, so the counts sit in the
            // centre of the row rather than in the centre of what is left of it.
            // The panel runs the width of the window, so its own line does too.
            // What is in it keeps the page's margin.
            let row = egui::vec2(ui.available_width(), TOOLBAR_HEIGHT);
            let (rect, _) = ui.allocate_exact_size(row, egui::Sense::hover());
            let rect = rect.shrink2(egui::vec2(PAGE_MARGIN, 0.0));

            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if left.checkbox(&mut self.multi_select, "allow multi-select").changed() {
                self.remember_multi_select();
            }

            let mut middle = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            // One line rather than four labels beside each other: a row of
            // widgets is laid out from where the row starts, and only a single
            // thing can be put in the middle of the space it is given.
            let counts = count_line(&middle, visible.len(), duplicates, going, reclaimable);
            middle.add(egui::Label::new(counts).wrap_mode(egui::TextWrapMode::Extend));

            let mut right = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            let go = egui::Button::new(
                egui::RichText::new("Clean up").strong().color(egui::Color32::WHITE),
            )
            .fill(egui::Color32::from_rgb(60, 110, 180))
            .min_size(egui::vec2(120.0, 28.0));
            if right.add_enabled(going > 0, go).clicked() {
                self.view = View::Cleanup;
            }
            ui.add_space(4.0);
        });

        for (key, direction) in [
            (egui::Key::ArrowRight, Direction::Forward),
            (egui::Key::ArrowLeft, Direction::Back),
            (egui::Key::ArrowDown, Direction::NextSet),
            (egui::Key::ArrowUp, Direction::PreviousSet),
        ] {
            if ui.input(|input| input.key_pressed(key)) {
                self.walk(&visible, direction);
            }
        }
        if ui.input(|input| input.key_pressed(egui::Key::Space)) {
            self.keep_selected();
        }

        self.preview_pane(ui, &root);

        // Only the rows on screen are built, so a folder with thousands of sets
        // costs the same per frame as one with ten.
        let row_height = set_row_height(ui);
        let spacing = ui.spacing().item_spacing.y;
        let mut list = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(row) = self.scroll_to.take() {
            if let Some(offset) = scroll_to_show(
                row,
                visible.len(),
                row_height,
                spacing,
                self.list_offset,
                self.list_viewport,
            ) {
                list = list.vertical_scroll_offset(offset);
            }
        }

        // The page's margin down the left of the list and a gap above the first
        // set, kept here rather than by the panel, so the panels above can run
        // the width of the window and draw their lines across all of it. Put
        // into the rectangle the list is drawn in, because that is the one thing
        // that decides where the list starts.
        let room = ui.available_rect_before_wrap();
        let room = egui::Rect::from_min_max(
            egui::pos2(room.left() + PAGE_MARGIN, room.top() + SECTION_GAP),
            room.max,
        );
        // What a row comes out as, worked out here where the list's own room is
        // known: inside a scroll area nothing is told where the area ends.
        let row_width = (room.width() - SCROLL_BAR - PAGE_MARGIN).max(0.0);

        let (_, offset, viewport) = ui
            .allocate_new_ui(egui::UiBuilder::new().max_rect(room), |ui| {
                let list_id = egui::Id::new("review list");
                scrolled(
                    ui,
                    list_id,
                    true,
                    row_height + spacing,
                    list,
                    |list, ui| {
                        list.show_rows(ui, row_height, visible.len(), |ui, range| {
                            for position in range {
                                let index = visible[position];
                                self.set_row(ui, index, &root, row_width);
                            }
                        })
                    },
                )
            })
            .inner;
        self.list_offset = offset;
        self.list_viewport = viewport;
    }

    /// Whether marking a picture adds to what its set keeps. This belongs to the
    /// folder as well: a folder reviewed one picture at a time is reviewed that
    /// way again the next time it is opened.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn remember_multi_select(&self) {
        let Some(db_path) = &self.db_path else {
            return;
        };
        let stored = if self.multi_select { "1" } else { "0" };
        let result =
            db::open_for_notes(db_path).and_then(|conn| db::set_meta(&conn, "multi_select", stored));
        if let Err(err) = result {
            runlog::log_line!("the multi-select choice could not be written: {err:#}");
        }
    }

    /// Whether every picture in a set has been said not to be a copy of every
    /// other picture in it. A set like that is shown, so it can be seen and
    /// changed back, and nothing else in the program acts on it.
    ///
    /// Every pair, not some: a set where two of five have been separated is
    /// still a set of copies, and the three that are copies still are.
    fn is_ignored(&self, set: &DuplicateSet) -> bool {
        if set.members.len() < 2 {
            return false;
        }
        pairs_of(set).all(|pair| self.ignored.contains(&pair))
    }

    /// Say that none of the pictures in this set are copies of each other, and
    /// write that down in the folder's index so the next search knows it too.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn ignore_set(&mut self, set_id: i64) {
        let Some(set) = self.sets.iter().find(|set| set.set_id == set_id) else {
            return;
        };
        let pairs: Vec<(i64, i64)> = pairs_of(set).collect();
        self.ignored.extend(pairs.iter().copied());
        // What the set kept stays with it, and so does where the preview was.
        // Neither is acted on while it is ignored: nothing goes from a set that
        // is not a set of copies, and no ring is drawn round a picture in one.
        // Both are what taking it back gives back — the mark is where it was —
        // and the preview is where the cursor keys walk from: left and up out of
        // a set that has just been ignored go to the set before it, right and
        // down to the set after it, which they cannot do from nowhere.
        let Some(db_path) = &self.db_path else {
            return;
        };
        let result =
            db::open_for_notes(db_path).and_then(|conn| db::ignore(&conn, &pairs));
        if let Err(err) = result {
            runlog::log_line!("the ignored pairs could not be written: {err:#}");
        }
    }

    /// Take it back: the pictures in this set are copies of each other after
    /// all. What was written down goes, and the set is a set again.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn unignore_set(&mut self, set_id: i64) {
        let Some(set) = self.sets.iter().find(|set| set.set_id == set_id) else {
            return;
        };
        let pairs: Vec<(i64, i64)> = pairs_of(set).collect();
        for pair in &pairs {
            self.ignored.remove(pair);
        }
        let Some(db_path) = &self.db_path else {
            return;
        };
        let result = db::open_for_notes(db_path).and_then(|conn| db::unignore(&conn, &pairs));
        if let Err(err) = result {
            runlog::log_line!("the ignored pairs could not be taken back: {err:#}");
        }
    }

    /// Take what a folder's index says about itself. Every one of these belongs
    /// to the folder rather than to the program, and a folder that has never
    /// said leaves what is on screen alone.
    fn take_notes(&mut self, notes: crate::notes::Notes) {
        self.noted = true;
        // An index built over the subfolders has to be scanned that way again,
        // or the next pass drops every row under them.
        if let Some(setting) = notes.recurse {
            self.recurse = setting;
        }
        if let Some(choice) = notes.disposal.as_deref().and_then(Destination::from_name) {
            self.destination = choice;
        }
        if let Some(folder) = notes.move_dir {
            self.move_dir = folder;
        }
        if let Some(setting) = notes.multi_select {
            self.multi_select = setting;
        }
        if let Some(setting) = notes.match_whole_frame {
            self.match_whole_frame = setting;
        }
        if let Some(setting) = notes.match_corners {
            self.match_corners = setting;
        }
        if let Some(setting) = notes.within_a_folder {
            self.within_a_folder = setting;
        }
        if let Some(setting) = notes.auto_rescan {
            self.auto_rescan = setting;
        }
        // Neither box means anything without the one above it, and a box that
        // means nothing is not left ticked.
        self.settle_the_boxes();
    }

    /// The boxes that depend on another box. Ticked, they say something; with
    /// what they depend on switched off they say nothing, so they come off and
    /// cannot be put back on until it returns.
    fn settle_the_boxes(&mut self) {
        if !self.recurse {
            self.within_a_folder = false;
        }
        if !self.keep_index {
            self.auto_rescan = false;
        }
    }

    /// Which ways of matching this folder is searched with. A fact about the
    /// folder: a folder where crops matter is searched for crops every time it
    /// is opened, and one where they do not is not made to wait for them.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn remember_ways_of_matching(&self) {
        let Some(db_path) = &self.db_path else {
            return;
        };
        use crate::notes::{mark, MATCH_CORNERS, MATCH_WHOLE_FRAME, AUTO_RESCAN, WITHIN_A_FOLDER};
        let result = db::open_for_notes(db_path).and_then(|conn| {
            db::set_meta(&conn, MATCH_WHOLE_FRAME, mark(self.match_whole_frame))?;
            db::set_meta(&conn, MATCH_CORNERS, mark(self.match_corners))?;
            db::set_meta(&conn, WITHIN_A_FOLDER, mark(self.within_a_folder))?;
            db::set_meta(&conn, AUTO_RESCAN, mark(self.auto_rescan))
        });
        if let Err(err) = result {
            runlog::log_line!("the ways of matching could not be written: {err:#}");
        }
    }

    /// Where this folder's duplicates go, and the folder they are moved to. This
    /// belongs to the folder that was scanned rather than to the application:
    /// what is safe to delete outright somewhere is not safe everywhere.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn remember_disposal(&self) {
        let Some(db_path) = &self.db_path else {
            return;
        };
        let stored = self.destination.name();
        let result = db::open_for_notes(db_path).and_then(|conn| {
            db::set_meta(&conn, "disposal", stored)?;
            db::set_meta(&conn, "move_dir", &self.move_dir)
        });
        if let Err(err) = result {
            runlog::log_line!("the cleanup choice could not be written: {err:#}");
        }
    }

    /// Take back what this folder's index records: where a cleanup sends what it
    /// removes, and how far down the folder the index reaches.

    /// Move the preview with the cursor keys. Nothing happens at either end.
    fn walk(&mut self, visible: &[usize], direction: Direction) {
        let counts: Vec<usize> =
            visible.iter().map(|index| self.sets[*index].members.len()).collect();
        let Some(at) = self.position(visible) else {
            return;
        };
        let Some(mut landed) = step(&counts, at, direction) else {
            return;
        };
        // A set nobody calls a set of copies is not somewhere to be: the keys
        // step over it to the next set that is one, and stop where they are when
        // there is none.
        let ignored: Vec<bool> =
            visible.iter().map(|index| self.is_ignored(&self.sets[*index])).collect();
        while ignored.get(landed.0).copied().unwrap_or(false) {
            let from = match direction {
                Direction::Forward => (landed.0, counts[landed.0].saturating_sub(1)),
                Direction::Back => (landed.0, 0),
                Direction::NextSet | Direction::PreviousSet => landed,
            };
            let Some(next) = step(&counts, from, direction) else {
                return;
            };
            landed = next;
        }
        let (set, member) = landed;
        self.selected = Some(self.sets[visible[set]].members[member].file_id);
        self.scroll_to = Some(set);
        self.show_selected = true;
    }

    /// Mark or unmark the picture the preview is showing, which is what the space
    /// bar and a double click both do.
    ///
    /// One already marked comes off, and a set can end up keeping nothing. One
    /// that is not marked goes on: on its own while "allow multi-select" is off, and
    /// beside whatever the set already keeps while it is on.
    fn keep_selected(&mut self) {
        let Some(file_id) = self.selected else {
            return;
        };
        let Some(set) =
            self.sets.iter().find(|set| set.members.iter().any(|member| member.file_id == file_id))
        else {
            return;
        };
        // A set nobody calls a set of copies keeps nothing and loses nothing, so
        // there is no mark in it to move. What it kept before it was ignored is
        // left exactly as it was, for the day somebody takes it back.
        if self.is_ignored(set) {
            return;
        }
        let set_id = set.set_id;
        let members: Vec<i64> = set.members.iter().map(|member| member.file_id).collect();

        let keeping = self.keep.get(&set_id);
        let now = if keeps(keeping, file_id) {
            unmarked(keeping, &members, file_id)
        } else if self.multi_select {
            marked(keeping, &members, file_id)
        } else {
            Some(Keep::One(file_id))
        };
        match now {
            Some(keep) => self.keep.insert(set_id, keep),
            None => self.keep.remove(&set_id),
        };
    }

    /// Where the preview is in the list on screen, as a set and a place in it.
    fn position(&self, visible: &[usize]) -> Option<(usize, usize)> {
        let file_id = self.selected?;
        visible.iter().enumerate().find_map(|(set, index)| {
            let member = self.sets[*index]
                .members
                .iter()
                .position(|member| member.file_id == file_id)?;
            Some((set, member))
        })
    }

    /// Open the preview on the keeper of the first set, so the review view starts
    /// on a picture rather than on an invitation to click one.
    /// How many pictures are not marked to keep, and how many bytes they are.
    /// That is what a cleanup would take, so it is what the toolbar counts.
    fn selected_for_removal(&self) -> (usize, i64) {
        let mut count = 0usize;
        let mut bytes = 0i64;
        for set in &self.sets {
            // Nothing goes from a set that is not a set of copies.
            if self.is_ignored(set) {
                continue;
            }
            let keeping = self.keep.get(&set.set_id);
            for member in &set.members {
                if !keeps(keeping, member.file_id) {
                    count += 1;
                    bytes += member.size_bytes;
                }
            }
        }
        (count, bytes)
    }

    /// The first set that is a set of copies, not simply the first set. A set
    /// nobody calls a set of copies keeps nothing and shows nothing as kept, so
    /// opening the review on a picture in one puts the preview somewhere that
    /// means nothing and gives the cursor keys nowhere sensible to start. A
    /// review of nothing but ignored sets opens on nothing.
    fn preselect_first_keeper(&mut self) {
        self.selected = self
            .sets
            .iter()
            .find(|set| !self.is_ignored(set))
            .and_then(|set| match self.keep.get(&set.set_id) {
                Some(Keep::One(file_id)) => Some(*file_id),
                Some(Keep::Several(kept)) => kept.first().copied(),
                _ => set.members.first().map(|member| member.file_id),
            });
        self.showing = None;
    }

    /// Whether there is anything to review, which is what the Review and Clean up
    /// tabs wait for.
    fn have_sets(&self) -> bool {
        !self.sets.is_empty()
    }

    /// Whether a pass of any kind is under way. Starting a second one while the
    /// first is going means nothing, so anything that would start one is off
    /// until it is over, whichever of the three it is.
    fn busy(&self) -> bool {
        self.running.is_some() || self.searching.is_some() || self.removing.is_some()
    }

    /// Stop whichever of the two waits is on: the indexing, the search, or both
    /// if the indexer has just handed over.
    /// The window comes back here, not when the work says it has stopped.
    ///
    /// Waiting for the pass to answer means waiting for whatever call it is
    /// inside, and on a folder that is not on this machine that is however long
    /// the other machine takes. Cancel is pressed by someone who wants the window
    /// back: they get it now, and can pick another folder or change a setting
    /// while the work winds itself up on its own thread and is listened to by
    /// nobody.
    fn cancel_work(&mut self) {
        if let Some(run) = self.running.as_mut() {
            run.cancel();
        }
        // Dropping the run asks the pass to stop and does not wait for it.
        self.running = None;
        if self.searching.is_some() {
            runlog::log_line!("cancelling: stopping the search");
            self.search_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
            self.searching = None;
        }

        // Back to before any of it started. A cancelled pass leaves numbers that
        // are true of nothing: a count of a listing that did not finish, lamps
        // for steps that were half done, an index in memory that describes a
        // folder as it was part way through being read. None of it is worth
        // keeping and all of it would be read as though it were.
        self.scan = ScanState::default();
        self.search = SearchState::default();
        self.lit.clear();
        self.images = None;
        self.sets.clear();
        self.keep.clear();
        self.selected = None;
        self.showing = None;
        self.thumbs.forget();
        self.error = None;
    }

    /// The picture that was clicked, at a size worth looking at. Sits beside the
    /// list rather than over it, so clicking through a set is one click each.
    fn preview_pane(&mut self, ui: &mut egui::Ui, root: &Path) {
        let find = |file_id: i64| {
            self.sets
                .iter()
                .flat_map(|set| set.members.iter().map(move |member| (set.set_id, member)))
                .find(|(_, member)| member.file_id == file_id)
                .map(|(set_id, member)| (set_id, member.clone()))
        };
        let chosen = self.selected.and_then(find);
        let held = self.showing.and_then(find);

        let width = self.preview_width.unwrap_or(ui.available_width() * 0.42);
        let pane = egui::SidePanel::right("preview")
            .resizable(true)
            .default_width(width)
            .min_width(260.0)
            .show_inside(ui, |ui| {
                let Some((set_id, member)) = chosen else {
                    ui.centered_and_justified(|ui| {
                        ui.label(egui::RichText::new("click a picture to see it here").weak());
                    });
                    return;
                };

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let keeping = keeps(self.keep.get(&set_id), member.file_id);
                    if ui
                        .add_enabled(!keeping, egui::Button::new("Keep this one"))
                        .clicked()
                    {
                        self.keep.insert(set_id, Keep::One(member.file_id));
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{}x{}  {}  {:.1} MB",
                            member.width,
                            member.height,
                            member.format,
                            member.size_bytes as f64 / 1_000_000.0
                        ))
                        .weak(),
                    );
                });
                ui.add(unwrapped(egui::RichText::new(&member.rel_path).weak()));
                ui.add_space(4.0);

                // The picture takes the top of the pane and what the file says
                // about itself takes the rest, so there is always a picture and
                // always somewhere for the words to go.
                let room = egui::vec2(ui.available_width(), ui.available_height() * 0.62);
                let wanted =
                    self.thumbs.get(member.file_id, thumbs::LARGE_EDGE, root, &member.rel_path);
                if wanted.is_some() {
                    self.showing = self.selected;
                }
                // The one being read is not on screen yet, so what is on screen
                // stays there. Asking for it again is what keeps it alive.
                let drawing = wanted.or_else(|| {
                    let (_, held) = held?;
                    self.thumbs.get(held.file_id, thumbs::LARGE_EDGE, root, &held.rel_path)
                });

                ui.allocate_ui(room, |ui| {
                    ui.centered_and_justified(|ui| match drawing {
                        Some(texture) => {
                            let shown = ui.add(
                                egui::Image::new(&texture)
                                    .max_size(room)
                                    .maintain_aspect_ratio(true)
                                    .sense(egui::Sense::click()),
                            );
                            // A hand over it, because a picture that does
                            // something when it is clicked has to look like one.
                            if shown.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if shown.clicked() {
                                self.filling_the_window = Some(member.file_id);
                            }
                        }
                        None => {
                            ui.label(egui::RichText::new("reading...").weak());
                        }
                    });
                });
                ui.add_space(6.0);
                self.written_beside_it(ui, &member, root);
            });
        self.preview_width = Some(pane.response.rect.width());
    }

    /// Everything the file says about itself, under the picture: what the camera
    /// was set to, when and where it was taken, and whatever anybody wrote into
    /// it since.
    ///
    /// The file is read on a thread of its own, because a raw file is tens of
    /// megabytes and often on another machine, and the window carries on drawing
    /// while it arrives.
    fn written_beside_it(
        &mut self,
        ui: &mut egui::Ui,
        member: &imgdedupe_core::matching::Member,
        root: &std::path::Path,
    ) {
        let path = root.join(&member.rel_path);
        let groups = self.metadata.get(member.file_id, path, ui.ctx());
        if groups.is_empty() {
            let waiting = self.metadata.reading();
            ui.label(
                egui::RichText::new(if waiting {
                    "reading..."
                } else {
                    "this file says nothing about itself"
                })
                .weak(),
            );
            return;
        }

        let lines: Vec<(Option<String>, String, String)> = groups
            .iter()
            .flat_map(|group| {
                std::iter::once((Some(group.name.clone()), String::new(), String::new())).chain(
                    group
                        .entries
                        .iter()
                        .map(|(name, value)| (None, name.clone(), value.clone())),
                )
            })
            .collect();

        let line = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
        let names = ui.available_width() * 0.38;
        // What is left of the pane, and no more. Without a height of its own the
        // list takes whatever it asks for, runs off the bottom of the window, and
        // the part of it below the edge cannot be reached by anything.
        let room = (ui.max_rect().bottom() - ui.cursor().top() - SCROLL_BAR).max(line * 3.0);
        let bar = egui::Id::new(("what it says", member.file_id));
        // The whole pane, not only the part the list covers: a scroll wheel
        // turned over the picture is a scroll wheel turned over this pane, and
        // the list is the only thing in it that can move.
        let where_it_is = ui.max_rect();
        let (scroll_wheel, pointer) =
            ui.input(|input| (input.smooth_scroll_delta, input.pointer.latest_pos()));
        let (_, offset, viewport) = scrolled(
            ui,
            bar,
            true,
            line * 3.0,
            egui::ScrollArea::vertical()
                .id_salt(("metadata", member.file_id))
                .max_height(room)
                .auto_shrink([false, false]),
            |area, ui| {
                area.show_rows(ui, line, lines.len(), |ui, rows| {
                    for (heading, name, value) in &lines[rows] {
                        match heading {
                            Some(heading) => {
                                // A band the width of the pane, so a heading is
                                // where one part ends and the next begins rather
                                // than another line of text among the lines.
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), line),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    2.0,
                                    ui.visuals().widgets.inactive.weak_bg_fill,
                                );
                                let ink = ui.visuals().strong_text_color();
                                let words = ui.painter().layout_no_wrap(
                                    heading.clone(),
                                    egui::TextStyle::Body.resolve(ui.style()),
                                    ink,
                                );
                                let down = (rect.height() - words.size().y) / 2.0;
                                ui.painter().galley(
                                    rect.left_top() + egui::vec2(6.0, down),
                                    words,
                                    ink,
                                );
                            }
                            None => {
                                ui.horizontal(|ui| {
                                    let room = ui.available_width();
                                    clipped_line_in(
                                        ui,
                                        egui::RichText::new(name).weak(),
                                        names,
                                    );
                                    clipped_line_in(
                                        ui,
                                        egui::RichText::new(value),
                                        room - names,
                                    );
                                });
                            }
                        }
                    }
                })
            },
        );

        // The scroll wheel, applied here rather than left to the toolkit. Inside
        // a panel it does not reach this list on its own, and a list nothing can
        // scroll is a list nobody can read past the first dozen lines of.
        let over = pointer.is_some_and(|at| where_it_is.contains(at));
        if over && scroll_wheel.y != 0.0 {
            let content = lines.len() as f32 * line;
            let wanted = (offset - scroll_wheel.y).clamp(0.0, (content - viewport).max(0.0));
            if wanted != offset {
                ui.data_mut(|data| data.insert_temp(bar, wanted));
                ui.ctx().request_repaint();
            }
        }
    }

    /// The picture, filling the window, over everything else.
    ///
    /// Asked for at the size of the window rather than at the size of the pane
    /// it came from, so it is the picture and not a blown up thumbnail of it. A
    /// click anywhere on it, or the escape key, puts it away.
    fn filling_the_window(&mut self, ctx: &egui::Context) {
        let Some(file_id) = self.filling_the_window else {
            return;
        };
        let Some(root) = self.folder.clone() else {
            self.filling_the_window = None;
            return;
        };
        let Some(member) = self
            .sets
            .iter()
            .flat_map(|set| set.members.iter())
            .find(|member| member.file_id == file_id)
            .cloned()
        else {
            self.filling_the_window = None;
            return;
        };

        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.filling_the_window = None;
            return;
        }

        let screen = ctx.screen_rect();
        let edge = (screen.width().max(screen.height()) * ctx.pixels_per_point()) as u32;
        let picture = self.thumbs.get(file_id, edge, &root, &member.rel_path);

        egui::Area::new(egui::Id::new("filling the window"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                // Everything behind it is covered, so what is on screen is the
                // picture and nothing else.
                ui.painter().rect_filled(screen, 0.0, egui::Color32::from_black_alpha(240));
                let taken = ui.allocate_rect(screen, egui::Sense::click());
                match picture {
                    Some(texture) => {
                        let size = texture.size_vec2();
                        let room = screen.size() * 0.98;
                        let scale = (room.x / size.x).min(room.y / size.y).min(1.0);
                        let shown = egui::Rect::from_center_size(screen.center(), size * scale);
                        egui::Image::new(&texture).paint_at(ui, shown);
                    }
                    None => {
                        ui.painter().text(
                            screen.center(),
                            egui::Align2::CENTER_CENTER,
                            "reading...",
                            egui::TextStyle::Body.resolve(ui.style()),
                            ui.visuals().weak_text_color(),
                        );
                    }
                }
                if taken.clicked() {
                    self.filling_the_window = None;
                }
            });
    }

    /// `width` is what the box around the set comes out as. The list works it
    /// out from its own room, because what is inside a scroll area is not told
    /// where the area ends.
    fn set_row(&mut self, ui: &mut egui::Ui, index: usize, root: &std::path::Path, width: f32) {
        let set_id = self.sets[index].set_id;
        let members = self.sets[index].members.clone();
        let keeping = self.keep.get(&set_id).cloned();

        // Built to a fixed height rather than measured afterwards. The list places
        // the rows it is not drawing by this number, and a row that came out any
        // other height would move the content under a scroll already in progress.
        //
        // The width is the room less what the frame drawn round the row adds to
        // it, and less the margin the window keeps at its edges, so the box ends
        // as far from the list's scroll bar as it begins from the window's edge.
        let size = egui::vec2(width.max(0.0), set_row_height(ui));
        let ignored = self.is_ignored(&self.sets[index]);
        // The row is the box and the space kept under it. The box is the top of
        // the row; the space below it is what separates one set from the next.
        let room = egui::Rect::from_min_size(ui.next_widget_position(), size);
        // The line round the box is drawn half on either side of the rectangle
        // it is given, so that rectangle is set in by the width of the line: the
        // box then ends exactly at the row's edges, and the gap from the window
        // to its left edge is the gap from its right edge to the scroll bar.
        let inside = egui::Rect::from_min_max(
            room.min,
            egui::pos2(room.right(), room.bottom() - BETWEEN_BOXES),
        )
        .shrink(BOX_EDGE);
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(room), |ui| {
        ui.set_min_size(size);
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inside), |ui| {
        egui::Frame::group(ui.style())
            .inner_margin(egui::Margin::ZERO)
            .show(ui, |ui| {
                let gap = ui.spacing().item_spacing.x;
                // The room for the strip's own scroll bar is kept whether or not
                // this set has one, so every box is the height the list places
                // its rows at and one box follows the next with the same space
                // between them.
                ui.set_height(inside.height());
                // The strip and the buttons are both drawn into rectangles of
                // their own, which claim no width for the box around them. Left
                // at that the box ends up as wide as the row of buttons.
                ui.set_min_width(inside.width());

                // The strip has the top of the box, down to where the buttons
                // begin. It runs the whole width of the box, so the bar under it
                // does too, the way the bar beside the list runs the height of
                // the list. The pictures keep the box's padding on either side;
                // that is theirs, not the bar's. Given the whole box the strip
                // would put its bar under the buttons, at the very bottom.
                let strip = egui::Rect::from_min_size(
                    egui::pos2(inside.left(), inside.top() + BOX_PADDING),
                    egui::vec2(inside.width(), tile_strip_height(ui) + SCROLL_BAR),
                );

                // One tile's width is what a click on the strip's scroll bar
                // moves by, and the first tile's is as good a step as any.
                let step = members.first().map_or(TILE.x, tile_width);
                let bar = egui::Id::new(("set bar", set_id));
                let (_, offset, viewport) = ui
                    .allocate_new_ui(egui::UiBuilder::new().max_rect(strip), |ui| {
                        // A set nobody calls a set of copies is barely there:
                        // everything above the buttons, the pictures and every
                        // line of writing under them, at a quarter of its
                        // opacity. The buttons are how it stops being ignored, so
                        // they are not faded with the rest of it.
                        if ignored {
                            ui.set_opacity(IGNORED_OPACITY);
                        }
                        scrolled(
                            ui,
                            bar,
                            false,
                            step,
                            egui::ScrollArea::horizontal().id_salt(("set", set_id)),
                            |area, ui| {
                                // The pictures keep the box's padding on either
                                // side of them, inside a strip that is the whole
                                // width of the box. Their own room, clipped to
                                // it, so a picture scrolled up against the edge
                                // stops there rather than in the padding.
                                let room = ui.max_rect().shrink2(egui::vec2(BOX_PADDING, 0.0));
                                let mut pictures =
                                    ui.new_child(egui::UiBuilder::new().max_rect(room));
                                pictures.set_clip_rect(room.intersect(ui.clip_rect()));
                                area.show(&mut pictures, |ui| {
                                    ui.horizontal_top(|ui| {
                                        for member in &members {
                                            let width = tile_width(member);
                                            self.member_tile(
                                                ui,
                                                member,
                                                keeping.as_ref(),
                                                root,
                                                width,
                                                ignored,
                                            );
                                        }
                                    });
                                })
                            },
                        )
                    })
                    .inner;

                // The cursor keys move the preview along the strip, and the strip
                // follows: as far as it takes to bring the picture on screen and
                // no further, so the set does not jump about under a selection
                // that was already in view.
                let holds_it =
                    members.iter().any(|member| Some(member.file_id) == self.selected);
                if self.show_selected && holds_it {
                    if let Some(wanted) = self.strip_offset(&members, gap, offset, viewport) {
                        ui.data_mut(|data| data.insert_temp(bar, wanted));
                        ui.ctx().request_repaint();
                    }
                    self.show_selected = false;
                }

                // What the whole set can be told to do, in a row along the
                // bottom of it: everything, nothing, or that it is not a set of
                // copies at all. Under the pictures rather than over them,
                // because the pictures are what a set is.
                // A band of its own under the pictures, so the row reads as the
                // foot of the set rather than as three buttons adrift in it. It
                // is the bottom of the box, corner to corner: the box keeps no
                // margin, so there is nothing between the two.
                let band = egui::Rect::from_min_max(
                    egui::pos2(inside.left(), inside.bottom() - button_row_height(ui)),
                    inside.max,
                );
                ui.painter().rect_filled(band, 0.0, BUTTON_ROW_BACKGROUND);
                // A line of its own along the top of the band. Without it the
                // band's edge is the same colour as the line round the box and
                // as the one under the strip's scroll bar, and three edges of the
                // same colour a few points apart read as one thick edge.
                ui.painter().hline(
                    band.x_range(),
                    band.top(),
                    egui::Stroke::new(1.0_f32, BUTTON_ROW_EDGE),
                );

                let mut pressed = None;
                ui.allocate_new_ui(
                    egui::UiBuilder::new()
                        .max_rect(band)
                        .layout(egui::Layout::left_to_right(egui::Align::Center)),
                    |ui| {
                        // The presets under the slider are a row of buttons the
                        // size of their own words, spaced by the style, with the
                        // one that is on drawn as pressed. These are the same,
                        // because they are the same kind of thing.
                        ui.spacing_mut().button_padding = PRESET_PADDING;
                        for (label, what) in [
                            ("keep all", SetAction::KeepAll),
                            ("keep none", SetAction::KeepNone),
                            // The third one undoes itself: a set that has been
                            // ignored is one press away from being a set again.
                            (
                                if ignored { "unignore" } else { "ignore" },
                                if ignored { SetAction::Unignore } else { SetAction::Ignore },
                            ),
                        ] {
                            // An ignored set keeps nothing, so the two about
                            // keeping mean nothing until it is a set again.
                            let usable = !ignored || what == SetAction::Unignore;
                            let button =
                                egui::Button::new(label).wrap_mode(egui::TextWrapMode::Extend);
                            if ui.add_enabled(usable, button).clicked() {
                                pressed = Some(what);
                            }
                        }
                    },
                );
                match pressed {
                    Some(SetAction::KeepAll) => {
                        self.keep.insert(set_id, Keep::All);
                    }
                    // Nothing marked is nothing kept, so the whole set goes.
                    Some(SetAction::KeepNone) => {
                        self.keep.remove(&set_id);
                    }
                    Some(SetAction::Ignore) => self.ignore_set(set_id),
                    Some(SetAction::Unignore) => self.unignore_set(set_id),
                    None => {}
                }
            });
        });
        });
    }

    /// How far along a set's strip has to be for the picture the preview is
    /// showing to be on it, or `None` if it is on it already.
    ///
    /// As little movement as the job takes: a picture off the left brings the
    /// strip back to its left edge, one off the right brings it just far enough
    /// to end at the right edge, and one already on screen moves nothing. The
    /// strip is a row of tiles with a gap between them, so where a tile starts is
    /// what the tiles before it took.
    fn strip_offset(
        &self,
        members: &[imgdedupe_core::matching::Member],
        gap: f32,
        offset: f32,
        viewport: f32,
    ) -> Option<f32> {
        let selected = self.selected?;
        let mut start = 0.0;
        for member in members {
            let width = tile_width(member);
            if member.file_id == selected {
                if start < offset {
                    return Some(start);
                }
                if start + width > offset + viewport {
                    return Some(start + width - viewport);
                }
                return None;
            }
            start += width + gap;
        }
        None
    }

    /// One image in a set: the picture, whether it is the one being kept, and the
    /// two facts that decide it.
    fn member_tile(
        &mut self,
        ui: &mut egui::Ui,
        member: &imgdedupe_core::matching::Member,
        keeping: Option<&Keep>,
        root: &std::path::Path,
        width: f32,
        ignored: bool,
    ) {
        // A set nobody calls a set of copies keeps nothing and shows nothing as
        // kept: no border and no ring. Half showing is the strip's business and
        // is done to the whole of it at once.
        let kept = !ignored && keeps(keeping, member.file_id);
        let showing = !ignored && self.selected == Some(member.file_id);
        let keep_colour = egui::Color32::from_rgb(90, 180, 110);

        let tall = tile_strip_height(ui);
        ui.allocate_ui(egui::vec2(width, tall), |ui| {
            ui.set_height(tall);
            // A set is a strip that scrolls sideways, and the ones off the end of
            // it are not on screen however much of the set is. Asking for them
            // would put a hundred pictures nobody can see in front of the next
            // set's, which is what made the sets below the first one wait.
            let on_screen = ui.is_rect_visible(ui.max_rect());
            ui.vertical(|ui| {
                ui.add_space(TILE_RING);
                // The keeper's border stays on the picture. Being the one on the
                // right is a second thing, drawn as a ring outside it, so a
                // picture that is both shows both.
                let frame = egui::Frame::none()
                    .stroke(if kept {
                        egui::Stroke::new(3.0_f32, keep_colour)
                    } else {
                        egui::Stroke::new(
                            1.0_f32,
                            ui.style().visuals.widgets.noninteractive.bg_stroke.color,
                        )
                    })
                    .inner_margin(TILE_BORDER / 2.0)
                    .outer_margin(egui::Margin::symmetric(TILE_RING, 0.0));

                let framed = frame.show(ui, |ui| {
                    // The same picture whether or not the set is ignored. There
                    // is one of each, read once: a second copy of every picture
                    // would be a second read and a second wait, and ignoring a
                    // set has to show the moment it is clicked.
                    let picture = on_screen
                        .then(|| {
                            self.thumbs.get(
                                member.file_id,
                                thumbs::THUMB_EDGE,
                                root,
                                &member.rel_path,
                            )
                        })
                        .flatten();
                    match picture {
                        // Half showing when the set is ignored, but not by this:
                        // the whole strip is drawn at half its opacity.
                        Some(texture) => ui.add(
                            egui::Image::new(&texture)
                                .fit_to_exact_size(TILE)
                                .sense(egui::Sense::click()),
                        ),
                        // The same space the picture will take, so nothing moves
                        // when it arrives.
                        None => ui.add_sized(
                            fitted(member.width, member.height),
                            egui::Label::new(egui::RichText::new("...").weak())
                                .sense(egui::Sense::click()),
                        ),
                    }
                });
                // A frame's response covers its outer margin as well, and the ring
                // goes around the picture, not around the space kept clear for it.
                let bordered = framed.response.rect.shrink2(egui::vec2(TILE_RING, 0.0));
                if showing {
                    ui.painter().rect_stroke(
                        bordered.expand(3.0),
                        2.0,
                        egui::Stroke::new(3.0_f32, ui.style().visuals.selection.bg_fill),
                    );
                }
                let picked = framed.inner;
                if picked.clicked() {
                    self.selected = Some(member.file_id);
                }
                // Twice on a picture keeps it, which is the space bar on the one
                // being shown, including that it takes the mark off again.
                if picked.double_clicked() {
                    self.selected = Some(member.file_id);
                    self.keep_selected();
                }

                // Centred on the picture this tile is about. The picture sits in
                // the middle of the column, so the column's width is its width.
                // The space is taken either way, so the rows under it line up
                // across a set whether or not anything is marked.
                let over_the_picture = egui::vec2(width, ui.spacing().interact_size.y);
                ui.allocate_ui_with_layout(
                    over_the_picture,
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.set_min_size(over_the_picture);
                        if kept {
                            ui.label(egui::RichText::new("KEEP").strong().color(keep_colour));
                        }
                    },
                );
                // Four lines, each of them one line. A tile is a column as wide
                // as its picture, and a line long enough to wrap in it would
                // make that tile taller than every other one in the strip.
                clipped_line_in(
                    ui,
                    egui::RichText::new(format!("{}x{}", member.width, member.height)),
                    width,
                );
                clipped_line_in(
                    ui,
                    egui::RichText::new(format!(
                        "{}  {:.1} MB",
                        member.format,
                        member.size_bytes as f64 / 1_000_000.0
                    ))
                    .weak(),
                    width,
                );
                clipped_line_in(ui, egui::RichText::new(file_date(member.mtime_ns)).weak(), width);
                clipped_line_in(ui, egui::RichText::new(&member.rel_path).weak(), width);
            });
        });
    }

    fn cleanup_view(&mut self, ui: &mut egui::Ui) {
        let plan = self.build_plan();
        let sets_in_play = self.sets.len();

        // The action sits top right, where the one that starts a scan and the one
        // that goes from the review to here both are.
        egui::TopBottomPanel::top("cleanup actions").show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if let Some(result) = &self.cleanup_result {
                    ui.label(egui::RichText::new(result).strong());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // While it runs, the button is the bar: there is nothing to
                    // press any more, and something has to say the files are going.
                    if self.removing.is_some() {
                        let done = self.removed_so_far;
                        let total = self.to_remove.max(1);
                        let mut bar =
                            egui::ProgressBar::new(done as f32 / total as f32).desired_width(210.0);
                        if !self.tidying {
                            bar = bar.text(format!(
                                "{} {done} of {total}",
                                match self.destination {
                                    Destination::MoveTo => "moving",
                                    _ => "removing",
                                }
                            ));
                        }
                        let painted = ui.add(bar);
                        // The bar's own text sits at its left edge. This one is a
                        // sentence rather than a count, so it goes in the middle.
                        if self.tidying {
                            ui.painter().text(
                                painted.rect.center(),
                                egui::Align2::CENTER_CENTER,
                                "tidying the index",
                                egui::TextStyle::Button.resolve(ui.style()),
                                ui.visuals().strong_text_color(),
                            );
                        }
                        return;
                    }
                    let ready = plan.files() > 0 && self.folder.is_some() && !self.busy();
                    let danger = self.destination == Destination::Delete;
                    let button = egui::Button::new(
                        egui::RichText::new(format!(
                            "{} {} files",
                            self.destination.verb(),
                            plan.files()
                        ))
                        .strong()
                        .color(egui::Color32::WHITE),
                    )
                    .fill(if danger {
                        egui::Color32::from_rgb(150, 50, 50)
                    } else {
                        egui::Color32::from_rgb(60, 110, 180)
                    })
                    .min_size(egui::vec2(210.0, 28.0));
                    if ui.add_enabled(ready, button).clicked() {
                        runlog::log_line!("the remove button was pressed");
                        self.run_cleanup(&plan);
                    }
                });
            });
            ui.add_space(4.0);
        });

        egui::SidePanel::left("cleanup settings")
            .resizable(false)
            .exact_width(320.0)
            .show_inside(ui, |ui| {
                ui.add_space(4.0);
                section(ui, "What will happen", |ui| {
                    egui::Grid::new("cleanup summary")
                        .num_columns(2)
                        .spacing([16.0, 4.0])
                        .show(ui, |ui| {
                            ui.label("Sets");
                            ui.label(egui::RichText::new(sets_in_play.to_string()).strong());
                            ui.end_row();
                            ui.label(match self.destination {
                                Destination::MoveTo => "Files moved",
                                _ => "Files removed",
                            });
                            ui.label(egui::RichText::new(plan.files().to_string()).strong());
                            ui.end_row();
                            ui.label("Space freed");
                            ui.label(
                                egui::RichText::new(format!(
                                    "{:.1} MB",
                                    plan.bytes() as f64 / 1_000_000.0
                                ))
                                .strong(),
                            );
                            ui.end_row();
                        });
                });

                ui.add_space(SECTION_GAP);
                let busy = self.busy();
                section(ui, "Where they go", |ui| {
                    for choice in [Destination::Trash, Destination::MoveTo, Destination::Delete] {
                        let picked = self.destination == choice;
                        if ui
                            .add_enabled(!busy, egui::RadioButton::new(picked, choice.label()))
                            .clicked()
                        {
                            self.destination = choice;
                            self.remember_disposal();
                        }
                    }
                    ui.add_space(4.0);
                    let note = egui::RichText::new(self.destination.note());
                    if self.destination == Destination::Delete {
                        ui.label(note.color(egui::Color32::from_rgb(200, 80, 80)));
                    } else {
                        ui.label(note.weak());
                    }

                    if self.destination == Destination::MoveTo {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut self.move_dir)
                                    .hint_text("folder")
                                    .desired_width(190.0),
                            );
                            if ui.add_enabled(!busy, egui::Button::new("choose")).clicked() {
                                if let Some(folder) = crate::folder_picker::pick(None) {
                                    self.move_dir = folder.display().to_string();
                                    self.remember_disposal();
                                }
                            }
                        });
                    }
                });

            });

        egui::CentralPanel::default().show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(match self.destination {
                    Destination::MoveTo => "Files that will be moved",
                    _ => "Files that will be removed",
                })
                .strong(),
            );
            ui.add_space(4.0);
            // What a cleanup could not remove is still in this list, because the
            // files are still there. Red, with what the system said on hover.
            let failed: std::collections::HashMap<&str, &str> = self
                .cleanup_failures
                .iter()
                .map(|(path, why)| (path.as_str(), why.as_str()))
                .collect();
            let line = ui.text_style_height(&egui::TextStyle::Body);
            scrolled(
                ui,
                egui::Id::new("cleanup list"),
                true,
                line,
                egui::ScrollArea::vertical().auto_shrink([false, false]),
                |area, ui| {
                    area.show_rows(ui, line, plan.removals.len(), |ui, range| {
                        for index in range {
                            let path = &plan.removals[index].rel_path;
                            match failed.get(path.as_str()) {
                                Some(why) => {
                                    // The reason goes on the line. There is
                                    // nowhere else to read it.
                                    ui.label(
                                        egui::RichText::new(format!("{path}  {why}"))
                                            .color(egui::Color32::from_rgb(200, 80, 80)),
                                    );
                                }
                                None => {
                                    ui.label(path);
                                }
                            }
                        }
                    })
                },
            );
        });
    }

    /// The destination and its folder, as the core layer wants it.
    fn disposal(&self) -> Disposal {
        match self.destination {
            Destination::Trash => Disposal::Trash,
            Destination::Delete => Disposal::Delete,
            Destination::MoveTo => Disposal::MoveTo(PathBuf::from(&self.move_dir)),
        }
    }

    /// What the keep marks imply. Rebuilt every frame so the count on the button
    /// is always the count of what the button does. What is marked is kept and
    /// everything else goes, including all of a set that is keeping nothing.
    fn build_plan(&self) -> Plan {
        let mut sets: Vec<Vec<imgdedupe_core::matching::Member>> = Vec::new();
        for set in &self.sets {
            // A set nobody calls a set of copies is a set nothing happens to:
            // not kept, not removed, not counted.
            if self.is_ignored(set) {
                continue;
            }
            let keeping = self.keep.get(&set.set_id);
            let members = set
                .members
                .iter()
                .map(|member| {
                    let mut member = member.clone();
                    member.auto_keep = keeps(keeping, member.file_id);
                    member
                })
                .collect();
            sets.push(members);
        }
        cleanup::plan_from_sets(sets.iter().map(|members| members.as_slice()))
    }

    /// Start removing. It runs on its own thread and says how far it has got,
    /// because deleting thousands of files takes long enough that a window doing
    /// it silently cannot be told from one that has hung.
    fn run_cleanup(&mut self, plan: &Plan) {
        let Some(root) = self.folder.clone() else {
            runlog::log_line!("cleanup asked for with no folder open");
            return;
        };
        if self.removing.is_some() {
            runlog::log_line!("cleanup asked for while one is already running");
            return;
        }
        let plan = plan.clone();
        let disposal = self.disposal();
        let total = plan.files();
        self.cleanup_failures.clear();
        runlog::log_line!(
            "cleanup starting: {total} files, {:.1} MB, to {:?}, under {}",
            plan.bytes() as f64 / 1_000_000.0,
            disposal,
            root.display()
        );
        #[cfg(feature = "logging")]
        for removal in plan.removals.iter().take(5) {
            runlog::log_line!("  removing {}", removal.rel_path);
        }
        #[cfg(feature = "logging")]
        if total > 5 {
            runlog::log_line!("  and {} more", total - 5);
        }

        let (send, receive) = std::sync::mpsc::channel::<Removal>();
        let steps = send.clone();
        let db_path = self.db_path.clone();
        let keep_index = self.keep_index;
        std::thread::spawn(move || {
            let result = cleanup::apply_reporting(&root, &plan, &disposal, &|done| {
                let _ = steps.send(Removal::Progress(done));
            });
            let _ = send.send(match result {
                Ok(outcome) => {
                    // Here rather than in the frame that takes the outcome: this
                    // rebuilds the index file, which on a large folder is seconds
                    // of copying, and the window would not paint for that long.
                    let _ = steps.send(Removal::Tidying);
                    let forgotten = if keep_index {
                        forget_rows(db_path.as_deref(), &outcome.removed)
                    } else {
                        discard_index(db_path.as_deref())
                    };
                    Removal::Done(Box::new(outcome), forgotten)
                }
                Err(err) => Removal::Failed(format!("{err:#}")),
            });
        });
        self.removing = Some(receive);
        self.tidying = false;
        self.removed_so_far = 0;
        self.to_remove = total;
    }

    /// Take what the removal has sent. Called once per frame.
    fn pump_cleanup(&mut self, ctx: &egui::Context) {
        let Some(receive) = &self.removing else {
            return;
        };
        ctx.request_repaint_after(std::time::Duration::from_millis(100));
        let waiting: Vec<Removal> = receive.try_iter().collect();
        let mut over = false;
        for step in waiting {
            match step {
                Removal::Progress(done) => self.removed_so_far = done,
                Removal::Tidying => self.tidying = true,
                Removal::Done(outcome, forgotten) => {
                    self.finish_cleanup(&outcome, forgotten);
                    over = true;
                }
                Removal::Failed(message) => {
                    self.fail(&message);
                    over = true;
                }
            }
        }
        if over {
            self.removing = None;
            self.tidying = false;
        }
    }

    fn finish_cleanup(&mut self, outcome: &cleanup::Outcome, forgotten: usize) {
        // Every file that would not go, by name and by reason. A cleanup that
        // quietly removes nothing is what this is here to explain.
        #[cfg(feature = "logging")]
        for (path, message) in &outcome.failed {
            runlog::log_line!("  could not remove {path}: {message}");
        }
        let index = if self.keep_index {
            format!("{forgotten} dropped from the index")
        } else {
            String::from("the index was deleted")
        };
        runlog::log_line!(
            "cleanup finished: {} removed, {} failed, {:.1} MB, {index}",
            outcome.removed.len(),
            outcome.failed.len(),
            outcome.bytes_freed as f64 / 1_000_000.0
        );
        self.cleanup_result = Some(format!(
            "removed {} files, freed {:.1} MB, {} failed, {index}",
            outcome.removed.len(),
            outcome.bytes_freed as f64 / 1_000_000.0,
            outcome.failed.len()
        ));
        self.cleanup_failures = outcome.failed.clone();

        // What went is out of the sets. What would not go stays, with its keeper,
        // so another destination can be chosen and the same files tried again.
        self.forget_members(&outcome.removed);

        if outcome.failed.is_empty() {
            self.sets.clear();
            self.keep.clear();
            self.selected = None;
            self.showing = None;
            self.view = View::Scan;
            // The index is gone with it, so nothing on screen describes anything
            // that still exists. The folder stays chosen and Scan builds it again.
            if !self.keep_index {
                self.thumbs.forget();
                self.scan = ScanState::default();
            }
            self.scan.finished = Some(String::from("cleanup done."));
        }
    }

    /// Take the files that are gone out of the sets on screen. A set with one
    /// picture left is not a duplicate set any more.
    fn forget_members(&mut self, removed: &[String]) {
        if removed.is_empty() {
            return;
        }
        let gone: std::collections::HashSet<&str> =
            removed.iter().map(String::as_str).collect();
        for set in &mut self.sets {
            set.members.retain(|member| !gone.contains(member.rel_path.as_str()));
        }
        self.sets.retain(|set| set.members.len() > 1);

        let left: std::collections::HashSet<i64> =
            self.sets.iter().map(|set| set.set_id).collect();
        self.keep.retain(|set_id, _| left.contains(set_id));

        let still_here: std::collections::HashSet<i64> =
            self.sets.iter().flat_map(|set| set.members.iter().map(|m| m.file_id)).collect();
        self.selected = self.selected.filter(|id| still_here.contains(id));
        self.showing = self.showing.filter(|id| still_here.contains(id));
    }

}

/// Take the index away entirely, for a folder nobody asked to keep.
///
/// Nothing is going to read it again: the next run opens on no folder, and a
/// scan of this one builds it from nothing. Deleting the file is what dropping
/// the rows and rebuilding around them was for, without the copy.
#[cfg_attr(not(feature = "logging"), allow(unused_variables, unused_assignments))]
fn discard_index(db_path: Option<&Path>) -> usize {
    let Some(db_path) = db_path else {
        return 0;
    };
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let mut gone = 0;
    for path in [
        db_path.to_path_buf(),
        with_suffix(db_path, "-wal"),
        with_suffix(db_path, "-shm"),
    ] {
        match std::fs::remove_file(&path) {
            Ok(()) => gone += 1,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => runlog::log_line!("{} would not go: {err}", path.display()),
        }
    }
    runlog::log_line!(
        "delete index: {:.2}s, {gone} files, {}",
        at.elapsed().as_secs_f64(),
        db_path.display()
    );
    0
}

/// The index's write-ahead log and shared memory file sit beside it under the
/// same name with a suffix.
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = path.as_os_str().to_os_string();
    name.push(suffix);
    PathBuf::from(name)
}

/// Take the files that were removed out of the index. Deleted or moved, they are
/// not at those paths any more, and an index that still lists them offers
/// duplicates of files that are gone.
///
/// This runs on the removal's own thread. It rewrites the index file, which on a
/// folder of thousands is seconds of work.
#[cfg_attr(not(feature = "logging"), allow(unused_variables))]
fn forget_rows(db_path: Option<&Path>, removed: &[String]) -> usize {
    let Some(db_path) = db_path else {
        runlog::log_line!("nothing was dropped from the index: no index is open");
        return 0;
    };
    if removed.is_empty() {
        return 0;
    }
    let dropped = db::open_for_notes(db_path).and_then(|mut conn| {
        #[cfg(feature = "logging")]
        let at = std::time::Instant::now();
        let tx = conn.transaction()?;
        let dropped = db::delete_paths(&tx, removed)?;
        tx.commit()?;
        drop(conn);
        runlog::log_line!("drop removed: {:.2}s, {dropped} rows", at.elapsed().as_secs_f64());

        // Rebuilding costs a copy of the whole index, so it happens here and only
        // here: a cleanup is the one thing that leaves enough behind to be worth
        // it, and only when it actually dropped rows.
        if dropped > 0 {
            #[cfg(feature = "logging")]
            let at = std::time::Instant::now();
            db::compact(db_path)?;
            runlog::log_line!("rebuild index: {:.2}s", at.elapsed().as_secs_f64());
        }
        Ok(dropped)
    });
    match dropped {
        Ok(count) => count,
        Err(err) => {
            runlog::log_line!("the index still lists the removed files: {err:#}");
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imgdedupe_core::matching::Member;

    /// A context set up the way the window sets one up: the face it carries and
    /// the style it installs. A bare context has no font at all, so text measures
    /// as nothing and every layout a test looks at is a different layout from the
    /// one on screen.
    fn window() -> egui::Context {
        let ctx = egui::Context::default();
        crate::fonts::install(&ctx);
        install_style(&ctx);
        ctx
    }

    fn member(id: i64, path: &str, size: i64) -> Member {
        Member {
            file_id: id,
            rel_path: path.to_string(),
            width: 100,
            height: 100,
            format: "jpeg".to_string(),
            channels: 3,
            size_bytes: size,
            mtime_ns: 1_700_000_000_000_000_000,
            auto_keep: false,
        }
    }

    /// The date on a tile is the file's own timestamp, turned into a date without
    /// a calendar library, so the arithmetic is what gets checked.
    #[test]
    fn a_file_stamp_becomes_the_date_and_time_it_stands_for() {
        assert_eq!(file_date(0), "1970-01-01 00:00");
        assert_eq!(file_date(1_000_000_000), "1970-01-01 00:00");
        assert_eq!(file_date(86_399 * 1_000_000_000), "1970-01-01 23:59");
        assert_eq!(file_date(86_400 * 1_000_000_000), "1970-01-02 00:00");

        // 2024-02-29, a leap day in a year that is a multiple of four.
        assert_eq!(file_date(1_709_164_800 * 1_000_000_000), "2024-02-29 00:00");
        // 2000-02-29: a multiple of a hundred that is still a leap year.
        assert_eq!(file_date(951_782_400 * 1_000_000_000), "2000-02-29 00:00");
        // 1900 was not one, being a multiple of a hundred but not four hundred,
        // so the day after the 28th of February is the first of March.
        assert_eq!(file_date(-2_203_977_600 * 1_000_000_000), "1900-02-28 00:00");
        assert_eq!(file_date(-2_203_891_200 * 1_000_000_000), "1900-03-01 00:00");

        assert_eq!(file_date(1_700_000_000 * 1_000_000_000), "2023-11-14 22:13");
        assert_eq!(file_date(2_000_000_000 * 1_000_000_000), "2033-05-18 03:33");
    }

    /// The space bar keeps whatever the preview is showing, in whichever set it
    /// belongs to.
    #[test]
    fn the_space_bar_keeps_the_picture_the_preview_is_showing() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        assert_eq!(app.sets.len(), 2, "the two pairs were not found");

        let (first, second) = (app.sets[0].set_id, app.sets[1].set_id);
        let opened = match app.keep.get(&first) {
            Some(Keep::One(file_id)) => *file_id,
            other => panic!("the search gave the first set no keeper: {other:?}"),
        };
        let other_in_first = app.sets[0]
            .members
            .iter()
            .map(|member| member.file_id)
            .find(|file_id| *file_id != opened)
            .expect("the set holds two pictures");
        let one_in_second = app.sets[1].members[1].file_id;

        app.selected = Some(other_in_first);
        app.keep_selected();
        assert_eq!(app.keep.get(&first), Some(&Keep::One(other_in_first)), "the keeper did not move");

        app.selected = Some(one_in_second);
        app.keep_selected();
        assert_eq!(
            app.keep.get(&second),
            Some(&Keep::One(one_in_second)),
            "the other set was not given the keeper it was asked for"
        );
        assert_eq!(
            app.keep.get(&first),
            Some(&Keep::One(other_in_first)),
            "it changed a set it was not on"
        );

        app.selected = None;
        app.keep_selected();
        assert_eq!(app.keep.len(), 2, "nothing was selected and something changed");

        // Again on the one already kept takes the mark off, and again puts it
        // back, so the key is a toggle rather than a one way door.
        app.selected = Some(other_in_first);
        app.keep_selected();
        assert_eq!(app.keep.get(&first), None, "the mark did not come off");
        assert_eq!(
            app.keep.get(&second),
            Some(&Keep::One(one_in_second)),
            "it changed a set it was not on"
        );
        app.keep_selected();
        assert_eq!(
            app.keep.get(&first),
            Some(&Keep::One(other_in_first)),
            "the mark did not go back on"
        );
    }

    /// Starting a second pass while one is going means nothing, so everything
    /// that would start one is off while any of the three is running.
    #[test]
    fn nothing_that_starts_work_is_offered_while_work_is_going() {
        let scanned = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        assert!(!app.busy(), "a window that has done nothing is busy");

        app.open_folder(scanned.path().to_path_buf());
        app.start_scan();
        assert!(app.busy(), "a pass over the folder does not count as busy");
        settle(&mut app);
        assert!(!app.busy(), "the window is still busy with a pass that finished");

        app.load_sets();
        assert!(app.busy(), "the search for duplicates does not count as busy");
        settle(&mut app);

        app.destination = Destination::Delete;
        let plan = app.build_plan();
        app.run_cleanup(&plan);
        assert!(app.busy(), "removing files does not count as busy");
        let ctx = window();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.removing.is_some() && std::time::Instant::now() < until {
            app.pump_cleanup(&ctx);
        }
        assert!(!app.busy(), "the window is still busy with a cleanup that finished");
    }

    /// A cleanup where nothing could be removed leaves everything as it was, on
    /// the page where another destination can be chosen, and says which files.
    #[test]
    fn a_cleanup_that_removed_nothing_stays_put_and_names_the_files() {
        let scanned = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(scanned.path().to_path_buf());
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        let keeping = app.keep.clone();

        // The file the plan is about is taken away before the cleanup runs, so
        // the removal fails the way a locked or missing file does.
        app.destination = Destination::Delete;
        let plan = app.build_plan();
        let going = plan.removals[0].rel_path.clone();
        std::fs::remove_file(scanned.path().join(&going)).expect("take the file away");

        app.view = View::Cleanup;
        app.run_cleanup(&plan);
        let ctx = window();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.removing.is_some() && std::time::Instant::now() < until {
            app.pump_cleanup(&ctx);
        }

        assert_eq!(app.view, View::Cleanup, "it left the page with the destinations on it");
        assert_eq!(app.sets.len(), 1, "the sets went even though the files did not");
        assert_eq!(app.keep, keeping, "the keeper was thrown away");
        assert_eq!(app.cleanup_failures.len(), 1);
        assert_eq!(app.cleanup_failures[0].0, going);
    }

    /// A cleanup that removed everything is over: the sets are gone and so is the
    /// page for them.
    #[test]
    fn a_cleanup_that_removed_everything_leaves_the_page() {
        let scanned = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(scanned.path().to_path_buf());
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        assert_eq!(app.sets.len(), 1, "the two copies were not found");

        // Delete outright, so the test does not depend on a recycle bin.
        app.destination = Destination::Delete;
        let plan = app.build_plan();
        assert_eq!(plan.files(), 1, "the plan is not the copy the set does not keep");
        let going = plan.removals[0].rel_path.clone();
        app.view = View::Cleanup;
        app.run_cleanup(&plan);
        let ctx = window();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.removing.is_some() && std::time::Instant::now() < until {
            app.pump_cleanup(&ctx);
        }

        assert!(!scanned.path().join(&going).exists(), "{going} is still on disk");
        assert_eq!(app.view, View::Scan);
        assert!(app.sets.is_empty());
        assert!(app.cleanup_failures.is_empty());
    }

    /// Some went and some did not. What went comes out of the sets, what did not
    /// stays on screen to be tried another way.
    #[test]
    fn a_cleanup_that_half_worked_keeps_what_is_still_there() {
        let dir = tempfile::tempdir().expect("tempdir");
        let picture = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
        }));
        for name in ["one.png", "two.png", "three.png"] {
            picture
                .save_with_format(dir.path().join(name), image::ImageFormat::Png)
                .expect("a fixture");
        }

        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(dir.path().to_path_buf());
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        assert_eq!(app.sets[0].members.len(), 3, "the three copies were not one set");

        // One of the two the plan would remove is taken away first, so that one
        // fails and the other goes.
        app.destination = Destination::Delete;
        let plan = app.build_plan();
        assert_eq!(plan.files(), 2);
        let fails = plan.removals[0].rel_path.clone();
        let goes = plan.removals[1].rel_path.clone();
        std::fs::remove_file(dir.path().join(&fails)).expect("take the file away");

        app.view = View::Cleanup;
        app.run_cleanup(&plan);
        let ctx = window();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.removing.is_some() && std::time::Instant::now() < until {
            app.pump_cleanup(&ctx);
        }

        assert_eq!(app.view, View::Cleanup);
        assert_eq!(app.sets.len(), 1, "the set lost more than the file that went");
        let left: Vec<&str> =
            app.sets[0].members.iter().map(|member| member.rel_path.as_str()).collect();
        assert!(!left.contains(&goes.as_str()), "the file that went is still listed");
        assert!(left.contains(&fails.as_str()), "the file that would not go was dropped");
        assert_eq!(app.cleanup_failures.len(), 1);
    }

    /// How the last pass ended is about that pass. Starting another clears it, or
    /// a new search sits under the word "cancelled" from the one before it.
    #[test]
    fn starting_a_pass_clears_what_the_last_one_ended_with() {
        let scanned = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(scanned.path().to_path_buf());

        // A finished pass and search leave their outcome on screen.
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        app.scan.finished = Some(String::from("cancelled"));

        app.load_sets();
        assert_eq!(app.scan.finished, None, "the search kept the last outcome on screen");
        assert_eq!(app.error, None, "the search kept the last error on screen");
        settle(&mut app);

        app.start_scan();
        assert_eq!(app.scan.finished, None, "the scan kept the last outcome on screen");
        settle(&mut app);
    }

    /// A search that finds nothing leaves the window where it is. There is
    /// nothing to review, and the Review tab stays shut.
    #[test]
    fn finding_no_duplicates_does_not_open_the_review() {
        let dir = tempfile::tempdir().expect("tempdir");
        let picture = |seed: u32| {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
                image::Rgb([((x * 7 + seed) % 256) as u8, ((y * 11 + seed) % 256) as u8, 20])
            }))
        };
        for (name, seed) in [("a.png", 0), ("b.png", 128)] {
            picture(seed)
                .save_with_format(dir.path().join(name), image::ImageFormat::Png)
                .expect("a fixture");
        }

        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(dir.path().to_path_buf());
        app.sensitivity = 0.5;
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);

        assert_eq!(app.view, View::Scan, "it went to the review with nothing in it");
        assert!(!app.have_sets(), "the tabs would be open on an empty list");
        assert_eq!(app.selected, None);
        assert_eq!(
            app.scan.finished.as_deref(),
            Some("No duplicates found for current settings")
        );
    }

    /// The preview pane opens on the keeper of the first set, so the review view
    /// starts on a picture rather than on an empty pane.
    #[test]
    fn the_first_sets_keeper_is_what_the_preview_starts_on() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        assert_eq!(app.sets.len(), 2, "the two pairs were not found");

        let keeper = match app.keep.get(&app.sets[0].set_id) {
            Some(Keep::One(file_id)) => *file_id,
            other => panic!("the first set was not given a keeper: {other:?}"),
        };
        assert_eq!(app.selected, Some(keeper), "the preview did not open on the keeper");

        // With no keeper there is still a picture to show: the first one.
        let first = app.sets[0].members[0].file_id;
        app.keep.remove(&app.sets[0].set_id);
        app.preselect_first_keeper();
        assert_eq!(app.selected, Some(first), "a first set with no keeper showed nothing");

        app.sets.clear();
        app.preselect_first_keeper();
        assert_eq!(app.selected, None);
    }

    /// The review opens on the first set that is a set of copies. A set nobody
    /// calls a set of copies is not somewhere to start: it keeps nothing, shows
    /// nothing as kept, and the cursor keys would be starting from a set they are
    /// only going to step out of.
    #[test]
    fn the_preview_does_not_open_inside_an_ignored_set() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![
            DuplicateSet {
                set_id: 1,
                members: vec![member(1, "a.jpg", 10), member(2, "b.jpg", 10)],
            },
            DuplicateSet {
                set_id: 2,
                members: vec![member(3, "c.jpg", 10), member(4, "d.jpg", 10)],
            },
        ];
        // The first set is not a set of copies.
        app.ignored.insert(db::pair(1, 2));

        app.preselect_first_keeper();
        assert_eq!(app.selected, Some(3), "the review opened inside the ignored set");

        // What the second set keeps, when it keeps something.
        app.keep.insert(2, Keep::One(4));
        app.preselect_first_keeper();
        assert_eq!(app.selected, Some(4), "the review did not open on what the set keeps");

        // Nothing but ignored sets is nowhere to open.
        app.ignored.insert(db::pair(3, 4));
        app.preselect_first_keeper();
        assert_eq!(app.selected, None, "the review opened inside an ignored set anyway");
    }

    #[test]
    fn right_and_left_run_through_the_whole_list_and_stop_at_its_ends() {
        let counts = [3, 1, 2];

        assert_eq!(step(&counts, (0, 0), Direction::Forward), Some((0, 1)));
        assert_eq!(step(&counts, (0, 2), Direction::Forward), Some((1, 0)));
        assert_eq!(step(&counts, (1, 0), Direction::Forward), Some((2, 0)));
        assert_eq!(step(&counts, (2, 1), Direction::Forward), None);

        assert_eq!(step(&counts, (2, 1), Direction::Back), Some((2, 0)));
        assert_eq!(step(&counts, (2, 0), Direction::Back), Some((1, 0)));
        assert_eq!(step(&counts, (1, 0), Direction::Back), Some((0, 2)));
        assert_eq!(step(&counts, (0, 0), Direction::Back), None);
    }

    #[test]
    fn up_and_down_move_a_set_at_a_time_and_keep_the_place_in_it() {
        let counts = [4, 2, 5];

        assert_eq!(step(&counts, (0, 1), Direction::NextSet), Some((1, 1)));
        assert_eq!(step(&counts, (1, 1), Direction::PreviousSet), Some((0, 1)));

        // The set arrived at is shorter, so it lands on the last picture in it.
        assert_eq!(step(&counts, (0, 3), Direction::NextSet), Some((1, 1)));
        assert_eq!(step(&counts, (2, 4), Direction::PreviousSet), Some((1, 1)));

        assert_eq!(step(&counts, (2, 0), Direction::NextSet), None);
        assert_eq!(step(&counts, (0, 0), Direction::PreviousSet), None);
    }

    #[test]
    fn a_list_with_nothing_in_it_moves_nowhere() {
        for direction in [
            Direction::Forward,
            Direction::Back,
            Direction::NextSet,
            Direction::PreviousSet,
        ] {
            assert_eq!(step(&[], (0, 0), direction), None, "on {direction:?}");
            assert_eq!(step(&[1], (0, 0), direction), None, "on {direction:?}");
        }
    }

    /// A cursor key moves the preview from where it is now, which is a file id,
    /// not a place in the list.
    #[test]
    fn walking_moves_the_preview_to_the_next_picture() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        assert_eq!(app.sets.len(), 2, "the two pairs were not found");
        let ids: Vec<Vec<i64>> = app
            .sets
            .iter()
            .map(|set| set.members.iter().map(|member| member.file_id).collect())
            .collect();
        let visible = vec![0, 1];

        app.selected = Some(ids[0][0]);
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, Some(ids[0][1]));

        app.walk(&visible, Direction::Forward);
        assert_eq!(
            app.selected,
            Some(ids[1][0]),
            "the end of a set did not cross into the next"
        );

        app.walk(&visible, Direction::Forward);
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, Some(ids[1][1]), "the end of the list moved somewhere");

        app.walk(&visible, Direction::PreviousSet);
        assert_eq!(app.selected, Some(ids[0][1]));

        app.selected = None;
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, None, "nothing was selected and something moved");
    }

    /// With the checkbox unticked, the index is deleted once the cleanup is over,
    /// along with its write-ahead log. The next run opens the folder with no
    /// index and a scan builds it again.
    #[test]
    fn an_unticked_folder_loses_its_index_when_the_cleanup_is_done() {
        let scanned = folder_with_a_duplicate();
        let db_path = headless::default_db_path(scanned.path());
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(scanned.path().to_path_buf());
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        assert!(db_path.is_file(), "the pass wrote no index");
        assert!(!app.keep_index, "a fresh folder is not remembered");

        app.destination = Destination::Delete;
        let plan = app.build_plan();
        app.view = View::Cleanup;
        app.run_cleanup(&plan);
        let ctx = window();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.removing.is_some() && std::time::Instant::now() < until {
            app.pump_cleanup(&ctx);
        }

        assert!(!db_path.exists(), "the index was left behind");
        assert!(!with_suffix(&db_path, "-wal").exists());
        assert!(!with_suffix(&db_path, "-shm").exists());

        // And the window is back on the scan with nothing on it, so the only way
        // on is another folder or another scan.
        assert_eq!(app.view, View::Scan);
        assert!(app.sets.is_empty());
        assert_eq!(app.scan.total, 0, "the last scan's numbers are still on screen");
    }

    /// Dropping the removed files rewrites the index, which is seconds of work on
    /// a large folder. It happens on the removal's thread, and the frame that
    /// takes the outcome is handed the count rather than working it out, or the
    /// window stops painting at exactly the moment the cleanup looks finished.
    #[test]
    fn taking_the_outcome_does_not_touch_the_index() {
        let scanned = folder_with_a_duplicate();
        let db_path = headless::default_db_path(scanned.path());
        let mut app = reviewing(scanned.path());
        // Kept, so the cleanup drops the rows rather than deleting the index.
        app.keep_index = true;
        app.destination = Destination::Delete;
        let plan = app.build_plan();
        let going = plan.removals[0].rel_path.clone();

        app.view = View::Cleanup;
        app.run_cleanup(&plan);
        let ctx = window();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while app.removing.is_some() && std::time::Instant::now() < until {
            app.pump_cleanup(&ctx);
        }

        let conn = db::open_read_only(&db_path).expect("the index");
        let left: i64 = conn
            .query_row("SELECT count(*) FROM files WHERE rel_path = ?1", [&going], |row| {
                row.get(0)
            })
            .expect("count");
        assert_eq!(left, 0, "the removed file is still in the index");

        let said = app.cleanup_result.clone().expect("a result");
        assert!(said.contains("1 dropped from the index"), "{said}");
    }

    /// Files that were not pictures, and files that could not be read, are not
    /// pictures this pass found. A folder whose only unindexed files are of those
    /// two kinds has found nothing, however many times it reads them.
    #[test]
    fn what_was_skipped_and_what_broke_are_not_counted_as_found() {
        // A folder of pictures, one of which is not a picture at all, read twice:
        // the first pass finds them, the second finds nothing new.
        let dir = tempfile::tempdir().expect("tempdir");
        for name in ["a.png", "b.png"] {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(48, 32, |x, y| {
                image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
            }))
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
        }
        std::fs::write(dir.path().join("notes.txt"), b"not a picture").expect("a fixture");
        std::fs::write(dir.path().join("broken.png"), b"\x89PNG\r\n\x1a\ncut").expect("a fixture");

        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(dir.path().to_path_buf());
        app.start_scan();
        settle(&mut app);

        assert_eq!(app.scan.done, 4, "the pass did not look at every file");
        assert_eq!(app.scan.ignored, 1, "the file that is not a picture was not counted");
        assert_eq!(app.scan.failures.len(), 1, "the broken file is its own number");
        assert_eq!(app.scan.found(), 2, "found is the pictures, not the files");

        app.start_scan();
        settle(&mut app);
        assert_eq!(app.scan.unchanged, 2, "the two pictures were read again");
        assert_eq!(app.scan.found(), 0, "a pass with nothing new found something");
    }

    /// A set's tiles are as wide as that set's own widest picture. Nothing about
    /// another set reaches into this one.
    #[test]
    fn a_set_of_portraits_is_not_given_the_width_of_a_landscape() {
        let portrait = fitted(900, 1200);
        let landscape = fitted(1600, 900);
        assert!(portrait.x < landscape.x, "the two shapes fitted to the same width");
        assert!(portrait.y <= TILE.y && landscape.y <= TILE.y, "a picture came out too tall");
        assert!(portrait.x <= TILE.x && landscape.x <= TILE.x, "a picture came out too wide");

        let of = |width: u32, height: u32| {
            let mut member = member(1, "a.jpg", 100);
            member.width = width;
            member.height = height;
            member
        };
        // The column is the picture plus room for the border and the ring drawn
        // around it, or the neighbouring tile clips them.
        let around = TILE_BORDER + TILE_RING * 2.0;
        assert!(around >= 12.0, "there is not enough room around a picture for its ring");
        assert_eq!(tile_width(&of(900, 1200)), portrait.x + around);
        assert_eq!(
            tile_width(&of(1600, 900)),
            landscape.x + around,
            "a tile is the width of its own picture"
        );
        assert!(
            tile_width(&of(900, 1200)) < tile_width(&of(1600, 900)),
            "a portrait was given a landscape's column, which is the gap beside it"
        );
    }

    /// The tally beside the set and duplicate counts: every picture that is not
    /// marked to keep, which is exactly what a cleanup would take. It follows the
    /// marks, so it moves as the marks do.
    #[test]
    fn the_selected_tally_is_every_picture_that_is_not_kept() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        assert_eq!(app.sets.len(), 2, "the two pairs were not found");
        let (first, second) = (app.sets[0].set_id, app.sets[1].set_id);
        let bytes_of = |app: &App, set: usize, keeper: i64| -> i64 {
            app.sets[set]
                .members
                .iter()
                .filter(|member| member.file_id != keeper)
                .map(|member| member.size_bytes)
                .sum()
        };
        let keeper = |app: &App, set_id: i64| match app.keep.get(&set_id) {
            Some(Keep::One(file_id)) => *file_id,
            other => panic!("no keeper for {set_id}: {other:?}"),
        };

        // Four pictures, one kept in each set.
        let (going, bytes) = app.selected_for_removal();
        assert_eq!(going, 2);
        assert_eq!(
            bytes,
            bytes_of(&app, 0, keeper(&app, first)) + bytes_of(&app, 1, keeper(&app, second))
        );

        // Keeping all of one set takes its picture out of the tally.
        app.keep.insert(second, Keep::All);
        let (going, bytes) = app.selected_for_removal();
        assert_eq!(going, 1);
        assert_eq!(bytes, bytes_of(&app, 0, keeper(&app, first)));

        // Keeping nothing in the other puts both of its pictures in.
        app.keep.remove(&first);
        let (going, bytes) = app.selected_for_removal();
        assert_eq!(going, 2);
        assert_eq!(bytes, bytes_of(&app, 0, -1), "both pictures should be counted");

        // And it is the same count the plan carries out.
        assert_eq!(app.build_plan().files(), 2);
    }

    /// What the toolbar counts: pictures that would go if every set kept one, not
    /// pictures that are in a set.
    #[test]
    fn the_duplicate_count_is_every_picture_but_the_one_each_set_keeps() {
        assert_eq!(duplicate_count(&[]), 0);

        // Five pictures in two sets: a pair and a triple.
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, seed) in [
            ("a1.png", 0),
            ("a2.png", 0),
            ("b1.png", 120),
            ("b2.png", 120),
            ("b3.png", 120),
        ] {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
                image::Rgb([((x * 3 + seed) % 256) as u8, ((y * 5 + seed) % 256) as u8, 40])
            }))
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
        }

        let app = reviewing(dir.path());
        assert_eq!(app.sets.len(), 2, "the pair and the triple were not two sets");
        assert_eq!(
            duplicate_count(&app.sets),
            3,
            "five pictures in two sets is three copies"
        );
    }

    /// The keys follow what is on screen. A set that is not in the list the walk
    /// was given is not somewhere the preview can go.
    #[test]
    fn walking_only_visits_the_sets_it_was_given() {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, seed) in
            [("a1.png", 0), ("a2.png", 0), ("b1.png", 90), ("b2.png", 90), ("c1.png", 180), ("c2.png", 180)]
        {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
                image::Rgb([((x * 3 + seed) % 256) as u8, ((y * 5 + seed) % 256) as u8, 40])
            }))
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
        }

        let mut app = reviewing(dir.path());
        assert_eq!(app.sets.len(), 3, "the three pairs were not three sets");
        let last_of_first = app.sets[0].members[1].file_id;
        let first_of_third = app.sets[2].members[0].file_id;

        app.selected = Some(last_of_first);
        app.walk(&[0, 2], Direction::Forward);
        assert_eq!(app.selected, Some(first_of_third), "the walk went into a hidden set");
    }

    /// A row the cursor keys reached must be brought into sight, and one already
    /// in sight must not shift the list under the pointer.
    #[test]
    fn the_row_walked_to_is_brought_to_the_middle() {
        let (height, spacing, viewport) = (240.0, 8.0, 700.0);
        let rows = 40;
        let show =
            |row: usize, offset: f32| scroll_to_show(row, rows, height, spacing, offset, viewport);

        // Rows are 248 apart and 700 is on screen, so a row in the middle sits
        // with its own middle at 350. Row one's middle is 368, so the list moves
        // by 18 even though the whole row was already in sight.
        assert_eq!(show(1, 0.0), Some(18.0), "a row in sight but off centre did not move");
        assert_eq!(show(2, 0.0), Some(616.0 - 350.0));
        assert_eq!(show(3, 0.0), Some(864.0 - 350.0));

        // Where it was reached from makes no difference: the row ends up in the
        // same place walking down to it or up to it.
        assert_eq!(show(3, 0.0), show(3, 5_000.0));

        // Nothing to do when it is already in the middle.
        assert_eq!(show(1, 18.0), None, "the list was moved to where it already was");

        // The ends are the exception. Row zero's middle is 120 and half a screen
        // above that is off the top, so it stops there.
        assert_eq!(show(0, 100.0), Some(0.0), "the first row scrolled past the top");
        let content = rows as f32 * (height + spacing) - spacing;
        assert_eq!(
            show(rows - 1, 0.0),
            Some(content - viewport),
            "the last row pulled the list past its own end"
        );
    }

    #[test]
    fn walking_asks_for_the_row_it_moved_to_to_be_shown() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        assert_eq!(app.sets.len(), 2, "the two pairs were not found");
        let visible = vec![0, 1];

        // From the last picture of the first set into the second set.
        app.selected = Some(app.sets[0].members[1].file_id);
        app.scroll_to = None;
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.scroll_to, Some(1));

        // And from the last picture of the list, which has nowhere to go.
        app.selected = Some(app.sets[1].members[1].file_id);
        app.scroll_to = None;
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.scroll_to, None, "the end of the list asked for a scroll");
    }

    /// The list places every row it is not drawing at a multiple of
    /// `SET_ROW_HEIGHT`. A row that comes out any other height moves the content
    /// under a scroll that is already running, which reads as the list trembling
    /// and jumping back the way it came.
    #[test]
    fn a_set_row_takes_exactly_the_height_the_list_places_it_at() {
        let ctx = window();
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        let root = found.path().to_path_buf();
        assert_eq!(app.sets.len(), 2, "the two pairs were not found");

        let mut taken = Vec::new();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for index in 0..app.sets.len() {
                    let placed = set_row_height(ui);
                    let before = ui.next_widget_position().y;
                    app.set_row(ui, index, &root, ui.available_width());
                    let spacing = ui.spacing().item_spacing.y;
                    taken.push((ui.next_widget_position().y - before - spacing, placed));
                }
            });
        });

        for (index, (height, placed)) in taken.iter().enumerate() {
            assert!(
                (height - placed).abs() < 0.5,
                "row {index} took {height}, the list places rows every {placed}"
            );
        }
    }

    /// Where duplicates go is a fact about the folder they were found in, so it
    /// lives in that folder's index and not in the application's settings.
    #[test]
    fn the_cleanup_choice_is_kept_with_the_folders_index() {
        // A folder the window scanned, with a destination chosen for it.
        let scanned = folder_with_a_duplicate();
        let held = tempfile::tempdir().expect("tempdir");
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(scanned.path().to_path_buf());
        app.start_scan();
        settle(&mut app);
        app.destination = Destination::MoveTo;
        app.move_dir = held.path().display().to_string();
        app.remember_disposal();

        // Opened again from nothing, as the next run of the window would.
        let mut opened = App::from_settings(crate::settings::Settings::default());
        assert_eq!(opened.destination, Destination::Trash, "the test started from the default");
        opened.open_folder(scanned.path().to_path_buf());
        settle(&mut opened);
        assert_eq!(opened.destination, Destination::MoveTo);
        assert_eq!(opened.move_dir, held.path().display().to_string());
    }

    /// How far down the folder an index reaches is a fact about the index, not a
    /// preference. Opening it with the box clear would drop every row under a
    /// subfolder as vanished on the next pass.
    #[test]
    fn an_index_built_over_the_subfolders_opens_with_the_box_ticked() {
        // A folder with a picture in a subfolder, scanned with the box ticked.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir(dir.path().join("under")).expect("mkdir");
        for name in ["top.png", "under/deep.png"] {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(48, 32, |x, y| {
                image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
            }))
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
        }

        let mut app = App::from_settings(crate::settings::Settings::default());
        assert!(!app.recurse, "the window starts on the folder itself");
        app.open_folder(dir.path().to_path_buf());
        app.recurse = true;
        app.start_scan();
        settle(&mut app);
        assert_eq!(app.scan.done, 2, "the pass did not go into the subfolder");

        // Opened again from nothing, as the next run of the window would.
        let mut opened = App::from_settings(crate::settings::Settings::default());
        opened.open_folder(dir.path().to_path_buf());
        settle(&mut opened);
        assert!(opened.recurse, "the index reaches into the subfolders and the box does not");

        // And a pass over the folder alone puts the box back down.
        opened.recurse = false;
        opened.start_scan();
        settle(&mut opened);
        let mut again = App::from_settings(crate::settings::Settings::default());
        again.open_folder(dir.path().to_path_buf());
        settle(&mut again);
        assert!(!again.recurse, "the index is the folder itself and the box says otherwise");
    }

    #[test]
    fn an_index_that_has_never_been_cleaned_up_keeps_the_safe_default() {
        let scanned = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(scanned.path().to_path_buf());
        app.start_scan();
        settle(&mut app);

        let mut opened = App::from_settings(crate::settings::Settings::default());
        opened.open_folder(scanned.path().to_path_buf());
        settle(&mut opened);
        assert_eq!(opened.destination, Destination::Trash);
        assert_eq!(opened.move_dir, "");
    }

    /// Moving files to a folder is not removing them, and the button that does it
    /// says so.
    #[test]
    fn the_button_says_what_the_chosen_destination_actually_does() {
        assert_eq!(Destination::MoveTo.verb(), "Move");
        assert_eq!(Destination::Trash.verb(), "Remove");
        assert_eq!(Destination::Delete.verb(), "Remove");
    }

    #[test]
    fn every_cleanup_choice_survives_being_written_and_read_back() {
        for choice in [Destination::Trash, Destination::MoveTo, Destination::Delete] {
            assert_eq!(Destination::from_name(choice.name()), Some(choice));
        }
        assert_eq!(Destination::from_name("something else"), None);
    }

    /// What was actually painted, so a scroll bar that is reserved but drawn in a
    /// colour nobody can see counts as missing. Twice now it has been invisible
    /// while the space for it was there.
    fn painted_rects() -> Vec<egui::epaint::RectShape> {
        let ctx = window();
        install_style(&ctx);
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        };

        let mut shapes = Vec::new();
        // A scroll area only knows on its second frame whether what it holds is
        // taller than it is, and then animates the bar to its full width over
        // several more. What is wanted here is where it settles.
        for _ in 0..30 {
            shapes = crate::shot::frame("a_list_that_scrolls", &ctx, input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    scrolled(
                        ui,
                        egui::Id::new("a list"),
                        true,
                        20.0,
                        egui::ScrollArea::vertical().auto_shrink([false, false]),
                        |area, ui| {
                            area.show(ui, |ui| {
                                for row in 0..200 {
                                    ui.label(format!("row {row}"));
                                }
                            })
                        },
                    );
                });
            });
        }

        shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Rect(rect) => Some(rect),
                _ => None,
            })
            .collect()
    }

    /// The review view as it is really built, panels and all, rather than a bare
    /// scroll area that proves nothing about it.
    fn review_rects(visuals: egui::Visuals) -> Vec<egui::epaint::RectShape> {
        review_rects_sized(visuals, egui::vec2(1200.0, 800.0), None)
    }

    fn review_rects_sized(
        visuals: egui::Visuals,
        screen: egui::Vec2,
        preview_width: Option<f32>,
    ) -> Vec<egui::epaint::RectShape> {
        let ctx = window();
        ctx.set_visuals(visuals);

        let mut app = App::from_settings(crate::settings::Settings {
            folder: Some(PathBuf::from(".")),
            preview_width,
            ..crate::settings::Settings::default()
        });
        app.view = View::Review;
        app.sets = (0..40)
            .map(|index| DuplicateSet {
                set_id: index,
                members: vec![
                    member(index * 2, "a.jpg", 500),
                    member(index * 2 + 1, "b.jpg", 300),
                ],
            })
            .collect();

        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), screen)),
            ..Default::default()
        };

        let mut shapes = Vec::new();
        for _ in 0..30 {
            shapes = crate::shot::frame("the_review_list_bar", &ctx, input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
            });
        }

        shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Rect(rect) => Some(rect),
                _ => None,
            })
            .collect()
    }

    /// The strip belongs to the list, so it sits at the left of whatever divides
    /// the list from the preview, not at the window edge.
    fn scroll_strip(rects: &[egui::epaint::RectShape]) -> Vec<&egui::epaint::RectShape> {
        rects
            .iter()
            .filter(|shape| {
                (shape.rect.width() - SCROLL_BAR).abs() < 0.5 && shape.rect.height() > SCROLL_BAR
            })
            .collect()
    }

    /// How far apart two colours are, so "visible" is a number and not a claim.
    fn contrast(a: egui::Color32, b: egui::Color32) -> i32 {
        let channel = |c: egui::Color32| {
            (c.r() as i32 * 299 + c.g() as i32 * 587 + c.b() as i32 * 114) / 1000
        };
        (channel(a) - channel(b)).abs()
    }

    #[test]
    fn the_review_list_paints_a_twelve_point_bar_beside_it_in_either_theme() {
        for (name, visuals) in
            [("light", egui::Visuals::light()), ("dark", egui::Visuals::dark())]
        {
            let rects = review_rects(visuals);
            let strip = scroll_strip(&rects);
            assert!(
                strip.len() >= 2,
                "{name}: no {SCROLL_BAR} point track and handle in the review view, \
                 the tall narrow rects were {:?}",
                rects
                    .iter()
                    .filter(|shape| shape.rect.width() < 40.0 && shape.rect.height() > 100.0)
                    .map(|shape| (shape.rect, shape.fill))
                    .collect::<Vec<_>>()
            );

            let tallest =
                strip.iter().max_by(|a, b| a.rect.height().total_cmp(&b.rect.height())).unwrap();
            let handle =
                strip.iter().min_by(|a, b| a.rect.height().total_cmp(&b.rect.height())).unwrap();
            println!(
                "{name}: track {:?} {:?}, handle {:?} {:?}",
                tallest.rect, tallest.fill, handle.rect, handle.fill
            );

            assert!(handle.rect.height() < tallest.rect.height(), "{name}: no handle in the track");
            assert!(handle.fill.a() > 0 && tallest.fill.a() > 0, "{name}: painted transparent");
            let apart = contrast(handle.fill, tallest.fill);
            assert!(apart >= 40, "{name}: the handle is {apart} apart from its track, invisible");
        }
    }

    /// The bar is drawn here rather than by the toolkit, so everything on it has
    /// to be checked: a track, a handle inside it, and a triangle at each end.
    #[test]
    fn the_bar_has_a_track_a_handle_and_a_button_at_each_end() {
        let ctx = window();
        ctx.set_visuals(egui::Visuals::light());
        install_style(&ctx);

        let strip = egui::Rect::from_min_size(egui::pos2(388.0, 0.0), egui::vec2(12.0, 300.0));
        let shapes = crate::shot::frame(
            "the_bar_has_a_track_a_handle_and_a_button_at_each_end",
            &ctx,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    paint_scroll_bar(ui, strip, true, 60.0, 3000.0, 300.0, 0.0);
                });
            },
        );

        let mut track = None;
        let mut handle = None;
        let mut triangles = 0;
        for clipped in shapes {
            match clipped.shape {
                egui::Shape::Rect(rect) if rect.rect == strip => track = Some(rect),
                egui::Shape::Rect(rect) if rect.rect.width() == strip.width() => {
                    handle = Some(rect);
                }
                egui::Shape::Path(path) if path.points.len() == 3 => triangles += 1,
                _ => {}
            }
        }

        let track = track.expect("no track filling the strip");
        let handle = handle.expect("no handle in the track");
        assert_eq!(triangles, 2, "expected a triangle at each end of the bar");
        assert!(
            handle.rect.height() < track.rect.height(),
            "the handle is not shorter than its track"
        );
        assert!(
            handle.rect.top() >= track.rect.top() + strip.width(),
            "the handle overlaps the button at the top"
        );
        assert!(
            handle.rect.bottom() <= track.rect.bottom() - strip.width(),
            "the handle overlaps the button at the bottom"
        );
        assert!(
            contrast(handle.fill, track.fill) >= 40,
            "the handle cannot be told from its track"
        );
    }

    /// Pressing the handle holds it where it is, and dragging moves it by how far
    /// the pointer moved. It must not jump so its middle is under the pointer.
    #[test]
    fn pressing_the_handle_holds_it_where_it_was_and_drags_from_there() {
        let ctx = window();
        ctx.set_visuals(egui::Visuals::light());

        let strip = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(12.0, 312.0));
        let (content, viewport, step) = (3000.0, 300.0, 60.0);

        // A third of the way down, and where the handle sits when it is, worked
        // out the same way the bar does rather than written down here.
        let offset = (content - viewport) / 3.0;
        let track_length = strip.height() - strip.width() * 2.0;
        let handle_length = (track_length * viewport / content).max(strip.width() * 2.0);
        let travel = track_length - handle_length;
        let handle_start = strip.width() + (offset / (content - viewport)) * travel;

        let press_at = |y: f32, offset: f32| {
            let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 400.0));
            let at = egui::pos2(6.0, y);
            let wanted = std::cell::RefCell::new(None);

            // The pointer moves on one frame, goes down on the next, and comes up
            // on the last. A widget has to have been there on a previous frame
            // before egui reports the pointer as being on it, and the release is
            // what lets go of the grip for whatever presses next.
            for button in [None, Some(true), Some(false)] {
                let mut input =
                    egui::RawInput { screen_rect: Some(screen), ..Default::default() };
                input.events.push(egui::Event::PointerMoved(at));
                if let Some(pressed) = button {
                    input.events.push(egui::Event::PointerButton {
                        pos: at,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
                // What the frame drew is not looked at here: this drives the bar
                // and reads what it decided, which comes back through `wanted`.
                // The picture is kept anyway, for when it decides something else.
                let _ = crate::shot::frame(
                    "pressing_the_handle_holds_it_where_it_was",
                    &ctx,
                    input,
                    |ctx| {
                        egui::CentralPanel::default().show(ctx, |ui| {
                            let moved =
                                paint_scroll_bar(ui, strip, true, step, content, viewport, offset);
                            if button == Some(true) {
                                *wanted.borrow_mut() = moved;
                            }
                        });
                    },
                );
            }
            wanted.into_inner()
        };

        // Anywhere on the handle: it stays where it is, rather than jumping so
        // its middle is under the pointer.
        for (name, at) in [
            ("its middle", handle_start + handle_length / 2.0),
            ("its top", handle_start + 2.0),
            ("its bottom", handle_start + handle_length - 2.0),
        ] {
            let held = press_at(at, offset).expect("the press did nothing");
            assert!(
                (held - offset).abs() < 1.0,
                "pressing {name} moved the handle from {offset} to {held}"
            );
        }

        // The track outside the handle has nothing to hold, so it jumps.
        let jumped = press_at(handle_start + handle_length + 40.0, offset)
            .expect("the press did nothing");
        assert!(jumped > offset, "pressing below the handle did not move down: {jumped}");
    }

    /// The same bar lies on its side for anything that scrolls sideways, and its
    /// buttons point the way they scroll.
    #[test]
    fn a_sideways_bar_has_the_same_parts_lying_down() {
        let ctx = window();
        ctx.set_visuals(egui::Visuals::light());

        let strip = egui::Rect::from_min_size(egui::pos2(0.0, 288.0), egui::vec2(400.0, 12.0));
        let shapes = crate::shot::frame(
            "a_sideways_bar_has_the_same_parts_lying_down",
            &ctx,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(400.0, 300.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    paint_scroll_bar(ui, strip, false, 60.0, 4000.0, 400.0, 0.0);
                });
            },
        );

        let mut track = None;
        let mut handle = None;
        let mut triangles = Vec::new();
        for clipped in shapes {
            match clipped.shape {
                egui::Shape::Rect(rect) if rect.rect == strip => track = Some(rect),
                egui::Shape::Rect(rect) if rect.rect.height() == strip.height() => {
                    handle = Some(rect);
                }
                egui::Shape::Path(path) if path.points.len() == 3 => triangles.push(path),
                _ => {}
            }
        }

        let track = track.expect("no track filling the strip");
        let handle = handle.expect("no handle in the track");
        assert_eq!(triangles.len(), 2, "expected a button at each end");
        assert!(handle.rect.width() < track.rect.width(), "the handle is as wide as its track");
        assert!(handle.rect.left() >= track.rect.left() + strip.height(), "over the left button");

        // One triangle points left and one right, or both buttons look the same.
        let widest = |path: &egui::epaint::PathShape| {
            let xs: Vec<f32> = path.points.iter().map(|point| point.x).collect();
            xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min)
        };
        assert!(
            triangles.iter().all(|path| widest(path) > 0.0),
            "the triangles have no width, so they are not pointing sideways"
        );
    }

    /// With the preview pane dragged wide, the list is a narrow column. The bar
    /// still belongs to it and still has to be there.
    #[test]
    fn the_bar_is_there_when_the_preview_has_taken_most_of_the_window() {
        let rects = review_rects_sized(
            egui::Visuals::light(),
            egui::vec2(1523.0, 1067.0),
            Some(1162.0),
        );
        let strip = scroll_strip(&rects);
        for shape in &strip {
            println!("narrow list: bar {:?} filled {:?}", shape.rect, shape.fill);
        }
        assert!(
            strip.len() >= 2,
            "no bar beside a list only {} points wide, the tall narrow rects were {:?}",
            1523.0 - 1162.0,
            rects
                .iter()
                .filter(|shape| shape.rect.width() < 40.0 && shape.rect.height() > 100.0)
                .map(|shape| (shape.rect, shape.fill))
                .collect::<Vec<_>>()
        );
    }

    #[test]
    fn a_list_that_scrolls_paints_a_twelve_point_handle_at_its_right_edge() {
        let rects = painted_rects();
        let right = 400.0;

        let strip: Vec<&egui::epaint::RectShape> = rects
            .iter()
            .filter(|shape| {
                (shape.rect.right() - right).abs() < 12.0
                    && (shape.rect.width() - SCROLL_BAR).abs() < 0.5
            })
            .collect();
        assert!(
            strip.len() >= 2,
            "no {SCROLL_BAR} point track and handle at the right edge, found {:?}",
            rects.iter().map(|shape| shape.rect).collect::<Vec<_>>()
        );

        // The track and the handle have to be told apart, or the bar is a strip
        // of one flat colour and says nothing about where the list is.
        let colours: std::collections::HashSet<[u8; 4]> =
            strip.iter().map(|shape| shape.fill.to_array()).collect();
        assert!(
            colours.len() >= 2,
            "the handle is the same colour as the track it sits in: {colours:?}"
        );
        assert!(
            strip.iter().all(|shape| shape.fill.a() > 0),
            "the bar was painted fully transparent: {colours:?}"
        );

        // And the handle is shorter than the track, or it is not showing how much
        // of the list is on screen.
        let tallest = strip.iter().map(|shape| shape.rect.height()).fold(0.0f32, f32::max);
        let shortest = strip.iter().map(|shape| shape.rect.height()).fold(f32::MAX, f32::min);
        assert!(shortest < tallest, "the handle fills the whole track: {shortest} of {tallest}");
    }

    /// The keeper is what puts a set in the plan. There is no second mark to set,
    /// so a set that was never touched is still cleaned up.
    #[test]
    fn a_set_removes_everything_but_the_kept_file() {
        let found = folder_with_a_duplicate();
        let app = reviewing(found.path());
        let kept = match app.keep.get(&app.sets[0].set_id) {
            Some(Keep::One(file_id)) => *file_id,
            other => panic!("the search gave the set no keeper: {other:?}"),
        };
        let plan = app.build_plan();

        assert_eq!(plan.files(), 1);
        let staying = app.sets[0]
            .members
            .iter()
            .find(|member| member.file_id == kept)
            .expect("the keeper is in the set");
        assert_ne!(plan.removals[0].rel_path, staying.rel_path, "the keeper is in the plan");
        assert_eq!(plan.bytes(), plan.removals[0].size_bytes);
    }

    #[test]
    fn moving_the_keep_mark_moves_what_gets_removed() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let was_going = app.build_plan().removals[0].rel_path.clone();
        let moving_to = app.sets[0]
            .members
            .iter()
            .find(|member| member.rel_path == was_going)
            .expect("the plan removes a picture in the set")
            .file_id;

        app.selected = Some(moving_to);
        app.keep_selected();

        let plan = app.build_plan();
        assert_eq!(plan.files(), 1);
        assert_ne!(plan.removals[0].rel_path, was_going, "the keeper did not move");
    }

    #[test]
    fn keeping_everything_in_a_set_removes_nothing_from_it() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        app.keep.insert(app.sets[0].set_id, Keep::All);
        assert_eq!(app.build_plan().files(), 0, "a set keeping all of it produced removals");
    }

    /// Everywhere a label was drawn, so a test can press a button or measure a
    /// row without working the layout out a second time here.
    fn label_rects(shapes: &[egui::epaint::ClippedShape], label: &str) -> Vec<egui::Rect> {
        fn walk(shape: &egui::Shape, label: &str, found: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Text(text) if text.galley.text() == label => {
                    found.push(text.galley.rect.translate(text.pos.to_vec2()));
                }
                egui::Shape::Vec(inner) => {
                    for shape in inner {
                        walk(shape, label, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, label, &mut found);
        }
        found
    }

    fn label_rect(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<egui::Rect> {
        label_rects(shapes, label).into_iter().next()
    }

    /// Every line of text that was painted, with where it went.
    /// The box a set is drawn in: the widest rectangle painted with an outline,
    /// which is the group frame around the row.
    fn box_around_the_set(shapes: &[egui::epaint::ClippedShape]) -> Option<egui::Rect> {
        fn walk(shape: &egui::Shape, found: &mut Vec<egui::Rect>) {
            match shape {
                egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => found.push(rect.rect),
                egui::Shape::Vec(inner) => {
                    for shape in inner {
                        walk(shape, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut found);
        }
        found.into_iter().max_by(|one, other| {
            one.width().partial_cmp(&other.width()).unwrap_or(std::cmp::Ordering::Equal)
        })
    }

    fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
        fn walk(shape: &egui::Shape, found: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(text) => found.push((
                    text.galley.text().to_string(),
                    text.galley.rect.translate(text.pos.to_vec2()),
                )),
                egui::Shape::Vec(inner) => {
                    for shape in inner {
                        walk(shape, found);
                    }
                }
                _ => {}
            }
        }
        let mut found = Vec::new();
        for clipped in shapes {
            walk(&clipped.shape, &mut found);
        }
        found
    }

    /// A set drawn from a really scanned folder, with one picture marked to keep
    /// and one not. Only the marked one says anything, and the space is taken
    /// either way, so the facts under the pictures stay on one line across the
    /// set.
    #[test]
    fn only_the_picture_being_kept_is_labelled_and_the_others_keep_the_space() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let members = &app.sets[0].members;
        assert_eq!(members.len(), 2, "the two copies were not found");
        let size = format!("{}x{}", members[0].width, members[0].height);
        assert_eq!(
            size,
            format!("{}x{}", members[1].width, members[1].height),
            "the fixture pictures are not the same shape"
        );
        assert!(
            app.keep.get(&app.sets[0].set_id).is_some(),
            "the search marked nothing to keep"
        );

        let root = found.path().to_path_buf();
        let ctx = window();
        let shapes = crate::shot::frame(
            "only_the_picture_being_kept_is_labelled_and_the_others_keep_the_space",
            &ctx,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 500.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            },
        );

        assert_eq!(
            label_rects(&shapes, "KEEP").len(),
            1,
            "the one picture being kept is not the only one saying so"
        );
        assert!(
            label_rects(&shapes, "keep this").is_empty(),
            "a picture that is not being kept was labelled anyway"
        );

        let sizes = label_rects(&shapes, &size);
        assert_eq!(sizes.len(), 2, "both pictures should say what shape they are");
        assert!(
            (sizes[0].top() - sizes[1].top()).abs() < 0.5,
            "the unmarked picture pulled its text up: {:?} against {:?}",
            sizes[0],
            sizes[1]
        );
    }

    /// One of something is not "1 sets". The counts above the review list say
    /// what they are counting, and the word changes with the number.
    #[test]
    fn one_of_something_is_written_in_the_singular() {
        assert_eq!(counted(1, "set", "sets"), "1 set");
        assert_eq!(counted(1, "duplicate", "duplicates"), "1 duplicate");
        assert_eq!(counted(0, "set", "sets"), "0 sets");
        assert_eq!(counted(2, "duplicate", "duplicates"), "2 duplicates");
    }

    /// The box around a set is as tall as what it holds. The strip is a fixed
    /// height, and any of it the tiles do not use is empty space under the file
    /// names in every row of the list.
    #[test]
    fn a_set_box_is_not_taller_than_the_tiles_in_it() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let root = found.path().to_path_buf();

        let ctx = window();
        let mut taken = 0.0;
        let mut buttons = 0.0;
        let shapes = crate::shot::frame(
            "a_set_box_is_not_taller_than_the_tiles_in_it",
            &ctx,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 500.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    buttons = button_row_height(ui);
                    let before = ui.next_widget_position().y;
                    app.set_row(ui, 0, &root, ui.available_width());
                    taken = ui.next_widget_position().y - before;
                });
            },
        );

        fn lowest(shape: &egui::Shape, so_far: &mut f32) {
            match shape {
                egui::Shape::Text(text) => {
                    *so_far = so_far.max(text.pos.y + text.galley.rect.height());
                }
                egui::Shape::Vec(inner) => {
                    for shape in inner {
                        lowest(shape, so_far);
                    }
                }
                _ => {}
            }
        }
        let mut bottom = 0.0_f32;
        for clipped in &shapes {
            lowest(&clipped.shape, &mut bottom);
        }
        assert!(bottom > 0.0, "the row painted no text at all");

        // What is under the last line: the strip's scroll bar, the row of
        // buttons, the padding the frame draws inside its own edge, and the
        // space to the next row. Nothing else.
        let spare = taken - bottom;
        assert!(
            spare < SCROLL_BAR + buttons + BOX_PADDING + 2.0 * BOX_EDGE + 8.0,
            "the box is {spare} points taller than the tiles in it"
        );
    }

    /// Nothing in the window explains itself by being hovered over. What a
    /// control does is written on it, or it does not belong there.
    #[test]
    fn nothing_in_the_window_shows_a_tooltip() {
        // Split so this test does not find itself.
        let hover = concat!("on_hover", "_text");
        let window = include_str!("app.rs");
        assert!(
            !window.contains(hover),
            "the window has gone back to explaining itself in tooltips"
        );
        // A label that has to cut its text puts the whole string in a tooltip of
        // egui's own making, which is why the ones here paint the text instead.
        assert!(
            !window.contains(concat!(".trunc", "ate()")),
            "a label is cutting its text, which egui explains in a tooltip"
        );
    }

    /// Dragging across the file names really tried, in a scanned folder. Nothing
    /// is a text field: no line highlights, and the pointer never becomes a
    /// text cursor.
    #[test]
    fn dragging_across_the_window_selects_no_text() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let root = found.path().to_path_buf();

        let ctx = window();
        install_style(&ctx);
        let screen =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let frame = |app: &mut App, at: egui::Pos2, pressed: Option<bool>| {
            let mut input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            input.events.push(egui::Event::PointerMoved(at));
            if let Some(pressed) = pressed {
                input.events.push(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
            ctx.run(input, |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            })
        };

        // Both themes: the machine's is applied after the window is set up, and
        // one of the two going unchanged is what shipped a selectable review tab.
        for theme in [egui::Theme::Dark, egui::Theme::Light] {
            assert!(
                !ctx.style_of(theme).interaction.selectable_labels,
                "labels are still text to be selected in the {theme:?} theme"
            );
        }

        // Press on the first line under a picture and drag across the rest.
        let start = egui::pos2(20.0, 200.0);
        frame(&mut app, start, None);
        frame(&mut app, start, Some(true));
        for step in 1..8 {
            frame(&mut app, start + egui::vec2(step as f32 * 20.0, 8.0), None);
        }
        let output = frame(&mut app, start + egui::vec2(160.0, 8.0), Some(false));

        assert!(
            output.platform_output.cursor_icon != egui::CursorIcon::Text,
            "the pointer turned into a text cursor over the window"
        );
    }

    /// The pointer really held over every part of a set, in a folder whose file
    /// names are far too long for a tile. Nothing pops up: not over the picture,
    /// not over the name that had to be cut, not over the buttons.
    #[test]
    fn holding_the_pointer_over_a_set_pops_nothing_up() {
        let dir = tempfile::tempdir().expect("tempdir");
        let long = "a_file_name_far_too_long_to_fit_under_a_picture_in_a_tile";
        for name in [format!("{long}_one.png"), format!("{long}_two.png")] {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
                image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
            }))
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
        }
        let mut app = reviewing(dir.path());
        let root = dir.path().to_path_buf();
        assert_eq!(app.sets.len(), 1, "the two copies were not found");

        let ctx = window();
        install_style(&ctx);
        let screen =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let frame = |app: &mut App, at: Option<egui::Pos2>, time: f64| {
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            if let Some(pos) = at {
                input.events.push(egui::Event::PointerMoved(pos));
            }
            crate::shot::frame("holding_the_pointer_over_a_set", &ctx, input, |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            })
        };

        // Every place a pointer can rest inside the row, one point apart, and
        // twice each: egui shows a tooltip on the frame after the pointer
        // arrives, never on the same one.
        let drawn = frame(&mut app, None, 0.0);
        let row = texts(&drawn)
            .iter()
            .map(|(_, rect)| *rect)
            .fold(egui::Rect::NOTHING, |all, rect| all.union(rect));
        let mut clock = 1.0;
        let mut y = row.top();
        while y < row.bottom() {
            let mut x = row.left();
            while x < row.right() {
                let at = egui::pos2(x, y);
                // Arrive, then wait: a tooltip is held back until the pointer has
                // been still for a moment.
                frame(&mut app, Some(at), clock);
                clock += 2.0;
                let painted = frame(&mut app, Some(at), clock);
                clock += 2.0;
                // One line per tile and no more. A tooltip would be a third
                // drawing of the name, over the top of the two.
                let names = texts(&painted)
                    .into_iter()
                    .filter(|(text, _)| text.contains(long))
                    .count();
                assert_eq!(
                    names, 2,
                    "the pointer at {at:?} left {names} copies of the name on screen"
                );
                x += 12.0;
            }
            y += 12.0;
        }
    }

    /// A set wider than the window, walked along with the cursor keys. The strip
    /// follows the selection: the picture the preview is showing is on screen,
    /// whichever end of the set it is at.
    #[test]
    fn walking_along_a_long_set_brings_the_selected_picture_into_view() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.view = View::Review;
        app.sets = vec![DuplicateSet {
            set_id: 7,
            members: (0..24).map(|index| member(index, "a.jpg", 500)).collect(),
        }];
        app.selected = Some(0);
        let root = PathBuf::from(".");
        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let bar = egui::Id::new(("set bar", 7));

        // What the strip is asked to do on the frame after a key press. It is
        // taken up by the scrolling itself on the frame after that, so it is read
        // straight away or not at all.
        let frame = |app: &mut App| {
            let _ = ctx.run(
                egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
                },
            );
            ctx.data(|data| data.get_temp::<f32>(bar))
        };

        frame(&mut app);
        // Walked along the set a picture at a time, the way the right cursor key
        // walks it.
        let mut asked: Vec<Option<f32>> = Vec::new();
        for _ in 0..23 {
            app.walk(&[0], Direction::Forward);
            asked.push(frame(&mut app));
        }
        assert_eq!(app.selected, Some(23), "the walk did not reach the end of the set");

        assert!(
            asked[..2].iter().all(Option::is_none),
            "the strip moved for pictures that were already on it: {:?}",
            &asked[..2]
        );
        let moved: Vec<f32> = asked.iter().flatten().copied().collect();
        assert!(!moved.is_empty(), "the strip never moved for a picture off the end of it");
        assert!(
            moved.windows(2).all(|pair| pair[1] > pair[0]),
            "the strip did not follow the selection along: {moved:?}"
        );

        // And back to the near end, one picture at a time.
        for _ in 0..23 {
            app.walk(&[0], Direction::Back);
            frame(&mut app);
        }
        assert_eq!(app.selected, Some(0));
        assert_eq!(
            frame(&mut app),
            None,
            "the first picture was on screen and the strip was asked to move anyway"
        );
    }

    /// The three buttons sit in a row along the bottom of a set, in order and
    /// with space between them, under the pictures rather than over them. The
    /// first two decide what the set keeps and the third says it is not a set of
    /// copies at all.
    #[test]
    fn a_set_has_its_buttons_in_a_row_along_the_bottom() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let root = found.path().to_path_buf();

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let drawn = crate::shot::frame(
            "a_set_has_its_buttons_in_a_row_along_the_bottom",
            &ctx,
            egui::RawInput { screen_rect: Some(screen), ..Default::default() },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            },
        );

        let painted = texts(&drawn);
        let where_it_is = |label: &str| {
            painted
                .iter()
                .find(|(text, _)| text == label)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("no {label} button was drawn"))
        };
        let all = where_it_is("keep all");
        let none = where_it_is("keep none");
        let ignore = where_it_is("ignore");

        // In that order, left to right, with room between them.
        assert!(all.right() < none.left(), "keep none is not right of keep all");
        assert!(none.right() < ignore.left(), "ignore is not right of keep none");
        assert!(none.left() - all.right() > 4.0, "the buttons are not spaced apart");
        // The same space after each of them, however long its word is.
        let first = none.left() - all.right();
        let second = ignore.left() - none.right();
        assert!(
            (first - second).abs() < 2.0,
            "the gaps between the buttons are {first} and {second}"
        );
        assert!(
            (all.center().y - ignore.center().y).abs() < 2.0,
            "the buttons are not on one row"
        );

        // Under the pictures: below the lowest line of text in the tiles.
        let lowest = painted
            .iter()
            .filter(|(text, _)| !["keep all", "keep none", "ignore"].contains(&text.as_str()))
            .map(|(_, rect)| rect.bottom())
            .fold(0.0_f32, f32::max);
        assert!(all.top() >= lowest, "the buttons are over the pictures rather than under them");
    }

    /// A set row is as tall as what it holds, and as wide as the room it is
    /// given without running under the list's scroll bar. The box around a set
    /// is drawn by a frame that adds its own margin, so a row built to the whole
    /// of the available width comes out that much wider than the room for it.
    #[test]
    fn a_set_row_fits_the_room_it_is_given() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let root = found.path().to_path_buf();

        let ctx = window();
        let screen =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let mut bar_at = 0.0_f32;
        let mut list_at = 0.0_f32;
        let mut buttons = 0.0_f32;
        let drawn = crate::shot::frame(
            "a_set_row_fits_the_room_it_is_given",
            &ctx,
            egui::RawInput { screen_rect: Some(screen), ..Default::default() },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    buttons = button_row_height(ui);
                    // As the list gives it: the page's margin down the left, and
                    // the width less the scroll bar it paints down the right and
                    // the gap kept before that bar.
                    let whole = ui.available_rect_before_wrap();
                    let room = egui::Rect::from_min_max(
                        egui::pos2(whole.left() + PAGE_MARGIN, whole.top()),
                        whole.max,
                    );
                    let inside = room.with_max_x(room.right() - SCROLL_BAR - PAGE_MARGIN);
                    list_at = whole.left();
                    bar_at = room.right() - SCROLL_BAR;
                    let wide = inside.width();
                    ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inside), |ui| {
                        app.set_row(ui, 0, &root, wide)
                    });
                });
            },
        );

        let outline = box_around_the_set(&drawn).expect("no box was drawn around the set");
        let painted = texts(&drawn);
        let last = painted
            .iter()
            .filter(|(text, _)| !text.starts_with("keep"))
            .map(|(_, rect)| rect.bottom())
            .fold(0.0_f32, f32::max);

        // The gap under the last line is the strip's own scroll bar, the row of
        // buttons, and the margin the frame draws with, and nothing else.
        let under = outline.bottom() - last;
        assert!(
            under < SCROLL_BAR + buttons + 14.0,
            "the box goes {under} past the last line under the pictures"
        );
        // The left of the box sits a margin in from where the list begins, and
        // its right leaves the same margin before the list's scroll bar.
        let left = outline.left() - list_at;
        let right = bar_at - outline.right();
        assert!(
            (left - right).abs() < 3.0,
            "the box is {left} from the left edge and {right} from the scroll bar"
        );
    }

    /// Two clicks on a picture in a really scanned folder do what the space bar
    /// does on it: keep that one, and on a second pair of clicks let it go again.
    #[test]
    fn two_clicks_on_a_picture_keep_it_the_way_the_space_bar_does() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let root = found.path().to_path_buf();
        let set_id = app.sets[0].set_id;
        let other = app.sets[0]
            .members
            .iter()
            .map(|member| member.file_id)
            .find(|file_id| app.keep.get(&set_id) != Some(&Keep::One(*file_id)))
            .expect("both pictures are the keeper");

        let ctx = window();
        let screen =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        // The clock has to move between the two pairs, or four clicks that close
        // together are one gesture rather than two.
        let frame = |app: &mut App, at: Option<egui::Pos2>, clicks: usize, time: f64| {
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            if let Some(pos) = at {
                input.events.push(egui::Event::PointerMoved(pos));
                for _ in 0..clicks {
                    for pressed in [true, false] {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        });
                    }
                }
            }
            crate::shot::frame("two_clicks_on_a_picture", &ctx, input, |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            })
        };

        // The pictures are the tile sized rectangles, and the one wanted here is
        // the one that is not already the keeper: the tiles are drawn in the
        // order the set holds them.
        let drawn = frame(&mut app, None, 0, 0.0);
        let mut pictures: Vec<egui::Rect> = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - TILE.x).abs() < 6.0
                        && (rect.rect.height() - TILE.y).abs() < 6.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        pictures.sort_by(|a, b| a.left().total_cmp(&b.left()));
        assert_eq!(pictures.len(), 2, "the two pictures were not drawn: {pictures:?}");
        let index = app.sets[0]
            .members
            .iter()
            .position(|member| member.file_id == other)
            .expect("the set lost a picture");
        let at = pictures[index].center();

        frame(&mut app, Some(at), 0, 0.1);
        frame(&mut app, Some(at), 2, 0.2);
        assert_eq!(
            app.keep.get(&set_id),
            Some(&Keep::One(other)),
            "two clicks did not keep the picture they were on"
        );

        frame(&mut app, Some(at), 0, 2.0);
        frame(&mut app, Some(at), 2, 2.1);
        assert_eq!(app.keep.get(&set_id), None, "twice more did not let it go again");
    }

    /// The toolbar over the review holds three things in one row, and each is in
    /// its own place: the checkbox against the left edge, the counts in the
    /// middle of the window, and the button against the right edge.
    #[test]
    fn the_review_toolbar_holds_the_box_left_the_counts_centred_and_the_button_right() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let shapes = crate::shot::frame(
            "the_review_toolbar_holds_the_box_left_the_counts_centred_and_the_button_right",
            &ctx,
            egui::RawInput { screen_rect: Some(screen), ..Default::default() },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
            },
        );

        // The toolbar is the top of the window, so what is drawn below it is the
        // review itself and no part of this.
        let painted: Vec<(String, egui::Rect)> =
            texts(&shapes).into_iter().filter(|(_, rect)| rect.top() < 40.0).collect();
        let one = |wanted: &str| {
            painted
                .iter()
                .find(|(text, _)| text == wanted)
                .map(|(_, rect)| *rect)
                .unwrap_or_else(|| panic!("{wanted} was not drawn in the toolbar: {painted:?}"))
        };
        let box_label = one("allow multi-select");
        let button = one("Clean up");
        let counts = painted
            .iter()
            .filter(|(text, _)| text != "allow multi-select" && text != "Clean up")
            .map(|(_, rect)| *rect)
            .reduce(|all, rect| all.union(rect))
            .expect("no counts were drawn");

        assert!(box_label.left() < 60.0, "the checkbox is not against the left edge: {box_label:?}");
        assert!(
            button.right() > screen.right() - 60.0,
            "the button is not against the right edge: {button:?}"
        );
        assert!(
            (counts.center().x - screen.center().x).abs() < 12.0,
            "the counts are centred on {} and the window on {}",
            counts.center().x,
            screen.center().x
        );
        assert!(
            counts.left() > box_label.right() && counts.right() < button.left(),
            "the counts run into the checkbox or the button: {counts:?}"
        );
    }

    /// What the file says about itself is under the picture in the preview: the
    /// name of each thing on the left and what it says on the right. It is read
    /// off the file on another thread, so it arrives a frame or two after the
    /// picture is clicked rather than with it.
    #[test]
    fn the_preview_shows_what_the_file_says_about_itself() {
        let found = folder_with_a_duplicate();
        // A comment written into one of them, the way a PNG carries text: the
        // name, a zero, and the words.
        let first = std::fs::read_dir(found.path())
            .expect("folder")
            .filter_map(|entry| entry.ok().map(|entry| entry.path()))
            .find(|path| path.extension().is_some_and(|ext| ext == "png"))
            .expect("a picture");
        let mut bytes = std::fs::read(&first).expect("read");
        let mut chunk = b"tEXtNotes\0taken in a garden".to_vec();
        let length = (chunk.len() - 4) as u32;
        let sum = png_check(&chunk);
        let mut piece = length.to_be_bytes().to_vec();
        piece.append(&mut chunk);
        piece.extend_from_slice(&sum.to_be_bytes());
        let end = bytes.len() - 12;
        bytes.splice(end..end, piece);
        std::fs::write(&first, &bytes).expect("write");

        let mut app = reviewing(found.path());
        app.selected = app.sets[0]
            .members
            .iter()
            .find(|member| first.ends_with(&member.rel_path))
            .map(|member| member.file_id);

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1100.0, 700.0));
        let waited = std::time::Instant::now();
        loop {
            let shapes = crate::shot::frame(
                "the_preview_shows_what_the_file_says_about_itself",
                &ctx,
                egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
                },
            );
            let painted = texts(&shapes);
            let said = |wanted: &str| painted.iter().any(|(text, _)| text == wanted);
            if said("Notes") && said("taken in a garden") {
                break;
            }
            assert!(
                waited.elapsed().as_secs() < 20,
                "the preview never showed what the file says: {:?}",
                painted.iter().map(|(text, _)| text.as_str()).collect::<Vec<&str>>()
            );
            std::thread::yield_now();
        }
    }

    /// A click on the preview fills the window with the picture, and the escape
    /// key puts it back. The picture is asked for at the size of the window
    /// rather than at the size of the pane it came from.
    #[test]
    fn a_click_on_the_preview_fills_the_window_and_escape_puts_it_back() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let first = app.sets[0].members[0].file_id;
        app.selected = Some(first);

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1100.0, 700.0));
        let mut clock = 0.0;
        let mut frame = |app: &mut App, events: Vec<egui::Event>| {
            clock += 0.1;
            let _ = ctx.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    time: Some(clock),
                    events,
                    ..Default::default()
                },
                |ctx| {
                    // What the workers have read becomes a texture on the frame
                    // that draws it, which is the window's own job.
                    app.thumbs.collect(ctx);
                    egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
                    app.filling_the_window(ctx);
                },
            );
        };

        // The picture in the pane, once it has been read.
        let waited = std::time::Instant::now();
        while app.showing.is_none() {
            frame(&mut app, Vec::new());
            assert!(waited.elapsed().as_secs() < 20, "the preview never arrived");
            std::thread::yield_now();
        }
        assert_eq!(app.filling_the_window, None, "it started out filling the window");

        // A click in the middle of the pane, which is where the picture is.
        let at = egui::pos2(screen.right() - 230.0, 220.0);
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        frame(
            &mut app,
            vec![
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: true,
                    modifiers: Default::default(),
                },
                egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                },
            ],
        );
        assert_eq!(
            app.filling_the_window,
            Some(first),
            "a click on the preview did not fill the window with it"
        );

        frame(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(app.filling_the_window, None, "escape did not put the picture back");
    }

    /// The check every PNG chunk carries, for the fixture above.
    fn png_check(bytes: &[u8]) -> u32 {
        let mut value = 0xFFFF_FFFFu32;
        for byte in bytes {
            value ^= *byte as u32;
            for _ in 0..8 {
                value = if value & 1 != 0 { 0xEDB8_8320 ^ (value >> 1) } else { value >> 1 };
            }
        }
        value ^ 0xFFFF_FFFF
    }

    /// The box is off to begin with, and ticking it is a fact about the folder:
    /// the index keeps it, and opening that folder again comes back with it.
    #[test]
    fn the_index_keeps_whether_multi_selected_was_ticked() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        assert!(!app.multi_select, "multi-select was on before anything ticked it");

        app.multi_select = true;
        app.remember_multi_select();

        let again = reviewing(found.path());
        assert!(again.multi_select, "the folder was opened again without the box ticked");
    }

    /// The two ways of matching are on the page, in the box that says what
    /// counts as a duplicate, and clicking one turns it off.
    #[test]
    fn the_ways_of_matching_are_boxes_on_the_page_that_can_be_clicked() {
        let ctx = egui::Context::default();
        let mut app = App::from_settings(crate::settings::Settings::default());
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let frame = |app: &mut App, at: Option<egui::Pos2>, pressed: Option<bool>| {
            let mut input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            if let Some(pos) = at {
                input.events.push(egui::Event::PointerMoved(pos));
                if let Some(pressed) = pressed {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
            crate::shot::frame("the_ways_of_matching_are_boxes", &ctx, input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.matching_section(ui, 600.0);
                });
            })
        };

        let drawn = frame(&mut app, None, None);
        let whole = label_rect(&drawn, "Match whole pictures")
            .expect("the box for matching whole pictures was not drawn");
        let crops = label_rect(&drawn, "Match partials")
            .expect("the box for matching partials was not drawn");
        let colour = label_rect(&drawn, "Match colour with grayscale")
            .expect("the box for matching colour with grayscale was not drawn");
        // The ways of matching first, then the colour box, which changes how the
        // first of them decides rather than being a way of its own.
        assert!(
            whole.top() < crops.top() && crops.top() < colour.top(),
            "the boxes are in the wrong order: {whole:?}, {crops:?}, {colour:?}"
        );

        // The tick itself sits to the left of the words.
        let at = egui::pos2(crops.left() - 8.0, crops.center().y);
        frame(&mut app, Some(at), Some(true));
        frame(&mut app, Some(at), Some(false));
        assert!(!app.match_corners, "clicking the box did not switch matching crops off");
        assert!(app.match_whole_frame, "clicking one box switched the other off");
    }

    /// Click where the index box is drawn, and say whether it went on.
    fn ticks_the_index_box(app: &mut App, ctx: &egui::Context, screen: egui::Rect) -> bool {
        let drawn = crate::shot::frame(
            "ticks_the_index_box",
            ctx,
            egui::RawInput { screen_rect: Some(screen), ..Default::default() },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.folder_section(ui, 700.0);
                });
            },
        );
        let Some(label) = label_rect(&drawn, "Save an index database for this folder") else {
            return false;
        };
        let at = egui::pos2(label.left() - 8.0, label.center().y);
        for pressed in [true, false] {
            let mut input =
                egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            input.events.push(egui::Event::PointerMoved(at));
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.folder_section(ui, 700.0);
                });
            });
        }
        app.keep_index
    }

    /// Two of the boxes are about the box above them, and mean nothing without
    /// it. Both are off and out of reach while what they depend on is off, and
    /// keeping an index asks for a rescan on opening by itself.
    #[test]
    fn a_box_that_depends_on_another_is_off_and_out_of_reach_without_it() {
        let ctx = window();
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let draw = |app: &mut App| {
            crate::shot::frame(
                "a_box_that_depends_on_another",
                &ctx,
                egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        app.folder_section(ui, 700.0);
                    });
                },
            )
        };

        // Both are ticked, and both lose what they depend on.
        app.recurse = true;
        app.within_a_folder = true;
        app.keep_index = true;
        app.auto_rescan = true;
        app.recurse = false;
        app.keep_index = false;
        app.settle_the_boxes();
        assert!(!app.within_a_folder, "matching within folders survived the subfolders going");
        assert!(!app.auto_rescan, "running on opening survived the index going");

        // And with what they depend on off, they are drawn but cannot be
        // reached: clicking where they are changes nothing.
        let drawn = draw(&mut app);
        let apart = label_rect(&drawn, "Only match within folders").expect("no box for that");
        let opening =
            label_rect(&drawn, "Automatically rescan when opening this index").expect("no box");
        for box_at in [apart, opening] {
            let at = egui::pos2(box_at.left() - 8.0, box_at.center().y);
            let mut input =
                egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            input.events.push(egui::Event::PointerMoved(at));
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            });
            let _ = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.folder_section(ui, 700.0);
                });
            });
        }
        assert!(!app.within_a_folder, "a box nobody can reach was ticked");
        assert!(!app.auto_rescan, "a box nobody can reach was ticked");

        // And saying yes to keeping an index says yes to rescanning on opening,
        // which is what the box under it is for.
        assert!(ticks_the_index_box(&mut app, &ctx, screen), "the index box was not ticked");
        assert!(app.auto_rescan, "ticking the index box did not ask for a rescan");
    }

    /// Both ways of matching are on to begin with, and switching one off is a
    /// fact about the folder: the index keeps it and opening that folder again
    /// comes back with it.
    #[test]
    fn the_index_keeps_which_ways_of_matching_were_ticked() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        assert!(app.match_whole_frame, "whole pictures were not being matched to begin with");
        assert!(app.match_corners, "crops were not being matched to begin with");

        app.match_corners = false;
        app.remember_ways_of_matching();

        let again = reviewing(found.path());
        assert!(again.match_whole_frame, "whole pictures came back switched off");
        assert!(!again.match_corners, "the folder was opened again still matching crops");
    }

    /// The whole review page: the list keeps to its own side of the window,
    /// whatever the preview pane beside it is dragged to. Nothing in it is
    /// painted over the pane, and no set runs under the scroll bar.
    #[test]
    fn the_review_list_keeps_to_its_own_side_of_the_window() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        app.selected = app.sets[0].members.first().map(|member| member.file_id);

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
        // The window's own panels, in the order the window puts them: the tabs
        // across the top, then the page with no margin of its own, which is what
        // lets the toolbar's line run the width of the window.
        let draw = |app: &mut App| {
            crate::shot::frame(
                "the_review_list_keeps_to_its_own_side_of_the_window",
                &ctx,
                egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                |ctx| {
                    egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("1  Scan");
                        });
                        ui.add_space(6.0);
                    });
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::central_panel(&ctx.style())
                                .inner_margin(egui::Margin::ZERO),
                        )
                        .show(ctx, |ui| app.review_view(ui));
                },
            )
        };
        // The panel settles on its width over a frame or two.
        draw(&mut app);
        draw(&mut app);
        let drawn = draw(&mut app);

        let pane = app.preview_width.expect("the preview pane was not drawn");
        // The first set starts below the toolbar rather than against it, and the
        // list's scroll bar starts level with it rather than above or below it.
        let bar = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - SCROLL_BAR).abs() < 1.0
                        && rect.rect.height() > 100.0 =>
                {
                    Some(rect.rect.top())
                }
                _ => None,
            })
            .fold(f32::MAX, f32::min);
        // The first set starts below the toolbar rather than against it.
        let toolbar = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.rect.width() > screen.width() - 1.0 => {
                    Some(rect.rect.bottom())
                }
                _ => None,
            })
            .filter(|bottom| *bottom < screen.height() / 2.0)
            .fold(0.0_f32, f32::max);
        let first = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.width > 0.0
                        && rect.rect.width() > 200.0
                        && rect.rect.height() > 100.0 =>
                {
                    Some(rect.rect.top())
                }
                _ => None,
            })
            .fold(f32::MAX, f32::min);
        assert!(
            first - toolbar >= SECTION_GAP - 1.0,
            "the first set starts {} under the toolbar",
            first - toolbar
        );
        // Level with the first set, to within the line drawn round it: the bar
        // begins at the row, and the box's line is drawn just inside that.
        assert!(
            (bar - first).abs() <= BOX_EDGE + 0.01,
            "the scroll bar starts at {bar} and the first set at {first}"
        );
        let outlined: Vec<egui::Rect> = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => Some(rect.rect),
                _ => None,
            })
            .collect();
        let boxes: Vec<&egui::Rect> =
            outlined.iter().filter(|rect| rect.width() > 200.0 && rect.height() > 100.0).collect();
        assert!(!boxes.is_empty(), "no set was drawn");
        let pane_starts = screen.right() - pane;
        for set in boxes {
            assert!(
                set.right() < pane_starts,
                "a set reaches {} and the preview pane starts at {pane_starts}",
                set.right()
            );
        }

        // And whatever the list draws is cut off at the list's own edge rather
        // than painted over the pane beside it. The page behind everything is
        // clipped to the whole window, which is not the list.
        for clipped in &drawn {
            let clip = clipped.clip_rect;
            let of_the_list = clip != screen && clip.left() < pane_starts && clip.top() > 56.0;
            if of_the_list {
                assert!(
                    clip.right() <= pane_starts + 0.5,
                    "something in the list is clipped to {clip:?}, which reaches over the pane"
                );
            }
        }
    }

    /// Every page of the window, drawn into picture files for the manual.
    ///
    /// A real folder, really scanned and really searched, so what the manual
    /// shows is the window rather than a drawing of it. Ignored unless it is
    /// asked for by name; the pictures go where `IMGDEDUPE_SHOT_DIR` says, or to
    /// the temporary folder.
    #[test]
    #[ignore = "writes the pictures the manual is built from"]
    fn the_pictures_for_the_manual() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        app.keep_index = true;
        app.recurse = true;
        app.selected = app.sets[0].members.first().map(|member| member.file_id);

        let ctx = window();
        // The window as it ships, not the toolkit's own dark.
        ctx.set_visuals(egui::Visuals::light());
        // Wide enough for the whole of the scan page, which is three boxes in a
        // row and a run of lamps under them.
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1280.0, 720.0));
        let mut camera = crate::shot::Camera::default();
        let folder = crate::shot::folder();

        // Each page, drawn the way the window draws it: the tabs across the top
        // and the page under them. Several frames each, because the pictures and
        // the panel beside them arrive over a few.
        let mut page = |app: &mut App, name: &str, view: View| {
            app.view = view;
            for _ in 0..10 {
                let drawing = &mut *app;
                camera.shoot(
                    &ctx,
                    egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                    |ctx| {
                        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                for (label, on) in [
                                    ("1  Scan", drawing.view == View::Scan),
                                    ("2  Review", drawing.view == View::Review),
                                    ("3  Clean up", drawing.view == View::Cleanup),
                                ] {
                                    ui.add(egui::SelectableLabel::new(on, label));
                                }
                            });
                            ui.add_space(6.0);
                        });
                        let margin = match drawing.view {
                            View::Cleanup | View::Review => egui::Margin::ZERO,
                            _ => egui::Margin::symmetric(16.0, 12.0),
                        };
                        egui::CentralPanel::default()
                            .frame(
                                egui::Frame::central_panel(&ctx.style()).inner_margin(margin),
                            )
                            .show(ctx, |ui| match drawing.view {
                                View::Scan => drawing.scan_view(ui),
                                View::Review => drawing.review_view(ui),
                                View::Cleanup => drawing.cleanup_view(ui),
                            });
                    },
                    &folder.join(format!("manual-{name}.png")),
                );
                app.thumbs.collect(&ctx);
            }
        };

        // The scan page is the one page that writes the folder's path on itself,
        // in the folder row and in the first lamp. What is drawn there is a
        // folder anybody could have, not the temporary one this ran against and
        // not the name of whoever ran it. Nothing is pressed while it is set, so
        // nothing goes looking for it.
        let real = app.folder.clone();
        app.folder = Some(PathBuf::from("D:\\Photos\\2026"));
        page(&mut app, "scan", View::Scan);
        app.folder = real;

        page(&mut app, "review", View::Review);

        // The same review with a set nobody calls a set of copies.
        let second = app.sets[1].set_id;
        app.ignore_set(second);
        page(&mut app, "ignored", View::Review);
        app.unignore_set(second);

        page(&mut app, "cleanup", View::Cleanup);
        println!("the manual's pictures are in {}", folder.display());
    }

    /// Draw the review page into a picture file, so what it looks like can be
    /// looked at. Names the file it wrote.
    ///
    /// Not a check of anything: it is the window, on paper, for whoever is
    /// changing the layout. Ignored unless it is asked for by name.
    #[test]
    #[ignore = "writes a picture of the review page instead of checking anything"]
    fn a_picture_of_the_review_page() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        app.selected = app.sets[0].members.first().map(|member| member.file_id);
        // A set wider than the box, so its own scroll bar is in the picture, and
        // a set nobody calls a set of copies, so the faded look is in it too.
        let held: Vec<_> = app.sets[0].members.clone();
        app.sets[0].members = (0..12)
            .map(|at| {
                let mut member = held[at % held.len()].clone();
                member.file_id = 100 + at as i64;
                member
            })
            .collect();
        if app.sets.len() > 1 {
            let second = app.sets[1].clone();
            for pair in pairs_of(&second) {
                app.ignored.insert(pair);
            }
        }

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
        // The picture is for looking at, so it is drawn the way the window is
        // rather than in whatever the toolkit ships as its default.
        ctx.set_visuals(egui::Visuals::light());
        let mut camera = crate::shot::Camera::default();
        let at = std::env::var_os("IMGDEDUPE_SHOT")
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("review.png"));
        // The pictures arrive over a few frames, and the panel settles on its
        // width over one or two, so the frame worth looking at is not the first.
        for _ in 0..8 {
            let input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            let drawing = &mut app;
            camera.shoot(&ctx, input, |ctx| {
                egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("1  Scan");
                        ui.label("2  Review");
                    });
                    ui.add_space(6.0);
                });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::ZERO),
                    )
                    .show(ctx, |ui| drawing.review_view(ui));
            }, &at);
            app.thumbs.collect(&ctx);
        }
        println!("the review page is at {}", at.display());
    }

    /// A set's own scroll bar runs the width of the box, edge to edge inside the
    /// line round it, and the band of buttons under it has a line of its own
    /// along the top.
    #[test]
    fn a_sets_bar_runs_the_width_of_the_box_and_the_band_has_a_line_on_it() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        // More pictures than fit across the box, so the strip has a bar at all.
        app.sets[0].members = (100..124).map(|id| member(id, "a.jpg", 500)).collect();

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
        let draw = |app: &mut App| {
            crate::shot::frame(
                "a_sets_bar_runs_the_width_of_the_box_and_the_band_has_a_line_on_it",
                &ctx,
                egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::central_panel(&ctx.style())
                                .inner_margin(egui::Margin::ZERO),
                        )
                        .show(ctx, |ui| app.review_view(ui));
                },
            )
        };
        draw(&mut app);
        draw(&mut app);
        let drawn = draw(&mut app);

        // The first box: the topmost outlined rectangle as wide as a set.
        let (set, stroke) = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.width > 0.0
                        && rect.rect.width() > 200.0
                        && rect.rect.height() > 100.0 =>
                {
                    Some((rect.rect, rect.stroke.width))
                }
                _ => None,
            })
            .fold((egui::Rect::NOTHING, 0.0), |first: (egui::Rect, f32), it| {
                if it.0.top() < first.0.top() {
                    it
                } else {
                    first
                }
            });
        assert!(set.is_finite(), "no set was drawn");
        let inside = set.shrink(stroke / 2.0);

        // The strip's own bar: as tall as a bar, lying down, inside this box.
        let bar = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.height() - SCROLL_BAR).abs() < 1.0
                        && rect.rect.width() > 100.0
                        && inside.contains(rect.rect.center()) =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .fold(egui::Rect::NOTHING, |widest, rect| {
                if rect.width() > widest.width() {
                    rect
                } else {
                    widest
                }
            });
        assert!(bar.is_finite(), "the set's own scroll bar was not drawn");
        assert!(
            (bar.left() - inside.left()).abs() < 0.51 && (bar.right() - inside.right()).abs() < 0.51,
            "the bar runs {:?} inside a box that runs {:?}",
            bar.x_range(),
            inside.x_range()
        );

        // And the line along the top of the band under it.
        let lines: Vec<(egui::Pos2, egui::Pos2)> = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::LineSegment { points, stroke }
                    if stroke.color == egui::epaint::ColorMode::Solid(BUTTON_ROW_EDGE) =>
                {
                    Some((points[0], points[1]))
                }
                _ => false.then_some((egui::Pos2::ZERO, egui::Pos2::ZERO)),
            })
            .collect();
        // Corner to corner of the box, which is the rectangle the line round it
        // was given rather than that rectangle less half of the line.
        let band_line = lines.iter().any(|(from, to)| {
            (to.x - from.x - set.width()).abs() < 0.51
                && from.y >= bar.bottom() - 0.01
                && from.y < inside.bottom()
        });
        assert!(
            band_line,
            "the band of buttons has no line along the top of it: lines {lines:?}, \
             box {inside:?}, bar bottom {}",
            bar.bottom()
        );
    }

    /// The bar beside the list marks the room the list is drawn in, so it is in
    /// the same place whatever the list is scrolled to. Only the handle inside it
    /// moves.
    #[test]
    fn the_scroll_bar_beside_the_list_stays_where_it_is_when_the_list_moves() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
        let draw = |app: &mut App| {
            crate::shot::frame(
                "the_scroll_bar_beside_the_list_stays_where_it_is_when_the_list_moves",
                &ctx,
                egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::central_panel(&ctx.style())
                                .inner_margin(egui::Margin::ZERO),
                        )
                        .show(ctx, |ui| app.review_view(ui));
                },
            )
        };
        // The track: as wide as a bar and as tall as the list. The handle inside
        // it is the same width and shorter, so the tallest of them is the track.
        let track = |drawn: &[egui::epaint::ClippedShape]| -> egui::Rect {
            drawn
                .iter()
                .filter_map(|clipped| match &clipped.shape {
                    egui::Shape::Rect(rect)
                        if (rect.rect.width() - SCROLL_BAR).abs() < 1.0
                            && rect.rect.height() > 100.0 =>
                    {
                        Some(rect.rect)
                    }
                    _ => None,
                })
                .fold(egui::Rect::NOTHING, |tallest, rect| {
                    if rect.height() > tallest.height() {
                        rect
                    } else {
                        tallest
                    }
                })
        };

        draw(&mut app);
        let before = track(&draw(&mut app));
        assert!(before.is_finite(), "the list's scroll bar was not drawn");
        let was = app.list_offset;

        // Down to the second set, the way the cursor keys take it there.
        app.scroll_to = Some(1);
        draw(&mut app);
        let after = track(&draw(&mut app));
        assert!(app.list_offset > was, "the list did not move, so this measures nothing");
        assert_eq!(before, after, "the bar moved with the list it is beside");
    }

    /// Every set is drawn whole: the line round it is inside what the list is
    /// allowed to paint in, top and bottom, so no box is sliced by the edge of
    /// the list. One box stands as far from the next as `BETWEEN_BOXES`, the gap
    /// from the window's edge to a box's left edge is the gap from its right
    /// edge to the scroll bar, and what is inside a box keeps the same room on
    /// either side.
    #[test]
    fn the_set_boxes_are_drawn_whole_and_evenly_spaced() {
        let found = folder_with_two_sets();
        let mut app = reviewing(found.path());
        assert!(app.sets.len() >= 2, "two sets were needed and {} were found", app.sets.len());

        let ctx = window();
        // Short enough that the list has more in it than fits, so the bar beside
        // it is drawn and can be measured against the boxes.
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
        let draw = |app: &mut App| {
            crate::shot::frame(
                "the_set_boxes_are_drawn_whole_and_evenly_spaced",
                &ctx,
                egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                |ctx| {
                    egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("1  Scan");
                        });
                        ui.add_space(6.0);
                    });
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::central_panel(&ctx.style())
                                .inner_margin(egui::Margin::ZERO),
                        )
                        .show(ctx, |ui| app.review_view(ui));
                },
            )
        };
        draw(&mut app);
        draw(&mut app);
        let drawn = draw(&mut app);

        // The boxes: the only outlined rectangles as wide as a set and as tall.
        let mut boxes: Vec<(egui::Rect, egui::Rect, f32)> = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.stroke.width > 0.0
                        && rect.rect.width() > 200.0
                        && rect.rect.height() > 100.0 =>
                {
                    Some((rect.rect, clipped.clip_rect, rect.stroke.width))
                }
                _ => None,
            })
            .collect();
        boxes.sort_by(|left, right| left.0.top().total_cmp(&right.0.top()));
        assert!(boxes.len() >= 2, "{} sets were drawn, not two", boxes.len());

        // Whole, not sliced: the line is drawn half on either side of the
        // rectangle, and all of it has to be inside what the list may paint in.
        for (set, clip, stroke) in &boxes {
            let line = set.expand(stroke / 2.0);
            // The bottom-most box runs off the end of the window, which is what
            // a list does; none of them is cut off at the top of it.
            assert!(
                clip.top() <= line.top() + 0.01,
                "a set drawn at {set:?} has its top cut off by the clip at {clip:?}"
            );
            assert!(
                clip.left() <= line.left() + 0.01 && clip.right() >= line.right() - 0.01,
                "a set drawn at {set:?} is cut off sideways by the clip at {clip:?}"
            );
        }

        // Spaced, not stacked.
        let gap = boxes[1].0.top() - boxes[0].0.bottom();
        assert!(
            gap >= BETWEEN_BOXES - 0.01,
            "one set ends at {} and the next starts at {}, {gap} apart",
            boxes[0].0.bottom(),
            boxes[1].0.top()
        );

        // The same room on either side of a box: the window's margin on the
        // left, and the same again between the box and the bar beside the list.
        let bar = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - SCROLL_BAR).abs() < 1.0
                        && rect.rect.height() > 100.0 =>
                {
                    Some(rect.rect.left())
                }
                _ => None,
            })
            .fold(f32::MAX, f32::min);
        assert!(bar < screen.right(), "the list's scroll bar was not drawn");
        let (first, _, stroke) = boxes[0];
        let left = first.left() - stroke / 2.0;
        let right = bar - (first.right() + stroke / 2.0);
        assert!(
            (left - right).abs() < 0.51,
            "a set has {left} to the left of it and {right} to the right of it"
        );

        // And inside a box, the pictures keep the same room on either side: what
        // the strip is clipped to sits `BOX_PADDING` inside the box at both
        // edges, so the first picture starts as far in as the last one ends.
        let strip = drawn
            .iter()
            .map(|clipped| clipped.clip_rect)
            .filter(|clip| {
                clip.top() > first.top()
                    && clip.bottom() < first.bottom()
                    && clip.width() > 100.0
                    && clip.right() < bar
            })
            .fold(egui::Rect::NOTHING, |widest, clip| {
                if clip.width() > widest.width() {
                    clip
                } else {
                    widest
                }
            });
        assert!(strip.is_finite(), "the pictures in the first set were not drawn");
        let inside = first.shrink(stroke / 2.0);
        let (before, after) = (strip.left() - inside.left(), inside.right() - strip.right());
        assert!(
            (before - after).abs() < 0.51,
            "the pictures start {before} inside the box and end {after} inside it"
        );
    }

    /// A set nobody calls a set of copies is barely drawn: the pictures and every
    /// line of writing under them at `IGNORED_OPACITY`. The buttons are not,
    /// because they are how it stops being ignored.
    #[test]
    fn an_ignored_set_is_drawn_faded_and_its_buttons_are_not() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let set_id = app.sets[0].set_id;

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
        let draw = |app: &mut App| {
            crate::shot::frame(
                "an_ignored_set_is_drawn_faded_and_its_buttons_are_not",
                &ctx,
                egui::RawInput { screen_rect: Some(screen), ..Default::default() },
                |ctx| {
                    egui::CentralPanel::default()
                        .frame(
                            egui::Frame::central_panel(&ctx.style())
                                .inner_margin(egui::Margin::ZERO),
                        )
                        .show(ctx, |ui| app.review_view(ui));
                },
            )
        };
        // What one line of writing in the list is drawn in. The file names under
        // the pictures are the strip; "keep all" is the row of buttons. Only what
        // is in the list: the pane beside it names the same file and is not part
        // of any set.
        let alpha_of = |drawn: &[egui::epaint::ClippedShape],
                        words: &str,
                        list_ends: f32|
         -> Option<u8> {
            drawn.iter().find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.pos.x < list_ends && text.galley.text().contains(words) =>
                {
                    Some(text.fallback_color.a())
                }
                _ => None,
            })
        };

        draw(&mut app);
        let plain = draw(&mut app);
        let list_ends = screen.right() - app.preview_width.expect("the preview pane was not drawn");
        let alpha_of = |drawn: &[egui::epaint::ClippedShape], words: &str| -> Option<u8> {
            alpha_of(drawn, words, list_ends)
        };
        let strip = alpha_of(&plain, "one.png").expect("no file name was drawn under a picture");
        let buttons = alpha_of(&plain, "keep all").expect("no buttons were drawn under the set");

        app.ignore_set(set_id);
        assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");
        draw(&mut app);
        let faded = draw(&mut app);

        let now = alpha_of(&faded, "one.png").expect("the file name went when the set was ignored");
        let wanted = f32::from(strip) * IGNORED_OPACITY;
        assert!(
            (f32::from(now) - wanted).abs() <= 2.0,
            "the writing under the pictures went from {strip} to {now}, and not to {wanted}"
        );
        assert_eq!(
            alpha_of(&faded, "keep all"),
            Some(buttons),
            "the buttons under an ignored set were faded with the rest of it"
        );
    }

    /// Ignoring a set says none of its pictures are copies of each other. The
    /// set stays on the review page, nothing in it is kept or dropped, the
    /// folder's index remembers it, and a review holding nothing else has
    /// nowhere for a cleanup to go.
    #[test]
    fn an_ignored_set_is_shown_and_left_alone() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        assert_eq!(app.sets.len(), 1, "the copies were not found");
        let set_id = app.sets[0].set_id;
        assert!(!app.is_ignored(&app.sets[0]), "a set was ignored before anybody said so");
        let (going, _) = app.selected_for_removal();
        assert!(going > 0, "nothing was going to be removed to begin with");

        app.ignore_set(set_id);

        assert_eq!(app.sets.len(), 1, "the set went instead of being left alone");
        assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");
        assert_eq!(app.selected_for_removal().0, 0, "an ignored set still had something going");
        assert!(app.build_plan().files() == 0, "the cleanup still had something to do");
        // What it kept is still written down, and counts for nothing while it is
        // ignored. That is what taking it back gives back.
        assert!(app.keep.contains_key(&set_id), "an ignored set forgot what it had kept");

        // Written down, so the next run knows it too.
        let mut again = App::from_settings(crate::settings::Settings::default());
        again.open_folder(found.path().to_path_buf());
        settle(&mut again);
        again.load_sets();
        settle(&mut again);
        assert_eq!(again.sets.len(), 1, "the set was not found again");
        assert!(again.is_ignored(&again.sets[0]), "the folder forgot that the set was ignored");
    }

    /// Ignoring is one press, and so is taking it back: the pairs go from the
    /// index, the set is a set again, and what it keeps works as it did.
    #[test]
    fn a_set_can_be_unignored_again() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let set_id = app.sets[0].set_id;

        app.ignore_set(set_id);
        assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");
        app.unignore_set(set_id);
        assert!(!app.is_ignored(&app.sets[0]), "the set is still ignored");
        assert!(app.selected_for_removal().0 > 0, "the set is not being cleaned up again");

        // And the index no longer holds any of it.
        let conn = db::open_read_only(app.db_path.as_ref().expect("a db")).expect("open");
        assert!(db::ignored(&conn).expect("read").is_empty(), "the index still holds the pairs");
        let _ = conn.close();

        // Opened again from nothing, the set is a set.
        let mut again = App::from_settings(crate::settings::Settings::default());
        again.open_folder(found.path().to_path_buf());
        settle(&mut again);
        again.load_sets();
        settle(&mut again);
        assert!(!again.is_ignored(&again.sets[0]), "the folder came back with it still ignored");
    }

    /// What the real folder's index actually holds about itself: every setting
    /// the window claims to keep in it, and whether it is there.
    ///
    /// Read-only, and prints rather than asserts, because the point is the list.
    #[test]
    #[ignore = "reads the index of the folder the application is set to"]
    fn what_the_real_folders_index_says_about_itself() {
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = headless::default_db_path(&folder);
        assert!(db_path.is_file(), "{} has no index", folder.display());
        let conn = db::open_read_only(&db_path).expect("open the real index");

        let mut tables = conn
            .prepare("SELECT name FROM sqlite_master WHERE type IN ('table','view') ORDER BY name")
            .unwrap();
        let names: Vec<String> =
            tables.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
        drop(tables);
        println!("tables and views: {names:?}");

        use crate::notes::{
            AUTO_RESCAN, DISPOSAL, MATCH_CORNERS, MATCH_WHOLE_FRAME, MOVE_DIR, MULTI_SELECT,
            RECURSE, WITHIN_A_FOLDER,
        };
        for key in [
            RECURSE,
            DISPOSAL,
            MOVE_DIR,
            MULTI_SELECT,
            MATCH_WHOLE_FRAME,
            MATCH_CORNERS,
            WITHIN_A_FOLDER,
            AUTO_RESCAN,
        ] {
            let held = db::get_meta(&conn, key).expect("read");
            println!("{key}: {held:?}");
        }
        let _ = conn.close();
    }

    /// With "only match within folders" on, a search of the real folder puts no
    /// set together out of two folders. Read-only: it searches the index that is
    /// there and writes nothing.
    #[test]
    #[ignore = "searches the index of the folder the application is set to"]
    fn only_matching_within_folders_holds_on_the_real_folder() {
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = headless::default_db_path(&folder);
        assert!(db_path.is_file(), "{} has no index to search", folder.display());

        let never = std::sync::atomic::AtomicBool::new(false);
        let images = {
            let conn = db::open_snapshot(&db_path).expect("read the real index");
            let held = matching::load_images(&conn, &never, &|_| {}).expect("load").expect("images");
            let _ = conn.close();
            std::sync::Arc::new(held)
        };
        println!("{} pictures in the index", images.len());

        let mut thresholds = Thresholds::at(matching::DEFAULT_SENSITIVITY);
        thresholds.within_a_folder = true;
        let started = std::time::Instant::now();
        let sets = matching::find_sets_in(&images, thresholds, &never, &|_| {})
            .expect("search")
            .expect("not cancelled");
        println!("{} sets, folders kept apart, in {:.1}s", sets.len(), started.elapsed().as_secs_f64());

        let folder_of = |path: &str| match path.rfind('/') {
            Some(at) => path[..at].to_string(),
            None => String::new(),
        };
        for set in &sets {
            let mut folders: Vec<String> =
                set.members.iter().map(|member| folder_of(&member.rel_path)).collect();
            folders.sort();
            folders.dedup();
            assert_eq!(
                folders.len(),
                1,
                "a set was made out of {} folders: {:?}",
                folders.len(),
                set.members.iter().map(|member| member.rel_path.as_str()).collect::<Vec<_>>()
            );
        }
        assert!(!sets.is_empty(), "the search found nothing, so this measured nothing");
    }

    /// Every box the window keeps in the folder's index goes into the real
    /// folder's index and comes back out of it, through the window's own writing
    /// and reading rather than through hand-written SQL.
    ///
    /// The index is left exactly as it was found.
    #[test]
    #[ignore = "writes to the index of the folder the application is set to"]
    fn every_box_the_window_keeps_survives_the_real_folders_index() {
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = headless::default_db_path(&folder);
        assert!(db_path.is_file(), "{} has no index to test against", folder.display());
        use crate::notes::mark;
        let before = crate::notes::of_folder(&db_path).expect("read what the folder says");
        println!("before: {before:?}");

        let mut app = App::from_settings(crate::settings::Settings::default());
        app.folder = Some(folder.clone());
        app.db_path = Some(db_path.clone());
        // Every one of them away from its default, so a box that is not written
        // at all cannot pass by looking like the default.
        app.recurse = true;
        app.keep_index = true;
        app.within_a_folder = true;
        app.auto_rescan = true;
        app.match_whole_frame = false;
        app.match_corners = false;
        app.multi_select = true;
        app.destination = Destination::Delete;
        app.move_dir = String::from("somewhere else");
        app.remember_ways_of_matching();
        app.remember_multi_select();
        app.remember_disposal();

        let said = crate::notes::of_folder(&db_path).expect("read what the folder says");

        // Put the folder back the way it was before anything is asserted.
        let restore = db::open_for_notes(&db_path).expect("open the real index to write to");
        for (key, was) in [
            (crate::notes::WITHIN_A_FOLDER, before.within_a_folder.map(|it| mark(it).to_string())),
            (crate::notes::AUTO_RESCAN, before.auto_rescan.map(|it| mark(it).to_string())),
            (
                crate::notes::MATCH_WHOLE_FRAME,
                before.match_whole_frame.map(|it| mark(it).to_string()),
            ),
            (crate::notes::MATCH_CORNERS, before.match_corners.map(|it| mark(it).to_string())),
            (crate::notes::MULTI_SELECT, before.multi_select.map(|it| mark(it).to_string())),
            (crate::notes::DISPOSAL, before.disposal.clone()),
            (crate::notes::MOVE_DIR, before.move_dir.clone()),
        ] {
            match was {
                Some(value) => {
                    let _ = db::set_meta(&restore, key, &value);
                }
                None => {
                    let _ = restore.execute("DELETE FROM meta WHERE key = ?1", [key]);
                }
            }
        }
        drop(restore);

        assert_eq!(said.within_a_folder, Some(true), "only match within folders was not kept");
        assert_eq!(said.auto_rescan, Some(true), "rescan on opening was not kept");
        assert_eq!(said.match_whole_frame, Some(false), "matching whole frames was not kept");
        assert_eq!(said.match_corners, Some(false), "matching corners was not kept");
        assert_eq!(said.multi_select, Some(true), "allow multi-select was not kept");
        assert_eq!(said.disposal.as_deref(), Some("delete"), "the destination was not kept");
        assert_eq!(
            said.move_dir.as_deref(),
            Some("somewhere else"),
            "the folder to move to was not kept"
        );
    }

    /// Every setting the window keeps in the real folder's index survives that
    /// index being written out, which is what a pass does at the end of a run.
    ///
    /// This is what was wrong: the window writes a setting into the file, the
    /// pass writes the copy it has been holding over the top, and the setting is
    /// gone. `recurse` was the only one that ever came back, because the pass
    /// writes that one itself.
    ///
    /// The index is left exactly as it was found.
    #[test]
    #[ignore = "writes to the index of the folder the application is set to"]
    fn the_real_folders_index_keeps_every_setting_the_window_writes() {
        use crate::notes::{
            AUTO_RESCAN, DISPOSAL, MATCH_CORNERS, MATCH_WHOLE_FRAME, MOVE_DIR, MULTI_SELECT,
            WITHIN_A_FOLDER,
        };
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = headless::default_db_path(&folder);
        assert!(db_path.is_file(), "{} has no index to test against", folder.display());

        let keys = [
            DISPOSAL,
            MOVE_DIR,
            MULTI_SELECT,
            MATCH_WHOLE_FRAME,
            MATCH_CORNERS,
            WITHIN_A_FOLDER,
            AUTO_RESCAN,
        ];
        // Values nothing else would write, so what comes back is what this wrote.
        let mine = |key: &str| format!("written by the test: {key}");

        let before: Vec<(String, Option<String>)> = {
            let conn = db::open_read_only(&db_path).expect("open the real index");
            let held = keys
                .iter()
                .map(|key| ((*key).to_string(), db::get_meta(&conn, key).expect("read")))
                .collect();
            let _ = conn.close();
            held
        };
        println!("before: {before:?}");

        // A pass, holding the index in memory from before any of this.
        let held = db::open(&db_path).expect("open the real index for writing");
        // The window, writing every one of them while that pass runs.
        let window = db::open_for_notes(&db_path).expect("open the real index to write to");
        for key in keys {
            db::set_meta(&window, key, &mine(key)).expect("write the setting");
        }
        drop(window);
        // The pass finishing.
        db::close(held, &db_path).expect("write the real index out");

        let after: Vec<(String, Option<String>)> = {
            let conn = db::open_read_only(&db_path).expect("open the real index");
            let held = keys
                .iter()
                .map(|key| ((*key).to_string(), db::get_meta(&conn, key).expect("read")))
                .collect();
            let _ = conn.close();
            held
        };

        // Put the folder back the way it was, whatever the answer is.
        let restore = db::open_for_notes(&db_path).expect("open the real index to write to");
        for (key, was) in &before {
            match was {
                Some(value) => {
                    let _ = db::set_meta(&restore, key, value);
                }
                None => {
                    let _ = restore.execute("DELETE FROM meta WHERE key = ?1", [key]);
                }
            }
        }
        drop(restore);

        for (key, now) in &after {
            assert_eq!(
                now.as_deref(),
                Some(mine(key).as_str()),
                "the pass wrote over {key}, which the window had just set"
            );
        }

        // And what the pass owns is still the pass's: how far it reached is a
        // fact about the index it just built, not something the window keeps.
        let conn = db::open_read_only(&db_path).expect("open the real index");
        assert!(
            db::get_meta(&conn, "recurse").expect("read").is_some(),
            "the pass lost how far it had reached"
        );
        let _ = conn.close();
    }

    /// The pairs marked in the real folder's index survive that index being
    /// written out, which is what a pass does at the end of every run.
    ///
    /// Run against the folder the application is set to, because that is where
    /// this went wrong: the index there is on a network mount, is megabytes, and
    /// is read into memory and written back whole. A generated folder on this
    /// disk passed this while the real one lost every pair.
    ///
    /// The index is left exactly as it was found.
    #[test]
    #[ignore = "writes to the index of the folder the application is set to"]
    fn the_real_folders_index_keeps_what_was_marked_when_it_is_written_out() {
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = headless::default_db_path(&folder);
        assert!(db_path.is_file(), "{} has no index to test against", folder.display());

        let before = {
            let conn = db::open_read_only(&db_path).expect("open the real index");
            let pairs = db::ignored(&conn).expect("read what is marked");
            let _ = conn.close();
            pairs
        };
        println!("{} holds {} marked pairs", db_path.display(), before.len());

        // Two pictures out of the real index, and a pair that is not already
        // marked, so what this writes is this test's own.
        let ids: Vec<i64> = {
            let conn = db::open_read_only(&db_path).expect("open the real index");
            let mut statement = conn.prepare("SELECT id FROM files ORDER BY id LIMIT 2").unwrap();
            let rows: Vec<i64> =
                statement.query_map([], |row| row.get(0)).unwrap().map(Result::unwrap).collect();
            drop(statement);
            let _ = conn.close();
            rows
        };
        assert_eq!(ids.len(), 2, "the real index holds fewer than two pictures");
        let mine = db::pair(ids[0], ids[1]);
        assert!(!before.contains(&mine), "the pair this test uses is already marked");

        // A pass, holding the index in memory from before the button is pressed.
        let held = db::open(&db_path).expect("open the real index for writing");
        // The window, marking a set while that pass runs.
        let window = db::open_for_notes(&db_path).expect("open the real index to write to");
        db::ignore(&window, &[mine]).expect("mark the pair");
        drop(window);
        // The pass finishing.
        db::close(held, &db_path).expect("write the real index out");

        let after = {
            let conn = db::open_read_only(&db_path).expect("open the real index");
            let pairs = db::ignored(&conn).expect("read what is marked");
            let _ = conn.close();
            pairs
        };

        // Put it back the way it was, whatever the answer is.
        let restore = db::open_for_notes(&db_path).expect("open the real index to write to");
        let _ = db::unignore(&restore, &[mine]);
        drop(restore);

        assert!(after.contains(&mine), "the pass wrote over what was marked while it ran");
        for was in &before {
            assert!(after.contains(was), "the pass lost a pair that was already marked: {was:?}");
        }
    }

    /// A window opened on the folder it was left on comes up knowing which of its
    /// sets are not sets of copies. That folder is never chosen: it is set before
    /// the first frame, so whatever reads a folder's index has to read the
    /// ignored pairs too, or a set ignored last time comes back as a set.
    #[test]
    fn a_folder_the_window_opens_on_comes_up_with_its_ignored_sets_ignored() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        assert_eq!(app.sets.len(), 1, "the copies were not found");
        app.ignore_set(app.sets[0].set_id);
        assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");

        // The next run of the window, opened on the folder it was left on rather
        // than on one somebody picked.
        let saved = crate::settings::Settings {
            folder: Some(found.path().to_path_buf()),
            ..crate::settings::Settings::default()
        };
        let mut again = App::from_settings(saved);
        again.open_what_was_left_open();
        settle(&mut again);

        assert!(!again.ignored.is_empty(), "the folder came up knowing nothing was ignored");
        again.load_sets();
        settle(&mut again);
        assert_eq!(again.sets.len(), 1, "the set was not found again");
        assert!(again.is_ignored(&again.sets[0]), "the set came back as a set of copies");
    }

    /// Taking a set back gives back the picture it was keeping. Ignoring it does
    /// not throw the mark away: while a set is ignored the mark means nothing and
    /// the cleanup passes over it, and the moment it is a set again the mark is
    /// where it was left.
    #[test]
    fn unignoring_a_set_gives_back_the_picture_it_was_keeping() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![DuplicateSet {
            set_id: 1,
            members: vec![member(1, "a.jpg", 300), member(2, "b.jpg", 200)],
        }];
        app.selected = Some(2);
        app.keep_selected();
        assert_eq!(app.keep.get(&1), Some(&Keep::One(2)), "the picture was not kept");

        app.ignore_set(1);
        assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");
        assert_eq!(app.selected_for_removal().0, 0, "an ignored set still had something going");
        // And nothing in it can be marked or unmarked while it is ignored.
        app.selected = Some(1);
        app.keep_selected();

        app.unignore_set(1);
        assert_eq!(
            app.keep.get(&1),
            Some(&Keep::One(2)),
            "the set came back keeping something other than what it had kept"
        );
        assert_eq!(app.selected_for_removal().0, 1, "the set is not being cleaned up again");
    }

    /// Ignoring the set the preview is in leaves the preview where it was, so the
    /// cursor keys have somewhere to walk from: left and up to the set before it,
    /// right and down to the set after it.
    #[test]
    fn ignoring_the_set_the_preview_is_in_leaves_the_keys_somewhere_to_go() {
        let sets = || {
            vec![
                DuplicateSet {
                    set_id: 1,
                    members: vec![member(1, "a.jpg", 10), member(2, "b.jpg", 10)],
                },
                DuplicateSet {
                    set_id: 2,
                    members: vec![member(3, "c.jpg", 10), member(4, "d.jpg", 10)],
                },
                DuplicateSet {
                    set_id: 3,
                    members: vec![member(5, "e.jpg", 10), member(6, "f.jpg", 10)],
                },
            ]
        };
        let visible: Vec<usize> = (0..3).collect();

        // Left and right cross into the set at the picture nearest to where they
        // left; up and down keep their place in the set they land in.
        for (direction, wanted, what) in [
            (Direction::Back, 2, "left"),
            (Direction::PreviousSet, 2, "up"),
            (Direction::Forward, 5, "right"),
            (Direction::NextSet, 6, "down"),
        ] {
            let mut app = App::from_settings(crate::settings::Settings::default());
            app.sets = sets();
            // The preview is on the second picture of the middle set, and that
            // set is the one being ignored.
            app.selected = Some(4);
            app.ignore_set(2);
            assert!(app.is_ignored(&app.sets[1]), "the set was not ignored");
            assert_eq!(app.selected, Some(4), "{what}: ignoring took the preview away");

            app.walk(&visible, direction);
            assert_eq!(app.selected, Some(wanted), "{what} did not leave the ignored set");
        }
    }

    /// The cursor keys step over a set nobody calls a set of copies: on to the
    /// next set that is one, and nowhere at all when there is none.
    #[test]
    fn the_cursor_keys_step_over_ignored_sets() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![
            DuplicateSet { set_id: 1, members: vec![member(1, "a.jpg", 10), member(2, "b.jpg", 10)] },
            DuplicateSet { set_id: 2, members: vec![member(3, "c.jpg", 10), member(4, "d.jpg", 10)] },
            DuplicateSet { set_id: 3, members: vec![member(5, "e.jpg", 10), member(6, "f.jpg", 10)] },
        ];
        // The middle set is not a set of copies.
        app.ignored.insert(db::pair(3, 4));
        let visible: Vec<usize> = (0..app.sets.len()).collect();

        // Walking forward off the end of the first set lands in the third.
        app.selected = Some(2);
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, Some(5), "forward did not step over the ignored set");

        // And back again the same way.
        app.walk(&visible, Direction::Back);
        assert_eq!(app.selected, Some(2), "back did not step over the ignored set");

        // A set at a time, the same.
        app.selected = Some(1);
        app.walk(&visible, Direction::NextSet);
        assert_eq!(app.selected, Some(5), "the next set was the ignored one");

        // With nothing but ignored sets beyond it, the keys do nothing.
        app.ignored.insert(db::pair(5, 6));
        app.selected = Some(2);
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, Some(2), "the keys moved into an ignored set");
        app.walk(&visible, Direction::NextSet);
        assert_eq!(app.selected, Some(2), "the keys moved into an ignored set");
    }

    /// Ignoring one pair of a larger set is not ignoring the set: the rest of
    /// them are still copies of each other.
    #[test]
    fn a_set_is_only_ignored_when_every_pair_in_it_is() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![DuplicateSet {
            set_id: 1,
            members: vec![member(1, "a.jpg", 100), member(2, "b.jpg", 100), member(3, "c.jpg", 100)],
        }];
        app.ignored.insert(db::pair(1, 2));
        assert!(!app.is_ignored(&app.sets[0]), "one pair of three was enough to ignore the set");

        app.ignored.insert(db::pair(1, 3));
        app.ignored.insert(db::pair(2, 3));
        assert!(app.is_ignored(&app.sets[0]), "every pair was ignored and the set was not");
    }

    /// The numbers beside the steps are the run's own clock: how long since the
    /// Scan button was pressed, not how long the window has been open. A window
    /// left open for an hour and then told to scan reports milliseconds, not an
    /// hour.
    #[test]
    fn the_times_beside_the_steps_are_measured_from_the_scan_button() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        // As if the window had been sitting there for a while before the press.
        app.started = std::time::Instant::now() - std::time::Duration::from_secs(3600);

        app.start_scan();
        settle(&mut app);

        let lit: Vec<(Lamp, u128)> = LAMPS
            .iter()
            .filter_map(|(lamp, _)| self_lit(&app, *lamp).map(|at| (*lamp, at)))
            .collect();
        assert!(!lit.is_empty(), "the pass lit nothing");
        for (lamp, at) in lit {
            assert!(at < 60_000, "{lamp:?} says {at} ms, which is the window's clock");
        }
    }

    fn self_lit(app: &App, lamp: Lamp) -> Option<u128> {
        app.lit.get(&lamp).copied()
    }

    /// A step nothing was going to do is not a step that failed. A pass over a
    /// folder where nothing has changed reads no files and indexes none, and
    /// those four steps end as passed over rather than as still to happen.
    #[test]
    fn the_steps_a_pass_had_nothing_to_do_are_marked_as_passed_over() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());

        // Every file was read the first time round, so all four turned.
        for lamp in [
            Lamp::StartedReadingNewFiles,
            Lamp::FinishedReadingNewFiles,
            Lamp::StartedIndexingNewFiles,
            Lamp::FinishedIndexingNewFiles,
        ] {
            assert_eq!(app.how_it_went(lamp), Went::Happened, "{lamp:?} did not happen");
        }

        // A second pass over the same folder has nothing to read or index.
        app.start_scan();
        settle(&mut app);
        assert!(app.scan.unchanged > 0, "the second pass read the folder again");
        for lamp in [
            Lamp::StartedReadingNewFiles,
            Lamp::FinishedReadingNewFiles,
            Lamp::StartedIndexingNewFiles,
            Lamp::FinishedIndexingNewFiles,
        ] {
            assert_eq!(app.how_it_went(lamp), Went::Skipped, "{lamp:?} was not passed over");
        }
        // And the ones that did happen still say so.
        assert_eq!(app.how_it_went(Lamp::CheckedForIndexFile), Went::Happened);
    }

    /// Opening a folder that has been scanned before reads its index into
    /// memory whatever the rescan box says. That is what the index is for: the
    /// pictures are known, so Find duplicates costs the comparing and nothing
    /// else. Only the pass over the files is what the box decides.
    #[test]
    fn opening_a_folder_reads_its_index_without_scanning_it() {
        let found = folder_with_a_duplicate();
        let app = reviewing(found.path());
        assert!(!app.auto_rescan, "the folder asked to be rescanned on its own");

        let mut again = App::from_settings(crate::settings::Settings::default());
        assert!(again.images.is_none(), "a window with no folder open holds pictures");
        again.open_folder(found.path().to_path_buf());
        settle(&mut again);

        assert!(again.running.is_none(), "the folder was scanned although the box is off");
        let held = again.images.as_ref().expect("the index was not read into memory");
        assert_eq!(held.len(), 3, "the index came back holding {} pictures", held.len());
        assert!(
            again.lit.contains_key(&Lamp::LoadedIndexIntoMemory),
            "nothing said the index had been read"
        );

        // And it can be searched straight away, without a pass.
        again.load_sets();
        settle(&mut again);
        assert_eq!(again.sets.len(), 1, "the copies were not found from the index alone");
    }

    /// The other two boxes are kept with the folder as well: whether it is
    /// searched one folder at a time, and whether opening it runs a pass.
    #[test]
    fn the_index_keeps_matching_within_folders_and_running_on_opening() {
        let found = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(found.path().to_path_buf());
        assert!(!app.within_a_folder, "folders were being kept apart to begin with");
        assert!(!app.auto_rescan, "opening the folder was going to run a pass to begin with");

        // Matching within folders needs the folder to be scanned with its
        // subfolders, which is a fact the index holds, the pass writes, and a
        // later pass over the same index does not argue with. So it is set
        // before the folder has an index at all.
        app.recurse = true;
        app.start_scan();
        settle(&mut app);
        app.within_a_folder = true;
        app.auto_rescan = true;
        app.remember_ways_of_matching();
        let written = crate::notes::of_folder(app.db_path.as_ref().expect("a db")).expect("notes");
        assert_eq!(
            (written.recurse, written.within_a_folder, written.auto_rescan),
            (Some(true), Some(true), Some(true)),
            "the index did not come out of that holding all three"
        );

        let mut again = App::from_settings(crate::settings::Settings::default());
        again.open_folder(found.path().to_path_buf());
        settle(&mut again);
        assert!(again.within_a_folder, "the folder came back without folders kept apart");
        assert!(again.auto_rescan, "the folder came back without running on opening");
    }

    /// With the box unticked the mark moves: keeping one picture lets go of the
    /// one that was kept before it.
    #[test]
    fn without_multi_selected_marking_a_picture_lets_the_last_one_go() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![DuplicateSet {
            set_id: 1,
            members: vec![
                member(1, "a.jpg", 500),
                member(2, "b.jpg", 400),
                member(3, "c.jpg", 300),
            ],
        }];
        let set_id = app.sets[0].set_id;

        app.selected = Some(1);
        app.keep_selected();
        app.selected = Some(2);
        app.keep_selected();

        assert_eq!(app.keep.get(&set_id), Some(&Keep::One(2)), "the mark did not move");
    }

    /// With the box ticked the marks add up, and taking them off again one at a
    /// time can leave the set keeping nothing at all.
    #[test]
    fn with_multi_selected_marks_add_up_and_come_off_one_at_a_time() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![DuplicateSet {
            set_id: 1,
            members: vec![
                member(1, "a.jpg", 500),
                member(2, "b.jpg", 400),
                member(3, "c.jpg", 300),
            ],
        }];
        let set_id = app.sets[0].set_id;
        app.multi_select = true;

        app.selected = Some(1);
        app.keep_selected();
        app.selected = Some(3);
        app.keep_selected();
        assert_eq!(
            app.keep.get(&set_id),
            Some(&Keep::Several(vec![1, 3])),
            "the second mark did not join the first"
        );

        app.selected = Some(1);
        app.keep_selected();
        assert_eq!(
            app.keep.get(&set_id),
            Some(&Keep::One(3)),
            "taking one off left the wrong picture"
        );

        app.selected = Some(3);
        app.keep_selected();
        assert_eq!(app.keep.get(&set_id), None, "the set is still keeping something");
    }

    /// Keep all marks the whole set. Taking one picture off it leaves the rest
    /// marked rather than throwing the lot away.
    #[test]
    fn taking_one_picture_off_a_set_that_keeps_all_of_it_leaves_the_rest() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![DuplicateSet {
            set_id: 1,
            members: vec![
                member(1, "a.jpg", 500),
                member(2, "b.jpg", 400),
                member(3, "c.jpg", 300),
            ],
        }];
        let set_id = app.sets[0].set_id;
        app.keep.insert(set_id, Keep::All);

        app.selected = Some(2);
        app.keep_selected();

        assert_eq!(
            app.keep.get(&set_id),
            Some(&Keep::Several(vec![1, 3])),
            "the other two did not stay marked"
        );
    }

    /// Two clicks on a second picture while the box is ticked keep it as well as
    /// the one already kept, rather than in place of it.
    #[test]
    fn two_clicks_with_multi_selected_keep_both_pictures() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let root = found.path().to_path_buf();
        let set_id = app.sets[0].set_id;
        app.multi_select = true;
        let first = match app.keep.get(&set_id) {
            Some(Keep::One(file_id)) => *file_id,
            other => panic!("the search did not pick one picture to keep: {other:?}"),
        };
        let other = app.sets[0]
            .members
            .iter()
            .map(|member| member.file_id)
            .find(|file_id| *file_id != first)
            .expect("the set holds one picture");

        let ctx = window();
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let frame = |app: &mut App, at: Option<egui::Pos2>, clicks: usize, time: f64| {
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            if let Some(pos) = at {
                input.events.push(egui::Event::PointerMoved(pos));
                for _ in 0..clicks {
                    for pressed in [true, false] {
                        input.events.push(egui::Event::PointerButton {
                            pos,
                            button: egui::PointerButton::Primary,
                            pressed,
                            modifiers: Default::default(),
                        });
                    }
                }
            }
            crate::shot::frame("two_clicks_with_multi_selected", &ctx, input, |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            })
        };

        let drawn = frame(&mut app, None, 0, 0.0);
        let mut pictures: Vec<egui::Rect> = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - TILE.x).abs() < 6.0
                        && (rect.rect.height() - TILE.y).abs() < 6.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        pictures.sort_by(|a, b| a.left().total_cmp(&b.left()));
        let index = app.sets[0]
            .members
            .iter()
            .position(|member| member.file_id == other)
            .expect("the set lost a picture");
        let at = pictures[index].center();

        frame(&mut app, Some(at), 0, 0.1);
        frame(&mut app, Some(at), 2, 0.2);

        let keeping = app.keep.get(&set_id).expect("the set is keeping nothing");
        assert!(keeping.keeps(first), "the picture kept before the clicks was let go");
        assert!(keeping.keeps(other), "the picture that was clicked twice is not kept");
    }

    /// The two buttons on a set really pressed, in a really scanned folder. Keep
    /// none takes every picture of the set into the plan, and keep all takes
    /// them all back out.
    #[test]
    fn the_buttons_on_a_set_decide_all_of_it_or_none_of_it() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        assert_eq!(app.sets.len(), 1, "the two copies were not found");
        assert_eq!(app.build_plan().files(), 1, "the search kept nothing to start from");

        let root = found.path().to_path_buf();
        let ctx = window();
        let screen =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let frame = |app: &mut App, at: Option<egui::Pos2>, pressed: Option<bool>| {
            let mut input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            if let Some(pos) = at {
                input.events.push(egui::Event::PointerMoved(pos));
                if let Some(pressed) = pressed {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
            crate::shot::frame("the_buttons_on_a_set_decide_all_or_none", &ctx, input, |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            })
        };

        // The pointer has to have been over a widget on an earlier frame before
        // egui reports a click on it, so each press takes three.
        let drawn = frame(&mut app, None, None);
        let none_at = label_rect(&drawn, "keep none")
            .expect("no keep none button was drawn on the set")
            .center();
        let all_at = label_rect(&drawn, "keep all")
            .expect("no keep all button was drawn on the set")
            .center();

        frame(&mut app, Some(none_at), None);
        frame(&mut app, Some(none_at), Some(true));
        frame(&mut app, Some(none_at), Some(false));
        assert_eq!(app.build_plan().files(), 2, "keep none left a picture behind");

        frame(&mut app, Some(all_at), None);
        frame(&mut app, Some(all_at), Some(true));
        frame(&mut app, Some(all_at), Some(false));
        assert_eq!(app.build_plan().files(), 0, "keep all still gave the set up");
    }

    /// What is marked is kept and everything else goes, so a set with the mark
    /// taken off loses every picture in it.
    #[test]
    fn a_set_keeping_nothing_loses_all_of_it() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        // The space bar on the picture already kept takes the mark off.
        app.selected = match app.keep.get(&app.sets[0].set_id) {
            Some(Keep::One(file_id)) => Some(*file_id),
            other => panic!("the search gave the set no keeper: {other:?}"),
        };
        app.keep_selected();

        let plan = app.build_plan();
        assert_eq!(plan.files(), 2, "a set that keeps nothing kept something anyway");
        let mut going: Vec<&str> =
            plan.removals.iter().map(|removal| removal.rel_path.as_str()).collect();
        going.sort();
        let mut all: Vec<&str> =
            app.sets[0].members.iter().map(|member| member.rel_path.as_str()).collect();
        all.sort();
        assert_eq!(going, all);
    }

    #[test]
    fn the_review_state_is_not_written_to_the_index() {
        // The marks live on the window and the plan is built from them alone,
        // which is what makes a review something the index never hears about.
        let found = folder_with_a_duplicate();
        let db_path = headless::default_db_path(found.path());
        let mut app = reviewing(found.path());
        let before = std::fs::metadata(&db_path).expect("the index").len();

        let moving_to = app.sets[0].members[1].file_id;
        app.selected = Some(moving_to);
        app.keep_selected();
        let plan = app.build_plan();

        assert_eq!(plan.files(), 1);
        assert_eq!(app.keep.len(), 1);
        assert_eq!(
            std::fs::metadata(&db_path).expect("the index").len(),
            before,
            "reviewing wrote to the index"
        );
        let conn = db::open_read_only(&db_path).expect("the index");
        let rows: i64 =
            conn.query_row("SELECT count(*) FROM files", [], |row| row.get(0)).expect("count");
        assert_eq!(rows, 3, "reviewing changed what the index holds");
    }

    /// A real pass at the top of the scale, and then the window closed and
    /// opened again from what it wrote down. The folder comes back, the slider
    /// does not: what counts as a duplicate is decided against the pictures on
    /// screen and never carried over from a run that is over.
    #[test]
    fn the_setting_for_what_counts_as_a_duplicate_is_not_kept_across_a_restart() {
        let folder = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(folder.path().to_path_buf());
        app.keep_index = true;
        app.sensitivity = matching::MAX_SENSITIVITY;
        app.ignore_colour = true;
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        assert!(!app.sets.is_empty(), "the pass found nothing to review");

        let next = App::from_settings(app.settings());
        assert_eq!(
            next.sensitivity,
            matching::DEFAULT_SENSITIVITY,
            "the last run's sensitivity came back"
        );
        assert_eq!(next.folder, app.folder, "the folder was not remembered");
        assert!(next.ignore_colour, "the colour setting was not remembered");
    }

    /// A folder of one picture under the given name, so a test can say what
    /// order two of them come out in.
    fn folder_named(root: &std::path::Path, name: &str) -> PathBuf {
        let dir = root.join(name);
        std::fs::create_dir(&dir).expect("mkdir");
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
        }))
        .save_with_format(dir.join("one.png"), image::ImageFormat::Png)
        .expect("a fixture");
        dir
    }

    /// A folder dropped on the window is opened, scanned, and searched, without
    /// anything being pressed, from whichever tab the window happened to be on.
    #[test]
    fn a_folder_dropped_on_the_window_is_scanned_and_searched() {
        let reviewed = folder_with_two_sets();
        let mut app = reviewing(reviewed.path());
        assert_eq!(app.view, View::Review, "the fixture did not reach the review");

        let folder = folder_with_a_duplicate();
        let ctx = window();

        let input = egui::RawInput {
            dropped_files: vec![egui::DroppedFile {
                path: Some(folder.path().to_path_buf()),
                ..Default::default()
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.take_dropped_folder(ctx));

        assert_eq!(app.folder.as_deref(), Some(folder.path()), "the folder was not opened");
        assert!(app.running.is_some(), "the drop did not start a scan");
        assert_eq!(app.view, View::Scan, "the drop left the window on the old tab");

        settle(&mut app);
        assert_eq!(app.sets.len(), 1, "the search did not follow the scan");
        assert_eq!(app.view, View::Review, "the window did not move on to the review");
    }

    /// Only folders. A file dropped on the window is not a folder to scan, and
    /// neither is a drop while a pass is already running.
    #[test]
    fn dropping_anything_but_a_folder_does_nothing() {
        let folder = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        let ctx = window();

        let drop = |app: &mut App, path: PathBuf| {
            let input = egui::RawInput {
                dropped_files: vec![egui::DroppedFile { path: Some(path), ..Default::default() }],
                ..Default::default()
            };
            let _ = ctx.run(input, |ctx| app.take_dropped_folder(ctx));
        };

        drop(&mut app, folder.path().join("one.png"));
        assert!(app.folder.is_none(), "a dropped file was taken for a folder");
        assert!(app.running.is_none(), "a dropped file started a scan");

        // Now with a pass under way: the second folder is not taken up.
        drop(&mut app, folder.path().to_path_buf());
        assert!(app.running.is_some(), "the folder was not scanned");
        let second = folder_with_a_duplicate();
        drop(&mut app, second.path().to_path_buf());
        assert_eq!(
            app.folder.as_deref(),
            Some(folder.path()),
            "a drop during a pass changed the folder"
        );
        settle(&mut app);
    }

    /// Two folders really scanned, one of them twice, and a third only opened.
    /// The list offers what was scanned, in alphabetical order, once each.
    #[test]
    fn a_folder_joins_the_previous_list_by_being_scanned_and_not_by_being_opened() {
        let root = tempfile::tempdir().expect("tempdir");
        let zebra = folder_named(root.path(), "Zebra");
        let apple = folder_named(root.path(), "apple");
        let passed_over = folder_named(root.path(), "middle");

        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(zebra.clone());
        assert!(app.previous.is_empty(), "choosing a folder was enough to list it");

        app.start_scan();
        settle(&mut app);
        app.open_folder(apple.clone());
        app.start_scan();
        settle(&mut app);
        app.open_folder(passed_over);
        assert_eq!(
            app.previous,
            vec![apple.clone(), zebra.clone()],
            "the two scanned folders are not listed alphabetically"
        );

        app.open_folder(zebra.clone());
        app.start_scan();
        settle(&mut app);
        assert_eq!(app.previous, vec![apple, zebra], "scanning again listed it twice");
    }

    /// The last entry of the list really clicked, in a window that has scanned
    /// two folders. It empties the list, and the box goes with it.
    #[test]
    fn the_last_entry_of_the_previous_list_empties_it() {
        let root = tempfile::tempdir().expect("tempdir");
        let first = folder_named(root.path(), "Zebra");
        let second = folder_named(root.path(), "apple");

        let mut app = App::from_settings(crate::settings::Settings::default());
        for folder in [first, second] {
            app.open_folder(folder);
            app.start_scan();
            settle(&mut app);
        }
        assert_eq!(app.previous.len(), 2, "the scanned folders were not listed");

        let ctx = window();
        let screen =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let frame = |app: &mut App, at: Option<egui::Pos2>, pressed: Option<bool>| {
            let mut input = egui::RawInput { screen_rect: Some(screen), ..Default::default() };
            if let Some(pos) = at {
                input.events.push(egui::Event::PointerMoved(pos));
                if let Some(pressed) = pressed {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
            crate::shot::frame("the_last_entry_of_the_previous_list", &ctx, input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.folder_section(ui, 600.0);
                });
            })
        };

        let drawn = frame(&mut app, None, None);
        let box_rect =
            label_rect(&drawn, "previous").expect("no previous box was drawn beside the folder");
        let button = label_rect(&drawn, "Choose folder").expect("no folder button was drawn");
        assert!(
            box_rect.left() > button.right(),
            "the box is not right of the folder picker: {box_rect:?} against {button:?}"
        );
        let path = label_rect(&drawn, &app.folder.as_ref().unwrap().display().to_string())
            .expect("the folder was not shown");
        assert!(
            box_rect.left() >= path.right(),
            "the box is not right of the folder it belongs to: {box_rect:?} against {path:?}"
        );
        // The row was given 600 points, and the button belongs against the far
        // edge of it rather than trailing the path.
        assert!(
            box_rect.right() > 560.0,
            "the box is not against the right edge: {box_rect:?}"
        );
        let box_at = box_rect.center();
        frame(&mut app, Some(box_at), None);
        frame(&mut app, Some(box_at), Some(true));
        frame(&mut app, Some(box_at), Some(false));
        // The list is a popup, and it is drawn on the frame after the one that
        // opened it.
        let opened = frame(&mut app, Some(box_at), None);

        let clear_at = label_rect(&opened, "clear previous locations")
            .expect("the list has no entry to clear it with")
            .center();
        frame(&mut app, Some(clear_at), None);
        frame(&mut app, Some(clear_at), Some(true));
        frame(&mut app, Some(clear_at), Some(false));

        assert!(app.previous.is_empty(), "the list was not emptied");
        let after = frame(&mut app, None, None);
        assert!(
            label_rect(&after, "previous").is_none(),
            "an empty list still offers a box to pick from"
        );
    }

    #[test]
    fn saved_settings_reach_the_window() {
        let saved = crate::settings::Settings {
            folder: Some(PathBuf::from("/photos")),
            previous: vec![PathBuf::from("/photos"), PathBuf::from("/more photos")],
            recurse: false,
            ignore_colour: true,
            window: Some(crate::settings::Window {
                x: 40.0,
                y: 80.0,
                width: 1200.0,
                height: 800.0,
                maximized: false,
            }),
            preview_width: Some(520.0),
        };
        let app = App::from_settings(saved.clone());
        assert_eq!(app.window, saved.window, "the window place was not restored");
        assert_eq!(app.preview_width, Some(520.0), "the divider was not restored");
        assert_eq!(app.folder, Some(PathBuf::from("/photos")));
        assert!(!app.recurse, "the subfolder setting was not restored");
        assert!(app.ignore_colour, "the colour setting was not restored");
        assert_eq!(
            app.previous,
            vec![PathBuf::from("/more photos"), PathBuf::from("/photos")],
            "the folders scanned before were not restored in order"
        );
        assert!(app.db_path.is_some(), "the index path was not derived");
    }

    /// At startup the window opens the saved folder and checks whether it
    /// contains an index file. If it does, the checkbox is ticked and a scan
    /// starts immediately. If it does not, the checkbox is unticked and nothing
    /// happens until the Scan button is pressed. The settings file has no say
    /// in this.
    #[test]
    fn a_folder_with_an_index_is_asked_about_on_opening_and_one_without_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().to_path_buf();
        let settings = |folder: &std::path::Path| crate::settings::Settings {
            folder: Some(folder.to_path_buf()),
            ..crate::settings::Settings::default()
        };

        let app = App::from_settings(settings(&folder));
        assert!(!app.scan_on_open, "an index that is not there was going to be asked anything");
        assert!(!app.keep_index, "the checkbox was ticked although there is no index");

        // Scan the folder so that it has an index. A pass writes one when it has
        // read something, so there has to be something in the folder to read.
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 40])
        }))
        .save_with_format(folder.join("a.png"), image::ImageFormat::Png)
        .expect("a fixture");
        let mut built = App::from_settings(crate::settings::Settings::default());
        built.open_folder(folder.clone());
        built.start_scan();
        settle(&mut built);
        assert!(headless::default_db_path(&folder).is_file(), "the scan did not write an index");

        let app = App::from_settings(settings(&folder));
        assert!(app.scan_on_open, "an index exists but nothing was going to ask it anything");
        assert!(app.keep_index, "an index exists but the checkbox was not ticked");
    }

    /// The checkbox belongs to the folder that is open. Opening a different
    /// folder resets it and the subfolder setting; opening the same folder again
    /// leaves them alone; and opening a folder that contains an index ticks the
    /// checkbox whatever it was before.
    #[test]
    fn the_checkbox_follows_the_folder_that_is_opened() {
        // Scan a folder so that it contains an index.
        let indexed = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(indexed.path().to_path_buf());
        app.keep_index = true;
        app.recurse = true;
        app.start_scan();
        settle(&mut app);

        let fresh = tempfile::tempdir().expect("tempdir");
        app.open_folder(fresh.path().to_path_buf());
        assert!(!app.keep_index, "the checkbox was carried over from the last folder");
        assert!(!app.recurse, "the subfolder setting was carried over from the last folder");

        app.keep_index = true;
        app.open_folder(fresh.path().to_path_buf());
        assert!(app.keep_index, "opening the same folder again cleared the checkbox");

        app.keep_index = false;
        app.open_folder(indexed.path().to_path_buf());
        settle(&mut app);
        assert!(app.keep_index, "the folder contains an index but the checkbox was not ticked");
    }

    /// A folder holding two pairs, so a real pass finds two sets.
    fn folder_with_two_sets() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, seed) in
            [("a1.png", 0), ("a2.png", 0), ("b1.png", 120), ("b2.png", 120)]
        {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
                image::Rgb([((x * 3 + seed) % 256) as u8, ((y * 5 + seed) % 256) as u8, 40])
            }))
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
        }
        dir
    }

    /// A window that has really scanned a folder and found what is in it.
    fn reviewing(folder: &std::path::Path) -> App {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(folder.to_path_buf());
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        app
    }

    /// A folder of pictures, two of them the same, so a real pass over it has
    /// something to find.
    fn folder_with_a_duplicate() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        let picture = |seed: u32| {
            image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
                image::Rgb([((x * 3 + seed) % 256) as u8, ((y * 5) % 256) as u8, 40])
            }))
        };
        for (name, seed) in [("one.png", 0), ("two.png", 0), ("other.png", 90)] {
            picture(seed)
                .save_with_format(dir.path().join(name), image::ImageFormat::Png)
                .expect("a fixture");
        }
        dir
    }

    /// Run the window's own frame loop until the pass and the search it starts
    /// have both finished, or give up rather than hang.
    fn settle(app: &mut App) {
        let ctx = window();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < until {
            // What the folder's index says about itself arrives on a thread of
            // its own, and may start a pass, so it is waited for as well.
            app.hear_the_index(&ctx);
            app.pump_indexer(&ctx);
            app.pump_search(&ctx);
            if app.asking.is_none() && app.running.is_none() && app.searching.is_none() {
                return;
            }
        }
        panic!("the pass never finished");
    }

    /// The window after really using it: a folder scanned, its duplicates found,
    /// one of them chosen. Picking another folder leaves none of it behind.
    #[test]
    fn a_folder_picked_after_a_real_pass_leaves_nothing_of_the_last_one() {
        let scanned = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());

        app.open_folder(scanned.path().to_path_buf());
        app.start_scan();
        settle(&mut app);
        assert_eq!(app.scan.done, 3, "the pass did not read the folder");
        assert_eq!(app.scan.indexed, 3, "the pass did not index the folder");

        app.load_sets();
        settle(&mut app);
        assert_eq!(app.sets.len(), 1, "the two copies were not found");
        app.selected = app.sets[0].members.first().map(|member| member.file_id);
        assert!(app.selected.is_some());

        let next = tempfile::tempdir().expect("tempdir");
        app.open_folder(next.path().to_path_buf());

        assert_eq!(app.folder.as_deref(), Some(next.path()));
        assert!(app.sets.is_empty(), "the sets from the last folder are still here");
        assert_eq!(app.selected, None);
        assert_eq!(app.scan.done, 0, "the last folder's counters are still on screen");
        assert_eq!(app.scan.indexed, 0);
        assert_eq!(app.scan.total, 0);
        assert_eq!(app.scan.finished, None, "the last folder's outcome is still on screen");
        assert!(app.running.is_none(), "a folder with no index was scanned unasked");
    }

    /// The scan button pressed, and the window painted before a single file has
    /// been counted. A bar at nothing paints nothing: a rounded cap around an
    /// empty span is still a bubble on screen saying work has begun.
    #[test]
    fn a_bar_with_nothing_done_paints_no_fill_at_all() {
        let folder = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(folder.path().to_path_buf());
        app.start_scan();
        assert_eq!(app.scan.done, 0, "the pass had already counted something");
        assert_eq!(app.scan.total, 0);

        fn fills(app: &mut App) -> Vec<egui::Rect> {
            let ctx = window();
            let fill = ctx.style().visuals.selection.bg_fill;
            let shapes = crate::shot::frame(
                "a_bar_with_nothing_done",
                &ctx,
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(700.0, 400.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.progress_section(ui));
                },
            );
            shapes
                .into_iter()
                .filter_map(|clipped| match clipped.shape {
                    egui::Shape::Rect(rect) if rect.fill == fill => Some(rect.rect),
                    _ => None,
                })
                .collect()
        }

        let empty = fills(&mut app);
        assert!(
            empty.is_empty(),
            "a bar with no progress painted {} filled rects: {empty:?}",
            empty.len()
        );

        settle(&mut app);
        assert_eq!(app.scan.done, app.scan.total, "the pass did not finish");
        assert_eq!(
            fills(&mut app).len(),
            3,
            "the read, indexed and duplicates bars are not all filled once the pass is done"
        );
    }

    /// The Finding duplicates is the stages that are going to happen and no
    /// others. A search straight after a pass reads no index, because the pass
    /// built what it searches, and a bar that gave that stage a share of itself
    /// would open a third full for work nobody is going to do.
    #[test]
    fn a_search_that_reads_no_index_does_not_start_its_bar_part_full() {
        // What a search after a pass reports first: everything is in memory
        // already, so it says how many pictures there are and starts work.
        let after_a_pass = SearchState {
            stage: Some("comparing"),
            loaded: 900,
            to_load: 900,
            ..SearchState::default()
        };
        let (done, of) = after_a_pass.progress();
        assert_eq!(done, 0, "the bar opened {done} of {of} before anything was done");

        // The same search once the shortlist is half drawn up: half of the
        // first of its two stages.
        let halfway = SearchState {
            shortlisted: 450,
            to_shortlist: 900,
            ..after_a_pass.clone()
        };
        let (done, of) = halfway.progress();
        assert_eq!((done, of), (500, 2000), "half the shortlist is not a quarter of the bar");

        // A search that does read the index has three stages, and reading half
        // of it is half of the first of them.
        let reading = SearchState {
            stage: Some("reading the index"),
            reads_the_index: true,
            loaded: 450,
            to_load: 900,
            ..SearchState::default()
        };
        let (done, of) = reading.progress();
        assert_eq!((done, of), (500, 3000), "half the reading is not a sixth of the bar");
    }

    /// A folder whose index already holds every file in it is fully read and
    /// fully indexed, and both bars say so. Nothing has to run for that to be
    /// true, and a bar left empty because no work happened is a lie about a
    /// folder that is entirely done.
    #[test]
    fn a_pass_with_nothing_left_to_do_fills_both_bars_anyway() {
        let folder = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(folder.path().to_path_buf());
        app.start_scan();
        settle(&mut app);

        // Nothing has changed in the folder, so this pass reads nothing.
        app.start_scan();
        settle(&mut app);
        assert!(app.scan.unchanged > 0, "the second pass read the folder again");
        assert_eq!(app.scan.reading, Stage::Over, "the read bar is not full");
        assert_eq!(app.scan.writing, Stage::Over, "the indexed bar is not full");
    }

    /// The listing is not the reading. A pass that is still working out what is
    /// in the folder has read nothing, and the bar for reading says nothing.
    #[test]
    fn listing_the_folder_does_not_move_the_read_bar() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.scan.listing = Some(4000);
        app.scan.done = 0;
        app.scan.total = 0;
        assert_eq!(app.scan.reading, Stage::Waiting, "the reading had not begun");

        // What the pass says when the listing is over: a total to measure the
        // reading against, and nothing read yet.
        app.scan.total = 4000;
        assert_eq!(
            app.scan.reading,
            Stage::Waiting,
            "a total to read is not the same as having read any of it"
        );
    }

    /// A folder that has been scanned before is brought up to date the moment it
    /// is opened. One that has not waits for the button.
    #[test]
    fn opening_a_folder_scans_it_only_when_its_index_asks_for_that() {
        let known = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());

        app.open_folder(known.path().to_path_buf());
        settle(&mut app);
        assert!(
            app.running.is_none() && app.error.is_none(),
            "a folder nothing is known about was scanned without being asked"
        );

        app.start_scan();
        settle(&mut app);
        assert!(!app.auto_rescan, "the box was ticked without anybody ticking it");

        // Scanned before, and its index does not ask to be brought up to date on
        // sight, so opening it does nothing.
        app.open_folder(known.path().to_path_buf());
        settle(&mut app);
        assert!(app.running.is_none(), "the folder was scanned although its index did not ask");

        // Ticked, and now opening it is enough.
        app.auto_rescan = true;
        app.remember_ways_of_matching();
        let mut again = App::from_settings(crate::settings::Settings::default());
        again.open_folder(known.path().to_path_buf());
        let ctx = window();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        let mut started = false;
        while !started && std::time::Instant::now() < until {
            again.hear_the_index(&ctx);
            again.pump_indexer(&ctx);
            started = again.running.is_some() || again.error.is_some();
        }
        assert!(started, "the index asked to be brought up to date and nothing happened");
        assert!(again.auto_rescan, "the box came back unticked");
        settle(&mut again);

        let fresh = tempfile::tempdir().expect("tempdir");
        again.open_folder(fresh.path().to_path_buf());
        settle(&mut again);
        assert!(again.running.is_none(), "the empty folder was scanned unasked");
        assert!(!again.auto_rescan, "a folder with no index came up ticked");
    }

    /// A different folder is different pictures, so what counted as a duplicate
    /// in the last one does not carry over. Opening the same folder again leaves
    /// the setting alone.
    #[test]
    fn a_different_folder_starts_on_the_default_setting() {
        // A folder really scanned at the top of the scale, with a cleanup
        // destination chosen for it.
        let found = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(found.path().to_path_buf());
        app.sensitivity = matching::MAX_SENSITIVITY;
        app.ignore_colour = true;
        app.destination = Destination::Delete;
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        assert!(!app.sets.is_empty(), "the pass found nothing to review");

        let dir = tempfile::tempdir().expect("tempdir");
        app.open_folder(dir.path().to_path_buf());
        assert_eq!(
            app.sensitivity,
            matching::DEFAULT_SENSITIVITY,
            "the last folder's sensitivity was carried over"
        );
        assert!(!app.ignore_colour, "the last folder's colour setting was carried over");
        assert_eq!(
            app.destination,
            Destination::Trash,
            "the last folder's cleanup choice was carried over"
        );
        assert!(app.move_dir.is_empty(), "the last folder's move folder was carried over");

        app.sensitivity = matching::MAX_SENSITIVITY;
        app.open_folder(dir.path().to_path_buf());
        assert_eq!(
            app.sensitivity,
            matching::MAX_SENSITIVITY,
            "opening the same folder again threw away the setting chosen for it"
        );
    }

    #[test]
    fn no_saved_settings_leaves_the_window_empty() {
        let app = App::from_settings(crate::settings::Settings::default());
        assert_eq!(app.folder, None);
        assert_eq!(app.db_path, None);
        assert!(!app.recurse, "a folder is the folder, not everything under it");
    }

    /// The row says which preset the setting is on, and only that one. A setting
    /// between two of them lights up neither.
    #[test]
    fn the_preset_row_marks_the_one_the_slider_is_on() {
        for (name, percent) in matching::PRESETS {
            let lit: Vec<&str> = matching::PRESETS
                .iter()
                .filter(|(_, other)| on_preset(percent, *other))
                .map(|(other, _)| *other)
                .collect();
            assert_eq!(lit, vec![name], "{name} at {percent} lit up {lit:?}");
        }

        let between = 20.0;
        assert!(
            !matching::PRESETS.iter().any(|(_, percent)| on_preset(between, *percent)),
            "a setting between the presets was drawn as one of them"
        );
        assert!(
            matching::PRESETS.iter().all(|(_, percent)| *percent <= matching::MAX_SENSITIVITY),
            "a preset sits past the end of the slider"
        );
    }

    #[test]
    fn the_slider_widens_what_counts_as_a_duplicate() {
        assert!(Thresholds::at(4.0).max_bits < Thresholds::at(30.0).max_bits);
        assert!(Thresholds::at(30.0).max_bits < Thresholds::at(50.0).max_bits);
        assert!(Thresholds::at(4.0).max_ring < Thresholds::at(50.0).max_ring);
    }

    #[test]
    fn the_app_starts_on_the_default_setting() {
        // Not `App::default`, which reads whatever this machine was last left
        // set to and would pass or fail depending on it.
        let app = App::from_settings(crate::settings::Settings::default());
        assert_eq!(app.sensitivity, matching::DEFAULT_SENSITIVITY);
        assert!(matching::DEFAULT_SENSITIVITY <= matching::MAX_SENSITIVITY);
    }

    #[test]
    fn a_row_fills_the_width_it_is_given() {
        let content = [200.0, 320.0, 180.0];
        let available = 1067.0;
        let widths = share_row_width(available, &content, SECTION_GAP);
        let used: f32 = widths.iter().map(|w| w + FRAME_EXTRA).sum::<f32>() + SECTION_GAP * 2.0;
        assert!((used - available).abs() < 0.5, "used {used} of {available}");
    }

    #[test]
    fn every_box_gets_the_same_share_of_the_leftover() {
        let content = [200.0, 320.0, 180.0];
        let widths = share_row_width(1067.0, &content, SECTION_GAP);
        let shares: Vec<f32> = widths
            .iter()
            .zip(content.iter())
            .map(|(width, natural)| width - natural)
            .collect();
        assert!((shares[0] - shares[1]).abs() < 0.5, "{shares:?}");
        assert!((shares[1] - shares[2]).abs() < 0.5, "{shares:?}");
        assert!(shares[0] > 0.0, "nothing was shared out");
    }

    #[test]
    fn a_wider_box_stays_wider_than_a_narrow_one() {
        // Sharing the leftover equally keeps the differences between the boxes,
        // which is what splitting the row into equal columns threw away.
        let content = [200.0, 320.0, 180.0];
        let widths = share_row_width(1067.0, &content, SECTION_GAP);
        assert!(widths[1] > widths[0]);
        assert!(widths[0] > widths[2]);
    }

    #[test]
    fn a_row_too_narrow_for_its_content_shares_nothing() {
        let content = [200.0, 320.0, 180.0];
        let widths = share_row_width(300.0, &content, SECTION_GAP);
        assert_eq!(widths, content.to_vec());
    }

    #[test]
    fn cleanup_starts_on_the_recycle_bin() {
        assert_eq!(App::default().disposal(), Disposal::Trash);
    }

    #[test]
    fn exactly_one_destination_is_selected_at_a_time() {
        // Held as one value, so the three cannot all read as chosen. They were
        // three separate booleans and did.
        let mut app = App::default();
        for choice in [Destination::Trash, Destination::MoveTo, Destination::Delete] {
            app.destination = choice;
            let selected = [Destination::Trash, Destination::MoveTo, Destination::Delete]
                .iter()
                .filter(|other| **other == app.destination)
                .count();
            assert_eq!(selected, 1, "{:?} did not select exactly one", choice);
        }
    }

    #[test]
    fn each_destination_maps_to_what_the_cleanup_layer_expects() {
        let mut app = App::default();
        app.destination = Destination::Delete;
        assert_eq!(app.disposal(), Disposal::Delete);

        app.destination = Destination::MoveTo;
        app.move_dir = String::from("held");
        assert_eq!(app.disposal(), Disposal::MoveTo(PathBuf::from("held")));
    }

    #[test]
    fn every_destination_has_a_label_and_a_note() {
        for choice in [Destination::Trash, Destination::MoveTo, Destination::Delete] {
            assert!(!choice.label().is_empty());
            assert!(!choice.note().is_empty());
        }
        assert!(
            Destination::Delete.note().contains("cannot be undone"),
            "the permanent option does not say so"
        );
    }
}
