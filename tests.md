# The tests

Every test in the workspace, and what it is for. Run them with `cargo test
--release --workspace`, or one at a time by name.

A behavioural test uses the application: it scans a real folder of pictures,
searches it, and looks at what the window is left holding. The rest are unit
tests of a function whose inputs are its whole world.

## crates/imgdedupe-core/src/cleanup.rs

### a_plan_counts_its_files_and_bytes

A plan reports how many files it would remove and how many bytes that is, which
is what the toolbar and the cleanup page show before anything is touched.

### a_plan_takes_everything_but_the_keeper

A set puts every picture in the plan except the one marked to keep.

### a_set_with_nothing_kept_loses_all_of_it

What is marked is what is kept, so a set with no mark on it keeps nothing and
every picture in it goes.

### a_set_with_everything_kept_loses_none_of_it

A set marked to keep all of it contributes nothing to the plan.

### removing_says_how_many_files_it_has_been_through

The window draws a bar while files are going, so the removal counts them off one
at a time and finishes on the total.

### deleting_removes_the_planned_files_and_nothing_else

Deleting takes the files in the plan off the disk and leaves every other file in
the folder where it was.

### moving_to_a_folder_keeps_the_path_under_it

Moving a file to another folder recreates the path it had inside the scanned
folder, so two files with the same name do not land on each other.

### a_file_that_is_already_gone_is_reported_and_does_not_stop_the_rest

A file that cannot be removed is named in the outcome and the rest of the plan
still runs.

### an_empty_plan_does_nothing

Nothing marked means nothing removed, with no error.

### the_default_disposal_is_the_recycle_bin

The destination a cleanup starts on is the one that can be undone.

## crates/imgdedupe-core/src/db.rs

### a_file_id_is_reused_after_the_rows_above_it_are_deleted

SQLite hands a deleted row's id to the next file inserted. This is why nothing
outside one search result may be keyed on a file id, and why the thumbnails are
dropped when a new result arrives.

### upsert_writes_every_table_and_is_idempotent

Writing a picture fills the file, image and fingerprint rows, and writing the
same picture again updates them rather than adding more.

### deleting_a_file_cascades_to_the_derived_tables

Dropping a file's row takes its image and fingerprint rows with it.

### load_known_reports_what_the_diff_needs

The index hands back the size, timestamp and fingerprint version of every path it
holds, which is what decides whether a file needs reading again.

### a_file_without_fingerprints_reads_as_stale

A row with no fingerprint is treated as never indexed, so a pass interrupted
between writes picks it up next time.

### reopening_an_index_from_another_schema_version_is_refused

An index written by a different schema version is refused rather than read as if
it matched.

### compacting_gives_back_the_space_of_deleted_rows_and_keeps_the_rest

Rebuilding the index after a cleanup shrinks the file and keeps every row that is
still in it.

### compacting_leaves_the_original_alone_when_it_cannot_finish

The rebuild works on a copy, so a failure at any step leaves the index exactly as
it was.

### compacting_clears_what_a_failed_attempt_left_behind

A half-written copy from an earlier attempt is cleared rather than reused.

### compacting_keeps_the_rows_that_are_still_only_in_the_write_ahead_log

The tail of the index lives in the log beside it, so the copy is not a copy of
everything without it.

### closing_an_index_leaves_one_file_behind

A closed index is one file. The write-ahead log and the shared-memory file that a
database in WAL mode keeps while it is open both go when it is closed, and what
was written is still there afterwards.

### a_fresh_index_records_its_schema_version

A new index writes down the version it was made with.

## crates/imgdedupe-core/src/decode.rs

### fit_within_preserves_the_long_edge_and_aspect

Reducing to a long edge keeps the shape of the picture.

### fit_within_leaves_small_images_alone

A picture already smaller than the target is not enlarged.

### fit_within_never_returns_a_zero_edge

An extreme aspect ratio still produces at least one pixel on the short edge.

### decodes_png_and_reports_the_original_size

A PNG decodes, and the size reported is the file's own, not the reduced one.

### decodes_jpeg_and_reports_the_original_size

The same for JPEG, which takes a different path.

### jpeg_scaled_decode_produces_a_buffer_smaller_than_the_original

The JPEG decoder is asked for a scale rather than the full picture, so a large
photograph never becomes a large buffer.

### decodes_gif

A GIF decodes through the general path.

### a_grayscale_jpeg_reports_one_channel

The channel count is read from the file rather than assumed, because the keeper
score uses it.

### truncated_input_is_an_error_and_not_a_panic

A cut-off file gives an error the pass can report and carry on from.

## crates/imgdedupe-core/src/fingerprint.rs

### a_rotated_non_square_image_matches_the_original

A rotated copy of a picture hashes close to one of the original's variants.

### a_mirrored_non_square_image_matches_the_original

The same for a mirrored copy.

### a_rotated_copy_that_was_also_resized_still_matches

Rotation and resizing together stay within the threshold.

### an_unrelated_image_is_far_from_every_variant

A different picture is far from all eight variants, which is what stops the
variants from matching everything.

### the_variant_hashes_of_a_symmetry_are_a_permutation_of_the_originals

The eight variants of a rotated picture are the same eight hashes in a different
order, which is why one comparison covers every rotation.

### hashes_round_trip_through_storage

The packed blob written to the index unpacks to the hashes that went in.

### resampling_onto_a_fixed_square_carries_rotation_through

The square the hash is computed from keeps a rotation recognisable.

### a_strongly_rectangular_image_matches_its_rotation

A long thin picture still matches its own rotation, which is the hardest case for
a square resample.

### a_square_image_still_matches_its_rotations

A square picture, where the resample changes nothing, still matches.

### a_resized_copy_hashes_close_to_the_original

A resize moves the hash by less than the threshold.

### a_recompressed_copy_hashes_close_to_the_original

A re-encode moves the hash by less than the threshold.

### a_grayscale_copy_hashes_close_to_the_colour_original

Dropping the colour barely moves the hash, which is why the colour signature
exists separately.

### the_ring_signature_separates_colour_from_grayscale

The colour signature does tell those two apart.

### the_ring_signature_survives_rotation

The colour signature is unchanged by a rotation, so it can be compared against
any variant.

### the_ring_signature_survives_rescaling

The same for a resize.

### bands_reassemble_into_the_hash

The bands a hash is split into put it back together, so the band index cannot
lose part of a hash.

### hashes_within_the_band_bound_share_a_band

Two hashes closer than the pigeonhole radius share at least one band value, which
is what makes the band index find every real pair.

### a_band_is_wide_enough_to_be_selective

A band value is wide enough that unrelated pictures rarely collide.

### the_hash_leaves_the_unused_bit_clear

The one bit the hash does not use stays zero, so two encodings of the same
picture cannot differ in it.

### the_ring_signature_records_a_colour_for_a_flat_image

A picture of one colour produces a signature that says so.

### ring_distance_rejects_mismatched_signatures

Two signatures of different lengths are not compared as though they matched.

### comparing_in_words_gives_what_comparing_in_bytes_gives

The word-at-a-time distance the search uses agrees with the byte-at-a-time one.

### stopping_early_still_reports_a_distance_the_threshold_can_judge

The comparison gives up once it is past the limit, and what it reports is still
enough to reject the pair.

### a_pre_weighted_signature_measures_the_same_distance

Weighting the signature once when it is loaded gives the same distance as
weighting it on every comparison.

## crates/imgdedupe-core/src/format.rs

### detects_each_supported_format

Every format the indexer reads is recognised from its first bytes.

### rejects_formats_that_are_not_images

A file that is not a picture is not claimed as one.

### riff_that_is_not_webp_is_not_an_image

A RIFF container holding something else is not read as WebP.

### lossiness_matches_the_format

Whether a format loses data is recorded correctly, because the keeper score uses
it.

## crates/imgdedupe-core/src/frames.rs

### a_still_gif_is_not_animated

A single-frame GIF is indexed like any picture.

### a_gif_with_two_image_descriptors_is_animated

A GIF with more than one frame is left out of the index.

### a_plain_png_is_not_animated

A still PNG is indexed.

### a_png_with_actl_is_animated

An APNG is left out.

### a_still_webp_is_not_animated

A still WebP is indexed.

### a_webp_with_the_animation_flag_is_animated

An animated WebP is left out.

### truncated_files_do_not_panic

A cut-off file gives an answer rather than crashing the pass.

## crates/imgdedupe-core/src/matching.rs

### a_search_stops_when_it_is_told_to_and_gives_back_nothing

Cancelling a search returns nothing rather than a partial result that would look
like the answer.

### identical_hashes_land_in_one_set

Two copies of a picture are found as a set.

### unrelated_hashes_do_not_match

Two different pictures are not.

### a_chain_of_matches_becomes_one_set

A matches B and B matches C makes one set of three, so a set is a group and not a
pair.

### the_colour_signature_can_split_a_pair_and_the_setting_can_rejoin_it

Two pictures alike in shape but not in colour are separate until the colour
setting is turned off.

### a_different_shape_is_not_a_duplicate

Two pictures of different aspect ratios are not a pair however close the hashes
are.

### the_balanced_threshold_covers_what_the_same_picture_actually_moves

The balanced preset is wide enough for the distance a rotation was measured to
move a hash.

### the_presets_widen_in_order

Each preset is wider than the one before it, with balanced below the distance
unrelated pictures were measured at and yolo above it.

### the_default_is_the_balanced_preset

Where the window starts, and where opening a different folder puts the slider
back to, is the balanced preset. The default is not a value of its own that
could drift away from the button that claims to be on it.

### a_preset_that_is_not_one_lands_on_the_default

An unknown preset name gives the default rather than something arbitrary.

### a_threshold_can_be_set_anywhere_on_the_scale

The scale is continuous: a setting between two presets is wider than one and
narrower than the other.

### the_presets_are_points_on_the_same_scale

A preset is a place on the slider, so the two can never disagree about what the
search will use.

### a_threshold_reports_where_it_sits

A threshold can say what percentage it is, which is what puts the slider back
where it was.

### a_threshold_stops_at_the_ends_of_its_scale

The scale stops at both ends however far the control is dragged.

### a_rotation_keeps_the_same_shape

The shape test treats a rotated picture as the same shape, or every rotated
duplicate would be rejected before its hash was looked at.

### the_bigger_image_is_marked_to_keep

The picture a set suggests keeping is the one the score picks.

### recoverable_bytes_counts_everything_but_the_keeper

What a set reports as reclaimable is every picture in it but the keeper.

### a_band_files_every_variant_of_every_image_under_its_value

The band index holds all eight variants of every picture, so a rotated duplicate
is reachable from either side.

### copies_of_one_picture_are_folded_to_a_single_entry

Identical pictures are folded to one entry before pairing, which is what stops a
folder of copies from being compared to itself.

### the_same_hash_at_a_different_shape_is_not_folded

Folding is by everything the comparison reads, so two pictures with the same hash
but different shapes stay separate.

### inside_the_guaranteed_radius_it_finds_what_comparing_everything_finds

The band search finds exactly what comparing every pair finds, inside the radius
the bands guarantee.

## crates/imgdedupe-core/src/runlog.rs

### the_log_sits_beside_the_executable

The run log is written next to the program rather than in the folder being
scanned.

### writing_before_starting_does_nothing_rather_than_failing

A log line written before logging was turned on is dropped quietly.

## crates/imgdedupe-core/src/scan.rs

### indexing_is_reported_while_the_folder_is_still_being_read

Indexing is reported as it happens rather than once at the end, and a second pass
over a folder that is already indexed reports the whole folder as indexed. Both
halves run a real pass over a folder of pictures.

### a_first_pass_indexes_every_image

A pass over a folder of pictures indexes all of them.

### a_second_pass_over_an_unchanged_folder_reads_nothing

Files whose size and timestamp are unchanged are left alone: nothing is indexed
again. The bar is against the folder rather than the work, so the pass still
counts the file it looked at and ends on every file in the folder.

### a_removed_file_leaves_the_index

A file that is no longer on disk is dropped from the index.

### a_changed_file_is_reindexed

A file whose size or timestamp moved is read again.

### files_that_are_not_images_are_neither_indexed_nor_failures

A file that is not a picture is counted on its own, as neither indexed nor
broken.

### a_malformed_image_is_reported_and_does_not_stop_the_pass

A broken picture is reported by name and the pass carries on.

### without_recurse_subfolders_are_not_walked

A pass over the folder alone does not descend, and how far it reached is written
into the index so a later pass does not drop what it cannot see.

### the_index_file_does_not_index_itself

The index sitting in the folder is not treated as a picture, and not as a
vanished one on the next pass.

### paths_are_stored_with_forward_slashes

Paths are stored one way whatever the platform, so an index moves between them.

### a_cancelled_pass_leaves_a_readable_index

Stopping a pass part way leaves an index that opens and holds what it had
committed.

### animated_files_are_not_indexed

Animations are skipped, because one frame does not stand for them.

### the_event_stream_starts_and_ends

A pass always reports a start and a done, which is what the window's bars are
driven by.

## crates/imgdedupe-core/src/score.rs

### more_pixels_wins

Resolution is the first thing that decides which picture a set suggests keeping.

### resolution_outweighs_everything_below_it

A higher resolution wins even when every other term favours the other picture.

### lossless_wins_at_equal_resolution

At the same resolution, the format that did not throw data away wins.

### the_smaller_file_wins_at_the_same_resolution

At the same resolution and format, the smaller file is the better copy, not the
bigger one.

### the_small_copy_of_three_identical_pictures_is_the_keeper

Three copies of one picture keep the smallest.

### colour_wins_over_grayscale

More channels wins.

### alpha_wins_over_flattened

A picture that kept its transparency wins over one that lost it.

### a_copy_marker_loses_to_a_clean_name

A name that says "copy" loses to one that does not.

### a_copy_folder_loses_to_the_same_file_elsewhere

The same for a folder named as a copy.

### the_path_penalty_cannot_beat_a_resolution_doubling

Nothing about a name can outweigh twice the pixels.

### everything_below_resolution_together_is_worth_less_than_one_doubling

All the smaller terms added up still lose to a doubling, which is what keeps the
ordering readable.

### a_shorter_path_wins_all_else_equal

With everything else equal, the shallower path wins.

### scoring_is_deterministic

The same picture scores the same every time.

### a_number_that_is_part_of_the_name_is_not_a_copy_marker

A file called `img_2.jpg` is not treated as a copy of something.

## crates/imgdedupe-core/tests/jpeg_decoder_choice.rs

### the_scaled_path_is_the_faster_of_the_two

The decoder the indexer uses is the faster of the two that were measured.

### the_scaled_path_allocates_a_fraction_of_the_pixels

It also allocates a fraction of the memory, which is what keeps many threads
decoding at once from exhausting it.

### both_paths_reduce_to_the_same_picture

The two decoders agree on the picture, so the choice is about speed and not about
the result.

## crates/imgdedupe-core/tests/search_speed.rs

### what_a_search_costs_on_a_folder_worth_running_it_on

Four times the folder costs about four times the time, not sixteen: the search
does not become quadratic as a folder grows.

### a_folder_full_of_one_picture_does_not_become_quadratic

Two thousand copies of one picture are found without comparing them all to each
other.

## crates/imgdedupe/src/app.rs

### a_file_stamp_becomes_the_date_and_time_it_stands_for

The date under a tile is worked out from the file's timestamp without a calendar
library, so the arithmetic is what gets checked.

### the_space_bar_keeps_the_picture_the_preview_is_showing

On a real folder of two pairs, scanned and searched: the space bar moves the keep
mark to the picture the preview is on, does not touch the other set, does nothing
with no selection, and takes the mark off again when pressed on the picture
already kept.

### nothing_that_starts_work_is_offered_while_work_is_going

The window is busy while a real pass, a real search and a real cleanup are
running, and not busy once each has finished.

### a_cleanup_that_removed_nothing_stays_put_and_names_the_files

A real cleanup where the file was taken away first leaves the page where it is,
keeps the sets and the keeper, and names the file that would not go.

### a_cleanup_that_removed_everything_leaves_the_page

A real cleanup that removed its file leaves the review empty and the window back
on the scan.

### a_cleanup_that_half_worked_keeps_what_is_still_there

With three copies and one of the two doomed files taken away first, the one that
went leaves the set and the one that would not go stays on screen.

### starting_a_pass_clears_what_the_last_one_ended_with

After a real pass and search, starting another search or another pass clears the
outcome and the error from the last one.

### finding_no_duplicates_does_not_open_the_review

A real folder of two different pictures, searched at the narrowest setting, finds
nothing: the window stays on the scan, the tabs stay shut and it says so.

### the_first_sets_keeper_is_what_the_preview_starts_on

After a real search the preview opens on the first set's keeper, and on the first
picture when a set keeps nothing.

### right_and_left_run_through_the_whole_list_and_stop_at_its_ends

The step function walks forward and back through the list of sets and stops at
both ends.

### up_and_down_move_a_set_at_a_time_and_keep_the_place_in_it

Moving between sets keeps the position within the set where it can, and lands on
the last picture where it cannot.

### a_list_with_nothing_in_it_moves_nowhere

Walking an empty list does nothing.

### walking_moves_the_preview_to_the_next_picture

On a real result, a cursor key moves the preview from the picture it is on, into
the next set at the end of one, and nowhere at the end of the list.

### a_folder_that_is_not_remembered_loses_its_index_when_the_cleanup_is_done

A real scan, search and cleanup on a folder nobody asked to keep: the index and
its write-ahead log are deleted and the window is left on the scan with nothing
on it.

### taking_the_outcome_does_not_touch_the_index

A real cleanup on a remembered folder drops the removed file's row from the index
and reports how many rows went.

### what_was_skipped_and_what_broke_are_not_counted_as_found

A real folder holding two pictures, a text file and a broken PNG, scanned twice.
The pass looks at all four, counts the text file as ignored and the broken one as
a failure, and reports two found. The second pass finds nothing new, because what
was left alone was not read.

### a_set_of_portraits_is_not_given_the_width_of_a_landscape

A tile is the width of its own picture, so a portrait beside a landscape leaves
no gap.

### the_selected_tally_is_every_picture_that_is_not_kept

On a real result of two sets, the tally beside the set count is every picture that
is not marked to keep, and it follows the marks as they move.

### the_duplicate_count_is_every_picture_but_the_one_each_set_keeps

A real folder of a pair and a triple is five pictures in two sets, which is three
copies.

### walking_only_visits_the_sets_it_was_given

On a real result of three sets, a walk given only two of them does not go into the
third.

### the_row_walked_to_is_brought_to_the_middle

A row the cursor keys reach is scrolled to the middle of the list, clamped at the
ends.

### walking_asks_for_the_row_it_moved_to_to_be_shown

On a real result, walking into another set asks for that row to be shown, and
walking off the end of the list asks for nothing.

### a_set_row_takes_exactly_the_height_the_list_places_it_at

A row of a real result drawn in a real frame takes exactly the height the list
placed it at, or the content moves under a scroll that is already running.

### the_cleanup_choice_is_kept_with_the_folders_index

A destination chosen for a folder the window scanned comes back when the folder is
opened again from nothing.

### an_index_built_over_the_subfolders_opens_with_the_box_ticked

A real pass with subfolders included writes that into the index, and opening the
folder again ticks the box. A pass over the folder alone puts it back down.

### an_index_that_has_never_been_cleaned_up_keeps_the_safe_default

A folder scanned but never cleaned up opens on the recycle bin.

### the_button_says_what_the_chosen_destination_actually_does

Moving files is not removing them, and the button says so.

### every_cleanup_choice_survives_being_written_and_read_back

Each destination's stored name reads back as itself, and an unknown name is
nothing rather than a wrong guess.

### the_review_list_paints_a_twelve_point_bar_beside_it_in_either_theme

The scroll bar is actually painted, in a colour that can be seen, in both themes.

### the_bar_has_a_track_a_handle_and_a_button_at_each_end

The scroll bar has the parts a scroll bar has.

### pressing_the_handle_holds_it_where_it_was_and_drags_from_there

Clicking the handle does not jump the list; it drags from where it was grabbed.

### a_sideways_bar_has_the_same_parts_lying_down

The horizontal bar is the same bar.

### the_bar_is_there_when_the_preview_has_taken_most_of_the_window

A narrow list still has its scroll bar.

### a_list_that_scrolls_paints_a_twelve_point_handle_at_its_right_edge

The handle is drawn at the right edge of the list at its full width.

### a_set_removes_everything_but_the_kept_file

On a real result, the plan is every picture in the set except the keeper, and its
byte count is that file's.

### moving_the_keep_mark_moves_what_gets_removed

Pressing keep on the picture the plan was going to remove moves the plan onto the
other one.

### keeping_everything_in_a_set_removes_nothing_from_it

A set marked to keep all of it puts nothing in the plan.

### only_the_picture_being_kept_is_labelled_and_the_others_keep_the_space

Draws a set from a really scanned folder where one picture is marked to keep and
one is not. Only the marked one is labelled, and the line it sits on is taken on
both tiles, so the size, format and date under the pictures stay level across the
set instead of riding up under the unmarked one.

### the_buttons_on_a_set_decide_all_of_it_or_none_of_it

Draws a set from a really scanned folder, finds the keep all and keep none
buttons by their labels in what was painted, and really presses them. Keep none
puts every picture of the set in the plan, keep all takes them all back out.

### a_set_keeping_nothing_loses_all_of_it

Taking the mark off with the space bar puts every picture in that set into the
plan.

### the_review_state_is_not_written_to_the_index

Moving keep marks and building a plan on a real result leaves the index file
unchanged, byte for byte and row for row.

### saved_settings_reach_the_window

Everything in the settings file arrives in the window: folder, subfolders,
colour, window place and divider.

### the_setting_for_what_counts_as_a_duplicate_is_not_kept_across_a_restart

Really scans a folder at the top of the scale with the colour setting on, then
opens a new window from what that one would have written down. The folder and
the colour setting come back and the sensitivity does not: it starts on the
default every run, because what counts as a duplicate is decided against the
pictures on screen.

### a_folder_with_an_index_is_scanned_on_opening_and_one_without_is_not

A remembered folder with an index the application built is brought up to date on
opening; one with no index, and one nobody asked to keep, are not.

### a_location_is_only_remembered_while_it_is_asked_for

Opening another folder clears the tick and the subfolder box; opening the same
folder again keeps them; opening a folder that already holds an index ticks it.

### a_folder_picked_after_a_real_pass_leaves_nothing_of_the_last_one

The window after really being used: a folder scanned, its duplicates found, one
selected. Picking another folder leaves none of the counters, the outcome, the
sets or the selection behind, and does not scan the new folder.

### a_bar_with_nothing_done_paints_no_fill_at_all

Opens a folder, presses scan and paints the progress panel before a single file
has been counted, collecting the shapes that came out. Nothing is painted in the
fill colour. egui's own progress bar widens its fill to the corner radius so the
rounding has something to round, which puts an eighteen point bubble on a bar
that has made no progress. After the pass has really run, both bars are filled.

### opening_a_folder_only_scans_one_that_has_been_scanned_before

A folder the application has scanned is brought up to date the moment it is
opened. One it has not waits for the button.

### a_different_folder_starts_on_the_default_setting

After a real pass at the top of the scale with a destination chosen, another
folder starts on the default sensitivity, colour setting, destination and move
folder. The same folder again keeps them.

### no_saved_settings_leaves_the_window_empty

With no settings file the window opens on no folder, with subfolders off.

### the_preset_row_marks_the_one_the_slider_is_on

Exactly one preset is drawn as pressed for each preset value, none between two of
them, and every preset is inside the slider's range.

### the_slider_widens_what_counts_as_a_duplicate

Further along the scale allows more difference, in bits and in colour.

### the_app_starts_on_the_default_setting

The window opens on the default sensitivity, which is on the scale.

### a_row_fills_the_width_it_is_given

The three boxes on the scan page use the whole width of the row.

### every_box_gets_the_same_share_of_the_leftover

The spare width is split evenly rather than given to one box.

### a_wider_box_stays_wider_than_a_narrow_one

Sharing out the spare does not reorder the boxes by width.

### a_row_too_narrow_for_its_content_shares_nothing

A row with no spare width gives none away.

### cleanup_starts_on_the_recycle_bin

The cleanup page opens on the destination that can be undone.

### exactly_one_destination_is_selected_at_a_time

The three destinations are a choice of one.

### each_destination_maps_to_what_the_cleanup_layer_expects

The destination on screen becomes the disposal the cleanup runs.

### every_destination_has_a_label_and_a_note

Each choice has a name and a line saying what it does.

## crates/imgdedupe/src/folder_picker.rs

### a_picker_exists_for_this_platform

There is a folder picker compiled in for whatever platform this is.

## crates/imgdedupe/src/fonts.rs

### every_text_style_is_the_same_size

The window uses one text size.

### every_proportional_style_is_the_same_face

And one face.

### the_size_is_not_scaled_by_anything

Nothing multiplies the text size behind the theme's back.

### a_system_font_is_found_on_this_machine

The font the window asks the system for is there.

### asking_for_a_font_that_does_not_exist_gives_nothing

A missing font gives nothing rather than something arbitrary.

### a_light_face_is_not_accepted_as_the_interface_font

A light weight is rejected, because it is unreadable at this size.

### the_chosen_face_is_normal_weight

What is chosen is a normal weight.

## crates/imgdedupe/src/headless.rs

### opening_a_missing_index_says_to_scan_the_folder

Opening a folder with no index says to scan it, rather than failing with
something about a database.

### the_default_index_sits_in_the_scanned_folder

The index lives in the folder it describes.

## crates/imgdedupe/src/indexer.rs

### a_pass_reports_what_it_did_and_then_says_it_is_over

A real pass over a temporary folder reports its start, what it indexed, and that
it finished, and writes an index. Nothing is spawned and nothing is parsed out of
a pipe.

### a_pass_that_cannot_open_its_index_says_so_and_stops

An index that cannot be opened is reported by the pass rather than leaving the
window waiting for a run that never says anything.

### dropping_a_run_stops_the_pass_it_started

Dropping a run asks the pass to stop and waits for it, so a pass cannot outlive
the window and leave a write-ahead log behind.

## crates/imgdedupe/src/settings.rs

### the_folder_and_the_recurse_flag_both_come_back

A written settings file reads back as what was written, with or without the
remember tick.

### a_folder_that_cannot_be_reached_right_now_is_still_remembered

A folder that has gone offline is kept, because checking it exists on load once
threw the setting away for good.

### a_unc_path_survives_the_round_trip_untouched

A network path comes back exactly as it was written.

### the_window_place_comes_back_as_it_was_left

Position, size and whether it was maximized.

### the_divider_between_the_list_and_the_preview_comes_back_where_it_was

The review's divider is remembered.

### no_window_line_means_no_remembered_place_rather_than_a_broken_one

A settings file with no window line gives no remembered place.

### a_sensitivity_line_left_by_an_older_version_is_ignored

A settings file written by an older version still carries a sensitivity line.
Reading it changes nothing, so a value saved before the setting was dropped
cannot come back and move the slider.

### no_settings_file_gives_the_defaults

A missing file is the defaults, not an error.

### a_damaged_file_gives_the_defaults_rather_than_failing

A settings file full of nonsense still opens the window.

### a_path_with_spaces_survives_the_round_trip

Spaces in a folder name are not lost.

### the_path_is_stored_exactly_as_it_was_given

A path is not normalised, resolved or rewritten on the way in or out.

### saving_again_replaces_what_was_there

The file is replaced rather than appended to.

### the_index_is_not_asked_about_any_of_this

These are the application's settings, not facts about a folder, so nothing here
goes near a database.

### the_settings_go_where_the_operating_system_keeps_configuration

The settings file is in the platform's configuration folder, not beside the
executable.

## crates/imgdedupe/src/thumbs.rs

### loading_reduces_to_the_preview_size

A picture read for a tile comes back at the tile's size.

### the_preview_edge_leaves_a_photograph_on_the_half_scale_path

The size the preview asks for keeps a photograph on the JPEG decoder's half scale
path, so the pane does not wait for twelve million pixels.

### the_large_edge_gives_a_bigger_image_than_the_thumbnail_edge

The preview is decoded larger than the tile, so a thumbnail is not what gets
stretched across the pane.

### priming_reads_every_picture_and_keeps_them

Everything in the result is read, and what is read stays.

### a_picture_being_drawn_is_read_before_the_ones_that_are_not

With four hundred pictures asked for and one drawn, the drawn one comes back
within the first fifty answers rather than last.

### a_new_result_keeps_nothing_the_last_one_read

A new search result drops every texture and every decoded picture from the last
one, including whatever a worker was in the middle of, because a file id names a
different file once the index has changed.

### a_picture_becomes_a_texture_when_it_is_drawn_and_not_before

Decoded pictures wait in memory and become textures on the frame that draws them,
so a pass over thousands does not spend the window's frames uploading pictures
nobody is looking at.

### a_picture_put_in_front_is_still_only_read_once

Promoting a picture to the front leaves its place in the queue behind it, and that
place does not turn into a second read of the same file.

### a_tile_that_scrolled_away_does_not_hold_up_the_one_on_screen

A big picture already being decoded when the view moves does not delay the small
one now on screen.

### loading_something_that_is_not_an_image_gives_nothing_rather_than_panicking

Text, a broken file and a missing file all give nothing.

### a_small_image_is_not_enlarged

A picture smaller than the tile is left at its own size.

## crates/imgdedupe/src/tools.rs

### the_json_report_names_the_keeper

The debug report says which picture of each set is the keeper.

### the_csv_report_has_a_header_and_quotes_commas

The CSV report is readable by something else.

### the_plan_covers_everything_but_the_keeper

The debug cleanup plans the same thing the window would.

### cleaning_up_forgets_the_removed_files_and_only_those

The debug cleanup drops the removed rows from the index and leaves the rest.

### a_cleanup_that_removed_nothing_touches_no_rows

Nothing removed means nothing dropped.

### no_flags_asks_for_neither_report_nor_clean

The debug build with no flags opens the window like the release build.

### report_and_clean_each_take_the_folder_and_cannot_be_combined

The two debug flags each need a folder and cannot be given together.
