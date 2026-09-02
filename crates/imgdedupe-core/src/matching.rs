use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::Connection;

use crate::fingerprint::{self, Words};
use crate::format::Format;
use crate::score::keep_score;

/// How alike two images must be to land in the same set.
#[derive(Debug, Clone, Copy)]
pub struct Thresholds {
    /// Maximum differing bits between two perceptual hashes.
    pub max_bits: u32,
    /// Maximum distance between two ring colour signatures.
    pub max_ring: f32,
    /// Skip the colour check, so a colourised copy and its grayscale original match.
    pub ignore_colour: bool,
}

/// The largest distance the band lookup is guaranteed to find.
///
/// Two hashes differing in at most `BANDS - 1` bits must leave one band untouched.
/// This is a floor on what candidate generation finds, not a ceiling: a pair
/// twenty bits apart spreads those bits over sixteen bands and usually leaves
/// several clean, so it is found too. Setting the verification threshold to this
/// number was measured to reject half of all rotated duplicates, which are the
/// ones that land furthest out.
pub const GUARANTEED_RADIUS: u32 = (fingerprint::BANDS - 1) as u32;

/// Thresholds are a share of the hash, so they keep their meaning if the hash
/// length changes. The figures come from measurement: a resize, a recompression
/// or a rotation of the same picture moves the hash by up to about 8 percent, and
/// unrelated pictures sit above 25.
fn share(percent: f64) -> u32 {
    (fingerprint::HASH_BITS as f64 * percent / 100.0) as u32
}

/// The widest setting offered. Unrelated pictures were measured above 25 percent
/// apart, so the top of this range reports them as duplicates. That is the
/// point of it: what is a duplicate is the person's to decide, and the review
/// step is where they decide it.
pub const MAX_SENSITIVITY: f64 = 50.0;

/// What the window starts on, and what it goes back to for a folder it has not
/// been set for.
pub const DEFAULT_SENSITIVITY: f64 = 15.0;

/// The named points on the scale. Everything between them is reachable too: a
/// preset is a place on the slider, not a separate setting.
pub const PRESETS: [(&str, f64); 4] = [
    // Re-encodes and resizes of the same picture.
    ("close", 5.0),
    // Heavier edits, crops and rotations.
    ("balanced", DEFAULT_SENSITIVITY),
    // Pictures of the same thing, and some that are not.
    ("wide", 30.0),
    // Everything, including pictures with nothing to do with each other.
    ("yolo", MAX_SENSITIVITY),
];

impl Thresholds {
    /// The threshold a preset stands for, or the closest one if the name is not
    /// a preset.
    pub fn preset(name: &str) -> Self {
        let percent = PRESETS
            .iter()
            .find(|(preset, _)| *preset == name)
            .map_or(DEFAULT_SENSITIVITY, |(_, percent)| *percent);
        Thresholds::at(percent)
    }

    /// A threshold anywhere between the presets, so the recall against false
    /// pairs trade is the user's to make rather than three points on it.
    ///
    /// `percent` is a share of the hash, which keeps its meaning if the hash
    /// length changes. The ring colour distance is scaled with it, since a
    /// picture that has drifted far enough for the hash to notice has usually
    /// drifted in colour too.
    pub fn at(percent: f64) -> Self {
        let percent = percent.clamp(0.0, MAX_SENSITIVITY);
        Thresholds {
            max_bits: share(percent),
            max_ring: (0.005 * percent).max(0.005) as f32,
            ignore_colour: false,
        }
    }

    /// Where this threshold sits on the scale the UI shows.
    pub fn percent(&self) -> f64 {
        self.max_bits as f64 * 100.0 / fingerprint::HASH_BITS as f64
    }
}

#[derive(Debug, Clone)]
pub struct Member {
    pub file_id: i64,
    pub rel_path: String,
    pub width: u32,
    pub height: u32,
    pub format: String,
    pub channels: u8,
    pub size_bytes: i64,
    /// When the file was last written, as the file system reports it. Nothing
    /// here reads a date out of the file's own metadata.
    pub mtime_ns: i64,
    pub auto_keep: bool,
}

#[derive(Debug, Clone)]
pub struct DuplicateSet {
    pub set_id: i64,
    pub members: Vec<Member>,
}

impl DuplicateSet {
    /// Bytes freed by keeping only the marked member.
    pub fn recoverable_bytes(&self) -> i64 {
        self.members.iter().filter(|m| !m.auto_keep).map(|m| m.size_bytes).sum()
    }
}

/// Two images are the same picture only if they have the same shape, allowing for
/// a 90 degree rotation having swapped the axes.
fn aspect_ok(w1: f64, h1: f64, w2: f64, h2: f64) -> bool {
    if w1 <= 0.0 || h1 <= 0.0 || w2 <= 0.0 || h2 <= 0.0 {
        return false;
    }
    let a = w1.max(h1) / w1.min(h1);
    let b = w2.max(h2) / w2.min(h2);
    (a - b).abs() / a.max(b) < 0.06
}

fn parse_format(name: &str) -> Format {
    match name {
        "png" => Format::Png,
        "gif" => Format::Gif,
        "webp" => Format::WebP,
        _ => Format::Jpeg,
    }
}

/// Everything about one image the comparison needs, in the form it compares in.
///
/// The stored blobs are turned into machine words and pre-weighted floats once,
/// here. A folder produces far more comparisons than it has images, and unpacking
/// the same blob on each of them is the whole cost of the old approach.
struct Image {
    file_id: i64,
    rel_path: String,
    width: u32,
    height: u32,
    format: String,
    channels: u8,
    size_bytes: i64,
    mtime_ns: i64,
    /// How good a keeper this is. Fixed by the file, so it is worked out once.
    score: f64,
    /// The hash the image was indexed under, and the seven for its rotations and
    /// mirrors.
    variants: [Words; fingerprint::VARIANTS],
    /// `bands[variant][band]`: what each variant's hash reads as in each band.
    bands: [[u16; fingerprint::BANDS]; fingerprint::VARIANTS],
    /// The ring signature, pre-weighted. Empty when it cannot be compared.
    ring: Vec<f32>,
}

const LOAD_IMAGES: &str = "
SELECT id, rel_path, width, height, format, channels, size_bytes, mtime_ns,
       dct_hashes, ring_stats
FROM indexed_images
ORDER BY id
";

/// Every image filed under the value its hash takes in one band.
///
/// This is what replaces the join. The entries are laid out in band-value order,
/// so everything sharing a value is one run of them, and the images to compare
/// against are two array reads away rather than an index seek per row. Every
/// variant of every image is filed, and the lookup is made with the variant the
/// image was indexed under, which is what finds a rotated copy.
struct BandIndex {
    /// Where each value's run begins, and one past the end for the last one.
    starts: Vec<u32>,
    /// Image positions, in band-value order.
    entries: Vec<u32>,
}

/// Values one band can take. The band is 16 bits wide, so this array is the
/// lookup: there is nothing to search.
const BAND_VALUES: usize = 1 << fingerprint::BAND_BITS;

impl BandIndex {
    /// `of` is the images to file, by position. Only one image out of each set of
    /// identical ones is filed, so a folder holding a thousand copies of one
    /// picture costs one entry rather than a thousand that all collide.
    fn build(images: &[Image], of: &[u32], band: usize) -> BandIndex {
        let mut starts = vec![0u32; BAND_VALUES + 1];
        for position in of {
            for variant in &images[*position as usize].bands {
                starts[variant[band] as usize + 1] += 1;
            }
        }
        for value in 1..starts.len() {
            starts[value] += starts[value - 1];
        }

        let mut cursor = starts.clone();
        let mut entries = vec![0u32; of.len() * fingerprint::VARIANTS];
        for position in of {
            for variant in &images[*position as usize].bands {
                let value = variant[band] as usize;
                entries[cursor[value] as usize] = *position;
                cursor[value] += 1;
            }
        }
        BandIndex { starts, entries }
    }

    fn holders(&self, value: u16) -> &[u32] {
        let value = value as usize;
        &self.entries[self.starts[value] as usize..self.starts[value + 1] as usize]
    }
}

/// Every pair one band puts together, each pair once and in order.
fn pairs_in_band(images: &[Image], of: &[u32], band: usize) -> Vec<(u32, u32)> {
    let index = BandIndex::build(images, of, band);
    let mut out: Vec<(u32, u32)> = Vec::new();
    for a in of {
        for &b in index.holders(images[*a as usize].bands[0][band]) {
            if b != *a {
                out.push(((*a).min(b), (*a).max(b)));
            }
        }
    }
    out.sort_unstable();
    out.dedup();
    out
}

/// Everything the comparison looks at. Two images with the same one of these
/// answer the same way to every test the search makes, against every other image,
/// so only one of them has to be compared to anything.
///
/// The dimensions are part of it because the shape test is the one thing that can
/// separate two pictures with the same hash.
#[derive(PartialEq, Eq, Hash)]
struct Identity {
    variants: [Words; fingerprint::VARIANTS],
    ring: Vec<u32>,
    width: u32,
    height: u32,
}

impl Identity {
    fn of(image: &Image) -> Identity {
        Identity {
            variants: image.variants,
            ring: image.ring.iter().map(|value| value.to_bits()).collect(),
            width: image.width,
            height: image.height,
        }
    }
}

/// Group the images that are indistinguishable to the comparison.
///
/// One pass, one map: an identity that is already known gains a position, one
/// that is not starts a group with the position that found it. The first position
/// in each group is the one that goes through the pairing, and it stands for the
/// rest.
///
/// This is what keeps a folder holding many copies of one picture cheap. Those
/// copies all land in the same band bucket, and comparing a bucket is comparing
/// everything in it to everything else in it.
fn fold_identical(images: &[Image]) -> Vec<Vec<u32>> {
    let mut known: std::collections::HashMap<Identity, usize> =
        std::collections::HashMap::with_capacity(images.len());
    let mut groups: Vec<Vec<u32>> = Vec::with_capacity(images.len());
    for (position, image) in images.iter().enumerate() {
        match known.get(&Identity::of(image)) {
            Some(group) => groups[*group].push(position as u32),
            None => {
                known.insert(Identity::of(image), groups.len());
                groups.push(vec![position as u32]);
            }
        }
    }
    groups
}

/// Whether a candidate pair really is the same picture. `a` is the one with the
/// lower file id, which is the side that contributes all eight of its variants.
fn is_match(a: &Image, b: &Image, thresholds: Thresholds) -> bool {
    if !aspect_ok(a.width as f64, a.height as f64, b.width as f64, b.height as f64) {
        return false;
    }
    let distance =
        fingerprint::hamming_any_words(&a.variants, &b.variants[0], thresholds.max_bits);
    if distance > thresholds.max_bits {
        return false;
    }
    thresholds.ignore_colour
        || fingerprint::ring_distance_weighted(&a.ring, &b.ring) <= thresholds.max_ring
}

/// Which images ended up connected to which. Matched pairs are edges and a set is
/// everything one of them can be walked to from, so a chain of near matches is
/// one set even where its ends do not match each other.
struct Groups {
    parent: Vec<u32>,
}

impl Groups {
    fn new(count: usize) -> Groups {
        Groups { parent: (0..count as u32).collect() }
    }

    fn root(&mut self, mut of: u32) -> u32 {
        while self.parent[of as usize] != of {
            let grandparent = self.parent[self.parent[of as usize] as usize];
            self.parent[of as usize] = grandparent;
            of = grandparent;
        }
        of
    }

    /// The lower position wins, so a set is named after its earliest image and the
    /// name does not depend on the order the edges arrived in.
    fn join(&mut self, a: u32, b: u32) {
        let (a, b) = (self.root(a), self.root(b));
        if a == b {
            return;
        }
        let (low, high) = (a.min(b), a.max(b));
        self.parent[high as usize] = low;
    }
}

/// Read the whole index into memory, in file id order. Nothing comes back when
/// the search is stopped part way through.
fn load(conn: &Connection, cancel: &AtomicBool) -> Result<Option<Vec<Image>>> {
    let total: usize = conn
        .query_row("SELECT count(*) FROM indexed_images", [], |row| row.get::<_, i64>(0))
        .context("counting the indexed images")? as usize;

    let mut statement = conn.prepare(LOAD_IMAGES)?;
    let mut rows = statement.query([])?;
    let mut images: Vec<Image> = Vec::with_capacity(total);
    while let Some(row) = rows.next()? {
        if images.len() % 1024 == 0 && cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let packed: Vec<u8> = row.get(8)?;
        let Some(hashes) = fingerprint::unpack_hashes(&packed) else {
            continue;
        };
        let ring: Vec<u8> = row.get(9)?;
        let width = row.get::<_, i64>(2)? as u32;
        let height = row.get::<_, i64>(3)? as u32;
        let format: String = row.get(4)?;
        let channels = row.get::<_, i64>(5)? as u8;
        let size_bytes: i64 = row.get(6)?;
        let rel_path: String = row.get(1)?;

        images.push(Image {
            file_id: row.get(0)?,
            score: keep_score(
                width,
                height,
                parse_format(&format),
                channels,
                size_bytes,
                &rel_path,
            ),
            rel_path,
            width,
            height,
            format,
            channels,
            size_bytes,
            mtime_ns: row.get(7)?,
            variants: hashes.map(|hash| fingerprint::words(&hash)),
            bands: hashes.map(|hash| fingerprint::bands(&hash)),
            ring: fingerprint::ring_weighted(&ring),
        });
    }
    Ok(Some(images))
}

/// Find every duplicate set in the index.
pub fn find_sets(conn: &Connection, thresholds: Thresholds) -> Result<Vec<DuplicateSet>> {
    let never = AtomicBool::new(false);
    Ok(find_sets_cancellable(conn, thresholds, &never)?
        .expect("a search that is never cancelled cannot come back cancelled"))
}

/// Batches the comparing is cut into, so it spreads across the machine's cores.
const COMPARE_BATCHES: u64 = 32;

/// As `find_sets`, stopping when asked.
///
/// `cancel` is looked at between the pieces the work is cut into, which is often
/// enough that a search stops in a fraction of a second. Nothing comes back when
/// it is stopped: a half finished search has no answer to give.
pub fn find_sets_cancellable(
    conn: &Connection,
    thresholds: Thresholds,
    cancel: &AtomicBool,
) -> Result<Option<Vec<DuplicateSet>>> {
    let mut timing = Timing::new();
    let stopped = || cancel.load(Ordering::Relaxed);

    let Some(images) = load(conn, cancel)? else {
        return Ok(None);
    };
    timing.step("loading", images.len(), "images");
    if stopped() {
        return Ok(None);
    }

    let families = fold_identical(&images);
    let one_of_each: Vec<u32> = families.iter().map(|family| family[0]).collect();
    crate::runlog::line(&format!(
        "search folding: {} images are {} different pictures",
        images.len(),
        one_of_each.len()
    ));

    let by_band: Vec<Vec<(u32, u32)>> = (0..fingerprint::BANDS)
        .into_par_iter()
        .map(|band| {
            if stopped() {
                return Vec::new();
            }
            pairs_in_band(&images, &one_of_each, band)
        })
        .collect();
    if stopped() {
        return Ok(None);
    }

    let mut candidates: Vec<(u32, u32)> = by_band.concat();
    candidates.par_sort_unstable();
    candidates.dedup();
    timing.step("pairing", candidates.len(), "candidate pairs");

    let bounds: Vec<usize> = (0..=COMPARE_BATCHES)
        .map(|batch| (candidates.len() as u64 * batch / COMPARE_BATCHES) as usize)
        .collect();
    let matched: Vec<Vec<(u32, u32)>> = (0..COMPARE_BATCHES as usize)
        .into_par_iter()
        .map(|batch| {
            if stopped() {
                return Vec::new();
            }
            candidates[bounds[batch]..bounds[batch + 1]]
                .iter()
                .copied()
                .filter(|(a, b)| {
                    is_match(&images[*a as usize], &images[*b as usize], thresholds)
                })
                .collect::<Vec<(u32, u32)>>()
        })
        .collect();
    if stopped() {
        return Ok(None);
    }
    let matches = matched.concat();
    timing.step("comparing", matches.len(), "matches");

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
    timing.step("grouping", members.len(), "images in a set");

    let sets = build_sets(&images, &members);
    timing.step("listing", sets.len(), "sets");
    timing.total(sets.len());
    Ok(Some(sets))
}

/// Turn the grouped positions into the sets the caller sees: the best keeper
/// first, the rest by path, and the sets themselves in the order of the file that
/// names each one.
fn build_sets(images: &[Image], members: &[(u32, u32)]) -> Vec<DuplicateSet> {
    let mut grouped: Vec<(u32, Vec<u32>)> = Vec::new();
    for (at, position) in members {
        match grouped.last_mut() {
            Some((root, positions)) if root == at => positions.push(*position),
            _ => grouped.push((*at, vec![*position])),
        }
    }

    grouped
        .into_iter()
        .map(|(root, positions)| {
            // The best keeper: the most picture for the fewest bytes, and where
            // two are equal the older file and then the earlier path, so the same
            // folder always produces the same answer.
            let keeper = positions
                .iter()
                .copied()
                .max_by(|a, b| {
                    let (a, b) = (&images[*a as usize], &images[*b as usize]);
                    a.score
                        .partial_cmp(&b.score)
                        .expect("no NaN in a keep score")
                        .then(b.mtime_ns.cmp(&a.mtime_ns))
                        .then(b.rel_path.cmp(&a.rel_path))
                })
                .expect("a set has members");

            let mut members: Vec<Member> = positions
                .iter()
                .map(|position| {
                    let image = &images[*position as usize];
                    Member {
                        file_id: image.file_id,
                        rel_path: image.rel_path.clone(),
                        width: image.width,
                        height: image.height,
                        format: image.format.clone(),
                        channels: image.channels,
                        size_bytes: image.size_bytes,
                        mtime_ns: image.mtime_ns,
                        auto_keep: *position == keeper,
                    }
                })
                .collect();
            members.sort_by(|a, b| {
                b.auto_keep.cmp(&a.auto_keep).then(a.rel_path.cmp(&b.rel_path))
            });
            DuplicateSet { set_id: images[root as usize].file_id, members }
        })
        .collect()
}

/// What each step of a search cost, written to the run log. A search that is slow
/// on someone's folder is a fact about that folder, and the only way to know
/// which step it is spending the time in is for the run to say so.
struct Timing {
    started: std::time::Instant,
    step_started: std::time::Instant,
}

impl Timing {
    fn new() -> Self {
        let now = std::time::Instant::now();
        Timing { started: now, step_started: now }
    }

    /// What a step cost and how much work it handed on, so a search that is slow
    /// on someone's folder says which step it was and on how much.
    fn step(&mut self, name: &str, count: usize, of: &str) {
        let took = self.step_started.elapsed();
        self.step_started = std::time::Instant::now();
        crate::runlog::line(&format!(
            "search {name}: {:.2}s, {count} {of}",
            took.as_secs_f64()
        ));
    }

    fn total(&self, sets: usize) {
        crate::runlog::line(&format!(
            "search finished: {:.2}s, {sets} sets",
            self.started.elapsed().as_secs_f64()
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db;
    use crate::fingerprint::Fingerprint;
    use rusqlite::Connection;

    fn open() -> Connection {
        let conn = Connection::open_in_memory().expect("open");
        conn.pragma_update(None, "foreign_keys", "ON").unwrap();
        conn.execute_batch(db::SCHEMA).expect("schema");
        conn
    }

    /// A hash whose low bits carry the seed, so two seeds differ by a known
    /// number of bits and a threshold in a test means what it says.
    fn hash_seeded(seed: u64) -> fingerprint::Hash {
        let mut out = [0u8; fingerprint::HASH_BYTES];
        out[..8].copy_from_slice(&seed.to_le_bytes());
        out
    }

    fn insert(conn: &mut Connection, path: &str, seed: u64, width: u32, height: u32, size: i64, ring: Vec<u8>) {
        let hash = hash_seeded(seed);
        let record = db::Record {
            rel_path: path.to_string(),
            size_bytes: size,
            mtime_ns: 1,
            width,
            height,
            format: Format::Jpeg,
            channels: 3,
            fingerprint: Fingerprint {
                dct_hashes: [hash, hash, hash, hash, hash, hash, hash, hash],
                ring_stats: ring,
            },
        };
        let tx = conn.transaction().expect("tx");
        db::upsert(&tx, &record, 1).expect("upsert");
        tx.commit().expect("commit");
    }

    fn ring(value: f32) -> Vec<u8> {
        let mut out = Vec::new();
        for _ in 0..48 {
            out.extend_from_slice(&value.to_le_bytes());
        }
        out
    }

    /// A search can be stopped from another thread. It gives back nothing when it
    /// is: half a search has no answer, and reporting one would be a lie about
    /// which duplicates a folder holds.
    #[test]
    fn a_search_stops_when_it_is_told_to_and_gives_back_nothing() {
        let mut conn = open();
        for index in 0..40 {
            insert(&mut conn, &format!("{index}.jpg"), 0x1234, 800, 600, 100_000, ring(0.5));
        }

        let stop = AtomicBool::new(true);
        let outcome = find_sets_cancellable(&conn, Thresholds::preset("balanced"), &stop)
            .expect("the search failed rather than stopping");
        assert!(outcome.is_none(), "a stopped search still handed back sets");

        // And it is only stopped when it is asked: the same search finishes.
        let never = AtomicBool::new(false);
        let sets = find_sets_cancellable(&conn, Thresholds::preset("balanced"), &never)
            .expect("find")
            .expect("a search that was not stopped came back stopped");
        assert_eq!(sets.len(), 1, "the fixture stopped finding what it used to");
    }

    #[test]
    fn identical_hashes_land_in_one_set() {
        let mut conn = open();
        insert(&mut conn, "a.jpg", 0x1234, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "b.jpg", 0x1234, 800, 600, 90_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds::preset("balanced")).expect("find");
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].members.len(), 2);
    }

    #[test]
    fn unrelated_hashes_do_not_match() {
        let mut conn = open();
        insert(&mut conn, "a.jpg", 0x0000_0000_0000_0000, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "b.jpg", 0xFFFF_FFFF_FFFF_FFFF, 800, 600, 100_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds::preset("balanced")).expect("find");
        assert!(sets.is_empty(), "unrelated images were matched");
    }

    #[test]
    fn a_chain_of_matches_becomes_one_set() {
        // a matches b, b matches c, a does not match c directly.
        let mut conn = open();
        insert(&mut conn, "a.jpg", 0b0000, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "b.jpg", 0b0011, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "c.jpg", 0b1111, 800, 600, 100_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds { max_bits: 2, max_ring: 1.0, ignore_colour: true }).expect("find");
        assert_eq!(sets.len(), 1, "the chain did not collapse into one set");
        assert_eq!(sets[0].members.len(), 3);
    }

    #[test]
    fn the_colour_signature_can_split_a_pair_and_the_setting_can_rejoin_it() {
        let mut conn = open();
        insert(&mut conn, "colour.jpg", 0x1234, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "gray.jpg", 0x1234, 800, 600, 100_000, ring(0.0));

        let split = find_sets(&conn, Thresholds { max_bits: 6, max_ring: 0.05, ignore_colour: false }).expect("find");
        assert!(split.is_empty(), "the colour check did not separate them");

        let joined = find_sets(&conn, Thresholds { max_bits: 6, max_ring: 0.05, ignore_colour: true }).expect("find");
        assert_eq!(joined.len(), 1, "the setting did not rejoin them");
    }

    #[test]
    fn a_different_shape_is_not_a_duplicate() {
        let mut conn = open();
        insert(&mut conn, "wide.jpg", 0x1234, 1600, 400, 100_000, ring(0.5));
        insert(&mut conn, "square.jpg", 0x1234, 800, 800, 100_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds::preset("balanced")).expect("find");
        assert!(sets.is_empty(), "shapes that differ were matched");
    }

    #[test]
    fn the_balanced_threshold_covers_what_the_same_picture_actually_moves() {
        // Measured on a corpus: a resize or a recompression moves the hash by up
        // to about 6 percent and a rotation, which re-encodes on a shifted block
        // grid, by about 8. Setting this to the pigeonhole radius instead was
        // measured to reject half of all rotated duplicates.
        let balanced = Thresholds::preset("balanced");
        let eight_percent = (fingerprint::HASH_BITS as f64 * 0.08) as u32;
        assert!(
            balanced.max_bits >= eight_percent,
            "balanced allows {} bits, below the {eight_percent} a rotation moves",
            balanced.max_bits
        );
    }

    /// The presets widen in order. Unrelated pictures were measured above 25
    /// percent apart, so the last two are past that on purpose and the first two
    /// are not.
    #[test]
    fn the_presets_widen_in_order() {
        let bits: Vec<u32> =
            PRESETS.iter().map(|(_, percent)| Thresholds::at(*percent).max_bits).collect();
        assert!(bits.windows(2).all(|pair| pair[0] < pair[1]), "{bits:?} do not widen");

        let unrelated = (fingerprint::HASH_BITS as f64 * 0.25) as u32;
        assert!(
            Thresholds::preset("balanced").max_bits < unrelated,
            "balanced already reaches unrelated pictures"
        );
        assert!(
            Thresholds::preset("yolo").max_bits > unrelated,
            "yolo does not reach past what it is named for"
        );
    }

    /// Where the window starts, and where a new folder puts the slider back to,
    /// is the balanced preset and not a value of its own.
    #[test]
    fn the_default_is_the_balanced_preset() {
        let balanced =
            PRESETS.iter().find(|(name, _)| *name == "balanced").expect("a balanced preset");
        assert_eq!(balanced.1, DEFAULT_SENSITIVITY);
        assert_eq!(
            Thresholds::preset("balanced").max_bits,
            Thresholds::at(DEFAULT_SENSITIVITY).max_bits
        );
    }

    #[test]
    fn a_preset_that_is_not_one_lands_on_the_default() {
        assert_eq!(
            Thresholds::preset("something else").max_bits,
            Thresholds::at(DEFAULT_SENSITIVITY).max_bits
        );
    }

    #[test]
    fn a_threshold_can_be_set_anywhere_on_the_scale() {
        let strict = Thresholds::at(4.0);
        let between = Thresholds::at(7.0);
        let balanced = Thresholds::at(10.0);
        assert!(strict.max_bits < between.max_bits);
        assert!(between.max_bits < balanced.max_bits);
        assert!(strict.max_ring < between.max_ring);
    }

    /// A preset is a place on the slider, so the two can never disagree about
    /// what the search will use.
    #[test]
    fn the_presets_are_points_on_the_same_scale() {
        for (name, percent) in PRESETS {
            assert_eq!(Thresholds::preset(name).max_bits, Thresholds::at(percent).max_bits);
            assert!(
                (Thresholds::at(percent).percent() - percent).abs() < 1.0,
                "{name} does not round trip through the slider"
            );
        }
    }

    #[test]
    fn a_threshold_reports_where_it_sits() {
        for percent in [4.0, 10.0, 16.0] {
            let reported = Thresholds::at(percent).percent();
            assert!(
                (reported - percent).abs() < 1.0,
                "set {percent} percent, reported {reported}"
            );
        }
    }

    /// The scale stops at `MAX_SENSITIVITY` however far the control is dragged,
    /// and at nothing at the other end. The top of it is past the 25 percent
    /// unrelated pictures were measured at, so it reaches them on purpose.
    #[test]
    fn a_threshold_stops_at_the_ends_of_its_scale() {
        let widest = Thresholds::at(1000.0);
        assert_eq!(widest.max_bits, Thresholds::at(MAX_SENSITIVITY).max_bits);
        assert!(widest.max_bits < fingerprint::HASH_BITS as u32, "the whole hash may differ");
        let unrelated = (fingerprint::HASH_BITS as f64 * 0.25) as u32;
        assert!(widest.max_bits > unrelated, "the scale no longer reaches unrelated pictures");
        assert_eq!(Thresholds::at(-5.0).max_bits, 0);
    }

    #[test]
    fn a_rotation_keeps_the_same_shape() {
        assert!(aspect_ok(1600.0, 1200.0, 1200.0, 1600.0));
        assert!(aspect_ok(1600.0, 1200.0, 800.0, 600.0));
        assert!(!aspect_ok(1600.0, 1200.0, 1600.0, 400.0));
    }

    #[test]
    fn the_bigger_image_is_marked_to_keep() {
        let mut conn = open();
        insert(&mut conn, "small.jpg", 0x1234, 400, 300, 20_000, ring(0.5));
        insert(&mut conn, "big.jpg", 0x1234, 1600, 1200, 300_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds::preset("balanced")).expect("find");
        let kept: Vec<&Member> = sets[0].members.iter().filter(|m| m.auto_keep).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].rel_path, "big.jpg");
    }

    #[test]
    fn recoverable_bytes_counts_everything_but_the_keeper() {
        let mut conn = open();
        insert(&mut conn, "small.jpg", 0x1234, 400, 300, 20_000, ring(0.5));
        insert(&mut conn, "big.jpg", 0x1234, 1600, 1200, 300_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds::preset("balanced")).expect("find");
        assert_eq!(sets[0].recoverable_bytes(), 20_000);
    }

    /// The band index is the whole reason the search does not compare everything
    /// to everything: a value is a slice of an array, and the slice holds every
    /// variant of every image that reads as that value there.
    #[test]
    fn a_band_files_every_variant_of_every_image_under_its_value() {
        let mut conn = open();
        insert(&mut conn, "a.jpg", 0x1234, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "b.jpg", 0x1234, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "c.jpg", 0xFFFF_0000_FFFF_0000, 800, 600, 100_000, ring(0.5));
        let never = AtomicBool::new(false);
        let images = load(&conn, &never).expect("load").expect("not cancelled");

        let all: Vec<u32> = (0..images.len() as u32).collect();
        let index = BandIndex::build(&images, &all, 0);
        assert_eq!(
            index.entries.len(),
            images.len() * fingerprint::VARIANTS,
            "the index lost entries"
        );

        // The two that share a hash are filed together, and every one of the
        // eight variants of each is there.
        let shared = index.holders(images[0].bands[0][0]);
        assert!(shared.contains(&0) && shared.contains(&1), "the pair was not filed together");
        assert_eq!(
            shared.iter().filter(|position| **position == 0).count(),
            fingerprint::VARIANTS,
            "not every variant was filed"
        );
    }

    /// A folder holding many copies of one picture puts every one of them in the
    /// same band bucket, and a bucket is compared to itself. They are the same
    /// picture by every test the search makes, so one of them stands for all of
    /// them and the bucket holds one entry instead of a thousand.
    #[test]
    fn copies_of_one_picture_are_folded_to_a_single_entry() {
        let mut conn = open();
        for index in 0..50 {
            insert(&mut conn, &format!("copy{index}.jpg"), 0x1234, 800, 600, 100_000, ring(0.5));
        }
        insert(&mut conn, "other.jpg", 0xFFFF_FFFF_FFFF_FFFF, 800, 600, 100_000, ring(0.5));

        let never = AtomicBool::new(false);
        let images = load(&conn, &never).expect("load").expect("not cancelled");
        let families = fold_identical(&images);
        assert_eq!(families.len(), 2, "the copies were not folded together");
        assert_eq!(
            families.iter().map(|family| family.len()).max(),
            Some(50),
            "the fold lost copies"
        );

        // And every one of them still comes back as a duplicate of the rest.
        let sets = find_sets(&conn, Thresholds::preset("balanced")).expect("search");
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].members.len(), 50, "folding dropped copies from the set");
    }

    /// The fold is only allowed where nothing can tell the images apart. Two
    /// pictures with the same hash and a different shape are not duplicates, so
    /// they are not the same entry either.
    #[test]
    fn the_same_hash_at_a_different_shape_is_not_folded() {
        let mut conn = open();
        insert(&mut conn, "wide.jpg", 0x1234, 1600, 400, 100_000, ring(0.5));
        insert(&mut conn, "square.jpg", 0x1234, 800, 800, 100_000, ring(0.5));

        let never = AtomicBool::new(false);
        let images = load(&conn, &never).expect("load").expect("not cancelled");
        assert_eq!(fold_identical(&images).len(), 2, "two shapes were folded into one");
        assert!(find_sets(&conn, Thresholds::preset("balanced")).expect("search").is_empty());
    }

    /// The pigeonhole guarantee: inside the radius the bands are certain to put a
    /// pair together, so what the search finds is exactly what comparing every
    /// image to every other image finds. Anything the bands miss inside it is a
    /// duplicate the tool would never report.
    #[test]
    fn inside_the_guaranteed_radius_it_finds_what_comparing_everything_finds() {
        let mut conn = open();
        for index in 0..200u64 {
            // One in twenty is a near copy of the one before it, at a distance the
            // bands have to find, and one in twenty is an exact copy, which is the
            // one that gets folded away before the pairing. The rest are unrelated.
            let seed = match index % 20 {
                1 => (index - 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ 0b1010_1101,
                2 => (index - 2).wrapping_mul(0x9E37_79B9_7F4A_7C15),
                _ => index.wrapping_mul(0x9E37_79B9_7F4A_7C15),
            };
            insert(&mut conn, &format!("{index}.jpg"), seed, 800, 600, 100_000 + index as i64, ring(0.5));
        }

        let thresholds =
            Thresholds { max_bits: GUARANTEED_RADIUS, max_ring: 1.0, ignore_colour: true };
        let found = find_sets(&conn, thresholds).expect("search");

        let never = AtomicBool::new(false);
        let images = load(&conn, &never).expect("load").expect("not cancelled");
        let mut everything = Groups::new(images.len());
        let mut edges = 0;
        for a in 0..images.len() {
            for b in (a + 1)..images.len() {
                if is_match(&images[a], &images[b], thresholds) {
                    everything.join(a as u32, b as u32);
                    edges += 1;
                }
            }
        }
        assert!(edges > 0, "the fixture planted no duplicates at all");

        let mut expected: Vec<Vec<String>> = Vec::new();
        for root in 0..images.len() as u32 {
            let group: Vec<String> = (0..images.len() as u32)
                .filter(|position| everything.root(*position) == root)
                .map(|position| images[position as usize].rel_path.clone())
                .collect();
            if group.len() > 1 {
                expected.push(group);
            }
        }
        for group in &mut expected {
            group.sort();
        }
        expected.sort();

        let mut reported: Vec<Vec<String>> = found
            .iter()
            .map(|set| {
                let mut paths: Vec<String> =
                    set.members.iter().map(|member| member.rel_path.clone()).collect();
                paths.sort();
                paths
            })
            .collect();
        reported.sort();

        assert_eq!(reported, expected, "the band search and comparing everything disagree");
    }
}
