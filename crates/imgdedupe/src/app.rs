use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use eframe::egui;
use imgdedupe_core::cleanup::{self, Disposal, Plan};
use imgdedupe_core::db;
use imgdedupe_core::matching::{self, DuplicateSet, Thresholds};
use imgdedupe_core::runlog;

use crate::headless;
use crate::indexer::{self, Run, Update};
use crate::thumbs::{self, Thumbnails};

pub fn launch() -> Result<()> {
    let result = start_window();
    if let Err(err) = &result {
        // Without this a failure to open the window is invisible: a windowed
        // process has no console for the message to go to.
        runlog::line(&format!("the window could not be opened: {err:#}"));
    }
    result
}

fn start_window() -> Result<()> {
    let saved = crate::settings::Settings::load();
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1100.0, 750.0])
        .with_min_inner_size([700.0, 480.0])
        .with_title("imgdedupe");
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
    Sets(Vec<DuplicateSet>),
    Cancelled,
    Failed(String),
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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Keep {
    One(i64),
    All,
}

impl Keep {
    fn keeps(self, file_id: i64) -> bool {
        match self {
            Keep::One(kept) => kept == file_id,
            Keep::All => true,
        }
    }
}

/// Whether a set that is keeping this is keeping that picture.
fn keeps(keeping: Option<Keep>, file_id: i64) -> bool {
    keeping.is_some_and(|keep| keep.keeps(file_id))
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
            room.with_max_x(room.right() - SCROLL_BAR),
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
        .allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| show(area, ui))
        .inner;

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

/// The largest a picture in a set may be drawn, and the height of the strip of
/// them a set row holds: the picture, the keep control and the four lines under
/// it.
const TILE: egui::Vec2 = egui::vec2(156.0, 118.0);
const TILE_STRIP_HEIGHT: f32 = 216.0;

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

/// The buttons that decide a whole set at once, drawn over the right of its
/// strip: keep all of it at the top, none of it at the bottom.
const KEEP_BUTTON: egui::Vec2 = egui::vec2(90.0, 24.0);

/// Kept clear at the right of the folder row for the button that lists the
/// folders scanned before, so a long path stops short of it.
const PREVIOUS_ROOM: f32 = 76.0;

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
fn set_row_height(_ui: &egui::Ui) -> f32 {
    TILE_STRIP_HEIGHT + SCROLL_BAR + FRAME_EXTRA
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

pub struct App {
    view: View,
    folder: Option<PathBuf>,
    db_path: Option<PathBuf>,
    /// Whether this folder is written down for the next run. Off until it is
    /// asked for, and off again the moment another folder is chosen.
    remember_folder: bool,
    /// Folders that have been scanned, alphabetically. Choosing a folder does not
    /// put one here; scanning it does.
    previous: Vec<PathBuf>,
    recurse: bool,
    ignore_colour: bool,
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
        let indexed_already = db_path.as_deref().is_some_and(Path::is_file);
        App {
            view: View::Scan,
            folder: saved.folder,
            db_path,
            remember_folder: saved.remember_folder,
            previous: crate::settings::sorted(&saved.previous),
            recurse: saved.recurse,
            ignore_colour: saved.ignore_colour,
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
            list_offset: 0.0,
            list_viewport: 0.0,
            // A folder nobody asked to keep is shown but not acted on. The scan
            // button is there for that.
            scan_on_open: indexed_already && saved.remember_folder,
            window: saved.window,
            preview_width: saved.preview_width,
            error: None,
            scan_content: vec![0.0; 3],
            scan_row: 0.0,
        }
    }
}

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.note_window(ctx);
        if self.scan_on_open {
            self.scan_on_open = false;
            self.load_disposal();
            self.start_scan();
        }
        self.pump_indexer(ctx);
        self.pump_search(ctx);
        self.pump_cleanup(ctx);
        self.thumbs.collect(ctx);

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                let ready = self.have_sets();
                let tabs = [
                    (View::Scan, "1  Scan", true),
                    (View::Review, "2  Review", ready),
                    (View::Cleanup, "3  Clean up", ready),
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
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if let Some(folder) = &self.folder {
                        clipped_line(
                            ui,
                            egui::RichText::new(folder.display().to_string()).weak(),
                        );
                    }
                });
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
            View::Cleanup => egui::Margin::ZERO,
            _ => egui::Margin::symmetric(16.0, 12.0),
        };
        egui::CentralPanel::default()
            .frame(egui::Frame::central_panel(&ctx.style()).inner_margin(margin))
            .show(ctx, |ui| match self.view {
                View::Scan => self.scan_view(ui),
                View::Review => self.review_view(ui),
                View::Cleanup => self.cleanup_view(ui),
            });

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
    /// Show a problem and put it in the log, so a report of one has something
    /// behind it.
    fn fail(&mut self, message: &str) {
        runlog::line(&format!("ERROR {message}"));
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
                Update::Start { total } => {
                    self.scan = ScanState { total, ..ScanState::default() };
                }
                Update::Progress { done, per_sec, unchanged, removed, ignored } => {
                    self.scan.done = done;
                    self.scan.per_sec = per_sec;
                    self.scan.unchanged = unchanged;
                    self.scan.removed = removed;
                    self.scan.ignored = ignored;
                }
                Update::Indexed { done, total } => {
                    self.scan.indexed = done;
                    self.scan.to_index = total;
                }
                Update::Failed { path, message } => self.scan.failures.push((path, message)),
                Update::Done { indexed, removed, failed, elapsed_ms } => {
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
                })
            },
        );
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
            let remember = ui.add_enabled(
                !busy,
                egui::Checkbox::new(
                    &mut self.remember_folder,
                    "Save an index database for this folder",
                ),
            );
            if remember.changed() && !self.remember_folder {
                // The index is what remembering a folder amounts to, so taking
                // the tick off takes the index with it.
                discard_index(self.db_path.as_deref());
                self.thumbs.forget();
                self.sets.clear();
                self.keep.clear();
                self.selected = None;
                self.showing = None;
                self.scan = ScanState::default();
            }
            if subfolders.changed() || remember.changed() {
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

    /// A folder that has been indexed before is brought up to date on sight. One
    /// that has not waits for the Scan button.
    fn open_folder(&mut self, folder: PathBuf) {
        let db_path = headless::default_db_path(&folder);
        let indexed_already = db_path.is_file();
        let elsewhere = self.folder.as_deref() != Some(folder.as_path());
        self.db_path = Some(db_path);
        self.folder = Some(folder);
        // Everything about the last folder was about its pictures. A different
        // folder starts where a first look at a folder starts, and then takes
        // back whatever its own index has a record of.
        if elsewhere {
            self.sensitivity = matching::DEFAULT_SENSITIVITY;
            self.ignore_colour = false;
            self.recurse = false;
            // An index in the folder is what remembering a folder amounts to,
            // so a folder that has one arrives with the box already ticked.
            self.remember_folder = indexed_already;
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
        // A folder with an index is brought up to date on sight. One without is
        // a folder nothing is known about, and it waits for the Scan button.
        if indexed_already {
            self.load_disposal();
        }
        self.remember();
        if indexed_already {
            self.start_scan();
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
            remember_folder: self.remember_folder,
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
            changed |= ui
                .add_enabled(
                    !busy,
                    egui::Checkbox::new(
                        &mut self.ignore_colour,
                        "Match colour with grayscale",
                    ),
                )
                .changed();
            if changed {
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
                self.load_sets();
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
            let width = ui.available_width();
            // Nothing to do is done, so an empty total is a full bar.
            // Nothing counted yet is nothing done. A pass that finds nothing at
            // all ends with both totals at zero and both bars empty, which is
            // what happened: nothing.
            let bar = |ui: &mut egui::Ui, label: &str, count: u64, out_of: u64| {
                let fraction = if out_of == 0 { 0.0 } else { count as f32 / out_of as f32 };
                progress_bar(ui, label, fraction, width);
            };
            bar(ui, "read", self.scan.done, self.scan.total);
            ui.add_space(4.0);
            bar(ui, "indexed", self.scan.indexed, self.scan.to_index);
            ui.add_space(6.0);
            egui::Grid::new("scan counts")
                .num_columns(5)
                .spacing([24.0, 4.0])
                .show(ui, |ui| {
                    counter(ui, "found", self.scan.found());
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
        self.scan = ScanState::default();
        self.error = None;
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
    fn load_sets(&mut self) {
        let Some(db_path) = self.db_path.clone() else {
            return;
        };
        if self.searching.is_some() {
            return;
        }
        let mut thresholds = Thresholds::at(self.sensitivity);
        thresholds.ignore_colour = self.ignore_colour;
        runlog::line(&format!(
            "matching {} at {:.1}% ({} bits), ignore_colour {}",
            db_path.display(),
            self.sensitivity,
            thresholds.max_bits,
            thresholds.ignore_colour
        ));

        // What the last pass ended with says nothing about this one.
        self.scan.finished = None;
        self.error = None;

        let (send, receive) = std::sync::mpsc::channel::<Found>();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let asked = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let result = headless::open_index(&db_path)
                .and_then(|conn| matching::find_sets_cancellable(&conn, thresholds, &asked));
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
                Found::Sets(sets) => {
                    self.accept_sets(sets);
                    done = true;
                }
                Found::Cancelled => {
                    runlog::line("the search was cancelled");
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
        runlog::line(&format!("found {} duplicate sets", sets.len()));
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
            self.scan.finished = Some(String::from("No duplicates found for current settings"));
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
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new(counted(visible.len() as u64, "set", "sets")).strong(),
                );
                ui.label(
                    egui::RichText::new(counted(
                        duplicates as u64,
                        "duplicate",
                        "duplicates",
                    ))
                    .strong(),
                );
                ui.label(egui::RichText::new(format!("{going} to remove")).strong());
                ui.label(
                    egui::RichText::new(format!("{:.1} MB to reclaim", reclaimable as f64 / 1e6))
                        .weak(),
                );

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let go = egui::Button::new(
                        egui::RichText::new("Clean up").strong().color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(60, 110, 180))
                    .min_size(egui::vec2(120.0, 28.0));
                    if ui.add_enabled(going > 0, go).clicked() {
                        self.view = View::Cleanup;
                    }
                });
            });
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

        let (_, offset, viewport) = scrolled(
            ui,
            egui::Id::new("review list"),
            true,
            row_height + spacing,
            list,
            |list, ui| {
                list.show_rows(ui, row_height, visible.len(), |ui, range| {
                    for position in range {
                        let index = visible[position];
                        self.set_row(ui, index, &root);
                    }
                })
            },
        );
        self.list_offset = offset;
        self.list_viewport = viewport;
    }

    /// Where this folder's duplicates go, and the folder they are moved to. This
    /// belongs to the folder that was scanned rather than to the application:
    /// what is safe to delete outright somewhere is not safe everywhere.
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
            runlog::line(&format!("the cleanup choice could not be written: {err:#}"));
        }
    }

    /// Take back what this folder's index records: where a cleanup sends what it
    /// removes, and how far down the folder the index reaches.
    fn load_disposal(&mut self) {
        let Some(db_path) = &self.db_path else {
            return;
        };
        let read = headless::open_index(db_path).and_then(|conn| {
            Ok((
                db::get_meta(&conn, "disposal")?,
                db::get_meta(&conn, "move_dir")?,
                db::get_meta(&conn, "recurse")?,
            ))
        });
        match read {
            Ok((disposal, move_dir, recurse)) => {
                if let Some(choice) = disposal.as_deref().and_then(Destination::from_name) {
                    self.destination = choice;
                }
                if let Some(folder) = move_dir {
                    self.move_dir = folder;
                }
                // An index built over the subfolders has to be scanned that way
                // again, or the next pass drops every row under them.
                if let Some(setting) = recurse {
                    self.recurse = setting == "1";
                }
            }
            Err(err) => runlog::line(&format!("the cleanup choice could not be read: {err:#}")),
        }
    }

    /// Move the preview with the cursor keys. Nothing happens at either end.
    fn walk(&mut self, visible: &[usize], direction: Direction) {
        let counts: Vec<usize> =
            visible.iter().map(|index| self.sets[*index].members.len()).collect();
        let Some(at) = self.position(visible) else {
            return;
        };
        let Some((set, member)) = step(&counts, at, direction) else {
            return;
        };
        self.selected = Some(self.sets[visible[set]].members[member].file_id);
        self.scroll_to = Some(set);
    }

    /// Keep the picture the preview is showing, which is what the space bar does.
    fn keep_selected(&mut self) {
        let Some(file_id) = self.selected else {
            return;
        };
        let holder = self
            .sets
            .iter()
            .find(|set| set.members.iter().any(|member| member.file_id == file_id))
            .map(|set| set.set_id);
        // Pressing it again on the one already kept takes the mark off, so the
        // same key both makes a choice and undoes it.
        if let Some(set_id) = holder {
            if self.keep.get(&set_id) == Some(&Keep::One(file_id)) {
                self.keep.remove(&set_id);
            } else {
                self.keep.insert(set_id, Keep::One(file_id));
            }
        }
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
            let keeping = self.keep.get(&set.set_id).copied();
            for member in &set.members {
                if !keeps(keeping, member.file_id) {
                    count += 1;
                    bytes += member.size_bytes;
                }
            }
        }
        (count, bytes)
    }

    fn preselect_first_keeper(&mut self) {
        self.selected = self.sets.first().and_then(|set| match self.keep.get(&set.set_id) {
            Some(Keep::One(file_id)) => Some(*file_id),
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
    fn cancel_work(&mut self) {
        if let Some(run) = self.running.as_mut() {
            run.cancel();
        }
        if self.searching.is_some() {
            runlog::line("cancelling: stopping the search");
            self.search_cancel.store(true, std::sync::atomic::Ordering::Relaxed);
        }
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
                    let keeping = keeps(self.keep.get(&set_id).copied(), member.file_id);
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

                let room = ui.available_size();
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

                ui.centered_and_justified(|ui| match drawing {
                    Some(texture) => {
                        ui.add(
                            egui::Image::new(&texture).max_size(room).maintain_aspect_ratio(true),
                        );
                    }
                    None => {
                        ui.label(egui::RichText::new("reading...").weak());
                    }
                });
            });
        self.preview_width = Some(pane.response.rect.width());
    }

    fn set_row(&mut self, ui: &mut egui::Ui, index: usize, root: &std::path::Path) {
        let set_id = self.sets[index].set_id;
        let members = self.sets[index].members.clone();
        let keeping = self.keep.get(&set_id).copied();

        // Built to a fixed height rather than measured afterwards. The list places
        // the rows it is not drawing by this number, and a row that came out any
        // other height would move the content under a scroll already in progress.
        let size = egui::vec2(ui.available_width(), set_row_height(ui));
        ui.allocate_ui(size, |ui| {
        ui.set_min_size(size);
        egui::Frame::group(ui.style())
            .show(ui, |ui| {
                ui.set_height(TILE_STRIP_HEIGHT + SCROLL_BAR);
                let inside = ui.max_rect();

                // One tile's width is what a click on the strip's scroll bar
                // moves by, and the first tile's is as good a step as any.
                let step = members.first().map_or(TILE.x, tile_width);
                scrolled(
                    ui,
                    egui::Id::new(("set bar", set_id)),
                    false,
                    step,
                    egui::ScrollArea::horizontal().id_salt(("set", set_id)),
                    |area, ui| {
                        area.show(ui, |ui| {
                            ui.horizontal_top(|ui| {
                                for member in &members {
                                    let width = tile_width(member);
                                    self.member_tile(ui, member, keeping, root, width);
                                }
                            });
                        })
                    },
                );

                // Over the strip rather than above it. A row of its own to hold
                // one button is a row of the list nobody can see pictures in.
                let edge =
                    egui::Stroke::new(1.0_f32, egui::Color32::from_rgb(0x33, 0x33, 0x33));
                let at = egui::Rect::from_min_size(
                    egui::pos2(inside.right() - KEEP_BUTTON.x, inside.top()),
                    KEEP_BUTTON,
                );
                if ui.put(at, egui::Button::new("keep all").stroke(edge)).clicked() {
                    self.keep.insert(set_id, Keep::All);
                }

                // Above the strip's own scroll bar, which has an arrow in the
                // corner this would otherwise cover.
                let at = egui::Rect::from_min_size(
                    egui::pos2(
                        inside.right() - KEEP_BUTTON.x,
                        inside.bottom() - SCROLL_BAR - KEEP_BUTTON.y,
                    ),
                    KEEP_BUTTON,
                );
                if ui.put(at, egui::Button::new("keep none").stroke(edge)).clicked() {
                    // Nothing marked is nothing kept, so the whole set goes.
                    self.keep.remove(&set_id);
                }
            });
        });
    }

    /// One image in a set: the picture, whether it is the one being kept, and the
    /// two facts that decide it.
    fn member_tile(
        &mut self,
        ui: &mut egui::Ui,
        member: &imgdedupe_core::matching::Member,
        keeping: Option<Keep>,
        root: &std::path::Path,
        width: f32,
    ) {
        let kept = keeps(keeping, member.file_id);
        let showing = self.selected == Some(member.file_id);
        let keep_colour = egui::Color32::from_rgb(90, 180, 110);

        ui.allocate_ui(egui::vec2(width, TILE_STRIP_HEIGHT), |ui| {
            ui.set_height(TILE_STRIP_HEIGHT);
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
                    let picture = if on_screen {
                        self.thumbs.get(member.file_id, thumbs::THUMB_EDGE, root, &member.rel_path)
                    } else {
                        None
                    };
                    match picture {
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
                ui.label(format!("{}x{}", member.width, member.height));
                ui.label(
                    egui::RichText::new(format!(
                        "{}  {:.1} MB",
                        member.format,
                        member.size_bytes as f64 / 1_000_000.0
                    ))
                    .weak(),
                );
                ui.label(egui::RichText::new(file_date(member.mtime_ns)).weak());
                clipped_line(ui, egui::RichText::new(&member.rel_path).weak());
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
                        runlog::line("the remove button was pressed");
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
            let keeping = self.keep.get(&set.set_id).copied();
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
            runlog::line("cleanup asked for with no folder open");
            return;
        };
        if self.removing.is_some() {
            runlog::line("cleanup asked for while one is already running");
            return;
        }
        let plan = plan.clone();
        let disposal = self.disposal();
        let total = plan.files();
        self.cleanup_failures.clear();
        runlog::line(&format!(
            "cleanup starting: {total} files, {:.1} MB, to {:?}, under {}",
            plan.bytes() as f64 / 1_000_000.0,
            disposal,
            root.display()
        ));
        for removal in plan.removals.iter().take(5) {
            runlog::line(&format!("  removing {}", removal.rel_path));
        }
        if total > 5 {
            runlog::line(&format!("  and {} more", total - 5));
        }

        let (send, receive) = std::sync::mpsc::channel::<Removal>();
        let steps = send.clone();
        let db_path = self.db_path.clone();
        let keep_index = self.remember_folder;
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
        for (path, message) in &outcome.failed {
            runlog::line(&format!("  could not remove {path}: {message}"));
        }
        let index = if self.remember_folder {
            format!("{forgotten} dropped from the index")
        } else {
            String::from("the index was deleted")
        };
        runlog::line(&format!(
            "cleanup finished: {} removed, {} failed, {:.1} MB, {index}",
            outcome.removed.len(),
            outcome.failed.len(),
            outcome.bytes_freed as f64 / 1_000_000.0
        ));
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
            if !self.remember_folder {
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
fn discard_index(db_path: Option<&Path>) -> usize {
    let Some(db_path) = db_path else {
        return 0;
    };
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
            Err(err) => runlog::line(&format!("{} would not go: {err}", path.display())),
        }
    }
    runlog::line(&format!(
        "delete index: {:.2}s, {gone} files, {}",
        at.elapsed().as_secs_f64(),
        db_path.display()
    ));
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
fn forget_rows(db_path: Option<&Path>, removed: &[String]) -> usize {
    let Some(db_path) = db_path else {
        runlog::line("nothing was dropped from the index: no index is open");
        return 0;
    };
    if removed.is_empty() {
        return 0;
    }
    let dropped = db::open_for_notes(db_path).and_then(|mut conn| {
        let at = std::time::Instant::now();
        let tx = conn.transaction()?;
        let dropped = db::delete_paths(&tx, removed)?;
        tx.commit()?;
        drop(conn);
        runlog::line(&format!(
            "drop removed: {:.2}s, {dropped} rows",
            at.elapsed().as_secs_f64()
        ));

        // Rebuilding costs a copy of the whole index, so it happens here and only
        // here: a cleanup is the one thing that leaves enough behind to be worth
        // it, and only when it actually dropped rows.
        if dropped > 0 {
            let at = std::time::Instant::now();
            db::compact(db_path)?;
            runlog::line(&format!("rebuild index: {:.2}s", at.elapsed().as_secs_f64()));
        }
        Ok(dropped)
    });
    match dropped {
        Ok(count) => count,
        Err(err) => {
            runlog::line(&format!("the index still lists the removed files: {err:#}"));
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use imgdedupe_core::matching::Member;

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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
        app.cancel_work();
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

    /// A folder nobody asked to keep has no use for its index once the cleanup is
    /// over: the next run opens on nothing and a scan builds it again. The file
    /// goes, with the write-ahead log beside it.
    #[test]
    fn a_folder_that_is_not_remembered_loses_its_index_when_the_cleanup_is_done() {
        let scanned = folder_with_a_duplicate();
        let db_path = headless::default_db_path(scanned.path());
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(scanned.path().to_path_buf());
        app.start_scan();
        settle(&mut app);
        app.load_sets();
        settle(&mut app);
        assert!(db_path.is_file(), "the pass wrote no index");
        assert!(!app.remember_folder, "a fresh folder is not remembered");

        app.destination = Destination::Delete;
        let plan = app.build_plan();
        app.view = View::Cleanup;
        app.run_cleanup(&plan);
        let ctx = egui::Context::default();
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
        app.remember_folder = true;
        app.destination = Destination::Delete;
        let plan = app.build_plan();
        let going = plan.removals[0].rel_path.clone();

        app.view = View::Cleanup;
        app.run_cleanup(&plan);
        let ctx = egui::Context::default();
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
        let ctx = egui::Context::default();
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
                    app.set_row(ui, index, &root);
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
        let ctx = egui::Context::default();
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
            let output = ctx.run(input.clone(), |ctx| {
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
            shapes = output.shapes;
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
        window: egui::Vec2,
        preview_width: Option<f32>,
    ) -> Vec<egui::epaint::RectShape> {
        let ctx = egui::Context::default();
        ctx.set_visuals(visuals);
        install_style(&ctx);
        crate::fonts::install(&ctx);

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
            screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), window)),
            ..Default::default()
        };

        let mut shapes = Vec::new();
        for _ in 0..30 {
            let output = ctx.run(input.clone(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
            });
            shapes = output.shapes;
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
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::light());
        install_style(&ctx);

        let strip = egui::Rect::from_min_size(egui::pos2(388.0, 0.0), egui::vec2(12.0, 300.0));
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                paint_scroll_bar(ui, strip, true, 60.0, 3000.0, 300.0, 0.0);
            });
        });

        let mut track = None;
        let mut handle = None;
        let mut triangles = 0;
        for clipped in output.shapes {
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
        let ctx = egui::Context::default();
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
                ctx.run(input, |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let moved =
                            paint_scroll_bar(ui, strip, true, step, content, viewport, offset);
                        if button == Some(true) {
                            *wanted.borrow_mut() = moved;
                        }
                    });
                });
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
        let ctx = egui::Context::default();
        ctx.set_visuals(egui::Visuals::light());

        let strip = egui::Rect::from_min_size(egui::pos2(0.0, 288.0), egui::vec2(400.0, 12.0));
        let output = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                paint_scroll_bar(ui, strip, false, 60.0, 4000.0, 400.0, 0.0);
            });
        });

        let mut track = None;
        let mut handle = None;
        let mut triangles = Vec::new();
        for clipped in output.shapes {
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
        let ctx = egui::Context::default();
        let shapes = ctx
            .run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(900.0, 500.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.set_row(ui, 0, &root));
                },
            )
            .shapes;

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

        let ctx = egui::Context::default();
        let mut taken = 0.0;
        let output = ctx.run(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(900.0, 500.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let before = ui.next_widget_position().y;
                    app.set_row(ui, 0, &root);
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
        for clipped in &output.shapes {
            lowest(&clipped.shape, &mut bottom);
        }
        assert!(bottom > 0.0, "the row painted no text at all");

        // What is under the last line: the strip's scroll bar, and the few points
        // of padding the frame draws inside its own edge. Nothing else.
        let spare = taken - bottom;
        assert!(
            spare < SCROLL_BAR + 6.0,
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

        let ctx = egui::Context::default();
        install_style(&ctx);
        let screen =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let mut frame = |app: &mut App, at: egui::Pos2, pressed: Option<bool>| {
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
                egui::CentralPanel::default().show(ctx, |ui| app.set_row(ui, 0, &root));
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

        let ctx = egui::Context::default();
        install_style(&ctx);
        let screen =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
        let mut frame = |app: &mut App, at: Option<egui::Pos2>, time: f64| {
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(time),
                ..Default::default()
            };
            if let Some(pos) = at {
                input.events.push(egui::Event::PointerMoved(pos));
            }
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.set_row(ui, 0, &root));
            })
            .shapes
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

    /// Keep none sits at the foot of the tiles, level with the last line under
    /// the pictures, rather than floating somewhere above it.
    #[test]
    fn keep_none_sits_level_with_the_last_line_under_the_pictures() {
        let found = folder_with_a_duplicate();
        let mut app = reviewing(found.path());
        let root = found.path().to_path_buf();

        let ctx = egui::Context::default();
        let shapes = ctx
            .run(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::pos2(0.0, 0.0),
                        egui::vec2(900.0, 500.0),
                    )),
                    ..Default::default()
                },
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| app.set_row(ui, 0, &root));
                },
            )
            .shapes;

        let painted = texts(&shapes);
        let button = painted
            .iter()
            .find(|(text, _)| text == "keep none")
            .map(|(_, rect)| *rect)
            .expect("no keep none button was drawn");
        let last = painted
            .iter()
            .filter(|(text, _)| text != "keep none")
            .map(|(_, rect)| rect.bottom())
            .fold(0.0_f32, f32::max);

        assert!(
            (button.bottom() - last).abs() < 8.0,
            "keep none ends at {} and the last line under the pictures at {last}",
            button.bottom()
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

        let ctx = egui::Context::default();
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
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.set_row(ui, 0, &root));
            })
            .shapes
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
        let ctx = egui::Context::default();
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
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.set_row(ui, 0, &root));
            })
            .shapes
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
        app.remember_folder = true;
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

        let ctx = egui::Context::default();
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
            ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.folder_section(ui, 600.0);
                });
            })
            .shapes
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
            box_rect.left() > path.right(),
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
            remember_folder: true,
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

    /// A remembered folder that has been indexed before is brought up to date
    /// without being asked. One that has not been indexed waits for the button.
    #[test]
    fn a_folder_with_an_index_is_scanned_on_opening_and_one_without_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().to_path_buf();
        let settings = |folder: &std::path::Path| crate::settings::Settings {
            folder: Some(folder.to_path_buf()),
            remember_folder: true,
            ..crate::settings::Settings::default()
        };

        let app = App::from_settings(settings(&folder));
        assert!(!app.scan_on_open, "there is no index to bring up to date");

        // An index the application itself built, by scanning the folder.
        let mut built = App::from_settings(crate::settings::Settings::default());
        built.open_folder(folder.clone());
        built.start_scan();
        settle(&mut built);
        assert!(headless::default_db_path(&folder).is_file(), "the pass wrote no index");

        let app = App::from_settings(settings(&folder));
        assert!(app.scan_on_open, "the index was there and was left alone");

        let app = App::from_settings(crate::settings::Settings {
            remember_folder: false,
            ..settings(&folder)
        });
        assert!(
            !app.scan_on_open,
            "a folder nobody asked to keep was scanned without being asked"
        );
    }

    /// The location is written down only while the box is ticked. Another folder
    /// clears it, unless that folder already has an index, which is what being
    /// remembered comes to.
    #[test]
    fn a_location_is_only_remembered_while_it_is_asked_for() {
        // A folder the application scanned, so it holds an index it built.
        let indexed = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.open_folder(indexed.path().to_path_buf());
        app.remember_folder = true;
        app.recurse = true;
        app.start_scan();
        settle(&mut app);

        let fresh = tempfile::tempdir().expect("tempdir");
        app.open_folder(fresh.path().to_path_buf());
        assert!(!app.remember_folder, "the last folder's choice was carried over");
        assert!(!app.recurse, "the last folder's subfolder setting was carried over");

        app.remember_folder = true;
        app.open_folder(fresh.path().to_path_buf());
        assert!(app.remember_folder, "opening the same folder cleared the choice");

        app.remember_folder = false;
        app.open_folder(indexed.path().to_path_buf());
        settle(&mut app);
        assert!(
            app.remember_folder,
            "a folder that already holds an index is a folder being remembered"
        );
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
        let ctx = egui::Context::default();
        let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while std::time::Instant::now() < until {
            app.pump_indexer(&ctx);
            app.pump_search(&ctx);
            if app.running.is_none() && app.searching.is_none() {
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
            let ctx = egui::Context::default();
            let fill = ctx.style().visuals.selection.bg_fill;
            let output = ctx.run(Default::default(), |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.progress_section(ui));
            });
            output
                .shapes
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
            2,
            "the read and indexed bars are not both filled once the pass is done"
        );
    }

    /// A folder that has been scanned before is brought up to date the moment it
    /// is opened. One that has not waits for the button.
    #[test]
    fn opening_a_folder_only_scans_one_that_has_been_scanned_before() {
        let known = folder_with_a_duplicate();
        let mut app = App::from_settings(crate::settings::Settings::default());

        app.open_folder(known.path().to_path_buf());
        assert!(
            app.running.is_none() && app.error.is_none(),
            "a folder nothing is known about was scanned without being asked"
        );

        app.start_scan();
        settle(&mut app);

        let fresh = tempfile::tempdir().expect("tempdir");
        app.open_folder(fresh.path().to_path_buf());
        assert!(app.running.is_none(), "the empty folder was scanned unasked");

        app.open_folder(known.path().to_path_buf());
        assert!(
            app.running.is_some() || app.error.is_some(),
            "the folder it had already scanned was not brought up to date"
        );
        settle(&mut app);
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
