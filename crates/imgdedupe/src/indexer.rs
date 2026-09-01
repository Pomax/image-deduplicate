use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::mpsc::{self, Receiver};

use anyhow::{Context, Result};
use imgdedupe_core::runlog;

/// What the GUI needs from one line of the indexer's output.
#[derive(Debug, Clone)]
pub enum Update {
    Start { total: u64 },
    Progress { done: u64, per_sec: u64, unchanged: u64, removed: u64 },
    /// Rows committed. The writer runs behind the readers and is still going
    /// after the last file has been decoded.
    Writing { done: u64, total: u64 },
    Failed { path: String, message: String },
    Done { indexed: u64, removed: u64, failed: u64, elapsed_ms: u64 },
    /// The process ended. Any exit code other than success is a problem to show.
    Exited { code: Option<i32> },
}

/// A running indexing pass. It does not outlive this: dropping it kills the
/// process, the same as cancelling does.
pub struct Run {
    child: Child,
    pub updates: Receiver<Update>,
}

impl Run {
    /// Killing the process is how a pass is cancelled. Commits are batched, so
    /// the index is consistent at whatever batch finished.
    pub fn cancel(&mut self) {
        runlog::line("cancelling: killing the indexer");
        let _ = self.child.kill();
    }
}

impl Drop for Run {
    /// A pass left running after the window has gone holds the index open, and
    /// the next run of the application cannot write to it. Closing the window
    /// while a scan is going has to take the scan with it.
    fn drop(&mut self) {
        match self.child.try_wait() {
            Ok(Some(_)) => {}
            _ => {
                runlog::line("the indexer is still running and is being stopped");
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }
    }
}

/// The indexer ships next to the GUI, so look there first and fall back to the
/// path for a development build.
pub fn binary_path() -> PathBuf {
    let name = if cfg!(windows) { "imgindex.exe" } else { "imgindex" };
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let beside = dir.join(name);
            if beside.exists() {
                return beside;
            }
        }
    }
    PathBuf::from(name)
}

pub fn start(root: &Path, db_path: &Path, recurse: bool) -> Result<Run> {
    let mut command = Command::new(binary_path());
    command
        .arg(root)
        .arg("--db")
        .arg(db_path)
        .arg("--progress")
        .arg("json")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if recurse {
        command.arg("--recurse");
    }
    // Only when this program is logging. Otherwise the child would leave a file
    // behind that the run it belongs to did not ask for.
    if runlog::is_on() {
        command.arg("--log");
    }
    #[cfg(windows)]
    {
        // `imgindex` is a console program and this one is not, so Windows gives
        // the child its own console window. Its output goes down a pipe, so that
        // window would only ever be empty.
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(CREATE_NO_WINDOW);
    }

    runlog::line(&format!(
        "spawning {} over {} (recurse {recurse}, db {})",
        binary_path().display(),
        root.display(),
        db_path.display()
    ));

    let mut child = command
        .spawn()
        .with_context(|| format!("starting {}", binary_path().display()))?;
    let stdout = child.stdout.take().context("the indexer gave no stdout")?;
    let stderr = child.stderr.take().context("the indexer gave no stderr")?;

    let (send, updates) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
            match parse(&line) {
                Some(update) => {
                    if send.send(update).is_err() {
                        return;
                    }
                }
                None => runlog::line(&format!("indexer said something unexpected: {line}")),
            }
        }
    });

    // This has to be drained. A pipe nobody reads fills up, and then the indexer
    // blocks forever on its next write and the scan stops with no error anywhere.
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            runlog::line(&format!("indexer stderr: {line}"));
        }
    });

    Ok(Run { child, updates })
}

/// Poll for the exit code without blocking the frame.
pub fn poll_exit(run: &mut Run) -> Option<Update> {
    match run.child.try_wait() {
        Ok(Some(status)) => Some(Update::Exited { code: status.code() }),
        _ => None,
    }
}

fn parse(line: &str) -> Option<Update> {
    let value: serde_json::Value = serde_json::from_str(line).ok()?;
    let number = |key: &str| value[key].as_u64().unwrap_or(0);
    match value["event"].as_str()? {
        "start" => Some(Update::Start { total: number("total") }),
        "progress" => Some(Update::Progress {
            done: number("done"),
            per_sec: number("per_sec"),
            unchanged: number("unchanged"),
            removed: number("removed"),
        }),
        "writing" => Some(Update::Writing { done: number("done"), total: number("total") }),
        "error" => Some(Update::Failed {
            path: value["path"].as_str().unwrap_or_default().to_string(),
            message: value["message"].as_str().unwrap_or_default().to_string(),
        }),
        "done" => Some(Update::Done {
            indexed: number("indexed"),
            removed: number("removed"),
            failed: number("failed"),
            elapsed_ms: number("elapsed_ms"),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_event_the_indexer_writes_is_understood() {
        assert!(matches!(
            parse(r#"{"event":"start","total":12}"#),
            Some(Update::Start { total: 12 })
        ));
        assert!(matches!(
            parse(r#"{"event":"progress","done":5,"per_sec":100,"unchanged":2,"removed":1}"#),
            Some(Update::Progress { done: 5, per_sec: 100, unchanged: 2, removed: 1 })
        ));
        assert!(matches!(
            parse(r#"{"event":"writing","done":4,"total":9}"#),
            Some(Update::Writing { done: 4, total: 9 })
        ));
        assert!(matches!(
            parse(r#"{"event":"done","indexed":3,"removed":0,"failed":1,"elapsed_ms":900}"#),
            Some(Update::Done { indexed: 3, failed: 1, .. })
        ));
        match parse(r#"{"event":"error","path":"a/b.png","message":"truncated"}"#) {
            Some(Update::Failed { path, message }) => {
                assert_eq!(path, "a/b.png");
                assert_eq!(message, "truncated");
            }
            other => panic!("expected a failure, got {other:?}"),
        }
    }

    #[test]
    fn a_line_that_is_not_an_event_is_ignored_rather_than_fatal() {
        assert!(parse("").is_none());
        assert!(parse("not json at all").is_none());
        assert!(parse(r#"{"event":"something_new"}"#).is_none());
        assert!(parse(r#"{"no_event_key":1}"#).is_none());
    }

    #[test]
    fn a_progress_event_missing_fields_reads_as_zero_rather_than_failing() {
        match parse(r#"{"event":"progress","done":7}"#) {
            Some(Update::Progress { done, per_sec, .. }) => {
                assert_eq!(done, 7);
                assert_eq!(per_sec, 0);
            }
            other => panic!("expected progress, got {other:?}"),
        }
    }

    /// A pass that outlives the window that started it keeps the index open, and
    /// the next run of the application cannot write to it: it exits with
    /// "database is locked". Dropping the run has to take the process with it.
    #[test]
    fn dropping_a_run_stops_the_process_it_started() {
        // A process that outlasts the test rather than the real indexer, which is
        // not beside the test binary and would have made this check nothing.
        let mut command = Command::new(if cfg!(windows) { "cmd" } else { "sh" });
        if cfg!(windows) {
            command.args(["/c", "ping -n 60 127.0.0.1 > NUL"]);
        } else {
            command.args(["-c", "sleep 60"]);
        }
        let child = command.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().expect("spawn");

        let (_send, updates) = mpsc::channel();
        let run = Run { child, updates };
        let id = run.child.id();
        assert!(process_is_running(id), "the fixture died before the test could run");

        drop(run);
        assert!(
            !process_is_running(id),
            "the process is still running after the run that owns it was dropped"
        );
    }

    #[cfg(windows)]
    fn process_is_running(id: u32) -> bool {
        let listed = Command::new("tasklist")
            .args(["/fi", &format!("PID eq {id}"), "/nh"])
            .output()
            .expect("tasklist");
        String::from_utf8_lossy(&listed.stdout).contains(&id.to_string())
    }

    #[cfg(not(windows))]
    fn process_is_running(id: u32) -> bool {
        std::path::Path::new(&format!("/proc/{id}")).exists()
    }

    #[test]
    fn the_indexer_is_looked_for_beside_the_gui() {
        let path = binary_path();
        let name = path.file_name().and_then(|name| name.to_str()).unwrap_or_default();
        assert!(name.starts_with("imgindex"), "looked for {name}");
    }
}
