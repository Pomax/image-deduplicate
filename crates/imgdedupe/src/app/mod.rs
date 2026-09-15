use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::Result;
use eframe::egui;
use imgdedupe_core::cleanup::{self, Disposal, Plan};
use imgdedupe_core::db;
use imgdedupe_core::matching::{self, DuplicateSet, Thresholds};
use imgdedupe_core::runlog;
use imgdedupe_core::scan;

use crate::constants::*;
use crate::headless;
use crate::indexer::{self, Run, Update};
use crate::thumbs::{self, Thumbnails};

mod cleanup_page;
mod dates;
mod marks;
mod progress;
mod review_page;
mod scan_page;
mod searching;
mod state;
mod widgets;

use self::cleanup_page::*;
use self::dates::*;
use self::progress::*;
use self::state::*;
use self::widgets::*;

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
        .with_inner_size([WINDOW_DEFAULT_WIDTH, WINDOW_DEFAULT_HEIGHT])
        .with_min_inner_size([WINDOW_MIN_WIDTH, WINDOW_MIN_HEIGHT])
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
    index: imgdedupe_core::catalogue::Catalogue,
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
    /// Set when new sets arrive, so the next frame of the review draws the list
    /// from its first set.
    list_to_top: bool,
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
            match_whole_frame: MATCH_WHOLE_PICTURES_DEFAULT,
            match_corners: MATCH_PARTIALS_DEFAULT,
            within_a_folder: ONLY_MATCH_WITHIN_FOLDERS_DEFAULT,
            ignored: std::collections::HashSet::new(),
            plan: cleanup::Plan::default(),
            opened_with_an_index: false,
            comparing: false,
            something_moved: false,
            question: None,
            sets_before: Vec::new(),
            kept_before: std::collections::HashSet::new(),
            auto_rescan: AUTOMATICALLY_RESCAN_DEFAULT,
            auto_mark: AUTOMATICALLY_MARK_TO_KEEP_DEFAULT,
            mark_on_arrival: false,
            asking: None,
            scanned_since_asking: false,
            index: imgdedupe_core::catalogue::Catalogue::start(),
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
            move_dir: MOVE_FOLDER_DEFAULT.to_string(),
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
            list_to_top: false,
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
            ui.add_space(WINDOW_BAR_VERTICAL_PADDING);
            ui.horizontal(|ui| {
                let ready = self.have_sets();
                // A review holding nothing but sets somebody has said are not
                // copies has nothing for a cleanup to do, so there is nowhere
                // for that tab to go.
                let anything_to_clean = self.sets.iter().any(|set| !self.is_ignored(set));
                let tabs = [
                    (View::Scan, SCAN_TAB_LABEL, true),
                    (View::Review, REVIEW_TAB_LABEL, ready),
                    (View::Cleanup, CLEANUP_TAB_LABEL, ready && anything_to_clean),
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
            ui.add_space(WINDOW_BAR_VERTICAL_PADDING);
        });

        if let Some(error) = self.error.clone() {
            egui::TopBottomPanel::bottom("error").show(ctx, |ui| {
                ui.add_space(WINDOW_BAR_VERTICAL_PADDING);
                ui.horizontal(|ui| {
                    ui.colored_label(ERROR_MESSAGE_TEXT_COLOUR, error);
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(DISMISS_ERROR_BUTTON_LABEL).clicked() {
                            self.error = None;
                        }
                    });
                });
                ui.add_space(WINDOW_BAR_VERTICAL_PADDING);
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
            _ => egui::Margin::symmetric(CONTENT_MARGIN, CONTENT_VERTICAL_MARGIN),
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
            ctx.request_repaint_after(WORK_PROGRESS_REPAINT_INTERVAL);
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
}

#[cfg(test)]
#[path = "../tests/app.rs"]
mod tests;
