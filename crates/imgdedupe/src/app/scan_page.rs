use super::*;

impl App {
    /// Take whatever the pass has reported since the last frame.
    pub(super) fn pump_indexer(&mut self, _ctx: &egui::Context) {
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

    pub(super) fn scan_view(&mut self, ui: &mut egui::Ui) {
        // Everything on one row: the groups and the buttons are all short and
        // stacking them full width leaves most of the window empty.
        let widths = share_row_width(
            ui.available_width(),
            &self.scan_content,
            SECTION_SPACING_GAP,
        );
        let mut measured = self.scan_content.clone();
        // The three boxes end level with each other, at the height of whichever
        // holds the most. Nothing is a fixed height, so taking a control out
        // takes its space with it.
        let mut tallest = 0.0_f32;
        ui.horizontal_top(|ui| {
            let folder = self.folder_section(ui, widths[0]);
            ui.add_space(SECTION_SPACING_GAP);
            let matching = self.matching_section(ui, widths[1]);
            ui.add_space(SECTION_SPACING_GAP);
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
        ui.add_space(SECTION_SPACING_GAP);
        let step = ui.spacing().interact_size.y * LAMP_LIST_SCROLL_STEP_CONTROL_HEIGHTS;
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
        let dot = |ui: &mut egui::Ui, state: Went| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(LAMP_SLOT_WIDTH, ui.spacing().interact_size.y),
                egui::Sense::hover(),
            );
            match state {
                Went::Happened => {
                    ui.painter()
                        .circle_filled(rect.center(), LAMP_DOT_RADIUS, LAMP_DONE_COLOUR)
                }
                Went::Waiting => {
                    ui.painter()
                        .circle_filled(rect.center(), LAMP_DOT_RADIUS, LAMP_WAITING_COLOUR)
                }
                // Nothing to do rather than not done yet: an empty ring, so a
                // pass that had no new files to read does not read as a pass
                // that failed to read them.
                Went::Skipped => ui.painter().circle_stroke(
                    rect.center(),
                    LAMP_DOT_RADIUS,
                    egui::Stroke::new(LAMP_SKIPPED_RING_WIDTH, LAMP_SKIPPED_RING_COLOUR),
                ),
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
    pub(super) fn how_it_went(&self, lamp: Lamp) -> Went {
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

    pub(super) fn folder_section(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Vec2 {
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
                        PREVIOUS_BUTTON_WIDTH
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
            ui.add_space(SECTION_ROW_GAP);
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
    pub(super) fn note_window(&mut self, ctx: &egui::Context) {
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
    pub(super) fn take_dropped_folder(&mut self, ctx: &egui::Context) {
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
    pub(super) fn open_folder(&mut self, folder: PathBuf) {
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
            self.ignore_colour = MATCH_COLOUR_WITH_GRAYSCALE_DEFAULT;
            self.recurse = INCLUDE_SUBFOLDERS_DEFAULT;
            // Both ways of matching, until this folder's index says otherwise,
            // and the whole folder at once rather than one folder at a time.
            self.match_whole_frame = MATCH_WHOLE_PICTURES_DEFAULT;
            self.match_corners = MATCH_PARTIALS_DEFAULT;
            self.within_a_folder = ONLY_MATCH_WITHIN_FOLDERS_DEFAULT;
            // Whether opening a folder runs a pass is that folder's own answer,
            // and a folder that has not been asked yet has not said yes.
            self.auto_rescan = AUTOMATICALLY_RESCAN_DEFAULT;
            // A folder that has an index arrives with the box already ticked.
            self.keep_index = has_index;
            self.destination = Destination::Trash;
            self.move_dir = MOVE_FOLDER_DEFAULT.to_string();
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
    pub(super) fn open_what_was_left_open(&mut self) {
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
    pub(super) fn hear_the_index(&mut self, ctx: &egui::Context) {
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
            ctx.request_repaint_after(INDEX_ARRIVAL_REPAINT_INTERVAL);
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

    pub(super) fn matching_section(&mut self, ui: &mut egui::Ui, width: f32) -> egui::Vec2 {
        let busy = self.busy();
        let row = self.scan_row;
        sized_section(
            ui,
            "What counts as a duplicate",
            egui::vec2(width, row),
            |ui| {
                ui.spacing_mut().slider_width = SENSITIVITY_SLIDER_WIDTH;
                // The number beside the slider is drawn in a box of this width, and
                // the box would otherwise size to the digits in it. The row's boxes
                // are shared out by what their contents measure, so 5.0 and 30.0
                // would each want a different share and move all three.
                ui.spacing_mut().interact_size.x = SENSITIVITY_PERCENTAGE_BOX_WIDTH;
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
                    ui.spacing_mut().button_padding = egui::vec2(
                        SMALL_BUTTON_HORIZONTAL_PADDING,
                        SMALL_BUTTON_VERTICAL_PADDING,
                    );
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
                ui.add_space(SECTION_ROW_GAP);
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
    pub(super) fn can_cancel(&self) -> bool {
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
                        .min_size(egui::vec2(SCAN_BUTTON_WIDTH, RUN_BUTTON_HEIGHT)),
                );
                if start.clicked() {
                    self.start_scan();
                }
                if ui
                    .add_enabled(
                        stoppable,
                        egui::Button::new("Cancel")
                            .min_size(egui::vec2(CANCEL_BUTTON_WIDTH, RUN_BUTTON_HEIGHT)),
                    )
                    .clicked()
                {
                    self.cancel_work();
                }
            });
            // A button's label sits where the layout puts it, and a row's layout
            // starts at the left, so the width `min_size` adds all lands on the
            // right of the text.
            let wide = egui::vec2(FIND_DUPLICATES_BUTTON_WIDTH, RUN_BUTTON_HEIGHT);
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

    pub(super) fn progress_section(&mut self, ui: &mut egui::Ui) {
        let running = self.running.is_some();
        let searching = self.searching.is_some();
        if !running && !searching && self.scan.total == 0 && self.scan.finished.is_none() {
            return;
        }

        ui.add_space(SECTION_SPACING_GAP);
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
                ui.add_space(SECTION_ROW_GAP);
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
            ui.add_space(PROGRESS_BAR_GAP);
            bar(
                ui,
                "Indexing files",
                self.scan.writing,
                self.scan.indexed,
                self.scan.to_index,
            );
            ui.add_space(PROGRESS_BAR_GAP);
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
            ui.add_space(SECTION_ROW_GAP);
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
                .spacing([SCAN_COUNTS_COLUMN_GAP, SCAN_COUNTS_ROW_GAP])
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
                ui.add_space(SECTION_ROW_GAP);
                ui.label(egui::RichText::new(finished).strong());
            }
            if !self.scan.failures.is_empty() {
                ui.add_space(SECTION_ROW_GAP);
                egui::CollapsingHeader::new(format!(
                    "{} files could not be read",
                    self.scan.failures.len()
                ))
                .show(ui, |ui| {
                    let line = ui.text_style_height(&egui::TextStyle::Body);
                    ui.set_max_height(FAILED_FILES_LIST_MAX_HEIGHT);
                    scrolled(
                        ui,
                        egui::Id::new("scan failures"),
                        true,
                        line,
                        None,
                        egui::ScrollArea::vertical().max_height(FAILED_FILES_LIST_MAX_HEIGHT),
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

    pub(super) fn start_scan(&mut self) {
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
}
