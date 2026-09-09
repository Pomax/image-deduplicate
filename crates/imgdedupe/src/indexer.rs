use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use imgdedupe_core::matching;
use imgdedupe_core::scan::{self, Event, Options};
use imgdedupe_core::runlog;

/// What the window needs from a pass while it runs.
#[derive(Debug, Clone)]
pub enum Update {
    /// A step of the pass has happened.
    Reached(scan::Step),
    /// The index, in the form the search works on, built the moment the pass
    /// finished writing it. Nothing in it changes until the folder does, so this
    /// is read once here and every search runs over it without touching storage.
    Images(std::sync::Arc<Vec<matching::Image>>),
    /// What the folder's own index says about how it was made. Read here rather
    /// than on the thread that draws, which used to open the index across the
    /// network for three small values and hold the window for as long as that
    /// took.
    Settings(crate::notes::Notes),
    /// The folder is being listed, and this is how many files that has found so
    /// far. There is no total yet: the listing is what produces it.
    Walking { found: u64, of: Option<u64> },
    Start { total: u64 },
    Progress { done: u64, per_sec: u64, unchanged: u64, removed: u64, ignored: u64 },
    Indexed { done: u64, total: u64, read: u64, unchanged: u64, ignored: u64 },
    Failed { path: String, message: String },
    Done { indexed: u64, removed: u64, failed: u64, elapsed_ms: u64 },
    /// The pass is over, one way or another. Nothing else follows it.
    Finished { cancelled: bool, error: Option<String> },
}

/// A pass, running on a thread of its own.
pub struct Run {
    cancel: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    pub updates: Receiver<Update>,
}

impl Run {
    /// Ask the pass to stop. It puts down whatever file it is on, commits what
    /// it has, and closes the index, so what is on disk is always consistent.
    pub fn cancel(&mut self) {
        runlog::log_line!("cancelling the pass");
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Run {
    /// Ask the pass to stop, and let go of it without waiting.
    ///
    /// This used to wait for the thread. Closing the window drops the run on the
    /// thread that draws, so the window closed only once the pass had noticed it
    /// was cancelled. The pass checks between files, and a file on a network
    /// mount is read by a call the operating system will not interrupt, so that
    /// wait was as long as the other machine took to answer. A window that will
    /// not close is not something a person can do anything about, and a thread in
    /// an uninterruptible wait does not die on a signal either, so the process
    /// could not be killed.
    ///
    /// The pass stops at the next file it looks at, commits what it has and
    /// closes the index. On the way out of the program it is the process ending
    /// that ends it.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        self.thread.take();
    }
}

pub fn start(
    index: imgdedupe_core::index::Index,
    root: &Path,
    db_path: &Path,
    recurse: bool,
    compare_only: bool,
) -> Result<Run> {
    let options = Options {
        root: root.to_path_buf(),
        db_path: db_path.to_path_buf(),
        recurse,
        compare_only,
    };
    runlog::log_line!(
        "{} {} (recurse {recurse}, db {})",
        if compare_only { "comparing" } else { "indexing" },
        root.display(),
        db_path.display()
    );

    let cancel = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&cancel);
    let (send, updates) = mpsc::channel();

    let thread = std::thread::spawn(move || {
        // `scan::run` reports from every thread it decodes on, and a channel's
        // sender is not shared between them without this.
        let send = Mutex::new(send);
        // What the folder's index says about itself, out of one whole-file read,
        // before anything is opened for writing. `recurse` decides which files
        // the pass is about to look at, so it has to be known first.
        let say = |step: scan::Step| {
            if let Ok(sender) = send.lock() {
                let _ = sender.send(Update::Reached(step));
            }
        };

        say(scan::Step::StartedOpeningTheIndexForWriting);
        #[cfg(feature = "logging")]
        let opening = std::time::Instant::now();
        let outcome = index.open(&options.db_path).and_then(|()| {
            runlog::log_line!(
                "  open the index for writing: {:.2}s",
                opening.elapsed().as_secs_f64()
            );
            say(scan::Step::FinishedOpeningTheIndexForWriting);

            // From the manager, which holds the index. `recurse` decides which
            // files the pass is about to look at, so it is read before the walk
            // starts.
            say(scan::Step::StartedReadingTheIndexSettings);
            let notes = crate::notes::read(&index);
            // Read and handed over, not applied: how far this pass reaches is
            // what the window was set to when the Scan button was pressed. A
            // folder that reaches into its subfolders says so when it is opened,
            // which is what puts the box up; taking the box down again and
            // scanning is somebody saying they mean the folder itself.
            if let Ok(sender) = send.lock() {
                let _ = sender.send(Update::Settings(notes));
            }
            say(scan::Step::FinishedReadingTheIndexSettings);
            let report = |event: Event| {
                if let Ok(sender) = send.lock() {
                    let _ = sender.send(update(event));
                }
            };
            let (summary, images) = scan::run(&index, &options, &stop, &report)?;
            // Handed over before the pass reports itself finished, so whatever
            // runs next already has it and no search ever asks the database.
            if let Some(images) = images {
                if let Ok(sender) = send.lock() {
                    let _ = sender.send(Update::Images(std::sync::Arc::new(images)));
                }
            }
            Ok(summary)
        });
        let last = match outcome {
            Ok(summary) => Update::Finished { cancelled: summary.cancelled, error: None },
            Err(err) => {
                runlog::log_line!("the pass stopped: {err:#}");
                Update::Finished { cancelled: false, error: Some(format!("{err:#}")) }
            }
        };
        if let Ok(sender) = send.lock() {
            let _ = sender.send(last);
        };
    });

    Ok(Run { cancel, thread: Some(thread), updates })
}

fn update(event: Event) -> Update {
    match event {
        Event::Reached(step) => Update::Reached(step),
        Event::Walking { found, of } => Update::Walking { found, of },
        Event::Start { total } => Update::Start { total },
        Event::Progress { done, per_sec, unchanged, removed, ignored, .. } => {
            Update::Progress { done, per_sec, unchanged, removed, ignored }
        }
        Event::Writing { done, total, read, unchanged, ignored } => {
            Update::Indexed { done, total, read, unchanged, ignored }
        }
        Event::Error { path, message } => Update::Failed { path, message },
        Event::Done { indexed, removed, failed, elapsed_ms } => {
            Update::Done { indexed, removed, failed, elapsed_ms }
        }
    }
}

#[cfg(test)]
mod tests {
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
        start(imgdedupe_core::index::Index::start(), root, db_path, recurse, false)
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
            seen.iter().any(|update| matches!(update, Update::Start { total: 3 })),
            "the pass did not announce the folder's total: {seen:?}"
        );
        assert!(
            seen.iter().any(|update| matches!(update, Update::Done { indexed: 3, .. })),
            "the pass did not say what it indexed: {seen:?}"
        );
        assert!(
            matches!(seen.last(), Some(Update::Finished { cancelled: false, error: None })),
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
        assert!(stop.load(Ordering::Relaxed), "the pass was not asked to stop");
        // What the index looks like afterwards is not asserted here. Dropping no
        // longer waits for the pass, so the pass is still winding up at this
        // point and the manager may still be writing the file.
        // `a_pass_puts_every_row_it_indexed_into_the_file` is where that is
        // checked, against a pass that has finished.
    }

    /// The measurements against a real folder of photographs. Not in the
    /// repository: see `docs/tests.md`.
    #[cfg(feature = "local")]
    #[path = "../../../../../local/indexer.rs"]
    mod local;
}
