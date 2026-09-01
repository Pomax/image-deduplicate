//! Settles which JPEG decoder the indexer uses.
//!
//! `zune-jpeg` is the faster decoder and has no scaled path. `jpeg-decoder` is
//! slower per unit work but reconstructs at 1/8 scale, giving 64 times fewer
//! output pixels.
//!
//! The margin is much smaller than it looks. A scaled decode still has to entropy
//! decode the entire bitstream; what it skips is the inverse DCT, the chroma
//! upsampling and the colour conversion. On a 4 megapixel file that is worth
//! around 20 percent against `zune-jpeg`, not the several times the arithmetic on
//! output pixels suggests.
//!
//! Both are timed over the whole path the indexer runs, decode and then reduce to
//! the fingerprint size, because the full decoder has a large buffer to shrink
//! afterwards and timing the decode alone would hide that.

use std::time::Instant;

use image::{imageops, DynamicImage, GrayImage, RgbImage};

/// What the indexer reduces to.
const TARGET: u32 = 128;

/// Large enough that the decode dominates the timing rather than the setup.
const WIDTH: u32 = 2400;
const HEIGHT: u32 = 1800;

fn jpeg_fixture() -> Vec<u8> {
    let image = RgbImage::from_fn(WIDTH, HEIGHT, |x, y| {
        let block = ((x / 37) + (y / 53)) % 3;
        let base = [40u8, 150, 220][block as usize];
        image::Rgb([base, base.wrapping_add((x % 61) as u8), (y % 97) as u8])
    });
    let mut buffer = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image)
        .write_to(&mut buffer, image::ImageFormat::Jpeg)
        .expect("encoding the fixture");
    buffer.into_inner()
}

fn scaled_dimensions(bytes: &[u8]) -> (u32, u32) {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    decoder.read_info().expect("header");
    let (w, h) = decoder.scale(WIDTH as u16 / 8, HEIGHT as u16 / 8).expect("scale");
    decoder.decode().expect("decode");
    (w as u32, h as u32)
}

/// Decode at 1/8 scale, then reduce the small result to the fingerprint size.
fn scaled_path(bytes: &[u8]) -> GrayImage {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    decoder.read_info().expect("header");
    let (w, h) = decoder.scale(WIDTH as u16 / 8, HEIGHT as u16 / 8).expect("scale");
    let pixels = decoder.decode().expect("decode");
    let image = RgbImage::from_raw(w as u32, h as u32, pixels).expect("buffer");
    shrink(&image)
}

/// Decode in full, then reduce the full-size result to the fingerprint size.
fn full_path(bytes: &[u8]) -> GrayImage {
    let mut decoder = zune_jpeg::JpegDecoder::new(bytes);
    let pixels = decoder.decode().expect("decode");
    let info = decoder.info().expect("info");
    let image = RgbImage::from_raw(info.width as u32, info.height as u32, pixels).expect("buffer");
    shrink(&image)
}

fn shrink(image: &RgbImage) -> GrayImage {
    let long = image.width().max(image.height());
    let scale = TARGET as f32 / long as f32;
    let width = ((image.width() as f32 * scale) as u32).max(1);
    let height = ((image.height() as f32 * scale) as u32).max(1);
    DynamicImage::ImageRgb8(imageops::thumbnail(image, width, height)).into_luma8()
}

fn time<T>(runs: u32, mut work: impl FnMut() -> T) -> f64 {
    // One untimed run so neither decoder pays for a cold cache the other avoids.
    work();
    let started = Instant::now();
    for _ in 0..runs {
        work();
    }
    started.elapsed().as_secs_f64() * 1000.0 / runs as f64
}

#[test]
fn the_scaled_path_is_the_faster_of_the_two() {
    let bytes = jpeg_fixture();

    let scaled_ms = time(3, || scaled_path(&bytes));
    let full_ms = time(3, || full_path(&bytes));
    println!(
        "{WIDTH}x{HEIGHT} JPEG to {TARGET}px: scaled {scaled_ms:.1}ms, full {full_ms:.1}ms, ratio {:.2}x",
        full_ms / scaled_ms
    );

    // The margin is modest, because entropy decoding the bitstream is unavoidable
    // and is most of the cost. 1.15 is a floor that a slow or loaded machine still
    // clears; the decision itself does not turn on the exact figure, since the
    // scaled path also allocates a two hundredth of the memory.
    assert!(
        scaled_ms * 1.15 < full_ms,
        "scaled path {scaled_ms:.1}ms did not beat full path {full_ms:.1}ms"
    );
}

#[test]
fn the_scaled_path_allocates_a_fraction_of_the_pixels() {
    // The other half of the reason, and the one that does not depend on timing:
    // one buffer per core, so the full path's peak memory is what constrains the
    // thread count on a large folder.
    let bytes = jpeg_fixture();
    let (width, height) = scaled_dimensions(&bytes);
    let scaled_pixels = width as u64 * height as u64;
    let full_pixels = WIDTH as u64 * HEIGHT as u64;
    assert!(
        scaled_pixels * 16 < full_pixels,
        "the scaled decode returned {width}x{height}, not a small fraction of {WIDTH}x{HEIGHT}"
    );
}

#[test]
fn both_paths_reduce_to_the_same_picture() {
    // Guards against the comparison being between a decode and a failure.
    //
    // The two do not agree exactly and are not meant to: an eighth-scale
    // reconstruction from DCT coefficients and a box filter over full-size pixels
    // are different resamplers, and this fixture is deliberately high frequency.
    // What matters is that they land on the same picture, so the bound is
    // relative to an unrelated one rather than an absolute figure.
    let bytes = jpeg_fixture();
    let scaled = scaled_path(&bytes);
    let full = full_path(&bytes);
    assert_eq!(scaled.dimensions(), full.dimensions());

    let mean_difference = |a: &GrayImage, b: &GrayImage| -> f64 {
        a.pixels()
            .zip(b.pixels())
            .map(|(x, y)| (x.0[0] as f64 - y.0[0] as f64).abs())
            .sum::<f64>()
            / (a.width() * a.height()) as f64
    };

    let other = RgbImage::from_fn(WIDTH, HEIGHT, |x, y| {
        image::Rgb([(255 - (x % 256)) as u8, 30, (y % 200) as u8])
    });
    let unrelated = shrink(&other);

    let agreement = mean_difference(&scaled, &full);
    let baseline = mean_difference(&scaled, &unrelated);
    assert!(
        agreement * 3.0 < baseline,
        "the two paths disagree by {agreement} levels, against {baseline} for an unrelated image"
    );
}
