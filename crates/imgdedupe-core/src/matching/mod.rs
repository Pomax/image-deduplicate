use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use anyhow::{Context, Result};
use rayon::prelude::*;
use rusqlite::Connection;

use crate::features;
use crate::fingerprint::{self, Words};
use crate::format::Format;
use crate::score::keep_score;

mod bands;
mod compare;
mod corners;
mod grouping;
mod search;

use self::bands::*;
use self::compare::*;
use self::corners::*;
use self::grouping::*;
pub use self::search::*;

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

use crate::constants::{
    CORNERS_DEFAULT, IGNORE_COLOUR_DEFAULT, WHOLE_FRAME_DEFAULT, WITHIN_A_FOLDER_DEFAULT,
};
pub use crate::constants::{
    SENSITIVITY_SLIDER_DEFAULT_PERCENT, SENSITIVITY_SLIDER_MAX_PERCENT, SENSITIVITY_SLIDER_PRESETS,
};

impl Thresholds {
    /// The threshold a preset stands for, or the closest one if the name is not
    /// a preset.
    pub fn preset(name: &str) -> Self {
        let percent = SENSITIVITY_SLIDER_PRESETS
            .iter()
            .find(|(preset, _)| *preset == name)
            .map_or(SENSITIVITY_SLIDER_DEFAULT_PERCENT, |(_, percent)| *percent);
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
        let percent = percent.clamp(0.0, SENSITIVITY_SLIDER_MAX_PERCENT);
        Thresholds {
            max_bits: share(percent),
            max_ring: (0.005 * percent).max(0.005) as f32,
            ignore_colour: IGNORE_COLOUR_DEFAULT,
            whole_frame: WHOLE_FRAME_DEFAULT,
            corners: CORNERS_DEFAULT,
            within_a_folder: WITHIN_A_FOLDER_DEFAULT,
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

#[cfg(test)]
#[path = "../tests/matching.rs"]
mod tests;
