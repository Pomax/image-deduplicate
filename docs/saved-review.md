# A review that survives being closed

Two things get written to the folder's index as they happen: what a review has
marked to keep, and the sets a search found. Closing the window loses neither,
and opening the folder again goes straight back to the review rather than
searching for what is already known.

They are separate pieces of work. The marks stand on their own — they are worth
keeping whether or not the sets ever are — so they finish before the sets start.

## When the stored sets still stand

The same files under the same settings give the same sets, so running the search
again is time spent to arrive where the index already is.

| what changed                    | what happens                      |
|---------------------------------|-----------------------------------|
| nothing                         | straight to the review, no search |
| a file added, removed or edited | ask: load, or scan                |
| the search settings             | ask: load, or scan                |
| both                            | ask, in the words for the files   |

A file with the same name and a different timestamp is a different picture, so it
counts as changed the same way an added or removed one does. Neither case is
decided for the person: both put the question up.

## What the settings are

Sensitivity, match whole pictures, match partials, match colour with grayscale,
and only match within folders. These are what a search reads, so these are what
the sets have to be checked against.

## Tasks

How the index reaches the disk, which everything below depends on:

- [ ] 1. The writer holds a connection to the index file
- [ ] 2. Changes are applied to the file, not the whole database written out
- [ ] 3. The whole-file write goes

What a review has marked to keep:

- [ ] 1. The `keep` table
- [ ] 2. `set_keep` and `kept` against it
- [ ] 3. `SetKeep` and `Kept` on the manager
- [ ] 4. The window writes the marks as they change
- [ ] 5. The window reads the marks back when a folder opens

The sets a search found:

- [ ] 6. The `duplicate_sets` table
- [ ] 7. `store_sets`, `stored_sets` and `clear_sets` against it
- [ ] 8. `StoreSets`, `StoredSets` and `ClearSets` on the manager
- [ ] 9. The search settings in the index, and what the sets were found under
- [ ] 10. The window writes the sets a search found
- [ ] 11. The window reads the sets back and builds them again

What the window does on opening a folder:

- [ ] 12. Learning whether the folder's content has changed
- [ ] 13. Skipping the search when the stored sets still stand
- [ ] 14. Asking when the settings or the files differ
- [ ] 15. Clearing the stored sets on "scan"
- [ ] 16. Clearing the stored sets after a cleanup

What a cleanup would take, which the review counts as it is drawn:

- [ ] 17. The `plan` field on `App`
- [ ] 18. The `replan` method on `App`
- [ ] 19. The drawing reads the field
- [ ] 20. `replan` where the marks change
- [ ] 21. `replan` where the sets change
- [ ] 22. `replan` where the ignored pairs change
- [ ] 23. `forget_members` drops marks whose files are gone

Finishing:

- [ ] 24. Named tests for each of the above
- [ ] 25. Update `docs/tests.md`
- [ ] 26. Run the named tests, then build

## 1. The `keep` table

In `crates/imgdedupe-core/src/db.rs`, in the `SCHEMA` string, as
`CREATE TABLE IF NOT EXISTS`.

One column, `file_id`, primary key, referencing `files(id)` with
`ON DELETE CASCADE`. A row per picture a review has marked to keep. It cascades
for the reason a row of `ignore` does: a mark on a file that is gone is not a
mark.

Nothing has to be migrated. An index with no such table is a folder with no
previous session, which is what `kept` says about it.

## 2. `set_keep` and `kept` against it

In `crates/imgdedupe-core/src/db.rs`, beside `ignore`, `unignore` and `ignored`,
each taking `&Connection`:

- `set_keep(conn, file_ids: &[i64]) -> Result<()>`. Deletes every row of `keep`
  and inserts one per id: the marks are the review's, and this is all of them.
- `kept(conn) -> Result<Vec<i64>>`. Every `file_id` in `keep`. An index with no
  such table reads as empty rather than failing, the way `ignored` does.

## 3. `SetKeep` and `Kept` on the manager

In `crates/imgdedupe-core/src/index.rs`, one variant of the `Job` enum and one
method on the `Index` struct for each, beside `Ignore` and `Ignored`. `SetKeep`
calls `sync` afterwards, the way `Job::Ignore` does, so the file on disk catches
up with what is held.

## 4. The window writes the marks as they change

In `crates/imgdedupe/src/app.rs`.

- A method on `App`, `remember_keep`, gathering every marked `file_id` out of the
  field `self.keep` and handing it to `Index::set_keep`.
- Called from every place a person changes `self.keep`: the methods
  `keep_selected`, `keep_only_selected` and `auto_mark_to_keep`, the
  `SetAction::KeepAll` and `SetAction::KeepNone` arms of the match in `set_row`,
  and the "Keep this one" button in the preview pane.
- Not from the places that clear the marks wholesale — opening another folder,
  starting a pass, taking a search result, finishing a cleanup. Those are tasks
  of their own below.
- Written whole rather than as a difference: the marks are small, and a write
  that replaces them cannot drift from what is on screen.

## 5. The window reads the marks back when a folder opens

In `crates/imgdedupe/src/app.rs`.

- A variant of the `Opened` enum, `Kept(Vec<i64>)`, sent by the thread the method
  `ask_the_index` starts, beside `Notes`, `Ignored` and `Index`.
- A field on `App`, `kept_before`, a `HashSet<i64>`, holding them until there is
  something to hang them on: the marks are per file, the window holds them per
  set, and a set does not exist until a search has run or a stored one is read.
- A method on `App`, `take_up_the_marks`, called from `accept_sets`, turning them
  into entries of `self.keep`: for each set, the members whose `file_id` is in
  `kept_before`.
- A mark whose file is in no set is left where it is. It is not on screen, so
  there is nothing for it to be, and the next write takes it out.

## 6. The `duplicate_sets` table

In `crates/imgdedupe-core/src/db.rs`, in the `SCHEMA` string, as
`CREATE TABLE IF NOT EXISTS`.

Three columns: `set_id`, `file_id`, and `at`, the set's place in the list the
review shows. Primary key over `set_id` and `file_id`; `file_id` references
`files(id)` with `ON DELETE CASCADE`. A row per picture per set.

`at` is there because the review is a list and somebody left off partway down it.
Read back in a different order it is a different list, which is not where they
left off. The pictures inside a set need no such column: they are ordered by
timestamp, and sorting them again gives the same order.

It cascades because a set that has lost a picture is not that set.

Nothing has to be migrated here either. No table is no previous session, which is
also the state a cleanup leaves behind, since that drops it.

## 7. `store_sets`, `stored_sets` and `clear_sets` against it

In `crates/imgdedupe-core/src/db.rs`, each taking `&Connection`:

- `store_sets(conn, sets: &[(i64, Vec<i64>)]) -> Result<()>`. Deletes every row
  of `duplicate_sets` and inserts one per picture per set, `at` being the set's
  place in the slice.
- `stored_sets(conn) -> Result<Vec<(i64, Vec<i64>)>>`. The rows gathered back
  into a set and its pictures, in `at` order. An index with no such table reads
  as empty.
- `clear_sets(conn) -> Result<()>`. Drops `duplicate_sets`. No table is no
  previous session, and the schema puts it back the next time one is needed.

## 8. `StoreSets`, `StoredSets` and `ClearSets` on the manager

In `crates/imgdedupe-core/src/index.rs`, one variant of the `Job` enum and one
method on the `Index` struct for each. The two that write call `sync` afterwards.

## 9. The search settings in the index, and what the sets were found under

Two keys join the eight in `crates/imgdedupe/src/notes.rs`: `sensitivity` and
`ignore_colour`. Field on the `Notes` struct, constant for the key, read in the
function `read`, applied in the method `take_notes`, written where the window
writes the other settings. Every search setting is then in the index, and a
folder opens on its own.

Beside them, the five search settings as they stood when the sets were stored:
`sets_sensitivity`, `sets_whole_frame`, `sets_corners`, `sets_ignore_colour`,
`sets_within_a_folder`, written by the same method that writes the sets. They are
what task 14 compares against when somebody moves the slider and closes the
window without searching again.

An index holding sets but not those rows was written before this existed, and
reads as a folder that has never been searched.

## 10. The window writes the sets a search found

In `crates/imgdedupe/src/app.rs`, in the method `accept_sets`, which is where a
search hands its result over: every set as its `set_id` and the `file_id` of each
member, in the order the review will show them, to `Index::store_sets`, and the
five settings rows beside them.

A search that found nothing writes an empty list and the settings, which is a
folder searched and found clean rather than one never searched.

## 11. The window reads the sets back and builds them again

In `crates/imgdedupe/src/app.rs`.

- A variant of the `Opened` enum, `Sets(Vec<(i64, Vec<i64>)>)`, sent by the same
  thread, and a field on `App`, `sets_before`, holding them.
- What is stored is file ids and nothing else. Each `matching::Member` is built
  from the `matching::Image` of the same `file_id`, out of the pictures the same
  open already read, so nothing about a picture is written down twice and nothing
  can drift between the two.
- A stored set that has lost members — its rows went when the files did — is
  dropped if fewer than two are left. One picture is not a set of copies.

## 12. Learning whether the folder's content has changed

In `crates/imgdedupe/src/app.rs` and `crates/imgdedupe-core/src/scan.rs`.

Opening a folder that holds stored sets lists the folder and compares it against
the index, which is the `walk` and `diff` of a pass and nothing after them: no
file is read, decoded or fingerprinted. On the folder this was measured against
that is half a second against nine and a half thousand files.

The answer is one of two things: every file in the folder is in the index with
the same size and timestamp, or it is not. `diff` already works this out; what is
needed is a way to ask for it without the pass that follows.

## 13. Skipping the search when the stored sets still stand

In `crates/imgdedupe/src/app.rs`.

A folder that comes back with stored sets, settings that match, and nothing added,
removed or written opens the review on those sets. The method `load_sets` is not
called at all, and neither is a pass.

The lamps say what happened rather than reporting a search that did not run. The
button "Find duplicates" still runs one when it is pressed.

## 14. Asking when the settings or the files differ

In `crates/imgdedupe/src/app.rs`.

- A field on `App` holding the question while it stands, and a modal window with
  two buttons: "load", which loads the stored sets for the person to work with,
  and "scan", which discards them and runs a new scan.
- It goes up for either reason, and says which:
  - the settings do not match what the sets were found under —
    "Previous session found. Load or rescan?"
  - a file was added, removed or written since —
    "Previous session found but folder content has changed. Load previous
    session or rescan?"
- Both at once says the second: the files are the stronger statement, and the
  settings can be changed back where the files cannot.
- Nothing is decided for the person: until they answer, the review is not opened
  and no search is started.
- "load" puts the settings back to what the sets were found under: the
  sensitivity slider and the four checkboxes for the ways of matching. The sets
  on screen are then the sets those settings give, and the window is not
  claiming otherwise.

## 15. Clearing the stored sets on "scan"

In `crates/imgdedupe/src/app.rs`: "scan" calls `Index::clear_sets` and takes the
five `meta` rows out with `Index::forget_meta`, then starts a pass, so an index
never holds sets nobody stands behind.

The marks are left. A mark is about a file, not about a set, and the files are
still there; the next search hangs them on whatever sets it finds.

"load" leaves the sets exactly as they are, including a file the pass found
changed: the person was asked and said to carry on.

## 16. Clearing the stored sets after a cleanup

In `crates/imgdedupe/src/app.rs`: the end of a cleanup does what "scan" does to
the sets and the settings rows. The files it took are gone, so the sets it took
them from describe a folder that no longer exists. That holds whether the cleanup
removed everything it planned to or only some of it.

The marks of the files that went go with them, because the rows cascade. The
marks of the files that stayed are left.

## 17. The `plan` field on `App`

`App::selected_for_removal` calls `App::build_plan`, which walks every set and
allocates a `cleanup::Removal` per file, each holding a cloned `rel_path`
`String`. The review toolbar calls it while it draws, so that is the whole plan
built again on every frame: on a folder of 9342 pictures with nothing marked,
nine thousand allocations and nine thousand string clones sixty times a second
for a review nobody is touching.

In `crates/imgdedupe/src/app.rs`: a field on the `App` struct, `plan`, a
`cleanup::Plan`, empty to begin with, beside the field `keep` it follows from.

Derived whole rather than edited a piece at a time. A plan with `add` and
`remove` called from each interaction is a second account of what the marks say,
and the two drift the first time a site is missed. Deriving it from the marks
cannot drift; doing that on the change rather than on the frame is what makes it
cheap.

It depends on three fields of `App`: `keep`, the marks; `sets`, what was found;
and `ignored`, the pairs said not to be copies. Every place any of them changes
is a place the plan is worked out again, and there is nowhere else.

## 18. The `replan` method on `App`

In `crates/imgdedupe/src/app.rs`: a method `replan(&mut self)`, which is
`self.plan = self.build_plan();` and nothing else. `build_plan` stays exactly as
it is — it is the rule, and the rule does not change here.

## 19. The drawing reads the field

In `crates/imgdedupe/src/app.rs`:

- The method `selected_for_removal` returns `(self.plan.files(),
  self.plan.bytes())` rather than building one.
- The method `cleanup_view` reads the field rather than building one.

## 20. `replan` where the marks change

In `crates/imgdedupe/src/app.rs`, after the change in each of:

- the method `keep_selected`
- the method `keep_only_selected`
- the method `auto_mark_to_keep`
- the `SetAction::KeepAll` arm of the match in the method `set_row`
- the `SetAction::KeepNone` arm of the same match
- the "Keep this one" button in the method `preview_pane`

## 21. `replan` where the sets change

In `crates/imgdedupe/src/app.rs`, after the change in each of:

- the method `accept_sets`, on both paths: the one that finds nothing and clears
  the sets, and the one that takes them
- the method `open_folder`
- the method `start_scan`
- the method `cancel_work`
- the method `finish_cleanup`
- the method `forget_members`
- the method `folder_section`, where unticking the box that keeps an index throws
  the sets away with it

## 22. `replan` where the ignored pairs change

In `crates/imgdedupe/src/app.rs`, after the change in each of:

- the method `ignore_set`
- the method `unignore_set`
- the method `ask_the_index`, which clears them for the folder being opened
- the `Opened::Ignored` arm of the match in the method `hear_the_index`

## 23. `forget_members` drops marks whose files are gone

In `crates/imgdedupe/src/app.rs`, in the method `forget_members`.

It drops the pictures a cleanup took out of every set, drops the sets left with
fewer than two, and drops the entries of `keep` whose set has gone. It does not
touch the entry of a set that survived, so that entry can go on naming a file
that is no longer in it, and a `Keep::Several` can be left holding one id, which
the type says never happens.

Each surviving set's entry is cut down to the ids still in that set and put back
through `as_keep`, so `One` and `Several` mean what they say and no mark names a
file that has gone.

## 24. Named tests for each of the above

- Marks written by one window are read back by the next.
- A set stored by one window is read back by the next, with its pictures, in the
  order it was shown in.
- An index with neither table reads as having no stored sets and no marks, rather
  than failing.
- A stored set that has lost a picture to a deleted file comes back without it,
  and one left with a single picture does not come back at all.
- Opening a folder whose sets still stand runs no search and no pass.
- Opening a folder whose settings differ opens the dialog, runs no search, and
  opens no review until it is answered.
- Opening a folder where a file was added, removed or written since opens the
  dialog, saying the folder's content has changed rather than the other wording.
- "load" opens the review on the stored sets.
- "scan" leaves no stored sets, no settings rows, and starts a pass.
- A cleanup leaves no stored sets.
- The count beside the sets follows a mark going on and coming off again.
- Ignoring a set takes its pictures out of the count, and unignoring puts them
  back.
- After each kind of interaction, the held plan is the same as one derived from
  the marks then and there. This is the test that catches a missed site: it fails
  for whichever interaction forgot to work the plan out again.
- Drawing the review does not change the plan.
- After a cleanup that removed some of what it planned to, no mark names a file
  that is gone, and no `Keep::Several` holds a single id.

## 25. Update `docs/tests.md`

Every test above, with what it is for.

## 26. Run the named tests, then build

The suite, then `scripts\build.bat`.
