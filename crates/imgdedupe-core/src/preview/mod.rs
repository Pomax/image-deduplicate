//! The JPEG a camera writes inside a raw file, and how to find it.
//!
//! A raw file holds what came off the sensor: one value per photosite, in a
//! layout that differs by manufacturer and by body, and that needs the maker's
//! own demosaic to become a picture. Every camera writes a JPEG of that picture
//! beside it, which is what the viewfinder and every other program shows. That
//! JPEG is the same picture, so it is what this tool indexes.
//!
//! Two containers hold all of them. Canon's older bodies, Nikon, Sony and
//! Panasonic write a TIFF whose directories point at the JPEG; Canon's newer
//! bodies write the ISO base media container, where it sits in a box of its own.
//! Nothing here decodes sensor data.

mod boxes;
mod jpeg;
mod tiff;

pub(crate) use self::boxes::*;
use self::jpeg::*;
pub(crate) use self::tiff::*;

/// What was found inside a container: the picture, and how big the file says its
/// own picture is.
pub struct Preview<'a> {
    pub jpeg: &'a [u8],
    /// The size of the picture the file is of, when the file says. A preview is
    /// smaller than the sensor image it was made from, and it is the sensor
    /// image that the index is about: it is what the file is worth keeping for.
    pub full: Option<(u32, u32)>,
}

/// Directories deep enough to reach every preview any of these formats hold,
/// without following a file that points at itself.
const DEPTH: usize = 8;

/// The biggest JPEG inside a container, and the size of the picture it previews.
pub fn find(bytes: &[u8]) -> Option<Preview<'_>> {
    if byte_order(bytes).is_some() || bytes.starts_with(b"IIU\x00") {
        return from_tiff(bytes);
    }
    if bytes.len() >= 12 && &bytes[4..8] == b"ftyp" {
        return from_boxes(bytes);
    }
    None
}

/// Which way up the camera was held, as the file records it.
///
/// A camera writes the picture the way the sensor read it and a number saying
/// what to do with it: leave it, turn it a quarter turn, half a turn, or turn it
/// and flip it. Everything that shows the picture has to do that, or every
/// portrait photograph is on its side.
///
/// The eight values are the ones the TIFF and Exif standards define. 1 is
/// upright, which is also the answer when the file does not say.
pub fn the_way_up(bytes: &[u8]) -> u16 {
    // A JPEG keeps its Exif in a segment near the front, and that segment holds
    // a TIFF of its own.
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return match exif_inside_jpeg(bytes) {
            Some(tiff) => from_tiff_directory(tiff),
            None => 1,
        };
    }
    from_tiff_directory(bytes)
}

/// Where the TIFF inside a JPEG's Exif segment starts.
pub(crate) fn exif_inside_jpeg(bytes: &[u8]) -> Option<&[u8]> {
    let mut at = 2;
    loop {
        if *bytes.get(at)? != 0xFF {
            return None;
        }
        let marker = *bytes.get(at + 1)?;
        // The picture itself has begun and there was no Exif before it.
        if marker == 0xDA || marker == 0xD9 {
            return None;
        }
        let length = u16::from_be_bytes(bytes.get(at + 2..at + 4)?.try_into().ok()?) as usize;
        if marker == 0xE1 && bytes.get(at + 4..at + 10)? == b"Exif\0\0" {
            return bytes.get(at + 10..at + 2 + length.max(2));
        }
        at += 2 + length.max(2);
    }
}

fn from_tiff_directory(bytes: &[u8]) -> u16 {
    let Some(order) = byte_order(bytes) else {
        return 1;
    };
    let Some(first) = order.long(bytes, 4) else {
        return 1;
    };
    let Some(entries) = entries(bytes, order, first as usize) else {
        return 1;
    };
    entries
        .iter()
        .find(|entry| entry.tag == TAG_ORIENTATION)
        .and_then(|entry| entry.number(bytes, order))
        .filter(|value| (1..=8).contains(value))
        .unwrap_or(1) as u16
}

/// Which end of a number comes first in this file, or `None` if it is not a TIFF
/// at all. Panasonic writes `U` where the version number goes; everything else
/// about the file is a TIFF.
pub fn byte_order(bytes: &[u8]) -> Option<Order> {
    match bytes.get(..4)? {
        [b'I', b'I', 0x2A, 0x00] | [b'I', b'I', b'U', 0x00] => Some(Order::Little),
        [b'M', b'M', 0x00, 0x2A] => Some(Order::Big),
        _ => None,
    }
}

/// The name of whoever made the camera, out of the first directory. Used to tell
/// the raw formats that share a plain TIFF header apart from each other.
pub fn maker(bytes: &[u8]) -> Option<String> {
    let order = byte_order(bytes)?;
    let first = order.long(bytes, 4)? as usize;
    for entry in entries(bytes, order, first)? {
        if entry.tag != TAG_MAKE {
            continue;
        }
        let text = entry.bytes(bytes, order)?;
        let end = text
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(text.len());
        return Some(String::from_utf8_lossy(&text[..end]).trim().to_string());
    }
    None
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Order {
    Little,
    Big,
}

impl Order {
    pub fn is_little(self) -> bool {
        self == Order::Little
    }

    pub(crate) fn short(self, bytes: &[u8], at: usize) -> Option<u16> {
        let pair = bytes.get(at..at + 2)?.try_into().ok()?;
        Some(match self {
            Order::Little => u16::from_le_bytes(pair),
            Order::Big => u16::from_be_bytes(pair),
        })
    }

    pub(crate) fn long(self, bytes: &[u8], at: usize) -> Option<u32> {
        let four = bytes.get(at..at + 4)?.try_into().ok()?;
        Some(match self {
            Order::Little => u32::from_le_bytes(four),
            Order::Big => u32::from_be_bytes(four),
        })
    }
}

#[cfg(test)]
#[path = "../tests/preview.rs"]
mod tests;
