use super::*;

impl App {
    /// Where this folder's duplicates go, and the folder they are moved to. This
    /// belongs to the folder that was scanned rather than to the application:
    /// what is safe to delete outright somewhere is not safe everywhere.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    pub(super) fn remember_disposal(&self) {
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

    pub(super) fn cleanup_view(&mut self, ui: &mut egui::Ui) {
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

                ui.add_space(SECTION_SPACING_GAP);
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
    pub(super) fn disposal(&self) -> Disposal {
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
    pub(super) fn build_plan(&self) -> Plan {
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
    pub(super) fn run_cleanup(&mut self, plan: &Plan) {
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
    pub(super) fn pump_cleanup(&mut self, ctx: &egui::Context) {
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
    pub(super) fn forget_members(&mut self, removed: &[String]) {
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
pub(super) fn discard_index(index: &imgdedupe_core::catalogue::Catalogue) -> usize {
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
fn forget_rows(index: &imgdedupe_core::catalogue::Catalogue, removed: &[String]) -> usize {
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
