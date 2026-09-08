# Issue 9: a keep mark means keep this one, and nothing else

A mark today means "this one survives and the rest of the set goes", so a set
nobody has reached loses everything in it. It becomes a mark and no more than
that: a set nobody has marked loses nothing, and clearing a whole set out is
something a person says on purpose, with a button, while the set says so on the
screen.

## What a set can be

Every rule in the issue follows from these four states.

| the set                  | what a cleanup takes from it |
|--------------------------|------------------------------|
| nothing marked           | nothing                      |
| something marked         | everything not marked        |
| marked "clean all files" | everything                   |
| ignored                  | nothing                      |

## Tasks

- [ ] 1. What a cleanup takes from a set
- [ ] 2. Sets in timestamp order, and a name for the best candidate
- [ ] 3. Toggling a mark, and the end of multi-select
- [ ] 4. The buttons under a set
- [ ] 5. Auto-mark to keep
- [ ] 6. "automatically mark to keep" on the scan page
- [ ] 7. A set marked for clearing out is drawn faded and red-grey
- [ ] 8. Named tests for each of the above
- [ ] 9. Update `docs/tests.md`
- [ ] 10. Run the named tests, then build

## 1. What a cleanup takes from a set

`crates/imgdedupe-core/src/cleanup.rs`

- `plan_from_sets` stops reading a flag on each member and takes each set with
  what the review says about it: `(&[Member], Fate)`.
- `Fate` is `Keeping(&[i64])` or `CleanAll`. Keeping nothing is a set nobody has
  reached and contributes nothing, which is the rule that reverses. `CleanAll`
  contributes every file.
- An ignored set is not passed in at all, which is what happens today.

## 2. Sets in timestamp order, and a name for the best candidate

`crates/imgdedupe-core/src/matching.rs`

- `Member::auto_keep` means two things in two files: in the search it is "the
  best candidate in this set", and the window overwrites it with "the person
  marked this one". It becomes `best`, which is what the search means, and
  nothing writes to it afterwards.
- Members are sorted by `mtime_ms`, oldest first, with `rel_path` breaking a tie,
  rather than by the best candidate and then the path.
- `DuplicateSet::recoverable_bytes` counts everything but the keeper, which is
  not a rule any more, and nothing outside its own test calls it. It goes.

## 3. Toggling a mark, and the end of multi-select

`crates/imgdedupe/src/app.rs`, `crates/imgdedupe/src/notes.rs`

- The loop that marks each set's best candidate when a search arrives goes.
  Choosing which picture the preview opens on stays: that is the cursor, not a
  mark.
- `keep_selected` always toggles: marked becomes unmarked, unmarked becomes
  marked, and marking one never unmarks another. The branch on `multi_select`
  goes with it.
- `Keep::All` goes. `Keep::One` and `Keep::Several` stay, and a set keeping
  nothing has no entry, as now.
- The `multi_select` field, its checkbox and `remember_multi_select` go, and
  `notes.rs` stops reading the key. An index that holds one is left alone.

## 4. The buttons under a set

`crates/imgdedupe/src/app.rs`

- A set of the ids a person has marked "clean all files in this set", held in
  the window beside the ignored ones. Not written to the index: the issue calls
  it an internal flag, and it is a decision about one sitting.
- `SetAction::KeepAll` clears the set's marks.
- `SetAction::KeepNone` turns the flag on and off. Its button reads "undo" while
  the flag is on.
- Every button under a flagged set stays usable, unlike an ignored one.
- `SetAction::Ignore` on a flagged set clears the flag as it ignores the set.
- `build_plan` hands each set its `Fate` instead of writing `auto_keep` onto its
  members.

## 5. Auto-mark to keep

`crates/imgdedupe/src/app.rs`

- A button reading "auto-mark to keep", where the "allow multi-select" checkbox
  was.
- For every set that is neither ignored nor flagged, it marks the best candidate
  unless that picture is already marked. It never unmarks anything and never
  touches the other pictures in a set.

## 6. "automatically mark to keep" on the scan page

`crates/imgdedupe/src/app.rs`, `crates/imgdedupe/src/notes.rs`

- A checkbox under "Automatically rescan when opening this index", kept in the
  folder's index under `auto_mark` like the other choices.
- Unticked and unusable while automatic rescanning is off; usable, and ticked or
  unticked by the person, while it is on.
- When it is on, the end of a scan does what the button does.

## 7. A set marked for clearing out is drawn faded and red-grey

`crates/imgdedupe/src/thumbs.rs`, `crates/imgdedupe/src/app.rs`

- The pictures of a flagged set are drawn at 25% opacity, the way an ignored
  set's are.
- They are also drawn in red-grey: `(R + G + B) / 3`, with red at twice that, to
  start with. A tint cannot do this — drawing multiplies, and grey is not a
  multiple of the colours — so it is a texture of its own, made in the loading
  path from the same reduced picture and held under a key that says which of the
  two it is. Nothing asks for it until a set is flagged.

## 8. Named tests for each of the above

- A search that finds sets marks nothing.
- Members of a set come back oldest first.
- Space toggles the selected picture's mark on, and off again.
- Marking a second picture keeps both.
- A cleanup takes nothing from a set with no marks, everything but the marks
  from a set with some, everything from a flagged set, and nothing from an
  ignored one.
- "keep all" leaves the set with no marks.
- "keep none" reads "undo" while the flag is on, and every button under the set
  is usable; only "unignore" is usable under an ignored one.
- "ignore" on a flagged set leaves it ignored and not flagged.
- "auto-mark to keep" marks the best candidate in a set with nothing marked, and
  adds it to a set that marks something else without disturbing that.
- "auto-mark to keep" marks nothing in an ignored or flagged set.
- The scan-page checkbox is off and unusable while automatic rescanning is off,
  usable when it is on, and comes back from the index when the folder is opened
  again.
- A scan with it on leaves every set marking its best candidate.
- A flagged set's pictures are drawn faded and red-grey.

## 9. Update `docs/tests.md`

Every test above, with what it is for.

## 10. Run the named tests, then build

The suite, then `scripts\build.bat`.
