use anyhow::{anyhow, bail, Context, Result};
use image::{imageops, DynamicImage, RgbImage};

use crate::format::Format;

/// Long edge of the reduced image every fingerprint is computed from. Nothing
/// downstream needs more, and every decoder is asked for no more than this.
pub const SMALL_EDGE: u32 = 128;

/// A decoded image reduced to something small, plus the facts about the original
/// that the index records.
pub struct Decoded {
    pub width: u32,
    pub height: u32,
    pub channels: u8,
    pub small: RgbImage,
}

/// Everything a pass needs out of one decode.
///
/// The two fingerprints want the picture at two sizes: the hash wants it small,
/// because it describes the whole frame and 64 pixels is all it reads, and the
/// corners want it larger, because a corner needs a neighbourhood to be
/// described by. Decoding twice is decoding twice; this decodes once, at the
/// larger size, and reduces.
pub struct ForIndexing {
    pub decoded: Decoded,
    /// Grey, at most `features::FEATURE_EDGE` on its long edge.
    pub detail: image::GrayImage,
}

/// Decode once for both fingerprints.
pub fn decode_for_indexing(format: Format, bytes: &[u8]) -> Result<ForIndexing> {
    let decoded = decode_at_most(format, bytes, crate::features::FEATURE_EDGE)?;
    let detail = grey(&decoded.small);
    let small = shrink(decoded.small, SMALL_EDGE);
    Ok(ForIndexing { decoded: Decoded { small, ..decoded }, detail })
}

/// Brightness on its own, by the same weights the hash uses. Corners are found
/// in brightness: colour says nothing about where an edge is.
fn grey(picture: &RgbImage) -> image::GrayImage {
    image::GrayImage::from_fn(picture.width(), picture.height(), |x, y| {
        let [r, g, b] = picture.get_pixel(x, y).0;
        let luma = 0.2126 * r as f32 + 0.7152 * g as f32 + 0.0722 * b as f32;
        image::Luma([luma.round().clamp(0.0, 255.0) as u8])
    })
}

/// Decode `bytes` to an image whose long edge is at most `target`, using the
/// cheapest route the format allows. Callers never learn which route was taken.
pub fn decode_at_most(format: Format, bytes: &[u8], target: u32) -> Result<Decoded> {
    match format {
        Format::Jpeg => decode_jpeg(bytes, target),
        Format::Png => decode_via_image(bytes, image::ImageFormat::Png, target),
        Format::Gif => decode_via_image(bytes, image::ImageFormat::Gif, target),
        Format::WebP => decode_via_image(bytes, image::ImageFormat::WebP, target),
        Format::Tiff => decode_tiff(bytes, target),
        Format::Heic => decode_heic(bytes, target),
        Format::Cr2 | Format::Cr3 | Format::Nef | Format::Arw | Format::Rw2 => {
            decode_preview(bytes, target)
        }
    }
}

/// A TIFF is read like any other picture. Some files with a TIFF header are a
/// raw file wearing one, and hold no picture a TIFF decoder can read; the
/// preview inside them is what those are indexed from.
fn decode_tiff(bytes: &[u8], target: u32) -> Result<Decoded> {
    match decode_via_image(bytes, image::ImageFormat::Tiff, target) {
        Ok(decoded) => Ok(decoded),
        Err(err) => decode_preview(bytes, target).map_err(|_| err),
    }
}

/// The picture inside a raw file.
///
/// What the sensor recorded needs the maker's own demosaic, per manufacturer and
/// per body, to become a picture at all. The JPEG the camera writes beside it is
/// that picture, so it is what the fingerprint is built from. The size recorded
/// is the raw's own, because the file is worth keeping for the picture it holds
/// and not for the size of its preview.
fn decode_preview(bytes: &[u8], target: u32) -> Result<Decoded> {
    let found = crate::preview::find(bytes).context("no preview picture inside this file")?;
    let mut decoded = decode_jpeg(found.jpeg, target).context("decoding the preview inside")?;
    if let Some((width, height)) = found.full {
        if u64::from(width) * u64::from(height)
            > u64::from(decoded.width) * u64::from(decoded.height)
        {
            decoded.width = width;
            decoded.height = height;
        }
    }
    Ok(decoded)
}

/// HEIC holds its picture as HEVC, the video codec, one frame of it.
///
/// The thumbnail beside it is the same picture and costs a fraction of the work,
/// so it is used whenever it is bigger than what the fingerprint is built from.
/// The size recorded is the picture's, not the thumbnail's.
fn decode_heic(bytes: &[u8], target: u32) -> Result<Decoded> {
    let config = heic::DecoderConfig::new();
    let info = heic::ImageInfo::from_bytes(bytes).map_err(|err| anyhow!("reading HEIC: {err}"))?;
    if info.width == 0 || info.height == 0 {
        bail!("HEIC reports a zero dimension");
    }

    let thumbnail = config
        .decode_thumbnail(bytes, heic::PixelLayout::Rgb8)
        .ok()
        .flatten()
        .filter(|thumb| thumb.width.max(thumb.height) >= target);
    let output = match thumbnail {
        Some(thumbnail) => thumbnail,
        None => config
            .decode(bytes, heic::PixelLayout::Rgb8)
            .map_err(|err| anyhow!("decoding HEIC: {err}"))?,
    };

    let rgb = RgbImage::from_raw(output.width, output.height, output.data)
        .context("HEIC pixel buffer does not match its dimensions")?;
    Ok(Decoded {
        width: info.width,
        height: info.height,
        channels: if info.has_alpha { 4 } else { 3 },
        small: shrink(rgb, target),
    })
}

/// The one format with a real fast path: `jpeg-decoder` reconstructs from the DCT
/// coefficients at 1/8, 1/4 or 1/2 scale, so a 24 megapixel file never becomes a
/// 24 megapixel buffer.
fn decode_jpeg(bytes: &[u8], target: u32) -> Result<Decoded> {
    let mut decoder = jpeg_decoder::Decoder::new(std::io::Cursor::new(bytes));
    decoder.read_info().context("reading JPEG header")?;
    let info = decoder.info().context("JPEG header carried no dimensions")?;
    let (width, height) = (info.width as u32, info.height as u32);
    if width == 0 || height == 0 {
        bail!("JPEG reports a zero dimension");
    }

    let (req_w, req_h) = fit_within(width, height, target);
    let (scaled_w, scaled_h) = decoder
        .scale(req_w as u16, req_h as u16)
        .context("selecting a JPEG decode scale")?;
    let pixels = decoder.decode().context("decoding JPEG")?;

    let (sw, sh) = (scaled_w as u32, scaled_h as u32);
    let channels = match info.pixel_format {
        jpeg_decoder::PixelFormat::L8 | jpeg_decoder::PixelFormat::L16 => 1u8,
        jpeg_decoder::PixelFormat::RGB24 => 3,
        jpeg_decoder::PixelFormat::CMYK32 => 3,
    };

    let rgb = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => RgbImage::from_raw(sw, sh, pixels)
            .context("JPEG pixel buffer does not match its dimensions")?,
        jpeg_decoder::PixelFormat::L8 => {
            let gray = image::GrayImage::from_raw(sw, sh, pixels)
                .context("JPEG pixel buffer does not match its dimensions")?;
            DynamicImage::ImageLuma8(gray).to_rgb8()
        }
        jpeg_decoder::PixelFormat::L16 => {
            let samples: Vec<u16> = pixels
                .chunks_exact(2)
                .map(|pair| u16::from_ne_bytes([pair[0], pair[1]]))
                .collect();
            let gray = image::ImageBuffer::<image::Luma<u16>, _>::from_raw(sw, sh, samples)
                .context("JPEG pixel buffer does not match its dimensions")?;
            DynamicImage::ImageLuma16(gray).to_rgb8()
        }
        jpeg_decoder::PixelFormat::CMYK32 => cmyk_to_rgb(sw, sh, &pixels)?,
    };

    Ok(Decoded { width, height, channels, small: shrink(rgb, target) })
}

/// Adobe writes inverted CMYK into JPEG, which is what `jpeg-decoder` hands back.
fn cmyk_to_rgb(width: u32, height: u32, pixels: &[u8]) -> Result<RgbImage> {
    let expected = width as usize * height as usize * 4;
    if pixels.len() < expected {
        bail!("CMYK JPEG pixel buffer is short");
    }
    let mut out = Vec::with_capacity(width as usize * height as usize * 3);
    for px in pixels[..expected].chunks_exact(4) {
        let (c, m, y, k) = (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
        out.push((c * k / 255) as u8);
        out.push((m * k / 255) as u8);
        out.push((y * k / 255) as u8);
    }
    RgbImage::from_raw(width, height, out).context("building RGB from CMYK")
}

/// Everything else decodes in full and is reduced afterwards, because none of
/// these formats can produce a smaller image without reconstructing the whole one.
fn decode_via_image(bytes: &[u8], format: image::ImageFormat, target: u32) -> Result<Decoded> {
    let image = image::load_from_memory_with_format(bytes, format)
        .with_context(|| format!("decoding {format:?}"))?;
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        bail!("image reports a zero dimension");
    }
    let channels = match image.color() {
        image::ColorType::L8 | image::ColorType::L16 => 1,
        image::ColorType::La8 | image::ColorType::La16 => 2,
        image::ColorType::Rgb8 | image::ColorType::Rgb16 | image::ColorType::Rgb32F => 3,
        _ => 4,
    };
    Ok(Decoded { width, height, channels, small: shrink(image.to_rgb8(), target) })
}

/// Put a picture the way up the file says it should be.
///
/// The camera wrote the picture the way the sensor read it and a number saying
/// what to do about the way it was being held. Anything showing the picture has
/// to do that, or every portrait photograph lies on its side. The number comes
/// from `preview::the_way_up`, and 1, which is upright, costs nothing.
pub fn turn_upright(picture: RgbImage, the_way_up: u16) -> RgbImage {
    match the_way_up {
        2 => imageops::flip_horizontal(&picture),
        3 => imageops::rotate180(&picture),
        4 => imageops::flip_vertical(&picture),
        5 => imageops::rotate90(&imageops::flip_horizontal(&picture)),
        6 => imageops::rotate90(&picture),
        7 => imageops::rotate270(&imageops::flip_horizontal(&picture)),
        8 => imageops::rotate270(&picture),
        _ => picture,
    }
}

/// Reduce to the target long edge with a box average, which is both the fastest
/// and the least aliased choice for a large reduction.
fn shrink(image: RgbImage, target: u32) -> RgbImage {
    let (w, h) = (image.width(), image.height());
    if w <= target && h <= target {
        return image;
    }
    let (tw, th) = fit_within(w, h, target);
    imageops::thumbnail(&image, tw.max(1), th.max(1))
}

/// The largest size with the same aspect ratio whose long edge is `target`.
fn fit_within(width: u32, height: u32, target: u32) -> (u32, u32) {
    if width <= target && height <= target {
        return (width, height);
    }
    if width >= height {
        let h = ((height as u64 * target as u64) / width as u64).max(1) as u32;
        (target, h)
    } else {
        let w = ((width as u64 * target as u64) / height as u64).max(1) as u32;
        (w, target)
    }
}

#[cfg(test)]
mod tests {
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
            image::Rgb([(x * 255 / width.max(1)) as u8, (y * 255 / height.max(1)) as u8, 96])
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
        let picture = RgbImage::from_fn(4, 2, |x, y| {
            image::Rgb([(x * 60) as u8, (y * 60) as u8, 0])
        });
        let corner = |image: &RgbImage, x: u32, y: u32| image.get_pixel(x, y).0;

        // 1 is upright, and 3 is upside down: the far corner becomes the near one.
        assert_eq!(turn_upright(picture.clone(), 1).dimensions(), (4, 2));
        assert_eq!(corner(&turn_upright(picture.clone(), 1), 0, 0), [0, 0, 0]);
        assert_eq!(corner(&turn_upright(picture.clone(), 3), 0, 0), [180, 60, 0]);
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
        assert_eq!((decoded.width, decoded.height), (6000, 4000), "the preview's size was recorded");
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
        assert!(decode_at_most(Format::Heic, b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00", 32)
            .is_err());
    }
}
