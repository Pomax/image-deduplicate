use super::*;

pub(super) const TAG_PANASONIC_JPEG: u16 = 0x002E;
pub(super) const TAG_IMAGE_WIDTH: u16 = 0x0100;
pub(super) const TAG_IMAGE_HEIGHT: u16 = 0x0101;
pub(super) const TAG_COMPRESSION: u16 = 0x0103;
pub(super) const TAG_ORIENTATION: u16 = 0x0112;
pub(super) const TAG_MAKE: u16 = 0x010F;
pub(super) const TAG_STRIP_OFFSET: u16 = 0x0111;
pub(super) const TAG_STRIP_LENGTH: u16 = 0x0117;
pub(super) const TAG_SUB_DIRECTORIES: u16 = 0x014A;
/// The directory of camera settings, which is where the size of the picture the
/// file is of is written when the directories themselves describe pieces of it.
pub(super) const TAG_EXIF_DIRECTORY: u16 = 0x8769;
pub(super) const TAG_EXIF_WIDTH: u16 = 0xA002;
pub(super) const TAG_EXIF_HEIGHT: u16 = 0xA003;
pub(super) const TAG_JPEG_OFFSET: u16 = 0x0201;
pub(super) const TAG_JPEG_LENGTH: u16 = 0x0202;
/// Panasonic records the sensor's size rather than the picture's.
pub(super) const TAG_SENSOR_WIDTH: u16 = 0x0002;
pub(super) const TAG_SENSOR_HEIGHT: u16 = 0x0003;
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

pub(super) fn from_tiff(bytes: &[u8]) -> Option<Preview<'_>> {
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
