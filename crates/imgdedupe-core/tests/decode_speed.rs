//! What one picture costs to turn into a thumbnail.
//!
//! The review reads every picture in the result, so the time it takes to appear
//! is this figure times the number of duplicates, divided by the cores. This says
//! which of the two is the problem.
//!
//! Point it at a real folder: `IMGDEDUPE_SPEED_FOLDER=D:\pictures cargo test -p
//! imgdedupe-core --test decode_speed -- --nocapture`. Without that it has
//! nothing to measure and passes.

use std::path::PathBuf;
use std::time::Instant;

use imgdedupe_core::decode::decode_at_most;
use imgdedupe_core::format::{self, Format, SNIFF_LEN};

/// The long edge the grid asks for, and the one the preview asks for.
const THUMB_EDGE: u32 = 256;
const LARGE_EDGE: u32 = 1600;

struct One {
    path: PathBuf,
    format: Format,
    bytes: usize,
    pixels: u64,
    took: f64,
}

#[test]
fn what_one_thumbnail_costs() {
    let Ok(folder) = std::env::var("IMGDEDUPE_SPEED_FOLDER") else {
        println!("set IMGDEDUPE_SPEED_FOLDER to a folder of pictures to measure it");
        return;
    };
    let folder = PathBuf::from(folder);

    let mut timings = Vec::new();
    for entry in std::fs::read_dir(&folder).expect("reading the folder").flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Ok(bytes) = std::fs::read(&path) else {
            continue;
        };
        let head = &bytes[..bytes.len().min(SNIFF_LEN)];
        let Some(format) = format::detect(head) else {
            continue;
        };

        let started = Instant::now();
        let Ok(decoded) = decode_at_most(format, &bytes, THUMB_EDGE) else {
            continue;
        };
        timings.push(One {
            path,
            format,
            bytes: bytes.len(),
            pixels: decoded.width as u64 * decoded.height as u64,
            took: started.elapsed().as_secs_f64(),
        });
    }

    assert!(!timings.is_empty(), "{} held no pictures", folder.display());
    report("thumbnail", &mut timings);

    // And the same pictures at the size the preview beside the list asks for.
    let mut large = Vec::new();
    for one in timings.iter().take(40) {
        let Ok(bytes) = std::fs::read(&one.path) else {
            continue;
        };
        let started = Instant::now();
        let Ok(decoded) = decode_at_most(one.format, &bytes, LARGE_EDGE) else {
            continue;
        };
        large.push(One {
            path: one.path.clone(),
            format: one.format,
            bytes: bytes.len(),
            pixels: decoded.width as u64 * decoded.height as u64,
            took: started.elapsed().as_secs_f64(),
        });
    }
    if !large.is_empty() {
        report("preview", &mut large);
    }
}

fn report(what: &str, timings: &mut [One]) {
    let count = timings.len();
    let total: f64 = timings.iter().map(|one| one.took).sum();
    let megapixels: f64 = timings.iter().map(|one| one.pixels as f64).sum::<f64>() / 1e6;
    println!(
        "{what}: {count} pictures, {total:.2}s, {:.1}ms each, {:.0} megapixels read",
        total * 1000.0 / count as f64,
        megapixels
    );

    for format in [Format::Jpeg, Format::Png, Format::WebP, Format::Gif] {
        let of_this: Vec<&One> = timings.iter().filter(|one| one.format == format).collect();
        if of_this.is_empty() {
            continue;
        }
        let took: f64 = of_this.iter().map(|one| one.took).sum();
        println!(
            "  {format:?}: {} pictures, {took:.2}s, {:.1}ms each",
            of_this.len(),
            took * 1000.0 / of_this.len() as f64
        );
    }

    timings.sort_by(|a, b| b.took.partial_cmp(&a.took).expect("no NaN"));
    println!("  slowest:");
    for one in timings.iter().take(5) {
        println!(
            "    {:.0}ms {:?} {:.1} MB {} pixels {}",
            one.took * 1000.0,
            one.format,
            one.bytes as f64 / 1e6,
            one.pixels,
            one.path.file_name().unwrap_or_default().to_string_lossy()
        );
    }
}
