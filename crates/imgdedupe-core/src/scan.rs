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
use crate::fingerprint::{fingerprint, FINGERPRINT_VERSION};
use crate::format::{self, SNIFF_LEN};
use crate::frames;

/// Records written per transaction. A killed run loses at most this many.
const BATCH: usize = 5000;
/// Files between progress reports.
const REPORT_EVERY: u64 = 200;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "event", rename_all = "lowercase")]
pub enum Event {
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
    /// Rows committed to the index. The writer runs behind the decoders, so this
    /// is still moving after the last file has been read.
    Writing {
        done: u64,
        total: u64,
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
fn walk(options: &Options) -> Result<Vec<Candidate>> {
    let mut walker = walkdir::WalkDir::new(&options.root).follow_links(false);
    if !options.recurse {
        walker = walker.max_depth(1);
    }

    let sidecars = index_sidecars(&options.db_path);
    let mut out = Vec::new();
    for entry in walker.into_iter().filter_map(|entry| entry.ok()) {
        if !entry.file_type().is_file() {
            continue;
        }
        let path = entry.path();
        if sidecars.iter().any(|sidecar| path == sidecar) {
            continue;
        }
        let Ok(relative) = path.strip_prefix(&options.root) else {
            continue;
        };
        let Some(rel_path) = to_portable_path(relative) else {
            continue;
        };
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        out.push(Candidate {
            rel_path,
            abs_path: path.to_path_buf(),
            size_bytes: metadata.len() as i64,
            mtime_ns: mtime_nanos(&metadata),
        });
    }
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

fn mtime_nanos(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .map(|delta| delta.as_nanos().min(i64::MAX as u128) as i64)
        .unwrap_or(0)
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
fn index_one(candidate: &Candidate) -> Outcome {
    // Logged before the read, so a run that dies without unwinding still names the
    // file it was on. An allocation failure aborts the process and no panic hook
    // runs, which is what a large image on many threads at once can do.
    let bytes = match std::fs::read(&candidate.abs_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            return Outcome::Failed {
                path: candidate.rel_path.clone(),
                message: err.to_string(),
            }
        }
    };

    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    let Some(format) = format::detect(head) else {
        return Outcome::NotAnImage;
    };
    if frames::is_animated(format, &bytes) {
        return Outcome::NotAnImage;
    }

    let decoded = match decode_at_most(format, &bytes, SMALL_EDGE) {
        Ok(decoded) => decoded,
        Err(err) => {
            return Outcome::Failed {
                path: candidate.rel_path.clone(),
                message: format!("{err:#}"),
            }
        }
    };
    crate::runlog::line(&format!(
        "decoded {} as {format}, {}x{}, {} bytes on disk",
        candidate.rel_path,
        decoded.width,
        decoded.height,
        bytes.len()
    ));

    let bytes_hash = xxhash_rust::xxh3::xxh3_64(&bytes) as u32;
    let print = fingerprint(&decoded);

    Outcome::Indexed(Box::new(Record {
        rel_path: candidate.rel_path.clone(),
        size_bytes: candidate.size_bytes,
        mtime_ns: candidate.mtime_ns,
        bytes_hash,
        width: decoded.width,
        height: decoded.height,
        format,
        channels: decoded.channels,
        fingerprint: print,
    }))
}

/// Run one indexing pass. Decoding runs on every core; writing runs on one
/// thread in batched transactions, so the index is consistent at every commit.
pub fn run(
    conn: &mut Connection,
    options: &Options,
    cancel: &AtomicBool,
    report: &(dyn Fn(Event) + Sync),
) -> Result<Summary> {
    let started = Instant::now();
    let candidates = walk(options).context("walking the folder")?;
    let known = db::load_known(conn).context("reading the existing index")?;
    let Diff { to_index, removed, unchanged } = diff(candidates, &known);

    let mut summary = Summary { unchanged, ..Summary::default() };

    if !removed.is_empty() {
        let tx = conn.transaction()?;
        summary.removed = db::delete_paths(&tx, &removed)? as u64;
        tx.commit()?;
    }

    let total = to_index.len() as u64;
    report(Event::Start { total });

    let done = AtomicU64::new(0);
    let ignored = AtomicU64::new(0);
    let (send, recv) = mpsc::channel::<Outcome>();

    let write_result = std::thread::scope(|scope| -> Result<(u64, u64)> {
        let writer = scope.spawn(|| -> Result<(u64, u64)> {
            let mut indexed = 0u64;
            let mut failed = 0u64;
            let mut pending: Vec<Box<Record>> = Vec::with_capacity(BATCH);
            let scanned_at = now_seconds();

            let flush = |conn: &mut Connection, pending: &mut Vec<Box<Record>>| -> Result<()> {
                if pending.is_empty() {
                    return Ok(());
                }
                let tx = conn.transaction()?;
                for record in pending.iter() {
                    db::upsert(&tx, record, scanned_at)?;
                }
                tx.commit()?;
                pending.clear();
                Ok(())
            };

            // Against what has been handed to the writer, not against the number
            // of files read: most of a pass can be files that turn out to need no
            // row at all, and none of those are work left to do.
            let mut committed = 0u64;

            for outcome in recv {
                match outcome {
                    Outcome::Indexed(record) => {
                        indexed += 1;
                        pending.push(record);
                        if pending.len() >= BATCH {
                            flush(conn, &mut pending)?;
                            committed = indexed;
                        }
                        if indexed % REPORT_EVERY == 0 {
                            report(Event::Writing { done: committed, total: indexed });
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
            report(Event::Writing { done: indexed, total: indexed });
            Ok((indexed, failed))
        });

        let announce = |count: u64| {
            let elapsed = started.elapsed().as_secs_f64().max(0.001);
            report(Event::Progress {
                done: count,
                new: count,
                changed: 0,
                unchanged,
                removed: summary.removed,
                ignored: ignored.load(Ordering::Relaxed),
                per_sec: (count as f64 / elapsed) as u64,
            });
        };

        to_index.par_iter().for_each_with(send.clone(), |send, candidate| {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            let outcome = index_one(candidate);
            if matches!(outcome, Outcome::NotAnImage) {
                ignored.fetch_add(1, Ordering::Relaxed);
            }
            let _ = send.send(outcome);

            let count = done.fetch_add(1, Ordering::Relaxed) + 1;
            if count % REPORT_EVERY == 0 {
                announce(count);
            }
        });
        drop(send);
        // The last partial group of files would otherwise never be announced and
        // the bar would stop short of the end.
        announce(done.load(Ordering::Relaxed));

        writer.join().map_err(|_| anyhow::anyhow!("the index writer thread panicked"))?
    })?;

    summary.indexed = write_result.0;
    summary.failed = write_result.1;
    summary.cancelled = cancel.load(Ordering::Relaxed);

    db::set_meta(conn, "last_scan", &now_seconds().to_string())?;

    report(Event::Done {
        indexed: summary.indexed,
        removed: summary.removed,
        failed: summary.failed,
        elapsed_ms: started.elapsed().as_millis() as u64,
    });

    Ok(summary)
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
        let summary = run(&mut conn, &fixture.options, &cancel, &|event| {
            events.lock().unwrap().push(event);
        })
        .expect("scan");
        (summary, events.into_inner().unwrap())
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
        assert_eq!(summary.indexed, 0);
        assert_eq!(summary.unchanged, 1);
        let start = events.iter().find_map(|e| match e {
            Event::Start { total } => Some(*total),
            _ => None,
        });
        assert_eq!(start, Some(0), "an unchanged folder still queued work");
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
        let summary = run(&mut conn, &shallow, &cancel, &|_| {}).expect("scan");
        assert_eq!(summary.indexed, 1);
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
        let summary = run(&mut conn, &fx.options, &cancel, &|_| {}).expect("scan");
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
        assert!(matches!(events.first(), Some(Event::Start { .. })));
        assert!(matches!(events.last(), Some(Event::Done { .. })));
    }
}
