use super::*;
use crate::matching::Member;

fn member(id: i64, path: &str, size: i64, auto_keep: bool) -> Member {
    Member {
        file_id: id,
        rel_path: path.to_string(),
        width: 100,
        height: 100,
        format: "jpeg".to_string(),
        channels: 3,
        size_bytes: size,
        mtime_seconds: 1,
        auto_keep,
    }
}

fn plan_of(paths: &[(&str, i64)]) -> Plan {
    Plan {
        removals: paths
            .iter()
            .enumerate()
            .map(|(index, (path, size))| Removal {
                file_id: index as i64,
                rel_path: path.to_string(),
                size_bytes: *size,
            })
            .collect(),
    }
}

#[test]
fn a_plan_counts_its_files_and_bytes() {
    let plan = plan_of(&[("a.jpg", 100), ("b.jpg", 250)]);
    assert_eq!(plan.files(), 2);
    assert_eq!(plan.bytes(), 350);
    assert_eq!(plan.to_text(), "a.jpg\nb.jpg\n");
}

#[test]
fn a_plan_takes_everything_that_is_not_marked() {
    let members = vec![
        member(1, "keep.jpg", 500, true),
        member(2, "drop.jpg", 300, false),
        member(3, "drop2.jpg", 200, false),
    ];
    let plan = plan_from_sets([(members.as_slice(), [1].as_slice())]);
    assert_eq!(plan.files(), 2);
    assert_eq!(plan.bytes(), 500);
    assert!(!plan.to_text().contains("keep.jpg"));
}

/// What is marked is kept, so a set that marks nothing keeps nothing and
/// every picture in it goes.
#[test]
fn a_set_with_nothing_marked_loses_all_of_it() {
    let members = vec![
        member(1, "a.jpg", 100, false),
        member(2, "b.jpg", 100, false),
    ];
    let plan = plan_from_sets([(members.as_slice(), [].as_slice())]);
    assert_eq!(plan.files(), 2);
    assert_eq!(plan.bytes(), 200);
}

#[test]
fn a_set_with_everything_marked_loses_none_of_it() {
    let members = vec![member(1, "a.jpg", 100, true), member(2, "b.jpg", 100, true)];
    assert_eq!(
        plan_from_sets([(members.as_slice(), [1, 2].as_slice())]).files(),
        0
    );
}

/// The window shows a bar while files are going, so the removal has to count
/// them off one at a time and finish on the total.
#[test]
fn removing_says_how_many_files_it_has_been_through() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut paths = Vec::new();
    for index in 0..5 {
        let name = format!("{index}.jpg");
        std::fs::write(dir.path().join(&name), b"x").unwrap();
        paths.push((name, 1i64));
    }
    let plan = plan_of(
        &paths
            .iter()
            .map(|(name, size)| (name.as_str(), *size))
            .collect::<Vec<_>>(),
    );

    let seen = std::cell::RefCell::new(Vec::new());
    let outcome = apply_reporting(dir.path(), &plan, &Disposal::Delete, &|done| {
        seen.borrow_mut().push(done);
    })
    .expect("remove");

    assert_eq!(outcome.removed.len(), 5);
    assert_eq!(
        seen.into_inner(),
        vec![0, 1, 2, 3, 4, 5],
        "the count skipped or stopped short"
    );
}

#[test]
fn deleting_removes_the_planned_files_and_nothing_else() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("keep.jpg"), b"keep").unwrap();
    std::fs::write(dir.path().join("drop.jpg"), b"drop").unwrap();

    let plan = plan_of(&[("drop.jpg", 4)]);
    let outcome = apply(dir.path(), &plan, &Disposal::Delete).expect("apply");

    assert_eq!(outcome.removed, vec!["drop.jpg".to_string()]);
    assert_eq!(outcome.bytes_freed, 4);
    assert!(outcome.failed.is_empty());
    assert!(dir.path().join("keep.jpg").exists());
    assert!(!dir.path().join("drop.jpg").exists());
}

#[test]
fn moving_to_a_folder_keeps_the_path_under_it() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir_all(dir.path().join("one/two")).unwrap();
    std::fs::write(dir.path().join("one/two/drop.jpg"), b"drop").unwrap();
    let held = dir.path().join("somewhere else");

    let plan = plan_of(&[("one/two/drop.jpg", 4)]);
    let outcome = apply(dir.path(), &plan, &Disposal::MoveTo(held.clone())).expect("apply");

    assert_eq!(outcome.failed, Vec::new());
    assert!(!dir.path().join("one/two/drop.jpg").exists());
    assert_eq!(
        std::fs::read(held.join("one/two/drop.jpg")).expect("moved file"),
        b"drop"
    );
}

#[test]
fn a_file_that_is_already_gone_is_reported_and_does_not_stop_the_rest() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("present.jpg"), b"here").unwrap();

    let plan = plan_of(&[("missing.jpg", 10), ("present.jpg", 4)]);
    let outcome = apply(dir.path(), &plan, &Disposal::Delete).expect("apply");

    assert_eq!(outcome.removed, vec!["present.jpg".to_string()]);
    assert_eq!(outcome.failed.len(), 1);
    assert_eq!(outcome.failed[0].0, "missing.jpg");
    assert_eq!(outcome.bytes_freed, 4);
}

#[test]
fn an_empty_plan_does_nothing() {
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::write(dir.path().join("a.jpg"), b"a").unwrap();
    let outcome = apply(dir.path(), &Plan::default(), &Disposal::Delete).expect("apply");
    assert_eq!(outcome, Outcome::default());
    assert!(dir.path().join("a.jpg").exists());
}

#[test]
fn the_default_disposal_is_the_recycle_bin() {
    // Stated as a test so that changing the default has to change a test that
    // says what the default is.
    assert_eq!(Disposal::default_for_review(), Disposal::Trash);
}
