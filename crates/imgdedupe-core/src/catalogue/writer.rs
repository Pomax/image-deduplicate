use super::*;

/// The thread that makes on disk the changes the manager has made in memory.
///
/// It holds a connection to the catalogue's file and applies each change with a
/// statement, so a change of one row writes one row. The manager is free for the
/// next job as soon as it has sent the change.
pub(super) struct Writer {
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
    pub(super) fn start() -> Writer {
        let (to, errands) = channel::<Errand>();
        std::thread::Builder::new()
            .name(String::from("index writer"))
            .spawn(move || {
                // The catalogue's file, once the manager has opened one.
                let mut file: Option<Connection> = None;
                // What went wrong since anything last asked. A caller waiting on
                // the file is told, so a catalogue is never closed in the belief
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

    /// Open the file this catalogue lives in, and say if it could not be opened.
    pub(super) fn open(&self, path: &Path) -> Result<()> {
        let path = path.to_path_buf();
        self.wait_on(|back| Errand::Open(path, back))
    }

    /// Send a change to make to the file. Nothing waits for it: the caller is
    /// answered when the manager has the change, and `drain` is what waits for
    /// the disk.
    pub(super) fn apply(&self, change: Change) {
        let _ = self.to.send(Errand::Do(change));
    }

    /// Close the file, once everything sent has been written to it.
    pub(super) fn close(&self) -> Result<()> {
        self.wait_on(Errand::Close)
    }

    /// Wait until everything sent so far is on disk, and say what stopped any of
    /// it getting there.
    pub(super) fn drain(&self) -> Result<()> {
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
