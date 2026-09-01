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
use crate::Strictness;

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
            install_scroll_style(&cc.egui_ctx);
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
    Done(Box<cleanup::Outcome>),
    Failed(String),
}

/// What the thread searching for duplicates sends back.
enum Found {
    Progress(matching::Progress),
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

/// Where the list has to be scrolled to for one row to be wholly in sight, or
/// nothing when it already is.
///
/// The rows the list is not drawing cannot be asked to scroll themselves into
/// view, so the place of the one wanted is worked out from its number: every row
/// is the same height, and they are laid out one after another.
fn scroll_to_show(
    row: usize,
    row_height: f32,
    spacing: f32,
    offset: f32,
    viewport: f32,
) -> Option<f32> {
    let top = row as f32 * (row_height + spacing);
    let bottom = top + row_height;
    if top < offset {
        Some(top)
    } else if bottom > offset + viewport {
        Some((bottom - viewport).max(0.0))
    } else {
        None
    }
}

/// Where removed files go. Held as one value rather than three booleans, so the
/// three choices cannot all appear selected at once.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Destination {
    Trash,
    Quarantine,
    Delete,
}

impl Destination {
    fn label(self) -> &'static str {
        match self {
            Destination::Trash => "Recycle bin",
            Destination::Quarantine => "Move to a folder",
            Destination::Delete => "Delete permanently",
        }
    }

    fn note(self) -> &'static str {
        match self {
            Destination::Trash => "Recoverable from the recycle bin.",
            Destination::Quarantine => "Keeps the folder structure, so the files can be put back.",
            Destination::Delete => "This cannot be undone.",
        }
    }

    fn name(self) -> &'static str {
        match self {
            Destination::Trash => "trash",
            Destination::Quarantine => "quarantine",
            Destination::Delete => "delete",
        }
    }

    /// What the button that carries this out does, in the words for it. Moving a
    /// file to another folder is not removing it.
    fn verb(self) -> &'static str {
        match self {
            Destination::Trash | Destination::Delete => "Remove",
            Destination::Quarantine => "Move",
        }
    }

    fn from_name(name: &str) -> Option<Self> {
        match name {
            "trash" => Some(Destination::Trash),
            "quarantine" => Some(Destination::Quarantine),
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

/// A strip of `SCROLL_BAR` points at the edge of anything that scrolls, with the
/// handle filling it when there is something to scroll.
///
/// `solid` is the preset that takes that space rather than floating over the
/// content. Its handle is drawn in the widget background colour, which is pale
/// grey on a near white track and comes out invisible; the foreground colour is
/// what makes a handle that can be seen.
fn install_scroll_style(ctx: &egui::Context) {
    ctx.style_mut(|style| {
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = SCROLL_BAR;
        style.spacing.scroll.bar_inner_margin = 0.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        style.spacing.scroll.foreground_color = true;
    });
}

/// Spacing used between the sections of a view, so they are consistent.
const SECTION_GAP: f32 = 14.0;

/// Height of the boxes on the scan row. Fixed, so the row is flush and nothing
/// the pointer does can change it. Sized to the tallest of the three, which is
/// the matching box at four rows.
const SCAN_ROW_HEIGHT: f32 = 100.0;

/// Border and inner margin `Frame::group` adds around its contents, so the
/// arithmetic below is about outer widths.
const FRAME_EXTRA: f32 = 14.0;

/// One picture in a set, and the strip of them a set row holds: the picture, the
/// keep control and the three lines under it.
const TILE: egui::Vec2 = egui::vec2(156.0, 118.0);
const TILE_STRIP_HEIGHT: f32 = 214.0;

/// The line above the strip: the button to keep the whole set.
const SET_HEADER_HEIGHT: f32 = 28.0;

/// What a set row takes. The list places the rows it is not drawing by this, and
/// a row is built to exactly it, so a row can never be a few points out and shift
/// the content under a scroll that is already running.
fn set_row_height(ui: &egui::Ui) -> f32 {
    TILE_STRIP_HEIGHT + SCROLL_BAR + SET_HEADER_HEIGHT + ui.spacing().item_spacing.y + FRAME_EXTRA
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
) -> f32 {
    let mut content_width = 0.0;
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(size.x);
            ui.set_min_height(size.y);
            content_width = ui.scope(contents).response.rect.width();
        });
    });
    content_width
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

/// A label that takes the width it needs rather than breaking to fit.
fn unwrapped(text: egui::RichText) -> egui::Label {
    egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend)
}

/// What the scan screen is showing.
#[derive(Debug, Default, Clone)]
struct ScanState {
    total: u64,
    done: u64,
    /// Rows committed, and how many the writer has been handed. A pass can read
    /// thousands of files and have nothing to write, and then there is nothing
    /// outstanding.
    written: u64,
    to_write: u64,
    per_sec: u64,
    unchanged: u64,
    removed: u64,
    failures: Vec<(String, String)>,
    finished: Option<String>,
}

pub struct App {
    view: View,
    folder: Option<PathBuf>,
    db_path: Option<PathBuf>,
    recurse: bool,
    ignore_colour: bool,
    /// How far apart two pictures may be and still count as the same one, as a
    /// share of the hash. The presets set this; the slider overrides them.
    sensitivity: f64,

    running: Option<Run>,
    scan: ScanState,
    /// The search for duplicates, while it is running, and how far it has got.
    searching: Option<std::sync::mpsc::Receiver<Found>>,
    search_progress: Option<matching::Progress>,
    /// Set to stop the search. It is looked at between the pieces of the work.
    search_cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,

    sets: Vec<DuplicateSet>,
    /// The keep mark per set, held here and not in the index: a review session is
    /// not a fact about a file on disk.
    keep: HashMap<i64, i64>,
    min_recoverable: i64,

    destination: Destination,
    quarantine_dir: String,
    /// The removal, while it is running, and how far through it is.
    removing: Option<std::sync::mpsc::Receiver<Removal>>,
    removed_so_far: usize,
    to_remove: usize,
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
            recurse: saved.recurse,
            ignore_colour: saved.ignore_colour,
            sensitivity: saved.sensitivity,
            running: None,
            scan: ScanState::default(),
            searching: None,
            search_progress: None,
            search_cancel: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            sets: Vec::new(),
            keep: HashMap::new(),
            min_recoverable: 0,
            destination: Destination::Trash,
            quarantine_dir: String::new(),
            removing: None,
            removed_so_far: 0,
            to_remove: 0,
            cleanup_failures: Vec::new(),
            cleanup_result: None,
            thumbs: Thumbnails::new(),
            selected: None,
            showing: None,
            scroll_to: None,
            list_offset: 0.0,
            list_viewport: 0.0,
            scan_on_open: indexed_already,
            window: saved.window,
            preview_width: saved.preview_width,
            error: None,
            scan_content: vec![0.0; 3],
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
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(folder.display().to_string()).weak(),
                            )
                            .truncate(),
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

    /// Drain whatever the indexer has written since the last frame.
    fn pump_indexer(&mut self, _ctx: &egui::Context) {
        let Some(run) = self.running.as_mut() else {
            return;
        };
        while let Ok(update) = run.updates.try_recv() {
            match update {
                Update::Start { total } => {
                    self.scan = ScanState { total, ..ScanState::default() };
                }
                Update::Progress { done, per_sec, unchanged, removed } => {
                    self.scan.done = done;
                    self.scan.per_sec = per_sec;
                    self.scan.unchanged = unchanged;
                    self.scan.removed = removed;
                }
                Update::Writing { done, total } => {
                    self.scan.written = done;
                    self.scan.to_write = total;
                }
                Update::Failed { path, message } => self.scan.failures.push((path, message)),
                Update::Done { indexed, removed, failed, elapsed_ms } => {
                    self.scan.finished = Some(format!(
                        "indexed {indexed}, removed {removed}, failed {failed}, in {:.1}s",
                        elapsed_ms as f64 / 1000.0
                    ));
                }
                Update::Exited { .. } => {}
            }
        }

        if let Some(Update::Exited { code }) = indexer::poll_exit(run) {
            self.running = None;
            runlog::line(&format!("indexer exited with {code:?}"));
            match code {
                Some(0) => self.load_sets(),
                Some(2) => self.scan.finished = Some(String::from("cancelled")),
                other => {
                    self.error = Some(format!(
                        "the indexer stopped with {}. See imgdedupe.log.",
                        other.map(|c| c.to_string()).unwrap_or_else(|| String::from("no exit code"))
                    ))
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
                    ui.horizontal_top(|ui| {
                        measured[0] = self.folder_section(ui, widths[0]);
                        ui.add_space(SECTION_GAP);
                        measured[1] = self.matching_section(ui, widths[1]);
                        ui.add_space(SECTION_GAP);
                        measured[2] = self.run_section(ui, widths[2]);
                    });
                    self.scan_content = measured;
                    self.progress_section(ui);
                })
            },
        );
    }

    fn folder_section(&mut self, ui: &mut egui::Ui, width: f32) -> f32 {
        let busy = self.busy();
        sized_section(ui, "Folder", egui::vec2(width, SCAN_ROW_HEIGHT), |ui| {
            ui.horizontal(|ui| {
                if ui.add_enabled(!busy, egui::Button::new("Choose folder")).clicked() {
                    if let Some(folder) = crate::folder_picker::pick(self.folder.as_deref()) {
                        self.open_folder(folder);
                    }
                }
                match &self.folder {
                    Some(folder) => ui
                        .add(
                            egui::Label::new(
                                egui::RichText::new(folder.display().to_string()).strong(),
                            )
                            .truncate(),
                        )
                        .on_hover_text(folder.display().to_string()),
                    None => ui.label(egui::RichText::new("none chosen").weak()),
                };
            });
            ui.add_space(6.0);
            let subfolders = ui.add_enabled(
                !busy,
                egui::Checkbox::new(&mut self.recurse, "Include subfolders"),
            );
            if subfolders.changed() {
                self.remember();
            }
        })
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
        self.db_path = Some(db_path);
        self.folder = Some(folder);
        self.sets.clear();
        self.keep.clear();
        self.selected = None;
        self.showing = None;
        self.remember();
        if indexed_already {
            self.load_disposal();
        }
        self.start_scan();
    }

    /// Write everything the window was left set to, so the next run opens the
    /// same way.
    fn remember(&self) {
        crate::settings::Settings {
            folder: self.folder.clone(),
            recurse: self.recurse,
            sensitivity: self.sensitivity,
            ignore_colour: self.ignore_colour,
            window: self.window,
            preview_width: self.preview_width,
        }
        .save();
    }

    fn matching_section(&mut self, ui: &mut egui::Ui, width: f32) -> f32 {
        let busy = self.busy();
        sized_section(
            ui,
            "What counts as a duplicate",
            egui::vec2(width, SCAN_ROW_HEIGHT),
            |ui| {
            ui.spacing_mut().slider_width = 150.0;
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
                ui.label(egui::RichText::new("presets").weak());
                for level in [Strictness::Strict, Strictness::Balanced, Strictness::Loose] {
                    let target = level.thresholds(self.ignore_colour).percent();
                    let selected = (self.sensitivity - target).abs() < 0.5;
                    if ui
                        .add_enabled(!busy, egui::SelectableLabel::new(selected, level.label()))
                        .clicked()
                    {
                        self.sensitivity = target;
                        changed = true;
                    }
                }
            });
            ui.add_space(4.0);
            ui.label(
                egui::RichText::new(match self.sensitivity {
                    s if s <= 5.0 => "Exact copies and re-encodes. Misses most rotations.",
                    s if s <= 12.0 => "Resizes, re-encodes and most rotations.",
                    _ => "Heavier edits too, and it will start offering pairs that are not duplicates.",
                })
                .weak(),
            );
            ui.add_space(6.0);
            changed |= ui
                .add_enabled(
                    !busy,
                    egui::Checkbox::new(
                        &mut self.ignore_colour,
                        "Match colour with grayscale",
                    ),
                )
                .on_hover_text(
                    "A colourised copy and its grayscale original count as duplicates",
                )
                .changed();
            if changed {
                self.remember();
            }
            },
        )
    }

    fn run_section(&mut self, ui: &mut egui::Ui, width: f32) -> f32 {
        // Cancel covers the indexing and the search, which are the two a person
        // waits through. It does not cover a removal: files are going, and
        // stopping halfway leaves a job half done with nothing said about it.
        let stoppable = self.running.is_some() || self.searching.is_some();
        let busy = self.busy();
        let have_folder = self.folder.is_some();

        sized_section(ui, "Run", egui::vec2(width, SCAN_ROW_HEIGHT), |ui| {
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
            if ui
                .add_enabled(
                    !busy && self.db_path.is_some(),
                    egui::Button::new("Find duplicates").min_size(egui::vec2(178.0, 30.0)),
                )
                .clicked()
            {
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
            let bar = |ui: &mut egui::Ui, label: &str, count: u64, out_of: u64| {
                let fraction = if out_of == 0 { 1.0 } else { count as f32 / out_of as f32 };
                ui.add(
                    egui::ProgressBar::new(fraction)
                        .text(format!("{label} {:.0}%", fraction * 100.0))
                        .desired_width(width),
                );
            };
            bar(ui, "read", self.scan.done, self.scan.total);
            ui.add_space(4.0);
            bar(ui, "written", self.scan.written, self.scan.to_write);
            if searching {
                ui.add_space(4.0);
                match self.search_progress {
                    Some(progress) => bar(
                        ui,
                        &format!("looking for duplicates: {}", progress.step.label()),
                        progress.done,
                        progress.total,
                    ),
                    None => bar(ui, "looking for duplicates", 0, 4),
                }
            }
            ui.add_space(6.0);
            egui::Grid::new("scan counts")
                .num_columns(4)
                .spacing([24.0, 4.0])
                .show(ui, |ui| {
                    counter(ui, "indexed", self.scan.done);
                    counter(ui, "unchanged", self.scan.unchanged);
                    counter(ui, "removed", self.scan.removed);
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
        let steps = send.clone();
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let asked = std::sync::Arc::clone(&stop);
        std::thread::spawn(move || {
            let result = headless::open_index(&db_path).and_then(|conn| {
                matching::find_sets_reporting(&conn, thresholds, &asked, &|progress| {
                    let _ = steps.send(Found::Progress(progress));
                })
            });
            let _ = send.send(match result {
                Ok(Some(sets)) => Found::Sets(sets),
                Ok(None) => Found::Cancelled,
                Err(err) => Found::Failed(format!("{err:#}")),
            });
        });
        self.search_progress = None;
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
                Found::Progress(progress) => self.search_progress = Some(progress),
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

        // Nothing was found, so there is nothing to review. Say so and stay here.
        if sets.is_empty() {
            self.sets.clear();
            self.scan.finished = Some(String::from("no duplicates found."));
            return;
        }

        for set in &sets {
            if let Some(member) = set.members.iter().find(|member| member.auto_keep) {
                self.keep.insert(set.set_id, member.file_id);
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

        let visible: Vec<usize> = self
            .sets
            .iter()
            .enumerate()
            .filter(|(_, set)| set.recoverable_bytes() >= self.min_recoverable)
            .map(|(index, _)| index)
            .collect();
        let reclaimable: i64 = visible
            .iter()
            .filter(|index| self.keep.contains_key(&self.sets[**index].set_id))
            .map(|index| self.sets[*index].recoverable_bytes())
            .sum();

        egui::TopBottomPanel::top("review toolbar").show_inside(ui, |ui| {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{} sets", visible.len())).strong());
                ui.label(
                    egui::RichText::new(format!("{:.1} MB to reclaim", reclaimable as f64 / 1e6))
                        .weak(),
                );

                ui.separator();
                ui.label("hide under");
                let mut megabytes = self.min_recoverable as f64 / 1_000_000.0;
                ui.spacing_mut().slider_width = 120.0;
                if ui
                    .add(egui::Slider::new(&mut megabytes, 0.0..=50.0).suffix(" MB"))
                    .changed()
                {
                    self.min_recoverable = (megabytes * 1_000_000.0) as i64;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // A set is in the cleanup when it has a keeper. Every set has
                    // one to start with, and "keep all" is what takes it away.
                    let keepers = visible
                        .iter()
                        .filter(|index| self.keep.contains_key(&self.sets[**index].set_id))
                        .count();
                    let go = egui::Button::new(
                        egui::RichText::new("Clean up").strong().color(egui::Color32::WHITE),
                    )
                    .fill(egui::Color32::from_rgb(60, 110, 180))
                    .min_size(egui::vec2(120.0, 28.0));
                    if ui
                        .add_enabled(keepers > 0, go)
                        .on_hover_text(format!(
                            "go to step 3 with the {keepers} sets that have a keeper"
                        ))
                        .clicked()
                    {
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
            if let Some(offset) =
                scroll_to_show(row, row_height, spacing, self.list_offset, self.list_viewport)
            {
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
            db::set_meta(&conn, "quarantine_dir", &self.quarantine_dir)
        });
        if let Err(err) = result {
            runlog::line(&format!("the cleanup choice could not be written: {err:#}"));
        }
    }

    fn load_disposal(&mut self) {
        let Some(db_path) = &self.db_path else {
            return;
        };
        let read = headless::open_index(db_path).and_then(|conn| {
            Ok((db::get_meta(&conn, "disposal")?, db::get_meta(&conn, "quarantine_dir")?))
        });
        match read {
            Ok((disposal, quarantine)) => {
                if let Some(choice) = disposal.as_deref().and_then(Destination::from_name) {
                    self.destination = choice;
                }
                if let Some(folder) = quarantine {
                    self.quarantine_dir = folder;
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
        if let Some(set_id) = holder {
            self.keep.insert(set_id, file_id);
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
    fn preselect_first_keeper(&mut self) {
        self.selected = self.sets.first().and_then(|set| self.keep.get(&set.set_id).copied());
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
                    let keeping = self.keep.get(&set_id) == Some(&member.file_id);
                    if ui
                        .add_enabled(!keeping, egui::Button::new("Keep this one"))
                        .clicked()
                    {
                        self.keep.insert(set_id, member.file_id);
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
                ui.add(unwrapped(egui::RichText::new(&member.rel_path).weak()))
                    .on_hover_text(&member.rel_path);
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
        let chosen = self.keep.get(&set_id).copied();

        // Built to a fixed height rather than measured afterwards. The list places
        // the rows it is not drawing by this number, and a row that came out any
        // other height would move the content under a scroll already in progress.
        let size = egui::vec2(ui.available_width(), set_row_height(ui));
        ui.allocate_ui(size, |ui| {
        ui.set_min_size(size);
        egui::Frame::group(ui.style())
            .show(ui, |ui| {
                ui.set_height(
                    TILE_STRIP_HEIGHT + SCROLL_BAR + SET_HEADER_HEIGHT + ui.spacing().item_spacing.y,
                );
                ui.allocate_ui(egui::vec2(ui.available_width(), SET_HEADER_HEIGHT), |ui| {
                ui.set_min_height(SET_HEADER_HEIGHT);
                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let keep_all = egui::Button::new("keep all")
                            .min_size(egui::vec2(90.0, SET_HEADER_HEIGHT - 4.0));
                        if ui.add(keep_all).clicked() {
                            self.keep.remove(&set_id);
                        }
                    });
                });
                });

                scrolled(
                    ui,
                    egui::Id::new(("set bar", set_id)),
                    false,
                    TILE.x,
                    egui::ScrollArea::horizontal().id_salt(("set", set_id)),
                    |area, ui| {
                        area.show(ui, |ui| {
                            ui.horizontal_top(|ui| {
                                for member in &members {
                                    self.member_tile(ui, set_id, member, chosen, root);
                                }
                            });
                        })
                    },
                );
            });
        });
    }

    /// One image in a set: the picture, whether it is the one being kept, and the
    /// two facts that decide it.
    fn member_tile(
        &mut self,
        ui: &mut egui::Ui,
        set_id: i64,
        member: &imgdedupe_core::matching::Member,
        chosen: Option<i64>,
        root: &std::path::Path,
    ) {
        let kept = chosen == Some(member.file_id);
        let showing = self.selected == Some(member.file_id);
        let keep_colour = egui::Color32::from_rgb(90, 180, 110);

        ui.allocate_ui(egui::vec2(TILE.x, TILE_STRIP_HEIGHT), |ui| {
            ui.set_height(TILE_STRIP_HEIGHT);
            ui.vertical(|ui| {
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
                    .inner_margin(2.0);

                let framed = frame.show(ui, |ui| {
                    match self.thumbs.get(member.file_id, thumbs::THUMB_EDGE, root, &member.rel_path)
                    {
                        Some(texture) => ui.add(
                            egui::Image::new(&texture)
                                .fit_to_exact_size(TILE)
                                .sense(egui::Sense::click()),
                        ),
                        None => ui.add_sized(
                            TILE,
                            egui::Label::new(egui::RichText::new("...").weak())
                                .sense(egui::Sense::click()),
                        ),
                    }
                });
                if showing {
                    ui.painter().rect_stroke(
                        framed.response.rect.expand(3.0),
                        2.0,
                        egui::Stroke::new(3.0_f32, ui.style().visuals.selection.bg_fill),
                    );
                }
                if framed.inner.on_hover_text("show this one on the right").clicked() {
                    self.selected = Some(member.file_id);
                }

                if kept {
                    // Centred under the picture it belongs to, so which one is
                    // being kept can be read off the strip at a glance.
                    ui.add_sized(
                        egui::vec2(TILE.x, ui.spacing().interact_size.y),
                        egui::Label::new(
                            egui::RichText::new("KEEP").strong().color(keep_colour),
                        ),
                    );
                } else if ui
                    .add(
                        egui::Button::new(egui::RichText::new("keep this").weak())
                            .small()
                            .frame(false),
                    )
                    .clicked()
                {
                    self.keep.insert(set_id, member.file_id);
                }
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
                ui.add(
                    egui::Label::new(egui::RichText::new(&member.rel_path).weak()).truncate(),
                )
                .on_hover_text(&member.rel_path);
            });
        });
    }

    fn cleanup_view(&mut self, ui: &mut egui::Ui) {
        let plan = self.build_plan();
        let sets_in_play = self.sets.iter().filter(|set| self.keep.contains_key(&set.set_id)).count();

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
                        ui.add(
                            egui::ProgressBar::new(done as f32 / total as f32)
                                .text(format!(
                                    "{} {done} of {total}",
                                    match self.destination {
                                        Destination::Quarantine => "moving",
                                        _ => "removing",
                                    }
                                ))
                                .desired_width(210.0),
                        );
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
                                Destination::Quarantine => "Files moved",
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
                    for choice in [Destination::Trash, Destination::Quarantine, Destination::Delete] {
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

                    if self.destination == Destination::Quarantine {
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_enabled(
                                !busy,
                                egui::TextEdit::singleline(&mut self.quarantine_dir)
                                    .hint_text("folder")
                                    .desired_width(190.0),
                            );
                            if ui.add_enabled(!busy, egui::Button::new("choose")).clicked() {
                                if let Some(folder) = crate::folder_picker::pick(None) {
                                    self.quarantine_dir = folder.display().to_string();
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
                    Destination::Quarantine => "Files that will be moved",
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
                                    ui.label(
                                        egui::RichText::new(path)
                                            .color(egui::Color32::from_rgb(200, 80, 80)),
                                    )
                                    .on_hover_text(*why);
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
            Destination::Quarantine => Disposal::Quarantine(PathBuf::from(&self.quarantine_dir)),
        }
    }

    /// What the keep marks imply. Rebuilt every frame so the count on the button
    /// is always the count of what the button does. A set whose keeper has been
    /// taken away by "keep all" is left alone entirely.
    fn build_plan(&self) -> Plan {
        let mut sets: Vec<(Vec<imgdedupe_core::matching::Member>, bool)> = Vec::new();
        for set in &self.sets {
            let chosen = self.keep.get(&set.set_id).copied();
            let members = set
                .members
                .iter()
                .map(|member| {
                    let mut member = member.clone();
                    member.auto_keep = chosen == Some(member.file_id);
                    member
                })
                .collect();
            sets.push((members, chosen.is_some()));
        }
        cleanup::plan_from_sets(sets.iter().map(|(members, wanted)| (members.as_slice(), *wanted)))
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
        std::thread::spawn(move || {
            let result = cleanup::apply_reporting(&root, &plan, &disposal, &|done| {
                let _ = steps.send(Removal::Progress(done));
            });
            let _ = send.send(match result {
                Ok(outcome) => Removal::Done(Box::new(outcome)),
                Err(err) => Removal::Failed(format!("{err:#}")),
            });
        });
        self.removing = Some(receive);
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
                Removal::Done(outcome) => {
                    self.finish_cleanup(&outcome);
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
        }
    }

    fn finish_cleanup(&mut self, outcome: &cleanup::Outcome) {
        // Every file that would not go, by name and by reason. A cleanup that
        // quietly removes nothing is what this is here to explain.
        for (path, message) in &outcome.failed {
            runlog::line(&format!("  could not remove {path}: {message}"));
        }
        let forgotten = self.forget(&outcome.removed);
        runlog::line(&format!(
            "cleanup finished: {} removed, {} failed, {:.1} MB, {forgotten} rows dropped",
            outcome.removed.len(),
            outcome.failed.len(),
            outcome.bytes_freed as f64 / 1_000_000.0
        ));
        self.cleanup_result = Some(format!(
            "removed {} files, freed {:.1} MB, {} failed, {forgotten} dropped from the index",
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

    /// Take the files that were removed out of the index. Deleted or moved, they
    /// are not at those paths any more, and an index that still lists them offers
    /// duplicates of files that are gone.
    fn forget(&mut self, removed: &[String]) -> usize {
        let Some(db_path) = self.db_path.clone() else {
            runlog::line("nothing was dropped from the index: no index is open");
            return 0;
        };
        if removed.is_empty() {
            return 0;
        }
        let dropped = db::open_for_notes(&db_path).and_then(|mut conn| {
            let tx = conn.transaction()?;
            let dropped = db::delete_paths(&tx, removed)?;
            tx.commit()?;
            drop(conn);

            // Rebuilding costs a copy of the whole index, so it happens here and
            // only here: a cleanup is the one thing that leaves enough behind to
            // be worth it, and only when it actually dropped rows.
            if dropped > 0 {
                db::compact(&db_path)?;
            }
            Ok(dropped)
        });
        match dropped {
            Ok(count) => count,
            Err(err) => {
                self.fail(&format!("the index still lists the removed files: {err:#}"));
                0
            }
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

    fn app_with_one_set() -> App {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![DuplicateSet {
            set_id: 7,
            members: vec![member(1, "a.jpg", 500), member(2, "b.jpg", 300)],
        }];
        app.keep.insert(7, 1);
        app
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
        let mut app = app_with_one_set();
        app.sets.push(DuplicateSet {
            set_id: 9,
            members: vec![member(3, "c.jpg", 400), member(4, "d.jpg", 100)],
        });
        assert_eq!(app.keep.get(&7), Some(&1), "the fixture starts keeping the first");

        app.selected = Some(2);
        app.keep_selected();
        assert_eq!(app.keep.get(&7), Some(&2), "it did not move the keeper");

        app.selected = Some(4);
        app.keep_selected();
        assert_eq!(app.keep.get(&9), Some(&4), "the other set was not given a keeper");
        assert_eq!(app.keep.get(&7), Some(&2), "it changed a set it was not on");

        app.selected = None;
        app.keep_selected();
        assert_eq!(app.keep.len(), 2, "nothing was selected and something changed");
    }

    /// Starting a second pass while one is going means nothing, so everything
    /// that would start one is off while any of the three is running.
    #[test]
    fn nothing_that_starts_work_is_offered_while_work_is_going() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        assert!(!app.busy(), "a window that has done nothing is busy");

        let (_send, receive) = std::sync::mpsc::channel();
        app.searching = Some(receive);
        assert!(app.busy(), "the search for duplicates does not count as busy");
        app.searching = None;

        let (_send, receive) = std::sync::mpsc::channel();
        app.removing = Some(receive);
        assert!(app.busy(), "removing files does not count as busy");
    }

    /// A cleanup where nothing could be removed leaves everything as it was, on
    /// the page where another destination can be chosen, and says which files.
    #[test]
    fn a_cleanup_that_removed_nothing_stays_put_and_names_the_files() {
        let mut app = app_with_one_set();
        app.view = View::Cleanup;

        app.finish_cleanup(&cleanup::Outcome {
            removed: Vec::new(),
            failed: vec![("b.jpg".to_string(), "no recycle bin here".to_string())],
            bytes_freed: 0,
        });

        assert_eq!(app.view, View::Cleanup, "it left the page with the destinations on it");
        assert_eq!(app.sets.len(), 1, "the sets went even though the files did not");
        assert_eq!(app.keep.get(&7), Some(&1), "the keeper was thrown away");
        assert_eq!(app.cleanup_failures.len(), 1);
        assert_eq!(app.cleanup_failures[0].0, "b.jpg");
    }

    /// A cleanup that removed everything is over: the sets are gone and so is the
    /// page for them.
    #[test]
    fn a_cleanup_that_removed_everything_leaves_the_page() {
        let mut app = app_with_one_set();
        app.view = View::Cleanup;

        app.finish_cleanup(&cleanup::Outcome {
            removed: vec!["b.jpg".to_string()],
            failed: Vec::new(),
            bytes_freed: 300,
        });

        assert_eq!(app.view, View::Scan);
        assert!(app.sets.is_empty());
        assert!(app.cleanup_failures.is_empty());
    }

    /// Some went and some did not. What went comes out of the sets, what did not
    /// stays on screen to be tried another way.
    #[test]
    fn a_cleanup_that_half_worked_keeps_what_is_still_there() {
        let mut app = app_with_one_set();
        app.sets[0].members.push(member(3, "c.jpg", 200));
        app.view = View::Cleanup;

        app.finish_cleanup(&cleanup::Outcome {
            removed: vec!["b.jpg".to_string()],
            failed: vec![("c.jpg".to_string(), "no recycle bin here".to_string())],
            bytes_freed: 300,
        });

        assert_eq!(app.view, View::Cleanup);
        assert_eq!(app.sets.len(), 1, "the set lost more than the file that went");
        let left: Vec<&str> =
            app.sets[0].members.iter().map(|member| member.rel_path.as_str()).collect();
        assert_eq!(left, vec!["a.jpg", "c.jpg"], "the file that went is still listed");
        assert_eq!(app.cleanup_failures.len(), 1);
    }

    /// How the last pass ended is about that pass. Starting another clears it, or
    /// a new search sits under the word "cancelled" from the one before it.
    #[test]
    fn starting_a_pass_clears_what_the_last_one_ended_with() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("index.sqlite");
        imgdedupe_core::db::open(&db_path).expect("an index");

        let mut app = App::from_settings(crate::settings::Settings::default());
        app.folder = Some(dir.path().to_path_buf());
        app.db_path = Some(db_path);
        app.scan.finished = Some(String::from("cancelled"));
        app.error = Some(String::from("something went wrong before"));

        app.load_sets();
        assert_eq!(app.scan.finished, None, "the search kept the last outcome on screen");
        assert_eq!(app.error, None, "the search kept the last error on screen");

        // And an indexing pass clears it too.
        app.scan.finished = Some(String::from("cancelled"));
        app.start_scan();
        assert_eq!(app.scan.finished, None, "the scan kept the last outcome on screen");
    }

    /// A search that finds nothing leaves the window where it is. There is
    /// nothing to review, and the Review tab stays shut.
    #[test]
    fn finding_no_duplicates_does_not_open_the_review() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.view = View::Scan;

        app.accept_sets(Vec::new());

        assert_eq!(app.view, View::Scan, "it went to the review with nothing in it");
        assert!(!app.have_sets(), "the tabs would be open on an empty list");
        assert_eq!(app.selected, None);
        assert_eq!(app.scan.finished.as_deref(), Some("no duplicates found."));
    }

    /// The preview pane opens on the keeper of the first set, so the review view
    /// starts on a picture rather than on an empty pane.
    #[test]
    fn the_first_sets_keeper_is_what_the_preview_starts_on() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![
            DuplicateSet { set_id: 7, members: vec![member(1, "a.jpg", 500)] },
            DuplicateSet { set_id: 9, members: vec![member(3, "c.jpg", 400)] },
        ];
        app.keep.insert(7, 1);
        app.keep.insert(9, 3);

        app.preselect_first_keeper();
        assert_eq!(app.selected, Some(1));

        app.keep.remove(&7);
        app.preselect_first_keeper();
        assert_eq!(app.selected, None, "a first set with no keeper selected something");

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
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![
            DuplicateSet {
                set_id: 7,
                members: vec![member(1, "a.jpg", 500), member(2, "b.jpg", 300)],
            },
            DuplicateSet { set_id: 9, members: vec![member(3, "c.jpg", 400)] },
        ];
        let visible = vec![0, 1];

        app.selected = Some(1);
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, Some(2));

        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, Some(3), "the end of a set did not cross into the next");

        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, Some(3), "the end of the list moved somewhere");

        app.walk(&visible, Direction::PreviousSet);
        assert_eq!(app.selected, Some(1));

        app.selected = None;
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.selected, None, "nothing was selected and something moved");
    }

    /// The keys follow what is on screen. A set hidden by the size filter is not
    /// somewhere the preview can walk into.
    #[test]
    fn walking_skips_sets_the_filter_is_hiding() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![
            DuplicateSet { set_id: 7, members: vec![member(1, "a.jpg", 500)] },
            DuplicateSet { set_id: 8, members: vec![member(2, "b.jpg", 300)] },
            DuplicateSet { set_id: 9, members: vec![member(3, "c.jpg", 400)] },
        ];

        app.selected = Some(1);
        app.walk(&[0, 2], Direction::Forward);
        assert_eq!(app.selected, Some(3));
    }

    /// A row the cursor keys reached must be brought into sight, and one already
    /// in sight must not shift the list under the pointer.
    #[test]
    fn a_row_off_the_screen_is_scrolled_to_and_one_on_it_is_left_alone() {
        let (height, spacing, viewport) = (240.0, 8.0, 700.0);
        let show = |row: usize, offset: f32| scroll_to_show(row, height, spacing, offset, viewport);

        assert_eq!(show(1, 0.0), None, "a row already in sight was scrolled to");

        // Two rows fit in 700, so row two runs from 496 to 736 and hangs off the
        // bottom by 36. Only that much moves, not a whole row.
        assert_eq!(show(2, 0.0), Some(36.0));

        // Row three starts at 744 and ends at 984, past the 700 on screen.
        assert_eq!(show(3, 0.0), Some(984.0 - viewport));

        // Scrolled down past row one, which starts at 248.
        assert_eq!(show(1, 400.0), Some(248.0));

        // The first row never scrolls to a negative place.
        assert_eq!(show(0, 100.0), Some(0.0));
    }

    #[test]
    fn walking_asks_for_the_row_it_moved_to_to_be_shown() {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = vec![
            DuplicateSet { set_id: 7, members: vec![member(1, "a.jpg", 500)] },
            DuplicateSet { set_id: 9, members: vec![member(2, "b.jpg", 400)] },
        ];
        let visible = vec![0, 1];

        app.selected = Some(1);
        app.walk(&visible, Direction::Forward);
        assert_eq!(app.scroll_to, Some(1));

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
        let mut app = app_with_one_set();
        app.sets.push(DuplicateSet {
            set_id: 8,
            members: (10..16).map(|id| member(id, "wide.jpg", 100)).collect(),
        });

        let mut taken = Vec::new();
        let _ = ctx.run(Default::default(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                for index in 0..app.sets.len() {
                    let placed = set_row_height(ui);
                    let before = ui.next_widget_position().y;
                    app.set_row(ui, index, std::path::Path::new("."));
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
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("index.sqlite");
        imgdedupe_core::db::open(&db_path).expect("an index");

        let mut app = App::from_settings(crate::settings::Settings::default());
        app.db_path = Some(db_path.clone());
        app.destination = Destination::Quarantine;
        app.quarantine_dir = String::from("D:\\held");
        app.remember_disposal();

        let mut opened = App::from_settings(crate::settings::Settings::default());
        opened.db_path = Some(db_path);
        assert_eq!(opened.destination, Destination::Trash, "the test started from the default");
        opened.load_disposal();
        assert_eq!(opened.destination, Destination::Quarantine);
        assert_eq!(opened.quarantine_dir, "D:\\held");
    }

    #[test]
    fn an_index_that_has_never_been_cleaned_up_keeps_the_safe_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("index.sqlite");
        imgdedupe_core::db::open(&db_path).expect("an index");

        let mut app = App::from_settings(crate::settings::Settings::default());
        app.db_path = Some(db_path);
        app.load_disposal();
        assert_eq!(app.destination, Destination::Trash);
        assert_eq!(app.quarantine_dir, "");
    }

    /// Moving files to a folder is not removing them, and the button that does it
    /// says so.
    #[test]
    fn the_button_says_what_the_chosen_destination_actually_does() {
        assert_eq!(Destination::Quarantine.verb(), "Move");
        assert_eq!(Destination::Trash.verb(), "Remove");
        assert_eq!(Destination::Delete.verb(), "Remove");
    }

    #[test]
    fn every_cleanup_choice_survives_being_written_and_read_back() {
        for choice in [Destination::Trash, Destination::Quarantine, Destination::Delete] {
            assert_eq!(Destination::from_name(choice.name()), Some(choice));
        }
        assert_eq!(Destination::from_name("something else"), None);
    }

    /// What was actually painted, so a scroll bar that is reserved but drawn in a
    /// colour nobody can see counts as missing. Twice now it has been invisible
    /// while the space for it was there.
    fn painted_rects() -> Vec<egui::epaint::RectShape> {
        let ctx = egui::Context::default();
        install_scroll_style(&ctx);
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
        install_scroll_style(&ctx);
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
        install_scroll_style(&ctx);

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
        let app = app_with_one_set();
        let plan = app.build_plan();
        assert_eq!(plan.files(), 1);
        assert_eq!(plan.removals[0].rel_path, "b.jpg");
        assert_eq!(plan.bytes(), 300);
    }

    #[test]
    fn moving_the_keep_mark_moves_what_gets_removed() {
        let mut app = app_with_one_set();
        app.keep.insert(7, 2);
        let plan = app.build_plan();
        assert_eq!(plan.removals[0].rel_path, "a.jpg");
    }

    #[test]
    fn keeping_everything_in_a_set_removes_nothing_from_it() {
        let mut app = app_with_one_set();
        app.keep.remove(&7);
        assert_eq!(app.build_plan().files(), 0, "a set with no keeper produced removals");
    }

    #[test]
    fn the_review_state_is_not_written_to_the_index() {
        // The marks live on the app and the plan is built from them alone, which
        // is what makes a review session something the database never hears about.
        let mut app = app_with_one_set();
        app.db_path = None;

        let plan = app.build_plan();
        assert_eq!(plan.files(), 1, "the plan needed a database to be built");
        assert_eq!(app.keep.len(), 1);
    }

    #[test]
    fn saved_settings_reach_the_window() {
        let saved = crate::settings::Settings {
            folder: Some(PathBuf::from("/photos")),
            recurse: false,
            sensitivity: 7.5,
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
        assert_eq!(app.sensitivity, 7.5, "the sensitivity was not restored");
        assert!(app.ignore_colour, "the colour setting was not restored");
        assert!(app.db_path.is_some(), "the index path was not derived");
    }

    /// A folder that has been indexed before is brought up to date without being
    /// asked. One that has not been indexed waits for the button.
    #[test]
    fn a_folder_with_an_index_is_scanned_on_opening_and_one_without_is_not() {
        let dir = tempfile::tempdir().expect("tempdir");
        let folder = dir.path().to_path_buf();
        let settings = |folder: &std::path::Path| crate::settings::Settings {
            folder: Some(folder.to_path_buf()),
            ..crate::settings::Settings::default()
        };

        let app = App::from_settings(settings(&folder));
        assert!(!app.scan_on_open, "there is no index to bring up to date");

        std::fs::write(headless::default_db_path(&folder), b"").expect("index");
        let app = App::from_settings(settings(&folder));
        assert!(app.scan_on_open, "the index was there and was left alone");
    }

    /// Picking a folder starts a pass on it, index or no index. Whatever was on
    /// screen belonged to the folder before it and goes.
    #[test]
    fn picking_a_folder_starts_a_pass_and_drops_what_the_last_one_found() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut app = app_with_one_set();
        app.selected = Some(1);

        app.open_folder(dir.path().to_path_buf());

        assert_eq!(app.folder.as_deref(), Some(dir.path()));
        assert!(app.sets.is_empty(), "the sets from the last folder are still here");
        assert!(app.keep.is_empty());
        assert_eq!(app.selected, None);
        assert!(
            app.running.is_some() || app.error.is_some(),
            "no pass was started for the folder that was picked"
        );
    }

    #[test]
    fn no_saved_settings_leaves_the_window_empty() {
        let app = App::from_settings(crate::settings::Settings::default());
        assert_eq!(app.folder, None);
        assert_eq!(app.db_path, None);
        assert!(app.recurse);
    }

    #[test]
    fn strictness_presets_widen_in_order() {
        let strict = Strictness::Strict.thresholds(false);
        let balanced = Strictness::Balanced.thresholds(false);
        let loose = Strictness::Loose.thresholds(false);
        assert!(strict.max_bits < balanced.max_bits);
        assert!(balanced.max_bits < loose.max_bits);
        assert!(strict.max_ring < loose.max_ring);
    }

    #[test]
    fn the_colour_setting_reaches_the_thresholds() {
        assert!(!Strictness::Balanced.thresholds(false).ignore_colour);
        assert!(Strictness::Balanced.thresholds(true).ignore_colour);
    }

    #[test]
    fn the_app_starts_on_the_balanced_setting() {
        let app = App::default();
        assert!((app.sensitivity - Thresholds::balanced().percent()).abs() < 0.01);
    }

    #[test]
    fn a_preset_button_moves_the_slider_to_that_preset() {
        // The presets are points on the slider, not a separate control, so the
        // two can never disagree about what the match will use.
        for level in [Strictness::Strict, Strictness::Balanced, Strictness::Loose] {
            let sensitivity = level.thresholds(false).percent();
            assert_eq!(
                Thresholds::at(sensitivity).max_bits,
                level.thresholds(false).max_bits,
                "{} does not round-trip through the slider",
                level.label()
            );
        }
    }

    #[test]
    fn the_slider_reaches_wider_and_narrower_than_every_preset() {
        let widest = Strictness::Loose.thresholds(false);
        assert!(
            Thresholds::at(matching::MAX_SENSITIVITY).max_bits > widest.max_bits,
            "the slider cannot go past the loosest preset"
        );
        let narrowest = Strictness::Strict.thresholds(false);
        assert!(Thresholds::at(0.5).max_bits < narrowest.max_bits);
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
        for choice in [Destination::Trash, Destination::Quarantine, Destination::Delete] {
            app.destination = choice;
            let selected = [Destination::Trash, Destination::Quarantine, Destination::Delete]
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

        app.destination = Destination::Quarantine;
        app.quarantine_dir = String::from("held");
        assert_eq!(app.disposal(), Disposal::Quarantine(PathBuf::from("held")));
    }

    #[test]
    fn every_destination_has_a_label_and_a_note() {
        for choice in [Destination::Trash, Destination::Quarantine, Destination::Delete] {
            assert!(!choice.label().is_empty());
            assert!(!choice.note().is_empty());
        }
        assert!(
            Destination::Delete.note().contains("cannot be undone"),
            "the permanent option does not say so"
        );
    }
}
