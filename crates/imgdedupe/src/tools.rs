use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use imgdedupe_core::cleanup::{self, Disposal, Plan};
use imgdedupe_core::index::Index;
use imgdedupe_core::matching::{DuplicateSet, Thresholds};

use crate::headless;
use crate::Strictness;

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum ReportFormat {
    Json,
    Csv,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Destination {
    /// The operating system's recycle bin.
    Trash,
    /// A folder, keeping the relative paths.
    Move,
    /// Unlink. Not recoverable.
    Delete,
}

#[derive(Parser, Debug)]
#[command(name = "imgdedupe", disable_help_flag = true)]
struct Args {
    /// Print the duplicate sets the index for this folder holds.
    #[arg(long, value_name = "FOLDER")]
    report: Option<PathBuf>,

    /// Show what the automatic pick would remove from the index for this folder.
    #[arg(long, value_name = "FOLDER", conflicts_with = "report")]
    clean: Option<PathBuf>,

    /// The index to read, when it is not the one in the folder.
    #[arg(long)]
    db: Option<PathBuf>,

    #[arg(long, value_enum, default_value_t = ReportFormat::Json)]
    format: ReportFormat,

    #[arg(long, value_enum, default_value_t = Strictness::Balanced)]
    strictness: Strictness,

    /// How different two pictures may be and still count as the same one, as a
    /// percentage. Overrides --strictness.
    #[arg(long)]
    sensitivity: Option<f64>,

    /// Treat a colourised copy and its grayscale original as duplicates.
    #[arg(long)]
    ignore_colour: bool,

    /// Carry out what --clean lists, rather than only listing it.
    #[arg(long)]
    apply: bool,

    #[arg(long, value_enum, default_value_t = Destination::Trash)]
    to: Destination,

    /// Folder to move files into when `--to move` is used.
    #[arg(long)]
    move_dir: Option<PathBuf>,

    /// Write what this run did to imgdedupe.log, beside this program.
    #[cfg(feature = "logging")]
    #[arg(long)]
    log: bool,
}

/// Read the command line, and either do what it asks or open the window.
pub fn start() -> Result<()> {
    let args = Args::parse();
    #[cfg(feature = "logging")]
    if args.log {
        imgdedupe_core::runlog::start("imgdedupe");
    }
    match (&args.report, &args.clean) {
        (Some(folder), _) => report(folder.clone(), &args),
        (_, Some(folder)) => clean(folder.clone(), &args),
        _ => crate::app::launch(),
    }
}

fn thresholds(args: &Args) -> Thresholds {
    args.strictness
        .resolve(args.sensitivity, args.ignore_colour)
}

fn report(folder: PathBuf, args: &Args) -> Result<()> {
    let db_path = args
        .db
        .clone()
        .unwrap_or_else(|| headless::default_db_path(&folder));
    let index = headless::open_index(&db_path)?;
    let sets = sets_of(&index, thresholds(args))?;
    let text = match args.format {
        ReportFormat::Json => report_json(&sets),
        ReportFormat::Csv => report_csv(&sets),
    };
    println!("{text}");
    Ok(())
}

fn clean(folder: PathBuf, args: &Args) -> Result<()> {
    let db_path = args
        .db
        .clone()
        .unwrap_or_else(|| headless::default_db_path(&folder));
    let index = headless::open_index(&db_path)?;
    let sets = sets_of(&index, thresholds(args))?;
    let plan = plan_from(&sets);

    if !args.apply {
        print!("{}", describe(&plan));
        println!("\nnothing was removed. Pass --apply to carry this out.");
        return Ok(());
    }

    let disposal = match args.to {
        Destination::Trash => Disposal::Trash,
        Destination::Delete => Disposal::Delete,
        Destination::Move => Disposal::MoveTo(
            args.move_dir
                .clone()
                .ok_or_else(|| anyhow::anyhow!("--to move needs --move-dir"))?,
        ),
    };
    print!("{}", apply(&folder, &plan, &disposal, &index)?);
    Ok(())
}

fn report_json(sets: &[DuplicateSet]) -> String {
    let value: Vec<serde_json::Value> = sets
        .iter()
        .map(|set| {
            serde_json::json!({
                "set_id": set.set_id,
                "recoverable_bytes": set.recoverable_bytes(),
                "members": set.members.iter().map(|member| serde_json::json!({
                    "file_id": member.file_id,
                    "path": member.rel_path,
                    "width": member.width,
                    "height": member.height,
                    "format": member.format,
                    "size_bytes": member.size_bytes,
                    "keep": member.auto_keep,
                })).collect::<Vec<_>>(),
            })
        })
        .collect();
    serde_json::to_string_pretty(&value).unwrap_or_else(|_| String::from("[]"))
}

fn report_csv(sets: &[DuplicateSet]) -> String {
    let mut out = String::from("set_id,keep,path,width,height,format,size_bytes\n");
    for set in sets {
        for member in &set.members {
            out.push_str(&format!(
                "{},{},{},{},{},{},{}\n",
                set.set_id,
                if member.auto_keep { "keep" } else { "remove" },
                csv_field(&member.rel_path),
                member.width,
                member.height,
                member.format,
                member.size_bytes
            ));
        }
    }
    out
}

fn csv_field(value: &str) -> String {
    if value.contains(',') || value.contains('"') || value.contains('\n') {
        format!("\"{}\"", value.replace('"', "\"\""))
    } else {
        value.to_string()
    }
}

/// Everything the automatic pick would remove, across every set.
fn plan_from(sets: &[DuplicateSet]) -> Plan {
    // Nobody is here to mark anything, so what the search would keep is what is
    // kept: the best copy in each set, and the rest of the set goes.
    let kept: Vec<Vec<i64>> = sets
        .iter()
        .map(|set| {
            set.members
                .iter()
                .filter(|m| m.auto_keep)
                .map(|m| m.file_id)
                .collect()
        })
        .collect();
    cleanup::plan_from_sets(
        sets.iter()
            .zip(&kept)
            .map(|(set, kept)| (set.members.as_slice(), kept.as_slice())),
    )
}

fn describe(plan: &Plan) -> String {
    format!(
        "{} files, {:.1} MB\n{}",
        plan.files(),
        plan.bytes() as f64 / 1_000_000.0,
        plan.to_text()
    )
}

fn apply(root: &Path, plan: &Plan, disposal: &Disposal, index: &Index) -> Result<String> {
    let outcome = cleanup::apply(root, plan, disposal).context("carrying out the plan")?;
    let mut out = format!(
        "removed {} files, freed {:.1} MB\n",
        outcome.removed.len(),
        outcome.bytes_freed as f64 / 1_000_000.0
    );
    for (path, message) in &outcome.failed {
        out.push_str(&format!("failed {path}: {message}\n"));
    }

    let forgotten = forget(index, &outcome.removed)?;
    out.push_str(&format!("dropped {forgotten} rows from the index\n"));
    Ok(out)
}

/// Take the removed files out of the index. Whether they went to the recycle bin,
/// to another folder or nowhere, they are not at those paths any more, and an
/// index that still lists them offers duplicates of files that are gone.
fn forget(index: &Index, removed: &[String]) -> Result<usize> {
    if removed.is_empty() {
        return Ok(0);
    }
    let dropped = index.delete_paths(removed.to_vec())?;

    // Rebuilding costs a rewrite of the whole index, so it happens here and only
    // here: a cleanup is the one thing that leaves enough behind to be worth it,
    // and only when it actually dropped rows.
    if dropped > 0 {
        index.compact()?;
    }
    Ok(dropped)
}

/// Search whatever the manager is holding.
fn sets_of(index: &Index, thresholds: Thresholds) -> Result<Vec<DuplicateSet>> {
    let found = index.find_sets(
        thresholds,
        std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
        std::sync::Arc::new(|_| {}),
    )?;
    let mut sets = found.unwrap_or_default();
    // Largest reclaim first, which is the order a person wants to work in.
    sets.sort_by_key(|set| std::cmp::Reverse(set.recoverable_bytes()));
    Ok(sets)
}

#[cfg(test)]
#[path = "tests/tools.rs"]
mod tests;
