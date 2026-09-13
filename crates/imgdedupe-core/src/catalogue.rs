//! The one owner of a folder's catalogue.
//!
//! Nothing else opens `imgdedupe.sqlite`, holds a connection to it, or names its
//! path. Everything that wants something from the catalogue sends this a message
//! and waits for the answer.
//!
//! What it holds in memory is the data. The file is a copy of that, kept in step
//! on a thread of its own, so a caller is answered as soon as the manager has the
//! change rather than when the disk does. Nothing reads the file after it has
//! been opened.
//!
//! One manager holds one folder at a time. `hold` lets go of whatever it has and
//! takes up another folder's catalogue: it opens the file, brings it to the
//! current shape on disk, and reads it in. A catalogue it cannot read, cannot
//! write to, or cannot bring to the current shape is a broken catalogue: `hold`
//! says so, the manager holds nothing, and no file is touched.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::db::{self, Connection, Known, Record};
use crate::matching::{self, DuplicateSet, Image, Progress, Thresholds};

mod serve;
mod writer;

use self::serve::*;
use self::writer::*;

/// What a caller asks of the manager. Every one carries the way back.
enum Job {
    Open(PathBuf, Sender<Result<()>>),
    Close(Sender<Result<()>>),
    OpenCataloguePath(Sender<Option<PathBuf>>),
    Images(
        Arc<AtomicBool>,
        Reporter,
        Sender<Result<Option<Vec<Image>>>>,
    ),
    FindSets(
        Thresholds,
        Arc<AtomicBool>,
        Reporter,
        Sender<Result<Option<Vec<DuplicateSet>>>>,
    ),
    Known(Sender<Result<std::collections::HashMap<String, Known>>>),
    Ignored(Sender<Result<Vec<(i64, i64)>>>),
    Meta(String, Sender<Result<Option<String>>>),
    SetMeta(String, String, Sender<Result<()>>),
    ForgetMeta(String, Sender<Result<()>>),
    Ignore(Vec<(i64, i64)>, Sender<Result<()>>),
    Unignore(Vec<(i64, i64)>, Sender<Result<()>>),
    BeginReview(Sender<Result<()>>),
    Kept(Sender<Result<Vec<i64>>>),
    KeepThese(Vec<i64>, Sender<Result<()>>),
    UnkeepThese(Vec<i64>, Sender<Result<()>>),
    ClearKeep(Sender<Result<()>>),
    StoredSets(Sender<Result<Vec<(i64, Vec<i64>)>>>),
    StoreSets(Vec<(i64, Vec<i64>)>, Sender<Result<()>>),
    ClearSets(Sender<Result<()>>),
    Upsert(Vec<Record>, i64, Sender<Result<()>>),
    NotPictures(Vec<db::Looked>, i64, Sender<Result<()>>),
    DeletePaths(Vec<String>, Sender<Result<usize>>),
    Compact(Sender<Result<()>>),
    Delete(Sender<Result<usize>>),
    Synced(Sender<Result<()>>),
}

/// Where a long read says how far it has got. Sent with the job, because the
/// reading happens on the manager's thread.
type Reporter = Arc<dyn Fn(Progress) + Send + Sync>;

/// A way to ask the manager for something. Cloning gives another way to ask the
/// same manager; the thread ends when the last one goes.
#[derive(Clone)]
pub struct Catalogue {
    to: Sender<Job>,
}

impl std::fmt::Debug for Catalogue {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.open_catalogue_path() {
            Some(path) => write!(out, "index open on {}", path.display()),
            None => write!(out, "no index open"),
        }
    }
}

/// The catalogue the manager has open.
struct OpenCatalogue {
    path: PathBuf,
    conn: Connection,
}

impl Catalogue {
    /// Start the manager. It holds nothing until it is told to hold a folder,
    /// and touches no file until then.
    pub fn start() -> Catalogue {
        let (to, jobs) = channel::<Job>();
        std::thread::Builder::new()
            .name(String::from("index"))
            .spawn(move || serve(jobs))
            .expect("a thread for the index");
        Catalogue { to }
    }

    fn ask<T>(&self, job: impl FnOnce(Sender<T>) -> Job) -> Result<T> {
        let (back, answer) = channel();
        self.to
            .send(job(back))
            .map_err(|_| anyhow::anyhow!("the index manager has stopped"))?;
        answer
            .recv()
            .map_err(|_| anyhow::anyhow!("the index manager gave no answer"))
    }

    /// Open a folder's catalogue: open the file, bring it to the current shape on
    /// disk, and read it in. A folder with no catalogue gets a new one.
    pub fn open(&self, path: &Path) -> Result<()> {
        self.ask(|back| Job::Open(path.to_path_buf(), back))?
    }

    /// Close it, once everything it changed is on disk.
    pub fn close(&self) -> Result<()> {
        self.ask(Job::Close)?
    }

    /// Which folder's catalogue is open, if any.
    pub fn open_catalogue_path(&self) -> Option<PathBuf> {
        self.ask(Job::OpenCataloguePath).unwrap_or(None)
    }

    /// Every picture in the catalogue, as the search wants them.
    pub fn images(&self, cancel: Arc<AtomicBool>, report: Reporter) -> Result<Option<Vec<Image>>> {
        self.ask(|back| Job::Images(cancel, report, back))?
    }

    /// Search the catalogue and give back what it found.
    pub fn find_sets(
        &self,
        thresholds: Thresholds,
        cancel: Arc<AtomicBool>,
        report: Reporter,
    ) -> Result<Option<Vec<DuplicateSet>>> {
        self.ask(|back| Job::FindSets(thresholds, cancel, report, back))?
    }

    /// What the diff needs about every path already indexed.
    pub fn known(&self) -> Result<std::collections::HashMap<String, Known>> {
        self.ask(Job::Known)?
    }

    /// Every pair said not to be copies of each other.
    pub fn ignored(&self) -> Result<Vec<(i64, i64)>> {
        self.ask(Job::Ignored)?
    }

    /// A review is starting: make the tables it is written in, so the folder has
    /// a review from the moment somebody can mark a picture in it.
    pub fn begin_review(&self) -> Result<()> {
        self.ask(Job::BeginReview)?
    }

    /// Every picture a review marked to keep, from whenever it was reviewed.
    pub fn kept(&self) -> Result<Vec<i64>> {
        self.ask(Job::Kept)?
    }

    /// Mark these pictures to keep: what somebody just marked, and nothing else.
    pub fn keep_these(&self, file_ids: &[i64]) -> Result<()> {
        self.ask(|back| Job::KeepThese(file_ids.to_vec(), back))?
    }

    /// Take the mark off these pictures.
    pub fn unkeep_these(&self, file_ids: &[i64]) -> Result<()> {
        self.ask(|back| Job::UnkeepThese(file_ids.to_vec(), back))?
    }

    /// Take the marks away, for a review that has been carried out.
    pub fn clear_keep(&self) -> Result<()> {
        self.ask(Job::ClearKeep)?
    }

    /// The sets a search found and wrote down, in the order they were shown in.
    pub fn stored_sets(&self) -> Result<Vec<(i64, Vec<i64>)>> {
        self.ask(Job::StoredSets)?
    }

    /// Write down the sets a search found.
    pub fn store_sets(&self, sets: &[(i64, Vec<i64>)]) -> Result<()> {
        self.ask(|back| Job::StoreSets(sets.to_vec(), back))?
    }

    /// Take the stored sets away, for a folder about to be searched again or one
    /// whose review has been carried out.
    pub fn clear_sets(&self) -> Result<()> {
        self.ask(Job::ClearSets)?
    }

    pub fn meta(&self, key: &str) -> Result<Option<String>> {
        self.ask(|back| Job::Meta(key.to_string(), back))?
    }

    pub fn set_meta(&self, key: &str, value: &str) -> Result<()> {
        self.ask(|back| Job::SetMeta(key.to_string(), value.to_string(), back))?
    }

    /// Take a setting out, leaving a folder that has never been asked about it.
    pub fn forget_meta(&self, key: &str) -> Result<()> {
        self.ask(|back| Job::ForgetMeta(key.to_string(), back))?
    }

    pub fn ignore(&self, pairs: &[(i64, i64)]) -> Result<()> {
        self.ask(|back| Job::Ignore(pairs.to_vec(), back))?
    }

    pub fn unignore(&self, pairs: &[(i64, i64)]) -> Result<()> {
        self.ask(|back| Job::Unignore(pairs.to_vec(), back))?
    }

    /// Write a batch of pictures into the catalogue.
    pub fn upsert(&self, records: Vec<Record>, scanned_at: i64) -> Result<()> {
        self.ask(|back| Job::Upsert(records, scanned_at, back))?
    }

    /// Write down a batch of files the pass read and did not index, so the next
    /// pass and the next comparison know they have been looked at.
    pub fn not_pictures(&self, looked_at: Vec<db::Looked>, scanned_at: i64) -> Result<()> {
        self.ask(|back| Job::NotPictures(looked_at, scanned_at, back))?
    }

    /// Take paths out of the catalogue, giving back how many rows went.
    pub fn delete_paths(&self, paths: Vec<String>) -> Result<usize> {
        self.ask(|back| Job::DeletePaths(paths, back))?
    }

    /// Give back the space deleted rows left behind.
    pub fn compact(&self) -> Result<()> {
        self.ask(Job::Compact)?
    }

    /// Remove the catalogue from the folder. Gives back how many pictures it held.
    pub fn delete(&self) -> Result<usize> {
        self.ask(Job::Delete)?
    }

    /// Wait until the file holds everything the manager does. The file catches up
    /// on its own; this is for the times something wants to know that it has.
    pub fn synced(&self) -> Result<()> {
        self.ask(Job::Synced)?
    }
}

#[cfg(test)]
#[path = "tests/catalogue.rs"]
mod tests;
