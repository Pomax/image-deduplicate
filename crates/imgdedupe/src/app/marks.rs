use super::*;

impl App {
    /// Whether marking a picture adds to what its set keeps. This belongs to the
    /// folder as well: a folder reviewed one picture at a time is reviewed that
    /// Whether every picture in a set has been said not to be a copy of every
    /// other picture in it. A set like that is shown, so it can be seen and
    /// changed back, and nothing else in the program acts on it.
    ///
    /// Every pair, not some: a set where two of five have been separated is
    /// still a set of copies, and the three that are copies still are.
    pub(super) fn is_ignored(&self, set: &DuplicateSet) -> bool {
        if set.members.len() < 2 {
            return false;
        }
        pairs_of(set).all(|pair| self.ignored.contains(&pair))
    }

    /// Say that none of the pictures in this set are copies of each other, and
    /// write that down in the folder's index so the next search knows it too.
    #[cfg_attr(not(feature = "logging"), allow(unused_variables))]
    pub(super) fn ignore_set(&mut self, set_id: i64) {
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
    pub(super) fn unignore_set(&mut self, set_id: i64) {
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
    pub(super) fn take_up_the_marks(&mut self) {
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

    /// Somebody marked these pictures to keep. Two things follow from that and
    /// they follow together: the marks are written down, and what a cleanup would
    /// take is worked out again.
    pub(super) fn marked(&mut self, file_ids: &[i64]) {
        self.write_marks(file_ids, true);
        self.replan();
    }

    /// And the other way: somebody took the mark off these.
    pub(super) fn unmarked(&mut self, file_ids: &[i64]) {
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
    pub(super) fn walk(&mut self, visible: &[usize], direction: Direction) {
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
    pub(super) fn unmark_everything(&mut self) {
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
    pub(super) fn keep_everything(&mut self) {
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
    pub(super) fn auto_mark_to_keep(&mut self) {
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
    pub(super) fn keep_selected(&mut self) {
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
    pub(super) fn keep_only_selected(&mut self) {
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
    pub(super) fn selected_for_removal(&self) -> (usize, i64) {
        // Asked of the plan rather than worked out again here. What a cleanup
        // takes is one rule, and a count that reads the marks its own way is a
        // second copy of it that can disagree with the button.
        (self.plan.files(), self.plan.bytes())
    }

    /// Work out again what a cleanup would take. Called wherever the marks, the
    /// sets or the ignored pairs change, and nowhere else.
    pub(super) fn replan(&mut self) {
        self.plan = self.build_plan();
    }

    /// The first set that is a set of copies, not simply the first set. A set
    /// nobody calls a set of copies keeps nothing and shows nothing as kept, so
    /// opening the review on a picture in one puts the preview somewhere that
    /// means nothing and gives the cursor keys nowhere sensible to start. A
    /// review of nothing but ignored sets opens on nothing.
    pub(super) fn preselect_first_keeper(&mut self) {
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
    pub(super) fn have_sets(&self) -> bool {
        !self.sets.is_empty()
    }

    /// Whether a pass of any kind is under way. Starting a second one while the
    /// first is going means nothing, so anything that would start one is off
    /// until it is over, whichever of the three it is.
    pub(super) fn busy(&self) -> bool {
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
    pub(super) fn cancel_work(&mut self) {
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
}
