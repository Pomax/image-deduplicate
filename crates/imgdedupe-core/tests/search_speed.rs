//! What a search costs on an index the size of a real folder.
//!
//! The index is read into memory and the comparing happens there, so what this
//! has to say is that the work stays proportional to the folder rather than to
//! the square of it.
//!
//! The times it prints are worth reading. These run unoptimised, so they are
//! several times what a release build takes.

use std::time::Instant;

use imgdedupe_core::db::{self, Connection, Record};
use imgdedupe_core::fingerprint::{self, Fingerprint};
use imgdedupe_core::format::Format;
use imgdedupe_core::matching::{self, Thresholds};

const FILES: usize = 4_000;

/// A tenth of the folder above. Comparing everything to everything grows sixteen
/// times between the two; the band index grows about four.
const FEW_FILES: usize = 1_000;

/// One picture in ten is a copy of the one before it, which is what a folder that
/// is worth running this on looks like. The rest are unrelated.
///
/// The seed is stirred rather than used directly: two seeds a small number apart
/// have to give hashes that are far apart, or the bands fill up with near
/// neighbours and what gets measured is the fixture rather than the search.
fn hash_for(index: usize) -> fingerprint::Hash {
    let seed = if index % 10 == 0 && index > 0 { index as u64 - 1 } else { index as u64 };
    let mut state = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    let mut out = [0u8; fingerprint::HASH_BYTES];
    for byte in out.iter_mut() {
        state ^= state >> 30;
        state = state.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        state ^= state >> 27;
        state = state.wrapping_mul(0x94D0_49BB_1331_11EB);
        state ^= state >> 31;
        *byte = state as u8;
    }
    out
}

fn build(path: &std::path::Path, files: usize) -> Connection {
    let mut conn = db::open(path).expect("index");
    let tx = conn.transaction().expect("tx");
    for index in 0..files {
        let hash = hash_for(index);
        let record = Record {
            rel_path: format!("folder{}/picture{index}.jpeg", index % 50),
            size_bytes: 4_000_000 + index as i64,
            mtime_ns: 1,
            bytes_hash: index as u32,
            width: 3000,
            height: 4000,
            format: Format::Jpeg,
            channels: 3,
            fingerprint: Fingerprint {
                dct_hash: hash,
                dct_hashes: [hash; fingerprint::VARIANTS],
                ring_stats: vec![0u8; 48 * 4],
            },
        };
        db::upsert(&tx, &record, 1).expect("upsert");
    }
    tx.commit().expect("commit");
    conn
}

#[test]
fn what_a_search_costs_on_a_folder_worth_running_it_on() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("speed.sqlite");
    let started = Instant::now();
    let conn = build(&path, FILES);
    println!("built {FILES} files in {:.1}s", started.elapsed().as_secs_f64());

    let started = Instant::now();
    let sets = matching::find_sets(&conn, Thresholds::balanced()).expect("search");
    let whole = started.elapsed().as_secs_f64();
    println!("{} sets in {whole:.2}s", sets.len());

    assert!(whole < 20.0, "a search over {FILES} files took {whole:.1}s");

    check_the_work_is_not_quadratic(&dir.path().join("few.sqlite"), whole);
}

/// Four times the folder, four times the work, near enough. Comparing every image
/// to every other one would be sixteen, and that is what this stops coming back.
fn check_the_work_is_not_quadratic(path: &std::path::Path, whole: f64) {
    let conn = build(path, FEW_FILES);
    let started = Instant::now();
    let few = matching::find_sets(&conn, Thresholds::balanced()).expect("search");
    let small = started.elapsed().as_secs_f64();

    let files = FILES as f64 / FEW_FILES as f64;
    let grew = whole / small.max(0.000_001);
    println!(
        "{FEW_FILES} files in {small:.2}s, {FILES} in {whole:.2}s: {files}x the folder, \
         {grew:.1}x the time, {} sets",
        few.len()
    );
    assert!(
        grew < files * files / 2.0,
        "{files}x the folder cost {grew:.1}x the time, which is the shape of comparing everything"
    );
}

/// A folder that is mostly copies of one picture. Every copy has the same hash,
/// so they all land in the same band bucket, and a bucket is compared to itself:
/// left alone this is the one shape that stays quadratic. Four times the copies
/// has to cost about four times the time, not sixteen.
#[test]
fn a_folder_full_of_one_picture_does_not_become_quadratic() {
    let dir = tempfile::tempdir().expect("tempdir");
    let small = time_copies(&dir.path().join("few.sqlite"), 500);
    let large = time_copies(&dir.path().join("many.sqlite"), 2_000);

    let grew = large / small.max(0.000_001);
    println!("500 copies in {small:.2}s, 2000 in {large:.2}s: 4x the copies, {grew:.1}x the time");
    assert!(
        grew < 8.0,
        "4x the copies of one picture cost {grew:.1}x the time, which is the shape of \
         comparing the whole bucket to itself"
    );
}

/// Build a folder of `copies` copies of one picture and time a search over it.
fn time_copies(path: &std::path::Path, copies: usize) -> f64 {
    let mut conn = db::open(path).expect("index");
    let tx = conn.transaction().expect("tx");
    let hash = hash_for(0);
    for index in 0..copies {
        let record = Record {
            rel_path: format!("copy{index}.jpeg"),
            size_bytes: 4_000_000,
            mtime_ns: 1,
            bytes_hash: 1,
            width: 3000,
            height: 4000,
            format: Format::Jpeg,
            channels: 3,
            fingerprint: Fingerprint {
                dct_hash: hash,
                dct_hashes: [hash; fingerprint::VARIANTS],
                ring_stats: vec![0u8; 48 * 4],
            },
        };
        db::upsert(&tx, &record, 1).expect("upsert");
    }
    tx.commit().expect("commit");

    let started = Instant::now();
    let sets = matching::find_sets(&conn, Thresholds::balanced()).expect("search");
    let took = started.elapsed().as_secs_f64();
    assert_eq!(sets.len(), 1, "the copies did not come back as one set");
    assert_eq!(sets[0].members.len(), copies, "the set lost copies");
    took
}

