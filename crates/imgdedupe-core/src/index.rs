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
    Hold(PathBuf, Sender<Result<()>>),
    LetGo(Sender<Result<()>>),
    Held(Sender<Option<PathBuf>>),
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
        match self.holding() {
            Some(path) => write!(out, "index holding {}", path.display()),
            None => write!(out, "index holding nothing"),
        }
    }
}

/// What the manager is holding.
struct Held {
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

    /// Take up a folder's index: open the file, bring it to the current shape on
    /// disk, and read it in. A folder with no index gets a new one.
    pub fn hold(&self, path: &Path) -> Result<()> {
        self.ask(|back| Job::Hold(path.to_path_buf(), back))?
    }

    /// Let the folder go, once everything it changed is on disk.
    pub fn let_go(&self) -> Result<()> {
        self.ask(Job::LetGo)?
    }

    /// Which folder's index is held, if any.
    pub fn holding(&self) -> Option<PathBuf> {
        self.ask(Job::Held).unwrap_or(None)
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
    let mut held: Option<Held> = None;
    let writer = Writer::start();

    while let Ok(job) = jobs.recv() {
        match job {
            Job::Hold(path, back) => {
                let outcome = take_up(&mut held, &writer, &path);
                let _ = back.send(outcome);
            }
            Job::LetGo(back) => {
                let _ = back.send(put_down(&mut held, &writer));
            }
            Job::Held(back) => {
                let _ = back.send(held.as_ref().map(|it| it.path.clone()));
            }
            Job::Images(cancel, report, back) => {
                let _ = back.send(with(&held, |it| {
                    matching::load_images(&it.conn, &cancel, &|progress| report(progress))
                }));
            }
            Job::FindSets(thresholds, cancel, report, back) => {
                let _ = back.send(with(&held, |it| {
                    matching::find_sets_cancellable(&it.conn, thresholds, &cancel, &|progress| {
                        report(progress)
                    })
                }));
            }
            Job::Known(back) => {
                let _ = back.send(with(&held, |it| db::load_known(&it.conn)));
            }
            Job::Ignored(back) => {
                let _ = back.send(with(&held, |it| db::ignored(&it.conn)));
            }
            Job::Meta(key, back) => {
                let _ = back.send(with(&held, |it| db::get_meta(&it.conn, &key)));
            }
            Job::SetMeta(key, value, back) => {
                let done = with(&held, |it| db::set_meta(&it.conn, &key, &value));
                let _ = back.send(done);
                sync(&held, &writer);
            }
            Job::ForgetMeta(key, back) => {
                let done = with(&held, |it| db::forget_meta(&it.conn, &key));
                let _ = back.send(done);
                sync(&held, &writer);
            }
            Job::Ignore(pairs, back) => {
                let done = with(&held, |it| db::ignore(&it.conn, &pairs));
                let _ = back.send(done);
                sync(&held, &writer);
            }
            Job::Unignore(pairs, back) => {
                let done = with(&held, |it| db::unignore(&it.conn, &pairs));
                let _ = back.send(done);
                sync(&held, &writer);
            }
            Job::Upsert(records, scanned_at, back) => {
                let done = with_mut(&mut held, |it| {
                    let tx = it.conn.transaction()?;
                    for record in &records {
                        db::upsert(&tx, record, scanned_at)?;
                    }
                    tx.commit()?;
                    Ok(())
                });
                let _ = back.send(done);
                sync(&held, &writer);
            }
            Job::DeletePaths(paths, back) => {
                let done = with_mut(&mut held, |it| {
                    let tx = it.conn.transaction()?;
                    let gone = db::delete_paths(&tx, &paths)?;
                    tx.commit()?;
                    Ok(gone)
                });
                let _ = back.send(done);
                sync(&held, &writer);
            }
            Job::Compact(back) => {
                let _ = back.send(compact(&mut held, &writer));
            }
            Job::Delete(back) => {
                let _ = back.send(delete(&mut held, &writer));
            }
            Job::Synced(back) => {
                sync(&held, &writer);
                let _ = back.send(writer.drain());
            }
        }
    }

    // The last way to ask has gone. Whatever is held goes to disk before the
    // thread does.
    let _ = put_down(&mut held, &writer);
}

/// Do something with what is held, or say that nothing is.
fn with<T>(held: &Option<Held>, work: impl FnOnce(&Held) -> Result<T>) -> Result<T> {
    match held {
        Some(it) => work(it),
        None => anyhow::bail!("no folder is open"),
    }
}

fn with_mut<T>(held: &mut Option<Held>, work: impl FnOnce(&mut Held) -> Result<T>) -> Result<T> {
    match held {
        Some(it) => work(it),
        None => anyhow::bail!("no folder is open"),
    }
}

/// Take up a folder's index: migrate the file, then read it in.
fn take_up(held: &mut Option<Held>, writer: &Writer, path: &Path) -> Result<()> {
    put_down(held, writer)?;
    let conn = db::open_and_migrate(path)?;
    *held = Some(Held { path: path.to_path_buf(), conn });
    // A folder with no index has one now, and one that was migrated is on disk
    // in the new shape.
    sync(held, writer);
    Ok(())
}

/// Let go of the folder, once the file has caught up with what is held.
fn put_down(held: &mut Option<Held>, writer: &Writer) -> Result<()> {
    if held.is_none() {
        return Ok(());
    }
    sync(held, writer);
    let caught_up = writer.drain();
    // Let go either way: holding on to an index whose file cannot be written
    // gains nothing. The caller is told what went wrong.
    *held = None;
    caught_up
}

/// Hand the writer the whole of what is held. The writer does the file work, so
/// the manager is free for the next job.
fn sync(held: &Option<Held>, writer: &Writer) {
    let Some(it) = held else {
        return;
    };
    match it.conn.serialize(rusqlite::DatabaseName::Main) {
        Ok(data) => writer.put(it.path.clone(), data.to_vec()),
        Err(err) => {
            let _ = &err;
            crate::log_line!("the index could not be taken out of memory: {err}");
        }
    }
}

fn compact(held: &mut Option<Held>, writer: &Writer) -> Result<()> {
    with_mut(held, |it| {
        it.conn.execute_batch("VACUUM").context("giving back the space of deleted rows")
    })?;
    // A rebuilt index is a smaller file, and that is the point of asking, so the
    // file is brought up to date here rather than whenever the writer gets to it.
    sync(held, writer);
    writer.drain()
}

/// Remove the index from the folder and let it go.
fn delete(held: &mut Option<Held>, writer: &Writer) -> Result<usize> {
    let (path, rows) = match held {
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
    // already on its way has to land first, or it lands on the empty space.
    let _ = writer.drain();
    *held = None;
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

/// The thread that puts what the manager holds on disk.
///
/// It is given the whole of the index each time. A newer copy arriving while one
/// is being written replaces whatever was waiting, so a run of changes costs one
/// write of the last of them rather than one write each.
struct Writer {
    to: Sender<Errand>,
}

enum Errand {
    Put(PathBuf, Vec<u8>),
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
                // What went wrong since anything last asked. A caller waiting on
                // the file is told, so a folder is never let go of in the belief
                // that its index reached the disk.
                let mut trouble: Option<String> = None;
                while let Ok(first) = errands.recv() {
                    // Take everything that is already waiting. Only the last copy
                    // of the index is worth writing, so a run of changes costs
                    // one write rather than one write each.
                    let mut latest: Option<(PathBuf, Vec<u8>)> = None;
                    let mut waiting: Vec<Sender<Option<String>>> = Vec::new();
                    let mut errand = Some(first);
                    while let Some(next) = errand {
                        match next {
                            Errand::Put(path, bytes) => latest = Some((path, bytes)),
                            Errand::Drained(back) => waiting.push(back),
                        }
                        errand = errands.try_recv().ok();
                    }
                    if let Some((path, bytes)) = latest {
                        if let Err(err) = write_file(&path, &bytes) {
                            crate::log_line!("{err}");
                            trouble = Some(err);
                        }
                    }
                    // Everything sent before these is on disk now.
                    for back in waiting {
                        let _ = back.send(trouble.take());
                    }
                }
            })
            .expect("a thread to write the index");
        Writer { to }
    }

    fn put(&self, path: PathBuf, bytes: Vec<u8>) {
        let _ = self.to.send(Errand::Put(path, bytes));
    }

    /// Wait until everything sent so far is on disk, and say what stopped any of
    /// it getting there.
    fn drain(&self) -> Result<()> {
        let (back, done) = channel();
        if self.to.send(Errand::Drained(back)).is_err() {
            anyhow::bail!("the thread that writes the index has stopped");
        }
        match done.recv() {
            Ok(None) => Ok(()),
            Ok(Some(trouble)) => anyhow::bail!(trouble),
            Err(_) => anyhow::bail!("the thread that writes the index gave no answer"),
        }
    }
}

/// One sequential write beside the index, then a rename onto it, so a run that
/// dies half way leaves the old index rather than half of a new one.
fn write_file(path: &Path, bytes: &[u8]) -> std::result::Result<(), String> {
    let beside = db::being_written(path);
    if let Err(err) = std::fs::write(&beside, bytes) {
        return Err(format!("the index could not be written to {}: {err}", beside.display()));
    }
    if let Err(err) = std::fs::rename(&beside, path) {
        let _ = std::fs::remove_file(&beside);
        return Err(format!("the index could not be moved onto {}: {err}", path.display()));
    }
    Ok(())
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
            mtime_ns: 1,
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

        assert!(index.holding().is_none(), "a manager that was told nothing holds something");
        let too_soon = index.known().expect_err("a read before a folder was given");
        assert!(too_soon.to_string().contains("no folder is open"), "{too_soon}");

        index.hold(&path).expect("hold");
        assert_eq!(index.holding().as_deref(), Some(path.as_path()));

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
        index.hold(&path).expect("hold");
        index.upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1).expect("write");
        index.set_meta("recurse", "1").expect("write");

        index.let_go().expect("let go");
        assert!(index.holding().is_none(), "the folder was not let go of");

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

        // A folder whose index was made by a build that knew nothing of corners.
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
                 CREATE TABLE meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);",
            )
            .expect("an older shape");
        drop(older);

        let index = Index::start();
        index.hold(&path).expect("hold");
        // Nothing at all happens to it: no pass, no row, no setting.
        index.let_go().expect("let go");

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
    }

    /// One of every kind of change, then let go: all of them are in the file.
    #[test]
    fn every_change_reaches_the_file() {
        let (_dir, path) = temp();
        let index = Index::start();
        index.hold(&path).expect("hold");

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

        index.let_go().expect("let go");

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

    /// The three ways an index is broken. Each is refused, each leaves the file
    /// exactly as it was, and the manager is left holding nothing.
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

        // One that cannot be written to, because the name a write needs is taken
        // by something that is not a file.
        let blocked = dir.path().join("blocked.sqlite");
        drop(db::open_and_migrate(&blocked).expect("make one"));
        std::fs::create_dir(db::being_written(&blocked)).expect("take the name");

        for (path, why) in [
            (&rubbish, "not a database"),
            (&ahead, "another schema version"),
            (&blocked, "a name a write needs"),
        ] {
            let was = std::fs::read(path).expect("read what is there");
            let index = Index::start();
            let refused = match index.hold(path) {
                Err(err) => err.to_string(),
                // An index that reads and cannot be written is found out when
                // the writing is waited for, which is where it is said.
                Ok(()) => index.let_go().expect_err("this was not refused").to_string(),
            };
            assert!(!refused.is_empty(), "{why} was refused without saying why");
            assert!(index.holding().is_none(), "{why} left the manager holding something");
            assert_eq!(std::fs::read(path).expect("read"), was, "{why} was written over: {refused}");
        }
    }

    /// A compaction whose write cannot finish leaves the index where it was
    /// rather than half of a new one or none of it.
    #[test]
    fn a_compaction_that_cannot_finish_leaves_the_index_where_it_was() {
        let (_dir, path) = temp();
        let index = Index::start();
        index.hold(&path).expect("hold");
        index.upsert(vec![row("a.jpg", 10), row("b.jpg", 20)], 1).expect("upsert");
        index.synced().expect("write it out");
        let was = std::fs::read(&path).expect("read the index");

        // The name a write goes to first, taken by something that is not a file,
        // so nothing can be written there.
        std::fs::create_dir(db::being_written(&path)).expect("take the name");

        let err = index.compact().expect_err("a compaction that cannot be written");
        assert!(err.to_string().contains("could not be written"), "{err}");
        assert_eq!(
            std::fs::read(&path).expect("read the index"),
            was,
            "a compaction that could not be written left the index changed"
        );
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
