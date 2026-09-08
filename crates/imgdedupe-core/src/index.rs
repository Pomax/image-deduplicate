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
use std::sync::atomic::AtomicBool;
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
    Images(Arc<AtomicBool>, Reporter, Sender<Result<Option<Vec<Image>>>>),
    FindSets(Thresholds, Arc<AtomicBool>, Reporter, Sender<Result<Option<Vec<DuplicateSet>>>>),
    Known(Sender<Result<std::collections::HashMap<String, Known>>>),
    Ignored(Sender<Result<Vec<(i64, i64)>>>),
    Meta(String, Sender<Result<Option<String>>>),
    SetMeta(String, String, Sender<Result<()>>),
    ForgetMeta(String, Sender<Result<()>>),
    Ignore(Vec<(i64, i64)>, Sender<Result<()>>),
    Unignore(Vec<(i64, i64)>, Sender<Result<()>>),
    Upsert(Vec<Record>, i64, Sender<Result<()>>),
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
        answer.recv().map_err(|_| anyhow::anyhow!("the index manager gave no answer"))
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
    pub fn images(
        &self,
        cancel: Arc<AtomicBool>,
        report: Reporter,
    ) -> Result<Option<Vec<Image>>> {
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
                let done = with(&current_open_index, |it| db::set_meta(&it.conn, &key, &value));
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
    if current_open_index.as_ref().is_some_and(|it| it.path.as_path() == path) {
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
    *current_open_index = Some(OpenIndex { path: path.to_path_buf(), conn });
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
    with_mut(current_open_index, |it| {
        it.conn.execute_batch("VACUUM").context("giving back the space of deleted rows")
    })?;
    // A VACUUM in memory does not change the size of the file, and a smaller
    // file is what the caller asked for.
    writer.apply(Box::new(|file| {
        file.execute_batch("VACUUM").context("giving back the space of deleted rows")
    }));
    writer.drain()
}

/// Remove the index from the folder and close it.
fn delete(current_open_index: &mut Option<OpenIndex>, writer: &Writer) -> Result<usize> {
    let (path, rows) = match current_open_index {
        Some(it) => {
            let rows: i64 = it
                .conn
                .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
                .unwrap_or(0);
            (it.path.clone(), rows as usize)
        }
        None => anyhow::bail!("no folder is open"),
    };
    // Nothing more is written on the way out: the file is going. Whatever is
    // already on its way has to land first, and the file has to be closed before
    // it is removed, or what is left behind is a `-journal` beside nothing.
    let _ = writer.close();
    *current_open_index = None;
    let mut gone = 0;
    for beside in db::files_of_the_index(&path) {
        if std::fs::remove_file(&beside).is_ok() {
            gone += 1;
        }
    }
    if gone == 0 {
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
mod tests {
    use super::*;
    use crate::db::Record;
    use crate::fingerprint::{Fingerprint, HASH_BYTES, VARIANTS};

    /// One picture's worth of index.
    fn row(rel_path: &str, size_bytes: i64) -> Record {
        Record {
            rel_path: rel_path.to_string(),
            size_bytes,
            mtime_ms: 1,
            width: 10,
            height: 10,
            format: crate::format::Format::Jpeg,
            channels: 3,
            fingerprint: Fingerprint {
                dct_hashes: [[0u8; HASH_BYTES]; VARIANTS],
                ring_stats: vec![0u8; 4],
            },
            corners: Vec::new(),
        }
    }

    /// The index as it is on disk. Nothing but this reads the file.
    fn on_disk(path: &Path) -> Connection {
        db::open_and_migrate(path).expect("read the index file")
    }

    fn temp() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("imgdedupe.sqlite");
        (dir, path)
    }

    /// A read and a write are both answered, and what comes back is an answer
    /// rather than a connection: the handle has no way of giving one out.
    ///
    /// An ask made before any folder is held says so, rather than answering with
    /// nothing, which is the empty database the old code substituted.
    #[test]
    fn nothing_but_the_manager_holds_the_index() {
        let (_dir, path) = temp();
        let index = Index::start();

        assert!(index.open_index_path().is_none(), "a manager that was told nothing holds something");
        let too_soon = index.known().expect_err("a read before a folder was given");
        assert!(too_soon.to_string().contains("no folder is open"), "{too_soon}");

        index.open(&path).expect("hold");
        assert_eq!(index.open_index_path().as_deref(), Some(path.as_path()));

        index.upsert(vec![row("a.jpg", 10)], 1).expect("a write");
        let known = index.known().expect("a read");
        assert_eq!(known.len(), 1, "the write was not there to be read back");
        assert!(known.contains_key("a.jpg"));
    }

    /// Letting go answers only once the file holds what the manager did.
    #[test]
    fn letting_go_of_a_folder_waits_for_the_file_to_catch_up() {
        let (_dir, path) = temp();
        let index = Index::start();
        index.open(&path).expect("hold");
        index.upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1).expect("write");
        index.set_meta("recurse", "1").expect("write");

        index.close().expect("let go");
        assert!(index.open_index_path().is_none(), "the folder was not let go of");

        let conn = on_disk(&path);
        let rows: i64 =
            conn.query_row("SELECT count(*) FROM files", [], |row| row.get(0)).expect("count");
        assert_eq!(rows, 2, "the file was behind what the manager had been holding");
        assert_eq!(db::get_meta(&conn, "recurse").expect("meta").as_deref(), Some("1"));
    }

    /// A pass that indexed nothing still leaves the file in step.
    ///
    /// This is what the old code got wrong: a pass that found no work skipped
    /// writing the index out, and the migration made on the way in went with it,
    /// so the next run migrated the same file again.
    #[test]
    fn a_scan_that_indexed_nothing_still_leaves_the_file_in_step() {
        let (_dir, path) = temp();

        // A folder whose index was made by a build that knew nothing of corners
        // and kept its stamps in nanoseconds.
        let older = rusqlite::Connection::open(&path).expect("make an older index");
        older
            .execute_batch(
                "CREATE TABLE files (
                   id INTEGER PRIMARY KEY,
                   rel_path TEXT NOT NULL UNIQUE,
                   size_bytes INTEGER NOT NULL,
                   mtime_ns INTEGER NOT NULL,
                   last_scanned_at INTEGER NOT NULL
                 );
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
                 INSERT INTO files(rel_path, size_bytes, mtime_ns, last_scanned_at)
                 VALUES ('a.jpg', 1, 1700000000123456789, 1);",
            )
            .expect("an older shape");
        drop(older);

        let index = Index::start();
        index.open(&path).expect("hold");
        // Nothing at all happens to it: no pass, no row, no setting.
        index.close().expect("let go");

        let conn = on_disk(&path);
        assert_eq!(
            db::get_meta(&conn, "schema_version").expect("meta").as_deref(),
            Some(db::SCHEMA_VERSION.to_string().as_str()),
            "the migration the manager made was thrown away"
        );
        let corners: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('fingerprints') WHERE name = 'corners'",
                [],
                |row| row.get(0),
            )
            .expect("looking for the column");
        assert_eq!(corners, 1, "the file was not left in the shape this build reads");

        // And the other thing an index that old needs: the stamp carried into
        // milliseconds and the nanosecond column gone.
        let known = db::load_known(&conn).expect("read the file back");
        assert_eq!(
            known["a.jpg"].mtime_ms, 1_700_000_000_123,
            "the stamp was not carried across"
        );
        let nanos: i64 = conn
            .query_row(
                "SELECT count(*) FROM pragma_table_info('files') WHERE name = 'mtime_ns'",
                [],
                |row| row.get(0),
            )
            .expect("looking for the column");
        assert_eq!(nanos, 0, "the nanosecond column is still there");
    }

    /// One of every kind of change, then let go: all of them are in the file.
    #[test]
    fn every_change_reaches_the_file() {
        let (_dir, path) = temp();
        let index = Index::start();
        index.open(&path).expect("hold");

        index
            .upsert(vec![row("a.jpg", 10), row("b.jpg", 20), row("gone.jpg", 30)], 1)
            .expect("upsert");
        index.set_meta("disposal", "delete").expect("set_meta");
        let ids: Vec<i64> = {
            let known = index.known().expect("known");
            let mut ids: Vec<i64> = ["a.jpg", "b.jpg"].iter().map(|path| known[*path].id).collect();
            ids.sort();
            ids
        };
        index.ignore(&[db::pair(ids[0], ids[1])]).expect("ignore");
        assert_eq!(index.delete_paths(vec![String::from("gone.jpg")]).expect("delete"), 1);
        index.compact().expect("compact");
        index.set_meta("to be forgotten", "here").expect("set_meta");
        index.forget_meta("to be forgotten").expect("forget_meta");

        index.close().expect("let go");

        let conn = on_disk(&path);
        let paths: Vec<String> = conn
            .prepare("SELECT rel_path FROM files ORDER BY rel_path")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .map(Result::unwrap)
            .collect();
        assert_eq!(paths, vec!["a.jpg".to_string(), "b.jpg".to_string()], "the rows are not right");
        assert_eq!(db::get_meta(&conn, "disposal").expect("meta").as_deref(), Some("delete"));
        assert_eq!(db::get_meta(&conn, "to be forgotten").expect("meta"), None);
        assert_eq!(db::ignored(&conn).expect("ignored"), vec![(ids[0], ids[1])]);
    }

    /// A change is on the disk once the writing has been waited for, without the
    /// index being closed first.
    #[test]
    fn a_change_is_in_the_file_once_the_writing_is_waited_for() {
        let (_dir, path) = temp();
        let index = Index::start();
        index.open(&path).expect("open");

        index.set_meta("disposal", "delete").expect("set_meta");
        index.synced().expect("wait for the disk");

        let conn = rusqlite::Connection::open(&path).expect("read the file");
        assert_eq!(db::get_meta(&conn, "disposal").expect("meta").as_deref(), Some("delete"));
    }

    /// A change is written into the file. It used to be written beside it and
    /// renamed over it, which left a different file each time.
    #[test]
    fn a_change_does_not_replace_the_file() {
        let (_dir, path) = temp();
        let index = Index::start();
        index.open(&path).expect("open");
        index.synced().expect("wait for the disk");
        let was = std::fs::metadata(&path).expect("the index").created().expect("when it was made");

        index.upsert(vec![row("a.jpg", 10)], 1).expect("upsert");
        index.set_meta("disposal", "delete").expect("set_meta");
        index.synced().expect("wait for the disk");

        let now = std::fs::metadata(&path).expect("the index").created().expect("when it was made");
        assert_eq!(was, now, "the index was replaced rather than written to");
    }

    /// The rows that follow a file are the file's: they go when it does, on the
    /// disk as well as in memory. This is what the writer's own connection needs
    /// `foreign_keys` on for.
    #[test]
    fn deleting_a_file_takes_its_ignored_pairs_out_of_the_file() {
        let (_dir, path) = temp();
        let index = Index::start();
        index.open(&path).expect("open");
        index.upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1).expect("upsert");
        let ids: Vec<i64> = {
            let known = index.known().expect("known");
            let mut ids: Vec<i64> = ["a.jpg", "b.jpg"].iter().map(|path| known[*path].id).collect();
            ids.sort();
            ids
        };
        index.ignore(&[db::pair(ids[0], ids[1])]).expect("ignore");
        index.synced().expect("wait for the disk");

        index.delete_paths(vec![String::from("a.jpg")]).expect("delete");
        index.synced().expect("wait for the disk");

        let conn = rusqlite::Connection::open(&path).expect("read the file");
        assert!(
            db::ignored(&conn).expect("ignored").is_empty(),
            "the pair outlived its picture in the file"
        );
    }

    /// Giving back the space of deleted rows is asked for to make the file
    /// smaller, so it has to make the file smaller.
    #[test]
    fn compacting_makes_the_file_smaller() {
        let (_dir, path) = temp();
        let index = Index::start();
        index.open(&path).expect("open");
        let rows: Vec<Record> =
            (0..400).map(|at| row(&format!("{at}.jpg"), at as i64)).collect();
        index.upsert(rows, 1).expect("upsert");
        index.synced().expect("wait for the disk");

        let paths: Vec<String> = (0..400).map(|at| format!("{at}.jpg")).collect();
        index.delete_paths(paths).expect("delete");
        index.synced().expect("wait for the disk");
        let full = std::fs::metadata(&path).expect("the index").len();

        index.compact().expect("compact");
        let after = std::fs::metadata(&path).expect("the index").len();
        assert!(after < full, "the file was {full} bytes and is {after} after compacting");
    }

    /// The two ways an index cannot be read. Each is refused, each leaves the
    /// file exactly as it was, and no index is left open.
    #[test]
    fn a_broken_index_stops_the_manager_and_writes_nothing() {
        let dir = tempfile::tempdir().expect("tempdir");

        // Not a database at all.
        let rubbish = dir.path().join("rubbish.sqlite");
        std::fs::write(&rubbish, b"not a database, just some bytes").expect("fixture");

        // A shape this build cannot bring to the current one: an index written
        // under another schema version.
        let ahead = dir.path().join("ahead.sqlite");
        drop(db::open_and_migrate(&ahead).expect("make one"));
        let conn = rusqlite::Connection::open(&ahead).expect("open it");
        db::set_meta(&conn, "schema_version", "99").expect("move it on");
        drop(conn);

        for (path, why) in [(&rubbish, "not a database"), (&ahead, "another schema version")] {
            let was = std::fs::read(path).expect("read what is there");
            let index = Index::start();
            let refused = index.open(path).expect_err("this was not refused").to_string();
            assert!(!refused.is_empty(), "{why} was refused without saying why");
            assert!(index.open_index_path().is_none(), "{why} left an index open");
            assert_eq!(std::fs::read(path).expect("read"), was, "{why} was written over: {refused}");
        }
    }


    /// Nothing outside the manager opens a database.
    ///
    /// This is the rule all of this is for, and the only thing that keeps it true
    /// once it is true. `db.rs` holds the one opener and `index.rs` is the
    /// manager that calls it; a connection made anywhere else is a second owner
    /// of the file.
    #[test]
    fn no_connection_is_made_outside_the_manager() {
        let crates = Path::new(env!("CARGO_MANIFEST_DIR")).parent().expect("the crates folder");
        let allowed = ["db.rs", "index.rs"];
        let ways = ["Connection::open", "open_in_memory", "open_with_flags"];

        let mut found = Vec::new();
        let mut looked_at = 0;
        for file in every_source_file(crates) {
            let name = file.file_name().and_then(|it| it.to_str()).unwrap_or_default();
            if allowed.contains(&name) {
                continue;
            }
            let text = std::fs::read_to_string(&file).expect("read a source file");
            looked_at += 1;
            // A test may forge a file on disk for the manager to open; nothing
            // the program does may open one.
            let program = match text.find("#[cfg(test)]") {
                Some(at) => &text[..at],
                None => &text[..],
            };
            for (number, line) in program.lines().enumerate() {
                if ways.iter().any(|way| line.contains(way)) {
                    found.push(format!("{}:{}: {}", file.display(), number + 1, line.trim()));
                }
            }
        }
        assert!(looked_at > 10, "the source of the two crates was not found to read");
        assert!(found.is_empty(), "a database is opened outside the manager:\n{}", found.join("\n"));
    }

    /// Every `.rs` file under the two crates.
    fn every_source_file(from: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        let mut queue = vec![from.to_path_buf()];
        while let Some(dir) = queue.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.filter_map(std::result::Result::ok) {
                let path = entry.path();
                if path.is_dir() {
                    if path.file_name().is_some_and(|it| it == "target") {
                        continue;
                    }
                    queue.push(path);
                } else if path.extension().is_some_and(|it| it == "rs") {
                    out.push(path);
                }
            }
        }
        out
    }
}
