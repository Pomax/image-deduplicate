# The index file is written through, not replaced

The manager reads a folder's index into memory and runs every query against that
copy. A search over a network folder therefore reads the file once instead of
several thousand times. That part stays.

Writing does not work that way and should. `sync` calls `conn.serialize` to get
the whole database back as bytes, sends them to the writer thread, and the writer
writes `imgdedupe.sqlite.writing` and renames it over the index. Changing one row
therefore copies the entire database and writes the entire file: 115 MB on the
folder this was measured on, over the network, for one mark.

SQLite is the permanent store. A change is applied to the file with a statement,
in a transaction, on a connection to that file. The copy in memory is what
computation runs against.

## What stays

- Opening still reads the file in one go and calls `deserialize`. There are
  several thousand reads in a search and one write per change.
- The migration still runs against the file before anything is read from it, so
  the file and the copy in memory start the same.
- Writing still happens on its own thread, so a caller is answered when the
  manager has the change rather than when the disk does.
- `drain` still answers when everything sent before it has been written, and
  still reports whatever failed.

## Tasks

- [ ] 1. The writer holds a connection to the index file
- [ ] 2. Add an errand that applies one change to that connection
- [ ] 3. Send every change from the manager to the writer
- [ ] 4. Delete the whole-file write
- [ ] 5. Fix closing an index, compacting it and deleting it
- [ ] 6. Rename opening and closing an index to say so
- [ ] 7. Delete the two tests for the whole-file write and write what replaces
      them
- [ ] 8. Stop treating a live rollback journal as a broken index
- [ ] 9. Named tests for each of the above
- [ ] 10. Update `docs/tests.md`
- [ ] 11. Run the named tests, then build

## 1. The writer holds a connection to the index file

In `crates/imgdedupe-core/src/index.rs`, in the thread started by
`Writer::start`.

- A `rusqlite::Connection` to the index the manager has open, opened when the
  manager opens that index and closed when it closes it.
- `PRAGMA foreign_keys = ON` on it, as `db::open_and_migrate` sets on the
  connection in memory. Without it, deleting a file removes its ignored pairs in
  memory but leaves them in the file.
- One index at a time, the same as the manager.

## 2. Add an errand that applies one change to that connection

In `crates/imgdedupe-core/src/index.rs`.

- Add the `Errand` variant `Do(Box<dyn FnOnce(&Connection) -> Result<()> +
  Send>)`. Remove `Put`.
- Add the `Errand` variants `Open(PathBuf, Sender<Option<String>>)` and
  `Close(Sender<Option<String>>)`. The manager sends `Open` when it opens an
  index and `Close` when it closes one, and waits for both: a file that cannot
  be opened for writing is a broken index and the caller has to be told, the
  same as one that cannot be read.
- `Do` is not waited for. The caller is answered when the manager has the change,
  and `drain` is what waits for the disk.
- The writer runs the closure against its connection and keeps any error for the
  next `drain`, which is what it does with a failed write now.
- The manager sends the same call it just made in memory. Every `db::` function
  takes `&Connection`, so the closure calls the same function with the writer's
  connection.

The change is written once as code and run twice, against two connections, so
the file and the copy in memory cannot end up different.

## 3. Send every change from the manager to the writer

In `crates/imgdedupe-core/src/index.rs`, in `serve`. Each of these makes its
change in memory, answers the caller, and sends the same call to the writer
instead of calling `sync`:

- `Job::SetMeta`, `Job::ForgetMeta`
- `Job::Ignore`, `Job::Unignore`
- `Job::Upsert`. A pass sends thousands of records through it. They go as one
  errand containing one transaction, so the file gets one commit rather than one
  per picture.
- `Job::DeletePaths`, in one transaction for the same reason.

## 4. Delete the whole-file write

In `crates/imgdedupe-core/src/index.rs`, delete the function `sync`, the `Errand`
variant `Put`, the method `Writer::put` and the function `write_file`. In
`crates/imgdedupe-core/src/db.rs`, delete the function `being_written` if nothing
else calls it.

Nothing writes `imgdedupe.sqlite.writing` after that. It existed because the
index was replaced rather than written to. A scan skips it so that it is not
counted as a picture; check whether anything else still creates it before taking
it out of the list a scan skips.

## 5. Fix closing an index, compacting it and deleting it

In `crates/imgdedupe-core/src/index.rs`.

- `put_down`, which closes the index, drains and closes the writer's connection.
  It no longer writes anything out first, because every change was written when
  it was made.
- `take_up`, which opens an index, opens the writer's connection on the new path.
- `compact` runs `VACUUM` in memory and sends one to the file as well. A `VACUUM`
  in memory does not change the size of the file, and a smaller file is what the
  caller asked for.
- `delete` drains, closes the writer's connection, and then removes the files. A
  connection left open on a file that is removed leaves a `-journal` beside it.

## 6. Rename opening and closing an index to say so

The manager's names for opening and closing an index do not say what they do.
Rename them across `crates/imgdedupe-core/src/index.rs` and the seven call sites
in `crates/imgdedupe/src`:

- the function `take_up` to `open_index`, and `put_down` to `close_index`
- the method `Index::hold` to `Index::open`, `Index::let_go` to `Index::close`,
  and `Index::holding` to `Index::open_index_path`
- the `Job` variants `Hold` to `Open`, `LetGo` to `Close`, and `Held` to
  `OpenIndexPath`
- the struct `Held` to `OpenIndex`, and the variable `held` in `serve` to
  `current_open_index`

## 7. Delete the two tests for the whole-file write and write what replaces them

Two tests in `crates/imgdedupe-core/src/index.rs` are about the whole-file write
and stop meaning anything once it is gone. Both make a write fail by creating a
directory named `imgdedupe.sqlite.writing`, so the file a write went to first
could not be written, and then check that the index was left as it was, which
was the point of writing beside the file and renaming.

- `a_compaction_that_cannot_finish_leaves_the_index_where_it_was` goes. A
  compaction is now a `VACUUM` on the file, and SQLite either finishes it or
  rolls it back; there is no half-written index to check for.
- The `blocked` case inside
  `an_index_that_cannot_be_read_or_written_is_refused_and_left_alone` goes with
  it, along with the two lines that set it up. The other two cases in that test,
  an index that is not a database and one written under a later schema version,
  are about reading and stay as they are.

Nothing replaces them. Both existed to test what happened when the temporary file
a write went to could not be written, and there is no such file. Nothing makes
the index read-only or otherwise unwritable, so there is no failure here worth
staging: the index is opened normally, and a write that fails is a disk or a
network that failed, which `drain` already reports.

What is left covering this is task 8: the changes reach the file, and they are
there when it is opened again.

## 8. Stop treating a live rollback journal as a broken index

`db::migrate_the_file` in `crates/imgdedupe-core/src/db.rs` refuses an index that
has a non-empty `imgdedupe.sqlite-journal` beside it, on the grounds that it was
left part way through a write.

That held when nothing kept the file open: a journal left behind meant a run that
died. It does not hold now. The writer keeps a connection on the file, so a
journal exists for as long as each transaction takes, and a second manager
opening the same folder in that moment reads a working index as a broken one.

The check goes. What it was for, an index left half written by a run that died,
is SQLite's own job: opening the file rolls the journal back. The refusal for an
index that is not a database, and the one for a schema version this build does
not speak, both stay.

The line at the end of the same function that deletes the journal goes with it,
for the same reason twice over. SQLite removes its own journal when it commits,
and with a connection live on the file that line can delete a journal another
manager is in the middle of using.

## 9. Named tests for each of the above

- A change made through the manager is in the file once `drain` has answered,
  checked by opening the file with a second connection and reading the row.
- A change does not rewrite the whole file: the file's modification time and
  size are what a one-row write leaves, not what a 115 MB rewrite leaves.
- A pass of many records reaches the file in one commit.
- Deleting a file removes its ignored pairs from the file, not only from memory.
- Closing an index and opening it again reads back every change written to it.
- `compact` makes the file smaller.

## 10. Update `docs/tests.md`

An entry for every test above, saying what it is for, and corrections to the
entries for the tests that described the whole-file write.

## 11. Run the named tests, then build

The suite, then `scripts\build.bat`.
