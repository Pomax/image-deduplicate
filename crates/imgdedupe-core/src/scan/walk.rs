use super::*;

/// One file the walk found, before anything has been read from it.
#[derive(Debug, Clone)]
pub(super) struct Candidate {
    pub(super) rel_path: String,
    pub(super) abs_path: PathBuf,
    pub(super) size_bytes: i64,
    pub(super) mtime_seconds: i64,
}

/// Walk the tree and list every file, ignoring the index and its sidecars.
///
/// Reports as it goes. On a folder the machine has to ask another machine about,
/// listing it is one call per file and most of the pass, and it used to say
/// nothing from the first file to the last.
/// Whether a folder is one a pass over the subfolders does not go into.
///
/// A name beginning with a dot is a folder something else keeps its workings in:
/// `.git`, `.thumbnails`, `.cache`. One beginning with an at sign is what network
/// storage puts its own beside a share: `@eaDir`, `@Recycle`. What is in them
/// belongs to the thing that made them, not to whoever is looking for their own
/// pictures, and both are full of copies of pictures that are already elsewhere.
///
/// The folder the pass was pointed at is not tested: somebody who asks for
/// `.private` means it.
fn kept_out(name: &str) -> bool {
    name.starts_with('.') || name.starts_with('@')
}

pub(super) fn walk(
    options: &Options,
    cancel: &AtomicBool,
    report: &(dyn Fn(Event) + Sync),
) -> Result<Vec<Candidate>> {
    report(Event::Reached(Step::StartedLookingForTheTotal));
    // Asked of the folder itself, in one call, so the bar has something to
    // measure against before a single entry has been listed. Only for one folder:
    // the size of a tree is as many answers as it has directories, and a total
    // that grows as they are found is a bar that goes backwards.
    let of = if options.recurse {
        None
    } else {
        dirlist::entry_count(&options.root)
    };
    let mut out = Vec::new();
    let mut queue = vec![options.root.clone()];
    let mut first = true;

    while let Some(dir) = queue.pop() {
        if cancel.load(Ordering::Relaxed) {
            return Ok(out);
        }
        let so_far = out.len() as u64;
        let listed = match dirlist::list(&dir, &|| cancel.load(Ordering::Relaxed), &|found| {
            // As the listing arrives, not when it is finished. This is the read
            // bar's first job: every one of these is a file that has been looked
            // at, and on a folder that answers slowly it is most of the wait.
            report(Event::Walking {
                found: so_far + found,
                of,
            });
        }) {
            Ok(listed) => listed,
            // The folder that was asked for has to be readable, or the pass would
            // see an empty folder and delete every row in the index. One
            // unreadable subfolder is skipped instead.
            Err(err) if first => {
                return Err(err).with_context(|| format!("listing {}", dir.display()))
            }
            Err(_) => continue,
        };
        first = false;

        for entry in listed {
            if cancel.load(Ordering::Relaxed) {
                return Ok(out);
            }
            if entry.is_dir {
                if options.recurse && !kept_out(&entry.name) {
                    queue.push(dir.join(&entry.name));
                }
                continue;
            }
            if !entry.is_file {
                continue;
            }
            // What the name claims. A file that claims none of the formats is
            // not read at all: reading one to find out it is not a picture is
            // the whole file over the network for an answer its name already
            // gave. What it turns out to be is still decided by its first bytes,
            // once there is a reason to have read them.
            if format::from_extension(&entry.name).is_none() {
                continue;
            }
            let path = dir.join(&entry.name);
            let Ok(relative) = path.strip_prefix(&options.root) else {
                continue;
            };
            let Some(rel_path) = to_portable_path(relative) else {
                continue;
            };
            out.push(Candidate {
                rel_path,
                abs_path: path,
                size_bytes: entry.size_bytes,
                mtime_seconds: entry.mtime_seconds,
            });
        }
        report(Event::Walking {
            found: out.len() as u64,
            of,
        });
    }
    // The listing is over, so the count it reached is the exact total, whatever
    // the folder said before it started.
    report(Event::Walking {
        found: out.len() as u64,
        of: Some(out.len() as u64),
    });
    Ok(out)
}

/// Relative paths are stored with forward slashes so an index built on one
/// platform still matches the same tree on another.
fn to_portable_path(relative: &Path) -> Option<String> {
    let mut parts = Vec::new();
    for component in relative.components() {
        match component {
            std::path::Component::Normal(part) => parts.push(part.to_str()?.to_string()),
            _ => return None,
        }
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join("/"))
    }
}

pub(super) fn now_seconds() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|delta| delta.as_secs() as i64)
        .unwrap_or(0)
}
