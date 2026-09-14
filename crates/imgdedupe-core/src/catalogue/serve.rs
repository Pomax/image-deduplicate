use super::*;

/// The manager's own loop. Owns the connection and the path; neither leaves it.
pub(super) fn serve(jobs: Receiver<Job>) {
    let mut current_catalogue: Option<OpenCatalogue> = None;
    let writer = Writer::start();

    while let Ok(job) = jobs.recv() {
        match job {
            Job::Open(path, back) => {
                let outcome = open_catalogue(&mut current_catalogue, &writer, &path);
                let _ = back.send(outcome);
            }
            Job::Close(back) => {
                let _ = back.send(close_catalogue(&mut current_catalogue, &writer));
            }
            Job::OpenCataloguePath(back) => {
                let _ = back.send(current_catalogue.as_ref().map(|it| it.path.clone()));
            }
            Job::Images(cancel, report, back) => {
                let _ = back.send(with(&current_catalogue, |it| {
                    matching::load_images(&it.conn, &cancel, &|progress| report(progress))
                }));
            }
            Job::FindSets(thresholds, cancel, report, back) => {
                let _ = back.send(with(&current_catalogue, |it| {
                    matching::find_sets_cancellable(&it.conn, thresholds, &cancel, &|progress| {
                        report(progress)
                    })
                }));
            }
            Job::Known(back) => {
                let _ = back.send(with(&current_catalogue, |it| db::load_known(&it.conn)));
            }
            Job::Ignored(back) => {
                let _ = back.send(with(&current_catalogue, |it| db::ignored(&it.conn)));
            }
            Job::Meta(key, back) => {
                let _ = back.send(with(&current_catalogue, |it| db::get_meta(&it.conn, &key)));
            }
            Job::SetMeta(key, value, back) => {
                let done = with(&current_catalogue, |it| {
                    db::set_meta(&it.conn, &key, &value)
                });
                let _ = back.send(done);
                writer.apply(Box::new(move |file| db::set_meta(file, &key, &value)));
            }
            Job::ForgetMeta(key, back) => {
                let done = with(&current_catalogue, |it| db::forget_meta(&it.conn, &key));
                let _ = back.send(done);
                writer.apply(Box::new(move |file| db::forget_meta(file, &key)));
            }
            Job::Ignore(pairs, back) => {
                let done = with(&current_catalogue, |it| db::ignore(&it.conn, &pairs));
                let _ = back.send(done);
                writer.apply(Box::new(move |file| db::ignore(file, &pairs)));
            }
            Job::Unignore(pairs, back) => {
                let done = with(&current_catalogue, |it| db::unignore(&it.conn, &pairs));
                let _ = back.send(done);
                writer.apply(Box::new(move |file| db::unignore(file, &pairs)));
            }
            Job::Kept(back) => {
                let _ = back.send(with(&current_catalogue, |it| db::kept(&it.conn)));
            }
            // These four are asked of a folder that may not be open yet: the
            // window writes a review as it happens, and a folder is chosen before
            // its catalogue has been opened. A change the copy in memory did not
            // make is not sent to the file, or the writer is handed work it has
            // nowhere to do and reports it as trouble.
            Job::BeginReview(back) => {
                let done = with(&current_catalogue, |it| db::begin_review(&it.conn));
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(db::begin_review));
                }
            }
            Job::KeepThese(file_ids, back) => {
                let done = with(&current_catalogue, |it| db::keep_these(&it.conn, &file_ids));
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(move |file| db::keep_these(file, &file_ids)));
                }
            }
            Job::UnkeepThese(file_ids, back) => {
                let done = with(&current_catalogue, |it| {
                    db::unkeep_these(&it.conn, &file_ids)
                });
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(move |file| db::unkeep_these(file, &file_ids)));
                }
            }
            Job::ClearKeep(back) => {
                let done = with(&current_catalogue, |it| db::clear_keep(&it.conn));
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(db::clear_keep));
                }
            }
            Job::StoredSets(back) => {
                let _ = back.send(with(&current_catalogue, |it| db::stored_sets(&it.conn)));
            }
            // Not somebody doing something: a search handing over everything it
            // found, which on a folder of any size is hundreds of rows. They go to
            // the file in one transaction, the way a pass's records do, so it is
            // one commit rather than one per picture per set.
            Job::StoreSets(sets, back) => {
                let done = with_mut(&mut current_catalogue, |it| {
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
                let done = with(&current_catalogue, |it| db::clear_sets(&it.conn));
                let made = done.is_ok();
                let _ = back.send(done);
                if made {
                    writer.apply(Box::new(db::clear_sets));
                }
            }
            Job::Upsert(records, scanned_at, back) => {
                let done = with_mut(&mut current_catalogue, |it| {
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
                let done = with_mut(&mut current_catalogue, |it| {
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
                let done = with_mut(&mut current_catalogue, |it| {
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
                let _ = back.send(compact(&mut current_catalogue, &writer));
            }
            Job::Delete(back) => {
                let _ = back.send(delete(&mut current_catalogue, &writer));
            }
            Job::Synced(back) => {
                let _ = back.send(writer.drain());
            }
        }
    }

    // The last way to ask has gone. Whatever is held goes to disk before the
    // thread does.
    let _ = close_catalogue(&mut current_catalogue, &writer);
}

/// Do something with the open catalogue, or say that none is open.
fn with<T>(
    current_catalogue: &Option<OpenCatalogue>,
    work: impl FnOnce(&OpenCatalogue) -> Result<T>,
) -> Result<T> {
    match current_catalogue {
        Some(it) => work(it),
        None => anyhow::bail!("no folder is open"),
    }
}

fn with_mut<T>(
    current_catalogue: &mut Option<OpenCatalogue>,
    work: impl FnOnce(&mut OpenCatalogue) -> Result<T>,
) -> Result<T> {
    match current_catalogue {
        Some(it) => work(it),
        None => anyhow::bail!("no folder is open"),
    }
}

/// Open a folder's catalogue: migrate the file, then read it in.
fn open_catalogue(
    current_catalogue: &mut Option<OpenCatalogue>,
    writer: &Writer,
    path: &Path,
) -> Result<()> {
    // Already open on this one. Closing it and opening it again reads the whole
    // file back for nothing. The window opens a folder and the pass asks for the
    // same folder a moment later, so this is the usual case, not a rare one.
    if current_catalogue
        .as_ref()
        .is_some_and(|it| it.path.as_path() == path)
    {
        return Ok(());
    }
    close_catalogue(current_catalogue, writer)?;
    #[cfg(feature = "logging")]
    let at = std::time::Instant::now();
    let conn = db::open_and_migrate(path)?;
    crate::log_line!("  open and migrate: {:.2}s", at.elapsed().as_secs_f64());
    // The file, for the thread that writes to it. Opening it is waited for: a
    // catalogue that cannot be written to is a broken catalogue and the caller is
    // told now rather than at the first change.
    writer.open(path)?;
    *current_catalogue = Some(OpenCatalogue {
        path: path.to_path_buf(),
        conn,
    });
    Ok(())
}

/// Close the catalogue, once the file has caught up with what was changed in it.
fn close_catalogue(current_catalogue: &mut Option<OpenCatalogue>, writer: &Writer) -> Result<()> {
    if current_catalogue.is_none() {
        return Ok(());
    }
    // Nothing to write out first: every change was written when it was made.
    let caught_up = writer.close();
    // Close it either way: keeping a catalogue open whose file cannot be written
    // gains nothing. The caller is told what went wrong.
    *current_catalogue = None;
    caught_up
}

fn compact(current_catalogue: &mut Option<OpenCatalogue>, writer: &Writer) -> Result<()> {
    // Tidied on this machine's own disk, never on the folder's.
    //
    // Tidying writes a database out a page at a time. The folder can be on
    // another machine, and done there that is the whole catalogue across the
    // network in small writes with a journal beside it: a minute on a
    // hundred-megabyte catalogue. On a local disk those pages cost nothing, and
    // what crosses the network afterwards is one finished file, written once.
    //
    // This is the one thing that replaces the catalogue's file rather than writing
    // changes into it, because it is the one thing that changes every byte of it.
    //
    // Written out by SQLite rather than built in memory first: the catalogue is
    // already held whole, a large folder's being hundreds of megabytes, and
    // taking the tidied database as bytes as well would be a second copy of it
    // beside the first.
    let path = {
        let it = current_catalogue.as_ref().context("no folder is open")?;
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
        let it = current_catalogue.as_ref().context("no folder is open")?;
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
    // Back over the catalogue's file, in one copy. If that fails the folder still
    // has the file it had: the tidied file is the copy, and nothing has been
    // taken away from the folder until this succeeds.
    let put_back = std::fs::copy(&local, &path)
        .with_context(|| format!("putting the tidied index back at {}", path.display()));
    let _ = std::fs::remove_file(&local);
    put_back?;
    writer.open(&path)
}

/// Remove the catalogue from the folder and close it.
fn delete(current_catalogue: &mut Option<OpenCatalogue>, writer: &Writer) -> Result<usize> {
    let (path, rows) = match current_catalogue {
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
    *current_catalogue = None;
    if std::fs::remove_file(&path).is_err() {
        anyhow::bail!("nothing was removed at {}", path.display());
    }
    Ok(rows)
}

/// The catalogue's file, as the thread that writes to it holds it.
pub(super) fn open_the_file(path: &Path) -> std::result::Result<Connection, String> {
    let conn = Connection::open(path)
        .map_err(|err| format!("the index at {} could not be opened: {err}", path.display()))?;
    // The same as the copy in memory is opened with. Without it, deleting a file
    // takes its rows elsewhere with it in memory and leaves them in the file.
    conn.pragma_update(None, "foreign_keys", "ON")
        .map_err(|err| format!("the index at {} refused a setting: {err}", path.display()))?;
    Ok(conn)
}
