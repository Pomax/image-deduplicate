use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// What happens to the files that are not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposal {
    /// The default: the operating system's recycle bin, so a wrong choice is recoverable.
    Trash,
    /// Move into a folder, keeping the relative path, so the originals can be put back.
    Quarantine(PathBuf),
    /// Unlink. Not recoverable, and never the default.
    Delete,
}

/// One file the plan will remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Removal {
    pub file_id: i64,
    pub rel_path: String,
    pub size_bytes: i64,
}

/// Everything that will happen, assembled before anything is touched so it can be
/// shown, counted and exported first.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Plan {
    pub removals: Vec<Removal>,
}

impl Plan {
    pub fn files(&self) -> usize {
        self.removals.len()
    }

    pub fn bytes(&self) -> i64 {
        self.removals.iter().map(|removal| removal.size_bytes).sum()
    }

    /// The list a person reads before confirming.
    pub fn to_text(&self) -> String {
        let mut out = String::new();
        for removal in &self.removals {
            out.push_str(&removal.rel_path);
            out.push('\n');
        }
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outcome {
    pub removed: Vec<String>,
    pub failed: Vec<(String, String)>,
    pub bytes_freed: i64,
}

/// Carry out a plan. Nothing here decides what to remove; that is settled before
/// this is called and confirmed by a person.
pub fn apply(root: &Path, plan: &Plan, disposal: &Disposal) -> Result<Outcome> {
    apply_reporting(root, plan, disposal, &|_| {})
}

/// As `apply`, saying how many files it has been through. Removing thousands of
/// files takes long enough that a window doing it silently looks stuck.
pub fn apply_reporting(
    root: &Path,
    plan: &Plan,
    disposal: &Disposal,
    done: &dyn Fn(usize),
) -> Result<Outcome> {
    if let Disposal::Quarantine(target) = disposal {
        std::fs::create_dir_all(target)
            .with_context(|| format!("creating the quarantine folder {}", target.display()))?;
    }

    let mut outcome = Outcome::default();
    for (index, removal) in plan.removals.iter().enumerate() {
        done(index);
        let path = root.join(&removal.rel_path);
        let result = match disposal {
            Disposal::Trash => trash::delete(&path).map_err(|err| err.to_string()),
            Disposal::Delete => std::fs::remove_file(&path).map_err(|err| err.to_string()),
            Disposal::Quarantine(target) => quarantine(&path, &removal.rel_path, target),
        };
        match result {
            Ok(()) => {
                outcome.bytes_freed += removal.size_bytes;
                outcome.removed.push(removal.rel_path.clone());
            }
            Err(message) => outcome.failed.push((removal.rel_path.clone(), message)),
        }
    }
    done(plan.removals.len());
    Ok(outcome)
}

/// Move under the quarantine folder keeping the relative path, so the tree can be
/// put back over the original by copying it in.
fn quarantine(path: &Path, rel_path: &str, target: &Path) -> std::result::Result<(), String> {
    let destination = target.join(rel_path);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    match std::fs::rename(path, &destination) {
        Ok(()) => Ok(()),
        // A rename across volumes fails, and the quarantine folder is often on
        // another one, so fall back to a copy and then remove the original.
        Err(_) => {
            std::fs::copy(path, &destination).map_err(|err| err.to_string())?;
            std::fs::remove_file(path).map_err(|err| err.to_string())
        }
    }
}

/// Build a plan from the members of a set that are not marked to keep.
///
/// A set with nothing kept is left alone entirely. Removing every copy of an
/// image is never what someone meant, and it is the one mistake this tool could
/// make that has no undo.
pub fn plan_from_sets<'a>(
    sets: impl IntoIterator<Item = (&'a [crate::matching::Member], bool)>,
) -> Plan {
    let mut plan = Plan::default();
    for (members, resolved) in sets {
        if !resolved {
            continue;
        }
        if !members.iter().any(|member| member.auto_keep) {
            continue;
        }
        for member in members.iter().filter(|member| !member.auto_keep) {
            plan.removals.push(Removal {
                file_id: member.file_id,
                rel_path: member.rel_path.clone(),
                size_bytes: member.size_bytes,
            });
        }
    }
    plan
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::matching::Member;

    fn member(id: i64, path: &str, size: i64, keep: bool) -> Member {
        Member {
            file_id: id,
            rel_path: path.to_string(),
            width: 100,
            height: 100,
            format: "jpeg".to_string(),
            channels: 3,
            size_bytes: size,
            mtime_ns: 1,
            auto_keep: keep,
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
    fn a_plan_takes_everything_but_the_keeper() {
        let members = vec![
            member(1, "keep.jpg", 500, true),
            member(2, "drop.jpg", 300, false),
            member(3, "drop2.jpg", 200, false),
        ];
        let plan = plan_from_sets([(members.as_slice(), true)]);
        assert_eq!(plan.files(), 2);
        assert_eq!(plan.bytes(), 500);
        assert!(!plan.to_text().contains("keep.jpg"));
    }

    #[test]
    fn an_unresolved_set_contributes_nothing() {
        let members = vec![member(1, "a.jpg", 100, true), member(2, "b.jpg", 100, false)];
        assert_eq!(plan_from_sets([(members.as_slice(), false)]).files(), 0);
    }

    #[test]
    fn a_set_with_nothing_kept_is_left_alone() {
        // The one mistake with no undo. A set where every member is unmarked must
        // never turn into a plan that removes all of them.
        let members = vec![member(1, "a.jpg", 100, false), member(2, "b.jpg", 100, false)];
        assert_eq!(plan_from_sets([(members.as_slice(), true)]).files(), 0);
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
            &paths.iter().map(|(name, size)| (name.as_str(), *size)).collect::<Vec<_>>(),
        );

        let seen = std::cell::RefCell::new(Vec::new());
        let outcome = apply_reporting(dir.path(), &plan, &Disposal::Delete, &|done| {
            seen.borrow_mut().push(done);
        })
        .expect("remove");

        assert_eq!(outcome.removed.len(), 5);
        assert_eq!(seen.into_inner(), vec![0, 1, 2, 3, 4, 5], "the count skipped or stopped short");
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
    fn quarantine_moves_the_file_and_keeps_its_path() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("one/two")).unwrap();
        std::fs::write(dir.path().join("one/two/drop.jpg"), b"drop").unwrap();
        let held = dir.path().join("quarantine");

        let plan = plan_of(&[("one/two/drop.jpg", 4)]);
        let outcome = apply(dir.path(), &plan, &Disposal::Quarantine(held.clone())).expect("apply");

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
}

impl Disposal {
    /// What the cleanup screen starts on. Recoverable, always.
    pub fn default_for_review() -> Self {
        Disposal::Trash
    }
}
