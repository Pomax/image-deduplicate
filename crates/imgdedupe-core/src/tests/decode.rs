use super::*;

#[test]
fn fit_within_preserves_the_long_edge_and_aspect() {
    assert_eq!(fit_within(4000, 3000, 128), (128, 96));
    assert_eq!(fit_within(3000, 4000, 128), (96, 128));
    assert_eq!(fit_within(500, 500, 128), (128, 128));
}

#[test]
fn fit_within_leaves_small_images_alone() {
    assert_eq!(fit_within(100, 60, 128), (100, 60));
    assert_eq!(fit_within(128, 128, 128), (128, 128));
}

#[test]
fn fit_within_never_returns_a_zero_edge() {
    let (w, h) = fit_within(10000, 3, 128);
    assert!(w >= 1 && h >= 1, "got {w}x{h}");
}

fn encode(image: &RgbImage, format: image::ImageFormat) -> Vec<u8> {
    let mut out = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(image.clone())
        .write_to(&mut out, format)
        .expect("encoding a fixture");
    out.into_inner()
}

fn gradient(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([
            (x * 255 / width.max(1)) as u8,
            (y * 255 / height.max(1)) as u8,
            96,
        ])
    })
}

#[test]
fn decodes_png_and_reports_the_original_size() {
    let bytes = encode(&gradient(300, 200), image::ImageFormat::Png);
    let decoded = decode_at_most(Format::Png, &bytes, 64).expect("decode");
    assert_eq!((decoded.width, decoded.height), (300, 200));
    assert_eq!((decoded.small.width(), decoded.small.height()), (64, 42));
}

#[test]
fn decodes_jpeg_and_reports_the_original_size() {
    let bytes = encode(&gradient(320, 240), image::ImageFormat::Jpeg);
    let decoded = decode_at_most(Format::Jpeg, &bytes, 64).expect("decode");
    assert_eq!((decoded.width, decoded.height), (320, 240));
    assert!(decoded.small.width() <= 64 && decoded.small.height() <= 64);
}

#[test]
fn jpeg_scaled_decode_produces_a_buffer_smaller_than_the_original() {
    // The point of the fast path: the reconstructed image is never full size.
    let bytes = encode(&gradient(800, 600), image::ImageFormat::Jpeg);
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(&bytes));
    decoder.read_info().expect("header");
    let (w, h) = decoder.scale(100, 75).expect("scale");
    assert!(w < 800 && h < 600, "scaled decode returned {w}x{h}");
}

#[test]
fn decodes_gif() {
    for format in [image::ImageFormat::Gif] {
        let bytes = encode(&gradient(64, 48), format);
        let detected = crate::format::detect(&bytes).expect("sniff");
        let decoded = decode_at_most(detected, &bytes, 32).expect("decode");
        assert_eq!((decoded.width, decoded.height), (64, 48));
    }
}

#[test]
fn a_grayscale_jpeg_reports_one_channel() {
    let gray = image::GrayImage::from_fn(64, 64, |x, _| image::Luma([(x * 4) as u8]));
    let mut out = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageLuma8(gray)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .expect("encoding");
    let decoded = decode_at_most(Format::Jpeg, &out.into_inner(), 32).expect("decode");
    assert_eq!(decoded.channels, 1);
}

/// The eight ways a camera can say a picture goes, done to a picture whose
/// corners are all different, so which corner ends up where is the answer.
#[test]
fn a_picture_is_turned_the_way_the_file_says() {
    let picture = RgbImage::from_fn(4, 2, |x, y| image::Rgb([(x * 60) as u8, (y * 60) as u8, 0]));
    let corner = |image: &RgbImage, x: u32, y: u32| image.get_pixel(x, y).0;

    // 1 is upright, and 3 is upside down: the far corner becomes the near one.
    assert_eq!(turn_upright(picture.clone(), 1).dimensions(), (4, 2));
    assert_eq!(corner(&turn_upright(picture.clone(), 1), 0, 0), [0, 0, 0]);
    assert_eq!(
        corner(&turn_upright(picture.clone(), 3), 0, 0),
        [180, 60, 0]
    );
    // 2 is a mirror, so the near corner comes from the other end of the row.
    assert_eq!(corner(&turn_upright(picture.clone(), 2), 0, 0), [180, 0, 0]);
    assert_eq!(corner(&turn_upright(picture.clone(), 4), 0, 0), [0, 60, 0]);

    // The quarter turns swap the axes.
    for way_up in [5, 6, 7, 8] {
        assert_eq!(
            turn_upright(picture.clone(), way_up).dimensions(),
            (2, 4),
            "{way_up} did not put the picture on its end"
        );
    }
    // Turned a quarter clockwise, the bottom left corner is now top left.
    assert_eq!(corner(&turn_upright(picture.clone(), 6), 0, 0), [0, 60, 0]);
    // And the other way for a quarter anticlockwise.
    assert_eq!(corner(&turn_upright(picture.clone(), 8), 0, 0), [180, 0, 0]);

    // Anything else is left alone rather than guessed at.
    assert_eq!(corner(&turn_upright(picture.clone(), 0), 0, 0), [0, 0, 0]);
    assert_eq!(corner(&turn_upright(picture, 99), 0, 0), [0, 0, 0]);
}

#[test]
fn truncated_input_is_an_error_and_not_a_panic() {
    let bytes = encode(&gradient(64, 64), image::ImageFormat::Png);
    assert!(decode_at_most(Format::Png, &bytes[..20], 32).is_err());
}

#[test]
fn decodes_tiff_and_reports_the_original_size() {
    let bytes = encode(&gradient(200, 150), image::ImageFormat::Tiff);
    let detected = crate::format::detect(&bytes).expect("sniff");
    assert_eq!(detected, Format::Tiff);
    let decoded = decode_at_most(detected, &bytes, 64).expect("decode");
    assert_eq!((decoded.width, decoded.height), (200, 150));
    assert_eq!((decoded.small.width(), decoded.small.height()), (64, 48));
}

/// A raw file: the picture comes from the preview inside it, and the size
/// recorded is the sensor's, which is what the file is worth keeping for.
#[test]
fn a_raw_file_is_read_from_its_preview_at_the_size_of_its_own_picture() {
    let preview = encode(&gradient(160, 120), image::ImageFormat::Jpeg);
    let mut file = Vec::new();
    file.extend_from_slice(b"II\x2a\x00");
    file.extend_from_slice(&8u32.to_le_bytes());
    // One directory: the sensor's size, and where the preview went.
    let directory: u32 = 8;
    let entries: [(u16, u16, u32, u32); 4] = [
        (0x0100, 4, 1, 6000),
        (0x0101, 4, 1, 4000),
        (0x0201, 4, 1, directory + 2 + 4 * 12 + 4),
        (0x0202, 4, 1, preview.len() as u32),
    ];
    file.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    for (tag, kind, count, value) in entries {
        file.extend_from_slice(&tag.to_le_bytes());
        file.extend_from_slice(&kind.to_le_bytes());
        file.extend_from_slice(&count.to_le_bytes());
        file.extend_from_slice(&value.to_le_bytes());
    }
    file.extend_from_slice(&0u32.to_le_bytes());
    file.extend_from_slice(&preview);

    let decoded = decode_at_most(Format::Arw, &file, 64).expect("decode");
    assert_eq!(
        (decoded.width, decoded.height),
        (6000, 4000),
        "the preview's size was recorded"
    );
    assert_eq!((decoded.small.width(), decoded.small.height()), (64, 48));
}

#[test]
fn a_raw_file_with_no_preview_in_it_is_an_error_and_not_a_panic() {
    let mut file = Vec::new();
    file.extend_from_slice(b"II\x2a\x00");
    file.extend_from_slice(&8u32.to_le_bytes());
    file.extend_from_slice(&0u16.to_le_bytes());
    file.extend_from_slice(&0u32.to_le_bytes());
    assert!(decode_at_most(Format::Cr2, &file, 32).is_err());
    assert!(decode_at_most(Format::Cr3, b"\x00\x00\x00\x18ftypcrx isom", 32).is_err());
}

#[test]
fn a_heic_file_that_holds_nothing_is_an_error_and_not_a_panic() {
    assert!(decode_at_most(
        Format::Heic,
        b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00",
        32
    )
    .is_err());
}
