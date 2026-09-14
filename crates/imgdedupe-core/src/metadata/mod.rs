//! What a file says about itself: the camera settings, the date, the place, the
//! captions and keywords somebody typed, and everything else written beside the
//! picture.
//!
//! Four things hold it, and a file can carry any of them at once. Exif is a TIFF
//! directory, whether it is a raw file's own or a copy of one inside a JPEG's
//! segment. IPTC is a run of numbered fields, from the days of wire services,
//! and is where captions and credits usually are. XMP is Adobe's XML, and holds
//! whatever the program that wrote it felt like. And PNG, GIF and WebP each have
//! their own place to put text.
//!
//! Nothing here is interpreted beyond making it readable. A tag this has no
//! name for is left out: `Tag 0xA302` tells nobody anything, and a list of
//! numbers is worse than a short list of things somebody can read.

use crate::format::Format;
use crate::preview::{self, Order};

mod containers;
mod iptc;
mod tags;
mod tiff;
mod xmp;

use self::containers::*;
use self::iptc::*;
pub use self::tags::*;
use self::tiff::*;
use self::xmp::*;

/// One heading and what is under it.
pub struct Group {
    pub name: String,
    pub entries: Vec<(String, String)>,
}

/// Everything the file says about itself, grouped by where it was written.
pub fn read(bytes: &[u8], format: Format) -> Vec<Group> {
    let mut out = Vec::new();
    match format {
        Format::Jpeg => {
            if let Some(tiff) = preview::exif_inside_jpeg(bytes) {
                out.extend(from_tiff(tiff));
            }
            for (marker, payload) in jpeg_segments(bytes) {
                match marker {
                    0xE1 if payload.starts_with(XMP_MARK) => {
                        out.extend(from_xmp(&payload[XMP_MARK.len()..]));
                    }
                    0xED => out.extend(from_photoshop(payload)),
                    0xFE => out.push(one("Comment", text_of(payload))),
                    _ => {}
                }
            }
        }
        Format::Png => out.extend(from_png(bytes)),
        Format::Gif => out.extend(from_gif(bytes)),
        Format::WebP => out.extend(from_riff(bytes)),
        Format::Heic | Format::Cr3 => out.extend(from_boxes(bytes)),
        _ => out.extend(from_tiff(bytes)),
    }
    out.retain(|group| !group.entries.is_empty());
    out
}

fn one(name: &str, value: String) -> Group {
    Group {
        name: String::from("File"),
        entries: vec![(name.to_string(), value)],
    }
}

/// Text as written, with the padding and the terminator taken off, and with
/// anything that is not a character to read turned into a space.
///
/// A camera's own notes are bytes it makes its own sense of, and some of them
/// look enough like text to be shown as text. Put on screen as they are, the
/// control characters among them draw as boxes or move the line about.
fn text_of(raw: &[u8]) -> String {
    let end = raw.iter().position(|byte| *byte == 0).unwrap_or(raw.len());
    let readable: String = String::from_utf8_lossy(&raw[..end])
        .chars()
        .map(|letter| if letter.is_control() { ' ' } else { letter })
        .collect();
    readable.split_whitespace().collect::<Vec<&str>>().join(" ")
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}

/// The segments a JPEG holds before the picture itself.
fn jpeg_segments(bytes: &[u8]) -> Vec<(u8, &[u8])> {
    let mut out = Vec::new();
    let mut at = 2;
    while at + 4 <= bytes.len() {
        if bytes[at] != 0xFF {
            break;
        }
        let marker = bytes[at + 1];
        if marker == 0xDA || marker == 0xD9 {
            break;
        }
        let length = u16::from_be_bytes([bytes[at + 2], bytes[at + 3]]).max(2) as usize;
        if let Some(payload) = bytes.get(at + 4..at + 2 + length) {
            out.push((marker, payload));
        }
        at += 2 + length;
    }
    out
}

#[cfg(test)]
#[path = "../tests/metadata.rs"]
mod tests;
