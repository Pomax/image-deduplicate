use anyhow::{bail, Context, Result};
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

/// Decode `bytes` to an image whose long edge is at most `target`, using the
/// cheapest route the format allows. Callers never learn which route was taken.
pub fn decode_at_most(format: Format, bytes: &[u8], target: u32) -> Result<Decoded> {
    match format {
        Format::Jpeg => decode_jpeg(bytes, target),
        Format::Png => decode_via_image(bytes, image::ImageFormat::Png, target),
        Format::Gif => decode_via_image(bytes, image::ImageFormat::Gif, target),
        Format::WebP => decode_via_image(bytes, image::ImageFormat::WebP, target),
    }
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

    #[test]
    fn truncated_input_is_an_error_and_not_a_panic() {
        let bytes = encode(&gradient(64, 64), image::ImageFormat::Png);
        assert!(decode_at_most(Format::Png, &bytes[..20], 32).is_err());
    }
}
