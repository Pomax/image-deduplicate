use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::Connection;

use crate::features;
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
    /// Look for pictures that fill the frame the same way: the hash and the
    /// colour signature. This is what finds a resize, a recompression or a
    /// rotation, and it costs almost nothing.
    pub whole_frame: bool,
    /// Look for one picture inside another: the corners. This is what finds a
    /// crop, and it is most of what a search spends its time on.
    pub corners: bool,
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
            whole_frame: true,
            corners: true,
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
        "tiff" => Format::Tiff,
        "heic" => Format::Heic,
        "cr2" => Format::Cr2,
        "cr3" => Format::Cr3,
        "nef" => Format::Nef,
        "arw" => Format::Arw,
        "rw2" => Format::Rw2,
        _ => Format::Jpeg,
    }
}

/// Everything about one image the comparison needs, in the form it compares in.
///
/// The stored blobs are turned into machine words and pre-weighted floats once,
/// here. A folder produces far more comparisons than it has images, and unpacking
/// the same blob on each of them is the whole cost of the old approach.
/// One picture as the search needs it: everything a comparison asks about it,
/// worked out once.
///
/// This is what the index is for. It is read out of the database once, and every
/// search after that runs over these and touches no storage at all, so changing
/// the sensitivity and looking again costs the comparing and nothing else.
#[derive(Debug)]
pub struct Image {
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
    /// The picture's corners and what each one looks like. Empty for a picture
    /// with nothing corner-shaped in it, and for one indexed before this build.
    corners: Vec<features::Keypoint>,
}

const LOAD_IMAGES: &str = "
SELECT id, rel_path, width, height, format, channels, size_bytes, mtime_ns,
       dct_hashes, ring_stats, corners
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

/// Corners of one picture the quick look tries, against all of the other's. The
/// strongest, because those are the ones another picture of the same thing also
/// picked out.
const QUICK_CORNERS: usize = 48;

/// How alike two descriptions have to be for the quick look to count them.
///
/// Tight, and that is the whole trick. Loosely alike means nothing: with three
/// hundred corners to choose from, almost every corner of an unrelated
/// photograph has one within a quarter of its bits somewhere, and measured that
/// way unrelated pictures scored forty-one out of forty-eight. Within a
/// twelfth of the bits, unrelated pictures scored three and a picture against a
/// crop of itself scored thirty-five.
const QUICK_BITS: u32 = 24;

/// Corners that have to look alike before a pair is worth the real test. Three
/// was the most any of a hundred and ninety unrelated pairs managed.
const QUICK_AGREEING: usize = 3;

/// Corners of a picture the index holds, strongest first. A picture asks with
/// its strongest few dozen and is answered from these.
const INDEXED_CORNERS: usize = 128;

/// Ways the index files one description.
///
/// A filing is a sample of the description's bits. Two descriptions differing
/// in a twelfth of their bits land under the same value about a quarter of the
/// time, so several filings put them together nine times in ten, which is the
/// banding of the hash index applied to a corner. Sampled bits rather than a
/// fixed slice, because a fixed slice is precisely what fails when the
/// difference happens to fall inside it.
const CORNER_TABLES: usize = 16;

/// Pictures the shortlist gets through between saying so. Often enough that the
/// bar moves on a small folder, rarely enough that saying so costs nothing.
const SHORTLIST_REPORT_EVERY: u64 = 16;

/// Corners a bucket is meant to hold, which is what one look at one table reads.
/// The number of buckets follows the folder to keep it there, so a folder ten
/// times the size is ten times the buckets and the same reading per picture.
/// That is what makes this pass grow with the folder rather than its square.
const BUCKET_TARGET: usize = 96;

/// Bits a table may sample, at fewest and at most. The floor is a small folder,
/// where buckets are nearly empty anyway; the ceiling is where the bucket ends
/// are more memory than the entries in them.
const FEWEST_TABLE_BITS: u32 = 6;
const MOST_TABLE_BITS: u32 = 20;

/// How many times the average a bucket may hold before it is skipped as saying
/// nothing. A description shared by that many corners does not tell one picture
/// from another, and reading those buckets was measured to be nineteen
/// twentieths of the pass and none of the answer.
const BUCKET_LIMIT: usize = 32;

/// Room in an entry for which corner of the picture it is, leaving the rest of
/// the entry for which picture. Seven bits is [`INDEXED_CORNERS`], and the
/// twenty five left over are more pictures than a folder can hold.
const CORNER_ROOM: u32 = 7;

const _: () = assert!(QUICK_CORNERS <= u64::BITS as usize);
const _: () = assert!(INDEXED_CORNERS <= 1 << CORNER_ROOM);

/// The bit positions each table samples, most it could need first. Fixed, so a
/// folder is filed the same way on every run and on every machine.
static CORNER_BITS: std::sync::LazyLock<[[u8; MOST_TABLE_BITS as usize]; CORNER_TABLES]> =
    std::sync::LazyLock::new(|| {
        let mut state = 0x2f6e_2b1d_9a37_c5e1_u64;
        let mut next = move || {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (state >> 33) as usize
        };
        let mut tables = [[0u8; MOST_TABLE_BITS as usize]; CORNER_TABLES];
        for table in tables.iter_mut() {
            let mut taken = [false; features::DESCRIPTOR_BITS];
            for place in table.iter_mut() {
                loop {
                    let bit = next() % features::DESCRIPTOR_BITS;
                    if !taken[bit] {
                        taken[bit] = true;
                        *place = bit as u8;
                        break;
                    }
                }
            }
        }
        tables
    });

/// The value a description files under in one table: its sampled bits, in order.
fn corner_key(descriptor: &[u8; features::DESCRIPTOR_BYTES], bits: &[u8]) -> usize {
    let mut key = 0;
    for (place, bit) in bits.iter().enumerate() {
        let set = descriptor[(bit / 8) as usize] >> (bit % 8) & 1;
        key |= (set as usize) << place;
    }
    key
}

/// Bits a table samples for a folder holding this many corners: enough buckets
/// to leave [`BUCKET_TARGET`] of them in each.
fn table_bits(entries: usize) -> u32 {
    let wanted = (entries / BUCKET_TARGET).max(1).next_power_of_two().trailing_zeros();
    wanted.clamp(FEWEST_TABLE_BITS, MOST_TABLE_BITS)
}

/// One filing of every corner in the folder: buckets of entries, an entry being
/// a picture and which of its corners this is.
struct CornerTable {
    bits: Vec<u8>,
    /// Bucket `k` is `entries[starts[k]..starts[k + 1]]`.
    starts: Vec<u32>,
    entries: Vec<u32>,
}

impl CornerTable {
    fn of(images: &[Image], of: &[u32], sampling: &[u8], bits: u32) -> Self {
        let bits = sampling[..bits as usize].to_vec();
        let mut starts = vec![0u32; (1 << bits.len()) + 1];
        for id in of {
            for corner in images[*id as usize].corners.iter().take(INDEXED_CORNERS) {
                starts[corner_key(&corner.descriptor, &bits) + 1] += 1;
            }
        }
        for k in 1..starts.len() {
            starts[k] += starts[k - 1];
        }
        let mut filled = starts.clone();
        let mut entries = vec![0u32; *starts.last().unwrap_or(&0) as usize];
        for id in of {
            let corners = images[*id as usize].corners.iter().take(INDEXED_CORNERS);
            for (which, corner) in corners.enumerate() {
                let k = corner_key(&corner.descriptor, &bits);
                entries[filled[k] as usize] = (id << CORNER_ROOM) | which as u32;
                filled[k] += 1;
            }
        }
        CornerTable { bits, starts, entries }
    }

    fn bucket(&self, descriptor: &[u8; features::DESCRIPTOR_BYTES]) -> &[u32] {
        let k = corner_key(descriptor, &self.bits);
        &self.entries[self.starts[k] as usize..self.starts[k + 1] as usize]
    }
}

/// Pairs of pictures worth comparing corner by corner.
///
/// A picture is worth comparing against another when several of its strongest
/// corners are described almost exactly as one of the other's is. Finding those
/// by looking at every pair is quadratic, and a folder of ten thousand pictures
/// is forty five million pairs, which is minutes of work for an answer about a
/// few hundred of them.
///
/// So the corners are filed instead. A description is a string of bits where
/// near means alike, which is what the hash index already exploits per picture:
/// file each corner under a sample of its bits, several samples over, and two
/// descriptions a few bits apart share a bucket in at least one filing. A
/// picture then reads a few hundred corners rather than the folder's three
/// million, and the pass stops growing with the square of the folder.
///
/// Nothing here is learned from the folder: the samples are fixed and a
/// picture's own corners are its own.
fn pairs_by_corner(
    images: &[Image],
    of: &[u32],
    stopped: &(dyn Fn() -> bool + Sync),
    report: &(dyn Fn(Progress) + Sync),
) -> Vec<(u32, u32)> {
    // An entry is a picture and one of its corners in one number, which stops
    // being true at a folder of thirty three million pictures. Nothing else
    // here works at that size either.
    if images.len() >= 1 << (u32::BITS - CORNER_ROOM) {
        return Vec::new();
    }
    let filed: usize =
        of.iter().map(|id| images[*id as usize].corners.len().min(INDEXED_CORNERS)).sum();
    let bits = table_bits(filed);
    let most_in_a_bucket = (filed >> bits).max(1) * BUCKET_LIMIT;
    let tables: Vec<CornerTable> =
        CORNER_BITS.par_iter().map(|s| CornerTable::of(images, of, s, bits)).collect();
    crate::log_line!(
        "corner index: {} corners over {} tables of {} buckets, skipping past {}",
        filed,
        CORNER_TABLES,
        1 << bits,
        most_in_a_bucket
    );
    let looked = AtomicU64::new(0);
    report(Progress::Shortlisting { done: 0, total: of.len() as u64 });
    of.par_iter()
        .flat_map_iter(|id| {
            let mut agreeing: std::collections::HashMap<u32, u64> =
                std::collections::HashMap::new();
            let done = looked.fetch_add(1, Ordering::Relaxed) + 1;
            if done % SHORTLIST_REPORT_EVERY == 0 || done == of.len() as u64 {
                report(Progress::Shortlisting { done, total: of.len() as u64 });
            }
            if !stopped() {
                let asking = images[*id as usize].corners.iter().take(QUICK_CORNERS);
                for (place, corner) in asking.enumerate() {
                    for table in &tables {
                        let bucket = table.bucket(&corner.descriptor);
                        // A description thousands of pictures share tells
                        // nothing about which picture this is. Reading such a
                        // bucket is most of the work and none of the answer:
                        // measured on ten thousand photographs, the fullest
                        // bucket held two percent of every corner in the folder
                        // and the pass spent nineteen twentieths of its time in
                        // buckets like it.
                        if bucket.len() > most_in_a_bucket {
                            continue;
                        }
                        for entry in bucket {
                            let other = entry >> CORNER_ROOM;
                            if other == *id {
                                continue;
                            }
                            let which = (entry & ((1 << CORNER_ROOM) - 1)) as usize;
                            let found = &images[other as usize].corners[which];
                            if features::distance(&corner.descriptor, &found.descriptor)
                                <= QUICK_BITS
                            {
                                *agreeing.entry(other).or_insert(0) |= 1 << place;
                            }
                        }
                    }
                }
            }
            let id = *id;
            agreeing
                .into_iter()
                .filter(|(_, alike)| alike.count_ones() as usize >= QUICK_AGREEING)
                .map(move |(other, _)| (id.min(other), id.max(other)))
        })
        .collect()
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
    (thresholds.whole_frame && whole_frame_match(a, b, thresholds))
        || (thresholds.corners && same_picture_inside(a, b))
}

/// The two pictures are the same picture filling the frame the same way: a
/// resize, a recompression, a rotation. Cheap, and it is most of what a folder
/// of duplicates holds.
fn whole_frame_match(a: &Image, b: &Image, thresholds: Thresholds) -> bool {
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

/// One picture is the other, or part of it.
///
/// Enough of the corners describe the same things, arranged the same way, which
/// a crop of a picture keeps and a different picture of the same subject does
/// not. This is what the whole-frame hash cannot see: crop a picture and every
/// number in that hash changes at once, while its corners stay where they were.
fn same_picture_inside(a: &Image, b: &Image) -> bool {
    features::agreement(&a.corners, &b.corners) >= features::AGREEING_CORNERS
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
/// What a search is doing, so the window can show it rather than sit on the last
/// thing the pass said until the sets appear.
#[derive(Debug, Clone, Copy)]
pub enum Progress {
    /// Pictures read out of the index, of how many it holds.
    Loading { done: u64, total: u64 },
    /// Every row is in memory and the structure the search works on is built.
    Loaded { images: u64 },
    /// Pictures whose corners have been looked up, of how many there are. This
    /// is the shortlist being drawn up, and on a large folder it is the longest
    /// part of a search.
    Shortlisting { done: u64, total: u64 },
    /// Pairs compared, of how many the shortlist produced.
    Comparing { done: u64, total: u64 },
    /// Everything is compared and the sets are being put together.
    Grouping,
}

/// Rows between reports while loading. Reading a row is cheap, so this is often
/// enough to move and rare enough not to be the cost.
const LOAD_REPORT_EVERY: usize = 256;

/// Read the index into the form the search works on.
///
/// Done once. Nothing in here changes unless the folder does, so a second search
/// at a different sensitivity reads nothing: it runs over what this produced.
pub fn load_images(
    conn: &Connection,
    cancel: &AtomicBool,
    report: &(dyn Fn(Progress) + Sync),
) -> Result<Option<Vec<Image>>> {
    // Before the count, not after it. Counting the rows is itself a scan of the
    // whole view, half a second on a warm index and longer on a cold one, and a
    // search that says nothing until it has a denominator says nothing for all of
    // that. A total of zero means there is no fraction to draw yet.
    report(Progress::Loading { done: 0, total: 0 });
    // Counted off `files` alone. `indexed_images` is that joined to two more
    // tables, and counting it walks all three to produce one number, which is the
    // whole of the wait before this can say anything with a denominator in it. A
    // file without a fingerprint is dropped as the rows are read, so this is an
    // upper bound rather than an exact count, which is what a bar needs.
    let total: usize = conn
        .query_row("SELECT count(*) FROM files", [], |row| row.get::<_, i64>(0))
        .context("counting the indexed images")? as usize;
    report(Progress::Loading { done: 0, total: total as u64 });

    let mut statement = conn.prepare(LOAD_IMAGES)?;
    let mut rows = statement.query([])?;
    let mut images: Vec<Image> = Vec::with_capacity(total);
    while let Some(row) = rows.next()? {
        if images.len() % 1024 == 0 && cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if images.len() % LOAD_REPORT_EVERY == 0 {
            report(Progress::Loading { done: images.len() as u64, total: total as u64 });
        }
        let packed: Vec<u8> = row.get(8)?;
        let Some(hashes) = fingerprint::unpack_hashes(&packed) else {
            continue;
        };
        let ring: Vec<u8> = row.get(9)?;
        let corners: Vec<u8> = row.get(10).unwrap_or_default();
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
            corners: features::unpack(&corners),
        });
    }
    report(Progress::Loading { done: images.len() as u64, total: total as u64 });
    Ok(Some(images))
}

/// Find every duplicate set in the index.
pub fn find_sets(conn: &Connection, thresholds: Thresholds) -> Result<Vec<DuplicateSet>> {
    let never = AtomicBool::new(false);
    Ok(find_sets_cancellable(conn, thresholds, &never, &|_| {})?
        .expect("a search that is never cancelled cannot come back cancelled"))
}

#[cfg(test)]
mod search_reports {
    use super::*;

    /// A search says what it is doing while it does it, against a real index.
    ///
    /// It used to say nothing at all until it had the answer, so the window sat on
    /// whatever the pass had last put there for the whole of the search. Reading
    /// the index is most of that on a folder that is not on this machine.
    #[test]
    fn a_search_reports_while_it_runs() {
        let Some(folder) = std::env::var_os("IMGDEDUPE_TEST_FOLDER").map(std::path::PathBuf::from)
        else {
            panic!("set IMGDEDUPE_TEST_FOLDER to the folder to search");
        };
        let db_path = folder.join(crate::db::INDEX_FILENAME);
        let conn = crate::db::open_read_only(&db_path).expect("the index");

        let seen = std::sync::Mutex::new(Vec::new());
        let never = AtomicBool::new(false);
        let started = std::time::Instant::now();
        let sets = find_sets_cancellable(&conn, Thresholds::preset("balanced"), &never, &|p| {
            seen.lock().unwrap().push((started.elapsed(), p));
        })
        .expect("the search")
        .expect("not cancelled");

        let seen = seen.into_inner().unwrap();
        assert!(!seen.is_empty(), "the search reported nothing at all");
        let loading = seen
            .iter()
            .filter(|(_, p)| matches!(p, Progress::Loading { .. }))
            .count();
        let comparing = seen
            .iter()
            .filter(|(_, p)| matches!(p, Progress::Comparing { .. }))
            .count();
        assert!(loading > 1, "the search reported reading the index {loading} times");
        assert!(comparing > 1, "the search reported comparing {comparing} times");
        // And the first word from it comes before it has done any of the work,
        // rather than after the count of what there is to do.
        let first = seen[0].0;
        assert!(
            first < std::time::Duration::from_millis(50),
            "the search said nothing for its first {first:?}"
        );
        println!("{} sets, {} reports", sets.len(), seen.len());
    }
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
    report(Progress::Loaded { images: images.len() as u64 });
    if stopped() {
        return Ok(None);
    }

    let families = fold_identical(&images);
    let one_of_each: Vec<u32> = families.iter().map(|family| family[0]).collect();
    crate::log_line!(
        "search folding: {} images are {} different pictures",
        images.len(),
        one_of_each.len()
    );

    // Each way of matching draws up its own shortlist, and a way that is turned
    // off draws up none: there is nothing to be gained by shortlisting pairs for
    // a test that is not going to be made.
    let by_band: Vec<Vec<(u32, u32)>> = (0..fingerprint::BANDS)
        .into_par_iter()
        .map(|band| {
            if stopped() || !thresholds.whole_frame {
                return Vec::new();
            }
            pairs_in_band(&images, &one_of_each, band)
        })
        .collect();
    if stopped() {
        return Ok(None);
    }

    let mut candidates: Vec<(u32, u32)> = by_band.concat();
    #[cfg(feature = "logging")]
    timing.step("banding", candidates.len(), "candidate pairs");
    // The second way in: pictures holding some of the same corners. The bands
    // above only ever put together pictures that fill the frame the same way.
    let by_corner = if thresholds.corners {
        pairs_by_corner(images, &one_of_each, &stopped, report)
    } else {
        Vec::new()
    };
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
            let found = batch_pairs
                .iter()
                .copied()
                .filter(|(a, b)| {
                    is_match(&images[*a as usize], &images[*b as usize], thresholds)
                })
                .collect::<Vec<(u32, u32)>>();
            let done = compared.fetch_add(batch_pairs.len() as u64, Ordering::Relaxed)
                + batch_pairs.len() as u64;
            report(Progress::Comparing { done, total: pairs });
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
#[cfg(feature = "logging")]
struct Timing {
    started: std::time::Instant,
    step_started: std::time::Instant,
}

#[cfg(feature = "logging")]
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
        crate::log_line!("search {name}: {:.2}s, {count} {of}", took.as_secs_f64());
    }

    fn total(&self, sets: usize) {
        crate::log_line!(
            "search finished: {:.2}s, {sets} sets",
            self.started.elapsed().as_secs_f64()
        );
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
            corners: Vec::new(),
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
        let outcome = find_sets_cancellable(&conn, Thresholds::preset("balanced"), &stop, &|_| {})
            .expect("the search failed rather than stopping");
        assert!(outcome.is_none(), "a stopped search still handed back sets");

        // And it is only stopped when it is asked: the same search finishes.
        let never = AtomicBool::new(false);
        let sets = find_sets_cancellable(&conn, Thresholds::preset("balanced"), &never, &|_| {})
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
        let sets = find_sets(
            &conn,
            Thresholds {
                max_bits: 2,
                max_ring: 1.0,
                ignore_colour: true,
                whole_frame: true,
                corners: true,
            },
        )
        .expect("find");
        assert_eq!(sets.len(), 1, "the chain did not collapse into one set");
        assert_eq!(sets[0].members.len(), 3);
    }

    #[test]
    fn the_colour_signature_can_split_a_pair_and_the_setting_can_rejoin_it() {
        let mut conn = open();
        insert(&mut conn, "colour.jpg", 0x1234, 800, 600, 100_000, ring(0.5));
        insert(&mut conn, "gray.jpg", 0x1234, 800, 600, 100_000, ring(0.0));

        let split = find_sets(
            &conn,
            Thresholds {
                max_bits: 6,
                max_ring: 0.05,
                ignore_colour: false,
                whole_frame: true,
                corners: true,
            },
        )
        .expect("find");
        assert!(split.is_empty(), "the colour check did not separate them");

        let joined = find_sets(
            &conn,
            Thresholds {
                max_bits: 6,
                max_ring: 0.05,
                ignore_colour: true,
                whole_frame: true,
                corners: true,
            },
        )
        .expect("find");
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
        let images = load_images(&conn, &never, &|_| {}).expect("load").expect("not cancelled");

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
        let images = load_images(&conn, &never, &|_| {}).expect("load").expect("not cancelled");
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
        let images = load_images(&conn, &never, &|_| {}).expect("load").expect("not cancelled");
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
            Thresholds {
                max_bits: GUARANTEED_RADIUS,
                max_ring: 1.0,
                ignore_colour: true,
                whole_frame: true,
                corners: true,
            };
        let found = find_sets(&conn, thresholds).expect("search");

        let never = AtomicBool::new(false);
        let images = load_images(&conn, &never, &|_| {}).expect("load").expect("not cancelled");
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
