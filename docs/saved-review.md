# A review that survives being closed

Two things get written to the folder's index as they happen: what a review has
marked to keep, and the sets a search found. Closing the window loses neither,
and opening the folder again goes straight back to the review rather than
searching for what is already known.

They are separate pieces of work. The marks stand on their own — they are worth
keeping whether or not the sets ever are — so they finish before the sets start.

Underneath both is the one thing a review is: which files are being kept and
which are going. Every mark, every set, every ignored pair is a statement about
that list, and the window has to hold it correctly at every moment — not work it
out again on each frame that happens to draw, which is what it does today.
Keeping it right across a mark, across a search, across a cleanup, and across
being closed and opened again is the same job as writing it down, so it is
decided here and not somewhere else.

## What opening a folder does

One flow, and it runs every time a folder is opened. Open the index, take its
settings — they are the folder's — and compare the folder against it. The
comparison is the listing and the difference and nothing after them: no file is
read.

Are there differences?

| differences | the box is ticked | a saved review | what happens             |
|-------------|-------------------|----------------|--------------------------|
| yes         | yes               | either         | rescan, at once          |
| yes         | no                | yes            | ask: load, or rescan     |
| yes         | no                | no             | rescan, at once          |
| no          | either            | either         | nothing to rescan        |

The box is "automatically rescan when opening this index". This is what it is
for: it says that when something has moved, do not ask, bring it up to date. With
no saved review there is nothing to protect and nothing to ask about, so the pass
runs either way.

Then, with no rescan left to do — nothing had changed, or the pass has finished,
or they said load — the flow parts on the one thing that can part it:

- a saved review: open it, on the sets the index holds. No search runs.
- no saved review: search, on the pictures the open already read. No pass runs.

The same files under the same settings give the same sets, so running the search
again is time spent to arrive where the index already is.

A file with the same name and a different timestamp is a different picture, so it
counts as a difference the same way an added or removed one does.

The search settings are not part of the comparison. They reach the index only
where a search runs, and opening a folder puts the window on the ones the index
holds, so what the window is set to and what the sets were found under are the
same five values. They cannot disagree, so nothing asks about them.

## What counts as a difference

A file the index does not name is not the same as a file nobody has looked at.

The walk lists every file whose name claims a supported format. The index names
every file that was indexed — sniffed, decoded, fingerprinted. A file that claims
a format and is not one, or is animated, or could not be read, is in the first
list and not the second, so it reads as new on every opening for ever. On the
folder this was found on, three `.tif` files did exactly that, and the question
went up the moment the window opened.

So the index also records the files a pass looked at and did not index: the path,
the size and the timestamp, and nothing else. The comparison then has a third
answer — known, unchanged, and not a picture — which is neither new nor changed.

A pass stops reading those files as well. Today they are read in full on every
pass, which for a video named `.tif` on another machine is the whole file across
the network every time.

## What the settings are

Sensitivity, match whole pictures, match partials, match colour with grayscale,
and only match within folders. These are what a search reads, so these are what
the sets have to be checked against.

## How a change reaches the file

Every table below is written the way the index is written now: the change is
applied to the open index in memory, the caller is answered, and the same call is
handed to the writer thread, which runs it against the connection it holds on the
file. Nothing here writes the index out whole, and nothing waits for the disk.

## Tasks

What a review has marked to keep:

- [x] 1. The `keep` table
- [x] 2. `keep_these`, `unkeep_these` and `kept` against it
- [x] 3. `KeepThese`, `UnkeepThese` and `Kept` on the manager
- [x] 4. The window writes the marks as they change
- [x] 5. The window reads the marks back when a folder opens

The sets a search found:

- [x] 6. The `duplicate_sets` table
- [x] 7. `store_sets`, `stored_sets` and `clear_sets` against it
- [x] 8. `StoreSets`, `StoredSets` and `ClearSets` on the manager
- [x] 9. The search settings in the index, and what the sets were found under
- [x] 10. The window writes the sets a search found
- [x] 11. The window reads the sets back and builds them again

What the window does on opening a folder:

- [x] 12. Learning whether the folder's content has changed
- [x] 13. Skipping the search when the stored sets still stand
- [x] 14. Asking when the folder has changed or asks to be rescanned
- [x] 15. Clearing the stored sets on "scan"
- [x] 16. Clearing the stored sets after a cleanup

What a cleanup would take, which the review counts as it is drawn:

- [x] 17. The `plan` field on `App`
- [x] 18. The `replan` method on `App`
- [x] 19. The drawing reads the field
- [x] 20. `replan` where the marks change
- [x] 21. `replan` where the sets change
- [x] 22. `replan` where the ignored pairs change
- [x] 23. `forget_members` drops marks whose files are gone

Finishing:

- [x] 24. Named tests for each of the above
- [x] 25. Update `docs/tests.md`
- [x] 26. Run the named tests, then build

What a pass looked at and did not index, found by running it:

- [x] 27. `files` records the ones that are not pictures
- [x] 28. The pass writes that where it decides it
- [x] 29. The comparison stops calling them differences
- [x] 30. Everything that counts `files` counts pictures
- [x] 31. Opening a folder does the least the comparison implies
- [x] 32. The window waits for the writer on the way out
- [x] 33. Named tests for each of the above
- [x] 34. Update `docs/tests.md`
- [x] 35. Run the named tests, then build

## 1. The `keep` table

In `crates/imgdedupe-core/src/db.rs`, as a statement of its own beside the
function that writes it, and not in the `SCHEMA` string. A review is one sitting
and the table holding it is not part of what an index is: it is made where it is
first written and dropped when the sitting is over.

One column, `file_id`, primary key, referencing `files(id)` with
`ON DELETE CASCADE`. A row per picture a review has marked to keep. It cascades
for the reason a row of `ignore` does: a mark on a file that is gone is not a
mark.

No table is a folder nobody has reviewed, which is what `kept` says about it.

## 2. `keep_these`, `unkeep_these` and `kept` against it

In `crates/imgdedupe-core/src/db.rs`, beside `ignore`, `unignore` and `ignored`,
each taking `&Connection`:

- `keep_these(conn, file_ids: &[i64]) -> Result<()>`. Makes `keep` if it is not
  there and inserts a row per id. What somebody just marked, and nothing else
  touched.
- `unkeep_these(conn, file_ids: &[i64]) -> Result<()>`. Deletes a row per id.
  The other half of the same thing.
- `kept(conn) -> Result<Vec<i64>>`. Every `file_id` in `keep`. No such table
  reads as empty rather than failing, the way `ignored` does.
- `clear_keep(conn) -> Result<()>`. Drops `keep`, for the end of a review. The
  table is there for as long as there is a review, and the next mark makes it
  again.

## 3. `KeepThese`, `UnkeepThese` and `Kept` on the manager

In `crates/imgdedupe-core/src/index.rs`, one variant of the `Job` enum and one
method on the `Index` struct for each, beside `Ignore` and `Ignored`, and one for
`ClearKeep` alongside them. `KeepThese` is written the way `Job::Ignore` is:
`db::keep_these` against the open index, the answer back to the caller, and the
same call handed to the writer through `Writer::apply` to be run against the
file. `UnkeepThese` and `ClearKeep` the same.

## 4. The window writes the marks as they change

In `crates/imgdedupe/src/app.rs`.

- Two methods on `App`, `marked` and `unmarked`, each taking the `file_id`s that
  just changed, writing them through `Index::keep_these` or `Index::unkeep_these`
  and working the plan out again.
- Called from every place a person changes `self.keep`: the methods
  `keep_selected`, `keep_only_selected` and `auto_mark_to_keep`, the
  `SetAction::KeepAll` and `SetAction::KeepNone` arms of the match in `set_row`,
  and the "Keep this one" button in the preview pane. Each of them says which
  pictures changed, because each of them knows.
- Not from the places that clear the marks wholesale — opening another folder,
  starting a pass, taking a search result, finishing a cleanup. Those are tasks
  of their own below.
- What changed, not the review. Marking one picture is one row written: the
  index is a file, often on another machine, and every statement against it is a
  journal written and deleted beside it. Redoing the whole review on every click
  cost one of those per mark the review held, and they queue up behind the person
  all session. There is no transaction here — one statement is not a batch.
  Transactions are for what a scan and a search hand over.

## 5. The window reads the marks back when a folder opens

In `crates/imgdedupe/src/app.rs`.

- A variant of the `Opened` enum, `Kept(Vec<i64>)`, sent by the thread the method
  `ask_the_index` starts, beside `Notes`, `Ignored` and `Index`.
- A field on `App`, `kept_before`, a `HashSet<i64>`, holding them until there is
  something to hang them on: the marks are per file, the window holds them per
  set, and a set does not exist until a search has run or a stored one is read.
- A method on `App`, `take_up_the_marks`, called from `accept_sets`, turning them
  into entries of `self.keep`: for each set, the members whose `file_id` is in
  `kept_before`. It runs after `accept_sets` has cleared `self.keep` and before
  the auto-marking, so a mark the person made outranks one the window would have
  added and the auto-marking adds only what is missing.
- A mark whose file is in no set is left where it is. It is not on screen, so
  there is nothing for it to be, and the next write takes it out.
- `kept_before` is not cleared by a pass. `start_scan` empties `self.keep`, and
  the marks are meant to survive that and be hung on whatever the next search
  finds; the field is emptied only where the folder itself changes, in
  `ask_the_index`, which is where the next answer refills it.

## 6. The `duplicate_sets` table

In `crates/imgdedupe-core/src/db.rs`, beside the function that writes it and not
in the `SCHEMA` string, for the reason the `keep` table is not in there either.

Three columns: `set_id`, `file_id`, and `at`, the set's place in the list the
review shows. Primary key over `set_id` and `file_id`; `file_id` references
`files(id)` with `ON DELETE CASCADE`. A row per picture per set.

`at` is there because the review is a list and somebody left off partway down it.
Read back in a different order it is a different list, which is not where they
left off. The pictures inside a set need no such column: they are ordered by
timestamp, and sorting them again gives the same order.

It cascades because a set that has lost a picture is not that set.

No table is no previous session, which is also the state a cleanup leaves behind,
since that drops it.

## 7. `store_sets`, `stored_sets` and `clear_sets` against it

In `crates/imgdedupe-core/src/db.rs`, each taking `&Connection`:

- `store_sets(conn, sets: &[(i64, Vec<i64>)]) -> Result<()>`. Makes
  `duplicate_sets` if it is not there, deletes every row of it and inserts one
  per picture per set, `at` being the set's place in the slice.
- `stored_sets(conn) -> Result<Vec<(i64, Vec<i64>)>>`. The rows gathered back
  into a set and its pictures, in `at` order. No such table reads as empty.
- `clear_sets(conn) -> Result<()>`. Drops `duplicate_sets`. The table exists for
  as long as there is a review to hold, and a search that finds something makes
  it again.

## 8. `StoreSets`, `StoredSets` and `ClearSets` on the manager

In `crates/imgdedupe-core/src/index.rs`, one variant of the `Job` enum and one
method on the `Index` struct for each. The two that write hand their call to the
writer as well, the way `KeepThese` does.

`StoreSets` is the one write here that is not somebody doing something: it is a
search handing over everything it found, which on a folder of any size is
hundreds of rows. Those go in one transaction, the way a pass's records do, so
the file takes one commit rather than one per picture per set.

## 9. The search settings in the index, and what the sets were found under

The index holds the settings a search really ran under, and nothing else. A
control moved and never used has changed nothing, so moving the slider, clicking
a preset or ticking a way of matching writes nothing at all; the five reach the
index where a search hands its sets over, in the same place and at the same
moment as the sets they found.

That is one set of keys, not two. There is no such thing as the folder's setting
apart from what its sets were found under: they are the same fact.

Two keys join the eight in `crates/imgdedupe/src/notes.rs`: `sensitivity` and
`ignore_colour`. Field on the `Notes` struct, constant for the key, read in the
function `read`, applied in the method `take_notes`. The three that already exist
— `match_whole_frame`, `match_corners`, `within_a_folder` — stop being written
when their boxes are ticked and are written with the other two instead.

What stays written on being clicked is `auto_rescan` and `auto_mark`. Those are
not search settings: they say what the folder does when it is opened, and ticking
one is using it.

A folder whose index holds fewer than all five was written before one of them
existed, and reads as a folder that has never been searched.

## 10. The window writes the sets a search found

In `crates/imgdedupe/src/app.rs`, in the method `accept_sets`, which is where a
search hands its result over: every set as its `set_id` and the `file_id` of each
member, in the order the review will show them, to `Index::store_sets`, and the
five settings rows beside them.

A search that found nothing writes an empty list and the settings, which is a
folder searched and found clean rather than one never searched. `accept_sets`
returns early on that path, before it takes the sets, so the write sits above
that return and both paths go through it.

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

A folder that comes back with stored sets, nothing added, removed or written, and
no request to be rescanned on opening, opens the review on those sets. The method
`load_sets` is not called at all, and neither is a pass.

The lamps say what happened rather than reporting a search that did not run. The
button "Find duplicates" still runs one when it is pressed.

## 14. Asking when the folder has changed or asks to be rescanned

In `crates/imgdedupe/src/app.rs`.

- A field on `App` holding the question while it stands, and a modal window with
  two buttons: "load", which loads the stored sets for the person to work with,
  and "scan", which discards them and runs a new scan.
- It goes up for either of two reasons, and says which:
  - the folder asks to be rescanned when it is opened, with nothing about the
    folder changed — "Previous session found. Load or rescan?"
  - a file was added, removed or written since —
    "Previous session found but folder content has changed. Load previous
    session or rescan?"
- Both at once says the second: the files are the stronger statement.
- The pass a folder asks for on opening does not run while the question stands.
  In the method `hear_the_index`, the arm that takes `Opened::Notes` starts one
  as soon as the notes arrive, which is before the sets have been read; a folder
  that turns out to have stored sets is a folder where that pass is what the
  question is asking about, and starting it would throw the review away before
  anybody was asked.
- Nothing is decided for the person: until they answer, the review is not opened
  and no search is started.
- The settings need no putting back. Opening the folder already put the window on
  the ones its index holds, which are the ones its sets were found under.

## 15. Clearing the stored sets on "scan"

In `crates/imgdedupe/src/app.rs`: "scan" calls `Index::clear_sets` and takes the
five `meta` rows out with `Index::forget_meta`, then starts a pass, so an index
never holds sets nobody stands behind.

The marks are left. A mark is about a file, not about a set, and the files are
still there; the next search hangs them on whatever sets it finds.

"load" leaves the sets exactly as they are, including a file the pass found
changed: the person was asked and said to carry on. That is also the answer to a
folder that asked to be rescanned — the request is not a decision, and saying
"load" is the person declining it for this opening. The setting itself is left
ticked; it is what that folder does on opening, and this was one opening.

## 16. Clearing the stored sets after a cleanup

In `crates/imgdedupe/src/app.rs`, in the method `finish_cleanup`: the end of a
cleanup does what "scan" does to the sets and the settings rows. The files it
took are gone, so the sets it took them from describe a folder that no longer
exists. That holds whether the cleanup removed everything it planned to or only
some of it, so it happens on both paths through that method and not only where
nothing failed.

A cleanup on a folder whose index is not being kept deletes the index outright,
through `discard_index`. There is nothing left to clear there and nothing to do.

The marks go with them, through `Index::clear_keep`. A review that has been
carried out is over: what it marked to keep is what is still in the folder, and
holding it as a review of sets that no longer exist says nothing.

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
- the method `take_up_the_marks` from task 5, which puts back the marks a
  previous session left. They are marks like any other and they change what a
  cleanup would take.

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
- wherever task 13 opens the review on the stored sets, which is a review with
  sets and marks in it that no search handed over

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

A test that opens the index file itself waits for the writer first, through
`Index::synced` or by closing the index, the way the tests that read the file
already do. A caller is answered when the open index has the change, not when the
file does.

- Marks written by one window are read back by the next.
- A set stored by one window is read back by the next, with its pictures, in the
  order it was shown in.
- An index with neither table reads as having no stored sets and no marks, rather
  than failing.
- A stored set that has lost a picture to a deleted file comes back without it,
  and one left with a single picture does not come back at all.
- Opening a folder whose sets still stand runs no search and no pass.
- Opening a folder where a file was added, removed or written since opens the
  dialog, saying the folder's content has changed rather than the other wording,
  and opens no review until it is answered.
- A search setting is not written down by being changed, only by a search using
  it, so a folder opened again is set to what it was last searched with.
- Opening a folder that asks to be rescanned and has stored sets opens the
  dialog and starts no pass, and "load" opens the review with the folder still
  asking to be rescanned next time.
- Opening a folder that asks to be rescanned and has no stored sets starts the
  pass as it does now, and puts no dialog up.
- "load" opens the review on the stored sets.
- "scan" leaves no stored sets, no settings rows, and starts a pass.
- A cleanup leaves no stored sets, whether everything it planned to take went or
  only some of it did.
- Marks a previous session left are on the pictures when the review opens, and
  what a cleanup would take counts them.
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

## 27. `files` records the ones that are not pictures

`crates/imgdedupe-core/src/db.rs`

- A column on `files`, saying the pass looked at this file and it is not a
  picture. Every row written before this has it off, which is what every row is
  today: a file that was indexed.
- `files` then means every file the pass has been through. `images` and
  `fingerprints` hold what it made of the ones that were pictures, and the view
  `indexed_images` already joins all three, so the search sees pictures only and
  nothing downstream of it changes.
- `db::Known` carries the flag, so `load_known` gives the diff the path, size,
  timestamp and this, in one read as now.
- A separate table was the other way to do this. It would be a second list of
  files to keep in step with the folder, with its own rule for when a row goes
  and no foreign key to lean on, because these files have no `images` row to
  hang off. The column reuses the key, the size and timestamp, the delete
  cascade, and `delete_paths`.

## 28. The pass writes that where it decides it

`crates/imgdedupe-core/src/scan.rs`

Where a file comes back `NotAnImage`, animated, or `Failed`, a row goes into
`files` with the flag on, its size and its timestamp, through the manager the way
an indexed row does. A file that was one of those and is now a picture is indexed
in the ordinary way and the flag comes off.

The row is what is written, not the reason. Whether it was not a picture, or was
animated, or could not be read, the answer to the next comparison is the same:
this file has been looked at, and looking again would find what it found.

## 29. The comparison stops calling them differences

`crates/imgdedupe-core/src/scan.rs`, in `diff`.

A candidate whose row has the flag on and whose size and timestamp match is left
alone: not to index, not a difference. One whose size or timestamp has moved is a
different file, so the flag means nothing about it and it is read again. One whose
file is gone is removed like any other row.

`folder_is_as_indexed` then answers what it says it answers, and the pass stops
reading those files on every run.

## 30. Everything that counts `files` counts pictures

`crates/imgdedupe-core`, `crates/imgdedupe`.

A count of rows in `files` is no longer a count of pictures. Every place that
counts them is either about pictures, and counts the unflagged, or is about rows
in the index and is left alone. The cleanup's "N dropped from the index" is the
one on screen; `discard_index` and the report are the others to look at.

## 31. Opening a folder does the least the comparison implies

`crates/imgdedupe/src/app.rs`

One flow, as the top of this document has it. Open the index, take its settings,
compare. Differences and the box ticked: the pass, at once. Differences, no box,
a saved review: the question. Differences, no box, no saved review: the pass, at
once. No differences: no pass.

Then a saved review is opened, and a folder without one is searched, on the
pictures the open already read.

- The `Question` enum loses `ThisFolderRescans` and everything that set it. A
  folder that asks to be rescanned and has nothing to rescan is not asked about,
  and one that has something to rescan is rescanned, which is what the box says.
- The one wording left is the one about the folder's content having changed.
- A folder with no saved review is searched on opening, which is what it opened
  for. The pictures are already in memory and no pass has run.

## 32. The window waits for the writer on the way out

`crates/imgdedupe/src/app.rs`, in `on_exit`.

It saves the settings file, cancels a running pass, and stops. It does not close
the index, so every change still on the writer's queue goes with the process. A
review written in the last moments of a session is exactly what is lost.

`Index::close` is what waits, and it is what the tests call. The window calls it
where it ends.

## 33. Named tests for each of the above

- A pass over a folder holding a file that claims a format and is not one writes
  it down as looked at, and a second pass over that folder reads no file at all.
- A folder like that is as indexed: the comparison says no differences, where
  today it says one.
- A file that was not a picture and has been replaced by one is indexed on the
  next pass, and stops being flagged.
- A file that was not a picture and is gone leaves no row behind.
- The count on the cleanup button, and what it says afterwards, are counts of
  pictures on a folder that holds files of both kinds.
- Opening a folder with no saved review and nothing changed searches it, and runs
  no pass.
- Opening a folder with differences and the box ticked runs the pass and asks
  nothing, with a saved review and without one.
- Opening a folder with differences, no box and no saved review runs the pass and
  asks nothing.
- Opening a folder with differences, no box and a saved review asks, and the
  wording is the one about the folder's content.
- A mark made and the window closed at once is in the index file when it is
  opened again.

## 34. Update `docs/tests.md`

Every test above, with what it is for, and the entries for the tests that
described the old flow.

## 35. Run the named tests, then build

The suite, then `scripts\build.bat`.
