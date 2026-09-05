use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use imgdedupe_core::matching;
use imgdedupe_core::scan::{self, Event, Options};
use imgdedupe_core::{db, runlog};

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
    Settings {
        recurse: Option<bool>,
        disposal: Option<String>,
        move_dir: Option<String>,
        multi_select: Option<bool>,
        match_whole_frame: Option<bool>,
        match_corners: Option<bool>,
    },
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

pub fn start(root: &Path, db_path: &Path, recurse: bool) -> Result<Run> {
    let options = Options {
        root: root.to_path_buf(),
        db_path: db_path.to_path_buf(),
        recurse,
    };
    runlog::log_line!(
        "indexing {} (recurse {recurse}, db {})",
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

        let mut options = options;
        say(scan::Step::StartedOpeningTheIndexForWriting);
        #[cfg(feature = "logging")]
        let opening = std::time::Instant::now();
        let outcome = db::open(&options.db_path).and_then(|mut conn| {
            runlog::log_line!(
                "  open the index for writing: {:.2}s",
                opening.elapsed().as_secs_f64()
            );
            say(scan::Step::FinishedOpeningTheIndexForWriting);

            // Off the connection that has just read the file, rather than reading
            // the same file a second time. `recurse` decides which files the pass
            // is about to look at, so it is read before the walk starts.
            say(scan::Step::StartedReadingTheIndexSettings);
            let meta = |key: &str| db::get_meta(&conn, key).ok().flatten();
            let recurse = meta("recurse").map(|value| value == "1");
            if let Some(recurse) = recurse {
                options.recurse = recurse;
            }
            if let Ok(sender) = send.lock() {
                let _ = sender.send(Update::Settings {
                    recurse,
                    disposal: meta("disposal"),
                    move_dir: meta("move_dir"),
                    multi_select: meta("multi_select").map(|value| value == "1"),
                    match_whole_frame: meta("match_whole_frame").map(|value| value == "1"),
                    match_corners: meta("match_corners").map(|value| value == "1"),
                });
            }
            say(scan::Step::FinishedReadingTheIndexSettings);
            let report = |event: Event| {
                if let Ok(sender) = send.lock() {
                    let _ = sender.send(update(event));
                }
            };
            let (summary, images) = scan::run(&mut conn, &options, &stop, &report)?;
            // Only when the pass changed something. Writing the file back is a
            // few megabytes across the network, and a pass over a folder where
            // nothing has moved has nothing to say that the file does not already
            // hold.
            if summary.indexed > 0 || summary.removed > 0 {
                db::close(conn, &options.db_path)?;
            } else {
                runlog::log_line!("the index is unchanged, so it is not written back");
                drop(conn);
            }
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

        let mut run = start(dir.path(), &db_path, false).expect("start");
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

        let mut run = start(dir.path(), &db_path, false).expect("start");
        let seen = drain(&mut run);
        assert!(
            matches!(seen.last(), Some(Update::Finished { error: Some(_), .. })),
            "the pass said nothing about failing: {seen:?}"
        );
    }

    /// Dropping a run must not wait for the pass to notice it has been cancelled.
    ///
    /// Closing the window drops the run on the thread that draws. When the pass is
    /// inside a call the operating system will not interrupt, which is what a read
    /// of a file on a network mount is, waiting for it holds that thread for as
    /// long as the other machine takes. The window then cannot be closed, and the
    /// process cannot be killed either, because a thread in an uninterruptible
    /// wait does not die on a signal.
    /// Reading the existing index is a read of one small file, not thousands of
    /// round trips, run against the folder the application is set to.
    ///
    /// SQLite reads a database in pages as a query asks for them, and on a network
    /// mount every page is its own round trip. This times the whole stretch from
    /// the pass starting to the bars having a total, which is the listing plus the
    /// index read plus the diff.
    #[test]
    #[ignore = "runs a real pass over the folder the application is set to"]
    fn a_pass_reaches_its_total_without_reading_the_index_page_by_page() {
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = crate::headless::default_db_path(&folder);

        let started = std::time::Instant::now();
        let run = start(&folder, &db_path, false).expect("start");
        let mut reached = None;
        while let Ok(update) = run.updates.recv_timeout(std::time::Duration::from_secs(120)) {
            if let Update::Start { total } = update {
                reached = Some((started.elapsed(), total));
                break;
            }
        }
        let (took, total) = reached.expect("the pass never announced a total");
        assert!(
            took < std::time::Duration::from_secs(5),
            "the bars had no total for {took:?} ({total} files)"
        );
    }

    /// How fast files are actually read and indexed, against the folder the
    /// application is set to. Prints the rate rather than asserting a number,
    /// because the number is the point.
    #[test]
    #[ignore = "runs a real pass over the folder the application is set to"]
    fn how_fast_new_files_are_read_and_indexed() {
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = crate::headless::default_db_path(&folder);

        let run = start(&folder, &db_path, false).expect("start");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(60);
        let mut best = 0;
        let mut last = 0;
        while std::time::Instant::now() < deadline {
            match run.updates.recv_timeout(std::time::Duration::from_secs(10)) {
                Ok(Update::Progress { per_sec, done, .. }) => {
                    best = best.max(per_sec);
                    last = done;
                }
                Ok(Update::Finished { .. }) | Err(_) => break,
                Ok(_) => {}
            }
        }
        println!("read {last} files, peak {best} per second");
        assert!(best > 0, "the pass never reported a rate");
    }

    /// The window hears from a pass almost immediately, run against the folder
    /// the application is set to.
    ///
    /// Nothing was reported until the listing, the index read and the diff had all
    /// finished, and the listing asked the file system about every file one at a
    /// time. On a folder on a network mount that was over thirty seconds of a
    /// window that had been told nothing, which is indistinguishable from a window
    /// that has locked up.
    #[test]
    #[ignore = "runs a real pass over the folder the application is set to"]
    fn a_pass_says_something_almost_at_once() {
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = crate::headless::default_db_path(&folder);

        let started = std::time::Instant::now();
        let run = start(&folder, &db_path, false).expect("start");
        let first = run.updates.recv_timeout(std::time::Duration::from_secs(60));
        let waited = started.elapsed();
        assert!(first.is_ok(), "the pass said nothing at all in {waited:?}");
        assert!(
            waited < std::time::Duration::from_secs(3),
            "the window was told nothing for {waited:?} after the pass began"
        );
    }

    /// Run against the folder the application is set to, which is the only place
    /// this fault exists. On a local disk the pass sees the cancel flag within
    /// milliseconds and waiting for it looks free; it is a lock only when the
    /// pass is inside a call the operating system will not interrupt, which is
    /// what a read from a network mount is.
    #[test]
    #[ignore = "runs a real pass over the folder the application is set to"]
    fn dropping_a_run_does_not_wait_for_the_pass_to_finish() {
        let folder = crate::settings::Settings::load()
            .folder
            .expect("the application has no folder set to test against");
        let db_path = crate::headless::default_db_path(&folder);

        let run = start(&folder, &db_path, false).expect("start");
        // Long enough to be inside the folder. Nothing is reported during the
        // listing, so there is no message to wait for: this is the stretch where
        // the pass is in a read that will not be interrupted, and it is the
        // stretch a person closing the window lands in.
        std::thread::sleep(std::time::Duration::from_secs(2));

        let at = std::time::Instant::now();
        drop(run);
        let waited = at.elapsed();
        assert!(
            waited < std::time::Duration::from_millis(50),
            "dropping the run held the thread that draws for {waited:?}, \
             which is how long the window would refuse to close"
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

        let run = start(dir.path(), &db_path, false).expect("start");
        let stop = std::sync::Arc::clone(&run.cancel);
        drop(run);
        assert!(stop.load(Ordering::Relaxed), "the pass was not asked to stop");
        // What the index looks like afterwards is not asserted here. Dropping no
        // longer waits for the pass, so the pass is still winding up at this
        // point and the write-ahead log beside the index may or may not be gone
        // yet. `closing_an_index_leaves_one_file_behind` is where that is checked,
        // against a pass that has finished.
    }
}
