use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use anyhow::{Context, Result};
use rusqlite::functions::FunctionFlags;
use rusqlite::{params, Connection};

use crate::fingerprint;
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
/// apart, so past this the tool starts reporting them as duplicates.
pub const MAX_SENSITIVITY: f64 = 20.0;

impl Thresholds {
    /// Only files that are the same picture, allowing for a re-encode.
    pub fn strict() -> Self {
        Thresholds::at(4.0)
    }

    /// Resizes, recompressions, format changes and rotations.
    pub fn balanced() -> Self {
        Thresholds::at(10.0)
    }

    /// Also catches heavier edits, at the cost of more false pairs to reject by eye.
    pub fn loose() -> Self {
        Thresholds::at(16.0)
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

/// Register the scalar functions the matching and scoring queries call. They are
/// declared deterministic so SQLite can cache and hoist them.
pub fn register_functions(conn: &Connection) -> Result<()> {
    let flags = FunctionFlags::SQLITE_UTF8 | FunctionFlags::SQLITE_DETERMINISTIC;

    // One image's eight rotation and mirror hashes against another's indexed one.
    conn.create_scalar_function("hamming_any", 2, flags, |ctx| {
        let packed = ctx.get_raw(0).as_blob().unwrap_or(&[]).to_vec();
        let other = ctx.get_raw(1).as_blob().unwrap_or(&[]).to_vec();
        Ok(
            match (fingerprint::unpack_hashes(&packed), fingerprint::unpack_hash(&other)) {
                (Some(hashes), Some(other)) => fingerprint::hamming_any(&hashes, &other) as i64,
                _ => i64::MAX,
            },
        )
    })?;

    conn.create_scalar_function("ring_distance", 2, flags, |ctx| {
        let a = ctx.get_raw(0).as_blob().unwrap_or(&[]).to_vec();
        let b = ctx.get_raw(1).as_blob().unwrap_or(&[]).to_vec();
        Ok(fingerprint::ring_distance(&a, &b) as f64)
    })?;

    conn.create_scalar_function("aspect_ok", 4, flags, |ctx| {
        let w1 = ctx.get::<i64>(0)? as f64;
        let h1 = ctx.get::<i64>(1)? as f64;
        let w2 = ctx.get::<i64>(2)? as f64;
        let h2 = ctx.get::<i64>(3)? as f64;
        Ok(aspect_ok(w1, h1, w2, h2))
    })?;

    conn.create_scalar_function("keep_score", 6, flags, |ctx| {
        let width = ctx.get::<i64>(0)? as u32;
        let height = ctx.get::<i64>(1)? as u32;
        let format = parse_format(&ctx.get::<String>(2)?);
        let channels = ctx.get::<i64>(3)? as u8;
        let size_bytes = ctx.get::<i64>(4)?;
        let rel_path = ctx.get::<String>(5)?;
        Ok(keep_score(width, height, format, channels, size_bytes, &rel_path))
    })?;

    Ok(())
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

/// One side contributes only the variant the image was indexed under and the
/// other contributes all eight, which is enough to find a rotated pair and keeps
/// the join from producing every variant pairing.
const CREATE_CANDIDATES: &str = "
DROP TABLE IF EXISTS temp.candidate_rows;
CREATE TEMP TABLE candidate_rows(a INTEGER NOT NULL, b INTEGER NOT NULL);
";

/// The same join as `BUILD_CANDIDATES`, one band at a time, so there is something
/// to count while it runs.
///
/// The cut is by band because that is the leading column of `bands_lookup`, the
/// index this reads. One band is a range of that index and costs a sixteenth of
/// the scan. Cutting by file cannot restrict the scan at all: the index is not in
/// file order, so every piece reads all of it and discards what falls outside its
/// range, which measured as one whole pairing step per piece.
const PAIR_ONE_BAND: &str = "
INSERT INTO candidate_rows(a, b)
SELECT DISTINCT min(a.file_id, b.file_id), max(a.file_id, b.file_id)
FROM phash_bands a
JOIN phash_bands b
  ON b.band_index = a.band_index
 AND b.band_value = a.band_value
 AND b.file_id <> a.file_id
WHERE a.variant = 0 AND a.band_index = ?1
";

/// One sort over what the bands produced. A pair that collides in more than one
/// band is found once in each of them.
const DEDUPE_CANDIDATES: &str = "
DROP TABLE IF EXISTS temp.candidates;
CREATE TEMP TABLE candidates AS SELECT DISTINCT a, b FROM candidate_rows;
DROP TABLE temp.candidate_rows;
";

const CREATE_MATCHES: &str = "
DROP TABLE IF EXISTS temp.matches;
CREATE TEMP TABLE matches(a INTEGER NOT NULL, b INTEGER NOT NULL);
";

/// A range of the candidate pairs at a time. The pairs are a plain table, so a
/// range of its rowids is a range of the table and each piece reads only its own
/// share, which is what makes cutting this up cost nothing.
const COMPARE_SOME_PAIRS: &str = "
INSERT INTO matches(a, b)
SELECT c.a, c.b FROM candidates c
JOIN indexed_images ia ON ia.id = c.a
JOIN indexed_images ib ON ib.id = c.b
WHERE c.rowid > ?4 AND c.rowid <= ?5
  AND hamming_any(ia.dct_hashes, ib.dct_hash) <= ?1
  AND (?2 OR ring_distance(ia.ring_stats, ib.ring_stats) <= ?3)
  AND aspect_ok(ia.width, ia.height, ib.width, ib.height)
";

const BUILD_SETS: &str = "
DROP TABLE IF EXISTS temp.sets;
CREATE TEMP TABLE sets AS
WITH RECURSIVE
  edges(x, y) AS (SELECT a, b FROM matches UNION SELECT b, a FROM matches),
  reach(root, node) AS (
    SELECT x, x FROM edges
    UNION
    SELECT r.root, e.y FROM reach r JOIN edges e ON e.x = r.node
  )
SELECT node AS file_id, min(root) AS set_id FROM reach GROUP BY node;
";

const LIST_SETS: &str = "
SELECT set_id, file_id, rel_path, width, height, format, channels, size_bytes,
       mtime_ns, auto_keep
FROM (
  SELECT s.set_id, i.id AS file_id, i.rel_path, i.width, i.height, i.format,
         i.channels, i.size_bytes, i.mtime_ns,
         row_number() OVER (
           PARTITION BY s.set_id
           ORDER BY keep_score(i.width, i.height, i.format, i.channels,
                               i.size_bytes, i.rel_path) DESC,
                    i.mtime_ns ASC, i.rel_path ASC
         ) = 1 AS auto_keep
  FROM sets s JOIN indexed_images i ON i.id = s.file_id
)
ORDER BY set_id, auto_keep DESC, rel_path;
";

/// Find every duplicate set in the index. The work happens in SQLite; nothing is
/// compared in application code.
pub fn find_sets(conn: &Connection, thresholds: Thresholds) -> Result<Vec<DuplicateSet>> {
    let never = AtomicBool::new(false);
    Ok(find_sets_reporting(conn, thresholds, &never, &|_| {})?
        .expect("a search that is never cancelled cannot come back cancelled"))
}

/// How much of a search has happened: the step running, and how many of its
/// units of work are behind it.
///
/// Pairing is cut into slices of files so there is something real to count while
/// it runs. What each slice costs goes to the run log, so a search that is slow
/// on a folder says where the time went rather than leaving it to be guessed at.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub step: Step,
    pub done: u64,
    pub total: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Pairing,
    Comparing,
    Grouping,
    Listing,
}

impl Step {
    pub fn label(self) -> &'static str {
        match self {
            Step::Pairing => "pairing",
            Step::Comparing => "comparing",
            Step::Grouping => "grouping",
            Step::Listing => "listing",
        }
    }
}

/// Pieces the comparing step is cut into. A fixed count rather than a fixed size,
/// so how far the whole search has to go is known before it starts.
const COMPARE_BATCHES: u64 = 32;

/// Every piece of the search: one per band, the comparing batches, the grouping
/// and the listing.
const TOTAL_PIECES: u64 = fingerprint::BANDS as u64 + COMPARE_BATCHES + 2;

/// As `find_sets`, reporting how far it has got and stopping when asked.
///
/// `cancel` is looked at between the pieces the work is cut into, which is often
/// enough that a search stops in a fraction of a second. Nothing comes back when
/// it is stopped: a half finished search has no answer to give.
pub fn find_sets_reporting(
    conn: &Connection,
    thresholds: Thresholds,
    cancel: &AtomicBool,
    report: &dyn Fn(Progress),
) -> Result<Option<Vec<DuplicateSet>>> {
    let mut timing = Timing::new(conn);
    // Both long steps are cut into a number of pieces settled before either of
    // them starts, so the bar knows what it is counting towards from the first
    // frame and never has to revise it downwards.
    let mut done = 0u64;
    let step_on = |step: Step, at: u64| Progress { step, done: at, total: TOTAL_PIECES };
    let stopped = || cancel.load(Ordering::Relaxed);

    conn.execute_batch(CREATE_CANDIDATES).context("preparing the candidate pairs")?;
    timing.plan(PAIR_ONE_BAND, "one band of pairing");
    report(step_on(Step::Pairing, done));
    {
        let mut pair = conn.prepare(PAIR_ONE_BAND)?;
        for band in 0..fingerprint::BANDS as i64 {
            if stopped() {
                return Ok(None);
            }
            let started = std::time::Instant::now();
            pair.execute(params![band]).context("building candidate pairs")?;
            timing.slice(started.elapsed());
            done += 1;
            report(step_on(Step::Pairing, done));
        }
    }
    if stopped() {
        return Ok(None);
    }
    conn.execute_batch(DEDUPE_CANDIDATES).context("collecting the candidate pairs")?;
    timing.slices("pairing");
    timing.step("pairing", "candidates");

    let pairs: i64 =
        conn.query_row("SELECT coalesce(max(rowid), 0) FROM candidates", [], |row| row.get(0))?;
    conn.execute_batch(CREATE_MATCHES).context("preparing the matches")?;
    timing.plan(COMPARE_SOME_PAIRS, "one batch of comparing");
    {
        let mut compare = conn.prepare(COMPARE_SOME_PAIRS)?;
        for batch in 0..COMPARE_BATCHES {
            if stopped() {
                return Ok(None);
            }
            let from = pairs * batch as i64 / COMPARE_BATCHES as i64;
            let to = pairs * (batch as i64 + 1) / COMPARE_BATCHES as i64;
            let started = std::time::Instant::now();
            compare
                .execute(params![
                    thresholds.max_bits as i64,
                    thresholds.ignore_colour as i64,
                    thresholds.max_ring as f64,
                    from,
                    to
                ])
                .context("verifying candidate pairs")?;
            timing.slice(started.elapsed());
            done += 1;
            report(step_on(Step::Comparing, done));
        }
    }
    timing.slices("comparing");
    timing.step("comparing", "matches");

    if stopped() {
        return Ok(None);
    }
    conn.execute_batch(BUILD_SETS).context("clustering matches into sets")?;
    done += 1;
    report(step_on(Step::Grouping, done));
    timing.step("grouping", "sets");

    let mut statement = conn.prepare(LIST_SETS)?;
    let rows = statement.query_map([], |row| {
        Ok((
            row.get::<_, i64>(0)?,
            Member {
                file_id: row.get(1)?,
                rel_path: row.get(2)?,
                width: row.get::<_, i64>(3)? as u32,
                height: row.get::<_, i64>(4)? as u32,
                format: row.get(5)?,
                channels: row.get::<_, i64>(6)? as u8,
                size_bytes: row.get(7)?,
                mtime_ns: row.get(8)?,
                auto_keep: row.get::<_, i64>(9)? != 0,
            },
        ))
    })?;

    let mut sets: Vec<DuplicateSet> = Vec::new();
    for row in rows {
        let (set_id, member) = row?;
        match sets.last_mut() {
            Some(last) if last.set_id == set_id => last.members.push(member),
            _ => sets.push(DuplicateSet { set_id, members: vec![member] }),
        }
    }
    done += 1;
    report(step_on(Step::Listing, done));
    timing.step("listing", "sets");
    timing.total(sets.len());
    Ok(Some(sets))
}

/// What each step of a search cost, written to the run log. A search that is slow
/// on someone's folder is a fact about that folder, and the only way to know
/// which step it is spending the time in is for the run to say so.
struct Timing<'a> {
    conn: &'a Connection,
    started: std::time::Instant,
    step_started: std::time::Instant,
    slices: Vec<std::time::Duration>,
}

impl<'a> Timing<'a> {
    fn new(conn: &'a Connection) -> Self {
        let now = std::time::Instant::now();
        Timing { conn, started: now, step_started: now, slices: Vec::new() }
    }

    /// What SQLite decided to do with a statement. A statement cut into pieces can
    /// be given a different plan from the whole one, and that is the first thing
    /// to look at when the pieces cost more than the whole.
    fn plan(&self, sql: &str, name: &str) {
        let plan = self.conn.prepare(&format!("EXPLAIN QUERY PLAN {sql}")).and_then(|mut stmt| {
            let rows = stmt.query_map([], |row| row.get::<_, String>(3))?;
            rows.collect::<Result<Vec<String>, _>>()
        });
        match plan {
            Ok(lines) => crate::runlog::line(&format!("plan for {name}: {}", lines.join(" | "))),
            Err(err) => crate::runlog::line(&format!("plan for {name} unavailable: {err}")),
        }
    }

    fn slice(&mut self, took: std::time::Duration) {
        self.slices.push(took);
    }

    /// What the pieces cost, so a step that is slow because of one bad slice is
    /// told apart from one that is slow all the way through.
    fn slices(&mut self, name: &str) {
        if self.slices.is_empty() {
            return;
        }
        let mut sorted: Vec<f64> = self.slices.iter().map(|t| t.as_secs_f64()).collect();
        sorted.sort_by(|a, b| a.partial_cmp(b).expect("no NaN"));
        let total: f64 = sorted.iter().sum();
        let count = sorted.len();
        crate::runlog::line(&format!(
            "{name} slices: {count} in {total:.2}s, fastest {:.3}s, median {:.3}s, \
             slowest {:.3}s, slowest five {:?}",
            sorted[0],
            sorted[count / 2],
            sorted[count - 1],
            sorted.iter().rev().take(5).map(|t| (t * 1000.0).round() / 1000.0).collect::<Vec<_>>()
        ));
        self.slices.clear();
    }

    /// `table` is the temporary table the step fills, counted so the log says how
    /// much work the next step was handed.
    fn step(&mut self, name: &str, table: &str) {
        let took = self.step_started.elapsed();
        self.step_started = std::time::Instant::now();
        let rows: i64 = self
            .conn
            .query_row(&format!("SELECT count(*) FROM temp.{table}"), [], |row| row.get(0))
            .unwrap_or(-1);
        crate::runlog::line(&format!("search {name}: {:.2}s, {rows} rows", took.as_secs_f64()));
    }

    fn total(&self, sets: usize) {
        crate::runlog::line(&format!(
            "search finished: {:.2}s, {sets} sets",
            self.started.elapsed().as_secs_f64()
        ));
    }
}

/// Paths that share a size and a content hash, which is a candidate for being
/// byte-identical and not yet a verdict.
pub fn exact_groups(conn: &Connection) -> Result<Vec<Vec<(i64, String)>>> {
    let mut statement = conn.prepare(
        "SELECT f.size_bytes, f.bytes_hash, f.id, f.rel_path
         FROM files f
         WHERE (f.size_bytes, f.bytes_hash) IN (
             SELECT size_bytes, bytes_hash FROM files
             GROUP BY size_bytes, bytes_hash HAVING count(*) > 1
         )
         ORDER BY f.size_bytes, f.bytes_hash, f.rel_path",
    )?;
    let rows = statement.query_map([], |row| {
        Ok((
            (row.get::<_, i64>(0)?, row.get::<_, i64>(1)?),
            (row.get::<_, i64>(2)?, row.get::<_, String>(3)?),
        ))
    })?;

    let mut groups: Vec<Vec<(i64, String)>> = Vec::new();
    let mut current_key: Option<(i64, i64)> = None;
    for row in rows {
        let (key, entry) = row?;
        if Some(key) == current_key {
            groups.last_mut().expect("a group is open").push(entry);
        } else {
            current_key = Some(key);
            groups.push(vec![entry]);
        }
    }
    Ok(groups)
}

/// A 32-bit hash is a bucketing key, not a verdict, so a group is only reported
/// as byte-identical after the files have actually been compared.
pub fn verified_identical(root: &Path, group: &[(i64, String)]) -> Result<Vec<Vec<i64>>> {
    let mut buckets: Vec<(Vec<u8>, Vec<i64>)> = Vec::new();
    for (id, rel_path) in group {
        let bytes = match std::fs::read(root.join(rel_path)) {
            Ok(bytes) => bytes,
            Err(_) => continue,
        };
        match buckets.iter_mut().find(|(known, _)| *known == bytes) {
            Some((_, ids)) => ids.push(*id),
            None => buckets.push((bytes, vec![*id])),
        }
    }
    Ok(buckets
        .into_iter()
        .map(|(_, ids)| ids)
        .filter(|ids| ids.len() > 1)
        .collect())
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
        register_functions(&conn).expect("functions");
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
            bytes_hash: 7,
            width,
            height,
            format: Format::Jpeg,
            channels: 3,
            fingerprint: Fingerprint {
                dct_hash: hash,
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

    /// The bar has to move through the longest step, not sit at one place per
    /// band while a folder of thousands is paired.
    /// A search can be stopped from another thread. It gives back nothing when it
    /// is: half a search has no answer, and reporting one would be a lie about
    /// which duplicates a folder holds.
    #[test]
    fn a_search_stops_when_it_is_told_to_and_gives_back_nothing() {
        let mut conn = open();
        for index in 0..40 {
            insert(&mut conn, &format!("{index}.jpg"), 0x1234, 800, 600, 100_000, ring(0.5));
        }

        // Stopped as soon as it says it has started.
        let stop = AtomicBool::new(false);
        let seen = std::cell::RefCell::new(0);
        let outcome = find_sets_reporting(&conn, Thresholds::balanced(), &stop, &|_| {
            *seen.borrow_mut() += 1;
            stop.store(true, Ordering::Relaxed);
        })
        .expect("the search failed rather than stopping");

        assert!(outcome.is_none(), "a stopped search still handed back sets");
        assert!(
            *seen.borrow() < TOTAL_PIECES as usize,
            "it reported every piece, so it ran to the end anyway"
        );

        // And it is only stopped when it is asked: the same search finishes.
        let never = AtomicBool::new(false);
        let sets = find_sets_reporting(&conn, Thresholds::balanced(), &never, &|_| {})
            .expect("find")
            .expect("a search that was not stopped came back stopped");
        assert_eq!(sets.len(), 1, "the fixture stopped finding what it used to");
    }

    /// Every step says when it starts, in order, and the last one says when the
    /// search is over. This is all a caller can be told: the steps are single SQL
    /// statements and cutting them up to count them made the search slower.
    #[test]
    fn a_search_says_which_of_its_steps_is_running() {
        let files = 40;
        let mut conn = open();
        for index in 0..files {
            let seed = if index % 2 == 0 { 0x1234 } else { index as u64 * 7919 };
            insert(&mut conn, &format!("{index}.jpg"), seed, 800, 600, 100_000, ring(0.5));
        }

        let seen = std::cell::RefCell::new(Vec::new());
        let never = AtomicBool::new(false);
        let sets = find_sets_reporting(&conn, Thresholds::balanced(), &never, &|progress| {
            seen.borrow_mut().push(progress);
        })
        .expect("find")
        .expect("a search that was not cancelled came back cancelled");
        let seen = seen.into_inner();

        let order = [Step::Pairing, Step::Comparing, Step::Grouping, Step::Listing];
        let steps: Vec<Step> = order
            .iter()
            .copied()
            .filter(|step| seen.iter().any(|progress| progress.step == *step))
            .collect();
        assert_eq!(steps, order, "a step never reported anything");

        // One piece at a time, from nothing to all of it, and the count is the
        // same from the first report to the last so the bar never jumps back.
        let counted: Vec<u64> = seen.iter().map(|progress| progress.done).collect();
        assert_eq!(counted, (0..=TOTAL_PIECES).collect::<Vec<_>>(), "the pieces did not add up");
        assert!(seen.iter().all(|progress| progress.total == TOTAL_PIECES));

        let mut at = 0;
        for progress in &seen {
            let place = order.iter().position(|step| *step == progress.step).expect("a step");
            assert!(place >= at, "the steps came back out of order at {progress:?}");
            at = place;
        }

        let reported: Vec<DuplicateSet> = sets;
        let plain = find_sets(&conn, Thresholds::balanced()).expect("find");
        assert_eq!(
            reported.iter().map(|set| set.members.len()).collect::<Vec<_>>(),
            plain.iter().map(|set| set.members.len()).collect::<Vec<_>>(),
            "reporting changed what the search found"
        );
    }

    #[test]
    fn identical_hashes_land_in_one_set() {
        let mut conn = open();
        insert(&mut conn, "a.jpg", 0x1234, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "b.jpg", 0x1234, 800, 600, 90_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds::balanced()).expect("find");
        assert_eq!(sets.len(), 1);
        assert_eq!(sets[0].members.len(), 2);
    }

    #[test]
    fn unrelated_hashes_do_not_match() {
        let mut conn = open();
        insert(&mut conn, "a.jpg", 0x0000_0000_0000_0000, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "b.jpg", 0xFFFF_FFFF_FFFF_FFFF, 800, 600, 100_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds::balanced()).expect("find");
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
        let sets = find_sets(&conn, Thresholds::balanced()).expect("find");
        assert!(sets.is_empty(), "shapes that differ were matched");
    }

    #[test]
    fn the_balanced_threshold_covers_what_the_same_picture_actually_moves() {
        // Measured on a corpus: a resize or a recompression moves the hash by up
        // to about 6 percent and a rotation, which re-encodes on a shifted block
        // grid, by about 8. Setting this to the pigeonhole radius instead was
        // measured to reject half of all rotated duplicates.
        let balanced = Thresholds::balanced();
        let eight_percent = (fingerprint::HASH_BITS as f64 * 0.08) as u32;
        assert!(
            balanced.max_bits >= eight_percent,
            "balanced allows {} bits, below the {eight_percent} a rotation moves",
            balanced.max_bits
        );
    }

    #[test]
    fn the_presets_widen_in_order_and_stay_well_below_unrelated() {
        let strict = Thresholds::strict();
        let balanced = Thresholds::balanced();
        let loose = Thresholds::loose();
        assert!(strict.max_bits < balanced.max_bits);
        assert!(balanced.max_bits < loose.max_bits);
        // Unrelated pictures were measured above 25 percent apart.
        let unrelated = (fingerprint::HASH_BITS as f64 * 0.25) as u32;
        assert!(loose.max_bits < unrelated, "the loosest preset reaches unrelated pictures");
    }

    #[test]
    fn a_threshold_can_be_set_anywhere_between_the_presets() {
        let strict = Thresholds::at(4.0);
        let between = Thresholds::at(7.0);
        let balanced = Thresholds::at(10.0);
        assert!(strict.max_bits < between.max_bits);
        assert!(between.max_bits < balanced.max_bits);
        assert!(strict.max_ring < between.max_ring);
    }

    #[test]
    fn the_presets_are_points_on_the_same_scale() {
        assert_eq!(Thresholds::strict().max_bits, Thresholds::at(4.0).max_bits);
        assert_eq!(Thresholds::balanced().max_bits, Thresholds::at(10.0).max_bits);
        assert_eq!(Thresholds::loose().max_bits, Thresholds::at(16.0).max_bits);
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

    #[test]
    fn a_threshold_cannot_be_set_where_unrelated_pictures_match() {
        // Unrelated pictures were measured above 25 percent apart, so the scale
        // stops well short of that however far the control is dragged.
        let widest = Thresholds::at(1000.0);
        assert_eq!(widest.max_bits, Thresholds::at(MAX_SENSITIVITY).max_bits);
        let unrelated = (fingerprint::HASH_BITS as f64 * 0.25) as u32;
        assert!(widest.max_bits < unrelated);
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
        let sets = find_sets(&conn, Thresholds::balanced()).expect("find");
        let kept: Vec<&Member> = sets[0].members.iter().filter(|m| m.auto_keep).collect();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].rel_path, "big.jpg");
    }

    #[test]
    fn recoverable_bytes_counts_everything_but_the_keeper() {
        let mut conn = open();
        insert(&mut conn, "small.jpg", 0x1234, 400, 300, 20_000, ring(0.5));
        insert(&mut conn, "big.jpg", 0x1234, 1600, 1200, 300_000, ring(0.5));
        let sets = find_sets(&conn, Thresholds::balanced()).expect("find");
        assert_eq!(sets[0].recoverable_bytes(), 20_000);
    }

    #[test]
    fn the_candidate_query_uses_the_band_index() {
        let mut conn = open();
        insert(&mut conn, "a.jpg", 0x1234, 800, 600, 100_000, ring(0.5));
        let plan: Vec<String> = conn
            .prepare(
                "EXPLAIN QUERY PLAN
                 SELECT DISTINCT a.file_id, b.file_id
                 FROM phash_bands a JOIN phash_bands b
                   ON b.band_index = a.band_index AND b.band_value = a.band_value
                  AND b.file_id <> a.file_id
                 WHERE a.variant = 0",
            )
            .unwrap()
            .query_map([], |row| row.get::<_, String>(3))
            .unwrap()
            .map(|row| row.unwrap())
            .collect();
        let joined = plan.join(" | ");
        assert!(
            joined.contains("bands_lookup"),
            "the candidate join did not use the band index: {joined}"
        );
    }

    #[test]
    fn exact_groups_are_grouped_by_size_and_hash() {
        let mut conn = open();
        insert(&mut conn, "a.jpg", 1, 800, 600, 100, ring(0.5));
        insert(&mut conn, "b.jpg", 2, 800, 600, 100, ring(0.5));
        insert(&mut conn, "c.jpg", 3, 800, 600, 999, ring(0.5));
        let groups = exact_groups(&conn).expect("groups");
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].len(), 2);
    }

    #[test]
    fn verification_splits_a_group_whose_bytes_differ() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.bin"), b"same").unwrap();
        std::fs::write(dir.path().join("b.bin"), b"same").unwrap();
        std::fs::write(dir.path().join("c.bin"), b"diff").unwrap();
        let group = vec![
            (1, "a.bin".to_string()),
            (2, "b.bin".to_string()),
            (3, "c.bin".to_string()),
        ];
        let verified = verified_identical(dir.path(), &group).expect("verify");
        assert_eq!(verified, vec![vec![1, 2]]);
    }
}
