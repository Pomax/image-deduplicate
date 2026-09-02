use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};

use anyhow::Result;
use imgdedupe_core::scan::{self, Event, Options};
use imgdedupe_core::{db, runlog};

/// What the window needs from a pass while it runs.
#[derive(Debug, Clone)]
pub enum Update {
    Start { total: u64 },
    Progress { done: u64, per_sec: u64, unchanged: u64, removed: u64, ignored: u64 },
    Indexed { done: u64, total: u64 },
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
        runlog::line("cancelling the pass");
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Drop for Run {
    /// A pass outliving the window would hold the index open and write to a
    /// database nobody is watching. Closing the window takes the pass with it.
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

pub fn start(root: &Path, db_path: &Path, recurse: bool) -> Result<Run> {
    let options = Options {
        root: root.to_path_buf(),
        db_path: db_path.to_path_buf(),
        recurse,
    };
    runlog::line(&format!(
        "indexing {} (recurse {recurse}, db {})",
        root.display(),
        db_path.display()
    ));

    let cancel = Arc::new(AtomicBool::new(false));
    let stop = Arc::clone(&cancel);
    let (send, updates) = mpsc::channel();

    let thread = std::thread::spawn(move || {
        // `scan::run` reports from every thread it decodes on, and a channel's
        // sender is not shared between them without this.
        let send = Mutex::new(send);
        let outcome = db::open(&options.db_path).and_then(|mut conn| {
            let report = |event: Event| {
                if let Ok(sender) = send.lock() {
                    let _ = sender.send(update(event));
                }
            };
            let summary = scan::run(&mut conn, &options, &stop, &report)?;
            db::close(conn)?;
            Ok(summary)
        });
        let last = match outcome {
            Ok(summary) => Update::Finished { cancelled: summary.cancelled, error: None },
            Err(err) => {
                runlog::line(&format!("the pass stopped: {err:#}"));
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
        Event::Start { total } => Update::Start { total },
        Event::Progress { done, per_sec, unchanged, removed, ignored, .. } => {
            Update::Progress { done, per_sec, unchanged, removed, ignored }
        }
        Event::Writing { done, total } => Update::Indexed { done, total },
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

        assert!(matches!(seen.first(), Some(Update::Start { total: 3 })));
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
        // Dropping joins, so by here the index is closed and one file is all
        // that is left of it.
        assert!(!db_path.with_file_name("index.sqlite-wal").exists());
    }
}
