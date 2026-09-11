//! The one owner of a folder's index.
//!
//! Nothing else opens `imgdedupe.sqlite`, holds a connection to it, or names its
//! path. Everything that wants something from the index sends this a message and
//! waits for the answer.
//!
//! What it holds in memory is the data. The file is a copy of that, kept in step
//! on a thread of its own, so a caller is answered as soon as the manager has the
//! change rather than when the disk does. Nothing reads the file after it has
//! been opened.
//!
//! One manager holds one folder at a time. `hold` lets go of whatever it has and
//! takes up another folder's index: it opens the file, brings it to the current
//! shape on disk, and reads it in. An index it cannot read, cannot write to, or
//! cannot bring to the current shape is a broken index: `hold` says so, the
//! manager holds nothing, and no file is touched.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;

use anyhow::{Context, Result};

use crate::db::{self, Connection, Known, Record};
use crate::matching::{self, DuplicateSet, Image, Progress, Thresholds};

/// What a caller asks of the manager. Every one carries the way back.
enum Job {
    Open(PathBuf, Sender<Result<()>>),
    Close(Sender<Result<()>>),
    OpenIndexPath(Sender<Option<PathBuf>>),
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
pub struct Index {
    to: Sender<Job>,
}

impl std::fmt::Debug for Index {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self.open_index_path() {
            Some(path) => write!(out, "index open on {}", path.display()),
            None => write!(out, "no index open"),
        }
    }
}

/// The index the manager has open.
struct OpenIndex {
    path: PathBuf,
    conn: Connection,
}

impl Index {
    /// Start the manager. It holds nothing until it is told to hold a folder,
    /// and touches no file until then.
    pub fn start() -> Index {
        let (to, jobs) = channel::<Job>();
        std::thread::Builder::new()
            .name(String::from("index"))
            .spawn(move || serve(jobs))
            .expect("a thread for the index");
        Index { to }
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

    /// Open a folder's index: open the file, bring it to the current shape on
    /// disk, and read it in. A folder with no index gets a new one.
    pub fn open(&self, path: &Path) -> Result<()> {
        self.ask(|back| Job::Open(path.to_path_buf(), back))?
    }

    /// Close it, once everything it changed is on disk.
    pub fn close(&self) -> Result<()> {
        self.ask(Job::Close)?
    }

    /// Which folder's index is open, if any.
    pub fn open_index_path(&self) -> Option<PathBuf> {
        self.ask(Job::OpenIndexPath).unwrap_or(None)
    }

    /// Every picture in the index, as the search wants them.
    pub fn images(&self, cancel: Arc<AtomicBool>, report: Reporter) -> Result<Option<Vec<Image>>> {
        self.ask(|back| Job::Images(cancel, report, back))?
    }

    /// Search the index and give back what it found.
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

    /// Write a batch of pictures into the index.
    pub fn upsert(&self, records: Vec<Record>, scanned_at: i64) -> Result<()> {
        self.ask(|back| Job::Upsert(records, scanned_at, back))?
    }

    /// Write down a batch of files the pass read and did not index, so the next
    /// pass and the next comparison know they have been looked at.
    pub fn not_pictures(&self, looked_at: Vec<db::Looked>, scanned_at: i64) -> Result<()> {
        self.ask(|back| Job::NotPictures(looked_at, scanned_at, back))?
    }

    /// Take paths out of the index, giving back how many rows went.
    pub fn delete_paths(&self, paths: Vec<String>) -> Result<usize> {
        self.ask(|back| Job::DeletePaths(paths, back))?
    }

    /// Give back the space deleted rows left behind.
    pub fn compact(&self) -> Result<()> {
        self.ask(Job::Compact)?
    }

    /// Remove the index from the folder. Gives back how many pictures it held.
    pub fn delete(&self) -> Result<usize> {
        self.ask(Job::Delete)?
    }

    /// Wait until the file holds everything the manager does. The file catches up
    /// on its own; this is for the times something wants to know that it has.
    pub fn synced(&self) -> Result<()> {
        self.ask(Job::Synced)?
    }
}

/// The manager's own loop. Owns the connection and the path; neither leaves it.
fn serve(jobs: Receiver<Job>) {
    let mut current_open_index: Option<OpenIndex> = None;
    let writer = Writer::start();

    while let Ok(job) = jobs.recv() {
        match job {
            Job::Open(path, back) => {
                let outcome = open_index(&mut current_open_index, &writer, &path);
                let _ = back.send(outcome);
            }
            Job::Close(back) => {
                let _ = back.send(close_index(&mut current_open_index, &writer));
            }
            Job::OpenIndexPath(back) => {
                let _ = back.send(current_open_index.as_ref().map(|it| it.path.clone()));
            }
            Job::Images(cancel, report, back) => {
                let _ = back.send(with(&current_open_index, |it| {
                    matching::load_images(&it.conn, &cancel, &|progress| report(progress))
                }));
            }
            Job::FindSets(thresholds, cancel, report, back) => {
                let _ = back.send(with(&current_open_index, |it| {
                    matching::find_sets_cancellable(&it.conn, thresholds, &cancel, &|progress| {
                        report(progress)
                    })
                }));
            }
            Job::Known(back) => {
                let _ = back.send(with(&current_open_index, |it| db::load_known(&it.conn)));
            }
            Job::Ignored(back) => {
                let _ = back.send(with(&current_open_index, |it| db::ignored(&it.conn)));
            }
            Job::Meta(key, back) => {
                let _ = back.send(with(&current_open_index, |it| db::get_meta(&it.conn, &key)));
            }
            Job::SetMeta(key, value, back) => {
                let done = with(&current_open_index, |it| {
                    db::set_meta(&it.conn, &key, &value)
                });
                let _ = back.send(done);
                writer.apply(Box::new(move |file| db::set_meta(file, &key, &value)));
            }
            Job::ForgetMeta(key, back) => {
                let done = with(&current_open_index, |it| db::forget_meta(&it.conn, &key));
                let _ = back.send(done);
                writer.apply(Box::new(move |file| db::forget_meta(file, &key)));
            }
            Job::Ignore(pairs, back) => {
                let done = with(&current_open_index, |it| db::ignore(&it.conn, &pairs));
                let _ = back.send(done);
                writer.apply(Box::new(move |file| db::ignore(file, &pairs)));
            }
            Job::Unignore(pairs, back) => {
                let done = with(&current_open_index, |it| db::unignore(&it.conn, &pairs));
                let _ = back.send(done);
                writer.apply(Box::new(move |file| db::unignore(file, &pairs)));
            }
            Job::Kept(back) => {
                let _ = back.send(with(&current_open_index, |it| db::kept(&it.conn)));
            }
            // These four are asked of a folder that may not be open yet: the
            // window writes a review as it happens, and a folder is chosen before
            // its index has been opened. A change the copy in memory did not make
            // is not sent to the file, or the writer is handed work it has
            // nowhere to do and reports it as trouble.
            Job::BeginReview(back) => {
                let done = with(&current_open_index, |it| db::begin_review(&it.conn));
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(db::begin_review));
                }
            }
            Job::KeepThese(file_ids, back) => {
                let done = with(&current_open_index, |it| {
                    db::keep_these(&it.conn, &file_ids)
                });
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(move |file| db::keep_these(file, &file_ids)));
                }
            }
            Job::UnkeepThese(file_ids, back) => {
                let done = with(&current_open_index, |it| {
                    db::unkeep_these(&it.conn, &file_ids)
                });
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(move |file| db::unkeep_these(file, &file_ids)));
                }
            }
            Job::ClearKeep(back) => {
                let done = with(&current_open_index, |it| db::clear_keep(&it.conn));
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(db::clear_keep));
                }
            }
            Job::StoredSets(back) => {
                let _ = back.send(with(&current_open_index, |it| db::stored_sets(&it.conn)));
            }
            // Not somebody doing something: a search handing over everything it
            // found, which on a folder of any size is hundreds of rows. They go to
            // the file in one transaction, the way a pass's records do, so it is
            // one commit rather than one per picture per set.
            Job::StoreSets(sets, back) => {
                let done = with_mut(&mut current_open_index, |it| {
                    let tx = it.conn.transaction()?;
                    db::store_sets(&tx, &sets)?;
                    tx.commit()?;
                    Ok(())
                });
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(move |file| {
                        let tx = file.unchecked_transaction()?;
                        db::store_sets(&tx, &sets)?;
                        tx.commit()?;
                        Ok(())
                    }));
                }
            }
            Job::ClearSets(back) => {
                let done = with(&current_open_index, |it| db::clear_sets(&it.conn));
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(db::clear_sets));
                }
            }
            Job::Upsert(records, scanned_at, back) => {
                let done = with_mut(&mut current_open_index, |it| {
                    let tx = it.conn.transaction()?;
                    for record in &records {
                        db::upsert(&tx, record, scanned_at)?;
                    }
                    tx.commit()?;
                    Ok(())
                });
                let _ = back.send(done);
                // A pass sends thousands of records through here. They go to the
                // file in one transaction, so it gets one commit rather than one
                // per picture.
                writer.apply(Box::new(move |file| {
                    let tx = file.unchecked_transaction()?;
                    for record in &records {
                        db::upsert(&tx, record, scanned_at)?;
                    }
                    tx.commit()?;
                    Ok(())
                }));
            }
            Job::NotPictures(looked_at, scanned_at, back) => {
                let done = with_mut(&mut current_open_index, |it| {
                    let tx = it.conn.transaction()?;
                    for one in &looked_at {
                        db::not_a_picture(&tx, one, scanned_at)?;
                    }
                    tx.commit()?;
                    Ok(())
                });
                let _ = back.send(done);
                writer.apply(Box::new(move |file| {
                    let tx = file.unchecked_transaction()?;
                    for one in &looked_at {
                        db::not_a_picture(&tx, one, scanned_at)?;
                    }
                    tx.commit()?;
                    Ok(())
                }));
            }
            Job::DeletePaths(paths, back) => {
                let done = with_mut(&mut current_open_index, |it| {
                    let tx = it.conn.transaction()?;
                    let gone = db::delete_paths(&tx, &paths)?;
                    tx.commit()?;
                    Ok(gone)
                });
                let _ = back.send(done);
                writer.apply(Box::new(move |file| {
                    let tx = file.unchecked_transaction()?;
                    db::delete_paths(&tx, &paths)?;
                    tx.commit()?;
                    Ok(())
                }));
            }
            Job::Compact(back) => {
                let _ = back.send(compact(&mut current_open_index, &writer));
            }
            Job::Delete(back) => {
                let _ = back.send(delete(&mut current_open_index, &writer));
            }
            Job::Synced(back) => {
                let _ = back.send(writer.drain());
            }
        }
    }

    // The last way to ask has gone. Whatever is held goes to disk before the
    // thread does.
    let _ = close_index(&mut current_open_index, &writer);
}

/// Do something with the open index, or say that none is open.
fn with<T>(
    current_open_index: &Option<OpenIndex>,
    work: impl FnOnce(&OpenIndex) -> Result<T>,
) -> Result<T> {
    match current_open_index {
        Some(it) => work(it),
        None => anyhow::bail!("no folder is open"),
    }
}

fn with_mut<T>(
    current_open_index: &mut Option<OpenIndex>,
    work: impl FnOnce(&mut OpenIndex) -> Result<T>,
) -> Result<T> {
    match current_open_index {
        Some(it) => work(it),
        None => anyhow::bail!("no folder is open"),
    }
}

/// Open a folder's index: migrate the file, then read it in.
fn open_index(
    current_open_index: &mut Option<OpenIndex>,
    writer: &Writer,
    path: &Path,
) -> Result<()> {
    // Already open on this one. Closing it and opening it again reads the whole
    // file back for nothing. The window opens a folder and the pass asks for the
    // same folder a moment later, so this is the usual case, not a rare one.
    if current_open_index
        .as_ref()
        .is_some_and(|it| it.path.as_path() == path)
    {
        return Ok(());
    }
    close_index(current_open_index, writer)?;
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let conn = db::open_and_migrate(path)?;
    crate::log_line!("  open and migrate: {:.2}s", at.elapsed().as_secs_f64());
    // The file, for the thread that writes to it. Opening it is waited for: an
    // index that cannot be written to is a broken index and the caller is told
    // now rather than at the first change.
    writer.open(path)?;
    *current_open_index = Some(OpenIndex {
        path: path.to_path_buf(),
        conn,
    });
    Ok(())
}

/// Close the index, once the file has caught up with what was changed in it.
fn close_index(current_open_index: &mut Option<OpenIndex>, writer: &Writer) -> Result<()> {
    if current_open_index.is_none() {
        return Ok(());
    }
    // Nothing to write out first: every change was written when it was made.
    let caught_up = writer.close();
    // Close it either way: keeping an index open whose file cannot be written
    // gains nothing. The caller is told what went wrong.
    *current_open_index = None;
    caught_up
}

fn compact(current_open_index: &mut Option<OpenIndex>, writer: &Writer) -> Result<()> {
    // Tidied on this machine's own disk, never on the folder's.
    //
    // Tidying writes a database out a page at a time. The folder can be on
    // another machine, and done there that is the whole index across the network
    // in small writes with a journal beside it: a minute on a hundred-megabyte
    // index. On a local disk those pages cost nothing, and what crosses the
    // network afterwards is one finished file, written once.
    //
    // This is the one thing that replaces the index file rather than writing
    // changes into it, because it is the one thing that changes every byte of it.
    //
    // Written out by SQLite rather than built in memory first: the index is
    // already held whole, a large folder's being hundreds of megabytes, and
    // taking the tidied database as bytes as well would be a second copy of it
    // beside the first.
    let path = {
        let it = current_open_index.as_ref().context("no folder is open")?;
        it.path.clone()
    };
    // One name per tidying, not one per program: two folders can be tidied at
    // once, and they would otherwise write over each other's file.
    static TIDYINGS: AtomicU64 = AtomicU64::new(0);
    let local = std::env::temp_dir().join(format!(
        "imgdedupe-tidying-{}-{}.sqlite",
        std::process::id(),
        TIDYINGS.fetch_add(1, Ordering::Relaxed)
    ));
    let _ = std::fs::remove_file(&local);
    {
        let it = current_open_index.as_ref().context("no folder is open")?;
        it.conn
            .execute("VACUUM INTO ?1", [local.to_string_lossy().as_ref()])
            .with_context(|| format!("tidying the index at {}", local.display()))?;
    }

    // The writer's connection is closed while the file underneath it is replaced,
    // and opened again on the new one. Closing it is also what waits: the writer
    // takes its errands in order, so everything written before this has reached
    // the file by the time the close comes back, and any trouble with it is
    // reported here.
    writer.close()?;
    // Back over the index, in one copy. If that fails the folder still has the
    // index it had: the tidied file is the copy, and nothing has been taken away
    // from the folder until this succeeds.
    let put_back = std::fs::copy(&local, &path)
        .with_context(|| format!("putting the tidied index back at {}", path.display()));
    let _ = std::fs::remove_file(&local);
    put_back?;
    writer.open(&path)
}

/// Remove the index from the folder and close it.
fn delete(current_open_index: &mut Option<OpenIndex>, writer: &Writer) -> Result<usize> {
    let (path, rows) = match current_open_index {
        Some(it) => {
            // Pictures, not rows: `files` also holds what the pass looked at and
            // could not index, and "the index went, and N pictures with it" is
            // what this number is read as.
            let rows: i64 = it
                .conn
                .query_row(
                    "SELECT count(*) FROM files WHERE not_a_picture = 0",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(0);
            (it.path.clone(), rows as usize)
        }
        None => anyhow::bail!("no folder is open"),
    };
    // Nothing more is written on the way out: the file is going. Whatever is
    // already on its way has to land first, and the file is closed before it is
    // removed, which is when SQLite takes away whatever it keeps beside it.
    let _ = writer.close();
    *current_open_index = None;
    if std::fs::remove_file(&path).is_err() {
        anyhow::bail!("nothing was removed at {}", path.display());
    }
    Ok(rows)
}

/// The thread that makes on disk the changes the manager has made in memory.
///
/// It holds a connection to the index file and applies each change with a
/// statement, so a change of one row writes one row. The manager is free for the
/// next job as soon as it has sent the change.
struct Writer {
    to: Sender<Errand>,
}

/// One change to make to the file, as the call that made it in memory.
type Change = Box<dyn FnOnce(&Connection) -> Result<()> + Send>;

enum Errand {
    /// Open the file, ready to be written to.
    Open(PathBuf, Sender<Option<String>>),
    /// Close it. Nothing is written on the way out: everything was written as it
    /// was made.
    Close(Sender<Option<String>>),
    Do(Change),
    /// Answered when everything sent before it has been written, with whatever
    /// went wrong writing it.
    Drained(Sender<Option<String>>),
}

impl Writer {
    fn start() -> Writer {
        let (to, errands) = channel::<Errand>();
        std::thread::Builder::new()
            .name(String::from("index writer"))
            .spawn(move || {
                // The index file, once the manager has opened one.
                let mut file: Option<Connection> = None;
                // What went wrong since anything last asked. A caller waiting on
                // the file is told, so an index is never closed in the belief
                // that what was written to it reached the disk.
                let mut trouble: Option<String> = None;
                while let Ok(errand) = errands.recv() {
                    match errand {
                        Errand::Open(path, back) => {
                            let opened = open_the_file(&path);
                            let answer = match opened {
                                Ok(conn) => {
                                    file = Some(conn);
                                    None
                                }
                                Err(err) => {
                                    file = None;
                                    Some(err)
                                }
                            };
                            let _ = back.send(answer);
                        }
                        Errand::Close(back) => {
                            file = None;
                            let _ = back.send(trouble.take());
                        }
                        Errand::Do(change) => {
                            let done = match &file {
                                Some(conn) => change(conn).map_err(|err| format!("{err:#}")),
                                None => Err(String::from("no index is open to write to")),
                            };
                            if let Err(err) = done {
                                crate::log_line!("{err}");
                                trouble = Some(err);
                            }
                        }
                        // Everything sent before this has been applied, because
                        // this thread takes them one at a time in order.
                        Errand::Drained(back) => {
                            let _ = back.send(trouble.take());
                        }
                    }
                }
            })
            .expect("a thread to write the index");
        Writer { to }
    }

    /// Open the file this index lives in, and say if it could not be opened.
    fn open(&self, path: &Path) -> Result<()> {
        let path = path.to_path_buf();
        self.wait_on(|back| Errand::Open(path, back))
    }

    /// Send a change to make to the file. Nothing waits for it: the caller is
    /// answered when the manager has the change, and `drain` is what waits for
    /// the disk.
    fn apply(&self, change: Change) {
        let _ = self.to.send(Errand::Do(change));
    }

    /// Close the file, once everything sent has been written to it.
    fn close(&self) -> Result<()> {
        self.wait_on(Errand::Close)
    }

    /// Wait until everything sent so far is on disk, and say what stopped any of
    /// it getting there.
    fn drain(&self) -> Result<()> {
        self.wait_on(Errand::Drained)
    }

    /// Send an errand that answers, and wait for the answer.
    fn wait_on(&self, errand: impl FnOnce(Sender<Option<String>>) -> Errand) -> Result<()> {
        let (back, done) = channel();
        if self.to.send(errand(back)).is_err() {
            anyhow::bail!("the thread that writes the index has stopped");
        }
        match done.recv() {
            Ok(None) => Ok(()),
            Ok(Some(trouble)) => anyhow::bail!(trouble),
            Err(_) => anyhow::bail!("the thread that writes the index gave no answer"),
        }
    }
}

/// The index file, as the thread that writes to it holds it.
fn open_the_file(path: &Path) -> std::result::Result<Connection, String> {
    let conn = Connection::open(path)
        .map_err(|err| format!("the index at {} could not be opened: {err}", path.display()))?;
    // The same as the copy in memory is opened with. Without it, deleting a file
    // takes its rows elsewhere with it in memory and leaves them in the file.
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|err| format!("the index at {} refused a setting: {err}", path.display()))?;
    Ok(conn)
}

#[cfg(test)]
#[path = "tests/index.rs"]
mod tests;
