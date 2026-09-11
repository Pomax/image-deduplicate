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

const TAG_PANASONIC_JPEG: u16 = 0x002E;
const TAG_IMAGE_WIDTH: u16 = 0x0100;
const TAG_IMAGE_HEIGHT: u16 = 0x0101;
const TAG_COMPRESSION: u16 = 0x0103;
const TAG_ORIENTATION: u16 = 0x0112;
const TAG_MAKE: u16 = 0x010F;
const TAG_STRIP_OFFSET: u16 = 0x0111;
const TAG_STRIP_LENGTH: u16 = 0x0117;
const TAG_SUB_DIRECTORIES: u16 = 0x014A;
/// The directory of camera settings, which is where the size of the picture the
/// file is of is written when the directories themselves describe pieces of it.
const TAG_EXIF_DIRECTORY: u16 = 0x8769;
const TAG_EXIF_WIDTH: u16 = 0xA002;
const TAG_EXIF_HEIGHT: u16 = 0xA003;
const TAG_JPEG_OFFSET: u16 = 0x0201;
const TAG_JPEG_LENGTH: u16 = 0x0202;
/// Panasonic records the sensor's size rather than the picture's.
const TAG_SENSOR_WIDTH: u16 = 0x0002;
const TAG_SENSOR_HEIGHT: u16 = 0x0003;
/// The two ways a directory says its strips hold a JPEG rather than pixels.
const JPEG_COMPRESSION: [u32; 2] = [6, 7];

/// One tag in one directory.
pub(crate) struct Entry {
    pub(crate) tag: u16,
    pub(crate) kind: u16,
    pub(crate) count: u32,
    /// Where the value is, or the value itself when it fits in the four bytes
    /// the directory keeps for it.
    pub(crate) value: u32,
    pub(crate) at: usize,
}

impl Entry {
    pub(crate) fn size(&self) -> usize {
        let unit = match self.kind {
            1 | 2 | 6 | 7 => 1,
            3 | 8 => 2,
            4 | 9 | 11 => 4,
            5 | 10 | 12 => 8,
            _ => 1,
        };
        unit * self.count as usize
    }

    /// The value as a number, whichever width it was written in.
    pub(crate) fn number(&self, bytes: &[u8], order: Order) -> Option<u32> {
        match self.kind {
            3 => order.short(bytes, self.at + 8).map(u32::from),
            4 | 9 => Some(self.value),
            _ => None,
        }
    }

    /// The bytes of the value, from where the directory keeps them.
    pub(crate) fn bytes<'a>(&self, bytes: &'a [u8], _order: Order) -> Option<&'a [u8]> {
        let size = self.size();
        if size <= 4 {
            return bytes.get(self.at + 8..self.at + 8 + size);
        }
        bytes.get(self.value as usize..self.value as usize + size)
    }
}

/// The entries of one directory, if it is inside the file and says a size that
/// is inside the file too.
pub(crate) fn entries(bytes: &[u8], order: Order, at: usize) -> Option<Vec<Entry>> {
    let count = order.short(bytes, at)? as usize;
    let mut out = Vec::with_capacity(count.min(512));
    for index in 0..count {
        let at = at + 2 + index * 12;
        let entry = Entry {
            tag: order.short(bytes, at)?,
            kind: order.short(bytes, at + 2)?,
            count: order.long(bytes, at + 4)?,
            value: order.long(bytes, at + 8)?,
            at,
        };
        out.push(entry);
    }
    Some(out)
}

fn from_tiff(bytes: &[u8]) -> Option<Preview<'_>> {
    let order = byte_order(bytes)?;
    let mut sizes: Vec<(u32, u32)> = Vec::new();
    let mut best: Option<&[u8]> = None;

    // The chain of directories, and the ones hanging off them. Panasonic and
    // Nikon keep the picture in a directory that only the first one points at.
    let mut queue = vec![order.long(bytes, 4)? as usize];
    let mut seen: Vec<usize> = Vec::new();
    while let Some(at) = queue.pop() {
        if seen.len() >= DEPTH || seen.contains(&at) {
            continue;
        }
        seen.push(at);
        let Some(entries) = entries(bytes, order, at) else {
            continue;
        };

        let number = |tag: u16| {
            entries
                .iter()
                .find(|entry| entry.tag == tag)
                .and_then(|entry| entry.number(bytes, order))
        };
        if let (Some(width), Some(height)) = (number(TAG_IMAGE_WIDTH), number(TAG_IMAGE_HEIGHT)) {
            sizes.push((width, height));
        }
        if let (Some(width), Some(height)) = (number(TAG_SENSOR_WIDTH), number(TAG_SENSOR_HEIGHT)) {
            sizes.push((width, height));
        }
        if let (Some(width), Some(height)) = (number(TAG_EXIF_WIDTH), number(TAG_EXIF_HEIGHT)) {
            sizes.push((width, height));
        }

        for candidate in in_directory(bytes, order, &entries) {
            if best.is_none_or(|held| candidate.len() > held.len()) {
                best = Some(candidate);
            }
        }

        for entry in &entries {
            if entry.tag == TAG_EXIF_DIRECTORY {
                queue.push(entry.value as usize);
                continue;
            }
            if entry.tag != TAG_SUB_DIRECTORIES {
                continue;
            }
            // One directory is kept in the entry itself; several are a list of
            // places to look, written elsewhere.
            if entry.count == 1 {
                queue.push(entry.value as usize);
                continue;
            }
            for index in 0..entry.count.min(DEPTH as u32) {
                if let Some(at) = order.long(bytes, entry.value as usize + index as usize * 4) {
                    queue.push(at as usize);
                }
            }
        }

        // The next directory in the chain sits after the entries.
        let count = order.short(bytes, at)? as usize;
        if let Some(next) = order.long(bytes, at + 2 + count * 12) {
            if next != 0 {
                queue.push(next as usize);
            }
        }
    }

    let jpeg = best?;
    // The biggest picture the file describes is the one it is of. The preview is
    // in there too, and is smaller than it by definition.
    let full = sizes
        .into_iter()
        .max_by_key(|(width, height)| *width as u64 * *height as u64);
    Some(Preview { jpeg, full })
}

/// Every JPEG one directory points at: the preview, the thumbnail, and the
/// full-size picture Canon stores as though it were the file's pixels.
fn in_directory<'a>(bytes: &'a [u8], order: Order, entries: &[Entry]) -> Vec<&'a [u8]> {
    let find = |tag: u16| entries.iter().find(|entry| entry.tag == tag);
    let number = |tag: u16| find(tag).and_then(|entry| entry.number(bytes, order));

    let mut places: Vec<(usize, usize)> = Vec::new();
    if let (Some(at), Some(len)) = (number(TAG_JPEG_OFFSET), number(TAG_JPEG_LENGTH)) {
        places.push((at as usize, len as usize));
    }
    // Panasonic writes the whole JPEG into one tag rather than pointing at it.
    if let Some(entry) = find(TAG_PANASONIC_JPEG) {
        places.push((entry.value as usize, entry.size()));
    }
    // Canon's older bodies put the full-size JPEG where a TIFF keeps its pixels,
    // and say so in the compression tag.
    if JPEG_COMPRESSION.contains(&number(TAG_COMPRESSION).unwrap_or(0)) {
        if let (Some(at), Some(len)) = (number(TAG_STRIP_OFFSET), number(TAG_STRIP_LENGTH)) {
            places.push((at as usize, len as usize));
        }
    }

    places
        .into_iter()
        .filter_map(|(at, len)| jpeg_at(bytes, at, Some(len)))
        .collect()
}

/// The JPEG starting at `at`, as long as one really starts there. The length the
/// container gives is trusted only as far as the file goes; where there is none,
/// the JPEG's own markers say where it ends.
fn jpeg_at(bytes: &[u8], at: usize, len: Option<usize>) -> Option<&[u8]> {
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
fn jpeg_length(bytes: &[u8]) -> Option<usize> {
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

/// Canon's newer raw files, and HEIC, are boxes inside boxes. The preview sits
/// in one of its own; anything else in there that is a picture is HEVC, which
/// this does not read.
fn from_boxes(bytes: &[u8]) -> Option<Preview<'_>> {
    let mut best: Option<&[u8]> = None;
    walk_boxes(bytes, 0, &mut |kind, body| {
        if kind != *b"PRVW" && kind != *b"THMB" && kind != *b"mdat" {
            return;
        }
        // The box has a header of its own before the picture, and its length is
        // the box's, so the picture's own markers say where it ends.
        let mut at = 0;
        while let Some(start) = find_start(body, at) {
            match jpeg_at(body, start, None) {
                Some(jpeg) => {
                    if best.is_none_or(|held| jpeg.len() > held.len()) {
                        best = Some(jpeg);
                    }
                    at = start + jpeg.len();
                }
                None => at = start + 2,
            }
        }
    });
    Some(Preview {
        jpeg: best?,
        full: None,
    })
}

fn find_start(bytes: &[u8], from: usize) -> Option<usize> {
    (from..bytes.len().saturating_sub(2)).find(|at| bytes[*at..].starts_with(&[0xFF, 0xD8, 0xFF]))
}

/// Every box in the file, and every box inside the ones that hold others.
pub(crate) fn walk_boxes<'a>(
    bytes: &'a [u8],
    depth: usize,
    found: &mut impl FnMut([u8; 4], &'a [u8]),
) {
    if depth >= DEPTH {
        return;
    }
    let mut at = 0;
    while at + 8 <= bytes.len() {
        let size = u32::from_be_bytes(bytes[at..at + 4].try_into().unwrap_or([0; 4])) as usize;
        let Ok(kind) = <[u8; 4]>::try_from(&bytes[at + 4..at + 8]) else {
            return;
        };
        // A size of zero means the box runs to the end of the file. A size of one
        // means the real size is the eight bytes after the name, which is how
        // every box too big for four bytes is written, and the picture is often
        // behind one of those.
        let mut head = at + 8;
        let end = match size {
            0 => bytes.len(),
            1 => {
                let Some(long) = bytes.get(at + 8..at + 16) else {
                    return;
                };
                head = at + 16;
                let size = u64::from_be_bytes(long.try_into().unwrap_or([0; 8]));
                at.saturating_add(size.min(bytes.len() as u64) as usize)
            }
            _ => at + size,
        };
        // A file cut short says a size the file does not reach. What is there is
        // still worth reading.
        let end = end.min(bytes.len());
        if end <= head {
            return;
        }
        let body = &bytes[head..end];
        found(kind, body);
        if CONTAINERS.contains(&&kind) {
            walk_boxes(body, depth + 1, found);
        }
        // Canon hides theirs behind a sixteen byte name, and the boxes it holds
        // start after it.
        if &kind == b"uuid" && body.len() > 16 {
            walk_boxes(&body[16..], depth + 1, found);
        }
        at = end;
    }
}

/// The boxes that hold other boxes rather than data.
const CONTAINERS: [&[u8; 4]; 6] = [b"moov", b"trak", b"mdia", b"minf", b"stbl", b"meta"];

#[cfg(test)]
#[path = "tests/preview.rs"]
mod tests;
