use super::*;

/// The captions, credits and keywords, in the numbered fields a wire service
/// would have sent them in.
pub(super) fn from_iptc(bytes: &[u8]) -> Vec<Group> {
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut at = 0;
    while at + 5 <= bytes.len() {
        if bytes[at] != 0x1C {
            at += 1;
            continue;
        }
        let record = bytes[at + 1];
        let field = bytes[at + 2];
        let length = u16::from_be_bytes([bytes[at + 3], bytes[at + 4]]) as usize;
        // The long form, for anything over 32767 bytes, which captions can be.
        let (start, length) = if length & 0x8000 != 0 {
            let count = length & 0x7FFF;
            let mut long = 0usize;
            for index in 0..count {
                long = (long << 8) | *bytes.get(at + 5 + index).unwrap_or(&0) as usize;
            }
            (at + 5 + count, long)
        } else {
            (at + 5, length)
        };
        let Some(value) = bytes.get(start..start + length) else {
            break;
        };
        if record == 2 {
            if let Some(name) = iptc_name(field) {
                let value = text_of(value);
                if !value.is_empty() {
                    entries.push((name.to_string(), value));
                }
            }
        }
        at = start + length;
    }
    if entries.is_empty() {
        return Vec::new();
    }
    vec![Group {
        name: String::from("Description"),
        entries,
    }]
}

/// A JPEG keeps IPTC inside a Photoshop block, which is a run of named pieces of
/// which one is the wire service fields.
pub(super) fn from_photoshop(bytes: &[u8]) -> Vec<Group> {
    let Some(start) = find(bytes, b"Photoshop 3.0\0") else {
        return Vec::new();
    };
    let mut at = start + 14;
    while at + 12 <= bytes.len() {
        if &bytes[at..at + 4] != b"8BIM" {
            at += 1;
            continue;
        }
        let kind = u16::from_be_bytes([bytes[at + 4], bytes[at + 5]]);
        // A name nobody uses, padded to an even length.
        let name_length = bytes[at + 6] as usize;
        let mut after = at + 7 + name_length;
        if after % 2 != 0 {
            after += 1;
        }
        let Some(length) = bytes.get(after..after + 4) else {
            break;
        };
        let length = u32::from_be_bytes(length.try_into().unwrap_or([0; 4])) as usize;
        let payload = after + 4;
        if kind == 0x0404 {
            if let Some(block) = bytes.get(payload..payload + length) {
                return from_iptc(block);
            }
        }
        at = payload + length + length % 2;
    }
    Vec::new()
}

fn iptc_name(field: u8) -> Option<&'static str> {
    Some(match field {
        5 => "Title",
        25 => "Keywords",
        40 => "Instructions",
        55 => "Date created",
        80 => "Creator",
        90 => "City",
        92 => "Sublocation",
        95 => "State or province",
        101 => "Country",
        105 => "Headline",
        110 => "Credit line",
        115 => "Source",
        116 => "Copyright notice",
        118 => "Contact",
        120 => "Description",
        _ => return None,
    })
}
