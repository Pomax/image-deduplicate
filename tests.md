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

### a_search_reports_while_it_runs

A search says what it is doing while it does it: reading the index, then comparing
pairs, then grouping. It used to say nothing at all until it had the answer, so
the window sat on whatever the pass had last put there for the whole of it. It
also reports before counting the rows, because counting them is itself a scan of
the whole view and gated everything behind it.

Reads a real index, so it takes the folder from `IMGDEDUPE_TEST_FOLDER` and is
marked to be asked for by name.

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
halves run a real pass over a folder of pictures.

### what_was_left_alone_is_reported_from_the_first_tick

A second pass over a folder with unchanged files, new files and a removed file.
Every progress report the pass makes, from the first one, carries the full
unchanged and removed counts, because they are known before any file is read.
Nothing about them waits for the end.

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

### a_crop_of_a_picture_is_found_to_be_the_same_picture

Scans a picture, a crop of the middle of it, and a different picture, then
searches. The crop and the picture are one set and the third file is on its own.
The whole-frame hash cannot do this: cropping stretches a different region over
the same square and every number in it changes at once, while the corners the
crop kept are still where they were.

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

### an_unticked_folder_loses_its_index_when_the_cleanup_is_done

A scan, search and cleanup on a folder with the checkbox unticked: the index and
its write-ahead log are deleted and the window is left on the scan tab with
nothing on it.

### taking_the_outcome_does_not_touch_the_index

A cleanup on a folder whose index is kept drops the removed file's row from the
index and reports how many rows went.

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

### keep_none_sits_level_with_the_last_line_under_the_pictures

Draws a set from a really scanned folder and compares where the keep none button
ends with the lowest line of text under the pictures. They finish level, so the
button reads as the foot of the set rather than floating above it.

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

### the_review_toolbar_holds_the_box_left_the_counts_centred_and_the_button_right

Draws the review over a scanned folder and reads the toolbar off the frame. The
allow multi-select box sits against the left edge, the clean up button against the
right, and the counts in the middle of the window rather than in the middle of
what is left of the row.

### a_click_on_the_preview_fills_the_window_and_escape_puts_it_back

Really clicks the preview in a scanned folder. The picture fills the window, at
the window's own size rather than blown up from the pane's copy, and the escape
key puts it back.

### the_preview_shows_what_the_file_says_about_itself

A picture carrying a comment, clicked in the review. The name of the thing and
what it says both appear under the picture, a frame or two after the click
because the file is read off another thread.

### the_index_keeps_whether_multi_selected_was_ticked

Ticks allow multi-select on a scanned folder and opens that folder again. The box
starts unticked, and the folder's own index is what remembers that it was
ticked.

### the_index_keeps_which_ways_of_matching_were_ticked

Both ways of matching are on when a folder is opened. Switching the corner match
off and opening the folder again comes back with it off and the other still on.

### without_multi_selected_marking_a_picture_lets_the_last_one_go

Keeps one picture in a set and then another. With the box unticked the mark
moves rather than adding up, so the set keeps the second one and nothing else.

### with_multi_selected_marks_add_up_and_come_off_one_at_a_time

Keeps two pictures in a set with the box ticked, then takes both marks off.
Marking the second one leaves both kept, taking one off leaves the other, and
taking that one off leaves the set keeping nothing at all.

### taking_one_picture_off_a_set_that_keeps_all_of_it_leaves_the_rest

Marks a whole set with keep all and then unmarks one picture in it. The other
two stay marked: taking one off what a set keeps is not throwing the lot away.

### two_clicks_with_multi_selected_keep_both_pictures

Really clicks twice on the picture in a scanned set that the search did not
choose, with allow multi-select ticked. Both that picture and the one the search
chose are kept, rather than the second taking the place of the first.

### the_last_entry_of_the_previous_list_empties_it

Scans two folders, then really opens the previous box and clicks its last entry.
The box is drawn right of the picker button and right of the folder path. The
list is emptied, and with nothing left to offer the box is not drawn at all.

### the_setting_for_what_counts_as_a_duplicate_is_not_kept_across_a_restart

Really scans a folder at the top of the scale with the colour setting on, then
opens a new window from what that one would have written down. The folder and
the colour setting come back and the sensitivity does not: it starts on the
default every run, because what counts as a duplicate is decided against the
pictures on screen.

### a_folder_with_an_index_is_scanned_on_opening_and_one_without_is_not

At startup the window opens the saved folder and looks in it for an index file.
If there is one, the checkbox is ticked and a scan starts at once. If there is
not, the checkbox is unticked and nothing happens until Scan is pressed. The
settings file has no say in either.

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

### a_pass_says_something_almost_at_once

The window hears from a pass within a second of it starting. Nothing was reported
until the listing, the index read and the diff had all finished, and the listing
asked the file system about every file one at a time, so on a folder on a network
mount that was over thirty seconds of a window that had been told nothing, which
is indistinguishable from one that has locked up.

Run against the folder the application is set to, so it is marked to be asked for
by name rather than run with everything else.

### a_pass_reaches_its_total_without_reading_the_index_page_by_page

Times the whole stretch from a pass starting to the bars having a total, which is
the listing plus the index read plus the diff. SQLite reads a database in pages as
a query asks for them, and on a network mount every page is its own round trip;
the index is now read whole, in one go, and worked on in memory.

Timing-sensitive and run against the real folder, so it is marked to be asked for
by name. It has been seen to pass at 2.5s and fail at 6.3s against identical code,
because a share's latency is not a constant.

### how_fast_new_files_are_read_and_indexed

Prints the peak read rate against the real folder rather than asserting a number,
because the number is the point and it belongs to the storage rather than to the
code. Marked to be asked for by name.

### a_pass_that_cannot_open_its_index_says_so_and_stops

An index that cannot be opened is reported by the pass rather than leaving the
window waiting for a run that never says anything.

### dropping_a_run_stops_the_pass_it_started

Dropping a run asks the pass to stop.

### dropping_a_run_does_not_wait_for_the_pass_to_finish

Dropping a run returns at once. It used to wait for the pass's thread, and closing
the window drops the run on the thread that draws, so the window closed only once
the pass had noticed it was cancelled. The pass checks between files, and a file
on a network mount is read by a call the operating system will not interrupt, so
that wait was as long as the other machine took. The window then could not be
closed, and the process could not be killed either, because a thread in an
uninterruptible wait does not die on a signal.

Run against the folder the application is set to, because that is the only place
the fault exists: on a local disk the pass sees the flag within milliseconds and
waiting for it looks free. Measured on a folder on a network mount, dropping the
run held the thread for **85.2 seconds** against the version that waited, and
returns in under a millisecond against the version that does not.

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
