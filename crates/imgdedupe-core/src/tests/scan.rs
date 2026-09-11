use super::*;
use image::{DynamicImage, RgbImage};

fn write_image(path: &Path, width: u32, height: u32, seed: u32) {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([((x + seed) % 256) as u8, ((y * 3) % 256) as u8, 90])
    });
    DynamicImage::ImageRgb8(image)
        .save_with_format(path, image::ImageFormat::Png)
        .expect("writing a fixture");
}

/// A picture with corners in it: blocks of varying shade, which is a corner
/// at every join. The gradient above has none, and a picture with no corners
/// is one the feature fingerprint has nothing to say about.
fn write_detailed(path: &Path, width: u32, height: u32, seed: u32) {
    // Blobs of their own shade, none like another, with soft edges.
    //
    // Two things this has to be. Every neighbourhood different, or every
    // corner describes the same thing as every other and a corner that
    // matches everywhere matches nothing. And soft, because a picture of
    // hard-edged blocks a few pixels across turns into something else when
    // it is scaled, in a way a photograph never does: the blocks alias and
    // the corners land in different places.
    let shade = |x: u32, y: u32| {
        let cell = (x / 24).wrapping_mul(73_856_093)
            ^ (y / 24).wrapping_mul(19_349_663)
            ^ seed.wrapping_mul(83_492_791);
        let base = (cell.wrapping_mul(2_654_435_761) >> 24) as f32;
        let wave = ((x as f32 / 9.0).sin() + (y as f32 / 7.0).cos()) * 24.0;
        (base + wave).clamp(0.0, 255.0)
    };
    let image = RgbImage::from_fn(width, height, |x, y| {
        // Averaged with its neighbours, which is what makes the edges soft.
        let mut total = 0.0;
        for dy in 0..3 {
            for dx in 0..3 {
                total += shade(x + dx, y + dy);
            }
        }
        let value = (total / 9.0) as u8;
        image::Rgb([value, value.wrapping_add(40), value / 2 + 60])
    });
    DynamicImage::ImageRgb8(image)
        .save_with_format(path, image::ImageFormat::Png)
        .expect("writing a fixture");
}

/// The middle of a picture, saved beside it: what a crop is.
fn write_crop(from: &Path, to: &Path, keep: u32) {
    let whole = image::open(from).expect("reading a fixture").to_rgb8();
    let (width, height) = (whole.width() * keep / 100, whole.height() * keep / 100);
    let cut = image::imageops::crop_imm(
        &whole,
        (whole.width() - width) / 2,
        (whole.height() - height) / 2,
        width,
        height,
    )
    .to_image();
    DynamicImage::ImageRgb8(cut)
        .save_with_format(to, image::ImageFormat::Png)
        .expect("writing a fixture");
}

/// A crop is the same picture, and the whole-frame hash cannot see it: cut a
/// picture down and every number in that hash changes at once. The corners
/// that are still in the crop are still where they were, and that is what
/// puts the two in one set.
#[test]
fn a_crop_of_a_picture_is_found_to_be_the_same_picture() {
    let fx = fixture();
    let whole = fx.dir.path().join("whole.png");
    write_detailed(&whole, 900, 700, 4);
    write_crop(&whole, &fx.dir.path().join("cropped.png"), 60);
    // A different picture, to be sure the answer is not "everything matches".
    write_detailed(&fx.dir.path().join("other.png"), 900, 700, 91);

    let (summary, _) = scan(&fx);
    assert_eq!(summary.indexed, 3);

    let conn = on_disk(&fx);
    let sets =
        crate::matching::find_sets(&conn, crate::matching::Thresholds::at(15.0)).expect("search");
    assert_eq!(
        sets.len(),
        1,
        "the crop and the picture are not one set: {sets:?}"
    );
    let mut names: Vec<&str> = sets[0]
        .members
        .iter()
        .map(|member| member.rel_path.as_str())
        .collect();
    names.sort();
    assert_eq!(names, vec!["cropped.png", "whole.png"]);
}

/// A folder with the same picture in two of its subfolders, searched one
/// folder at a time. Copies that sit together are still one set; copies in
/// two places are two pictures and are never put together, however
/// identical they are.
#[test]
fn matching_within_folders_never_puts_two_folders_together() {
    let fx = fixture();
    std::fs::create_dir_all(fx.dir.path().join("one")).expect("subfolder");
    std::fs::create_dir_all(fx.dir.path().join("two")).expect("subfolder");
    write_detailed(&fx.dir.path().join("one/picture.png"), 400, 300, 7);
    write_detailed(&fx.dir.path().join("one/copy.png"), 400, 300, 7);
    write_detailed(&fx.dir.path().join("two/picture.png"), 400, 300, 7);

    let mut options = fx.options.clone();
    options.recurse = true;
    let cancel = AtomicBool::new(false);
    let (summary, _) = run(&fx.index, &options, &cancel, &|_| {}).expect("scan");
    assert_eq!(summary.indexed, 3);
    let conn = on_disk(&fx);

    // The whole folder at once: all three are one set.
    let together =
        crate::matching::find_sets(&conn, crate::matching::Thresholds::at(15.0)).expect("search");
    assert_eq!(
        together.len(),
        1,
        "the three copies were not one set: {together:?}"
    );
    assert_eq!(together[0].members.len(), 3);

    // One folder at a time: only the two that share a folder.
    let mut thresholds = crate::matching::Thresholds::at(15.0);
    thresholds.within_a_folder = true;
    let apart = crate::matching::find_sets(&conn, thresholds).expect("search");
    assert_eq!(
        apart.len(),
        1,
        "the copies in one folder were lost: {apart:?}"
    );
    let mut names: Vec<&str> = apart[0]
        .members
        .iter()
        .map(|member| member.rel_path.as_str())
        .collect();
    names.sort();
    assert_eq!(names, vec!["one/copy.png", "one/picture.png"]);
}

/// The same folder searched with the corners switched off. Matching one
/// picture inside another is what finds a crop and it is most of what a
/// search costs, so it can be left out, and then the crop is not found.
#[test]
fn with_the_corners_switched_off_a_crop_is_not_found() {
    let fx = fixture();
    let whole = fx.dir.path().join("whole.png");
    write_detailed(&whole, 900, 700, 4);
    write_crop(&whole, &fx.dir.path().join("cropped.png"), 60);

    scan(&fx);

    let conn = on_disk(&fx);
    let mut thresholds = crate::matching::Thresholds::at(15.0);
    thresholds.corners = false;
    let sets = crate::matching::find_sets(&conn, thresholds).expect("search");
    assert!(
        sets.is_empty(),
        "the crop was found with the corners off: {sets:?}"
    );
}

/// And with the whole frame switched off, a resize of a picture is not
/// found: that is the test the hash makes, and it is the one left out.
#[test]
fn with_the_whole_frame_switched_off_a_resize_is_not_found() {
    let fx = fixture();
    let whole = fx.dir.path().join("whole.png");
    write_detailed(&whole, 900, 700, 4);
    let picture = image::open(&whole).expect("read").to_rgb8();
    let smaller =
        image::imageops::resize(&picture, 450, 350, image::imageops::FilterType::CatmullRom);
    smaller
        .save(fx.dir.path().join("smaller.png"))
        .expect("write");

    scan(&fx);

    let conn = on_disk(&fx);
    let both =
        crate::matching::find_sets(&conn, crate::matching::Thresholds::at(15.0)).expect("search");
    assert_eq!(both.len(), 1, "the resize was not found with everything on");

    let mut thresholds = crate::matching::Thresholds::at(15.0);
    thresholds.whole_frame = false;
    thresholds.corners = false;
    let neither = crate::matching::find_sets(&conn, thresholds).expect("search");
    assert!(
        neither.is_empty(),
        "something matched with both ways off: {neither:?}"
    );
}

/// An index made by a build that had no feature fingerprint is read, given
/// the column it lacks, and every row in it read again: the fingerprint
/// version moved, and a row written under an older one is out of date by
/// definition. Nothing else about the file changes.
#[test]
fn an_index_from_a_build_without_corners_is_brought_up_to_date() {
    let fx = fixture();
    write_detailed(&fx.dir.path().join("a.png"), 300, 200, 1);
    write_detailed(&fx.dir.path().join("b.png"), 300, 200, 2);
    scan(&fx);

    // The index as an older build left it: no corners column, and rows that
    // say they were fingerprinted by the version before this one. Written
    // into the file for the manager to pick up, because that is where a file
    // from an older build comes from.
    fx.index.close().expect("let the folder go");
    let older = rusqlite::Connection::open(&fx.options.db_path).expect("the index file");
    older
        .execute_batch(
            "DROP VIEW IF EXISTS indexed_images;
             ALTER TABLE fingerprints DROP COLUMN corners;
             UPDATE fingerprints SET fingerprint_version = 1;",
        )
        .expect("making an older index");
    drop(older);

    fx.index
        .open(&fx.options.db_path)
        .expect("take the older index up");
    let conn = on_disk(&fx);
    let corners: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('fingerprints') WHERE name = 'corners'",
            [],
            |row| row.get(0),
        )
        .expect("looking for the column");
    assert_eq!(corners, 1, "the column an older index lacks was not added");

    let (summary, _) = scan(&fx);
    assert_eq!(
        summary.indexed, 2,
        "the rows from the older build were not read again"
    );
    assert_eq!(
        summary.unchanged, 0,
        "a row from the older build was left as it was"
    );

    let conn = on_disk(&fx);
    let filled: i64 = conn
        .query_row(
            "SELECT count(*) FROM fingerprints WHERE length(corners) > 0",
            [],
            |row| row.get(0),
        )
        .expect("counting");
    assert_eq!(
        filled, 2,
        "the rows were read again without their corners being written"
    );
}

struct Fixture {
    dir: tempfile::TempDir,
    options: Options,
    index: crate::index::Index,
}

fn fixture() -> Fixture {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().to_path_buf();
    let db_path = root.join(db::INDEX_FILENAME);
    let options = Options {
        root,
        db_path,
        recurse: true,
        compare_only: false,
    };
    let index = crate::index::Index::start();
    index.open(&options.db_path).expect("hold the index");
    Fixture {
        dir,
        options,
        index,
    }
}

/// The index as it is on disk, once the file has caught up with what the
/// manager holds. Reading it back is how these tests check that a pass wrote
/// what it says it wrote.
fn on_disk(fixture: &Fixture) -> db::Connection {
    fixture.index.synced().expect("wait for the file");
    db::open_and_migrate(&fixture.options.db_path).expect("read the index file")
}

/// One pass, the way the application makes one: everything through the
/// manager, which is holding the folder's index.
fn scan(fixture: &Fixture) -> (Summary, Vec<Event>) {
    let events = std::sync::Mutex::new(Vec::new());
    let cancel = AtomicBool::new(false);
    let (summary, _images) = run(&fixture.index, &fixture.options, &cancel, &|event| {
        events.lock().unwrap().push(event);
    })
    .expect("scan");
    (summary, events.into_inner().unwrap())
}

/// Indexing is reported while it is happening, not once at the end. A folder
/// smaller than one transaction's worth commits once, so a count of commits
/// says nothing until the pass is over.
#[test]
fn indexing_is_reported_while_the_folder_is_still_being_read() {
    let fx = fixture();
    let pictures = REPORT_EVERY as u32 + 50;
    for index in 0..pictures {
        write_image(&fx.dir.path().join(format!("{index}.png")), 32, 24, index);
    }
    std::fs::write(fx.dir.path().join("notes.txt"), b"not a picture").expect("fixture");

    let (summary, events) = scan(&fx);
    assert_eq!(summary.indexed, pictures as u64);

    let reported: Vec<(u64, u64)> = events
        .iter()
        .filter_map(|event| match event {
            Event::Writing { done, total, .. } => Some((*done, *total)),
            _ => None,
        })
        .collect();
    assert!(
        reported.len() >= 2,
        "indexing was only reported once, at the end"
    );
    let (done, total) = reported[0];
    // Reports come every `REPORT_EVERY` files or every `REPORT_AFTER`,
    // whichever falls first, so what the first one lands on is a count of the
    // files read by then and not a fixed number. What it may not be is
    // nothing, or the whole folder.
    assert!(
        done > 0 && done < pictures as u64,
        "the first report said {done} of {pictures}, which is not a report from part way"
    );
    // The folder holds one file that is not a picture, and the total only
    // loses it once a reader has looked at it.
    assert!(
        total >= done && total <= pictures as u64 + 1,
        "{done} of {total} is not a count of the pictures in the folder"
    );
    assert_eq!(
        reported.last().copied(),
        Some((pictures as u64, pictures as u64)),
        "the pass did not end on all of them"
    );

    // A second pass has nothing to do, and every picture in the folder is
    // still in the index, which is what the bar is counting.
    let (summary, events) = scan(&fx);
    assert_eq!(summary.indexed, 0);
    assert_eq!(summary.unchanged, pictures as u64);
    let last = events
        .iter()
        .filter_map(|event| match event {
            Event::Writing { done, total, .. } => Some((*done, *total)),
            _ => None,
        })
        .next_back();
    assert_eq!(
        last,
        Some((pictures as u64, pictures as u64)),
        "a folder that is already indexed did not report itself as indexed"
    );
}

/// A pass over a folder it has seen before knows which files it is leaving
/// alone before it reads anything, so every report it makes while it works
/// carries the whole split: what was left alone, what is being read, what has
/// gone. None of it may wait for the end.
#[test]
fn what_was_left_alone_is_reported_from_the_first_tick() {
    let fx = fixture();
    let old = 40u32;
    for index in 0..old {
        write_image(
            &fx.dir.path().join(format!("old{index}.png")),
            32,
            24,
            index,
        );
    }
    let gone = fx.dir.path().join("old0.png");
    scan(&fx);

    // Enough new pictures for the pass to report while it is still reading.
    let fresh = REPORT_EVERY as u32 + 50;
    for index in 0..fresh {
        write_image(
            &fx.dir.path().join(format!("new{index}.png")),
            32,
            24,
            1000 + index,
        );
    }
    std::fs::remove_file(&gone).expect("take one away");

    let (summary, events) = scan(&fx);
    let left_alone = old as u64 - 1;
    assert_eq!(summary.unchanged, left_alone);
    assert_eq!(summary.removed, 1);

    let progress: Vec<(u64, u64, u64)> = events
        .iter()
        .filter_map(|event| match event {
            Event::Progress {
                done,
                unchanged,
                removed,
                ..
            } => Some((*done, *unchanged, *removed)),
            _ => None,
        })
        .collect();
    assert!(
        progress.len() >= 2,
        "the pass only reported once, at the end"
    );

    for (index, (done, unchanged, removed)) in progress.iter().enumerate() {
        assert_eq!(
            *unchanged, left_alone,
            "report {index} said {unchanged} were left alone, not {left_alone}"
        );
        assert_eq!(
            *removed, 1,
            "report {index} had not counted the file that went"
        );
        assert!(
            *done >= left_alone,
            "report {index} counted {done} files, fewer than the {left_alone} it skipped"
        );
    }
}

/// Every row a pass says it indexed is in the file when the pass is over.
///
/// The pass tells the manager and the manager writes; nothing in the pass
/// opens or closes anything. What is checked here is the file, because the
/// file is what the next run of the program opens.
#[test]
fn a_pass_puts_every_row_it_indexed_into_the_file() {
    let fx = fixture();
    for n in 0..12 {
        write_image(&fx.dir.path().join(format!("{n}.png")), 40, 30, n);
    }
    let (summary, _) = scan(&fx);
    assert_eq!(summary.indexed, 12, "the pass did not index the folder");

    let conn = on_disk(&fx);
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .expect("count");
    assert_eq!(
        rows as u64, summary.indexed,
        "the file is behind what the pass indexed"
    );
    let fingerprinted: i64 = conn
        .query_row("SELECT count(*) FROM fingerprints", [], |r| r.get(0))
        .expect("count");
    assert_eq!(
        fingerprinted, 12,
        "rows reached the file without their fingerprints"
    );
    // And how far the pass reached, which the pass writes itself.
    assert_eq!(
        db::get_meta(&conn, "recurse").expect("meta").as_deref(),
        Some("1")
    );
}

/// A file whose name claims no picture format is never read. The index is
/// one of those, and so is everything SQLite keeps beside it, which is why
/// the walk no longer has to be told their names.
///
/// The proof that it is not read is its size: a file large enough that
/// reading it would be noticed, and the pass reports how many bytes it read.
#[test]
fn a_file_that_claims_no_format_is_not_read() {
    let fx = fixture();
    write_image(&fx.dir.path().join("a.png"), 40, 30, 1);
    let read_me_and_see = vec![b'x'; 4 * 1024 * 1024];
    std::fs::write(fx.dir.path().join("notes.txt"), &read_me_and_see).expect("a fixture");
    let beside = format!("{}-journal", db::INDEX_FILENAME);
    std::fs::write(fx.dir.path().join(beside), &read_me_and_see).expect("a fixture");

    let (summary, _) = scan(&fx);

    assert_eq!(
        summary.indexed, 1,
        "the pass indexed something that is not a picture"
    );
    assert_eq!(
        summary.failed, 0,
        "the pass tried to read something it should not have"
    );
    let conn = on_disk(&fx);
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .expect("count");
    assert_eq!(rows, 1, "the index holds a file that is not a picture");
}

/// The name says whether to read a file. The bytes say what it is. A JPEG
/// called `.png` is read because the name claims a format, and indexed as
/// the JPEG its first bytes say it is.
#[test]
fn what_a_file_is_comes_from_its_bytes_not_its_name() {
    let fx = fixture();
    let picture = RgbImage::from_fn(40, 30, |x, y| {
        image::Rgb([(x % 256) as u8, ((y * 3) % 256) as u8, 90])
    });
    DynamicImage::ImageRgb8(picture)
        .save_with_format(fx.dir.path().join("liar.png"), image::ImageFormat::Jpeg)
        .expect("writing a fixture");

    let (summary, _) = scan(&fx);

    assert_eq!(summary.indexed, 1, "the pass did not index it");
    let conn = on_disk(&fx);
    let format: String = conn
        .query_row("SELECT format FROM images", [], |row| row.get(0))
        .expect("the row it indexed");
    assert_eq!(format, "jpeg", "it was indexed as what it was called");
}

/// A read-ahead told what the machine has to spare.
fn read_ahead_with(spare: std::sync::Arc<std::sync::atomic::AtomicU64>) -> ReadAhead {
    ReadAhead::asking(Box::new(move || Some(spare.load(Ordering::Relaxed))))
}

const GB: u64 = 1 << 30;

#[test]
fn the_budget_is_ninety_percent_of_what_is_available() {
    let spare = std::sync::Arc::new(AtomicU64::new(10 * GB));
    let read_ahead = read_ahead_with(spare);
    assert_eq!(read_ahead.budget(0), 9 * GB);
}

/// The budget is the same whether the read-ahead is empty or holding four
/// gigabytes of the memory it is worked out from.
#[test]
fn filling_the_read_ahead_does_not_shrink_the_budget() {
    let empty = read_ahead_with(std::sync::Arc::new(AtomicU64::new(10 * GB)));
    assert_eq!(empty.budget(0), 9 * GB);

    // The same machine with four gigabytes of files read and waiting.
    let filled = read_ahead_with(std::sync::Arc::new(AtomicU64::new(6 * GB)));
    assert_eq!(filled.budget(4 * GB), 9 * GB);
}

#[test]
fn a_program_taking_memory_takes_the_budget_with_it() {
    let spare = std::sync::Arc::new(AtomicU64::new(10 * GB));
    let read_ahead = read_ahead_with(std::sync::Arc::clone(&spare));
    assert_eq!(read_ahead.budget(0), 9 * GB);

    spare.store(2 * GB, Ordering::Relaxed);
    std::thread::sleep(LOOK_AGAIN);
    assert_eq!(read_ahead.budget(0), 2 * GB / 10 * 9);
}

/// A waiting reader takes up memory the machine gave back, with no decode
/// having finished and nothing released.
#[test]
fn a_reader_waits_until_the_budget_grows_rather_than_for_a_release() {
    let spare = std::sync::Arc::new(AtomicU64::new(GB));
    let read_ahead = std::sync::Arc::new(read_ahead_with(std::sync::Arc::clone(&spare)));
    // Something already in hand, so the file that follows waits rather than
    // being let through as one too big for the budget.
    read_ahead.claim(GB / 2, &AtomicBool::new(false));

    let waiting = std::sync::Arc::clone(&read_ahead);
    let claimed = std::thread::spawn(move || {
        waiting.claim(4 * GB, &AtomicBool::new(false));
    });

    // Nothing is released. The machine simply has more to spare.
    std::thread::sleep(std::time::Duration::from_millis(50));
    spare.store(100 * GB, Ordering::Relaxed);

    let until = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !claimed.is_finished() && std::time::Instant::now() < until {
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    assert!(
        claimed.is_finished(),
        "the reader slept through the memory it was waiting for"
    );
    claimed.join().expect("the reader");
}

#[test]
fn a_file_larger_than_the_whole_budget_is_let_through() {
    let spare = std::sync::Arc::new(AtomicU64::new(GB));
    let read_ahead = read_ahead_with(spare);
    let cancel = AtomicBool::new(false);
    read_ahead.claim(8 * GB, &cancel);
    assert_eq!(*read_ahead.held.lock().expect("held"), 8 * GB);
}

#[test]
fn a_machine_that_will_not_say_gets_the_fallback() {
    let read_ahead = ReadAhead::asking(Box::new(|| None));
    assert_eq!(read_ahead.budget(0), FALLBACK_READ_AHEAD);
    assert_eq!(read_ahead.budget(4 * GB), FALLBACK_READ_AHEAD);
}

/// The platform code answers with a believable number on the machine this
/// is running on.
#[test]
fn the_machine_says_how_much_memory_is_available() {
    let spare = crate::memory::available_bytes().expect("the machine says nothing about memory");
    assert!(spare > 0, "the machine says it has no memory available");
    assert!(
        spare < 1 << 50,
        "the machine claims a petabyte of memory: {spare}"
    );
}

#[test]
fn a_first_pass_indexes_every_image() {
    let fx = fixture();
    write_image(&fx.dir.path().join("a.png"), 64, 48, 0);
    write_image(&fx.dir.path().join("b.png"), 40, 60, 7);
    let (summary, _) = scan(&fx);
    assert_eq!(summary.indexed, 2);
    assert_eq!(summary.unchanged, 0);
    assert_eq!(summary.failed, 0);
}

/// A folder is indexed, its index is rewritten the way a build keeping
/// nanoseconds would have described it, and it is passed over again: nothing
/// is read a second time.
#[test]
fn a_folder_indexed_on_one_machine_is_unchanged_on_another() {
    let fx = fixture();
    for n in 0..5 {
        write_image(&fx.dir.path().join(format!("{n}.png")), 40, 30, n);
    }
    let (first, _) = scan(&fx);
    assert_eq!(first.indexed, 5, "the folder was not indexed to begin with");

    // The same index as written by a build that kept nanoseconds.
    fx.index.close().expect("let the folder go");
    let other = rusqlite::Connection::open(&fx.options.db_path).expect("the index file");
    other
        .execute_batch(
            "DROP VIEW IF EXISTS indexed_images;
             ALTER TABLE files RENAME COLUMN mtime_seconds TO mtime_ns;
             UPDATE files SET mtime_ns = mtime_ns * 1000000000 + 654321;",
        )
        .expect("an index in nanoseconds");
    drop(other);
    fx.index
        .open(&fx.options.db_path)
        .expect("take it up again");

    let (again, _) = scan(&fx);
    assert_eq!(
        again.indexed, 0,
        "every file was read again on the other machine"
    );
    assert_eq!(
        again.unchanged, 5,
        "the files were not recognised as unchanged"
    );
}

#[test]
fn a_second_pass_over_an_unchanged_folder_reads_nothing() {
    let fx = fixture();
    write_image(&fx.dir.path().join("a.png"), 64, 48, 0);
    scan(&fx);
    let (summary, events) = scan(&fx);
    assert_eq!(summary.indexed, 0, "an unchanged folder still queued work");
    assert_eq!(summary.unchanged, 1);

    // The bar is against the folder, so it still counts the file it looked
    // at and left alone.
    let start = events.iter().find_map(|e| match e {
        Event::Start { total } => Some(*total),
        _ => None,
    });
    assert_eq!(start, Some(1), "the file it looked at was not counted");
    let last = events
        .iter()
        .filter_map(|e| match e {
            Event::Progress { done, .. } => Some(*done),
            _ => None,
        })
        .next_back();
    assert_eq!(
        last,
        Some(1),
        "the pass did not end on every file in the folder"
    );
}

#[test]
fn a_removed_file_leaves_the_index() {
    let fx = fixture();
    let path = fx.dir.path().join("a.png");
    write_image(&path, 64, 48, 0);
    write_image(&fx.dir.path().join("b.png"), 64, 48, 3);
    scan(&fx);
    std::fs::remove_file(&path).expect("remove");
    let (summary, _) = scan(&fx);
    assert_eq!(summary.removed, 1);

    let conn = on_disk(&fx);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, 1);
}

#[test]
fn a_changed_file_is_reindexed() {
    let fx = fixture();
    let path = fx.dir.path().join("a.png");
    write_image(&path, 64, 48, 0);
    scan(&fx);

    write_image(&path, 100, 80, 11);
    let (summary, _) = scan(&fx);
    assert_eq!(summary.indexed, 1);

    let conn = on_disk(&fx);
    let width: i64 = conn
        .query_row("SELECT width FROM images", [], |r| r.get(0))
        .unwrap();
    assert_eq!(width, 100);
}

/// A file that claimed a format and turned out not to be one gets no row, so
/// every later pass reads it again. The pass says how many of those it read,
/// so they are not mistaken for work that produced something.
#[test]
fn files_that_are_not_images_are_neither_indexed_nor_failures() {
    let fx = fixture();
    write_image(&fx.dir.path().join("a.png"), 64, 48, 0);
    std::fs::write(fx.dir.path().join("notes.png"), b"just some text").expect("write");
    std::fs::write(fx.dir.path().join("archive.png"), b"PK\x03\x04rest").expect("write");
    let (summary, events) = scan(&fx);
    assert_eq!(summary.indexed, 1);
    assert_eq!(summary.failed, 0);

    let counted = events
        .iter()
        .filter_map(|event| match event {
            Event::Progress { ignored, .. } => Some(*ignored),
            _ => None,
        })
        .max();
    assert_eq!(
        counted,
        Some(2),
        "the two files that are not pictures were not reported"
    );
}

/// A file the pass read and did not index is written down as read, so the
/// next pass has nothing to do with it and does not open it again.
///
/// This is what the whole folder being "as indexed" turns on: without the
/// row, a file that claims a format and is not one is missing from the index
/// for ever, and every comparison calls it new.
#[test]
fn what_a_pass_could_not_index_is_not_read_again_by_the_next_one() {
    let fx = fixture();
    write_image(&fx.dir.path().join("a.png"), 64, 48, 0);
    std::fs::write(fx.dir.path().join("notes.png"), b"just some text").expect("write");
    std::fs::write(fx.dir.path().join("bad.png"), b"\x89PNG\r\n\x1a\ntruncated").expect("write");

    let (first, _) = scan(&fx);
    assert_eq!((first.indexed, first.failed), (1, 1));

    let (again, _) = scan(&fx);
    assert_eq!(again.indexed, 0, "a file was indexed by the second pass");
    assert_eq!(
        again.failed, 0,
        "the broken file was opened and failed again"
    );
    // All three, not just the picture. A file counted as unchanged is a file
    // the pass did not open: that is what unchanged means.
    assert_eq!(
        again.unchanged, 3,
        "the files that were looked at were read again"
    );
}

/// And the folder then answers that it is as indexed, which is what decides
/// whether opening it asks anybody anything.
#[test]
fn a_folder_whose_files_were_all_looked_at_is_as_indexed() {
    let fx = fixture();
    write_image(&fx.dir.path().join("a.png"), 64, 48, 0);
    std::fs::write(fx.dir.path().join("notes.png"), b"just some text").expect("write");
    scan(&fx);

    // The same pass, told to stop once it has compared. Nothing to index and
    // nothing gone is a folder that is as its index says.
    let comparing = Options {
        compare_only: true,
        ..fx.options.clone()
    };
    let (found, _) = run(&fx.index, &comparing, &AtomicBool::new(false), &|_| {}).expect("compare");
    assert_eq!(
        (found.indexed, found.removed),
        (0, 0),
        "a file the pass looked at and could not index reads as a difference"
    );
    assert_eq!(
        found.unchanged, 2,
        "the files it accounted for are not what it said"
    );

    // A picture added since is a difference, which is the other half of the
    // same answer, and it is counted as the one file there is to read.
    write_image(&fx.dir.path().join("b.png"), 48, 32, 7);
    let (found, _) = run(&fx.index, &comparing, &AtomicBool::new(false), &|_| {}).expect("compare");
    assert_eq!((found.indexed, found.removed, found.unchanged), (1, 0, 2));
}

/// A file that was not a picture and has been replaced by one is indexed on
/// the next pass: the row says where it was and how big, and both have moved.
#[test]
fn a_file_that_became_a_picture_is_indexed_by_the_next_pass() {
    let fx = fixture();
    std::fs::write(fx.dir.path().join("a.png"), b"just some text").expect("write");
    let (first, _) = scan(&fx);
    assert_eq!(first.indexed, 0);

    write_image(&fx.dir.path().join("a.png"), 64, 48, 0);
    let (again, _) = scan(&fx);
    assert_eq!(
        again.indexed, 1,
        "the file that became a picture was not indexed"
    );
}

#[test]
fn a_malformed_image_is_reported_and_does_not_stop_the_pass() {
    let fx = fixture();
    write_image(&fx.dir.path().join("good.png"), 64, 48, 0);
    std::fs::write(fx.dir.path().join("bad.png"), b"\x89PNG\r\n\x1a\ntruncated").expect("write");
    let (summary, events) = scan(&fx);
    assert_eq!(summary.indexed, 1);
    assert_eq!(summary.failed, 1);
    let reported = events
        .iter()
        .any(|e| matches!(e, Event::Error { path, .. } if path == "bad.png"));
    assert!(reported, "the failure was not reported");
}

/// A pass over the subfolders goes into the folders somebody keeps their
/// pictures in, and not into the ones something else keeps its workings in.
/// A name beginning with a dot or an at sign is one of those.
#[test]
fn a_pass_over_the_subfolders_stays_out_of_dot_and_at_folders() {
    let fx = fixture();
    write_image(&fx.dir.path().join("top.png"), 32, 32, 0);
    for folder in [".git", ".thumbnails", "@eaDir", "keep me"] {
        std::fs::create_dir(fx.dir.path().join(folder)).expect("mkdir");
        write_image(&fx.dir.path().join(folder).join("deep.png"), 32, 32, 5);
    }
    // And a folder inside one that is kept out is kept out with it.
    std::fs::create_dir(fx.dir.path().join("@eaDir").join("under")).expect("mkdir");
    write_image(
        &fx.dir
            .path()
            .join("@eaDir")
            .join("under")
            .join("deeper.png"),
        32,
        32,
        6,
    );

    let (summary, _) = scan(&fx);
    assert_eq!(
        summary.indexed, 2,
        "the pass did not index the two it was meant to"
    );

    let conn = on_disk(&fx);
    let mut paths: Vec<String> = conn
        .prepare("SELECT rel_path FROM files")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .map(|row| row.unwrap())
        .collect();
    paths.sort();
    assert_eq!(
        paths,
        vec!["keep me/deep.png".to_string(), "top.png".to_string()]
    );
}

/// The folder somebody points the window at is the folder they meant, whether
/// or not its name begins with a dot.
#[test]
fn the_folder_the_pass_was_pointed_at_is_scanned_whatever_it_is_called() {
    let dir = tempfile::tempdir().expect("tempdir");
    let root = dir.path().join(".private");
    std::fs::create_dir(&root).expect("mkdir");
    write_image(&root.join("a.png"), 32, 32, 0);
    let db_path = root.join(db::INDEX_FILENAME);
    let options = Options {
        root: root.clone(),
        db_path: db_path.clone(),
        recurse: true,
        compare_only: false,
    };

    let index = crate::index::Index::start();
    index.open(&db_path).expect("hold the index");
    let cancel = AtomicBool::new(false);
    let (summary, _) = run(&index, &options, &cancel, &|_| {}).expect("scan");
    assert_eq!(
        summary.indexed, 1,
        "the folder it was pointed at was skipped"
    );
}

#[test]
fn without_recurse_subfolders_are_not_walked() {
    let fx = fixture();
    write_image(&fx.dir.path().join("top.png"), 32, 32, 0);
    std::fs::create_dir(fx.dir.path().join("sub")).expect("mkdir");
    write_image(&fx.dir.path().join("sub").join("deep.png"), 32, 32, 5);

    let mut shallow = fx.options.clone();
    shallow.recurse = false;
    let cancel = AtomicBool::new(false);
    let (summary, _) = run(&fx.index, &shallow, &cancel, &|_| {}).expect("scan");
    assert_eq!(summary.indexed, 1);

    // How far the pass reached is written down, because a later pass that
    // does not reach as far drops everything it cannot see.
    assert_eq!(
        fx.index.meta("recurse").expect("meta").as_deref(),
        Some("0")
    );
    let (summary, _) = run(&fx.index, &fx.options, &cancel, &|_| {}).expect("scan");
    assert_eq!(summary.indexed, 1, "the subfolder was not picked up");
    assert_eq!(
        fx.index.meta("recurse").expect("meta").as_deref(),
        Some("1")
    );
}

#[test]
fn the_index_file_does_not_index_itself() {
    let fx = fixture();
    write_image(&fx.dir.path().join("a.png"), 32, 32, 0);
    scan(&fx);
    let (summary, _) = scan(&fx);
    assert_eq!(
        summary.removed, 0,
        "a sidecar was treated as a vanished image"
    );

    let conn = on_disk(&fx);
    let paths: Vec<String> = conn
        .prepare("SELECT rel_path FROM files")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(|r| r.unwrap())
        .collect();
    assert_eq!(paths, vec!["a.png".to_string()]);
}

#[test]
fn paths_are_stored_with_forward_slashes() {
    let fx = fixture();
    std::fs::create_dir_all(fx.dir.path().join("one").join("two")).expect("mkdir");
    write_image(
        &fx.dir.path().join("one").join("two").join("deep.png"),
        32,
        32,
        0,
    );
    scan(&fx);

    let conn = on_disk(&fx);
    let path: String = conn
        .query_row("SELECT rel_path FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(path, "one/two/deep.png");
}

#[test]
fn a_cancelled_pass_leaves_a_readable_index() {
    let fx = fixture();
    for n in 0..8 {
        write_image(&fx.dir.path().join(format!("{n}.png")), 32, 32, n);
    }
    let cancel = AtomicBool::new(true);
    let (summary, _) = run(&fx.index, &fx.options, &cancel, &|_| {}).expect("scan");
    assert!(summary.cancelled);

    let conn = on_disk(&fx);
    let count: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |r| r.get(0))
        .unwrap();
    assert_eq!(count, summary.indexed as i64);
}

#[test]
fn animated_files_are_not_indexed() {
    let fx = fixture();
    write_image(&fx.dir.path().join("still.png"), 32, 32, 0);
    let mut apng = b"\x89PNG\r\n\x1a\n".to_vec();
    for kind in [b"IHDR", b"acTL"] {
        apng.extend_from_slice(&0u32.to_be_bytes());
        apng.extend_from_slice(kind);
        apng.extend_from_slice(&0u32.to_be_bytes());
    }
    std::fs::write(fx.dir.path().join("moving.png"), &apng).expect("write");

    let (summary, _) = scan(&fx);
    assert_eq!(summary.indexed, 1);
    assert_eq!(
        summary.failed, 0,
        "an animation was treated as a broken image"
    );
}

#[test]
fn the_event_stream_starts_and_ends() {
    let fx = fixture();
    write_image(&fx.dir.path().join("a.png"), 32, 32, 0);
    let (_, events) = scan(&fx);
    // A pass says what it is doing from its first moment. The folder's total
    // is not the first thing it can say, because the listing is what produces
    // it, and on a folder that answers slowly that listing is most of the
    // wait. It used to report nothing at all until then.
    assert!(
        matches!(
            events.first(),
            Some(Event::Reached(_) | Event::Walking { .. })
        ),
        "the pass said nothing until it had a total: {:?}",
        events.first()
    );
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Walking { .. })));
    assert!(events
        .iter()
        .any(|event| matches!(event, Event::Start { .. })));
    assert!(matches!(events.last(), Some(Event::Done { .. })));
}
