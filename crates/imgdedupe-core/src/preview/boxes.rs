use super::*;

/// Canon's newer raw files, and HEIC, are boxes inside boxes. The preview sits
/// in one of its own; anything else in there that is a picture is HEVC, which
/// this does not read.
pub(super) fn from_boxes(bytes: &[u8]) -> Option<Preview<'_>> {
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
