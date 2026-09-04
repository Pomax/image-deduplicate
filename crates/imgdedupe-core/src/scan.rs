use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc;
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::Connection;
use serde::Serialize;

use crate::db::{self, Record};
use crate::decode::{decode_at_most, SMALL_EDGE};
use crate::dirlist;
use crate::fingerprint::{fingerprint, FINGERPRINT_VERSION};
use crate::format::{self, SNIFF_LEN};
use crate::runlog;
use crate::frames;

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
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Summary {
    pub indexed: u64,
    pub removed: u64,
    pub failed: u64,
    pub unchanged: u64,
    pub cancelled: bool,
}

/// One file the walk found, before anything has been read from it.
#[derive(Debug, Clone)]
struct Candidate {
    rel_path: String,
    abs_path: PathBuf,
    size_bytes: i64,
    mtime_ns: i64,
}

/// Walk the tree and list every file, ignoring the index and its sidecars.
///
/// Reports as it goes. On a folder the machine has to ask another machine about,
/// listing it is one call per file and most of the pass, and it used to say
/// nothing from the first file to the last.
fn walk(
    options: &Options,
    cancel: &AtomicBool,
    report: &(dyn Fn(Event) + Sync),
) -> Result<Vec<Candidate>> {
    report(Event::Reached(Step::StartedLookingForTheTotal));
    // Asked of the folder itself, in one call, so the bar has something to
    // measure against before a single entry has been listed. Only for one folder:
    // the size of a tree is as many answers as it has directories, and a total
    // that grows as they are found is a bar that goes backwards.
    let of = if options.recurse { None } else { dirlist::entry_count(&options.root) };
    let sidecars = index_sidecars(&options.db_path);
    let mut out = Vec::new();
    let mut queue = vec![options.root.clone()];
    let mut first = true;

    while let Some(dir) = queue.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(out);
        }
        let so_far = out.len() as u64;
        let listed = match dirlist::list(&dir, &|| cancel.load(Ordering::Relaxed), &|found| {
            // As the listing arrives, not when it is finished. This is the read
            // bar's first job: every one of these is a file that has been looked
            // at, and on a folder that answers slowly it is most of the wait.
            report(Event::Walking { found: so_far + found, of });
        }) {
            Ok(listed) => listed,
            // The folder that was asked for has to be readable, or the pass would
            // see an empty folder and delete every row in the index. One
            // unreadable subfolder is skipped instead.
            Err(err) if first => {
                return Err(err).with_context(|| format!("listing {}", dir.display()))
            }
            Err(_) => continue,
        };
        first = false;

        for entry in listed {
            if cancel.load(Ordering::Relaxed) {
                return Ok(out);
            }
            if entry.is_dir {
                if options.recurse {
                    queue.push(dir.join(&entry.name));
                }
                continue;
            }
            if !entry.is_file {
                continue;
            }
            let path = dir.join(&entry.name);
            if sidecars.iter().any(|sidecar| &path == sidecar) {
                continue;
            }
            let Ok(relative) = path.strip_prefix(&options.root) else {
                continue;
            };
            let Some(rel_path) = to_portable_path(relative) else {
                continue;
            };
            out.push(Candidate {
                rel_path,
                abs_path: path,
                size_bytes: entry.size_bytes,
                mtime_ns: entry.mtime_ns,
            });
        }
        report(Event::Walking { found: out.len() as u64, of });
    }
    // The listing is over, so the count it reached is the exact total, whatever
    // the folder said before it started.
    report(Event::Walking { found: out.len() as u64, of: Some(out.len() as u64) });
    Ok(out)
}

/// The index plus the files SQLite keeps beside it in WAL mode.
fn index_sidecars(db_path: &Path) -> Vec<PathBuf> {
    let mut out = vec![db_path.to_path_buf()];
    if let Some(name) = db_path.file_name().and_then(|n| n.to_str()) {
        for suffix in ["-wal", "-shm", "-journal"] {
            out.push(db_path.with_file_name(format!("{name}{suffix}")));
        }
    }
    out
}

/// Relative paths are stored with forward slashes so an index built on one
/// platform still matches the same tree on another.
fn to_portable_path(relative: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

/// Which paths need reading and which can be skipped without touching the disk.
struct Diff {
    to_index: Vec<Candidate>,
    removed: Vec<String>,
    unchanged: u64,
}

fn diff(candidates: Vec<Candidate>, known: &std::collections::HashMap<String, db::Known>) -> Diff {
    let mut to_index = Vec::new();
    let mut unchanged = 0u64;
    let mut seen = std::collections::HashSet::with_capacity(candidates.len());

    for candidate in candidates {
        seen.insert(candidate.rel_path.clone());
        let fresh = known.get(&candidate.rel_path).is_some_and(|entry| {
            entry.size_bytes == candidate.size_bytes
                && entry.mtime_ns == candidate.mtime_ns
                && entry.fingerprint_version == FINGERPRINT_VERSION
        });
        if fresh {
            unchanged += 1;
        } else {
            to_index.push(candidate);
        }
    }

    let removed = known
        .keys()
        .filter(|path| !seen.contains(*path))
        .cloned()
        .collect();

    Diff { to_index, removed, unchanged }
}

/// What came back from reading one file.
enum Outcome {
    Indexed(Box<Record>),
    /// Not one of the supported formats, or animated. Not an error, and not indexed.
    NotAnImage,
    Failed { path: String, message: String },
}

/// Read, sniff, decode and fingerprint one file. Never panics on bad input: a
/// malformed file comes back as `Failed` and the pass continues.
/// How much of the folder is in the index, as the pass sees it.
///
/// The files that were left alone are already in it, and the ones that turned
/// out not to be pictures are not part of the folder as far as this is
/// concerned, so they leave the total rather than sitting in it unindexed.
fn indexed_so_far(
    indexed: u64,
    unchanged: u64,
    to_index: u64,
    read: &AtomicU64,
    ignored: &AtomicU64,
) -> Event {
    let ignored = ignored.load(Ordering::Relaxed);
    Event::Writing {
        done: unchanged + indexed,
        total: unchanged + to_index.saturating_sub(ignored),
        read: unchanged + read.load(Ordering::Relaxed),
        unchanged,
        ignored,
    }
}

/// Where a pass spends its time inside the files, added up over every thread.
/// The totals are larger than the wall clock, by roughly the number of threads.
#[derive(Default)]
struct Spent {
    reading: AtomicU64,
    decoding: AtomicU64,
    fingerprinting: AtomicU64,
}

impl Spent {
    fn add(counter: &AtomicU64, at: Instant) {
        counter.fetch_add(at.elapsed().as_nanos() as u64, Ordering::Relaxed);
    }

    fn seconds(counter: &AtomicU64) -> f64 {
        counter.load(Ordering::Relaxed) as f64 / 1e9
    }
}

fn index_one(candidate: &Candidate, bytes: &[u8], spent: &Spent) -> Outcome {
    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    let Some(format) = format::detect(head) else {
        return Outcome::NotAnImage;
    };
    if frames::is_animated(format, &bytes) {
        return Outcome::NotAnImage;
    }

    let at = Instant::now();
    let decoded = match decode_at_most(format, &bytes, SMALL_EDGE) {
        Ok(decoded) => decoded,
        Err(err) => {
            return Outcome::Failed {
                path: candidate.rel_path.clone(),
                message: format!("{err:#}"),
            }
        }
    };
    Spent::add(&spent.decoding, at);
    crate::runlog::line(&format!(
        "decoded {} as {format}, {}x{}, {} bytes on disk",
        candidate.rel_path,
        decoded.width,
        decoded.height,
        bytes.len()
    ));

    let at = Instant::now();
    let print = fingerprint(&decoded);
    Spent::add(&spent.fingerprinting, at);

    Outcome::Indexed(Box::new(Record {
        rel_path: candidate.rel_path.clone(),
        size_bytes: candidate.size_bytes,
        mtime_ns: candidate.mtime_ns,
        width: decoded.width,
        height: decoded.height,
        format,
        channels: decoded.channels,
        fingerprint: print,
    }))
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

/// How many bytes of already-read files may be waiting to be decoded.
///
/// The readers run ahead of the decoders and stop when this much is in hand, so a
/// folder of large pictures does not pull itself into memory all at once. Small
/// enough to be nothing on a machine with gigabytes, large enough to keep every
/// core fed through a slow patch of the network.
const READ_AHEAD_BYTES: u64 = 1 << 30;

/// Bytes of read-but-not-yet-decoded files, and the wait for room.
struct ReadAhead {
    held: std::sync::Mutex<u64>,
    room: std::sync::Condvar,
}

impl ReadAhead {
    fn new() -> Self {
        ReadAhead { held: std::sync::Mutex::new(0), room: std::sync::Condvar::new() }
    }

    /// Wait until this many bytes fit, then claim them. A single file larger than
    /// the whole budget is let through on its own rather than waiting for room
    /// that will never exist.
    fn claim(&self, bytes: u64, cancel: &AtomicBool) {
        let mut held = self.held.lock().expect("the read-ahead budget");
        while *held > 0 && *held + bytes > READ_AHEAD_BYTES && !cancel.load(Ordering::Relaxed) {
            held = self.room.wait(held).expect("the read-ahead budget");
        }
        *held += bytes;
    }

    fn release(&self, bytes: u64) {
        let mut held = self.held.lock().expect("the read-ahead budget");
        *held = held.saturating_sub(bytes);
        self.room.notify_all();
    }
}

/// Run one indexing pass. Files are read into memory by a wide pool and decoded
/// across every core; writing runs on one thread in batched transactions, so the
/// index is consistent at every commit.
pub fn run(
    conn: &mut Connection,
    options: &Options,
    cancel: &AtomicBool,
    report: &(dyn Fn(Event) + Sync),
) -> Result<(Summary, Option<Vec<crate::matching::Image>>)> {
    let started = Instant::now();
    let at = Instant::now();
    let candidates = walk(options, cancel, report).context("walking the folder")?;
    let found = candidates.len();
    report(Event::Reached(Step::FoundTheTotal));
    report(Event::Reached(Step::ListedTheFolder));
    runlog::line(&format!("walk: {:.2}s, {found} files", at.elapsed().as_secs_f64()));

    let at = Instant::now();
    // Read the file whole rather than through it. The log is folded in first, so
    // the file is everything that has been written. See `db::open_snapshot`.
    if cancel.load(Ordering::Relaxed) {
        return Ok((Summary { cancelled: true, ..Summary::default() }, None));
    }
    // Straight off the connection: the index is in memory from the moment it was
    // opened, so this reads nothing from anywhere.
    let step = Instant::now();
    let known = db::load_known(conn).context("reading the existing index")?;
    runlog::line(&format!("  known paths: {:.2}s", step.elapsed().as_secs_f64()));
    report(Event::Reached(Step::LoadedIndexIntoMemory));
    runlog::line(&format!(
        "load index: {:.2}s, {} rows",
        at.elapsed().as_secs_f64(),
        known.len()
    ));

    let Diff { to_index, removed, unchanged } = diff(candidates, &known);
    report(Event::Reached(Step::CrossReferencedWithTheIndex));
    report(Event::Reached(Step::CountedWhatChanged));
    runlog::line(&format!(
        "diff: {unchanged} unchanged, {} to index, {} gone",
        to_index.len(),
        removed.len()
    ));

    // The index is loaded and nothing in the folder has moved, so it is already
    // the answer. Convert it here, off the copy that is still in memory, rather
    // than leaving it for whatever runs next to read the file all over again.
    if cancel.load(Ordering::Relaxed) {
        return Ok((Summary { cancelled: true, unchanged, ..Summary::default() }, None));
    }

    let mut images = None;
    if unchanged == 0 && to_index.is_empty() {
        // An empty folder. There is nothing to convert and nothing to search, and
        // reporting either is reporting work that was never done.
    } else if to_index.is_empty() && removed.is_empty() {
        report(Event::Reached(Step::StartedConvertingTheIndex));
        let step = Instant::now();
        images =
            crate::matching::load_images(conn, cancel, &|_| {}).context("converting the index")?;
        runlog::line(&format!("  convert to memory: {:.2}s", step.elapsed().as_secs_f64()));
        report(Event::Reached(Step::FinishedConvertingTheIndex));
    }

    let mut summary = Summary { unchanged, ..Summary::default() };

    if !removed.is_empty() {
        let at = Instant::now();
        let tx = conn.transaction()?;
        summary.removed = db::delete_paths(&tx, &removed)? as u64;
        tx.commit()?;
        runlog::line(&format!(
            "drop gone: {:.2}s, {} rows",
            at.elapsed().as_secs_f64(),
            summary.removed
        ));
    }

    let total = to_index.len() as u64;
    // The bar is against the folder, not against the work: a pass over a folder
    // it has seen before still looks at every file to find what is new and what
    // has gone.
    report(Event::Start { total: unchanged + total });

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

            let flush = |conn: &mut Connection, pending: &mut Vec<Box<Record>>| -> Result<()> {
                if pending.is_empty() {
                    return Ok(());
                }
                let rows = pending.len();
                let at = Instant::now();
                let tx = conn.transaction()?;
                for record in pending.iter() {
                    db::upsert(&tx, record, scanned_at)?;
                }
                let inserted = at.elapsed().as_secs_f64();
                let at = Instant::now();
                tx.commit()?;
                runlog::line(&format!(
                    "commit: {rows} rows, {inserted:.2}s inserting and {:.2}s committing",
                    at.elapsed().as_secs_f64()
                ));
                pending.clear();
                Ok(())
            };

            for outcome in recv {
                match outcome {
                    Outcome::Indexed(record) => {
                        indexed += 1;
                        pending.push(record);
                        if pending.len() >= BATCH {
                            flush(conn, &mut pending)?;
                        }
                        if indexed % REPORT_EVERY == 0 || told.elapsed() >= REPORT_AFTER {
                            report(indexed_so_far(indexed, unchanged, total, &done, &ignored));
                            told = Instant::now();
                        }
                    }
                    Outcome::NotAnImage => {}
                    Outcome::Failed { path, message } => {
                        failed += 1;
                        report(Event::Error { path, message });
                    }
                }
            }

            flush(conn, &mut pending)?;
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
            runlog::line(&format!(
                "rate: {count} files in {elapsed:.1}s, {} in the last second; over every \
                 thread {:.1}s reading, {:.1}s decoding, {:.1}s fingerprinting; {} bytes \
                 read ahead",
                rate_now.load(Ordering::Relaxed),
                Spent::seconds(&spent.reading),
                Spent::seconds(&spent.decoding),
                Spent::seconds(&spent.fingerprinting),
                read_ahead.held.lock().map(|held| *held).unwrap_or(0),
            ));
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
                        let started = Instant::now();
                        match dirlist::read_whole(&candidate.abs_path, candidate.size_bytes) {
                            Ok(bytes) => {
                                Spent::add(&spent.reading, started);
                                read_ahead.claim(bytes.len() as u64, cancel);
                                if loaded_tx.send((at, bytes)).is_err() {
                                    return;
                                }
                            }
                            Err(err) => {
                                let _ = failures.send(Outcome::Failed {
                                    path: candidate.rel_path.clone(),
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
        loaded_rx.into_iter().par_bridge().for_each_with(send.clone(), |send, (at, bytes)| {
            let candidate = &to_index[at];
            if cancel.load(Ordering::Relaxed) {
                read_ahead.release(bytes.len() as u64);
                return;
            }
            let outcome = index_one(candidate, &bytes, &spent);
            read_ahead.release(bytes.len() as u64);
            if matches!(outcome, Outcome::NotAnImage) {
                ignored.fetch_add(1, Ordering::Relaxed);
            }
            let _ = send.send(outcome);

            let count = done.fetch_add(1, Ordering::Relaxed) + 1;
            // On a timer, not on a count of files. Whichever thread crosses the
            // interval first takes the report and the others carry on reading.
            let now = started.elapsed().as_millis() as u64;
            let last = reported.load(Ordering::Relaxed);
            let due = now.saturating_sub(last) >= REPORT_AFTER.as_millis() as u64;
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
        runlog::line(&format!(
            "read and fingerprint: {:.2}s of wall clock; over every thread, \
             {:.1}s reading, {:.1}s decoding, {:.1}s fingerprinting",
            started.elapsed().as_secs_f64(),
            Spent::seconds(&spent.reading),
            Spent::seconds(&spent.decoding),
            Spent::seconds(&spent.fingerprinting)
        ));

        writer.join().map_err(|_| anyhow::anyhow!("the index writer thread panicked"))?
    })?;

    summary.indexed = write_result.0;
    summary.failed = write_result.1;
    summary.cancelled = cancel.load(Ordering::Relaxed);

    db::set_meta(conn, "last_scan", &now_seconds().to_string())?;
    // What the index covers, not a preference: a pass that does not descend
    // where the last one did would drop every subfolder row as vanished.
    db::set_meta(conn, "recurse", if options.recurse { "1" } else { "0" })?;

    // The pass changed the index, so the copy taken at the load step describes a
    // folder that no longer matches it. Convert again, from what was just
    // written, so whatever runs next still starts from memory.
    // Not for a folder with nothing in it. Skipping the conversion above only to
    // do it here is the same claim made a moment later.
    let empty = unchanged == 0 && summary.indexed == 0;
    if images.is_none() && !summary.cancelled && !empty {
        report(Event::Reached(Step::StartedConvertingTheIndex));
        images =
            crate::matching::load_images(conn, cancel, &|_| {}).context("converting the index")?;
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

fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
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

    struct Fixture {
        dir: tempfile::TempDir,
        options: Options,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().expect("tempdir");
        let root = dir.path().to_path_buf();
        let db_path = root.join(db::INDEX_FILENAME);
        let options = Options { root, db_path, recurse: true };
        Fixture { dir, options }
    }

    fn scan(fixture: &Fixture) -> (Summary, Vec<Event>) {
        let mut conn = db::open(&fixture.options.db_path).expect("open");
        let events = std::sync::Mutex::new(Vec::new());
        let cancel = AtomicBool::new(false);
        let (summary, _images) = run(&mut conn, &fixture.options, &cancel, &|event| {
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
        assert!(reported.len() >= 2, "indexing was only reported once, at the end");
        let (done, total) = reported[0];
        assert_eq!(done, REPORT_EVERY, "the first report said nothing had been indexed");
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
            write_image(&fx.dir.path().join(format!("old{index}.png")), 32, 24, index);
        }
        let gone = fx.dir.path().join("old0.png");
        scan(&fx);

        // Enough new pictures for the pass to report while it is still reading.
        let fresh = REPORT_EVERY as u32 + 50;
        for index in 0..fresh {
            write_image(&fx.dir.path().join(format!("new{index}.png")), 32, 24, 1000 + index);
        }
        std::fs::remove_file(&gone).expect("take one away");

        let (summary, events) = scan(&fx);
        let left_alone = old as u64 - 1;
        assert_eq!(summary.unchanged, left_alone);
        assert_eq!(summary.removed, 1);

        let progress: Vec<(u64, u64, u64)> = events
            .iter()
            .filter_map(|event| match event {
                Event::Progress { done, unchanged, removed, .. } => {
                    Some((*done, *unchanged, *removed))
                }
                _ => None,
            })
            .collect();
        assert!(progress.len() >= 2, "the pass only reported once, at the end");

        for (index, (done, unchanged, removed)) in progress.iter().enumerate() {
            assert_eq!(
                *unchanged, left_alone,
                "report {index} said {unchanged} were left alone, not {left_alone}"
            );
            assert_eq!(*removed, 1, "report {index} had not counted the file that went");
            assert!(
                *done >= left_alone,
                "report {index} counted {done} files, fewer than the {left_alone} it skipped"
            );
        }
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
        assert_eq!(last, Some(1), "the pass did not end on every file in the folder");
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

        let conn = db::open(&fx.options.db_path).expect("open");
        let count: i64 = conn.query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap();
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

        let conn = db::open(&fx.options.db_path).expect("open");
        let width: i64 = conn
            .query_row("SELECT width FROM images", [], |r| r.get(0))
            .unwrap();
        assert_eq!(width, 100);
    }

    /// A file that is not a picture gets no row, so every later pass reads it
    /// again. The pass says how many of those it read, so they are not mistaken
    /// for work that produced something.
    #[test]
    fn files_that_are_not_images_are_neither_indexed_nor_failures() {
        let fx = fixture();
        write_image(&fx.dir.path().join("a.png"), 64, 48, 0);
        std::fs::write(fx.dir.path().join("notes.txt"), b"just some text").expect("write");
        std::fs::write(fx.dir.path().join("archive.zip"), b"PK\x03\x04rest").expect("write");
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
        assert_eq!(counted, Some(2), "the two files that are not pictures were not reported");
    }

    #[test]
    fn a_malformed_image_is_reported_and_does_not_stop_the_pass() {
        let fx = fixture();
        write_image(&fx.dir.path().join("good.png"), 64, 48, 0);
        std::fs::write(fx.dir.path().join("bad.png"), b"\x89PNG\r\n\x1a\ntruncated").expect("write");
        let (summary, events) = scan(&fx);
        assert_eq!(summary.indexed, 1);
        assert_eq!(summary.failed, 1);
        let reported = events.iter().any(|e| matches!(e, Event::Error { path, .. } if path == "bad.png"));
        assert!(reported, "the failure was not reported");
    }

    #[test]
    fn without_recurse_subfolders_are_not_walked() {
        let fx = fixture();
        write_image(&fx.dir.path().join("top.png"), 32, 32, 0);
        std::fs::create_dir(fx.dir.path().join("sub")).expect("mkdir");
        write_image(&fx.dir.path().join("sub").join("deep.png"), 32, 32, 5);

        let mut shallow = fx.options.clone();
        shallow.recurse = false;
        let mut conn = db::open(&shallow.db_path).expect("open");
        let cancel = AtomicBool::new(false);
        let (summary, _) = run(&mut conn, &shallow, &cancel, &|_| {}).expect("scan");
        assert_eq!(summary.indexed, 1);

        // How far the pass reached is written down, because a later pass that
        // does not reach as far drops everything it cannot see.
        assert_eq!(db::get_meta(&conn, "recurse").expect("meta").as_deref(), Some("0"));
        let (summary, _) = run(&mut conn, &fx.options, &cancel, &|_| {}).expect("scan");
        assert_eq!(summary.indexed, 1, "the subfolder was not picked up");
        assert_eq!(db::get_meta(&conn, "recurse").expect("meta").as_deref(), Some("1"));
    }

    #[test]
    fn the_index_file_does_not_index_itself() {
        let fx = fixture();
        write_image(&fx.dir.path().join("a.png"), 32, 32, 0);
        scan(&fx);
        let (summary, _) = scan(&fx);
        assert_eq!(summary.removed, 0, "a sidecar was treated as a vanished image");

        let conn = db::open(&fx.options.db_path).expect("open");
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
        write_image(&fx.dir.path().join("one").join("two").join("deep.png"), 32, 32, 0);
        scan(&fx);

        let conn = db::open(&fx.options.db_path).expect("open");
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
        let mut conn = db::open(&fx.options.db_path).expect("open");
        let cancel = AtomicBool::new(true);
        let (summary, _) = run(&mut conn, &fx.options, &cancel, &|_| {}).expect("scan");
        assert!(summary.cancelled);

        let count: i64 = conn.query_row("SELECT count(*) FROM files", [], |r| r.get(0)).unwrap();
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
        assert_eq!(summary.failed, 0, "an animation was treated as a broken image");
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
            matches!(events.first(), Some(Event::Reached(_) | Event::Walking { .. })),
            "the pass said nothing until it had a total: {:?}",
            events.first()
        );
        assert!(events.iter().any(|event| matches!(event, Event::Walking { .. })));
        assert!(events.iter().any(|event| matches!(event, Event::Start { .. })));
        assert!(matches!(events.last(), Some(Event::Done { .. })));
    }
}
