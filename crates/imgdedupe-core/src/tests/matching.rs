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

fn insert(
    conn: &mut Connection,
    path: &str,
    seed: u64,
    width: u32,
    height: u32,
    size: i64,
    ring: Vec<u8>,
) {
    insert_written_at(conn, path, seed, width, height, size, ring, 1);
}

#[allow(clippy::too_many_arguments)]
fn insert_written_at(
    conn: &mut Connection,
    path: &str,
    seed: u64,
    width: u32,
    height: u32,
    size: i64,
    ring: Vec<u8>,
    mtime_seconds: i64,
) {
    let hash = hash_seeded(seed);
    let record = db::Record {
        rel_path: path.to_string(),
        size_bytes: size,
        mtime_seconds,
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

/// Comparing says how far it has got while it is still comparing.
///
/// The pairs are cut into batches that all run at once, so a report at the
/// end of a batch is a report at the end of the whole thing: measured on a
/// real folder that was thirteen seconds of a bar standing still and then
/// filling in one step. Six hundred pictures alike enough to be worth
/// comparing is tens of thousands of pairs, which is hundreds of reports.
#[test]
fn comparing_is_reported_while_it_is_still_comparing() {
    let mut conn = open();
    for which in 0..600u64 {
        // A few bits apart, so every one of them is a candidate against
        // every other and the pairs are the folder squared. Different from
        // each other, or they would be folded into one before any of this.
        insert(
            &mut conn,
            &format!("{which}.jpg"),
            which,
            1000,
            800,
            1000 + which as i64,
            ring(0.5),
        );
    }

    let seen = std::sync::Mutex::new(Vec::new());
    let never = AtomicBool::new(false);
    let images = load_images(&conn, &never, &|_| {})
        .expect("read")
        .expect("not cancelled");
    find_sets_in(&images, Thresholds::at(15.0), &never, &|progress| {
        if let Progress::Comparing { done, total } = progress {
            seen.lock().unwrap().push((done, total));
        }
    })
    .expect("search")
    .expect("not cancelled");

    let seen = seen.into_inner().unwrap();
    // Many times more often than there are batches. One report per batch is
    // the thing this is here to catch: every batch is running at the same
    // time, so they all report at the end and the bar never moves until it
    // is over.
    assert!(
        seen.len() as u64 > COMPARE_BATCHES * 4,
        "comparing reported {} times over {COMPARE_BATCHES} batches",
        seen.len()
    );
    let (last, total) = *seen.last().expect("a report");
    assert_eq!(last, total, "comparing finished saying {last} of {total}");
    // Something arrives while there is still most of the work left.
    let early = seen.iter().any(|(done, total)| *done * 2 < *total);
    assert!(early, "nothing was reported before comparing was half over");
}

/// A search can be stopped from another thread. It gives back nothing when it
/// is: half a search has no answer, and reporting one would be a lie about
/// which duplicates a folder holds.
#[test]
fn a_search_stops_when_it_is_told_to_and_gives_back_nothing() {
    let mut conn = open();
    for index in 0..40 {
        insert(
            &mut conn,
            &format!("{index}.jpg"),
            0x1234,
            800,
            600,
            100_000,
            ring(0.5),
        );
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
    insert(
        &mut conn,
        "a.jpg",
        0x0000_0000_0000_0000,
        800,
        600,
        100_000,
        ring(0.5),
    );
    insert(
        &mut conn,
        "b.jpg",
        0xFFFF_FFFF_FFFF_FFFF,
        800,
        600,
        100_000,
        ring(0.5),
    );
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
            within_a_folder: false,
        },
    )
    .expect("find");
    assert_eq!(sets.len(), 1, "the chain did not collapse into one set");
    assert_eq!(sets[0].members.len(), 3);
}

#[test]
fn the_colour_signature_can_split_a_pair_and_the_setting_can_rejoin_it() {
    let mut conn = open();
    insert(
        &mut conn,
        "colour.jpg",
        0x1234,
        800,
        600,
        100_000,
        ring(0.5),
    );
    insert(&mut conn, "gray.jpg", 0x1234, 800, 600, 100_000, ring(0.0));

    let split = find_sets(
        &conn,
        Thresholds {
            max_bits: 6,
            max_ring: 0.05,
            ignore_colour: false,
            whole_frame: true,
            corners: true,
            within_a_folder: false,
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
            within_a_folder: false,
        },
    )
    .expect("find");
    assert_eq!(joined.len(), 1, "the setting did not rejoin them");
}

#[test]
fn a_different_shape_is_not_a_duplicate() {
    let mut conn = open();
    insert(&mut conn, "wide.jpg", 0x1234, 1600, 400, 100_000, ring(0.5));
    insert(
        &mut conn,
        "square.jpg",
        0x1234,
        800,
        800,
        100_000,
        ring(0.5),
    );
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
    let bits: Vec<u32> = PRESETS
        .iter()
        .map(|(_, percent)| Thresholds::at(*percent).max_bits)
        .collect();
    assert!(
        bits.windows(2).all(|pair| pair[0] < pair[1]),
        "{bits:?} do not widen"
    );

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
    let balanced = PRESETS
        .iter()
        .find(|(name, _)| *name == "balanced")
        .expect("a balanced preset");
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
        assert_eq!(
            Thresholds::preset(name).max_bits,
            Thresholds::at(percent).max_bits
        );
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
    assert!(
        widest.max_bits < fingerprint::HASH_BITS as u32,
        "the whole hash may differ"
    );
    let unrelated = (fingerprint::HASH_BITS as f64 * 0.25) as u32;
    assert!(
        widest.max_bits > unrelated,
        "the scale no longer reaches unrelated pictures"
    );
    assert_eq!(Thresholds::at(-5.0).max_bits, 0);
}

#[test]
fn a_rotation_keeps_the_same_shape() {
    assert!(aspect_ok(1600.0, 1200.0, 1200.0, 1600.0));
    assert!(aspect_ok(1600.0, 1200.0, 800.0, 600.0));
    assert!(!aspect_ok(1600.0, 1200.0, 1600.0, 400.0));
}

/// A duplicate is usually a copy made after the picture it came from, so a
/// set reads left to right in the order the files appeared, whatever order
/// the search happened to come across them in.
#[test]
fn a_set_comes_back_oldest_first() {
    let mut conn = open();
    insert_written_at(
        &mut conn,
        "later.jpg",
        0x1234,
        1600,
        1200,
        300_000,
        ring(0.5),
        3_000,
    );
    insert_written_at(
        &mut conn,
        "earlier.jpg",
        0x1234,
        400,
        300,
        20_000,
        ring(0.5),
        1_000,
    );
    insert_written_at(
        &mut conn,
        "between.jpg",
        0x1234,
        800,
        600,
        90_000,
        ring(0.5),
        2_000,
    );

    let sets = find_sets(&conn, Thresholds::preset("balanced")).expect("find");
    let order: Vec<&str> = sets[0]
        .members
        .iter()
        .map(|member| member.rel_path.as_str())
        .collect();
    assert_eq!(order, ["earlier.jpg", "between.jpg", "later.jpg"]);
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

/// A set read back out of the index is the set the search handed over: the
/// same pictures, the same order, the same best copy. Only file ids were
/// written down, and everything else comes from the pictures as they are.
#[test]
fn a_stored_set_is_built_back_into_the_set_it_was() {
    let mut conn = open();
    insert_written_at(
        &mut conn,
        "later.jpg",
        0x1234,
        1600,
        1200,
        300_000,
        ring(0.5),
        3_000,
    );
    insert_written_at(
        &mut conn,
        "earlier.jpg",
        0x1234,
        400,
        300,
        20_000,
        ring(0.5),
        1_000,
    );
    let found = find_sets(&conn, Thresholds::preset("balanced")).expect("find");
    let images = load_images(&conn, &AtomicBool::new(false), &|_| {})
        .expect("load")
        .expect("images");

    let stored: Vec<(i64, Vec<i64>)> = found
        .iter()
        .map(|set| (set.set_id, set.members.iter().map(|m| m.file_id).collect()))
        .collect();
    let built = sets_from_stored(&images, &stored);

    assert_eq!(built.len(), 1);
    assert_eq!(built[0].set_id, found[0].set_id);
    let names: Vec<&str> = built[0]
        .members
        .iter()
        .map(|m| m.rel_path.as_str())
        .collect();
    assert_eq!(
        names,
        ["earlier.jpg", "later.jpg"],
        "the order it was shown in was lost"
    );
    let keeper: Vec<&str> = built[0]
        .members
        .iter()
        .filter(|m| m.auto_keep)
        .map(|m| m.rel_path.as_str())
        .collect();
    assert_eq!(
        keeper,
        ["later.jpg"],
        "the best copy is not the one the search chose"
    );
}

/// A set whose pictures have gone comes back short, and one left holding a
/// single picture does not come back at all: one picture is not a set of
/// copies.
#[test]
fn a_stored_set_that_lost_its_pictures_comes_back_short_or_not_at_all() {
    let mut conn = open();
    insert(&mut conn, "a.jpg", 0x1234, 400, 300, 20_000, ring(0.5));
    insert(&mut conn, "b.jpg", 0x1234, 800, 600, 90_000, ring(0.5));
    insert(&mut conn, "c.jpg", 0x1234, 1600, 1200, 300_000, ring(0.5));
    let images = load_images(&conn, &AtomicBool::new(false), &|_| {})
        .expect("load")
        .expect("images");
    let ids: Vec<i64> = images.iter().map(|image| image.file_id).collect();
    let missing = ids.iter().max().expect("ids") + 100;

    let built = sets_from_stored(
        &images,
        &[
            (ids[0], vec![ids[0], ids[1], missing]),
            (ids[2], vec![ids[2], missing]),
        ],
    );
    assert_eq!(built.len(), 1, "a set of one picture came back");
    assert_eq!(
        built[0].members.len(),
        2,
        "the picture that is gone came back with it"
    );
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
    insert(
        &mut conn,
        "c.jpg",
        0xFFFF_0000_FFFF_0000,
        800,
        600,
        100_000,
        ring(0.5),
    );
    let never = AtomicBool::new(false);
    let images = load_images(&conn, &never, &|_| {})
        .expect("load")
        .expect("not cancelled");

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
    assert!(
        shared.contains(&0) && shared.contains(&1),
        "the pair was not filed together"
    );
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
        insert(
            &mut conn,
            &format!("copy{index}.jpg"),
            0x1234,
            800,
            600,
            100_000,
            ring(0.5),
        );
    }
    insert(
        &mut conn,
        "other.jpg",
        0xFFFF_FFFF_FFFF_FFFF,
        800,
        600,
        100_000,
        ring(0.5),
    );

    let never = AtomicBool::new(false);
    let images = load_images(&conn, &never, &|_| {})
        .expect("load")
        .expect("not cancelled");
    let families = fold_identical(&images, false);
    assert_eq!(families.len(), 2, "the copies were not folded together");
    assert_eq!(
        families.iter().map(|family| family.len()).max(),
        Some(50),
        "the fold lost copies"
    );

    // And every one of them still comes back as a duplicate of the rest.
    let sets = find_sets(&conn, Thresholds::preset("balanced")).expect("search");
    assert_eq!(sets.len(), 1);
    assert_eq!(
        sets[0].members.len(),
        50,
        "folding dropped copies from the set"
    );
}

/// The fold is only allowed where nothing can tell the images apart. Two
/// pictures with the same hash and a different shape are not duplicates, so
/// they are not the same entry either.
#[test]
fn the_same_hash_at_a_different_shape_is_not_folded() {
    let mut conn = open();
    insert(&mut conn, "wide.jpg", 0x1234, 1600, 400, 100_000, ring(0.5));
    insert(
        &mut conn,
        "square.jpg",
        0x1234,
        800,
        800,
        100_000,
        ring(0.5),
    );

    let never = AtomicBool::new(false);
    let images = load_images(&conn, &never, &|_| {})
        .expect("load")
        .expect("not cancelled");
    assert_eq!(
        fold_identical(&images, false).len(),
        2,
        "two shapes were folded into one"
    );
    assert!(find_sets(&conn, Thresholds::preset("balanced"))
        .expect("search")
        .is_empty());
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
        insert(
            &mut conn,
            &format!("{index}.jpg"),
            seed,
            800,
            600,
            100_000 + index as i64,
            ring(0.5),
        );
    }

    let thresholds = Thresholds {
        max_bits: GUARANTEED_RADIUS,
        max_ring: 1.0,
        ignore_colour: true,
        whole_frame: true,
        corners: true,
        within_a_folder: false,
    };
    let found = find_sets(&conn, thresholds).expect("search");

    let never = AtomicBool::new(false);
    let images = load_images(&conn, &never, &|_| {})
        .expect("load")
        .expect("not cancelled");
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
            let mut paths: Vec<String> = set
                .members
                .iter()
                .map(|member| member.rel_path.clone())
                .collect();
            paths.sort();
            paths
        })
        .collect();
    reported.sort();

    assert_eq!(
        reported, expected,
        "the band search and comparing everything disagree"
    );
}
