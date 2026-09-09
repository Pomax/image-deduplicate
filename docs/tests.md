# The tests

Every test in the workspace, and what it is for. Run them with `cargo test
--release --workspace`, or one at a time by name.

Every one of them runs anywhere. Nothing here reads a folder of somebody's own
photographs, names a path on anybody's disk, or needs a machine to be set up
first: a check that wants pictures makes them in a temporary folder and throws
them away. `.github/workflows/tests.yml` runs the whole suite on Ubuntu, macOS
and Windows, all three, on every push and every pull request, and a change is
not good until all three have passed.

The checks that do need a real folder of photographs are not in this repository.
See the last section.

A behavioural test uses the application: it scans a real folder of pictures,
searches it, and looks at what the window is left holding. The rest are unit
tests of a function whose inputs are its whole world.

Every check about where something landed draws its frame through `shot.rs`, which
fills the triangles the toolkit's tessellator produces into a buffer of its own
and writes a PNG named after the check. The window is drawn by a graphics card and
a test has none, so this is the only way to look at what a check was measuring.
The pictures go to `IMGDEDUPE_SHOT_DIR`, or the temporary folder when that says
nothing, and only the last frame a check drew is filled in — that is the frame it
went on to measure, and filling one is the slow part.

## crates/imgdedupe-core/src/cleanup.rs

### a_plan_counts_its_files_and_bytes

A plan reports how many files it would remove and how many bytes that is, which
is what the toolbar and the cleanup page show before anything is touched.

### a_plan_takes_everything_that_is_not_marked

A set puts every picture in the plan except the ones marked to keep.

### a_set_with_nothing_marked_loses_all_of_it

What is marked is kept, so a set that marks nothing keeps nothing and every
picture in it goes.

### a_set_with_everything_marked_loses_none_of_it

A set where every picture is marked contributes nothing to the plan.

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

### a_pair_is_the_same_pair_either_way_round

A pair of pictures said not to be copies is written down lower id first, whichever
way round it was given, and saying it twice says it once.

### a_pair_goes_when_either_of_its_pictures_does

Deleting one of the two files takes the pair with it: a pair with one side left
is not a pair.

### an_index_without_the_table_has_nothing_ignored

An index written before there was such a table reads back as a folder where
nothing has been ignored, rather than as a failure.

### a_file_that_is_not_a_picture_is_written_down_as_looked_at

The row comes back saying the file has been read, at that size and timestamp, and
is not a picture. That is the answer that stops the next comparison calling it
new and the next pass opening it.

### a_file_that_became_a_picture_stops_being_marked_as_not_one

A file that was not a picture and is one now is indexed in the ordinary way, and
the row stops saying otherwise.

### a_picture_that_stopped_being_one_loses_what_was_held_about_it

The other way about. What the index held about it as a picture goes, so nothing
downstream is left describing a picture that is not there.

### a_review_beginning_makes_the_tables_it_is_written_in

A review is its two tables, and they are made where a review begins — sets being
built into the review page — not by the first mark somebody happens to make. So a
review nobody has marked anything in yet is a review with nothing marked, and not
a folder that has no such thing. Beginning one on a folder that already has a
review leaves its marks where they are.

### a_cleanup_leaves_the_ignored_pairs_where_they_are

The end of a review takes the review away and nothing else. The pairs somebody
said are not copies of each other are not one sitting's work — they are a decision
about those pictures — so a cleanup drops the marks and the sets and never touches
them.

### an_index_nobody_has_reviewed_has_no_marks_and_no_sets

A review is one sitting's work, so neither of its tables is part of what an index
is. A fresh index holds neither, and reading a review out of it gives nothing back
rather than failing.

### a_mark_written_is_a_mark_read_back_and_the_rest_are_left_alone

A mark going on is one row written, a mark coming off is one row deleted, and
neither touches any other mark. Saying it twice says it once, and taking off what
is not on is nothing. This is what keeps a click on a picture to a single
statement: on a folder on another machine every statement is a journal written and
deleted beside the index, and a write that redid the whole review on every click
cost one of those per mark the review held.

### a_mark_goes_when_its_picture_does

A mark on a file that is gone is not a mark, so deleting the file takes the mark
with it.

### the_sets_come_back_in_the_order_they_were_stored_in

The review is a list and somebody left off partway down it, so the sets come back
in the order they were shown in and not in the order their numbers happen to fall.

### a_stored_set_loses_the_pictures_its_files_lost

A set that has lost a picture is not that set. Deleting a file takes its row out
of the stored sets, and what is left of the set is what comes back.

### a_review_that_is_over_leaves_neither_table

Clearing the marks and the sets leaves an index that reads as one nobody has
reviewed, and the next review makes both tables again.

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

### an_index_from_another_schema_version_is_refused

An index written by a different schema version is refused rather than read as if
it matched.

### an_index_in_an_older_shape_is_migrated_on_disk_when_it_is_opened

Opening brings the file itself to the current shape before anything is served
from it. Checked against the file on disk, not against what was read out of it,
because a migration that lives only in memory is lost the moment it is dropped.

### an_index_written_in_nanoseconds_comes_back_in_milliseconds

An index from a build that kept stamps in nanoseconds comes back holding
milliseconds and no nanosecond column, checked against the file rather than the
copy handed back. Shown to fail with the carrying step taken out before it was
claimed to catch anything.

### a_half_converted_index_is_finished_rather_than_converted_twice

A run killed after the stamps were carried across and before the old column was
dropped. The next open finishes the job and does not divide what it already
divided, because the old column is what says whether the dividing has happened.

### an_index_given_the_new_column_but_no_values_is_converted

The other half of that: a run killed after the column was added and before the
stamps were carried. The next open carries them.

### a_fresh_index_records_its_schema_version

A new index writes down the version it was made with.

## crates/imgdedupe-core/src/index.rs

The one owner of a folder's index.

### nothing_but_the_manager_holds_the_index

A read and a write are both answered by messages, and nothing that comes back is
a connection. A read made before any folder is held says so, rather than
answering with nothing: an empty answer is what the old code substituted when a
read failed, and it went on to be written over the real index.

### letting_go_of_a_folder_waits_for_the_file_to_catch_up

Closing an index answers only once the file holds every change made through the
manager.

### a_scan_that_indexed_nothing_still_leaves_the_file_in_step

A folder is opened, nothing at all happens to it, and it is let go: the migration
made on the way in is in the file. The old code wrote the index out only when a
pass had found work, so an index it had just migrated was thrown away and the
next run migrated it again.

### every_change_reaches_the_file

One of every kind of change — rows written, rows deleted, a setting set, a
setting forgotten, a pair marked, a compaction — then the index is closed, and
all of them are in the file.

### a_change_is_in_the_file_once_the_writing_is_waited_for

The same, without closing the index first: a setting is set, the writing is
waited for, and a second connection to the file reads it back.

### a_change_does_not_replace_the_file

The file's creation time is the same before and after a change. The index used to
be written beside the file and renamed onto it, which left a different file every
time; it is now written into.

### deleting_a_file_takes_its_ignored_pairs_out_of_the_file

Two pictures, marked as not copies of each other, then one of them deleted. The
pair is gone from the file, not only from the copy in memory. This fails if the
connection the writer holds does not have `foreign_keys` on.

### a_review_written_by_one_window_is_in_the_file_for_the_next

A review is written down as it is made, so what one window marked and the sets it
marked them in are in the file, not only in the copy in memory.

### a_review_that_is_over_is_out_of_the_file_too

Clearing the marks and the sets reaches the file as well, so the next window opens
a folder nobody has reviewed.

### a_review_written_with_no_index_open_is_refused_and_leaves_the_writer_clean

A window writes a review as it happens, and a folder is chosen before its index is
open. Asking for a mark then is refused, and — this is the point — the writer is
not handed the change anyway: work it has nowhere to do would be kept and reported
as trouble the next time somebody closed the index.

### compacting_makes_the_file_smaller

Four hundred rows written, then deleted, then a compaction: the file is smaller
afterwards. A `VACUUM` on the copy in memory does not change the size of the file,
so the file has to get one of its own.

### a_broken_index_stops_the_manager_and_writes_nothing

The two ways an index cannot be read: it is not a database, or it is written
under a schema version this build does not speak. Each is refused with a reason,
each leaves no index open, and each leaves the file byte for byte what it was.

### no_connection_is_made_outside_the_manager

Reads the source of both crates and fails on `Connection::open`,
`open_in_memory` or `open_with_flags` anywhere but `db.rs`, which holds the one
opener, and `index.rs`, which is the manager that calls it. This is the only
thing that keeps the rule true once it is true.

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

### a_picture_is_turned_the_way_the_file_says

All eight ways a camera can say a picture goes, done to a picture whose corners
are all different, so which corner ends up where says whether the turn was the
right one. The quarter turns stand the picture on its end; a value the standard
does not define leaves it alone.

### truncated_input_is_an_error_and_not_a_panic

A cut-off file gives an error the pass can report and carry on from.

### decodes_tiff_and_reports_the_original_size

A TIFF is sniffed as one and decodes through the general path, like any other
picture.

### a_raw_file_is_read_from_its_preview_at_the_size_of_its_own_picture

A raw file built by hand, holding a preview and saying its sensor is six
thousand pixels across. The picture comes from the preview, and the size
recorded is the sensor's: what the file is worth keeping for is the picture it
holds, not the size of the copy inside it.

### a_raw_file_with_no_preview_in_it_is_an_error_and_not_a_panic

A raw file whose directories point at nothing, and one holding no boxes worth
opening. Both give an error the pass can report and carry on from.

### a_heic_file_that_holds_nothing_is_an_error_and_not_a_panic

A HEIC header with no picture behind it is an error rather than a crash.

## crates/imgdedupe-core/src/features.rs

### a_picture_gives_corners_and_the_same_ones_twice

A picture with things in it produces corners, and fingerprinting it twice gives
the same ones. A fingerprint that differs between two runs over the same file
would put a file in a set with itself and nothing else.

### a_flat_picture_has_no_corners

A picture of one shade has no corners, and that is not an error. A flat sky, or
a photograph out of focus, is matched by the whole-frame hash and by nothing
here.

### corners_are_spread_over_the_picture_rather_than_bunched

The strongest corners of a picture all sit wherever it has the most texture, and
a crop of anywhere else would then match nothing. The frame is divided into
cells that each keep their own best, and this checks both halves of a picture
come away with corners.

### corners_of_the_same_place_are_described_the_same_way

A picture and the left half of it. Corners that survived the cut are described
as they were before it, which is the property the whole thing rests on.

### corners_survive_being_packed_and_read_back

What goes into the index comes back out of it unchanged.

### corners_that_all_match_the_same_corner_do_not_agree_on_anything

Forty corners spread over one picture whose nearest description in the other is
all the same single corner. One pair survives, not forty, and they agree on
nothing: a heap of matches that all end in the same place is not two pictures
arranged the same way.

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

### every_extension_names_the_format_it_belongs_to

Each extension a format lists is read back as that format, and a name with no
extension, or one this build does not know, is not read as any of them.

### an_extension_in_capitals_is_the_same_extension

`PICTURE.JPG` claims the same format as `picture.jpg`.

### detects_each_supported_format

Every format the indexer reads is recognised from its first bytes.

### detects_each_raw_format_and_heic

Canon's two containers, Nikon, Sony, Panasonic and HEIC are each recognised from
their first bytes: by a header of their own, by the marker after it, by the
maker's name in the first directory, or by the brand the container declares.

### a_maker_this_tool_has_no_name_for_is_a_tiff

A TIFF-headed file from a manufacturer this build has no name for is read as a
TIFF, which is what it is. The preview inside it is still found if the file
turns out to hold one.

### rejects_formats_that_are_not_images

A file that is not a picture is not claimed as one.

### a_container_this_tool_does_not_read_is_not_an_image

Video and audio use the same container as HEIC and CR3, and are not claimed as
pictures because of it.

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

### comparing_is_reported_while_it_is_still_comparing

Six hundred pictures alike enough that every pair is worth comparing, which is
hundreds of reports rather than one per batch. The pairs are cut into batches
that all run at once, so a report at the end of a batch is a report at the end
of the whole thing: on a real folder that was thirteen seconds of a bar standing
still and then filling in one step.

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

### a_set_comes_back_oldest_first

Three copies written at different times come back in the order the files
appeared, whatever order the search met them in.

### the_bigger_image_is_marked_to_keep

The picture a set offers as its best copy is the one the score picks. Nothing is
kept or removed on account of it: it is what "auto-mark to keep" marks.

### a_stored_set_is_built_back_into_the_set_it_was

Only file ids and their order are written down. Built back from the pictures as
they are now, a set has the same members in the same order with the same best
copy as the one the search handed over, so nothing about a picture is written
twice and the two cannot drift.

### a_stored_set_that_lost_its_pictures_comes_back_short_or_not_at_all

A set whose pictures are no longer in the index comes back without them, and one
left holding a single picture does not come back at all, because one picture is
not a set of copies.

### recoverable_bytes_counts_everything_but_the_keeper

What a set reports as reclaimable is every picture in it but its best copy. This
is the command line's figure; it has nobody to mark anything.

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

## crates/imgdedupe-core/src/metadata.rs

### a_cameras_own_directory_is_read

A TIFF directory as a camera writes one. The make, the model and the way up come
back with the names this gives them rather than as numbers.

### the_camera_settings_hanging_off_it_are_read_too

The settings sit in a directory of their own that the first one points at, and
its tags mean different things from the same numbers in the first, so it is read
against its own table. The exposure comes back as the fraction a shutter speed
is.

### the_numbered_fields_of_a_wire_service_are_read

Captions, credits and keywords are stored as numbered fields from the days of
wire services. A caption, a photographer and a keyword go in and come back under
the names those numbers stand for.

### only_what_a_photographer_would_look_at_is_kept

An editor writes hundreds of its own settings into a file: how much clarity was
applied, where the highlights were pulled to, which curve was used. None of that
is about the photograph, so none of it is shown. This also checks that
`rdf:Description`, the element every property sits inside, is not mistaken for
the photograph's own description, which is what once made every value in a file
come out labelled as a caption.

### a_date_is_said_the_way_somebody_would_say_it

A file writes a date as 2026:06:13 04:18:34, and XMP as 2026-06-13T04:18:34.
Both come out as June 13, 2026, 4:18:34 am. Anything that is not a date is left
out rather than shown wrong.

### adobes_xml_is_read_in_both_of_its_forms

XMP writes a property either as an attribute of the description or as an element
with the value inside it, and whatever wrote the file chose. Both are read.

### a_file_that_says_nothing_about_itself_has_nothing_to_show

Something that is not a picture, nothing at all, and a JPEG with no such segment
in it: all of them say nothing, rather than saying something wrong.

### a_file_that_lies_about_its_own_lengths_is_survived

A file claiming a value four thousand bytes long that is not there, one cut in
half, and a wire service field claiming more than it has. These files come off
other people's cameras, so a length is a claim and not a fact.

## crates/imgdedupe-core/src/preview.rs

### the_preview_a_directory_points_at_is_found

A TIFF-based raw file built by hand: a directory saying how big the sensor is
and where the preview went. The preview comes back byte for byte, and so does
the sensor's size.

### the_biggest_preview_in_the_file_is_the_one_taken

A file holding a thumbnail and a larger preview, in two directories. The larger
one is what comes back: a fingerprint built from a postage stamp is a worse one.

### a_picture_stored_as_though_it_were_the_files_pixels_is_found

Canon's older bodies write the full-size JPEG where a TIFF keeps its pixels, and
say so in the compression tag. That is followed as a preview like any other.

### a_preview_in_a_directory_off_the_first_one_is_found

Nikon keeps the preview in a directory that only the first one points at, so the
directories hanging off a directory are followed too.

### a_preview_written_into_a_tag_is_found

Panasonic writes the whole JPEG into a tag rather than pointing at it, and
records the size of the sensor rather than of the picture. Both are read.

### a_preview_in_a_box_of_its_own_is_found

Canon's newer bodies write boxes inside boxes, the preview in one of its own
behind a name sixteen bytes long. The boxes are walked and the picture found.

### a_jpeg_holding_a_smaller_one_is_measured_to_its_own_end

A JPEG with a thumbnail of itself in its metadata ends twice, and the first end
is not the file's. The markers are walked so the whole picture is taken and not
the part before the thumbnail's end.

### sensor_data_stored_as_a_lossless_jpeg_is_not_taken_for_the_preview

Canon stores what came off the sensor as a lossless JPEG of two channels, in the
same file, pointed at the same way as the preview and several times its size.
Nothing here decodes that, so the frame header is read and the preview taken
instead. A real Canon file indexed as nothing at all is what this is about.

### a_box_that_says_its_size_the_long_way_is_still_read

A box too big for a four byte size writes the size after its name instead.
Canon's newer files put the preview behind one, and stopping at it means finding
only the thumbnail.

### the_size_of_the_picture_is_taken_from_the_camera_settings_when_it_is_there

When the directories describe pieces of a picture rather than the whole one, the
size of the picture is with the camera's settings, in a directory of its own.
That is the size recorded, not the largest piece.

### a_file_that_is_not_a_container_holds_nothing

Plain text and a plain JPEG hold no preview, and asking for one gives nothing
rather than a wrong answer.

### a_directory_pointing_outside_the_file_is_not_followed

A file claiming its preview sits a megabyte past its own end gives nothing. The
files this reads come off other people's cameras and cards, so every offset is
checked against the file's length.

### a_directory_that_points_at_itself_ends

A file whose directory lists itself as the next one to read stops rather than
going round for ever.

### the_way_up_comes_out_of_a_jpegs_own_segment

A camera writes which way up a picture goes into a segment near the front of the
JPEG, holding a TIFF of its own. All eight of the values the standard defines
are read back from one built here.

### the_way_up_comes_out_of_a_raw_files_first_directory

A raw file says it in its first directory instead, where the rest of its tags
are.

### a_file_that_does_not_say_which_way_up_it_goes_is_upright

A JPEG with no such segment, a raw file with no such tag, and something that is
not a picture at all: all upright, which is the only safe answer.

### a_way_up_that_is_not_one_of_the_eight_is_ignored

A value the standard does not define means nothing, and turning a picture by
nothing in particular is worse than leaving it alone.

### the_maker_comes_out_of_the_first_directory

The name of whoever made the camera is read from the first directory, which is
what tells a Nikon raw from a Sony one when both carry a plain TIFF header.

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
halves run a real pass over a folder of pictures. Reports come every two hundred
files or every tenth of a second, whichever falls first, so the first one is
checked for being part way through rather than for a particular count.

### what_was_left_alone_is_reported_from_the_first_tick

A second pass over a folder with unchanged files, new files and a removed file.
Every progress report the pass makes, from the first one, carries the full
unchanged and removed counts, because they are known before any file is read.
Nothing about them waits for the end. Reports come on a timer, and the first file
read is announced whenever it lands rather than waiting for one, so a pass over a
folder that finishes inside the interval still reports while it works.

### a_pass_puts_every_row_it_indexed_into_the_file

Every row a pass says it indexed, with its fingerprints and how far the pass
reached, is in the file when the pass is over. The pass tells the manager and the
manager writes; nothing in the pass opens or closes anything. Checked against the
file, because the file is what the next run of the program opens.

### the_budget_is_ninety_percent_of_what_is_available

Ten gigabytes to spare and nothing in hand: nine gigabytes may be read ahead.

### filling_the_read_ahead_does_not_shrink_the_budget

Ten gigabytes spare and nothing held, then the same machine with four gigabytes
of files in hand and six spare: the budget is nine both times, because what is
held is added back before the nine tenths are taken.

### a_program_taking_memory_takes_the_budget_with_it

Ten gigabytes spare and then two: the budget falls from nine to 1.8.

### a_reader_waits_until_the_budget_grows_rather_than_for_a_release

A reader waiting for room takes up memory the machine gave back, with no decode
having finished and nothing released.

### a_file_larger_than_the_whole_budget_is_let_through

A file bigger than the whole budget goes through on its own rather than waiting
for room that will never exist.

### a_machine_that_will_not_say_gets_the_fallback

A machine that answers nothing about its memory gets the gigabyte the limit used
to be.

### the_machine_says_how_much_memory_is_available

The platform code answers with a believable number on the machine it is running
on: more than nothing, less than a petabyte.

### a_first_pass_indexes_every_image

A pass over a folder of pictures indexes all of them.

### a_folder_indexed_on_one_machine_is_unchanged_on_another

A folder is indexed, its index is rewritten the way a build keeping nanoseconds
would have described it, and it is passed over again: nothing is read a second
time. File systems disagree about the digits below a millisecond for the same
file, so an index carrying them called every file changed when it was read
anywhere else, and this is the check that it does not.

### a_second_pass_over_an_unchanged_folder_reads_nothing

Files whose size and timestamp are unchanged are left alone: nothing is indexed
again. The bar is against the folder rather than the work, so the pass still
counts the file it looked at and ends on every file in the folder.

### a_removed_file_leaves_the_index

A file that is no longer on disk is dropped from the index.

### a_changed_file_is_reindexed

A file whose size or timestamp moved is read again.

### files_that_are_not_images_are_neither_indexed_nor_failures

A file that claimed a picture format in its name and turned out not to be one is
counted on its own, as neither indexed nor broken.

### a_file_that_claims_no_format_is_not_read

A folder holding a picture, a four megabyte text file and a four megabyte
`imgdedupe.sqlite-journal`. Only the picture is indexed, and nothing is reported
as failing, because neither of the others is read at all: their names claim no
format this reads. This is why the walk no longer has to be told the index's
name.

### what_a_file_is_comes_from_its_bytes_not_its_name

A JPEG saved as `liar.png` is read, because the name claims a format, and indexed
as a JPEG, because its first bytes say so. The name decides what is worth
reading; the bytes decide what it is.

### what_a_pass_could_not_index_is_not_read_again_by_the_next_one

A picture, a file that claims a format and is not one, and a broken picture. The
first pass indexes one and fails on one; the second indexes nothing, fails on
nothing, and counts all three as unchanged — a file counted as unchanged is a
file the pass did not open. Without the row for what it could not index, those
two are read in full on every pass for ever.

### a_folder_whose_files_were_all_looked_at_is_as_indexed

And the folder then answers that it is as indexed, which is what decides whether
opening it asks anybody anything. A picture added since is a difference, which is
the other half of the same answer.

### a_file_that_became_a_picture_is_indexed_by_the_next_pass

The row says where the file was and how big it was. A file that was not a picture
and has been replaced by one differs in both, so the next pass reads it and
indexes it.

### a_malformed_image_is_reported_and_does_not_stop_the_pass

A broken picture is reported by name and the pass carries on.

### a_pass_over_the_subfolders_stays_out_of_dot_and_at_folders

A folder whose name begins with a dot or an at sign is something else's
workings — `.git`, `.thumbnails`, `@eaDir` — and a pass over the subfolders does
not go into one, nor into anything under it.

### the_folder_the_pass_was_pointed_at_is_scanned_whatever_it_is_called

The folder somebody points the window at is the folder they meant. A pass over
`.private` reads what is in it.

### without_recurse_subfolders_are_not_walked

A pass over the folder alone does not descend, and how far it reached is written
into the index so a later pass does not drop what it cannot see.

### the_index_file_does_not_index_itself

The index sitting in the folder is not treated as a picture, and not as a
vanished one on the next pass.

### a_crop_of_a_picture_is_found_to_be_the_same_picture

Scans a picture, a crop of the middle of it, and a different picture, then
searches. The crop and the picture are one set and the third file is on its own.
The whole-frame hash cannot do this: cropping stretches a different region over
the same square and every number in it changes at once, while the corners the
crop kept are still where they were.

### matching_within_folders_never_puts_two_folders_together

A folder with the same picture in two of its subfolders and a copy beside one of
them. Searched whole, all three are one set; searched one folder at a time, only
the two that share a folder are, and the third is left where it is.

### with_the_corners_switched_off_a_crop_is_not_found

The same picture and crop, searched with the corner match switched off. Nothing
is found, which is what switching it off is for.

### with_the_whole_frame_switched_off_a_resize_is_not_found

A picture and a half-size copy of it are one set with everything on, and nothing
at all with both ways of matching switched off.

### an_index_from_a_build_without_corners_is_brought_up_to_date

An index as an older build left it: no column for the corners, and rows saying
they were fingerprinted by the version before this one. Opening it adds the
column, and the pass reads every file again and fills it in. An index made
before this existed is not a dead index.

### paths_are_stored_with_forward_slashes

Paths are stored one way whatever the platform, so an index moves between them.

### a_cancelled_pass_leaves_a_readable_index

Stopping a pass part way leaves an index that opens and holds what it had
committed.

### animated_files_are_not_indexed

Animations are skipped, because one frame does not stand for them.

### the_event_stream_starts_and_ends

A pass says what it is doing from its first moment, and ends with a done. The
folder's total is not the first thing it can report, because the listing is what
produces it; before that there are step reports and a count of what the listing
has found. It used to say nothing at all until the listing, the index read and
the diff had all finished, which on a folder on another machine was half a minute
of a window that had been told nothing.

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

### a_folder_of_pictures_with_corners_does_not_become_quadratic

Pictures whose hashes are all far apart, so the corner pass is the only thing
running: four times the folder costs about four times the time, not sixteen.

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

### the_preview_does_not_open_inside_an_ignored_set

Two sets with the first one ignored. The review opens in the second, on what that
set marks once it marks something, and on nothing at all when every set is
ignored. A set nobody calls a set of copies is not somewhere to start: it keeps
nothing and the cursor keys would only step out of it.

### the_first_sets_keeper_is_what_the_preview_starts_on

A review arrives with nothing marked, so the preview opens on the first picture
of the first set. Once something is marked it opens on that instead.

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

### an_unticked_folder_loses_its_index_when_the_cleanup_is_done

A scan, search and cleanup on a folder with the checkbox unticked: the index and
everything the manager leaves beside it are deleted and the window is left on the
scan tab with nothing on it.

### a_folder_forgotten_leaves_no_index_and_says_how_many_rows_went

Forgetting a folder takes the index and everything beside it, and the count of
what went comes from the manager. The window used to remove the file itself and
report a hardcoded nought, because it had nothing left to count.

### a_folder_whose_index_is_in_an_older_shape_still_opens

A folder whose index was written by an older build opens and can be searched
without a pass having to happen first, and the file itself is left in the shape
this build reads. Only one of the four old openers migrated, so an index no scan
had touched was never brought up to date.

### a_broken_index_leaves_the_lamp_red_and_the_window_stopped

A folder whose index cannot be read stops there: the lamps for the index stay
red, what went wrong is on screen, no pictures come out of it, no pass starts,
and the file is not written to.

### taking_the_outcome_does_not_touch_the_index

A cleanup on a folder whose index is kept drops the removed file's row from the
index and reports how many rows went.

### what_was_skipped_and_what_broke_are_not_counted_as_found

A real folder holding two pictures, a text file, a file named `.png` that is not
one, and a broken PNG, scanned twice. The pass looks at four of the five — the
text file claims no format, so it is never read — counts the one that lied about
its name as ignored and the broken one as a failure, and reports two found. The
second pass finds nothing new and reads nothing at all: all four are unchanged,
including the two it could not index, because the first pass wrote down that it
had looked at them.

### a_set_of_portraits_is_not_given_the_width_of_a_landscape

A tile is the width of its own picture, so a portrait beside a landscape leaves
no gap.

### the_selected_tally_is_every_picture_a_cleanup_would_take

On a real result of two sets: nothing is going before anything is marked, marking
one in each set puts the others in, and taking a set's marks off takes the whole
set out. Marked the way a person marks one, because the tally follows from the
marking and not from the field it lands in. The tally is asked of the plan, so it
cannot disagree with the button.

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
folder again ticks the box. A pass over the folder alone puts it back down: what
the index says reaches the boxes when the folder is opened, and after that the
boxes are whatever the person at the window set them to.

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

### a_set_removes_everything_but_the_marked_file

On a real result, the plan is every picture in the set except the marked one, and
its byte count is that file's.

### moving_the_keep_mark_moves_what_gets_removed

Marking the picture the plan was going to remove and unmarking the other swaps
which of them the plan takes.

### keeping_everything_in_a_set_removes_nothing_from_it

A set where every picture is marked puts nothing in the plan.

### only_the_picture_being_kept_is_labelled_and_the_others_keep_the_space

Draws a set from a really scanned folder where one picture is marked to keep and
one is not. Only the marked one is labelled, and the line it sits on is taken on
both tiles, so the size, format and date under the pictures stay level across the
set instead of riding up under the unmarked one.

### the_buttons_on_a_set_decide_all_of_it_or_none_of_it

Draws a set from a really scanned folder, finds the keep all and keep none
buttons by their labels in what was painted, and really presses them. Keep all
clears the marks, which takes the set out of the plan; keep none puts every
picture of it in; keep all then does nothing, because a set being cleared out
answers to one button only; and that button reads "keeping none", which puts it
back.

### a_set_marked_with_nothing_loses_all_of_it

A review arrives marking nothing, so every picture of every set it found is
there to be cleaned up.

### auto_marking_adds_the_best_copy_and_disturbs_nothing

On a real result of two sets, one already marking the copy that is not the best
one. Auto-marking marks the best copy in the untouched set, and adds it to the
other beside the mark that was already there.

### auto_marking_leaves_ignored_sets_alone

A set nobody calls a set of copies is an answer already given, so it gets no
mark.

### a_marked_picture_is_drawn_with_a_border_and_an_unmarked_one_is_not

Keeping a picture is two things on screen: a green border round it and the word
KEEP under it. A review arrives with neither drawn, marking one draws both, and
taking the mark off takes both away.

### the_ring_round_the_picture_shown_is_not_the_keep_border

The ring says where the cursor keys are and the border says what is kept, so a
picture can have one, the other, both or neither. Taking the marks off leaves the
ring where it is.

### a_mark_reaches_the_index_and_leaves_the_pictures_alone

Marking a picture on a real result puts that mark in the folder's index, where the
next window will find it, and changes nothing the index knows about the pictures
themselves.

### saved_settings_reach_the_window

Everything in the settings file arrives in the window: folder, subfolders,
colour, the folders scanned before, window place and divider.

### a_folder_dropped_on_the_window_is_scanned_and_searched

A folder dropped on a window that is sitting on the review tab is opened, scanned
and searched without anything being pressed. The drop puts the window back on the
scan tab while that runs, and the search leaves it on the review tab with the
duplicates it found.

### dropping_anything_but_a_folder_does_nothing

A dropped file is not treated as a folder to scan, and a drop that arrives while
a pass is already running does not change the folder.

### a_folder_joins_the_previous_list_by_being_scanned_and_not_by_being_opened

Really scans two folders, one of them twice, and opens a third without scanning
it. The list of previous locations holds the two that were scanned, in
alphabetical order, once each. The one that was only opened is not in it.

### one_of_something_is_written_in_the_singular

The counts above the review list read "1 set" and "1 duplicate" rather than "1
sets" and "1 duplicates", and keep the plural for none and for more than one.

### holding_the_pointer_over_a_set_pops_nothing_up

Scans a folder whose file names are far too long to fit under a picture, then
rests the pointer on every point of the set a dozen points apart, waiting at each
one long enough for a tooltip to appear. The name is on screen twice throughout,
once per tile, so nothing popped up over the top of it.

### dragging_across_the_window_selects_no_text

Presses on the lines under a picture in a scanned folder and drags across them.
Labels are not selectable and the pointer never becomes a text cursor: this is a
window, not a document.

### nothing_in_the_window_shows_a_tooltip

The window's source contains no hover text at all, and no label that cuts its own
text: egui puts the whole string in a tooltip of its own making whenever a label
has to elide, so the lines that need cutting are painted rather than added as
widgets. What a control does is written on it. This is a guard against tooltips
creeping back in one at a time.

### walking_along_a_long_set_brings_the_selected_picture_into_view

A set of twenty-four pictures in a window that fits three, walked from one end to
the other with the cursor keys and back. The strip follows the selection: it does
not move while the picture is already on screen, it moves further along with
every step past the edge, and it comes back with the walk.

### the_review_list_keeps_to_its_own_side_of_the_window

Draws the whole review page with the preview pane beside the list. No set
reaches into the pane, and everything the list draws is cut off at the list's
own edge rather than painted over the pane.

### an_ignored_set_is_drawn_faded_and_its_buttons_are_not

Draws a really scanned set before and after it is ignored and reads the colour
the writing was drawn in. The file names under the pictures come out at a quarter
of the alpha they had; the row of buttons under them comes out unchanged, because
the buttons are how a set stops being ignored.

### a_sets_bar_runs_the_width_of_the_box_and_the_band_has_a_line_on_it

Draws a set with more pictures than fit across it and reads where its own scroll
bar was painted: corner to corner inside the line round the box, not set in from
either edge. The band of buttons under it has a line of its own along the top, so
its edge is not mistaken for the box's.

### the_scroll_bar_beside_the_list_stays_where_it_is_when_the_list_moves

Draws the review page, reads where the bar's track was painted, scrolls the list
to the second set, and reads it again. The track is in the same place: it marks
the room the list is drawn in, and only the handle inside it moves.

### the_set_boxes_are_drawn_whole_and_evenly_spaced

Draws the review page and reads the rectangles the set boxes were painted as,
with the rectangle each was clipped to. No box has its top cut off by the edge of
the list, one box stands `BETWEEN_BOXES` clear of the next, the gap from the
window's edge to a box's left edge is the gap from its right edge to the scroll
bar, and the pictures in a box start as far inside it as they end.

### a_set_has_its_buttons_in_a_row_along_the_bottom

Draws a set from a really scanned folder and reads where its three buttons
landed: keep all, then keep none, then ignore, left to right with space between
them, on one row, below the lowest line of text under the pictures.

### a_button_that_changes_its_word_does_not_move_the_ones_beside_it

Two of the buttons change what they say. Each is drawn to the widest thing it can
ever say, so flagging a set turns "keep none" into "keeping none" and every
button stays exactly where and as wide as it was. Fails against a button sized to
its current words, which changes width and drags "ignore" along with it.

### a_set_box_is_not_taller_than_the_tiles_in_it

Draws a set from a really scanned folder and measures the height the row took
against the lowest line of text painted in it. What is left over is the strip's
scroll bar and the frame's own padding, not a band of empty space under the file
names in every row of the list.

### a_set_row_fits_the_room_it_is_given

Draws a set into the room the list gives a row, which is the width less the
scroll bar the list paints down its right. The box ends as far from that bar as
it begins from the window's edge, and the gap under the last line is the strip's
own scroll bar and the frame's margin and nothing else.

### the_ways_of_matching_are_boxes_on_the_page_that_can_be_clicked

The box that says what counts as a duplicate holds a checkbox for each way of
matching, in order with the colour one last, and clicking one switches that way
off without touching the other.

### two_clicks_on_a_picture_keep_it_the_way_the_space_bar_does

Really clicks twice on the picture in a scanned set that the search did not
choose. It becomes the one being kept, and two more clicks a moment later let it
go again, which is what the space bar does on the picture being shown.

### the_review_toolbar_holds_marking_left_the_counts_centred_and_cleanup_right

Draws the review over a scanned folder and reads the toolbar off the frame. The
auto-mark to keep button sits against the left edge, the clean up button against
the right, and the counts in the middle of the window rather than in the middle
of what is left of the row.

### a_click_on_the_preview_fills_the_window_and_escape_puts_it_back

Really clicks the preview in a scanned folder. The picture fills the window, at
the window's own size rather than blown up from the pane's copy, and the escape
key puts it back.

### the_preview_shows_what_the_file_says_about_itself

A picture carrying a comment, clicked in the review. The name of the thing and
what it says both appear under the picture, a frame or two after the click
because the file is read off another thread.

### an_ignored_set_is_shown_and_left_alone

Ignoring a set on a really scanned folder: the set stays on the review page,
nothing in it is going to be removed, the cleanup has nothing to do, what it was
keeping is still written down and counts for nothing, and opening the folder
again from nothing comes back with the set still ignored.

### a_set_can_be_unignored_again

Ignoring a set and then taking it back: the pairs go from the index, the set is
being cleaned up again, and opening the folder from nothing comes back with it a
set of copies.

### a_folder_the_window_opens_on_comes_up_with_its_ignored_sets_ignored

A set ignored on a really scanned folder, then the window started again on that
folder rather than told to open it. The pairs are read from the index before
anything is drawn, so the set comes up ignored instead of coming back as a set of
copies.

### unignoring_a_set_gives_back_the_picture_it_was_keeping

A set keeping one picture is ignored and taken back. While it is ignored nothing
in it can be marked or unmarked and the cleanup passes over it; the moment it is
a set again the mark is on the picture it was left on.

### ignoring_the_set_the_preview_is_in_leaves_the_keys_somewhere_to_go

Three sets with the preview on a picture in the middle one, which is then
ignored. The preview stays where it is, and each of the four keys leaves the
ignored set: left and up to the set before it, right and down to the set after
it.

### the_cursor_keys_step_over_ignored_sets

Three sets with the middle one ignored. Forward off the end of the first lands in
the third and back again returns to the first; a set at a time does the same.
With nothing but ignored sets beyond, the keys move nothing at all.

### a_set_is_only_ignored_when_every_pair_in_it_is

A set of three with one pair ignored is still a set of copies. Only once all
three pairs are ignored is the set left alone.

### the_index_keeps_whether_marking_on_opening_was_ticked

Ticks automatically mark to keep on a scanned folder and opens that folder again.
The box starts unticked, and the folder's own index is what remembers that it was
ticked.

### a_pass_with_marking_on_leaves_every_set_marked

With the box on, a real pass over a folder of two pairs comes back with every set
marking its best copy, without anybody pressing the button.

### the_index_keeps_which_ways_of_matching_were_ticked

Both ways of matching are on when a folder is opened. Switching the corner match
off writes nothing on its own — a control moved and never used has changed
nothing — and searching with it off is what the index takes. Opening the folder
again comes back with it off and the other still on.

### the_index_keeps_matching_within_folders_and_running_on_opening

A folder scanned with its subfolders, then set to match within folders and to
rescan on opening. Rescanning on opening is a choice about the folder and is
written where it is made; matching within folders is a search setting and waits
for a search to use it, which is checked here rather than assumed. Opening the
folder again from nothing comes back with both, off the index.

### a_box_that_depends_on_another_is_off_and_out_of_reach_without_it

Matching within folders needs subfolders, rescanning on opening needs an index to
rescan, and marking on opening needs a rescan to mark at the end of. With what
they depend on switched off, all three come off, and clicking where they are
drawn does not put them back on. Ticking the index box asks for a rescan on
opening by itself, which is what that box is for, and the marking box can then be
reached and ticked.

### opening_a_folder_reads_its_index_without_scanning_it

A folder that has been scanned before is opened again with the rescan box off.
No pass runs, and the index is still read into memory: the pictures are there,
the lamp for it is lit, and Find duplicates finds the copies without a pass
having read a single file.

### shift_marks_the_selected_picture_and_unmarks_the_rest

Shift with the space bar, or with a double click, says which picture rather than
toggling one: a set marking two ends up marking only the one the preview is on,
and pressing it again leaves that mark where it is rather than taking it off.

### marks_add_up_and_come_off_one_at_a_time

Marks two pictures in a set, then takes both marks off. A mark speaks for its own
picture and no other, so the second joins the first rather than replacing it,
taking one off leaves the other, and taking that one off leaves the set marked
with nothing at all.

### two_clicks_on_a_second_picture_keep_both

Really clicks twice on a second picture in a scanned set. Both it and the one
already marked are kept, rather than the second taking the place of the first.

### the_last_entry_of_the_previous_list_empties_it

Scans two folders, then really opens the previous box and clicks its last entry.
The box is drawn right of the picker button and right of the folder path. The
list is emptied, and with nothing left to offer the box is not drawn at all.

### the_setting_for_what_counts_as_a_duplicate_is_not_kept_across_a_restart

Really scans a folder at the top of the scale with the colour setting on, then
opens a new window from what that one would have written down. This is the
application's own settings file, not a folder's index: the folder and the colour
setting come back from it and the sensitivity does not, so a window opens on the
default rather than on whatever the last run was left at. A folder whose index
holds what it was last searched with is a different matter, and brings it back.

### a_folder_with_an_index_is_asked_about_on_opening_and_one_without_is_not

At startup the window opens the saved folder and looks in it for an index file.
If there is one, the checkbox is ticked and the index is asked what it says about
itself, which is what decides whether a pass starts. If there is not, the
checkbox is unticked and nothing is asked or started. The settings file has no
say in either.

### a_review_survives_the_folder_changing_and_being_scanned_again

The path a folder that keeps changing really takes: marked, closed, a picture
added, opened again. The comparison finds the difference, the pass runs, the
search hands over sets that are not the ones that were saved, and the mark goes
back onto the picture it was on, with what a cleanup would take counting it. The
mark is checked in the index file first, so a failure here says which half broke.

### an_ignored_set_comes_back_ignored_after_a_rescan

The other half of what a review is. A set somebody said is not a set of copies
comes back that way after a pass and a new search, because the pairs are in the
index and not in the sitting.

### a_review_is_still_there_when_the_folder_is_opened_again

The point of the whole thing. A folder scanned, searched and marked, the window
closed, the folder opened again: the review comes back on the same sets with the
same picture marked, and what a cleanup would take comes back with it. Neither a
pass nor a search runs, because the answer was already written down.

### a_folder_that_changed_asks_before_its_saved_review_is_opened

A picture added since the review was saved. The sets stored for that folder do not
describe it any more, so the question goes up saying the folder's content has
changed, and until it is answered no review opens and no pass starts.

### a_folder_that_rescans_itself_with_nothing_to_rescan_opens_its_review

A folder set to bring itself up to date on opening, with a review saved for it
and nothing changed. There is nothing to bring up to date, so no pass runs and
nothing is asked: the review opens, and the folder still asks to be rescanned the
next time.

### a_folder_that_rescans_itself_is_rescanned_when_something_changed

The same folder with a picture added. That is what the box is for, so it is
brought up to date without anybody being asked, saved review or not.

### a_changed_folder_with_no_saved_review_is_rescanned_without_asking

A folder that has changed, with no review saved and the box unticked. There is
nothing to protect and so nothing to ask about, and the pass runs.

### an_unchanged_folder_with_no_saved_review_is_searched_without_a_pass

A folder with an index, nothing changed and no saved review was opened to be
searched, and its pictures are already in memory, so it is searched. No pass
runs.

### a_folder_with_no_index_is_left_alone_when_it_is_chosen

Choosing a folder that has no index is not asking for anything to happen to it.
There is nothing to compare it against, so nothing is compared, nothing is
scanned and nothing is searched.

### the_window_closes_the_index_on_the_way_out

A caller is answered when the copy in memory has its change, not when the file
does, so the window has to close the index as it ends: closing is what waits for
the writer. What is checked is that it closed it, not that a mark happened to
have landed — on a small folder the writer wins that race anyway, and a test that
reads the file would pass whether or not the window waited. That closing waits is
`letting_go_of_a_folder_waits_for_the_file_to_catch_up`.

### what_went_with_a_forgotten_index_is_counted_in_pictures

The index holds a row for every file the pass has been through, pictures or not.
A folder holding two pictures and one file that is not one reports two going with
the index, because "N rows went with it" is read as pictures.

### answering_rescan_leaves_no_stored_sets_and_starts_a_pass

Giving the saved review up takes the sets out of the index there and then. The
pass runs, and the search after it writes down what it found, which is what the
index holds from then on.

### a_cleanup_leaves_no_review_in_the_index

A cleanup is the review being carried out, so the review is over: neither the sets
nor the marks are in the index afterwards. What is left of the folder is what the
marks named, which says nothing, and there are no sets for them to be marks in.

### the_held_plan_follows_every_interaction

What a cleanup would take is held rather than worked out on every frame that
draws, so every interaction that changes it has to work it out again. After each
one — a mark on, a mark off, shift, auto-marking, a set ignored, a set taken back,
a picture removed, work cancelled, a pass started — the held plan is compared with
one derived then and there. This is the test that catches a site that forgot.

### drawing_the_review_does_not_change_the_plan

Drawing is not an interaction. Three frames of the review leave the plan exactly
as it was.

### marks_do_not_outlive_the_pictures_a_cleanup_took

A set of three with two marked, one of them removed. The surviving set's marks are
cut down to the pictures still in it, so no mark names a file that is gone and a
`Keep::Several` is not left holding a single id, which the type says never happens.

### the_pictures_read_on_opening_do_not_replace_a_finished_pass

Opening a folder reads its index on a thread of its own, and a pass over the same
folder reads it again. Both are answered by the one thing that owns the index, so
the opening read can come back after the pass has finished, holding the folder as
it was before it. The pass's pictures stay, and the search still finds the copies.

### the_checkbox_follows_the_folder_that_is_opened

Opening a different folder resets the checkbox and the subfolder setting.
Opening the same folder again leaves them alone. Opening a folder that contains
an index ticks the checkbox whatever it was before.

### a_folder_picked_after_a_real_pass_leaves_nothing_of_the_last_one

The window after really being used: a folder scanned, its duplicates found, one
selected. Picking another folder leaves none of the counters, the outcome, the
sets or the selection behind, and does not scan the new folder.

### a_bar_with_nothing_done_paints_no_fill_at_all

Opens a folder, presses scan and paints the progress panel before a single file
has been counted, collecting the shapes that came out. Nothing is painted in the
fill colour. egui's own progress bar widens its fill to the corner radius so the
rounding has something to round, which puts an eighteen point bubble on a bar
that has made no progress. After the pass has really run, all three bars are
filled.

### the_times_beside_the_steps_are_measured_from_the_scan_button

A window that has been open for an hour is told to scan. Every step reports
milliseconds, not an hour: the clock the steps are timed against starts when the
run does, and reading a folder's index on opening starts it as well.

### the_steps_a_pass_had_nothing_to_do_are_marked_as_passed_over

The four steps for reading and indexing new files happen on the first pass over
a folder. On a second pass over the same folder there is nothing new to read or
index, and those four end as passed over rather than as still to happen, which
is what the empty circle beside them means.

### a_pass_with_nothing_left_to_do_fills_both_bars_anyway

Scans a folder twice. The second pass reads nothing, because the index already
holds every file in the folder, and that is a folder fully read and fully
indexed: both bars are full. A bar left empty because no work happened would be
a lie about a folder that is entirely done.

### a_search_that_reads_no_index_does_not_start_its_bar_part_full

The duplicates scan bar is the stages that are going to run. A search after a
pass reads no index, because the pass built what it searches, so the bar is two
parts and opens at nothing; one that does read the index is three parts, and
half the reading is a sixth of the bar.

### listing_the_folder_does_not_move_the_read_bar

The listing is not the reading. While the folder is still being listed nothing
has been read, and a total to read is not the same as having read any of it, so
the bar for reading stays where it is: empty.

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

## crates/imgdedupe/src/metadata.rs

### asking_gives_nothing_back_at_once_and_something_back_later

What a file says about itself is read on a thread of its own: asking for it gives
nothing back on the spot and the answer turns up later. A raw file is tens of
megabytes and often on another machine, and the window has to go on drawing while
it arrives.

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

### the_bundled_face_is_the_only_one_the_window_has

The face carried in the binary is a real TrueType file, and text really lays out
through it: wide letters come out wider than narrow ones, which they would not if
the layout had fallen through to an empty fallback. Nothing is read from the
machine, so the window paints without waiting on a font database.

### there_is_one_face_and_both_families_use_it

One face is loaded, and the proportional and monospace families are both it. A
second face in the binary is a second face nobody asked for.

### the_bundled_face_has_the_letters_the_window_writes

Every letter, digit and mark the window itself writes is in the bundled subset.
The interface must not depend on what the machine happens to have installed.

### only_letters_the_bundled_face_lacks_send_anyone_looking

An ordinary name, and one with accented Latin in it, need nothing from the
machine. A name in Chinese reports exactly the letters that are missing, which is
what sends the search for a face off to look.

### a_face_is_taken_for_a_sans_serif_by_its_name

Neither the font database nor the parser says what kind of face something is, so
the name decides. The interface faces of the three platforms are taken as sans
serif; serif and script faces are not, including one whose name also contains a
sans serif word.

### a_face_without_the_letters_is_not_taken

A search over a database holding only the bundled face finds it for letters it
has, and finds nothing for a letter it does not. A face is only ever taken when
it covers every missing letter.

## crates/imgdedupe/src/headless.rs

### opening_a_missing_index_says_to_scan_the_folder

Opening a folder with no index says to scan it, rather than failing with
something about a database.

### the_default_index_sits_in_the_scanned_folder

The index lives in the folder it describes.

## crates/imgdedupe/src/icon.rs

### the_icon_holds_the_pixels_it_says_it_does

The icon is the size it tells the window it is, and holds every row of it. One
that says one size and holds another is not shown at all: the window quietly
keeps whatever it had.

### the_icon_is_one_picture_over_another

Reads the pixels: the corners are clear, the card is behind everything, the
picture in front holds its sky, its sun and its hill, and what shows of the one
behind is an outline with the card inside it.

## crates/imgdedupe/src/indexer.rs

### a_pass_reports_what_it_did_and_then_says_it_is_over

A real pass over a temporary folder announces the folder's total, says what it
indexed, says it finished, and writes an index. Nothing is spawned and nothing is
parsed out of a pipe. The total is no longer the first thing it says: a pass
reports the steps it goes through from the moment it starts, and the total is only
known once the listing is over.

### a_pass_that_cannot_open_its_index_says_so_and_stops

An index that cannot be opened is reported by the pass rather than leaving the
window waiting for a run that never says anything.

### dropping_a_run_stops_the_pass_it_started

Dropping a run asks the pass to stop.

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

### the_folders_scanned_before_come_back_in_order

The list of previous locations survives being written and read back. Whatever
order it went in, it comes back alphabetically with letter case ignored, and a
folder listed twice comes back once.

### no_folders_scanned_before_is_an_empty_list_rather_than_a_blank_entry

Settings with nothing scanned yet read back as an empty list, and a file whose
previous lines have nothing after the equals sign gives an empty list rather
than entries pointing at nowhere.

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

### a_picture_the_file_says_to_turn_arrives_turned

A wide picture with a segment saying to stand it on its end, read the way the
tiles and the preview read every picture. It comes back on its end. Both of them
come through this one function, so this is where the turn belongs.

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

## Not in this repository: the checks against a real folder

Eight checks only mean something against a real folder of photographs, on the
machine and the mount that folder lives on. They read the folder the application
is set to and print what they find there, so their source is in `local/`, which
is in `.gitignore` and is never committed. A checkout does not have it and does
not need it.

They are behind the `local` feature, which is off, so `cargo test --workspace`
never looks for the directory and CI never turns the feature on. With the
directory present:

    cargo test --features imgdedupe/local -- --ignored --nocapture

None of them could run in CI even if the source were here. Each is a measurement
of a large index over a network mount, and a generated folder on a runner's local
disk answers a different question. What each one checks about the program is
already checked in the suite proper, on a folder made for the purpose:

| Not in the repository | What checks the behaviour here |
| --- | --- |
| `a_search_reports_while_it_runs` | `comparing_is_reported_while_it_is_still_comparing` |
| `what_the_real_folders_index_says_about_itself` | nothing: it prints, it does not assert |
| `only_matching_within_folders_holds_on_the_real_folder` | `matching_within_folders_never_puts_two_folders_together` |
| `every_box_the_window_keeps_survives_the_real_folders_index` | `the_index_keeps_whether_marking_on_opening_was_ticked`, `the_index_keeps_which_ways_of_matching_were_ticked`, `the_index_keeps_matching_within_folders_and_running_on_opening` |
| `a_pass_says_something_almost_at_once` | `the_event_stream_starts_and_ends` |
| `a_pass_reaches_its_total_without_reading_the_index_page_by_page` | `a_folder_whose_index_is_in_an_older_shape_still_opens` |
| `how_fast_new_files_are_read_and_indexed` | nothing: it prints a rate that belongs to the storage |
| `dropping_a_run_does_not_wait_for_the_pass_to_finish` | `dropping_a_run_stops_the_pass_it_started` |
