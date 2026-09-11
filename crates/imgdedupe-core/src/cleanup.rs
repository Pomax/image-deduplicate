use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// What happens to the files that are not kept.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Disposal {
    /// The default: the operating system's recycle bin, so a wrong choice is recoverable.
    Trash,
    /// Move into a folder, keeping the relative path, so the originals can be put back.
    MoveTo(PathBuf),
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
    if let Disposal::MoveTo(target) = disposal {
        std::fs::create_dir_all(target)
            .with_context(|| format!("creating the folder {}", target.display()))?;
    }

    let mut outcome = Outcome::default();
    for (index, removal) in plan.removals.iter().enumerate() {
        done(index);
        let path = root.join(&removal.rel_path);
        let result = match disposal {
            Disposal::Trash => trash::delete(&path).map_err(|err| err.to_string()),
            Disposal::Delete => std::fs::remove_file(&path).map_err(|err| err.to_string()),
            Disposal::MoveTo(target) => move_to(&path, &removal.rel_path, target),
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

/// Move under the chosen folder keeping the relative path, so the tree can be put
/// back over the original by copying it in.
fn move_to(path: &Path, rel_path: &str, target: &Path) -> std::result::Result<(), String> {
    let destination = target.join(rel_path);
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|err| err.to_string())?;
    }
    match std::fs::rename(path, &destination) {
        Ok(()) => Ok(()),
        // A rename across volumes fails, and the chosen folder is often on
        // another one, so fall back to a copy and then remove the original.
        Err(_) => {
            std::fs::copy(path, &destination).map_err(|err| err.to_string())?;
            std::fs::remove_file(path).map_err(|err| err.to_string())
        }
    }
}

/// Build a plan from each set and the pictures in it marked to keep.
///
/// What is marked is kept and everything else goes, including every picture of a
/// set that marks nothing. A set nobody says anything about is not passed in at
/// all, which is what happens to one that has been ignored. The count is on the
/// button that carries the plan out.
pub fn plan_from_sets<'a>(
    sets: impl IntoIterator<Item = (&'a [crate::matching::Member], &'a [i64])>,
) -> Plan {
    let mut plan = Plan::default();
    for (members, kept) in sets {
        for member in members
            .iter()
            .filter(|member| !kept.contains(&member.file_id))
        {
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
#[path = "tests/cleanup.rs"]
mod tests;

impl Disposal {
    /// What the cleanup screen starts on. Recoverable, always.
    pub fn default_for_review() -> Self {
        Disposal::Trash
    }
}
