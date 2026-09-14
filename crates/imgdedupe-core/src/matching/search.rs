use super::*;

/// Find every duplicate set in the index.
pub fn find_sets(conn: &Connection, thresholds: Thresholds) -> Result<Vec<DuplicateSet>> {
    let never = AtomicBool::new(false);
    Ok(find_sets_cancellable(conn, thresholds, &never, &|_| {})?
        .expect("a search that is never cancelled cannot come back cancelled"))
}

/// Batches the comparing is cut into, so it spreads across the machine's cores.
pub(super) const COMPARE_BATCHES: u64 = 32;

/// Pairs a batch gets through between saying so, and between looking at whether
/// it has been told to stop. A pair is tens of microseconds, so this is a
/// fraction of a second either way.
const COMPARE_REPORT_EVERY: usize = 256;

/// As `find_sets`, stopping when asked.
///
/// `cancel` is looked at between the pieces the work is cut into, which is often
/// enough that a search stops in a fraction of a second. Nothing comes back when
/// it is stopped: a half finished search has no answer to give.
pub fn find_sets_cancellable(
    conn: &Connection,
    thresholds: Thresholds,
    cancel: &AtomicBool,
    report: &(dyn Fn(Progress) + Sync),
) -> Result<Option<Vec<DuplicateSet>>> {
    let Some(images) = load_images(conn, cancel, report)? else {
        return Ok(None);
    };
    find_sets_in(&images, thresholds, cancel, report)
}

/// Find every duplicate set among pictures already in memory.
///
/// No database. This is the whole of a second search: the reading was done once,
/// and changing what counts as a duplicate changes only the comparing.
pub fn find_sets_in(
    images: &[Image],
    thresholds: Thresholds,
    cancel: &AtomicBool,
    report: &(dyn Fn(Progress) + Sync),
) -> Result<Option<Vec<DuplicateSet>>> {
    #[cfg(feature = "logging")]
    let mut timing = Timing::new();
    let stopped = || cancel.load(Ordering::Relaxed);

    // Nothing to compare is not a search that ran and found nothing. Saying so
    // fills a bar and lights a lamp for work that did not happen.
    if images.is_empty() {
        return Ok(Some(Vec::new()));
    }
    report(Progress::Loaded {
        images: images.len() as u64,
    });
    if stopped() {
        return Ok(None);
    }

    let families = fold_identical(images, thresholds.within_a_folder);
    let one_of_each: Vec<u32> = families.iter().map(|family| family[0]).collect();
    crate::log_line!(
        "search folding: {} images are {} different pictures",
        images.len(),
        one_of_each.len()
    );

    // A search held to folders is not one search with pairs thrown away at the
    // end of it: it is a search of each folder, with its own index of hashes and
    // its own index of corners, holding what that folder holds and nothing else.
    // Smaller structures, and pictures in another folder are never candidates in
    // the first place.
    let searches = by_folder(images, one_of_each, thresholds.within_a_folder);
    crate::log_line!("search: {} folder(s) to look through", searches.len());

    let mut candidates: Vec<(u32, u32)> = Vec::new();
    // Each way of matching draws up its own shortlist, and a way that is turned
    // off draws up none: there is nothing to be gained by shortlisting pairs for
    // a test that is not going to be made.
    for of in &searches {
        let by_band: Vec<Vec<(u32, u32)>> = (0..fingerprint::BANDS)
            .into_par_iter()
            .map(|band| {
                if stopped() || !thresholds.whole_frame {
                    return Vec::new();
                }
                pairs_in_band(images, of, band)
            })
            .collect();
        candidates.extend(by_band.concat());
    }
    if stopped() {
        return Ok(None);
    }
    #[cfg(feature = "logging")]
    timing.step("banding", candidates.len(), "candidate pairs");

    // The second way in: pictures holding some of the same corners. The bands
    // above only ever put together pictures that fill the frame the same way.
    let looked = AtomicU64::new(0);
    let to_look: u64 = searches.iter().map(|of| of.len() as u64).sum();
    report(Progress::Shortlisting {
        done: 0,
        total: to_look,
    });
    let mut by_corner = Vec::new();
    if thresholds.corners {
        for of in &searches {
            by_corner.extend(pairs_by_corner(
                images, of, &stopped, report, &looked, to_look,
            ));
        }
    }
    #[cfg(feature = "logging")]
    timing.step("corners", by_corner.len(), "candidate pairs");
    candidates.extend(by_corner);
    candidates.par_sort_unstable();
    candidates.dedup();
    #[cfg(feature = "logging")]
    timing.step("pairing", candidates.len(), "candidate pairs");

    let bounds: Vec<usize> = (0..=COMPARE_BATCHES)
        .map(|batch| (candidates.len() as u64 * batch / COMPARE_BATCHES) as usize)
        .collect();
    let compared = AtomicU64::new(0);
    let pairs = candidates.len() as u64;
    let matched: Vec<Vec<(u32, u32)>> = (0..COMPARE_BATCHES as usize)
        .into_par_iter()
        .map(|batch| {
            if stopped() {
                return Vec::new();
            }
            let batch_pairs = &candidates[bounds[batch]..bounds[batch + 1]];
            let mut found = Vec::new();
            // Reported from inside the batch rather than at the end of it. Every
            // batch is running at once, so they all finish at about the same
            // moment: a bar fed by finished batches stands still for the whole
            // of the comparing and then fills in one step.
            for (which, pair) in batch_pairs.iter().enumerate() {
                let (a, b) = *pair;
                if is_match(&images[a as usize], &images[b as usize], thresholds) {
                    found.push((a, b));
                }
                if (which + 1) % COMPARE_REPORT_EVERY == 0 {
                    if stopped() {
                        return Vec::new();
                    }
                    let step = COMPARE_REPORT_EVERY as u64;
                    let done = compared.fetch_add(step, Ordering::Relaxed) + step;
                    report(Progress::Comparing { done, total: pairs });
                }
            }
            let rest = (batch_pairs.len() % COMPARE_REPORT_EVERY) as u64;
            if rest > 0 {
                let done = compared.fetch_add(rest, Ordering::Relaxed) + rest;
                report(Progress::Comparing { done, total: pairs });
            }
            found
        })
        .collect();
    if stopped() {
        return Ok(None);
    }
    let matches = matched.concat();
    #[cfg(feature = "logging")]
    timing.step("comparing", matches.len(), "matches");
    report(Progress::Grouping);

    let mut groups = Groups::new(images.len());
    let mut in_a_set: std::collections::BTreeSet<u32> = std::collections::BTreeSet::new();
    for (a, b) in &matches {
        groups.join(*a, *b);
        in_a_set.insert(*a);
        in_a_set.insert(*b);
    }
    // The copies that were folded away before the pairing. They are duplicates of
    // the one that stood for them whether or not it matched anything else.
    for family in &families {
        if family.len() > 1 {
            for position in family {
                groups.join(family[0], *position);
                in_a_set.insert(*position);
            }
        }
    }
    let mut members: Vec<(u32, u32)> = in_a_set
        .into_iter()
        .map(|position| (groups.root(position), position))
        .collect();
    members.sort_unstable();
    #[cfg(feature = "logging")]
    timing.step("grouping", members.len(), "images in a set");

    let sets = build_sets(&images, &members);
    #[cfg(feature = "logging")]
    timing.step("listing", sets.len(), "sets");
    #[cfg(feature = "logging")]
    timing.total(sets.len());
    Ok(Some(sets))
}

/// What each step of a search cost, written to the run log. A search that is slow
/// on someone's folder is a fact about that folder, and the only way to know
/// which step it is spending the time in is for the run to say so.
#[cfg(feature = "logging")]
struct Timing {
    started: std::time::Instant,
    step_started: std::time::Instant,
}

#[cfg(feature = "logging")]
impl Timing {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Timing {
            started: now,
            step_started: now,
        }
    }

    /// What a step cost and how much work it handed on, so a search that is slow
    /// on someone's folder says which step it was and on how much.
    fn step(&mut self, name: &str, count: usize, of: &str) {
        let took = self.step_started.elapsed();
        self.step_started = std::time::Instant::now();
        crate::log_line!("search {name}: {:.2}s, {count} {of}", took.as_secs_f64());
    }

    fn total(&self, sets: usize) {
        crate::log_line!(
            "search finished: {:.2}s, {sets} sets",
            self.started.elapsed().as_secs_f64()
        );
    }
}
