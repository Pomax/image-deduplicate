use super::*;

/// PNG keeps text in chunks of its own, and can carry a whole Exif directory in
/// another.
pub(super) fn from_png(bytes: &[u8]) -> Vec<Group> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut out = Vec::new();
    let mut at = 8;
    while at + 8 <= bytes.len() {
        let length = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4])) as usize;
        let Some(kind) = bytes.get(at + 4..at + 8) else {
            break;
        };
        let Some(payload) = bytes.get(at + 8..at + 8 + length) else {
            break;
        };
        match kind {
            b"tEXt" | b"iTXt" => {
                if let Some(split) = payload.iter().position(|byte| *byte == 0) {
                    let name = text_of(&payload[..split]);
                    // An international one has flags and a language between the
                    // name and the text, all of them terminated.
                    let value = if kind == b"iTXt" {
                        let rest = &payload[split + 1..];
                        let after = rest
                            .iter()
                            .enumerate()
                            .filter(|(_, byte)| **byte == 0)
                            .map(|(index, _)| index)
                            .nth(2)
                            .map(|index| index + 1)
                            .unwrap_or(0);
                        String::from_utf8_lossy(&rest[after.min(rest.len())..]).to_string()
                    } else {
                        String::from_utf8_lossy(&payload[split + 1..]).to_string()
                    };
                    entries.push((name, value.trim().to_string()));
                }
            }
            b"eXIf" => out.extend(from_tiff(payload)),
            b"IEND" => break,
            _ => {}
        }
        at += 12 + length;
    }
    if !entries.is_empty() {
        out.push(Group {
            name: String::from("Description"),
            entries,
        });
    }
    out
}

/// GIF holds comments in an extension block.
pub(super) fn from_gif(bytes: &[u8]) -> Vec<Group> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut at = 13;
    while at + 2 < bytes.len() {
        if bytes[at] == 0x21 && bytes[at + 1] == 0xFE {
            let mut text = Vec::new();
            let mut block = at + 2;
            while let Some(length) = bytes.get(block) {
                let length = *length as usize;
                if length == 0 {
                    break;
                }
                if let Some(piece) = bytes.get(block + 1..block + 1 + length) {
                    text.extend_from_slice(piece);
                }
                block += 1 + length;
            }
            entries.push((String::from("Comment"), text_of(&text)));
            at = block + 1;
            continue;
        }
        at += 1;
    }
    if entries.is_empty() {
        return Vec::new();
    }
    vec![Group {
        name: String::from("Description"),
        entries,
    }]
}

/// WebP is a RIFF, and the Exif and XMP sit in chunks of their own.
pub(super) fn from_riff(bytes: &[u8]) -> Vec<Group> {
    let mut out = Vec::new();
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let Some(kind) = bytes.get(at..at + 4) else {
            break;
        };
        let length =
            u32::from_be_bytes([bytes[at + 7], bytes[at + 6], bytes[at + 5], bytes[at + 4]])
                as usize;
        let Some(payload) = bytes.get(at + 8..at + 8 + length) else {
            break;
        };
        match kind {
            b"EXIF" => out.extend(from_tiff(payload)),
            b"XMP " => out.extend(from_xmp(payload)),
            _ => {}
        }
        at += 8 + length + length % 2;
    }
    out
}

/// HEIC and Canon's newer raw files keep theirs in boxes. Canon writes plain
/// TIFF directories; HEIC keeps an Exif item with a four byte header on it.
pub(super) fn from_boxes(bytes: &[u8]) -> Vec<Group> {
    let mut out = Vec::new();
    preview::walk_boxes(bytes, 0, &mut |kind, body| {
        match &kind {
            // Canon's four directories, in boxes named for the order they go
            // in: the file's own, the camera's settings, the camera's private
            // notes, and where the picture was taken.
            b"CMT1" => out.extend(from_tiff_as(body, "Image", Table::Tiff)),
            b"CMT2" => out.extend(from_tiff_as(body, "Settings", Table::Exif)),
            b"CMT4" => out.extend(from_tiff_as(body, "Place", Table::Gps)),
            b"mdat" | b"idat" => {
                if let Some(at) = find(body, b"Exif\0\0") {
                    out.extend(from_tiff(&body[at + 6..]));
                }
                if let Some(at) = find(body, b"<x:xmpmeta") {
                    out.extend(from_xmp(&body[at..]));
                }
            }
            _ => {}
        }
    });
    out
}
