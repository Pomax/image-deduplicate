//! What a search costs on an index the size of a real folder.
//!
//! The search is cut into pieces so the window can show how far through it is.
//! The pieces have to do the same work the whole statements did, and this is what
//! says whether they do.
//!
//! The times it prints are worth reading, and the check it makes is that the
//! search does not become something else: the shape of the work, and that it
//! reports often enough for a bar to move.

use std::time::Instant;

use imgdedupe_core::db::{self, Connection, Record};
use imgdedupe_core::fingerprint::{self, Fingerprint};
use imgdedupe_core::format::Format;
use imgdedupe_core::matching::{self, Step, Thresholds};

const FILES: usize = 4_000;

/// One picture in ten is a copy of another, which is what a folder that is worth
/// running this on looks like. The rest are unrelated, so the bands spread out
/// the way they do in a real index.
fn hash_for(index: usize) -> fingerprint::Hash {
    let mut out = [0u8; fingerprint::HASH_BYTES];
    let seed = if index % 10 == 0 { (index / 10) as u64 } else { index as u64 * 0x9E37_79B9 };
    for (chunk, byte) in out.iter_mut().enumerate() {
        *byte = (seed.wrapping_mul(chunk as u64 + 1) >> (chunk % 8)) as u8;
    }
    out
}

fn build(path: &std::path::Path) -> Connection {
    let mut conn = db::open(path).expect("index");
    let tx = conn.transaction().expect("tx");
    for index in 0..FILES {
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
fn what_a_search_costs_and_how_often_it_reports() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("speed.sqlite");
    let started = Instant::now();
    let conn = build(&path);
    println!("built {FILES} files in {:.1}s", started.elapsed().as_secs_f64());

    matching::register_functions(&conn).expect("functions");

    let steps = std::cell::RefCell::new(Vec::new());
    let started = Instant::now();
    let never = std::sync::atomic::AtomicBool::new(false);
    let sets = matching::find_sets_reporting(
        &conn,
        Thresholds::balanced(),
        &never,
        &|progress| {
            steps.borrow_mut().push((progress.step, Instant::now()));
        },
    )
    .expect("search")
    .expect("a search that was not cancelled came back cancelled");
    let whole = started.elapsed().as_secs_f64();

    let steps = steps.into_inner();
    println!("{} sets in {whole:.2}s, {} reports", sets.len(), steps.len());
    for step in [Step::Pairing, Step::Comparing, Step::Grouping, Step::Listing] {
        let count = steps.iter().filter(|(seen, _)| *seen == step).count();
        let last = steps.iter().filter(|(seen, _)| *seen == step).next_back();
        let first = steps.iter().find(|(seen, _)| *seen == step);
        let took = match (first, last) {
            (Some((_, a)), Some((_, b))) => b.duration_since(*a).as_secs_f64(),
            _ => 0.0,
        };
        println!("{}: {count} reports over {took:.2}s", step.label());
    }

    assert!(whole < 20.0, "a search over {FILES} files took {whole:.1}s");

    check_slicing_costs_nothing(&conn);
}

/// Cutting the pairing step into pieces, so a bar could count them, is what made
/// a search several times slower. This is the measurement that says so, and what
/// stops it being done again.
fn check_slicing_costs_nothing(conn: &Connection) {
    let last: i64 =
        conn.query_row("SELECT max(id) FROM files", [], |row| row.get(0)).expect("last file");

    let whole = time(|| {
        conn.execute_batch(
            "DROP TABLE IF EXISTS temp.one;
             CREATE TEMP TABLE one AS
             SELECT DISTINCT min(a.file_id, b.file_id) AS a, max(a.file_id, b.file_id) AS b
             FROM phash_bands a
             JOIN phash_bands b
               ON b.band_index = a.band_index AND b.band_value = a.band_value
              AND b.file_id <> a.file_id
             WHERE a.variant = 0;",
        )
        .expect("one statement");
    });

    // The real search, so what is timed and counted is the code that ships and
    // not a copy of it that could drift.
    let shipped = time(|| {
        matching::find_sets(conn, Thresholds::balanced()).expect("search");
    });
    conn.execute_batch(
        "DROP TABLE IF EXISTS temp.shipped;
         CREATE TEMP TABLE shipped AS SELECT a, b FROM candidates;",
    )
    .expect("keep the pairs it built");

    let by_band_and_file = time(|| {
        conn.execute_batch(
            "DROP TABLE IF EXISTS temp.by_band;
             CREATE TEMP TABLE by_band(a INTEGER NOT NULL, b INTEGER NOT NULL, UNIQUE(a, b));",
        )
        .expect("prepare");
        let mut slice = conn
            .prepare(
                "INSERT OR IGNORE INTO by_band(a, b)
                 SELECT min(a.file_id, b.file_id), max(a.file_id, b.file_id)
                 FROM phash_bands a
                 JOIN phash_bands b
                   ON b.band_index = a.band_index AND b.band_value = a.band_value
                  AND b.file_id <> a.file_id
                 WHERE a.variant = 0 AND a.band_index = ?1
                   AND a.file_id > ?2 AND a.file_id <= ?3",
            )
            .expect("prepare");
        for band in 0..fingerprint::BANDS as i64 {
            let mut from = 0i64;
            while from < last {
                let to = (from + 200).min(last);
                slice.execute([band, from, to]).expect("slice");
                from = to;
            }
        }
    });

    let count = |table: &str| -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| row.get(0))
            .expect("count")
    };
    println!(
        "pairing shapes: one statement {whole:.2}s, the whole search {shipped:.2}s, \
         cut into pieces {by_band_and_file:.2}s"
    );

    assert_eq!(count("one"), count("shipped"), "the search built different pairs");
    assert_eq!(count("one"), count("by_band"), "the cut-up shape found different pairs");

    // The whole search, against one statement of the step inside it. If reporting
    // ever costs more than this again, this is what says so.
    assert!(
        shipped < whole * 2.0,
        "the search takes {shipped:.2}s where its pairing step alone takes {whole:.2}s"
    );
}

fn time(work: impl FnOnce()) -> f64 {
    let started = Instant::now();
    work();
    started.elapsed().as_secs_f64()
}
