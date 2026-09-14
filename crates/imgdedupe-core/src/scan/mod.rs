use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rayon::prelude::*;
use serde::Serialize;

use crate::db::{self, Record};
use crate::decode::decode_for_indexing;
use crate::dirlist;
use crate::fingerprint::{fingerprint, FINGERPRINT_VERSION};
use crate::format::{self, SNIFF_LEN};
use crate::frames;
use crate::runlog;

mod diff;
mod one;
mod readahead;
mod walk;

use self::diff::*;
use self::one::*;
use self::readahead::*;
use self::walk::*;

/// Records written per transaction. A killed run loses at most this many.
const BATCH: usize = 5000;
/// Files between progress reports, when they are arriving fast enough that a
/// count is what limits how often the window hears anything.
const REPORT_EVERY: u64 = 200;
/// Time between progress reports when they are not. Reading a file off another
/// machine takes about a tenth of a second, so a report every two hundred files
/// is a report every twenty seconds and the bars stand still in between.
const REPORT_AFTER: std::time::Duration = std::time::Duration::from_millis(100);

/// A thing the pass has reached, reported the moment it happens.
///
/// Separate from the counters: these say which part of a pass is running, so a
/// stretch that produces no numbers is still visibly something rather than a
/// window sitting still.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Step {
    StartedReadingTheIndexSettings,
    FinishedReadingTheIndexSettings,
    StartedOpeningTheIndexForWriting,
    FinishedOpeningTheIndexForWriting,
    StartedConvertingTheIndex,
    FinishedConvertingTheIndex,
    StartedLookingForTheTotal,
    FoundTheTotal,
    LoadedIndexIntoMemory,
    ListedTheFolder,
    CrossReferencedWithTheIndex,
    CountedWhatChanged,
    StartedReadingNewFiles,
    FinishedReadingNewFiles,
    StartedIndexingNewFiles,
    FinishedIndexingNewFiles,
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
    /// One of the pass's steps has happened.
    Reached(Step),
    /// Files the listing has found so far.
    ///
    /// A count and not a fraction: the total is what the listing produces, so it
    /// does not exist yet. Listing a folder on another machine is one call per
    /// file, and this used to report nothing from the first file to the last,
    /// which is a window that has been told nothing for as long as that takes.
    Walking {
        found: u64,
        /// What the folder said it holds when asked, in one call, before the
        /// listing began. `None` when nothing can answer that, and then the count
        /// is all there is.
        of: Option<u64>,
    },
    Start {
        total: u64,
    },
    Progress {
        done: u64,
        new: u64,
        changed: u64,
        unchanged: u64,
        removed: u64,
        /// Files that were read and turned out not to be a picture this build can
        /// index. They are read on every pass, because a file with no fingerprint
        /// has no row and the next pass cannot tell it has been seen.
        ignored: u64,
        per_sec: u64,
    },
    /// How much of the folder the index holds: the pictures already in it plus
    /// the ones this pass has added, out of every picture the folder holds. A
    /// folder with nothing to do is all of it.
    Writing {
        done: u64,
        total: u64,
        /// The read side as it stands at the same moment, so the counters that
        /// come from it move with this bar as well as with the other.
        read: u64,
        unchanged: u64,
        ignored: u64,
    },
    Error {
        path: String,
        message: String,
    },
    Done {
        indexed: u64,
        removed: u64,
        failed: u64,
        elapsed_ms: u64,
    },
}

#[derive(Debug, Clone)]
pub struct Options {
    pub root: PathBuf,
    pub db_path: PathBuf,
    pub recurse: bool,
    /// Stop after comparing the folder with the index, without reading a file.
    ///
    /// A pass begins by listing the folder, reading what the index knows, and
    /// working out the difference, and it reports every one of those. Opening a
    /// folder needs that answer and nothing after it, so it asks for the same
    /// pass and says where to stop. There is one comparison in this program and
    /// this is it.
    pub compare_only: bool,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub indexed: u64,
    pub removed: u64,
    pub failed: u64,
    pub unchanged: u64,
    pub cancelled: bool,
}

/// Threads doing nothing but pulling file bytes into memory.
///
/// Far more than there are cores, on purpose. A read from another machine is a
/// wait, not work, so the number that matters is how many requests are in flight
/// rather than how many cores there are to run them on. Reading and decoding used
/// to be the same task on one rayon thread, which capped the whole pass at one
/// core's worth of files in flight and left threads waiting on the network while
/// others sat idle with nothing to decode.
const READERS: usize = 64;

/// Run one indexing pass. Files are read into memory by a wide pool and decoded
/// across every core; writing runs on one thread in batched transactions, so the
/// index is consistent at every commit.
pub fn run(
    index: &crate::catalogue::Catalogue,
    options: &Options,
    cancel: &AtomicBool,
    report: &(dyn Fn(Event) + Sync),
) -> Result<(Summary, Option<Vec<crate::matching::Image>>)> {
    let started = Instant::now();
    #[cfg(feature = "logging")]
    let at = Instant::now();
    let candidates = walk(options, cancel, report).context("walking the folder")?;
    #[cfg(feature = "logging")]
    let found = candidates.len();
    report(Event::Reached(Step::FoundTheTotal));
    report(Event::Reached(Step::ListedTheFolder));
    runlog::log_line!("walk: {:.2}s, {found} files", at.elapsed().as_secs_f64());

    #[cfg(feature = "logging")]
    let at = Instant::now();
    if cancel.load(Ordering::Relaxed) {
        return Ok((
            Summary {
                cancelled: true,
                ..Summary::default()
            },
            None,
        ));
    }
    // Straight out of the manager, which has held the index in memory since the
    // folder was taken up, so this reads nothing from anywhere.
    #[cfg(feature = "logging")]
    let step = Instant::now();
    let known = index.known().context("reading the existing index")?;
    runlog::log_line!("  known paths: {:.2}s", step.elapsed().as_secs_f64());
    report(Event::Reached(Step::LoadedIndexIntoMemory));
    runlog::log_line!(
        "load index: {:.2}s, {} rows",
        at.elapsed().as_secs_f64(),
        known.len()
    );

    let Diff {
        to_index,
        removed,
        unchanged,
    } = diff(candidates, &known);
    report(Event::Reached(Step::CrossReferencedWithTheIndex));
    report(Event::Reached(Step::CountedWhatChanged));
    runlog::log_line!(
        "diff: {unchanged} unchanged, {} to index, {} gone",
        to_index.len(),
        removed.len()
    );

    // Asked only what the folder looks like against the index. Everything below
    // reads files, and there is nothing here to read them for.
    if options.compare_only {
        report(indexed_so_far(
            0,
            unchanged,
            to_index.len() as u64,
            &AtomicU64::new(0),
            &AtomicU64::new(0),
        ));
        // What it found: the files the index does not have, and the files it has
        // that the folder does not. The caller decides what to do about them.
        report(Event::Done {
            indexed: to_index.len() as u64,
            removed: removed.len() as u64,
            failed: 0,
            elapsed_ms: started.elapsed().as_millis() as u64,
        });
        return Ok((
            Summary {
                indexed: to_index.len() as u64,
                removed: removed.len() as u64,
                unchanged,
                ..Summary::default()
            },
            None,
        ));
    }

    // The index is loaded and nothing in the folder has moved, so it is already
    // the answer. Convert it here, off the copy that is still in memory, rather
    // than leaving it for whatever runs next to read the file all over again.
    if cancel.load(Ordering::Relaxed) {
        return Ok((
            Summary {
                cancelled: true,
                unchanged,
                ..Summary::default()
            },
            None,
        ));
    }

    let mut images = None;
    if unchanged == 0 && to_index.is_empty() {
        // An empty folder. There is nothing to convert and nothing to search, and
        // reporting either is reporting work that was never done.
    } else if to_index.is_empty() && removed.is_empty() {
        report(Event::Reached(Step::StartedConvertingTheIndex));
        #[cfg(feature = "logging")]
        let step = Instant::now();
        images = index
            .images(
                std::sync::Arc::new(AtomicBool::new(false)),
                std::sync::Arc::new(|_| {}),
            )
            .context("converting the index")?;
        runlog::log_line!("  convert to memory: {:.2}s", step.elapsed().as_secs_f64());
        report(Event::Reached(Step::FinishedConvertingTheIndex));
    }

    let mut summary = Summary {
        unchanged,
        ..Summary::default()
    };

    if !removed.is_empty() {
        #[cfg(feature = "logging")]
        let at = Instant::now();
        summary.removed = index.delete_paths(removed.clone())? as u64;
        runlog::log_line!(
            "drop gone: {:.2}s, {} rows",
            at.elapsed().as_secs_f64(),
            summary.removed
        );
    }

    let total = to_index.len() as u64;
    // The bar is against the folder, not against the work: a pass over a folder
    // it has seen before still looks at every file to find what is new and what
    // has gone.
    report(Event::Start {
        total: unchanged + total,
    });

    let done = AtomicU64::new(0);
    let ignored = AtomicU64::new(0);
    // Milliseconds into the pass at which the last progress report went out, so
    // the reading reports on a clock rather than on a count of files.
    let reported = AtomicU64::new(0);
    // What the rate was over the last second, and the count and time it was
    // worked out from. Files divided by the whole pass so far is an average, and
    // an average of a run that started fast falls for the rest of it however
    // steady the real speed is. This is how many files went by in the last
    // second, which is what "per second" says.
    let rate_now = AtomicU64::new(0);
    let rate_from_count = AtomicU64::new(0);
    let rate_from_ms = AtomicU64::new(0);
    // How much is read but not yet decoded, and which file the readers take next.
    // Declared out here because the reader threads borrow them and outlive the
    // scope's own body.
    let read_ahead = ReadAhead::new();
    let next = std::sync::atomic::AtomicUsize::new(0);
    let spent = Spent::default();
    let (send, recv) = mpsc::channel::<Outcome>();

    let write_result = std::thread::scope(|scope| -> Result<(u64, u64)> {
        let writer = scope.spawn(|| -> Result<(u64, u64)> {
            let mut indexed = 0u64;
            let mut failed = 0u64;
            let mut pending: Vec<Box<Record>> = Vec::with_capacity(BATCH);
            let mut told = Instant::now();
            let scanned_at = now_seconds();
            // Only when there is something to index. A step that had nothing to
            // do did not happen, and saying it did is a claim that work was done.
            if !to_index.is_empty() {
                report(Event::Reached(Step::StartedIndexingNewFiles));
            }

            let flush = |index: &crate::catalogue::Catalogue,
                         pending: &mut Vec<Box<Record>>|
             -> Result<()> {
                if pending.is_empty() {
                    return Ok(());
                }
                #[cfg(feature = "logging")]
                let rows = pending.len();
                #[cfg(feature = "logging")]
                let at = Instant::now();
                let batch: Vec<Record> = pending.drain(..).map(|it| *it).collect();
                index.upsert(batch, scanned_at)?;
                #[cfg(feature = "logging")]
                let inserted = at.elapsed().as_secs_f64();
                #[cfg(feature = "logging")]
                let at = Instant::now();
                runlog::log_line!(
                    "commit: {rows} rows, {inserted:.2}s inserting and {:.2}s committing",
                    at.elapsed().as_secs_f64()
                );
                pending.clear();
                Ok(())
            };

            // The files that turned out not to be pictures, batched the way the
            // pictures are. Writing them down is what stops the next pass reading
            // them again and the next comparison calling them new.
            let mut looked: Vec<db::Looked> = Vec::new();
            let note =
                |index: &crate::catalogue::Catalogue, looked: &mut Vec<db::Looked>| -> Result<()> {
                    if looked.is_empty() {
                        return Ok(());
                    }
                    let batch: Vec<db::Looked> = looked.drain(..).collect();
                    index.not_pictures(batch, scanned_at)
                };

            for outcome in recv {
                match outcome {
                    Outcome::Indexed(record) => {
                        indexed += 1;
                        pending.push(record);
                        if pending.len() >= BATCH {
                            flush(index, &mut pending)?;
                        }
                        if indexed % REPORT_EVERY == 0 || told.elapsed() >= REPORT_AFTER {
                            report(indexed_so_far(indexed, unchanged, total, &done, &ignored));
                            told = Instant::now();
                        }
                    }
                    Outcome::NotAnImage(one) => {
                        looked.push(one);
                        if looked.len() >= BATCH {
                            note(index, &mut looked)?;
                        }
                    }
                    Outcome::Failed {
                        looked: one,
                        message,
                    } => {
                        failed += 1;
                        let path = one.rel_path.clone();
                        looked.push(one);
                        if looked.len() >= BATCH {
                            note(index, &mut looked)?;
                        }
                        report(Event::Error { path, message });
                    }
                }
            }

            flush(index, &mut pending)?;
            note(index, &mut looked)?;
            report(indexed_so_far(indexed, unchanged, total, &done, &ignored));
            if !to_index.is_empty() {
                report(Event::Reached(Step::FinishedIndexingNewFiles));
            }
            Ok((indexed, failed))
        });

        // From when the reading began, not from when the pass began. Dividing by
        // the whole pass mixes the listing into the rate and reports a number
        // that is not the speed of anything.
        let reading_since = Instant::now();
        let announce = |count: u64| {
            #[cfg(feature = "logging")]
            let elapsed = reading_since.elapsed().as_secs_f64().max(0.001);
            // Files in the last second, worked out once a second. Not files
            // divided by the whole pass, which is an average and falls for the
            // rest of a run that began fast however steady the real speed is.
            let now_ms = reading_since.elapsed().as_millis() as u64;
            let since = now_ms.saturating_sub(rate_from_ms.load(Ordering::Relaxed));
            if since >= 1000 {
                let went_by = count.saturating_sub(rate_from_count.load(Ordering::Relaxed));
                rate_now.store(went_by * 1000 / since, Ordering::Relaxed);
                rate_from_count.store(count, Ordering::Relaxed);
                rate_from_ms.store(now_ms, Ordering::Relaxed);
            }
            // Where the time is going, while it is going, rather than once at the
            // end of a pass that takes minutes. These are summed over every
            // thread, so they are larger than the wall clock.
            runlog::log_line!(
                "rate: {count} files in {elapsed:.1}s, {} in the last second; over every \
                 thread {:.1}s reading, {:.1}s decoding, {:.1}s fingerprinting; {} bytes \
                 read ahead",
                rate_now.load(Ordering::Relaxed),
                Spent::seconds(&spent.reading),
                Spent::seconds(&spent.decoding),
                Spent::seconds(&spent.fingerprinting),
                read_ahead.held.lock().map(|held| *held).unwrap_or(0),
            );
            report(Event::Progress {
                // Every file in the folder has been looked at, including the ones
                // whose size and timestamp said there was nothing to do.
                done: unchanged + count,
                new: count,
                changed: 0,
                unchanged,
                removed: summary.removed,
                ignored: ignored.load(Ordering::Relaxed),
                per_sec: rate_now.load(Ordering::Relaxed),
            });
        };

        if !to_index.is_empty() {
            report(Event::Reached(Step::StartedReadingNewFiles));
        }

        // Stage one: pull bytes into memory, on many more threads than there are
        // cores, because these threads are waiting rather than working.
        let (loaded_tx, loaded_rx) = mpsc::channel::<(usize, Vec<u8>)>();
        let readers: Vec<_> = (0..READERS.min(to_index.len().max(1)))
            .map(|_| {
                let loaded_tx = loaded_tx.clone();
                let failures = send.clone();
                scope.spawn(|| {
                    let loaded_tx = loaded_tx;
                    let failures = failures;
                    loop {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let at = next.fetch_add(1, Ordering::Relaxed);
                        let Some(candidate) = to_index.get(at) else {
                            return;
                        };
                        #[cfg(feature = "logging")]
                        let started = Instant::now();
                        match dirlist::read_whole(&candidate.abs_path, candidate.size_bytes) {
                            Ok(bytes) => {
                                #[cfg(feature = "logging")]
                                Spent::add(&spent.reading, started);
                                read_ahead.claim(bytes.len() as u64, cancel);
                                if loaded_tx.send((at, bytes)).is_err() {
                                    return;
                                }
                            }
                            Err(err) => {
                                let _ = failures.send(Outcome::Failed {
                                    looked: looked_at(candidate),
                                    message: err.to_string(),
                                });
                            }
                        }
                    }
                })
            })
            .collect();
        drop(loaded_tx);

        // Stage two: decode and fingerprint what is already in memory, across
        // every core, never waiting on the network.
        loaded_rx
            .into_iter()
            .par_bridge()
            .for_each_with(send.clone(), |send, (at, bytes)| {
                let candidate = &to_index[at];
                if cancel.load(Ordering::Relaxed) {
                    read_ahead.release(bytes.len() as u64);
                    return;
                }
                let outcome = index_one(candidate, &bytes, &spent);
                read_ahead.release(bytes.len() as u64);
                if matches!(outcome, Outcome::NotAnImage(_)) {
                    ignored.fetch_add(1, Ordering::Relaxed);
                }
                let _ = send.send(outcome);

                let count = done.fetch_add(1, Ordering::Relaxed) + 1;
                // On a timer, not on a count of files. Whichever thread crosses the
                // interval first takes the report and the others carry on reading.
                // The first file is announced whenever it lands, so a pass says
                // something about what it is reading before any interval has passed,
                // however few files there are or however fast they come.
                let now = started.elapsed().as_millis() as u64;
                let last = reported.load(Ordering::Relaxed);
                let due = count == 1 || now.saturating_sub(last) >= REPORT_AFTER.as_millis() as u64;
                if due
                    && reported
                        .compare_exchange(last, now, Ordering::Relaxed, Ordering::Relaxed)
                        .is_ok()
                {
                    announce(count);
                }
            });
        for reader in readers {
            let _ = reader.join();
        }
        drop(send);
        if !to_index.is_empty() {
            report(Event::Reached(Step::FinishedReadingNewFiles));
        }
        // The last partial group of files would otherwise never be announced and
        // the bar would stop short of the end.
        announce(done.load(Ordering::Relaxed));
        runlog::log_line!(
            "read and fingerprint: {:.2}s of wall clock; over every thread, \
             {:.1}s reading, {:.1}s decoding, {:.1}s fingerprinting",
            started.elapsed().as_secs_f64(),
            Spent::seconds(&spent.reading),
            Spent::seconds(&spent.decoding),
            Spent::seconds(&spent.fingerprinting)
        );

        writer
            .join()
            .map_err(|_| anyhow::anyhow!("the index writer thread panicked"))?
    })?;

    summary.indexed = write_result.0;
    summary.failed = write_result.1;
    summary.cancelled = cancel.load(Ordering::Relaxed);

    index.set_meta("last_scan", &now_seconds().to_string())?;
    // What the index covers, not a preference: a pass that does not descend
    // where the last one did would drop every subfolder row as vanished.
    index.set_meta("recurse", if options.recurse { "1" } else { "0" })?;

    // The pass changed the index, so the copy taken at the load step describes a
    // folder that no longer matches it. Convert again, from what was just
    // written, so whatever runs next still starts from memory.
    // Not for a folder with nothing in it. Skipping the conversion above only to
    // do it here is the same claim made a moment later.
    let empty = unchanged == 0 && summary.indexed == 0;
    if images.is_none() && !summary.cancelled && !empty {
        report(Event::Reached(Step::StartedConvertingTheIndex));
        images = index
            .images(
                std::sync::Arc::new(AtomicBool::new(false)),
                std::sync::Arc::new(|_| {}),
            )
            .context("converting the index")?;
        report(Event::Reached(Step::FinishedConvertingTheIndex));
    }

    report(Event::Done {
        indexed: summary.indexed,
        removed: summary.removed,
        failed: summary.failed,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });

    Ok((summary, images))
}

#[cfg(test)]
#[path = "../tests/scan.rs"]
mod tests;
