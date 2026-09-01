use crate::format::Format;

/// Whether a file holds more than one frame. Multi-frame files are video in an
/// image container and are not indexed. The answer comes from headers only, so
/// rejecting one costs no decode.
pub fn is_animated(format: Format, bytes: &[u8]) -> bool {
    match format {
        Format::Gif => gif_has_multiple_frames(bytes),
        Format::Png => png_has_actl(bytes),
        Format::WebP => webp_is_animated(bytes),
        Format::Jpeg => false,
    }
}

/// Walks the GIF block stream counting image descriptors, stopping at two.
fn gif_has_multiple_frames(bytes: &[u8]) -> bool {
    if bytes.len() < 13 {
        return false;
    }
    let packed = bytes[10];
    let mut pos = 13;
    if packed & 0x80 != 0 {
        let entries = 1usize << ((packed & 0x07) + 1);
        pos += entries * 3;
    }

    let mut frames = 0;
    while pos < bytes.len() {
        match bytes[pos] {
            0x2C => {
                frames += 1;
                if frames > 1 {
                    return true;
                }
                // Image descriptor is 10 bytes; a local colour table may follow.
                if pos + 10 > bytes.len() {
                    return false;
                }
                let local = bytes[pos + 9];
                pos += 10;
                if local & 0x80 != 0 {
                    pos += 3 * (1usize << ((local & 0x07) + 1));
                }
                // LZW minimum code size, then the sub-block chain.
                pos += 1;
                pos = match skip_sub_blocks(bytes, pos) {
                    Some(next) => next,
                    None => return false,
                };
            }
            0x21 => {
                // Extension: label byte then a sub-block chain.
                pos += 2;
                pos = match skip_sub_blocks(bytes, pos) {
                    Some(next) => next,
                    None => return false,
                };
            }
            0x3B => return false,
            _ => return false,
        }
    }
    false
}

fn skip_sub_blocks(bytes: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *bytes.get(pos)? as usize;
        pos += 1;
        if len == 0 {
            return Some(pos);
        }
        pos = pos.checked_add(len)?;
        if pos > bytes.len() {
            return None;
        }
    }
}

/// APNG is a PNG carrying an `acTL` chunk, which always precedes the first `IDAT`.
fn png_has_actl(bytes: &[u8]) -> bool {
    let mut pos = 8;
    while pos + 8 <= bytes.len() {
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]]) as usize;
        let kind = &bytes[pos + 4..pos + 8];
        if kind == b"acTL" {
            return true;
        }
        if kind == b"IDAT" || kind == b"IEND" {
            return false;
        }
        pos = match pos.checked_add(12).and_then(|p| p.checked_add(len)) {
            Some(next) => next,
            None => return false,
        };
    }
    false
}

/// An animated WebP is an extended file whose `VP8X` flags set the animation bit.
fn webp_is_animated(bytes: &[u8]) -> bool {
    let mut pos = 12;
    while pos + 8 <= bytes.len() {
        let kind = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes([bytes[pos + 4], bytes[pos + 5], bytes[pos + 6], bytes[pos + 7]]) as usize;
        if kind == b"VP8X" {
            return bytes.get(pos + 8).is_some_and(|flags| flags & 0x02 != 0);
        }
        if kind == b"ANIM" || kind == b"ANMF" {
            return true;
        }
        let padded = len + (len & 1);
        pos = match pos.checked_add(8).and_then(|p| p.checked_add(padded)) {
            Some(next) => next,
            None => return false,
        };
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn still_gif() -> Vec<u8> {
        let mut out = b"GIF89a".to_vec();
        out.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0]); // 1x1, global table of 2 entries
        out.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
        out.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]); // image descriptor
        out.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]); // LZW size, one sub-block, terminator
        out.push(0x3B);
        out
    }

    #[test]
    fn a_still_gif_is_not_animated() {
        assert!(!is_animated(Format::Gif, &still_gif()));
    }

    #[test]
    fn a_gif_with_two_image_descriptors_is_animated() {
        let mut bytes = still_gif();
        bytes.pop(); // drop the trailer
        bytes.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
        bytes.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
        bytes.push(0x3B);
        assert!(is_animated(Format::Gif, &bytes));
    }

    fn png_with_chunks(kinds: &[&[u8; 4]]) -> Vec<u8> {
        let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
        for kind in kinds {
            out.extend_from_slice(&0u32.to_be_bytes());
            out.extend_from_slice(*kind);
            out.extend_from_slice(&0u32.to_be_bytes());
        }
        out
    }

    #[test]
    fn a_plain_png_is_not_animated() {
        assert!(!is_animated(Format::Png, &png_with_chunks(&[b"IHDR", b"IDAT", b"IEND"])));
    }

    #[test]
    fn a_png_with_actl_is_animated() {
        assert!(is_animated(Format::Png, &png_with_chunks(&[b"IHDR", b"acTL", b"IDAT", b"IEND"])));
    }

    fn webp_with_vp8x(flags: u8) -> Vec<u8> {
        let mut out = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
        out.extend_from_slice(b"VP8X");
        out.extend_from_slice(&10u32.to_le_bytes());
        out.push(flags);
        out.extend_from_slice(&[0; 9]);
        out
    }

    #[test]
    fn a_still_webp_is_not_animated() {
        let mut out = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
        out.extend_from_slice(b"VP8 ");
        out.extend_from_slice(&4u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        assert!(!is_animated(Format::WebP, &out));
        assert!(!is_animated(Format::WebP, &webp_with_vp8x(0x10)));
    }

    #[test]
    fn a_webp_with_the_animation_flag_is_animated() {
        assert!(is_animated(Format::WebP, &webp_with_vp8x(0x02)));
    }

    #[test]
    fn truncated_files_do_not_panic() {
        for len in 0..40 {
            let gif = &still_gif()[..len.min(still_gif().len())];
            let _ = is_animated(Format::Gif, gif);
            let png = png_with_chunks(&[b"IHDR", b"acTL"]);
            let _ = is_animated(Format::Png, &png[..len.min(png.len())]);
            let webp = webp_with_vp8x(0x02);
            let _ = is_animated(Format::WebP, &webp[..len.min(webp.len())]);
        }
    }
}
