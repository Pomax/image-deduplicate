/// The JPEG starting at `at`, as long as one really starts there. The length the
/// container gives is trusted only as far as the file goes; where there is none,
/// the JPEG's own markers say where it ends.
pub(super) fn jpeg_at(bytes: &[u8], at: usize, len: Option<usize>) -> Option<&[u8]> {
    if !bytes.get(at..at + 3)?.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return None;
    }
    let end = match len {
        Some(len) if len > 4 && at + len <= bytes.len() => at + len,
        _ => at + jpeg_length(&bytes[at..])?,
    };
    let jpeg = bytes.get(at..end)?;
    is_a_picture(jpeg).then_some(jpeg)
}

/// Whether a JPEG is a picture rather than sensor data.
///
/// Canon stores what came off the sensor as a lossless JPEG of two channels,
/// inside the same file and pointed at the same way as the preview, and it is
/// the larger of the two. Nothing decodes that here, and taking it for the
/// preview means indexing nothing at all. The frame header says which it is:
/// a picture is a baseline, extended or progressive frame of one channel or
/// three.
fn is_a_picture(jpeg: &[u8]) -> bool {
    let mut at = 2;
    loop {
        let Some(&byte) = jpeg.get(at) else {
            return false;
        };
        if byte != 0xFF {
            return false;
        }
        let Some(&marker) = jpeg.get(at + 1) else {
            return false;
        };
        match marker {
            0xFF => at += 1,
            0x01 | 0xD0..=0xD8 => at += 2,
            // Start of frame, in every flavour: the number says which, and the
            // three that are pictures this can read are the first three.
            0xC0..=0xCF if marker != 0xC4 && marker != 0xC8 && marker != 0xCC => {
                let components = jpeg.get(at + 9).copied().unwrap_or(0);
                return matches!(marker, 0xC0 | 0xC1 | 0xC2) && matches!(components, 1 | 3);
            }
            0xD9 | 0xDA => return false,
            _ => {
                let Some(length) = jpeg.get(at + 2..at + 4) else {
                    return false;
                };
                let length = u16::from_be_bytes([length[0], length[1]]).max(2) as usize;
                at += 2 + length;
            }
        }
    }
}

/// How long the JPEG at the front of `bytes` is, by walking its markers to the
/// end. A JPEG can hold a smaller JPEG of itself, so the first end marker in the
/// file is not the end of the file.
pub(super) fn jpeg_length(bytes: &[u8]) -> Option<usize> {
    let mut at = 2;
    loop {
        // Markers are a 0xFF and a byte that is not one. Padding between them is
        // allowed and is more 0xFF.
        while *bytes.get(at)? == 0xFF && *bytes.get(at + 1)? == 0xFF {
            at += 1;
        }
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        match marker {
            0xD9 => return Some(at + 2),
            // Start of a picture, restarts and padding carry no length.
            0x01 | 0xD0..=0xD8 => at += 2,
            _ => {
                let length = u16::from_be_bytes(bytes.get(at + 2..at + 4)?.try_into().ok()?);
                at += 2 + length.max(2) as usize;
                if marker == 0xDA {
                    // The compressed picture itself, which has no length: it runs
                    // to the next marker that is not a restart or a stuffed byte.
                    at = scan_past_entropy(bytes, at)?;
                }
            }
        }
    }
}

fn scan_past_entropy(bytes: &[u8], from: usize) -> Option<usize> {
    let mut at = from;
    loop {
        if *bytes.get(at)? == 0xFF {
            let next = *bytes.get(at + 1)?;
            if next != 0x00 && !(0xD0..=0xD7).contains(&next) && next != 0xFF {
                return Some(at);
            }
        }
        at += 1;
    }
}
