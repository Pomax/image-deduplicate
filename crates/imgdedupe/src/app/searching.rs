use super::*;

impl App {
    /// What a search says while it runs, and what the reading of an index says
    /// on the way to one. The same bar draws both: reading an index on opening
    /// a folder is the same work a search would otherwise have done itself.
    pub(super) fn note_search_progress(&mut self, progress: matching::Progress) {
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
    pub(super) fn remember(&self) {
        self.settings().save();
    }

    /// What the next run would be started from. The sensitivity is not in it: it
    /// is a decision about the pictures on screen, not a preference.
    pub(super) fn settings(&self) -> crate::settings::Settings {
        crate::settings::Settings {
            folder: self.folder.clone(),
            previous: self.previous.clone(),
            recurse: self.recurse,
            ignore_colour: self.ignore_colour,
            window: self.window,
            preview_width: self.preview_width,
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
    pub(super) fn load_sets(&mut self) {
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
    pub(super) fn pump_search(&mut self, ctx: &egui::Context) {
        let Some(receive) = &self.searching else {
            return;
        };
        ctx.request_repaint_after(WORK_PROGRESS_REPAINT_INTERVAL);
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
                    self.scan.finished = Some(String::from(CANCELLED_TEXT));
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
                NO_FILES_IN_FOLDER_TEXT
            } else {
                NO_DUPLICATES_FOUND_TEXT
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
        // These are other sets, so the list starts at the first of them rather
        // than wherever the last review was left scrolled to.
        self.list_to_top = true;
        self.view = View::Review;
    }

    /// Take what a folder's index says about itself. Every one of these belongs
    /// to the folder rather than to the program, and a folder that has never
    /// said leaves what is on screen alone.
    pub(super) fn take_notes(&mut self, notes: crate::notes::Notes) {
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
    pub(super) fn settle_the_boxes(&mut self) {
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
    pub(super) fn remember_ways_of_matching(&self) {
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

    /// The review is finished, so what was written down for it goes: the sets and
    /// the marks both.
    ///
    /// Nothing is left to be marked. The cleanup took everything that was not
    /// marked, so the marks name every file still there, which says nothing, and
    /// there are no sets for them to be marks in.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    pub(super) fn the_review_is_over(&mut self) {
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
    pub(super) fn give_up_the_saved_review(&mut self) {
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
    pub(super) fn look_at_the_folder(&mut self) {
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
    pub(super) fn decide_what_opening_the_folder_does(&mut self, moved: bool) {
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
    pub(super) fn ask_about_the_saved_review(&mut self, ctx: &egui::Context) {
        let Some(question) = self.question else {
            return;
        };
        let mut answered = None;
        egui::Window::new(PREVIOUS_SESSION_TITLE)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.add_space(PANEL_VERTICAL_PADDING);
                ui.label(question.wording());
                ui.add_space(PREVIOUS_SESSION_BUTTONS_GAP);
                ui.horizontal(|ui| {
                    if ui.button(FINISH_PREVIOUS_SESSION_BUTTON_LABEL).clicked() {
                        answered = Some(true);
                    }
                    if ui.button(RESCAN_BUTTON_LABEL).clicked() {
                        answered = Some(false);
                    }
                });
                ui.add_space(PANEL_VERTICAL_PADDING);
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
}
