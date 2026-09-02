use std::io::Write;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use imgdedupe_core::scan::{self, Event, Options};
use imgdedupe_core::{db, runlog};

/// Exit code for a pass that was asked to stop before it finished.
const EXIT_CANCELLED: u8 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum Progress {
    /// One JSON object per line on stdout, for a program to read.
    Json,
    /// A line a person can read.
    Text,
}

#[derive(Parser, Debug)]
#[command(
    name = "imgindex",
    about = "Build a duplicate-detection index for a folder of images",
    long_about = "Read every image in a folder and record what it looks like, in an \
                  index kept inside that folder.\n\n\
                  A second pass over the same folder only reads what has changed: files \
                  whose size and timestamp still match are left alone, and files that \
                  have gone are dropped from the index.\n\n\
                  This builds the index and nothing else. Finding the duplicates in it, \
                  and doing anything about them, is imgdedupe.",
    after_help = "Examples:\n  \
                  imgindex \"D:\\photos\"\n  \
                  imgindex \"D:\\photos\" --recurse\n  \
                  imgindex \"D:\\photos\" --db D:\\indexes\\photos.sqlite\n  \
                  imgindex \"D:\\photos\" --progress json",
    // With nothing to work on, say how to use it rather than complaining about
    // one missing argument.
    arg_required_else_help = true
)]
struct Args {
    /// Folder to index.
    folder: PathBuf,

    /// Descend into subfolders.
    #[arg(long)]
    recurse: bool,

    /// Where the index lives. Defaults to the folder being indexed.
    #[arg(long)]
    db: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = Progress::Text)]
    progress: Progress,

    /// Write what this run did to imgindex.log, beside this program.
    #[arg(long)]
    log: bool,
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.log {
        runlog::start("imgindex");
    }
    runlog::line(&format!(
        "indexing {} (recurse {})",
        args.folder.display(),
        args.recurse
    ));

    match run(args) {
        Ok(cancelled) => {
            runlog::line(if cancelled { "cancelled" } else { "finished" });
            if cancelled {
                ExitCode::from(EXIT_CANCELLED)
            } else {
                ExitCode::SUCCESS
            }
        }
        Err(err) => {
            runlog::line(&format!("ERROR {err:#}"));
            eprintln!("imgindex: {err:#}");
            ExitCode::FAILURE
        }
    }
}

/// Make a path absolute without resolving links.
///
/// `canonicalize` is the wrong tool here: it follows symlinks and reparse points
/// to whatever they currently point at, and on Windows rewrites the result as a
/// `\\?\` path. The link is the stable thing the user chose and the target is not,
/// so an index keyed on the resolved path would be keyed on something that can
/// move out from under it.
fn absolute(folder: &std::path::Path) -> Result<PathBuf> {
    if folder.is_absolute() {
        return Ok(folder.to_path_buf());
    }
    let current = std::env::current_dir().context("finding the working directory")?;
    Ok(current.join(folder))
}

#[cfg(test)]
mod tests {
    use super::absolute;

    /// The folder someone names is the folder that gets indexed. A link is a name
    /// they chose and the target is not, so nothing here may swap one for the
    /// other.
    #[test]
    fn making_a_path_absolute_leaves_a_link_alone() {
        let dir = tempfile::tempdir().expect("tempdir");
        let real = dir.path().join("real");
        std::fs::create_dir(&real).expect("mkdir");

        let link = dir.path().join("link");
        #[cfg(windows)]
        let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
        #[cfg(unix)]
        let made = std::os::unix::fs::symlink(&real, &link).is_ok();
        if !made {
            // Windows needs developer mode or elevation to create one.
            return;
        }

        assert_eq!(absolute(&link).expect("absolute"), link);
    }

    #[test]
    fn a_relative_path_is_joined_to_where_the_program_was_run() {
        let here = std::env::current_dir().expect("cwd");
        assert_eq!(
            absolute(std::path::Path::new("pictures")).expect("absolute"),
            here.join("pictures")
        );
    }
}

fn run(args: Args) -> Result<bool> {
    if !args.folder.is_dir() {
        anyhow::bail!("{} is not a folder", args.folder.display());
    }
    let root = absolute(&args.folder)?;
    let db_path = args.db.unwrap_or_else(|| root.join(db::INDEX_FILENAME));

    let cancel = Arc::new(AtomicBool::new(false));
    let handler_flag = Arc::clone(&cancel);
    // A second interrupt is left to the default handler, so a wedged pass can
    // still be stopped the usual way.
    let _ = ctrlc::set_handler(move || handler_flag.store(true, Ordering::Relaxed));

    let mut conn = db::open(&db_path)?;
    let options = Options { root, db_path, recurse: args.recurse };

    let report = |event: Event| emit(args.progress, &event);
    let summary = scan::run(&mut conn, &options, &cancel, &report)?;

    Ok(summary.cancelled)
}

fn emit(progress: Progress, event: &Event) {
    match event {
        // Progress is every two hundred files and would bury the log.
        Event::Progress { .. } | Event::Writing { .. } => {}
        Event::Error { path, message } => runlog::line(&format!("failed {path}: {message}")),
        other => runlog::line(&format!("{other:?}")),
    }

    let mut out = std::io::stdout().lock();
    let line = match progress {
        Progress::Json => serde_json::to_string(event).unwrap_or_else(|_| String::from("{}")),
        Progress::Text => match event {
            Event::Start { total } => format!("indexing {total} files"),
            Event::Progress { done, per_sec, .. } => format!("{done} done, {per_sec}/s"),
            Event::Writing { done, total } => format!("written {done} of {total}"),
            Event::Error { path, message } => format!("skipped {path}: {message}"),
            Event::Done { indexed, removed, failed, elapsed_ms } => format!(
                "indexed {indexed}, removed {removed}, failed {failed}, in {:.1}s",
                *elapsed_ms as f64 / 1000.0
            ),
        },
    };
    let _ = writeln!(out, "{line}");
    let _ = out.flush();
}
