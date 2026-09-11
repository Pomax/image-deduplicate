use crate::format::Format;

/// Whether a file holds more than one frame. Multi-frame files are video in an
/// image container and are not indexed. The answer comes from headers only, so
/// rejecting one costs no decode.
pub fn is_animated(format: Format, bytes: &[u8]) -> bool {
    match format {
        Format::Gif => gif_has_multiple_frames(bytes),
        Format::Png => png_has_actl(bytes),
        Format::WebP => webp_is_animated(bytes),
        // A raw file, a TIFF and a HEIC hold one picture. A TIFF can hold pages
        // and a HEIC can hold a burst, but neither is video in an image
        // container, which is what this is about.
        Format::Jpeg
        | Format::Tiff
        | Format::Heic
        | Format::Cr2
        | Format::Cr3
        | Format::Nef
        | Format::Arw
        | Format::Rw2 => false,
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
        let len = u32::from_be_bytes([bytes[pos], bytes[pos + 1], bytes[pos + 2], bytes[pos + 3]])
            as usize;
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
        let len = u32::from_le_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
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
#[path = "tests/frames.rs"]
mod tests;
