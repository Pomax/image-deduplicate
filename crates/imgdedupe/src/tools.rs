use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use imgdedupe_core::cleanup::{self, Disposal, Plan};
use imgdedupe_core::db;
use imgdedupe_core::matching::{self, DuplicateSet, Thresholds};
use imgdedupe_core::runlog;

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
    #[arg(long)]
    log: bool,
}

/// Read the command line, and either do what it asks or open the window.
pub fn start() -> Result<()> {
    let args = Args::parse();
    if args.log {
        runlog::start("imgdedupe");
    }
    match (&args.report, &args.clean) {
        (Some(folder), _) => report(folder.clone(), &args),
        (_, Some(folder)) => clean(folder.clone(), &args),
        _ => crate::app::launch(),
    }
}

fn thresholds(args: &Args) -> Thresholds {
    args.strictness.resolve(args.sensitivity, args.ignore_colour)
}

fn report(folder: PathBuf, args: &Args) -> Result<()> {
    let db_path = args.db.clone().unwrap_or_else(|| headless::default_db_path(&folder));
    let conn = headless::open_index(&db_path)?;
    let sets = find_sets(&conn, thresholds(args))?;
    let text = match args.format {
        ReportFormat::Json => report_json(&sets),
        ReportFormat::Csv => report_csv(&sets),
    };
    println!("{text}");
    Ok(())
}

fn clean(folder: PathBuf, args: &Args) -> Result<()> {
    let db_path = args.db.clone().unwrap_or_else(|| headless::default_db_path(&folder));
    let conn = headless::open_index(&db_path)?;
    let sets = find_sets(&conn, thresholds(args))?;
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
    print!("{}", apply(&folder, &plan, &disposal, &db_path)?);
    Ok(())
}

fn find_sets(conn: &db::Connection, thresholds: Thresholds) -> Result<Vec<DuplicateSet>> {
    let mut sets = matching::find_sets(conn, thresholds)?;
    // Largest reclaim first, which is the order a person wants to work in.
    sets.sort_by_key(|set| std::cmp::Reverse(set.recoverable_bytes()));
    Ok(sets)
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
    cleanup::plan_from_sets(sets.iter().map(|set| set.members.as_slice()))
}

fn describe(plan: &Plan) -> String {
    format!(
        "{} files, {:.1} MB\n{}",
        plan.files(),
        plan.bytes() as f64 / 1_000_000.0,
        plan.to_text()
    )
}

fn apply(root: &Path, plan: &Plan, disposal: &Disposal, db_path: &Path) -> Result<String> {
    let outcome = cleanup::apply(root, plan, disposal).context("carrying out the plan")?;
    let mut out = format!(
        "removed {} files, freed {:.1} MB\n",
        outcome.removed.len(),
        outcome.bytes_freed as f64 / 1_000_000.0
    );
    for (path, message) in &outcome.failed {
        out.push_str(&format!("failed {path}: {message}\n"));
    }

    let forgotten = forget(db_path, &outcome.removed)?;
    out.push_str(&format!("dropped {forgotten} rows from the index\n"));
    Ok(out)
}

/// Take the removed files out of the index. Whether they went to the recycle bin,
/// to another folder or nowhere, they are not at those paths any more, and an
/// index that still lists them offers duplicates of files that are gone.
fn forget(db_path: &Path, removed: &[String]) -> Result<usize> {
    if removed.is_empty() {
        return Ok(0);
    }
    let mut conn = db::open_for_notes(db_path)?;
    let tx = conn.transaction()?;
    let dropped = db::delete_paths(&tx, removed)?;
    tx.commit()?;
    drop(conn);

    // Rebuilding costs a copy of the whole index, so it happens here and only
    // here: a cleanup is the one thing that leaves enough behind to be worth it,
    // and only when it actually dropped rows.
    if dropped > 0 {
        db::compact(db_path)?;
    }
    Ok(dropped)
}

#[cfg(test)]
mod tests {
    use super::*;
    use imgdedupe_core::matching::Member;

    fn member(id: i64, path: &str, keep: bool, size: i64) -> Member {
        Member {
            file_id: id,
            rel_path: path.to_string(),
            width: 800,
            height: 600,
            format: "jpeg".to_string(),
            channels: 3,
            size_bytes: size,
            mtime_ns: 1,
            auto_keep: keep,
        }
    }

    fn sets() -> Vec<DuplicateSet> {
        vec![DuplicateSet {
            set_id: 1,
            members: vec![
                member(1, "big.jpg", true, 500),
                member(2, "small, odd.jpg", false, 100),
            ],
        }]
    }

    #[test]
    fn the_json_report_names_the_keeper() {
        let text = report_json(&sets());
        let parsed: serde_json::Value = serde_json::from_str(&text).expect("valid JSON");
        assert_eq!(parsed[0]["recoverable_bytes"], 100);
        assert_eq!(parsed[0]["members"][0]["keep"], true);
        assert_eq!(parsed[0]["members"][1]["keep"], false);
    }

    #[test]
    fn the_csv_report_has_a_header_and_quotes_commas() {
        let text = report_csv(&sets());
        let lines: Vec<&str> = text.lines().collect();
        assert!(lines[0].starts_with("set_id,keep,path"));
        assert!(lines[1].contains("keep,big.jpg"));
        assert!(lines[2].contains("\"small, odd.jpg\""), "{}", lines[2]);
    }

    #[test]
    fn the_plan_covers_everything_but_the_keeper() {
        let plan = plan_from(&sets());
        assert_eq!(plan.files(), 1);
        assert_eq!(plan.bytes(), 100);
        assert!(describe(&plan).contains("small, odd.jpg"));
    }

    /// The removed files are gone from the index because they are gone from those
    /// paths. The one that was kept is still there and must still be indexed, or
    /// the next pass has to read it again for no reason.
    #[test]
    fn cleaning_up_forgets_the_removed_files_and_only_those() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("index.sqlite");
        let conn = db::open(&db_path).expect("an index");
        for path in ["big.jpg", "small, odd.jpg", "elsewhere.jpg"] {
            conn.execute(
                "INSERT INTO files(rel_path, size_bytes, mtime_ns, last_scanned_at)
                 VALUES (?1, 1, 1, 1)",
                [path],
            )
            .expect("insert");
        }
        drop(conn);

        let dropped = forget(&db_path, &[String::from("small, odd.jpg")]).expect("forget");
        assert_eq!(dropped, 1);

        let conn = headless::open_index(&db_path).expect("reopen");
        let mut left: Vec<String> = conn
            .prepare("SELECT rel_path FROM files")
            .expect("prepare")
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows");
        left.sort();
        assert_eq!(left, vec!["big.jpg".to_string(), "elsewhere.jpg".to_string()]);
    }

    #[test]
    fn a_cleanup_that_removed_nothing_touches_no_rows() {
        let dir = tempfile::tempdir().expect("tempdir");
        let db_path = dir.path().join("index.sqlite");
        db::open(&db_path).expect("an index");
        assert_eq!(forget(&db_path, &[]).expect("forget"), 0);
    }

    /// Neither flag means the window opens, and no flag is the help flag.
    #[test]
    fn no_flags_asks_for_neither_report_nor_clean() {
        let args = Args::try_parse_from(["imgdedupe"]).expect("parse");
        assert!(args.report.is_none());
        assert!(args.clean.is_none());
        assert!(Args::try_parse_from(["imgdedupe", "--help"]).is_err());
    }

    #[test]
    fn report_and_clean_each_take_the_folder_and_cannot_be_combined() {
        let args = Args::try_parse_from(["imgdedupe", "--report", "/photos"]).expect("parse");
        assert_eq!(args.report.as_deref(), Some(Path::new("/photos")));

        let args = Args::try_parse_from(["imgdedupe", "--clean", "/photos"]).expect("parse");
        assert_eq!(args.clean.as_deref(), Some(Path::new("/photos")));

        assert!(
            Args::try_parse_from(["imgdedupe", "--report", "/a", "--clean", "/a"]).is_err(),
            "asking for both at once has no meaning"
        );
    }
}
