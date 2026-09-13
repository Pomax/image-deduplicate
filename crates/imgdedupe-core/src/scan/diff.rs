use super::*;

/// Which paths need reading and which can be skipped without touching the disk.
pub(super) struct Diff {
    pub(super) to_index: Vec<Candidate>,
    pub(super) removed: Vec<String>,
    pub(super) unchanged: u64,
}

pub(super) fn diff(
    candidates: Vec<Candidate>,
    known: &std::collections::HashMap<String, db::Known>,
) -> Diff {
    let mut to_index = Vec::new();
    let mut unchanged = 0u64;
    let mut seen = std::collections::HashSet::with_capacity(candidates.len());

    for candidate in candidates {
        seen.insert(candidate.rel_path.clone());
        let entry = known.get(&candidate.rel_path);
        // The file is where it was, at the size it was. Whether that settles it
        // depends on what the index has to say about it: a picture is settled by
        // its fingerprints being current, and a file the pass has already read
        // and found not to be a picture is settled by having been read.
        let where_it_was = entry.is_some_and(|entry| {
            entry.size_bytes == candidate.size_bytes
                && entry.mtime_seconds == candidate.mtime_seconds
        });
        let fresh = where_it_was
            && entry.is_some_and(|entry| {
                entry.not_a_picture || entry.fingerprint_version == FINGERPRINT_VERSION
            });
        if fresh {
            unchanged += 1;
        } else {
            to_index.push(candidate);
        }
    }

    let removed = known
        .keys()
        .filter(|path| !seen.contains(*path))
        .cloned()
        .collect();

    Diff {
        to_index,
        removed,
        unchanged,
    }
}

/// What the index records about a file the pass read and did not index.
pub(super) fn looked_at(candidate: &Candidate) -> db::Looked {
    db::Looked {
        rel_path: candidate.rel_path.clone(),
        size_bytes: candidate.size_bytes,
        mtime_seconds: candidate.mtime_seconds,
    }
}

/// Read, sniff, decode and fingerprint one file. Never panics on bad input: a
/// malformed file comes back as `Failed` and the pass continues.
/// How much of the folder is in the index, as the pass sees it.
///
/// The files that were left alone are already in it, and the ones that turned
/// out not to be pictures are not part of the folder as far as this is
/// concerned, so they leave the total rather than sitting in it unindexed.
pub(super) fn indexed_so_far(
    indexed: u64,
    unchanged: u64,
    to_index: u64,
    read: &AtomicU64,
    ignored: &AtomicU64,
) -> Event {
    let ignored = ignored.load(Ordering::Relaxed);
    Event::Writing {
        done: unchanged + indexed,
        total: unchanged + to_index.saturating_sub(ignored),
        read: unchanged + read.load(Ordering::Relaxed),
        unchanged,
        ignored,
    }
}
