use super::*;

use image::{DynamicImage, RgbImage};

fn write_image(path: &Path, seed: u32) {
    let image = RgbImage::from_fn(48, 32, |x, y| {
        image::Rgb([((x + seed) % 256) as u8, ((y * 3) % 256) as u8, 90])
    });
    DynamicImage::ImageRgb8(image)
        .save_with_format(path, image::ImageFormat::Png)
        .expect("a fixture");
}

/// A pass, started the way the window starts one: on a manager of its own,
/// holding nothing until the pass gives it the folder.
fn start_a_pass(root: &Path, db_path: &Path, recurse: bool) -> Result<Run> {
    start(
        imgdedupe_core::index::Index::start(),
        root,
        db_path,
        recurse,
        false,
    )
}

fn drain(run: &mut Run) -> Vec<Update> {
    let mut seen = Vec::new();
    while let Ok(update) = run.updates.recv() {
        let last = matches!(update, Update::Finished { .. });
        seen.push(update);
        if last {
            break;
        }
    }
    seen
}

/// A pass runs here, on a thread, and reports as it goes. Nothing is spawned
/// and nothing is parsed back out of a pipe.
#[test]
fn a_pass_reports_what_it_did_and_then_says_it_is_over() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..3 {
        write_image(&dir.path().join(format!("{index}.png")), index);
    }
    let db_path = dir.path().join("index.sqlite");

    let mut run = start_a_pass(dir.path(), &db_path, false).expect("start");
    let seen = drain(&mut run);

    // Not the first thing said any more: a pass reports the steps it goes
    // through from the moment it begins, and the folder's total is only known
    // once the listing is over.
    assert!(
        seen.iter()
            .any(|update| matches!(update, Update::Start { total: 3 })),
        "the pass did not announce the folder's total: {seen:?}"
    );
    assert!(
        seen.iter()
            .any(|update| matches!(update, Update::Done { indexed: 3, .. })),
        "the pass did not say what it indexed: {seen:?}"
    );
    assert!(
        matches!(
            seen.last(),
            Some(Update::Finished {
                cancelled: false,
                error: None
            })
        ),
        "the pass did not finish cleanly: {seen:?}"
    );
    assert!(db_path.is_file(), "no index was written");
}

/// An index that cannot be opened is the pass's own failure to report, not a
/// window left waiting for a run that never says anything.
#[test]
fn a_pass_that_cannot_open_its_index_says_so_and_stops() {
    let dir = tempfile::tempdir().expect("tempdir");
    let db_path = dir.path().join("index.sqlite");
    std::fs::write(&db_path, b"not a database at all").expect("fixture");

    let mut run = start_a_pass(dir.path(), &db_path, false).expect("start");
    let seen = drain(&mut run);
    assert!(
        matches!(seen.last(), Some(Update::Finished { error: Some(_), .. })),
        "the pass said nothing about failing: {seen:?}"
    );
}

/// A pass left running after the window has gone would write to an index
/// nobody is watching. Dropping the run takes the pass with it.
#[test]
fn dropping_a_run_stops_the_pass_it_started() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..40 {
        write_image(&dir.path().join(format!("{index}.png")), index);
    }
    let db_path = dir.path().join("index.sqlite");

    let run = start_a_pass(dir.path(), &db_path, false).expect("start");
    let stop = std::sync::Arc::clone(&run.cancel);
    drop(run);
    assert!(
        stop.load(Ordering::Relaxed),
        "the pass was not asked to stop"
    );
    // What the index looks like afterwards is not asserted here. Dropping no
    // longer waits for the pass, so the pass is still winding up at this
    // point and the manager may still be writing the file.
    // `a_pass_puts_every_row_it_indexed_into_the_file` is where that is
    // checked, against a pass that has finished.
}
