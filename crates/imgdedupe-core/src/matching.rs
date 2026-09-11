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
    /// Only compare pictures that are in the same folder. A folder scanned with
    /// its subfolders is then searched one folder at a time, and two copies of a
    /// picture filed in two places are left where they are.
    pub within_a_folder: bool,
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
            within_a_folder: false,
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
    /// When the file was last written, as whole seconds since the epoch. Nothing
    /// here reads a date out of the file's own metadata.
    pub mtime_seconds: i64,
    /// The one the search would keep if it were choosing: the largest, least
    /// re-encoded copy in the set. Nothing is kept or removed on account of it.
    /// It is what "auto-mark to keep" marks.
    pub auto_keep: bool,
}

#[derive(Debug, Clone)]
pub struct DuplicateSet {
    pub set_id: i64,
    pub members: Vec<Member>,
}

impl DuplicateSet {
    /// Bytes freed by keeping the best copy and no other. What the command line
    /// reports and sorts by: it has nobody to mark anything, so what the search
    /// would keep is what it keeps.
    pub fn recoverable_bytes(&self) -> i64 {
        self.members
            .iter()
            .filter(|m| !m.auto_keep)
            .map(|m| m.size_bytes)
            .sum()
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
    mtime_seconds: i64,
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
SELECT id, rel_path, width, height, format, channels, size_bytes, mtime_seconds,
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
            state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
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
    let wanted = (entries / BUCKET_TARGET)
        .max(1)
        .next_power_of_two()
        .trailing_zeros();
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
        CornerTable {
            bits,
            starts,
            entries,
        }
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
    looked: &AtomicU64,
    to_look: u64,
) -> Vec<(u32, u32)> {
    // An entry is a picture and one of its corners in one number, which stops
    // being true at a folder of thirty three million pictures. Nothing else
    // here works at that size either.
    if images.len() >= 1 << (u32::BITS - CORNER_ROOM) {
        return Vec::new();
    }
    let filed: usize = of
        .iter()
        .map(|id| images[*id as usize].corners.len().min(INDEXED_CORNERS))
        .sum();
    let bits = table_bits(filed);
    let most_in_a_bucket = (filed >> bits).max(1) * BUCKET_LIMIT;
    let tables: Vec<CornerTable> = CORNER_BITS
        .par_iter()
        .map(|s| CornerTable::of(images, of, s, bits))
        .collect();
    crate::log_line!(
        "corner index: {} corners over {} tables of {} buckets, skipping past {}",
        filed,
        CORNER_TABLES,
        1 << bits,
        most_in_a_bucket
    );
    of.par_iter()
        .flat_map_iter(|id| {
            let mut agreeing: std::collections::HashMap<u32, u64> =
                std::collections::HashMap::new();
            let done = looked.fetch_add(1, Ordering::Relaxed) + 1;
            if done % SHORTLIST_REPORT_EVERY == 0 || done == to_look {
                report(Progress::Shortlisting {
                    done,
                    total: to_look,
                });
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
    /// The folder it is in, when a search only matches inside one. Two copies of
    /// a picture in two folders are then two pictures, not one standing for the
    /// other: folded together they would come back as one set spanning both.
    folder: Option<String>,
}

impl Identity {
    fn of(image: &Image, within_a_folder: bool) -> Identity {
        Identity {
            variants: image.variants,
            ring: image.ring.iter().map(|value| value.to_bits()).collect(),
            width: image.width,
            height: image.height,
            folder: within_a_folder.then(|| folder_of(&image.rel_path).to_string()),
        }
    }
}

/// The folder a file is in, as the index writes paths: separated by forward
/// slashes, and relative to the folder that was scanned. Everything up to the
/// last of them, which is nothing at all for a file in the folder itself.
fn folder_of(rel_path: &str) -> &str {
    match rel_path.rfind('/') {
        Some(at) => &rel_path[..at],
        None => "",
    }
}

/// The searches to run: one per folder when a search is held to folders, and
/// one holding everything when it is not.
///
/// In folder order, so a run reads the same way twice and a log of one can be
/// compared with a log of another.
fn by_folder(images: &[Image], of: Vec<u32>, within_a_folder: bool) -> Vec<Vec<u32>> {
    if !within_a_folder {
        return vec![of];
    }
    let mut folders: std::collections::BTreeMap<&str, Vec<u32>> = std::collections::BTreeMap::new();
    for id in of {
        folders
            .entry(folder_of(&images[id as usize].rel_path))
            .or_default()
            .push(id);
    }
    folders.into_values().collect()
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
fn fold_identical(images: &[Image], within_a_folder: bool) -> Vec<Vec<u32>> {
    let mut known: std::collections::HashMap<Identity, usize> =
        std::collections::HashMap::with_capacity(images.len());
    let mut groups: Vec<Vec<u32>> = Vec::with_capacity(images.len());
    for (position, image) in images.iter().enumerate() {
        match known.get(&Identity::of(image, within_a_folder)) {
            Some(group) => groups[*group].push(position as u32),
            None => {
                known.insert(Identity::of(image, within_a_folder), groups.len());
                groups.push(vec![position as u32]);
            }
        }
    }
    groups
}

/// Whether a candidate pair really is the same picture. `a` is the one with the
/// lower file id, which is the side that contributes all eight of its variants.
fn is_match(a: &Image, b: &Image, thresholds: Thresholds) -> bool {
    // A search that only matches inside a folder compares nothing across two of
    // them, however alike the two pictures are.
    if thresholds.within_a_folder && folder_of(&a.rel_path) != folder_of(&b.rel_path) {
        return false;
    }
    (thresholds.whole_frame && whole_frame_match(a, b, thresholds))
        || (thresholds.corners && same_picture_inside(a, b))
}

/// The two pictures are the same picture filling the frame the same way: a
/// resize, a recompression, a rotation. Cheap, and it is most of what a folder
/// of duplicates holds.
fn whole_frame_match(a: &Image, b: &Image, thresholds: Thresholds) -> bool {
    if !aspect_ok(
        a.width as f64,
        a.height as f64,
        b.width as f64,
        b.height as f64,
    ) {
        return false;
    }
    let distance = fingerprint::hamming_any_words(&a.variants, &b.variants[0], thresholds.max_bits);
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
        Groups {
            parent: (0..count as u32).collect(),
        }
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
    report(Progress::Loading {
        done: 0,
        total: total as u64,
    });

    let mut statement = conn.prepare(LOAD_IMAGES)?;
    let mut rows = statement.query([])?;
    let mut images: Vec<Image> = Vec::with_capacity(total);
    while let Some(row) = rows.next()? {
        if images.len() % 1024 == 0 && cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if images.len() % LOAD_REPORT_EVERY == 0 {
            report(Progress::Loading {
                done: images.len() as u64,
                total: total as u64,
            });
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
            mtime_seconds: row.get(7)?,
            variants: hashes.map(|hash| fingerprint::words(&hash)),
            bands: hashes.map(|hash| fingerprint::bands(&hash)),
            ring: fingerprint::ring_weighted(&ring),
            corners: features::unpack(&corners),
        });
    }
    report(Progress::Loading {
        done: images.len() as u64,
        total: total as u64,
    });
    Ok(Some(images))
}

/// Find every duplicate set in the index.
pub fn find_sets(conn: &Connection, thresholds: Thresholds) -> Result<Vec<DuplicateSet>> {
    let never = AtomicBool::new(false);
    Ok(find_sets_cancellable(conn, thresholds, &never, &|_| {})?
        .expect("a search that is never cancelled cannot come back cancelled"))
}

/// Batches the comparing is cut into, so it spreads across the machine's cores.
const COMPARE_BATCHES: u64 = 32;

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
                        .then(b.mtime_seconds.cmp(&a.mtime_seconds))
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
                        mtime_seconds: image.mtime_seconds,
                        auto_keep: *position == keeper,
                    }
                })
                .collect();
            // Oldest first: a duplicate is usually a copy made after the picture
            // it came from, so the set reads left to right in the order the
            // files appeared. The path settles two written in the same second,
            // so a set comes back in the same order every time.
            members.sort_by(|a, b| {
                a.mtime_seconds
                    .cmp(&b.mtime_seconds)
                    .then(a.rel_path.cmp(&b.rel_path))
            });
            DuplicateSet {
                set_id: images[root as usize].file_id,
                members,
            }
        })
        .collect()
}

/// The pictures without the ones a cleanup took.
///
/// A cleanup removes files the window chose itself, so what the folder holds
/// afterwards is known without looking: everything that was there, less that
/// list. Reading the folder again to find that out is asking a question already
/// answered, and on a folder on another machine it is a listing and a read of
/// the whole index for nothing.
pub fn without(images: Vec<Image>, gone: &[String]) -> Vec<Image> {
    let gone: std::collections::HashSet<&str> = gone.iter().map(String::as_str).collect();
    images
        .into_iter()
        .filter(|image| !gone.contains(image.rel_path.as_str()))
        .collect()
}

/// Build the sets a previous search wrote down, from the pictures as they are
/// now.
///
/// What was written down is file ids and their order and nothing else, so
/// everything a set says about a picture is read out of the index that was just
/// opened. Nothing about a picture is stored twice and the two cannot drift.
///
/// A set that has lost pictures, their rows going with the files, comes back
/// without them, and one left with fewer than two does not come back at all,
/// because one picture is not a set of copies.
pub fn sets_from_stored(images: &[Image], stored: &[(i64, Vec<i64>)]) -> Vec<DuplicateSet> {
    let where_it_is: std::collections::HashMap<i64, u32> = images
        .iter()
        .enumerate()
        .map(|(at, image)| (image.file_id, at as u32))
        .collect();
    stored
        .iter()
        .filter_map(|(set_id, file_ids)| {
            let positions: Vec<u32> = file_ids
                .iter()
                .filter_map(|file_id| where_it_is.get(file_id).copied())
                .collect();
            if positions.len() < 2 {
                return None;
            }
            // The same shaping the search does, so a set read back is the set it
            // was: the same keeper, the same order.
            let mut set = build_sets(
                images,
                &positions.iter().map(|at| (0, *at)).collect::<Vec<_>>(),
            )
            .pop()?;
            set.set_id = *set_id;
            Some(set)
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

#[cfg(test)]
#[path = "tests/matching.rs"]
mod tests;
