use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use eframe::egui;
use imgdedupe_core::cleanup::{self, Disposal, Plan};
use imgdedupe_core::db;
use imgdedupe_core::matching::{self, DuplicateSet, Thresholds};
use imgdedupe_core::runlog;
use imgdedupe_core::scan;

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
        .with_title(format!("imgdedupe {}", env!("CARGO_PKG_VERSION")))
        .with_icon(crate::icon::window_icon());
    if let Some(window) = saved.window {
        viewport = viewport
            .with_inner_size([window.width, window.height])
            .with_position([window.x, window.y])
            .with_maximized(window.maximized);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
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
    /// Whether the folder holds an index. Looked for on a thread, not while
    /// drawing.
    Found(bool),
    /// The pairs somebody said are not copies of each other, read off the same
    /// connection as the pictures. They are part of what a folder's index says
    /// about it, so they arrive with it and are held in memory from then on.
    Ignored(Vec<(i64, i64)>),
    /// The pictures a review marked to keep, whenever that review was. Read off
    /// the same connection as the pairs, and held until there are sets to hang
    /// them on.
    Kept(Vec<i64>),
    /// The sets the last search of this folder found, as file ids in the order
    /// they were shown in. Held until the pictures arrive, which is what they are
    /// built from.
    Sets(Vec<(i64, Vec<i64>)>),
    /// The pictures, as the search wants them.
    Index(std::sync::Arc<Vec<matching::Image>>),
    Failed(String),
}

/// Why a folder with a saved review is being asked about rather than opened on
/// it.
///
/// One reason, and it is the only one there can be: a file was added, removed or
/// written since the review was saved, so the sets in it are not certainly the
/// sets of what is there now. Nothing having changed is not a question: the
/// review stands and is opened. A folder set to rescan itself is not a question
/// either: with something to bring up to date it is brought up to date, which is
/// what the box says, and with nothing to bring up to date the pass has no work.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Question {
    TheFolderChanged,
}

impl Question {
    fn wording(self) -> &'static str {
        match self {
            Question::TheFolderChanged => {
                "Previous session found but folder content has changed. Load previous session \
                 or rescan?"
            }
        }
    }
}

/// Every pair of pictures in a set, lower file id first, which is how a pair is
/// written down and how it is looked up.
fn pairs_of(set: &DuplicateSet) -> impl Iterator<Item = (i64, i64)> + '_ {
    set.members.iter().enumerate().flat_map(|(at, one)| {
        set.members[at + 1..]
            .iter()
            .map(|other| db::pair(one.file_id, other.file_id))
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

/// What a set is keeping. A set with no entry at all has been marked with
/// nothing, which is a set nobody has reached: it keeps everything it has.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Keep {
    One(i64),
    /// More than one picture. Never one and never none: one picture is `One` and
    /// no picture is no entry at all.
    Several(Vec<i64>),
}

impl Keep {
    fn keeps(&self, file_id: i64) -> bool {
        match self {
            Keep::One(kept) => *kept == file_id,
            Keep::Several(kept) => kept.contains(&file_id),
        }
    }

    /// The pictures marked, for the cleanup to read.
    fn marked(&self) -> Vec<i64> {
        match self {
            Keep::One(kept) => vec![*kept],
            Keep::Several(kept) => kept.clone(),
        }
    }
}

/// Whether a set that is keeping this is keeping that picture.
fn keeps(keeping: Option<&Keep>, file_id: i64) -> bool {
    keeping.is_some_and(|keep| keep.keeps(file_id))
}

/// What a set keeps once one more picture is marked, and once one is unmarked.
/// A set that ends up marked with nothing has no entry, which is what `None`
/// says.
fn marked(keeping: Option<&Keep>, file_id: i64) -> Option<Keep> {
    let mut kept = keeping.map(Keep::marked).unwrap_or_default();
    kept.push(file_id);
    as_keep(kept)
}

fn unmarked(keeping: Option<&Keep>, file_id: i64) -> Option<Keep> {
    let mut kept = keeping.map(Keep::marked).unwrap_or_default();
    kept.retain(|id| *id != file_id);
    as_keep(kept)
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
/// The stamp is whole seconds since the epoch, which is what the index stores.
/// Nothing here reads a date out of the picture's own metadata: most of the
/// formats this reads do not carry one.
fn file_date(mtime_seconds: i64) -> String {
    let (days, rest) = (
        mtime_seconds.div_euclid(86_400),
        mtime_seconds.rem_euclid(86_400),
    );
    let (year, month, day) = civil_from_days(days);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}",
        rest / 3600,
        (rest % 3600) / 60
    )
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
    let month = if shifted_month < 10 {
        shifted_month + 3
    } else {
        shifted_month - 9
    } as u32;
    (if month <= 2 { year + 1 } else { year }, month, day)
}

/// The triangle on a scroll bar's end button, pointing at the end it scrolls to.
fn arrow(
    painter: &egui::Painter,
    button: egui::Rect,
    towards: f32,
    down: bool,
    ink: egui::Color32,
) {
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
/// `step` is how far one click of an end button moves. `wheel_over` is a
/// rectangle a wheel turned anywhere inside counts as a turn on this list, for a
/// list that is one part of a larger pane. Gives back what was shown, how far
/// along it is, how much of it is on screen, and how much of it there is.
fn scrolled<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    down: bool,
    step: f32,
    wheel_over: Option<egui::Rect>,
    area: egui::ScrollArea,
    show: impl FnOnce(egui::ScrollArea, &mut egui::Ui) -> egui::scroll_area::ScrollAreaOutput<R>,
) -> (R, f32, f32, f32) {
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
            egui::Rect::from_min_max(
                egui::pos2(room.left(), room.bottom() - SCROLL_BAR),
                room.max,
            ),
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
    // The bar first, so a drag on it beats a wheel turned at the same time.
    let wanted = paint_scroll_bar(ui, strip, down, step, content, viewport, offset).or_else(|| {
        let pane = wheel_over?;
        let (wheel, pointer) =
            ui.input(|input| (input.smooth_scroll_delta, input.pointer.latest_pos()));
        let at = pointer?;
        // Over the list itself the toolkit has already applied the wheel, and
        // applying it again here is one turn counted twice.
        if wheel[axis] == 0.0 || !pane.contains(at) || output.inner_rect.contains(at) {
            return None;
        }
        let moved = (offset - wheel[axis]).clamp(0.0, (content - viewport).max(0.0));
        (moved != offset).then_some(moved)
    });
    if let Some(wanted) = wanted {
        ui.data_mut(|data| data.insert_temp(id, wanted));
        ui.ctx().request_repaint();
    }
    (output.inner, offset, viewport, content)
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

    let handle_length = (track_length * viewport / content)
        .max(thickness * 2.0)
        .min(track_length);
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

    let base = ui
        .id()
        .with(("scroll bar", strip.left() as i32, strip.top() as i32));
    let stepped = |by: f32| Some((offset + by).clamp(0.0, furthest));
    if ui
        .interact(first, base.with("first"), egui::Sense::click())
        .clicked()
    {
        return stepped(-step);
    }
    if ui
        .interact(last, base.with("last"), egui::Sense::click())
        .clicked()
    {
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
    let along = if down {
        pointer.y - strip.top()
    } else {
        pointer.x - strip.left()
    };

    // Where on the handle the pointer went down, kept for as long as it is held.
    // Taken on the first frame of the press: a drag has not started yet then, and
    // a click is not reported until the button comes back up, so waiting for
    // either of those is what made the handle jump under the pointer.
    let held = ui
        .data_mut(|data| data.get_temp::<f32>(grip))
        .unwrap_or_else(|| {
            let handle_start = thickness + (offset / furthest).clamp(0.0, 1.0) * travel;
            let on_handle = along >= handle_start && along <= handle_start + handle_length;
            // A press on the track outside the handle has nowhere to hold, so it
            // jumps, and the handle arrives centred on the pointer.
            let from_start = if on_handle {
                along - handle_start
            } else {
                handle_length / 2.0
            };
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

/// Room above and below the buttons inside the band along the bottom of a set,
/// so the line along the top of the band stands clear of the buttons instead of
/// being drawn along their top edge. The space between the buttons is the space
/// the row lays them out with.
const BUTTON_ROW_GAP: f32 = 2.0;

/// How far in from the left edge of the box the row of buttons starts.
const BUTTON_ROW_INSET: f32 = 5.0;

/// The band those buttons sit on.
const BUTTON_ROW_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xe8, 0xe8);

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
    line.append(
        &counted(duplicates as u64, "duplicate", "duplicates"),
        gap,
        strong.clone(),
    );
    line.append(&format!("{going} to remove"), gap, strong);
    line.append(
        &format!("{:.1} MB to reclaim", reclaimable as f64 / 1e6),
        gap,
        weak,
    );
    line
}

/// The review toolbar's one row. The checkbox, the counts and the button are
/// laid out over the same rectangle, which has to be as tall as the tallest of
/// them: the button.
const TOOLBAR_HEIGHT: f32 = 28.0;

/// Between the buttons at the left of the review toolbar.
const TOOLBAR_BUTTON_GAP: f32 = 14.0;

/// The cleanup button at the right of it, which is a fixed width so the counts
/// know how much of the row is left for them.
const CLEANUP_BUTTON_WIDTH: f32 = 120.0;

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
        + STRIP_TO_BAR
        + button_row_height(ui)
        + 2.0 * BOX_EDGE
        + BETWEEN_BOXES
}

/// Kept between the writing under the pictures and the strip's own scroll bar,
/// which sits on the band of buttons below it. Enough to be seen: the bar reads
/// as another line of the writing when the two touch.
const STRIP_TO_BAR: f32 = 6.0;

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

/// What a button has to be to hold any of the words it can say, with the padding
/// around them.
fn button_width(ui: &egui::Ui, says: &[&str]) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let widest = says
        .iter()
        .map(|words| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap((*words).to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max);
    widest + 2.0 * PRESET_PADDING.x
}

/// The row of buttons along the bottom of a set, with the space above it.
fn button_row_height(ui: &egui::Ui) -> f32 {
    // What the buttons themselves come out as, with the same small space above
    // and below them. They are drawn with the presets' padding, not the style's
    // own, and a band built to the style's is half as tall again as it needs.
    2.0 * BUTTON_ROW_GAP + ui.text_style_height(&egui::TextStyle::Button) + 2.0 * PRESET_PADDING.y
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
    let under = 5.0 * gap
        + ui.spacing().interact_size.y
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
    painter.rect(rect, rounding, visuals.extreme_bg_color, egui::Stroke::NONE);
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
    ui.painter()
        .with_clip_rect(rect)
        .galley(rect.min, galley, ink);
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
            return (
                PART * (reading + 1) + fraction(self.compared, self.pairs, PART),
                whole,
            );
        }
        if self.to_shortlist > 0 {
            return (
                PART * reading + fraction(self.shortlisted, self.to_shortlist, PART),
                whole,
            );
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
    (
        Lamp::CheckedForIndexFile,
        "Checked for sqlite file in this folder",
    ),
    (
        Lamp::StartedOpeningTheIndexForWriting,
        "Started opening the index for writing",
    ),
    (
        Lamp::FinishedOpeningTheIndexForWriting,
        "Finished opening the index for writing",
    ),
    (
        Lamp::StartedReadingTheIndexSettings,
        "Started reading the index's own settings",
    ),
    (
        Lamp::FinishedReadingTheIndexSettings,
        "Finished reading the index's own settings",
    ),
    (
        Lamp::StartedLookingForTheTotal,
        "Started looking for total number of files",
    ),
    (Lamp::FoundTheTotal, "Found total number of files"),
    (
        Lamp::ListedTheFolder,
        "Retrieved full file list in the folder",
    ),
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
    (
        Lamp::FinishedFindingDuplicates,
        "Finished duplication computation",
    ),
];

impl From<scan::Step> for Lamp {
    fn from(step: scan::Step) -> Self {
        match step {
            scan::Step::StartedReadingTheIndexSettings => Lamp::StartedReadingTheIndexSettings,
            scan::Step::FinishedReadingTheIndexSettings => Lamp::FinishedReadingTheIndexSettings,
            scan::Step::StartedOpeningTheIndexForWriting => Lamp::StartedOpeningTheIndexForWriting,
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
    /// What a cleanup would take, as the review stands.
    ///
    /// Held rather than worked out where it is drawn. It follows from the marks,
    /// the sets and the ignored pairs, and every place any of those changes works
    /// it out again, derived whole each time, so it cannot come to disagree with
    /// them, and derived on the change rather than on the frame, so a review
    /// nobody is touching costs nothing.
    plan: cleanup::Plan,
    /// Whether the folder that is open arrived with an index. A folder without
    /// one has nothing to compare itself against, and choosing it is not asking
    /// for it to be scanned.
    opened_with_an_index: bool,
    /// Whether the pass that is running is the comparison a folder is opened
    /// with, rather than a pass over the folder. What it finds decides what
    /// opening the folder does.
    comparing: bool,
    /// Whether the last pass found a file the index does not have, or a file the
    /// index has that the folder does not.
    something_moved: bool,
    /// The question standing between a saved review and this folder being
    /// searched again. Nothing happens until it is answered.
    question: Option<Question>,
    /// The sets the last search of this folder found, as its index holds them:
    /// file ids and the order they were shown in, and nothing else about the
    /// pictures. Empty for a folder that has never been searched.
    sets_before: Vec<(i64, Vec<i64>)>,
    /// What the last review of this folder marked to keep, as the folder's index
    /// holds it.
    ///
    /// Held per file, because that is what a mark is about. The window holds its
    /// marks per set, and a set does not exist until a search has run, so these
    /// wait here until there are sets to hang them on. A pass does not empty
    /// them: it clears the marks on screen and the next search puts these back
    /// on whatever it finds.
    kept_before: std::collections::HashSet<i64>,
    /// Whether opening this folder starts a pass by itself. Off to begin with,
    /// and kept in the index, which is also the thing it depends on: a folder
    /// with no index has nothing to run on opening.
    auto_rescan: bool,
    /// Whether a pass that has run on opening goes on to mark the best copy in
    /// each set, so a folder can be opened with the obvious answers already
    /// filled in. Kept in the index, and depends on the box above it: there is
    /// nowhere for this to happen if no pass runs.
    auto_mark: bool,
    /// Whether the sets the search is about to hand over are the ones a pass
    /// asked to have marked. Set when that pass finishes and read when they
    /// arrive, since there is nothing to mark until then.
    mark_on_arrival: bool,
    /// The index being read, on a thread of its own. The folder can be on
    /// another machine, and this is a file opened across the network before
    /// anything has been drawn.
    asking: Option<std::sync::mpsc::Receiver<Opened>>,
    /// Whether a pass has started since the folder was asked about itself.
    ///
    /// The pictures read on opening describe the folder as it was before the
    /// pass, and the pass hands over its own, newer copy. Which of the two
    /// arrives first is a race: both are answered by the one thing that owns the
    /// index, and on a busy machine the read from opening can come back after
    /// the pass has already finished. This says the older copy is not to be
    /// taken, however late it turns up.
    scanned_since_asking: bool,
    /// The one thing that owns the folder's index. Everything that reads or
    /// writes it asks this.
    index: imgdedupe_core::index::Index,
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
        // Runs before the first frame, so it touches no files. A thread looks
        // for the index once the window is up and the answer ticks the box.
        let db_path_checked = db_path.is_some();
        let has_index = false;
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
            plan: cleanup::Plan::default(),
            opened_with_an_index: false,
            comparing: false,
            something_moved: false,
            question: None,
            sets_before: Vec::new(),
            kept_before: std::collections::HashSet::new(),
            auto_rescan: false,
            auto_mark: false,
            mark_on_arrival: false,
            asking: None,
            scanned_since_asking: false,
            index: imgdedupe_core::index::Index::start(),
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
            scan_on_open: db_path_checked,
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
                    let response =
                        ui.add_enabled(enabled, egui::SelectableLabel::new(selected, label));
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
        self.ask_about_the_saved_review(ctx);

        if self.running.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
    }

    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.remember();
        // The indexer is a separate process. Left running it holds the index
        // open, and the next run of the application cannot write to it.
        if let Some(run) = self.running.as_mut() {
            run.cancel();
        }
        self.running = None;
        // And the index is closed, which is what waits for the writer.
        //
        // A caller is answered when the copy in memory has its change, not when
        // the file does: the file is caught up on a thread of its own. Ending
        // here without waiting is ending with the last of the review still on
        // that thread's queue, and a mark made a moment before the window closed
        // is exactly what is lost.
        if let Err(err) = self.index.close() {
            runlog::log_line!("the index would not close: {err:#}");
        }
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
                Update::Progress {
                    done,
                    per_sec,
                    unchanged,
                    removed,
                    ignored,
                } => {
                    // These come off the reading, so the reading is under way
                    // whether or not its step arrived first.
                    self.scan.reading = self.scan.reading.begun();
                    self.scan.done = done;
                    self.scan.per_sec = per_sec;
                    self.scan.unchanged = unchanged;
                    self.scan.removed = removed;
                    self.scan.ignored = ignored;
                }
                Update::Indexed {
                    done,
                    total,
                    read,
                    unchanged,
                    ignored,
                } => {
                    // Reading is over when everything in the folder has been read
                    // or was already in the index, which for a folder that has
                    // not changed is true before a single file is opened. The
                    // same for the writing.
                    self.scan.reading = if read >= total {
                        Stage::Over
                    } else {
                        self.scan.reading.begun()
                    };
                    self.scan.writing = if done >= total {
                        Stage::Over
                    } else {
                        self.scan.writing.begun()
                    };
                    self.scan.indexed = done;
                    self.scan.to_index = total;
                    // The counters under the bars come from the read side, and
                    // they move whichever bar it was that moved.
                    self.scan.done = read;
                    self.scan.unchanged = unchanged;
                    self.scan.ignored = ignored;
                }
                Update::Failed { path, message } => self.scan.failures.push((path, message)),
                Update::Done {
                    indexed,
                    removed,
                    failed,
                    elapsed_ms,
                } => {
                    // Files the index does not have, and files it has that the
                    // folder does not. Taken from what the pass says it did, not
                    // from the bars, which count the whole folder.
                    self.something_moved = indexed > 0 || removed > 0;
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
                    let comparing = std::mem::take(&mut self.comparing);
                    match error {
                        Some(message) => self.error = Some(message),
                        None if cancelled => self.scan.finished = Some(String::from("cancelled")),
                        // That was the comparison, not a pass over the folder.
                        // What it found is what decides whether anything else
                        // happens at all.
                        None if comparing => {
                            let moved = self.something_moved;
                            self.decide_what_opening_the_folder_does(moved);
                        }
                        // Nothing was read, so there is nothing to look through.
                        // Searching an empty folder lights two more lamps and
                        // fills a bar for work that cannot have happened.
                        None if self.scan.total == 0 => {}
                        None => {
                            // The marking happens to the sets, which do not
                            // exist until the search this starts comes back.
                            self.mark_on_arrival = self.auto_mark;
                            self.load_sets();
                        }
                    }
                    return;
                }
            }
        }
    }

    fn scan_view(&mut self, ui: &mut egui::Ui) {
        // Everything on one row: the groups and the buttons are all short and
        // stacking them full width leaves most of the window empty.
        let widths = share_row_width(ui.available_width(), &self.scan_content, SECTION_GAP);
        let mut measured = self.scan_content.clone();
        // The three boxes end level with each other, at the height of whichever
        // holds the most. Nothing is a fixed height, so taking a control out
        // takes its space with it.
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

        // The escape key is the Cancel button. It stops whatever that button
        // would stop, and where the button is greyed out it does nothing, so the
        // key never means something the page does not show.
        if self.can_cancel() && ui.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.cancel_work();
        }

        self.progress_section(ui);

        // Only the lamps scroll. The boxes and the bars are the page: a window
        // too short for the whole run of lamps still shows what the folder is
        // set to and how far the run has got, and the bar beside the lamps
        // reaches from the first lamp to the bottom of the window rather than
        // down the whole page.
        ui.add_space(SECTION_GAP);
        let step = ui.spacing().interact_size.y * 3.0;
        scrolled(
            ui,
            egui::Id::new("scan lamps"),
            true,
            step,
            None,
            egui::ScrollArea::vertical().auto_shrink([false, false]),
            |area, ui| area.show(ui, |ui| self.lamps(ui)),
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
                Went::Skipped => {
                    ui.painter()
                        .circle_stroke(rect.center(), DOT, egui::Stroke::new(1.5_f32, GREY))
                }
            }
        };

        // The folder this is all about, before the things done to it. No time
        // against it: opening a folder is choosing one, not work that took a
        // while.
        ui.horizontal(|ui| {
            dot(
                ui,
                if self.folder.is_some() {
                    Went::Happened
                } else {
                    Went::Waiting
                },
            );
            match &self.folder {
                Some(folder) => clipped_line(
                    ui,
                    egui::RichText::new(format!("Loaded {}", folder.display())),
                ),
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
                    if ui
                        .add_enabled(!busy, egui::Button::new("Choose folder"))
                        .clicked()
                    {
                        if let Some(folder) = crate::folder_picker::pick(self.folder.as_deref()) {
                            self.open_folder(folder);
                        }
                    }
                    // The previous button sits over the right end of this row, so
                    // the path stops before it rather than running under it.
                    let reserved = if self.previous.is_empty() {
                        0.0
                    } else {
                        PREVIOUS_ROOM
                    };
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
            // Under the box about rescanning and about it: marking on its own
            // happens at the end of a pass, so with no pass running on opening
            // there is nothing for it to happen at the end of.
            let on_marking = ui.add_enabled(
                !busy && self.keep_index && self.auto_rescan,
                egui::Checkbox::new(&mut self.auto_mark, "Automatically mark to keep"),
            );
            if remember.changed() && !self.keep_index {
                // The index is what remembering a folder amounts to, so taking
                // the tick off takes the index with it. On its own thread: this
                // is up to three files removed, and when the folder is on another
                // machine that is three round trips the window would otherwise
                // sit through with the pointer as a spinning wheel. Nothing here
                // waits on the answer, and the box is already unticked.
                let index = self.index.clone();
                std::thread::spawn(move || {
                    discard_index(&index);
                });
                self.thumbs.forget();
                self.sets.clear();
                self.keep.clear();
                self.selected = None;
                self.showing = None;
                self.scan = ScanState::default();
                self.replan();
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
            let depended_on = subfolders.changed() || remember.changed() || on_opening.changed();
            if depended_on {
                self.settle_the_boxes();
            }
            if apart.changed() || on_opening.changed() || on_marking.changed() || depended_on {
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
                    if ui
                        .selectable_label(here, folder.display().to_string())
                        .clicked()
                    {
                        chosen = Some(folder.clone());
                    }
                }
                ui.separator();
                if ui
                    .selectable_label(false, "clear previous locations")
                    .clicked()
                {
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
            (
                viewport.outer_rect,
                viewport.inner_rect,
                viewport.maximized.unwrap_or(false),
            )
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
        self.replan();
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
        self.scanned_since_asking = false;
        // Whatever the last folder said is not true of this one. What this one
        // says arrives with its index.
        self.ignored.clear();
        self.replan();
        self.kept_before.clear();
        self.sets_before.clear();
        let Some(db_path) = self.db_path.clone() else {
            return;
        };
        // Reading the index is the first thing done to a folder, so the clock
        // the lamps are timed against starts with it, the way it starts again
        // at the press of the Scan button.
        self.started = std::time::Instant::now();
        self.lit.clear();
        let (send, receive) = std::sync::mpsc::channel();
        self.asking = Some(receive);
        let index = self.index.clone();
        // Whether the folder already had an index is a fact about the moment it
        // was opened, so it is read here rather than on the thread below. Taking
        // up a folder with no index makes one, and so does a pass; asked late, on
        // a busy machine, this answers about a file one of those had already
        // created and the folder comes up looking as though it was remembered.
        let there = db_path.is_file();
        std::thread::spawn(move || {
            // Everything below is the manager's work, on this thread rather than
            // the one that draws: for a folder on another machine, taking up an
            // index is a request over the network.
            if let Err(err) = index.open(&db_path) {
                let _ = send.send(Opened::Failed(format!("{err:#}")));
                return;
            }
            let _ = send.send(Opened::Found(there));
            // Whether there is a review to go back to comes first, before what
            // the folder says it does on opening. A folder that asks to be
            // rescanned and has a review saved is asked about rather than
            // rescanned, and the window cannot know that until it has this.
            match index.stored_sets() {
                Ok(sets) => {
                    let _ = send.send(Opened::Sets(sets));
                }
                Err(err) => {
                    let _ = send.send(Opened::Failed(format!("{err:#}")));
                    return;
                }
            }
            let _ = send.send(Opened::Notes(crate::notes::read(&index)));
            // The pairs are part of what the index holds about this folder, and
            // after this they are in memory and nothing asks again.
            match index.ignored() {
                Ok(pairs) => {
                    let _ = send.send(Opened::Ignored(pairs));
                }
                Err(err) => {
                    let _ = send.send(Opened::Failed(format!("{err:#}")));
                    return;
                }
            }
            // And what the last review of this folder marked to keep, for the
            // same reason: it is part of what the index says about the folder,
            // and after this it is in memory.
            match index.kept() {
                Ok(marks) => {
                    let _ = send.send(Opened::Kept(marks));
                }
                Err(err) => {
                    let _ = send.send(Opened::Failed(format!("{err:#}")));
                    return;
                }
            }
            // And the pictures. Having them in memory is what a folder that has
            // been scanned before is for: it can be searched without reading
            // anything again, whether or not a pass runs.
            let telling = send.clone();
            let read = index.images(
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
                std::sync::Arc::new(move |progress| {
                    let _ = telling.send(Opened::Reading(progress));
                }),
            );
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
            // The answers come from a thread of their own, and a window draws
            // only when it is given something to draw for. Left at that it sleeps
            // until the mouse happens to move over it, with the index sitting in
            // the channel: measured at one second on one run and two on the next,
            // for a folder whose index was ready in a fifth of a second.
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            return;
        }
        for said in arrived {
            match said {
                // What the folder was set to. Nothing is started here: what
                // opening a folder does is decided by comparing it with its
                // index, and that cannot be done until the index has arrived.
                Opened::Notes(notes) => self.take_notes(notes),
                Opened::Reading(progress) => self.note_search_progress(progress),
                Opened::Found(there) => {
                    self.light(Lamp::CheckedForIndexFile);
                    self.keep_index = there;
                    self.opened_with_an_index = there;
                    self.settle_the_boxes();
                }
                Opened::Ignored(pairs) => {
                    self.ignored.extend(pairs);
                    self.replan();
                }
                Opened::Kept(marks) => self.kept_before.extend(marks),
                Opened::Sets(sets) => self.sets_before = sets,
                Opened::Index(images) => {
                    self.light(Lamp::LoadedIndexIntoMemory);
                    // A pass that started in the meantime is reading the same
                    // folder and hands over its own, newer copy, whether it is
                    // still running or has already finished.
                    if !self.scanned_since_asking {
                        self.images = Some(images);
                        // Everything the folder had to say has now been said, so
                        // this is where it is compared with what is on disk.
                        self.look_at_the_folder();
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
                        egui::Checkbox::new(&mut self.ignore_colour, "Match colour with grayscale"),
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

    /// Whether there is work the Cancel button would stop.
    ///
    /// Cancel covers the indexing and the search, which are the two a person
    /// waits through. It does not cover a removal: files are going, and stopping
    /// halfway leaves a job half done with nothing said about it.
    ///
    /// Asked here rather than worked out where the button is drawn, because the
    /// escape key is that button and has to be able to say the same thing.
    fn can_cancel(&self) -> bool {
        self.running.is_some() || self.searching.is_some()
    }

    fn run_section(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Vec2 {
        let stoppable = self.can_cancel();
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
                .allocate_ui_with_layout(wide, egui::Layout::top_down(egui::Align::Center), |ui| {
                    ui.add_enabled(
                        !busy && self.db_path.is_some(),
                        egui::Button::new("Find duplicates").min_size(wide),
                    )
                })
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
                ui.label(format!(
                    "listing the folder: {}",
                    counted(found, "file", "files")
                ));
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
            bar(
                ui,
                "Scanning for files",
                self.scan.reading,
                self.scan.done,
                self.scan.total,
            );
            ui.add_space(4.0);
            bar(
                ui,
                "Indexing files",
                self.scan.writing,
                self.scan.indexed,
                self.scan.to_index,
            );
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
                        None,
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
        //
        // Not when the folder is still being opened. A folder that asks to be
        // brought up to date on sight starts its pass from here, and starting
        // the clock again would time it from the moment it began rather than
        // from the moment the folder was opened, hiding everything in between
        // and putting out the lamps that reported it.
        if self.asking.is_none() {
            self.started = std::time::Instant::now();
            self.lit.clear();
        }

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
        // Including the ones the opening read is still on its way back with.
        self.scanned_since_asking = true;
        // And the sets written down for this folder, which are not the sets this
        // pass and the search after it will find. No index holds sets nobody
        // stands behind.
        self.give_up_the_saved_review();
        self.sets.clear();
        self.keep.clear();
        self.replan();
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
        match indexer::start(self.index.clone(), &folder, &db_path, self.recurse, false) {
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
        self.search = SearchState {
            stage: Some("starting"),
            ..SearchState::default()
        };
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
                    // Taken first, because that is where a review begins and the
                    // tables it is written in are made. Then written down, here
                    // where the search is and not in `accept_sets`: sets read
                    // back out of the index go through that too, and there is
                    // nothing to write about sets that came from it. A search
                    // that found nothing writes an empty list, which is a folder
                    // searched and found clean rather than one never searched.
                    self.accept_sets(sets);
                    self.remember_the_sets();
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

    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn accept_sets(&mut self, sets: Vec<DuplicateSet>) {
        runlog::log_line!("found {} duplicate sets", sets.len());
        // Sets are what a review is of, so a review begins here: this is the one
        // place sets are built into the review page, whether they came from a
        // search or out of the index, and the one place the tables a review is
        // written in are made. A folder that already had a review is already
        // holding them and nothing is made.
        if self.db_path.is_some() {
            if let Err(err) = self.index.begin_review() {
                runlog::log_line!("the review could not be started in the index: {err:#}");
            }
        }
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
            self.replan();
            return;
        }

        self.sets = sets;
        // What the last review of this folder marked, before anything the window
        // would mark on its own: a mark somebody made outranks one that was
        // offered, and the auto-marking below only adds what is missing.
        self.take_up_the_marks();
        if std::mem::take(&mut self.mark_on_arrival) {
            self.auto_mark_to_keep();
        }
        self.replan();
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
                    set.members
                        .iter()
                        .map(|member| (member.file_id, member.rel_path.as_str()))
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
            if left.button("unmark all").clicked() {
                self.unmark_everything();
            }
            left.add_space(TOOLBAR_BUTTON_GAP);
            if left.button("mark all").clicked() {
                self.keep_everything();
            }
            left.add_space(TOOLBAR_BUTTON_GAP);
            if left.button("auto-mark to keep").clicked() {
                self.auto_mark_to_keep();
            }

            // In the middle of what is left between the buttons, not the middle
            // of the window: the buttons take the ends of the row, and centring
            // on the window puts the counts over them as soon as there are
            // enough of them.
            let between = egui::Rect::from_min_max(
                egui::pos2(left.min_rect().right() + TOOLBAR_BUTTON_GAP, rect.top()),
                egui::pos2(
                    rect.right() - CLEANUP_BUTTON_WIDTH - TOOLBAR_BUTTON_GAP,
                    rect.bottom(),
                ),
            );
            let mut middle = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(between)
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
                egui::RichText::new("Clean up")
                    .strong()
                    .color(egui::Color32::WHITE),
            )
            .fill(egui::Color32::from_rgb(60, 110, 180))
            .min_size(egui::vec2(CLEANUP_BUTTON_WIDTH, 28.0));
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
            if ui.input(|input| input.modifiers.shift) {
                self.keep_only_selected();
            } else {
                self.keep_selected();
            }
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

        // The page's margin down the left of the list, kept here rather than by
        // the panel, so the panels above can run the width of the window and
        // draw their lines across all of it. The top is the line under the
        // toolbar: the bar down the side of the list starts there, against the
        // line, and the gap above the first set is put inside the list instead.
        let room = ui.available_rect_before_wrap();
        let room =
            egui::Rect::from_min_max(egui::pos2(room.left() + PAGE_MARGIN, room.top()), room.max);
        // What a row comes out as, worked out here where the list's own room is
        // known: inside a scroll area nothing is told where the area ends.
        let row_width = (room.width() - SCROLL_BAR - PAGE_MARGIN).max(0.0);

        let (_, offset, viewport, _) = ui
            .allocate_new_ui(egui::UiBuilder::new().max_rect(room), |ui| {
                let list_id = egui::Id::new("review list");
                scrolled(
                    ui,
                    list_id,
                    true,
                    row_height + spacing,
                    None,
                    list,
                    |list, ui| {
                        // The gap above the first set, taken here rather than
                        // from the rectangle the list was given: the bar down
                        // the side of the list comes from that rectangle and has
                        // to start at the line above it, not at the first box.
                        ui.add_space(SECTION_GAP);
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
        // A set nobody calls a set of copies loses nothing, so what a cleanup
        // would take is not what it was. Worked out with the change, not after
        // the writing below, which a window with no folder open never reaches.
        self.replan();
        // What the set kept stays with it, and so does where the preview was.
        // Neither is acted on while it is ignored: nothing goes from a set that
        // is not a set of copies, and no ring is drawn round a picture in one.
        // Both are what taking it back gives back, the mark where it was,
        // and the preview is where the cursor keys walk from: left and up out of
        // a set that has just been ignored go to the set before it, right and
        // down to the set after it, which they cannot do from nowhere.
        let Some(db_path) = &self.db_path else {
            return;
        };
        let _ = db_path;
        let result = self.index.ignore(&pairs);
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
        self.replan();
        let Some(db_path) = &self.db_path else {
            return;
        };
        let _ = db_path;
        let result = self.index.unignore(&pairs);
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
        if let Some(setting) = notes.auto_mark {
            self.auto_mark = setting;
        }
        // What counts as a duplicate in this folder. A folder is searched the way
        // it was searched before, the same as the ways of matching above: the
        // sets it comes back with are the sets those settings give.
        if let Some(setting) = notes.sensitivity {
            self.sensitivity = setting;
        }
        if let Some(setting) = notes.ignore_colour {
            self.ignore_colour = setting;
        }
        // None of these boxes means anything without the one above it, and a box
        // that means nothing is not left ticked.
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
        if !self.auto_rescan {
            self.auto_mark = false;
        }
    }

    /// What this folder does when it is opened: whether it runs a pass, and
    /// whether that pass marks the best copy in each set.
    ///
    /// Choices about the folder, and they take effect where they are made, so
    /// they are written where they are made. The settings a search runs under
    /// are not here: a control moved and never used has changed nothing, and the
    /// index holds what a search really ran with.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn remember_ways_of_matching(&self) {
        let Some(db_path) = &self.db_path else {
            return;
        };
        let _ = db_path;
        use crate::notes::{mark, AUTO_MARK, AUTO_RESCAN};
        let result = self
            .index
            .set_meta(AUTO_RESCAN, mark(self.auto_rescan))
            .and_then(|()| self.index.set_meta(AUTO_MARK, mark(self.auto_mark)));
        if let Err(err) = result {
            runlog::log_line!("what the folder does on opening could not be written: {err:#}");
        }
    }

    /// Where this folder's duplicates go, and the folder they are moved to. This
    /// belongs to the folder that was scanned rather than to the application:
    /// what is safe to delete outright somewhere is not safe everywhere.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn remember_disposal(&self) {
        if self.db_path.is_none() {
            return;
        }
        let stored = self.destination.name();
        let result = self
            .index
            .set_meta("disposal", stored)
            .and_then(|()| self.index.set_meta("move_dir", &self.move_dir));
        if let Err(err) = result {
            runlog::log_line!("the cleanup choice could not be written: {err:#}");
        }
    }

    /// Put the marks a previous review left back on the sets in front of the
    /// person now.
    ///
    /// The marks are per file and the sets are whatever the search has just
    /// found, so a mark goes back wherever its picture turns up. A mark whose
    /// picture is in no set is left where it is: it is not on screen, so there is
    /// nothing for it to be, and the next thing written takes it out.
    ///
    /// Nothing is written here. These marks came out of the index and putting
    /// them back on screen does not change what it says.
    fn take_up_the_marks(&mut self) {
        if self.kept_before.is_empty() {
            return;
        }
        for set in &self.sets {
            let marked: Vec<i64> = set
                .members
                .iter()
                .map(|member| member.file_id)
                .filter(|file_id| self.kept_before.contains(file_id))
                .collect();
            if let Some(keep) = as_keep(marked) {
                self.keep.insert(set.set_id, keep);
            }
        }
        self.replan();
    }

    /// The review is finished, so what was written down for it goes: the sets and
    /// the marks both.
    ///
    /// Nothing is left to be marked. The cleanup took everything that was not
    /// marked, so the marks name every file still there, which says nothing, and
    /// there are no sets for them to be marks in.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn the_review_is_over(&mut self) {
        self.give_up_the_saved_review();
        self.kept_before.clear();
        if self.db_path.is_none() {
            return;
        }
        if let Err(err) = self.index.clear_keep() {
            runlog::log_line!("the marks could not be cleared: {err:#}");
        }
    }

    /// Throw away the sets written down for this folder, because it is about to
    /// be searched again and they are not what that search will find.
    ///
    /// The marks stay. A mark is about a file, not about a set, and the files are
    /// still there; the next search hangs them on whatever sets it finds.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn give_up_the_saved_review(&mut self) {
        self.sets_before.clear();
        if self.db_path.is_none() {
            return;
        }
        if let Err(err) = self.index.clear_sets() {
            runlog::log_line!("the stored sets could not be cleared: {err:#}");
        }
    }

    /// Compare the folder with its index, which is what decides everything that
    /// happens on opening one.
    ///
    /// The same pass, told to stop once it has compared. It lists the folder,
    /// reads what the index knows and works out the difference, and it reports
    /// every one of those the way it always does, so the bars and the counters
    /// say what is true of the folder: on one where nothing has moved, every file
    /// read and every file indexed, by an earlier run.
    ///
    /// Only for a folder that arrived with an index. One without has nothing to
    /// compare itself against, and choosing a folder is not asking for it to be
    /// scanned.
    fn look_at_the_folder(&mut self) {
        if !self.opened_with_an_index || self.busy() {
            return;
        }
        let (Some(folder), Some(db_path)) = (self.folder.clone(), self.db_path.clone()) else {
            return;
        };
        self.comparing = true;
        match indexer::start(self.index.clone(), &folder, &db_path, self.recurse, true) {
            Ok(run) => self.running = Some(run),
            Err(err) => {
                self.comparing = false;
                self.fail(&format!("{err:#}"));
            }
        }
    }

    /// Act on what the comparison found.
    ///
    /// The review opens on the stored sets when nothing has moved. Otherwise the
    /// folder is brought up to date, or the question goes up and nothing else
    /// happens until it is answered.
    fn decide_what_opening_the_folder_does(&mut self, moved: bool) {
        let saved = !self.sets_before.is_empty();
        runlog::log_line!(
            "opened a folder: something moved {moved}, rescans on opening {}, a saved review of \
             {} sets",
            self.auto_rescan,
            self.sets_before.len()
        );

        // Something has moved. The box says whether that is asked about or simply
        // brought up to date, and with no saved review there is nothing to ask
        // about and nothing to protect.
        if moved {
            if self.auto_rescan || !saved {
                self.start_scan();
            } else {
                self.question = Some(Question::TheFolderChanged);
            }
            return;
        }

        // Nothing has moved, so there is no pass to run whatever the box says.
        // What the comparison found is on the bars already, put there by the
        // comparison itself: every file read, every file indexed, by an earlier
        // run.
        //
        // A saved review is what the folder was left in the middle of; without
        // one, the folder was opened to be searched.
        if saved {
            // The search these sets came out of ran in an earlier session, and
            // this open has just found there is nothing new to run one on. An
            // empty bar would say that work never happened.
            if self.open_the_stored_review() {
                self.search.done = true;
            }
        } else {
            self.load_sets();
        }
    }

    /// Put the question up, and act on the answer.
    ///
    /// Two ways out and no third: this folder's review is finished, or it is
    /// given up and the folder is scanned again. The window is not usable behind
    /// it, because everything behind it is about one or the other.
    fn ask_about_the_saved_review(&mut self, ctx: &egui::Context) {
        let Some(question) = self.question else {
            return;
        };
        let mut answered = None;
        egui::Window::new("Previous session")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(question.wording());
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Finish previous session").clicked() {
                        answered = Some(true);
                    }
                    if ui.button("Rescan").clicked() {
                        answered = Some(false);
                    }
                });
                ui.add_space(4.0);
            });
        match answered {
            Some(true) => {
                self.question = None;
                self.open_the_stored_review();
            }
            Some(false) => {
                self.question = None;
                self.give_up_the_saved_review();
                self.start_scan();
            }
            None => {}
        }
    }

    /// Open the review on the sets a previous search wrote down, without running
    /// one.
    ///
    /// The pictures are the ones this open already read, so nothing is read
    /// again and nothing is compared again. A set whose pictures are gone comes
    /// back short or not at all, which `sets_from_stored` decides.
    ///
    /// Nothing is written: these sets came out of the index and putting them on
    /// screen does not change what it says.
    fn open_the_stored_review(&mut self) -> bool {
        let Some(images) = self.images.clone() else {
            return false;
        };
        if self.sets_before.is_empty() {
            return false;
        }
        let sets = matching::sets_from_stored(&images, &self.sets_before);
        if sets.is_empty() {
            return false;
        }
        runlog::log_line!("opened {} sets from the last session", sets.len());
        // The same way a search's sets are taken, because they are the same
        // thing: sets, going on the review page. The only difference is that
        // these came out of the index rather than out of a search, so there is
        // nothing to write down about them.
        self.accept_sets(sets);
        true
    }

    /// What the window is set to search for, which is what a set it found is the
    /// answer to.
    fn search_settings(&self) -> crate::notes::Search {
        crate::notes::Search {
            sensitivity: self.sensitivity,
            whole_frame: self.match_whole_frame,
            corners: self.match_corners,
            ignore_colour: self.ignore_colour,
            within_a_folder: self.within_a_folder,
        }
    }

    /// Write down the sets a search found, and the settings it found them under.
    ///
    /// The pictures are named by number and nothing about them is written twice:
    /// what each one is comes out of the index the sets were found in.
    ///
    /// This is also where the settings themselves reach the index. Moving the
    /// slider is not using it: until a search has run on a setting, the setting
    /// has done nothing, and what the folder is searched with next time is what
    /// it was last searched with.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn remember_the_sets(&mut self) {
        if self.db_path.is_none() {
            return;
        }
        let stored: Vec<(i64, Vec<i64>)> = self
            .sets
            .iter()
            .map(|set| {
                (
                    set.set_id,
                    set.members.iter().map(|member| member.file_id).collect(),
                )
            })
            .collect();
        let under = self.search_settings();
        let result = self
            .index
            .store_sets(&stored)
            .and_then(|()| crate::notes::ran_under(&self.index, &under));
        if let Err(err) = result {
            runlog::log_line!("the sets could not be written: {err:#}");
        }
    }

    /// Somebody marked these pictures to keep. Two things follow from that and
    /// they follow together: the marks are written down, and what a cleanup would
    /// take is worked out again.
    fn marked(&mut self, file_ids: &[i64]) {
        self.write_marks(file_ids, true);
        self.replan();
    }

    /// And the other way: somebody took the mark off these.
    fn unmarked(&mut self, file_ids: &[i64]) {
        self.write_marks(file_ids, false);
        self.replan();
    }

    /// Write down the marks that just changed, and only those.
    ///
    /// One row per picture somebody touched. Not the whole review: on a folder on
    /// another machine every statement is a journal written and deleted beside the
    /// index, so redoing every mark on every click cost as many of those as the
    /// review had marks, and they piled up behind the person all session.
    ///
    /// Not called where the marks are cleared wholesale: another folder, a pass,
    /// a search coming back, the end of a cleanup. None of those is somebody
    /// unmarking a picture, and what is written down outlives all of them.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    fn write_marks(&self, file_ids: &[i64], keeping: bool) {
        if self.db_path.is_none() || file_ids.is_empty() {
            return;
        }
        let done = if keeping {
            self.index.keep_these(file_ids)
        } else {
            self.index.unkeep_these(file_ids)
        };
        if let Err(err) = done {
            runlog::log_line!("the marks could not be written: {err:#}");
        }
    }

    /// Move the preview with the cursor keys. Nothing happens at either end.
    fn walk(&mut self, visible: &[usize], direction: Direction) {
        let counts: Vec<usize> = visible
            .iter()
            .map(|index| self.sets[*index].members.len())
            .collect();
        let Some(at) = self.position(visible) else {
            return;
        };
        let Some(mut landed) = step(&counts, at, direction) else {
            return;
        };
        // A set nobody calls a set of copies is not somewhere to be: the keys
        // step over it to the next set that is one, and stop where they are when
        // there is none.
        let ignored: Vec<bool> = visible
            .iter()
            .map(|index| self.is_ignored(&self.sets[*index]))
            .collect();
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

    /// Take every mark off every set, so the person can start choosing again.
    ///
    /// A set nobody calls a set of copies is left alone, the way it is everywhere
    /// else: what it was keeping before it was ignored is what it gets back if it
    /// is taken back.
    fn unmark_everything(&mut self) {
        let ignored: Vec<i64> = self
            .sets
            .iter()
            .filter(|set| self.is_ignored(set))
            .map(|set| set.set_id)
            .collect();
        let mut came_off = Vec::new();
        self.keep.retain(|set_id, keep| {
            if ignored.contains(set_id) {
                return true;
            }
            came_off.extend(keep.marked());
            false
        });
        self.unmarked(&came_off);
    }

    /// Mark every picture in every set that is a set of copies, so a cleanup
    /// takes nothing from any of them.
    ///
    /// A set nobody calls a set of copies is left alone, the way it is everywhere
    /// else: nothing goes from it and nothing is marked in it.
    fn keep_everything(&mut self) {
        let mut went_on = Vec::new();
        let sets: Vec<(i64, Vec<i64>)> = self
            .sets
            .iter()
            .filter(|set| !self.is_ignored(set))
            .map(|set| {
                (
                    set.set_id,
                    set.members.iter().map(|member| member.file_id).collect(),
                )
            })
            .collect();
        for (set_id, all) in sets {
            let already = self.keep.get(&set_id).map(Keep::marked).unwrap_or_default();
            went_on.extend(all.iter().copied().filter(|id| !already.contains(id)));
            if let Some(keep) = as_keep(all) {
                self.keep.insert(set_id, keep);
            }
        }
        self.marked(&went_on);
    }

    /// Mark the best copy in every set that has not been dealt with, leaving
    /// every other mark where it is.
    ///
    /// It only ever adds. A set that already marks something keeps that and
    /// gains the best copy as well, unless that is what it already marks. A set
    /// nobody calls a set of copies is an answer already given, so it gets no
    /// mark.
    fn auto_mark_to_keep(&mut self) {
        let best: Vec<(i64, i64)> = self
            .sets
            .iter()
            .filter(|set| !self.is_ignored(set))
            .filter_map(|set| {
                let best = set.members.iter().find(|member| member.auto_keep)?;
                Some((set.set_id, best.file_id))
            })
            .collect();
        let mut went_on = Vec::new();
        for (set_id, file_id) in best {
            let keeping = self.keep.get(&set_id);
            if keeps(keeping, file_id) {
                continue;
            }
            if let Some(now) = marked(keeping, file_id) {
                self.keep.insert(set_id, now);
                went_on.push(file_id);
            }
        }
        self.marked(&went_on);
    }

    /// Mark or unmark the picture the preview is showing, which is what the space
    /// bar and a double click both do.
    ///
    /// One already marked comes off, and a set can end up marked with nothing.
    /// One that is not marked goes on, beside whatever the set already marks.
    fn keep_selected(&mut self) {
        let Some(file_id) = self.selected else {
            return;
        };
        let Some(set) = self
            .sets
            .iter()
            .find(|set| set.members.iter().any(|member| member.file_id == file_id))
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
        let keeping = self.keep.get(&set_id);
        // A mark says to keep this picture and says nothing about any other, so
        // marking one never unmarks another. This is the only way a mark is put
        // on or taken off.
        let coming_off = keeps(keeping, file_id);
        let now = if coming_off {
            unmarked(keeping, file_id)
        } else {
            marked(keeping, file_id)
        };
        match now {
            Some(keep) => self.keep.insert(set_id, keep),
            None => self.keep.remove(&set_id),
        };
        // One picture changed, so one row is written.
        if coming_off {
            self.unmarked(&[file_id]);
        } else {
            self.marked(&[file_id]);
        }
    }

    /// Make the picture the preview is showing the one thing its set keeps,
    /// which is what shift and the space bar, or shift and a double click, do.
    ///
    /// Not a toggle. It says which picture, so pressing it on the one already
    /// marked leaves it marked, and every other mark in that set comes off.
    fn keep_only_selected(&mut self) {
        let Some(file_id) = self.selected else {
            return;
        };
        let Some(set) = self
            .sets
            .iter()
            .find(|set| set.members.iter().any(|member| member.file_id == file_id))
        else {
            return;
        };
        // As with the toggle: a set nobody calls a set of copies keeps nothing
        // and loses nothing, and what it kept is left as it was.
        if self.is_ignored(set) {
            return;
        }
        let set_id = set.set_id;
        // Whatever the set marked before comes off, and this one goes on.
        let came_off: Vec<i64> = self
            .keep
            .get(&set_id)
            .map(Keep::marked)
            .unwrap_or_default()
            .into_iter()
            .filter(|kept| *kept != file_id)
            .collect();
        self.keep.insert(set_id, Keep::One(file_id));
        self.unmarked(&came_off);
        self.marked(&[file_id]);
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
        // Asked of the plan rather than worked out again here. What a cleanup
        // takes is one rule, and a count that reads the marks its own way is a
        // second copy of it that can disagree with the button.
        (self.plan.files(), self.plan.bytes())
    }

    /// Work out again what a cleanup would take. Called wherever the marks, the
    /// sets or the ignored pairs change, and nowhere else.
    fn replan(&mut self) {
        self.plan = self.build_plan();
    }

    /// The first set that is a set of copies, not simply the first set. A set
    /// nobody calls a set of copies keeps nothing and shows nothing as kept, so
    /// opening the review on a picture in one puts the preview somewhere that
    /// means nothing and gives the cursor keys nowhere sensible to start. A
    /// review of nothing but ignored sets opens on nothing.
    fn preselect_first_keeper(&mut self) {
        self.selected =
            self.sets
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
            self.search_cancel
                .store(true, std::sync::atomic::Ordering::Relaxed);
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
        self.replan();
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
                        let came_off: Vec<i64> = self
                            .keep
                            .get(&set_id)
                            .map(Keep::marked)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|kept| *kept != member.file_id)
                            .collect();
                        self.keep.insert(set_id, Keep::One(member.file_id));
                        self.unmarked(&came_off);
                        self.marked(&[member.file_id]);
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
                    self.thumbs
                        .get(member.file_id, thumbs::LARGE_EDGE, root, &member.rel_path);
                if wanted.is_some() {
                    self.showing = self.selected;
                }
                // The one being read is not on screen yet, so what is on screen
                // stays there. Asking for it again is what keeps it alive.
                let drawing = wanted.or_else(|| {
                    let (_, held) = held?;
                    self.thumbs
                        .get(held.file_id, thumbs::LARGE_EDGE, root, &held.rel_path)
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
        scrolled(
            ui,
            bar,
            true,
            line * 3.0,
            Some(where_it_is),
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
                                    clipped_line_in(ui, egui::RichText::new(name).weak(), names);
                                    clipped_line_in(ui, egui::RichText::new(value), room - names);
                                });
                            }
                        }
                    }
                })
            },
        );
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
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(240));
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
                            egui::vec2(
                                inside.width(),
                                tile_strip_height(ui) + STRIP_TO_BAR + SCROLL_BAR,
                            ),
                        );

                        // One tile's width is what a click on the strip's scroll bar
                        // moves by, and the first tile's is as good a step as any.
                        let step = members.first().map_or(TILE.x, tile_width);
                        let bar = egui::Id::new(("set bar", set_id));
                        let (_, offset, viewport, _) = ui
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
                                    None,
                                    egui::ScrollArea::horizontal().id_salt(("set", set_id)),
                                    |area, ui| {
                                        // The pictures keep the box's padding on either
                                        // side of them, inside a strip that is the whole
                                        // width of the box. Their own room, clipped to
                                        // it, so a picture scrolled up against the edge
                                        // stops there rather than in the padding.
                                        let room =
                                            ui.max_rect().shrink2(egui::vec2(BOX_PADDING, 0.0));
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
                        let holds_it = members
                            .iter()
                            .any(|member| Some(member.file_id) == self.selected);
                        if self.show_selected && holds_it {
                            if let Some(wanted) = self.strip_offset(&members, gap, offset, viewport)
                            {
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
                        // A line of its own along the top of the band, in the grey the
                        // scroll bar is drawn round, which the bar above it now sits on.
                        ui.painter().hline(
                            band.x_range(),
                            band.top(),
                            egui::Stroke::new(
                                1.0_f32,
                                ui.visuals().widgets.noninteractive.bg_stroke.color,
                            ),
                        );

                        let mut pressed = None;
                        ui.allocate_new_ui(
                            egui::UiBuilder::new()
                                // In from the left edge of the box, so the line round the
                                // first button is drawn rather than clipped away by it.
                                .max_rect(band.with_min_x(band.left() + BUTTON_ROW_INSET))
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            |ui| {
                                // The presets under the slider are a row of buttons the
                                // size of their own words, spaced by the style, with the
                                // one that is on drawn as pressed. These are the same,
                                // because they are the same kind of thing.
                                ui.spacing_mut().button_padding = PRESET_PADDING;
                                for (label, what, says) in [
                                    ("keep all", SetAction::KeepAll, ["keep all"; 2]),
                                    ("keep none", SetAction::KeepNone, ["keep none"; 2]),
                                    // The third one undoes itself: a set that has been
                                    // ignored is one press away from being a set again.
                                    (
                                        if ignored { "ignored" } else { "ignore" },
                                        if ignored {
                                            SetAction::Unignore
                                        } else {
                                            SetAction::Ignore
                                        },
                                        ["ignore", "ignored"],
                                    ),
                                ] {
                                    // An ignored set keeps nothing, so the two about
                                    // keeping mean nothing until it is a set again.
                                    let usable = !ignored || what == SetAction::Unignore;
                                    // As wide as the widest thing it can ever say. A
                                    // button that fits its current word changes width
                                    // when the word changes, and takes every button to
                                    // the right of it along with it.
                                    let button = egui::Button::new(label)
                                        .wrap_mode(egui::TextWrapMode::Extend)
                                        .min_size(egui::vec2(button_width(ui, &says), 0.0));
                                    if ui.add_enabled(usable, button).clicked() {
                                        pressed = Some(what);
                                    }
                                }
                            },
                        );
                        match pressed {
                            // Every picture in the set marked, and shown as marked. A
                            // cleanup takes nothing from it, the same as a set nobody has
                            // reached, but the set says so rather than looking untouched.
                            Some(SetAction::KeepAll) => {
                                let all: Vec<i64> =
                                    members.iter().map(|member| member.file_id).collect();
                                // The ones that were not marked already: a row is written
                                // for what changed, not for the set.
                                let already =
                                    self.keep.get(&set_id).map(Keep::marked).unwrap_or_default();
                                let went_on: Vec<i64> = all
                                    .iter()
                                    .copied()
                                    .filter(|id| !already.contains(id))
                                    .collect();
                                match as_keep(all) {
                                    Some(keep) => self.keep.insert(set_id, keep),
                                    None => self.keep.remove(&set_id),
                                };
                                self.marked(&went_on);
                            }
                            // Nothing in the set is marked to keep any more. By the rule
                            // above that leaves the set losing nothing, the same as one
                            // nobody has reached.
                            Some(SetAction::KeepNone) => {
                                let came_off = self
                                    .keep
                                    .remove(&set_id)
                                    .map(|keep| keep.marked())
                                    .unwrap_or_default();
                                self.unmarked(&came_off);
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
                    // The same picture whether or not the set is ignored, which
                    // is drawn at half its opacity rather than read again.
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
                    // Inside the keep border, not around it. Drawn outside, the
                    // ring for the picture being looked at sits over the border
                    // that says the picture is being kept, and the one thing a
                    // person needs to see about the picture in front of them is
                    // hidden by the fact that they are looking at it.
                    ui.painter().rect_stroke(
                        bordered.shrink(3.0),
                        2.0,
                        egui::Stroke::new(3.0_f32, ui.style().visuals.selection.bg_fill),
                    );
                }
                let picked = framed.inner;
                if picked.clicked() {
                    self.selected = Some(member.file_id);
                }
                // Twice on a picture keeps it, which is the space bar on the one
                // being shown, including that it takes the mark off again and
                // that holding shift marks it and nothing else.
                if picked.double_clicked() {
                    self.selected = Some(member.file_id);
                    if ui.input(|input| input.modifiers.shift) {
                        self.keep_only_selected();
                    } else {
                        self.keep_selected();
                    }
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
                clipped_line_in(
                    ui,
                    egui::RichText::new(file_date(member.mtime_seconds)).weak(),
                    width,
                );
                clipped_line_in(ui, egui::RichText::new(&member.rel_path).weak(), width);
            });
        });
    }

    fn cleanup_view(&mut self, ui: &mut egui::Ui) {
        // The plan the window holds, not one built here: this runs on every frame
        // the page is on screen.
        let plan = self.plan.clone();
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
                None,
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

    /// What the marks imply. Rebuilt every frame so the count on the button is
    /// always the count of what the button does.
    ///
    /// What is marked is kept and everything else in that set goes. A set marked
    /// with nothing is a set nobody has reached, and none of it goes.
    fn build_plan(&self) -> Plan {
        // A set nobody calls a set of copies is a set nothing happens to: not
        // kept, not removed, not counted. The marks are spelled out here so each
        // list outlives the plan built from borrows of it.
        let said: Vec<(&[imgdedupe_core::matching::Member], Vec<i64>)> = self
            .sets
            .iter()
            .filter(|set| !self.is_ignored(set))
            .map(|set| {
                (
                    set.members.as_slice(),
                    self.keep
                        .get(&set.set_id)
                        .map(Keep::marked)
                        .unwrap_or_default(),
                )
            })
            .collect();
        cleanup::plan_from_sets(
            said.iter()
                .map(|(members, marks)| (*members, marks.as_slice())),
        )
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
        let index = self.index.clone();
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
                        forget_rows(&index, &outcome.removed)
                    } else {
                        discard_index(&index)
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
        // And out of the pictures held in memory, which the search runs over.
        //
        // The window removed those files itself, so what the folder holds now is
        // what it held less that list. Nothing has to be looked at to know it:
        // reading the folder again, or converting the index again, is asking a
        // question this already has the answer to. Until another folder is
        // opened, everything here is what it says it is.
        if let Some(images) = self.images.take() {
            // Taken apart rather than copied: this is the whole index in memory,
            // hundreds of megabytes on a large folder. Nothing else is holding it
            // here, since a cleanup runs from the cleanup page with no search on,
            // and if something were, the pictures are left as they are and the
            // next search reads them again.
            self.images = match std::sync::Arc::try_unwrap(images) {
                Ok(held) => Some(std::sync::Arc::new(matching::without(
                    held,
                    &outcome.removed,
                ))),
                Err(held) => {
                    runlog::log_line!("something else is holding the pictures; leaving them");
                    Some(held)
                }
            };
        }

        // The review has been carried out, so it is over: the files it took are
        // gone and the sets it took them from describe a folder that no longer
        // exists, whether everything it planned to take went or only some of it.
        // The marks go with them, because what is left is what they named.
        //
        // A folder whose index is not being kept has none of this: the index
        // itself goes, further down.
        if self.keep_index {
            self.the_review_is_over();
        }

        if outcome.failed.is_empty() {
            self.sets.clear();
            self.keep.clear();
            self.replan();
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
        let gone: std::collections::HashSet<&str> = removed.iter().map(String::as_str).collect();
        for set in &mut self.sets {
            set.members
                .retain(|member| !gone.contains(member.rel_path.as_str()));
        }
        self.sets.retain(|set| set.members.len() > 1);

        let left: std::collections::HashSet<i64> = self.sets.iter().map(|set| set.set_id).collect();
        self.keep.retain(|set_id, _| left.contains(set_id));

        let still_here: std::collections::HashSet<i64> = self
            .sets
            .iter()
            .flat_map(|set| set.members.iter().map(|m| m.file_id))
            .collect();
        // A set that survived can be left marking a picture that did not, and a
        // `Keep::Several` can be left holding one id, which the type says never
        // happens. Each surviving set's marks are cut down to the pictures still
        // in it and put back through `as_keep`, so `One` and `Several` mean what
        // they say and no mark names a file that has gone.
        let mut emptied = Vec::new();
        for (set_id, keep) in self.keep.iter_mut() {
            let left: Vec<i64> = keep
                .marked()
                .into_iter()
                .filter(|id| still_here.contains(id))
                .collect();
            match as_keep(left) {
                Some(now) => *keep = now,
                None => emptied.push(*set_id),
            }
        }
        for set_id in emptied {
            self.keep.remove(&set_id);
        }

        self.selected = self.selected.filter(|id| still_here.contains(id));
        self.showing = self.showing.filter(|id| still_here.contains(id));
        self.replan();
    }
}

/// Take the index away entirely, for a folder nobody asked to keep.
///
/// Nothing is going to read it again: the next run opens on no folder, and a
/// scan of this one builds it from nothing. Deleting the file is what dropping
/// the rows and rebuilding around them was for, without the copy.
#[cfg_attr(not(feature = "logging"), allow(unused_variables))]
fn discard_index(index: &imgdedupe_core::index::Index) -> usize {
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    match index.delete() {
        Ok(rows) => {
            runlog::log_line!(
                "delete index: {:.2}s, {rows} rows went with it",
                at.elapsed().as_secs_f64()
            );
            rows
        }
        Err(err) => {
            runlog::log_line!("the index would not go: {err:#}");
            0
        }
    }
}

/// Take the files that were removed out of the index. Deleted or moved, they are
/// not at those paths any more, and an index that still lists them offers
/// duplicates of files that are gone.
///
/// This runs on the removal's own thread. It rewrites the index file, which on a
/// folder of thousands is seconds of work.
#[cfg_attr(not(feature = "logging"), allow(unused_variables))]
fn forget_rows(index: &imgdedupe_core::index::Index, removed: &[String]) -> usize {
    if removed.is_empty() {
        return 0;
    }
    let dropped = (|| {
        #[cfg(feature = "logging")]
        let at = std::time::Instant::now();
        let dropped = index.delete_paths(removed.to_vec())?;
        runlog::log_line!(
            "drop removed: {:.2}s, {dropped} rows",
            at.elapsed().as_secs_f64()
        );

        // Rebuilding costs a rewrite of the whole index, so it happens here and
        // only here: a cleanup is the one thing that leaves enough behind to be
        // worth it, and only when it actually dropped rows.
        if dropped > 0 {
            #[cfg(feature = "logging")]
            let at = std::time::Instant::now();
            index.compact()?;
            runlog::log_line!("rebuild index: {:.2}s", at.elapsed().as_secs_f64());
        }
        anyhow::Ok(dropped)
    })();
    match dropped {
        Ok(count) => count,
        Err(err) => {
            runlog::log_line!("the index still lists the removed files: {err:#}");
            0
        }
    }
}

#[cfg(test)]
#[path = "tests/app.rs"]
mod tests;
