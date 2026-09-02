# What was done on the in-memory branch

Every change made on this branch, in an order it can be redone in, one at a time,
with the tests that hold each one. Each numbered item is a separate commit's worth
of work: build and run the named tests before starting the next.

## The measurement that matters

`crates/imgdedupe-core/tests/decode_speed.rs`, run against a real folder:

```
IMGDEDUPE_SPEED_FOLDER=D:\some\folder cargo test --release -p imgdedupe-core --test decode_speed -- --nocapture
```

On a folder of 2213 pictures:

| | count | total | each |
|---|---|---|---|
| thumbnail (256px) | 2213 | 30.54s | 13.8ms |
| jpeg | 2136 | 28.71s | 13.4ms |
| png | 73 | 1.70s | 23.2ms |
| webp | 4 | 0.13s | 33.4ms |
| preview (1600px) | 40 | 6.90s | 172.4ms |

The slowest single thumbnail was 383ms, a 20 megapixel JPEG.

So a review of 1900 duplicates is about 26 seconds of decoding, divided by the
cores. Nothing about the queue, the map key or the threading changes that number.
The only things that would are decoding fewer pictures, or not decoding the same
picture twice between runs.

## The search, moved out of SQL

1. **Duplicate finding in memory.** `matching::find_sets` reads the whole index
   in one query and does the rest in memory: an `Image` per row holding the eight
   variant hashes as machine words, the band values, the ring signature
   pre-weighted, and the keep score. One `BandIndex` per band, built by counting
   sort, so every image sharing a band value is a contiguous run. Pairing and
   comparing run under rayon. Union-find groups the matches. All the matching SQL
   and `register_functions` are gone; `headless::open_index` no longer calls it.
   Tests: `inside_the_guaranteed_radius_it_finds_what_comparing_everything_finds`,
   `a_band_files_every_variant_of_every_image_under_its_value`, and
   `tests/search_speed.rs`, which checks four times the folder costs about four
   times the time rather than sixteen.

2. **Fold identical pictures before pairing.** A hash map keyed on everything the
   comparison reads (the eight variant hashes, the ring signature, the width and
   height). One picture per group goes through the band index and stands for the
   rest. Without it a folder of copies is one huge band bucket compared to itself.
   Measured 2000 copies: 47.52s before, 0.04s after.
   Tests: `copies_of_one_picture_are_folded_to_a_single_entry`,
   `the_same_hash_at_a_different_shape_is_not_folded`,
   `a_folder_full_of_one_picture_does_not_become_quadratic`.

3. **The search progress bar is gone,** and with it `Progress`, `Step`,
   `find_sets_reporting` and the piece counting. `find_sets_cancellable` replaces
   it. The search is under a second on ten thousand pictures, so there was nothing
   to watch. Cancelling still works.

## What a cleanup does

4. **What is marked KEEP is kept and everything else goes.** `plan_from_sets`
   lost its `resolved` flag, which was always true, and lost the guard that left a
   set alone when nothing in it was marked. A set keeping nothing loses all of it.
   `Keep::One` or `Keep::All` per set; no entry means keeping nothing.
   Tests: `a_set_with_nothing_kept_loses_all_of_it`,
   `a_set_with_everything_kept_loses_none_of_it`, `a_set_keeping_nothing_loses_all_of_it`.

5. **The space bar toggles the keep mark.** Pressing it on the picture already
   kept takes the mark off.
   Test: `the_space_bar_keeps_the_picture_the_preview_is_showing`.

6. **The per-set checkbox is gone,** along with the "N copies" and "MB to reclaim"
   labels beside it and the whole header row. "keep all" is drawn over the top
   right of the strip with a one pixel #333 border, and sets `Keep::All`.
   Test: `a_set_row_takes_exactly_the_height_the_list_places_it_at`.

7. **The review toolbar** reads `N sets`, `N duplicates`, `N to remove`,
   `N MB to reclaim`. The "hide under" slider and `min_recoverable` are gone.
   Tests: `the_duplicate_count_is_every_picture_but_the_one_each_set_keeps`,
   `the_selected_tally_is_every_picture_that_is_not_kept`.

## How a picture is named

8. **A row id is not a name for a file.** SQLite hands a deleted row's id to the
   next file inserted, so anything holding an id across a cleanup and a rescan is
   holding the wrong file. This is what made the thumbnails show the wrong
   pictures.
   Test: `a_file_id_is_reused_after_the_rows_above_it_are_deleted` in `db.rs`.

9. **The path is the identity, and an in-memory number stands in for it.**
   `Pictures` hands each path a `u32` the first time it is seen and keeps it for
   the life of the run. `accept_sets` registers every member's path before
   anything else. `Keep::One(u32)`, `selected`, `showing` and the thumbnail key
   are all that number. Do not use `String` keys for this: they are compared
   thousands of times a frame.
   Test: `a_path_keeps_its_number_and_no_two_paths_share_one`.

10. **`cleanup::Removal` lost its `file_id`,** which was written and never read.
    The `--report` JSON no longer prints one either; it already carries the path.

11. **`meta.root_path` is no longer written.** Nothing read it. The folder is the
    one the index file sits in.
    Tests: `making_a_path_absolute_leaves_a_link_alone`,
    `a_symlinked_folder_is_indexed_through_the_link`.

## The cleanup run

12. **Dropping the removed rows happens on the removal's thread.** It rewrites the
    index, which is seconds of copying on a large folder, and it was running in a
    frame. The bar reads "tidying the index", centred, while it does.
    Test: `taking_the_outcome_does_not_touch_the_index`.

## What the scan reports

13. **Files that are not pictures are counted separately.** `Event::Progress`
    carries `ignored`. The counter row is `found`, `unchanged`, `removed`,
    `failed to read`, `per second`, where `found` excludes both.
    Tests: `files_that_are_not_images_are_neither_indexed_nor_failures`,
    `what_was_skipped_and_what_broke_are_not_counted_as_found`.

14. **The indexed bar stays at nothing until the read is done,** rather than
    reading full because there is nothing to write yet. Picking a folder resets
    the whole scan state, and a folder with no index starts on the recycle bin
    rather than the last folder's choice.
    Tests: `nothing_to_write_is_only_finished_once_there_is_nothing_left_to_read`,
    `picking_a_folder_puts_both_bars_back_to_nothing`,
    `a_folder_that_has_never_been_indexed_starts_on_the_recycle_bin`.

## Words

15. **"Quarantine" is gone.** `Disposal::MoveTo`, `Destination::MoveTo`,
    `move_dir`, the stored value `"move"`, and `--to move` with `--move-dir`.
    An index written before this stores `disposal = "quarantine"` and
    `quarantine_dir`; neither is read afterwards, so those folders lose that one
    setting.
    Test: `moving_to_a_folder_keeps_the_path_under_it`.

## The command lines

16. **`imgindex` takes the folder, `--recurse`, `--db`, `--progress`, `--log`.**
    `--verify` and `--threads` are gone, and rayon is off its dependencies.

17. **`imgdedupe`'s `--report` and `--clean` are `#[cfg(debug_assertions)]`,**
    in `tools.rs`. The release build parses no command line at all: it looks for
    `--log` in `env::args` and opens the window, so there is no clap and no
    `--help` in it.
    Tests: `no_flags_asks_for_neither_report_nor_clean`,
    `report_and_clean_each_take_the_folder_and_cannot_be_combined`.

## The review list

18. **A set's tiles are as wide as that set's own widest picture,** fitted into
    `TILE`, with `TILE_RING` clear around each picture for the keeper border and
    the selection ring. A frame's response covers its outer margin, so the ring is
    drawn around the picture's own rect, not the response rect.
    Test: `a_set_of_portraits_is_not_given_the_width_of_a_landscape`.

19. **KEEP is centred on its picture** with `Layout::top_down(Align::Center)`.
    `add_sized` does not centre it.

20. **The cursor keys put the walked-to row in the middle** of the list, clamped
    at the ends, rather than scrolling the minimum.
    Test: `the_row_walked_to_is_brought_to_the_middle`.

21. **The thumbnail hover tooltip is gone.**

22. **The sensitivity scale reaches 50 percent** and the slider is 300 wide. Above
    about 25 percent it reports unrelated pictures, which is the point of it.
    Test: `a_threshold_stops_at_the_ends_of_its_scale`.

## What went wrong, so it is not repeated

The thumbnail loader was working. It was changed three times in one go to fix a
problem nobody reported, and every one of those changes was mine to invent:

- a two-line queue with a mutex and condvar, so a clicked picture could jump ahead
  of the bulk
- `drop_queued`, throwing away what the last pass asked for
- a pause tied to the view, and then removing the worker threads entirely, which
  moved the decoding onto the UI thread and froze the window

None of it was asked for and none of it helped. On the new branch, change the
texture key from the row id to the picture number and change nothing else in
`thumbs.rs`: keep the worker threads, the single channel, `prime`, `collect` and
the placeholder.

The tests did not catch any of it, because every thumbnail test stops at the
channel. `priming_reads_every_picture_and_keeps_them` reads twelve results off
`thumbs.results` and never goes through `collect`, which is the only place a
texture is made. A test that drives `get` and `collect` the way a frame does,
over enough pictures, is missing and should be written first on the new branch.

## Snapshots

Every file was copied to the session scratchpad before each edit, under
`snap/`: `app.rs` through `app-12.rs`, `thumbs.rs`, `thumbs-paused.rs`,
`thumbs-12.rs`, `matching.rs` to `matching-3.rs`, `scan.rs` to `scan-3.rs`,
`cleanup.rs` to `cleanup-3.rs`, `tools.rs` to `tools-3.rs`, `search_speed.rs` to
`search_speed-3.rs`, `db.rs`, `fingerprint.rs`, `headless.rs`, `headless-2.rs`,
`indexer-2.rs`, `main.rs`, `imgindex-main.rs` to `imgindex-main-3.rs`, `cli.rs`,
`cli-2.rs` and `core-Cargo.toml`. They are session scoped and will not survive it.
