# The big source files come apart

Issue 17. Eleven source files carry more than four hundred lines each, one of them five thousand. Each becomes a directory of files named after what is in them.

## What is there now

Line counts, source only. The tests moved out to `src/tests/` already and are not counted here.

| file | lines | becomes |
|---|---|---|
| `crates/imgdedupe/src/app.rs` | 5205 | 10 |
| `crates/imgdedupe-core/src/matching.rs` | 1163 | 6 |
| `crates/imgdedupe-core/src/scan.rs` | 988 | 5 |
| `crates/imgdedupe-core/src/metadata.rs` | 979 | 6 |
| `crates/imgdedupe-core/src/db.rs` | 739 | 5 |
| `crates/imgdedupe-core/src/index.rs` | 738 | 3 |
| `crates/imgdedupe-core/src/features.rs` | 571 | 4 |
| `crates/imgdedupe-core/src/preview.rs` | 538 | 4 |
| `crates/imgdedupe/src/thumbs.rs` | 502 | 3 |
| `crates/imgdedupe-core/src/fingerprint.rs` | 465 | 4 |
| `crates/imgdedupe-core/src/dirlist.rs` | 410 | 3 |

Eleven files become fifty three. Everything else is already under four hundred lines and is left alone: `decode.rs` at 274, `tools.rs` at 265, `indexer.rs` at 256, `icon.rs` at 235, `fonts.rs` at 227, `settings.rs` at 214, `format.rs` at 190, `cleanup.rs` at 150, `memory.rs` at 140, `frames.rs` at 139, `runlog.rs` at 114, `score.rs` at 109, `main.rs` at 98, `notes.rs` at 95, `folder_picker.rs` at 86, the app's `metadata.rs` at 83, `mesa.rs` and `headless.rs` at 28, `lib.rs` at 20.

## The shape

A file `src/foo.rs` keeps its name and gains a directory `src/foo/` beside it. Rust reads a child of `foo` out of `src/foo/`, so `mod parts;` in `src/foo.rs` is `src/foo/parts.rs`. Nothing is renamed to `mod.rs`.

That matters for two reasons. `foo.rs` keeps its `#[cfg(test)] #[path = "tests/foo.rs"] mod tests;`, which is read from the directory `foo.rs` is in, so the test file stays exactly where it is and every test keeps its name. And `docs/tests.md` heads each group of tests with the source file they are about, which is still `src/foo.rs`.

## What changes besides position

A private item in `foo::parts` is not visible in `foo`, and an inherent method on `App` written inside `app::review` is private to `app::review`. Every item that crosses one of the new boundaries becomes `pub(super)`. Nothing becomes `pub`, and nothing leaves the crate that was not already leaving it.

The two platform modules in `dirlist.rs` are both called `imp`, so each gains a `#[path]` attribute to say which file it is. Those two attributes are the only ones this work adds.

`foo.rs` pulls the names back into itself with a plain `use self::parts::*;`. A glob import in a child module sees its parent's private `use` bindings, so `use super::*` at the top of `src/tests/foo.rs` still finds everything it found before, and no test file changes.

## The split, file by file

### `crates/imgdedupe/src/app.rs`

- `app.rs`: `launch`, `start_window`, `struct App`, its `Default`, `from_settings`, the `impl eframe::App`, and the module declarations.
- `app/state.rs`: `View`, `Removal`, `Found`, `Opened`, `Question`, `Went`, `Direction`, `Keep`, `Destination`, `SetAction`, and the free functions over them: `step`, `keeps`, `marked`, `unmarked`, `as_keep`, `pairs_of`.
- `app/progress.rs`: `ScanState`, `Stage`, `SearchState`, `fraction`, `Lamp`, `LAMPS`, `From<scan::Step> for Lamp`.
- `app/widgets.rs`: everything that draws and is not a method on `App`: `arrow`, `scrolled`, `paint_scroll_bar`, `install_style`, `section`, `sized_section`, `counter`, `progress_bar`, `counted`, `clipped_line`, `clipped_line_in`, `unwrapped`, `share_row_width`, `button_width`, `button_row_height`, `tile_strip_height`, `scroll_to_show`, `fitted`, `tile_width`, `set_row_height`, `count_line`, `on_preset`, `duplicate_count`, and the layout constants they read.
- `app/dates.rs`: `file_date` and `civil_from_days`.
- `app/scanning.rs`: the scan page and the folder: `scan_view`, `lamps`, `how_it_went`, `folder_section`, `previous_folders`, `note_window`, `take_dropped_folder`, `open_folder`, `open_what_was_left_open`, `ask_the_index`, `hear_the_index`, `start_scan`, `pump_indexer`, `matching_section`, `run_section`, `progress_section`, `can_cancel`.
- `app/searching.rs`: the search and what comes back from the index: `load_sets`, `pump_search`, `note_search_progress`, `accept_sets`, `search_settings`, `remember_the_sets`, `take_notes`, `settle_the_boxes`, `remember_ways_of_matching`, `look_at_the_folder`, `decide_what_opening_the_folder_does`, `ask_about_the_saved_review`, `open_the_stored_review`, `give_up_the_saved_review`, `the_review_is_over`, `remember`, `settings`.
- `app/review.rs`: the review page: `review_view`, `set_row`, `member_tile`, `strip_offset`, `preview_pane`, `written_beside_it`, `filling_the_window`.
- `app/marks.rs`: what is kept and what is chosen: `is_ignored`, `ignore_set`, `unignore_set`, `take_up_the_marks`, `marked`, `unmarked`, `write_marks`, `walk`, `unmark_everything`, `keep_everything`, `auto_mark_to_keep`, `keep_selected`, `keep_only_selected`, `position`, `selected_for_removal`, `replan`, `preselect_first_keeper`, `have_sets`, `busy`, `cancel_work`.
- `app/cleanup.rs`: the cleanup page and what follows it: `cleanup_view`, `disposal`, `build_plan`, `run_cleanup`, `pump_cleanup`, `finish_cleanup`, `forget_members`, `remember_disposal`, `discard_index`, `forget_rows`.

### `crates/imgdedupe-core/src/matching.rs`

- `matching.rs`: `Thresholds`, the presets, `Member`, `DuplicateSet`, `Image`, `Progress`, `load_images`, `parse_format`, `without`, `sets_from_stored`, `build_sets`.
- `matching/bands.rs`: `BandIndex`, `pairs_in_band`.
- `matching/corners.rs`: the corner shortlist: `CORNER_BITS`, `corner_key`, `table_bits`, `CornerTable`, `pairs_by_corner`, and the constants that size them.
- `matching/grouping.rs`: `Identity`, `folder_of`, `by_folder`, `fold_identical`, `Groups`.
- `matching/compare.rs`: `aspect_ok`, `is_match`, `whole_frame_match`, `same_picture_inside`.
- `matching/search.rs`: `find_sets`, `find_sets_cancellable`, `find_sets_in`, the batching constants, `Timing`.

### `crates/imgdedupe-core/src/scan.rs`

- `scan.rs`: `Step`, `Event`, `Options`, `Summary`, `run`.
- `scan/walk.rs`: `Candidate`, `kept_out`, `walk`, `to_portable_path`, `now_seconds`.
- `scan/diff.rs`: `Diff`, `diff`, `looked_at`, `indexed_so_far`.
- `scan/one.rs`: `Outcome`, `Spent`, `index_one`.
- `scan/readahead.rs`: `ReadAhead` and the constants that pace it.

`run` is not divided. It is one procedure from start to finish, and cutting it in half would put the halves in two files with a shared pile of state between them.

### `crates/imgdedupe-core/src/metadata.rs`

- `metadata.rs`: `Group`, `read`, `one`, `text_of`, `find`, `jpeg_segments`.
- `metadata/tiff.rs`: `from_tiff`, `from_tiff_as`, `where_it_was`, `Table`, the directory tags.
- `metadata/iptc.rs`: `from_iptc`, `from_photoshop`, `iptc_name`.
- `metadata/xmp.rs`: `XMP_MARK`, `xmp_name`, `from_xmp`, `values_inside`.
- `metadata/containers.rs`: `from_png`, `from_gif`, `from_riff`, `from_boxes`.
- `metadata/tags.rs`: `Shape`, `Named`, `tag`, `value_of`, `first_number`, `first_ratio`, `trimmed`, `said_plainly`, `flash`, and the tables of names a photographer reads.

### `crates/imgdedupe-core/src/db.rs`

- `db.rs`: `SCHEMA_VERSION`, `INDEX_FILENAME`, `SCHEMA`, `open_and_migrate`, `into_memory`, `put_the_index_back`.
- `db/migrate.rs`: `migrate_the_copy`, `has_table`, `has_column`, `drop_dead_columns`, `add_new_columns`, `carry_the_stamps_across`.
- `db/meta.rs`: `set_meta`, `forget_meta`, `get_meta`.
- `db/files.rs`: `Known`, `load_known`, `Record`, `upsert`, `Looked`, `not_a_picture`, `delete_paths`, `begin_review`.
- `db/review.rs`: `pair`, `ignore`, `unignore`, `ignored`, the keep table and `keep_these`, `unkeep_these`, `clear_keep`, `kept`, the sets table and `store_sets`, `stored_sets`, `clear_sets`.

### `crates/imgdedupe-core/src/index.rs`

- `index.rs`: `Index`, `OpenIndex`, `Job`, `Reporter`, the `Debug`, and the `impl Index` that is the manager's whole outside.
- `index/serve.rs`: `serve`, `with`, `with_mut`, `open_index`, `close_index`, `compact`, `delete`, `open_the_file`.
- `index/writer.rs`: `Writer`, `Change`, `Errand`, `impl Writer`.

### `crates/imgdedupe-core/src/features.rs`

- `features.rs`: the sizes, `Keypoint`, `features`, `pack`, `unpack`, `smooth`, `box_blur`, `strongest`.
- `features/corners.rs`: `CIRCLE`, `corners`, `arc`, `suppress`, `orientation`.
- `features/describe.rs`: `describe`, `PATTERN`, `distance`.
- `features/agreement.rs`: `agreement`, `paired`, `Arrangement`, `apply`, and the thresholds they read.

### `crates/imgdedupe-core/src/preview.rs`

- `preview.rs`: `Preview`, `find`, `the_way_up`, `byte_order`, `maker`, `Order`, `exif_inside_jpeg`, `from_tiff_directory`.
- `preview/tiff.rs`: the tag numbers, `Entry`, `entries`, `from_tiff`, `in_directory`.
- `preview/jpeg.rs`: `jpeg_at`, `is_a_picture`, `jpeg_length`, `scan_past_entropy`.
- `preview/boxes.rs`: `from_boxes`, `find_start`, `walk_boxes`, `CONTAINERS`.

### `crates/imgdedupe/src/thumbs.rs`

- `thumbs.rs`: `THUMB_EDGE`, `LARGE_EDGE`, `Thumbnails`, its `impl`, `Drop` and `Default`.
- `thumbs/queues.rs`: `Lanes`, `Wanted`, `Queues`, `Tally`, `Key`, `Decoded`, and the worker counts.
- `thumbs/load.rs`: `load` and `the_way_up`.

`impl Thumbnails` stays whole: it is the one thing the window talks to.

### `crates/imgdedupe-core/src/fingerprint.rs`

- `fingerprint.rs`: the version, `Fingerprint`, `fingerprint`, `Hash`, `Words`, the packing, `bands`.
- `fingerprint/hashes.rs`: `grid`, `to_luma`, `variant_hashes`, `transpose_block`, `mirror_horizontal`, `mirror_vertical`, `hash_block`, `median_of`, `dct_low_block`, `cosine_basis`.
- `fingerprint/distance.rs`: `hamming`, `hamming_words`, `hamming_any`, `hamming_any_words`, `ring_weighted`, `ring_weight`, `ring_distance`, `ring_distance_weighted`.
- `fingerprint/colour.rs`: `ring_stats`, `oklab`, `linear_table`.

### `crates/imgdedupe-core/src/dirlist.rs`

- `dirlist.rs`: `Listed`, `list`, `entry_count`, `mtime_seconds`, both `cfg` versions of `read_whole` and of `ask_for_it_early`, `Radvisory`, `F_RDADVISE`, and the two declarations of the platform module.
- `dirlist/macos.rs` and `dirlist/elsewhere.rs`: the two `mod imp` bodies, the first behind `#[cfg(target_os = "macos")]` and the second behind `#[cfg(not(target_os = "macos"))]`, which is the pair of gates already on them. Both declarations name the module `imp`, so each carries its own `#[path]` to say which file it is.

## The order of work

The suite is run first and its counts written down. Those counts are what the work is measured against: the same named tests, the same number of them, all passing.

Smallest first, largest last, one file at a time. `dirlist.rs` goes first: its split is along a line the compiler already draws, so it says early whether the directory shape works. `app.rs` goes last.

The whole suite must pass after every single file. Not the tests for that file, not the tests for that crate: the whole suite, every time, before the next file is started. A file whose split leaves anything failing is fixed or put back before anything else is touched, because one wrong `pub(super)` in a file split an hour ago is a thing nobody finds once five more files have moved on top of it.

No behaviour changes while this runs. If a fault turns up in something being moved, it is written down and left alone.

## What says it is done

- Every named test that runs today runs after, under the same name and in the same module path. The test files are not touched.
- The suite reports the same counts it reported at the start.
- Every file named in the split exists and holds what this plan says it holds.
- `scripts\build.bat` and `scripts\build.bat --test` both produce an executable.

## The documentation

`docs/tests.md` heads each group with the source file its tests are about, and names `scan.rs`, `shot.rs`, `db.rs` and `index.rs` in its prose as well. Every one of those files keeps its name, so the document needs no change. `.claude/CHRONICLE.md` names source files in its record of past work and is not a description of the arrangement as it stands, so it is left alone. Nothing else in the repository names a source file.
