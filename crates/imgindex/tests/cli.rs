use std::path::Path;
use std::process::{Command, Output};

use image::{DynamicImage, RgbImage};
use serde_json::Value;

fn write_image(path: &Path, width: u32, height: u32, seed: u32) {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([((x * 3 + seed) % 256) as u8, ((y * 5) % 256) as u8, 80])
    });
    DynamicImage::ImageRgb8(image)
        .save_with_format(path, image::ImageFormat::Png)
        .expect("writing a fixture");
}

fn run(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_imgindex"))
        .args(args)
        .output()
        .expect("running imgindex")
}

fn events(output: &Output) -> Vec<Value> {
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).unwrap_or_else(|err| panic!("not JSON: {line}: {err}")))
        .collect()
}

fn kind(event: &Value) -> &str {
    event["event"].as_str().unwrap_or("")
}

#[test]
fn a_successful_pass_exits_zero_and_brackets_its_output() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("a.png"), 64, 48, 0);
    write_image(&dir.path().join("b.png"), 40, 60, 9);

    let output = run(&[dir.path().to_str().unwrap(), "--progress", "json"]);
    assert!(output.status.success(), "exit {:?}", output.status.code());

    let events = events(&output);
    assert_eq!(kind(events.first().expect("a start event")), "start");
    let done = events.last().expect("a done event");
    assert_eq!(kind(done), "done");
    assert_eq!(done["indexed"], 2);
    assert_eq!(done["failed"], 0);
}

#[test]
fn every_json_line_is_one_object_the_gui_can_parse() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("a.png"), 32, 32, 0);
    let output = run(&[dir.path().to_str().unwrap(), "--progress", "json"]);

    for event in events(&output) {
        assert!(event.is_object(), "not an object: {event}");
        assert!(
            matches!(kind(&event), "start" | "progress" | "writing" | "error" | "done"),
            "unknown event kind: {event}"
        );
    }
}

/// The reading bar used to stop short of the end, because the last partial group
/// of files was never announced, and there was nothing at all for the writer,
/// which is still committing after the last file has been read.
#[test]
fn reading_and_writing_both_report_reaching_every_file() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..5 {
        write_image(&dir.path().join(format!("{index}.png")), 32, 32, index);
    }

    let events = events(&run(&[dir.path().to_str().unwrap(), "--progress", "json"]));
    let last = |wanted: &str| {
        events
            .iter()
            .filter(|event| kind(event) == wanted)
            .next_back()
            .unwrap_or_else(|| panic!("no {wanted} event"))
            .clone()
    };

    assert_eq!(last("progress")["done"], 5);
    assert_eq!(last("writing")["done"], 5);
    assert_eq!(last("writing")["total"], 5);
}

/// A pass that reads files and finds nothing to write has finished writing. The
/// count is against what reached the writer, not against the files read, or a
/// folder of things that are not images reports writing as untouched.
#[test]
fn a_pass_with_nothing_to_write_reports_writing_as_complete() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..3 {
        std::fs::write(dir.path().join(format!("{index}.txt")), b"not an image").expect("write");
    }

    let events = events(&run(&[dir.path().to_str().unwrap(), "--progress", "json"]));
    let writing = events
        .iter()
        .filter(|event| kind(event) == "writing")
        .next_back()
        .expect("a writing event");

    assert_eq!(writing["done"], 0);
    assert_eq!(writing["total"], 0);
}

/// Rebuilding the index costs a copy of the whole file. An indexing pass never
/// does it: the only thing that frees enough space to be worth it is a cleanup,
/// and that is where it happens.
#[test]
fn indexing_never_rebuilds_the_index() {
    let dir = tempfile::tempdir().expect("tempdir");
    for index in 0..5 {
        write_image(&dir.path().join(format!("{index}.png")), 64, 64, index);
    }
    let folder = dir.path().to_str().unwrap();

    run(&[folder, "--progress", "json"]);
    let db = dir.path().join("imgdedupe.sqlite");
    let inode_before = std::fs::metadata(&db).expect("stat").len();

    std::fs::remove_file(dir.path().join("0.png")).expect("remove");
    let events = events(&run(&[folder, "--progress", "json"]));
    assert_eq!(events.last().expect("a done event")["removed"], 1);

    let after = std::fs::metadata(&db).expect("stat").len();
    assert!(
        after >= inode_before,
        "an indexing pass rebuilt the file: {inode_before} then {after}"
    );
}

/// With nothing to work on, say how to use it. A complaint about one missing
/// argument tells someone who has just run it for the first time nothing.
#[test]
fn no_arguments_prints_the_help() {
    let output = run(&[]);
    let said = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );

    assert!(said.contains("Usage:"), "no usage line: {said}");
    assert!(said.contains("<FOLDER>"), "it does not say what it wants: {said}");
    assert!(said.contains("--recurse"), "the options are not listed: {said}");
    assert!(said.contains("Examples:"), "there are no examples: {said}");
    assert!(!output.status.success(), "it exited as though it had done something");
}

/// A tool that leaves a file beside itself every time it runs was not asked for.
/// The log is written when it is asked for and not otherwise.
#[test]
fn nothing_is_logged_unless_the_log_flag_is_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("a.png"), 32, 32, 0);
    let folder = dir.path().to_str().unwrap();

    // Beside the program that writes it, which is the binary under test.
    let beside = std::path::Path::new(env!("CARGO_BIN_EXE_imgindex"))
        .parent()
        .expect("a folder to be in")
        .join("imgindex.log");
    let before = std::fs::metadata(&beside).map(|meta| meta.len()).unwrap_or(0);

    run(&[folder, "--progress", "json"]);
    let after = std::fs::metadata(&beside).map(|meta| meta.len()).unwrap_or(0);
    assert_eq!(after, before, "a run without --log wrote to {}", beside.display());

    run(&[folder, "--progress", "json", "--log"]);
    let logged = std::fs::metadata(&beside).map(|meta| meta.len()).unwrap_or(0);
    assert!(logged > before, "a run with --log wrote nothing to {}", beside.display());
}

#[test]
fn text_progress_is_not_json() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("a.png"), 32, 32, 0);
    let output = run(&[dir.path().to_str().unwrap(), "--progress", "text"]);
    let text = String::from_utf8_lossy(&output.stdout);
    assert!(text.contains("indexing 1 files"), "{text}");
    assert!(text.contains("indexed 1"), "{text}");
}

#[test]
fn a_folder_that_does_not_exist_exits_one() {
    let dir = tempfile::tempdir().expect("tempdir");
    let missing = dir.path().join("nowhere");
    let output = run(&[missing.to_str().unwrap()]);
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("is not a folder"),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn a_malformed_image_is_an_error_event_and_not_a_failed_run() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("good.png"), 32, 32, 0);
    std::fs::write(dir.path().join("bad.png"), b"\x89PNG\r\n\x1a\ncut off").expect("write");

    let output = run(&[dir.path().to_str().unwrap(), "--progress", "json"]);
    assert!(output.status.success(), "a bad file failed the whole run");

    let events = events(&output);
    let errors: Vec<&Value> = events.iter().filter(|e| kind(e) == "error").collect();
    assert_eq!(errors.len(), 1);
    assert_eq!(errors[0]["path"], "bad.png");

    let done = events.last().expect("done");
    assert_eq!(done["indexed"], 1);
    assert_eq!(done["failed"], 1);
}

#[test]
fn a_second_pass_over_an_unchanged_folder_indexes_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("a.png"), 64, 48, 0);

    run(&[dir.path().to_str().unwrap(), "--progress", "json"]);
    let output = run(&[dir.path().to_str().unwrap(), "--progress", "json"]);

    let events = events(&output);
    assert_eq!(events.first().expect("start")["total"], 0);
    assert_eq!(events.last().expect("done")["indexed"], 0);
}

#[test]
fn recurse_reaches_subfolders_and_the_default_does_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("top.png"), 32, 32, 0);
    std::fs::create_dir(dir.path().join("sub")).expect("mkdir");
    write_image(&dir.path().join("sub").join("deep.png"), 32, 32, 4);

    let shallow_db = dir.path().join("shallow.sqlite");
    let output = run(&[
        dir.path().to_str().unwrap(),
        "--db",
        shallow_db.to_str().unwrap(),
        "--progress",
        "json",
    ]);
    assert_eq!(events(&output).last().expect("done")["indexed"], 1);

    let deep_db = dir.path().join("deep.sqlite");
    let output = run(&[
        dir.path().to_str().unwrap(),
        "--recurse",
        "--db",
        deep_db.to_str().unwrap(),
        "--progress",
        "json",
    ]);
    assert_eq!(events(&output).last().expect("done")["indexed"], 2);
}

#[test]
fn the_db_flag_puts_the_index_where_it_is_told() {
    let dir = tempfile::tempdir().expect("tempdir");
    let elsewhere = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("a.png"), 32, 32, 0);

    let db_path = elsewhere.path().join("index.sqlite");
    let output = run(&[
        dir.path().to_str().unwrap(),
        "--db",
        db_path.to_str().unwrap(),
    ]);
    assert!(output.status.success());
    assert!(db_path.exists(), "the index was not written where it was told");
    assert!(
        !dir.path().join("imgdedupe.sqlite").exists(),
        "an index was written into the scanned folder anyway"
    );
}

#[test]
fn a_symlinked_folder_is_indexed_through_the_link() {
    // The link is what the user chose. Nothing resolves it, so the pictures under
    // it are read through it and the index sits beside them.
    let dir = tempfile::tempdir().expect("tempdir");
    let real = dir.path().join("real");
    std::fs::create_dir(&real).expect("mkdir");
    write_image(&real.join("a.png"), 32, 32, 0);

    let link = dir.path().join("link");
    #[cfg(windows)]
    let made = std::os::windows::fs::symlink_dir(&real, &link).is_ok();
    #[cfg(unix)]
    let made = std::os::unix::fs::symlink(&real, &link).is_ok();
    if !made {
        // Windows needs developer mode or elevation to create one.
        return;
    }

    let db = dir.path().join("index.sqlite");
    let output = run(&[
        link.to_str().unwrap(),
        "--db",
        db.to_str().unwrap(),
        "--progress",
        "json",
    ]);
    assert!(output.status.success());
    assert_eq!(events(&output).last().expect("done")["indexed"], 1);

    let conn = rusqlite::Connection::open(&db).expect("open");
    let path: String = conn
        .query_row("SELECT rel_path FROM files", [], |row| row.get(0))
        .expect("the indexed file");
    assert_eq!(path, "a.png", "the path was stored against something other than the folder");
}

#[test]
fn a_removed_file_is_reported_as_removed() {
    let dir = tempfile::tempdir().expect("tempdir");
    write_image(&dir.path().join("a.png"), 32, 32, 0);
    write_image(&dir.path().join("b.png"), 32, 32, 7);
    run(&[dir.path().to_str().unwrap(), "--progress", "json"]);

    std::fs::remove_file(dir.path().join("a.png")).expect("remove");
    let output = run(&[dir.path().to_str().unwrap(), "--progress", "json"]);
    assert_eq!(events(&output).last().expect("done")["removed"], 1);
}

