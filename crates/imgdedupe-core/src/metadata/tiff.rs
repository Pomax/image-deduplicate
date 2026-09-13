use super::*;

/// Every directory a TIFF holds: the file's own, the camera settings hanging off
/// it, where the picture was taken, and the one describing the thumbnail.
pub(super) fn from_tiff(bytes: &[u8]) -> Vec<Group> {
    from_tiff_as(bytes, "Image", Table::Tiff)
}

/// The same, for a directory that arrives on its own rather than at the front of
/// a file, so what it is has to be said rather than assumed. Canon's newer raw
/// files keep each of theirs in a box of its own.
pub(super) fn from_tiff_as(bytes: &[u8], name: &str, table: Table) -> Vec<Group> {
    let Some(order) = preview::byte_order(bytes) else {
        return Vec::new();
    };
    let Some(first) = order.long(bytes, 4) else {
        return Vec::new();
    };

    let mut out = Vec::new();
    let mut seen: Vec<usize> = Vec::new();
    // The directory, what to call it, and which table names its tags.
    let mut queue = vec![(first as usize, name.to_string(), table)];
    while let Some((at, name, table)) = queue.pop() {
        if seen.len() > 8 || seen.contains(&at) {
            continue;
        }
        seen.push(at);
        let Some(entries) = preview::entries(bytes, order, at) else {
            continue;
        };

        let mut group = Group {
            name,
            entries: Vec::new(),
        };
        for entry in &entries {
            match entry.tag {
                TAG_EXIF_DIRECTORY => {
                    queue.push((entry.value as usize, String::from("Settings"), Table::Exif))
                }
                TAG_GPS_DIRECTORY => {
                    queue.push((entry.value as usize, String::from("Place"), Table::Gps))
                }
                // The captions and credits, kept inside the TIFF rather than in
                // a segment of their own.
                TAG_IPTC => {
                    if let Some(payload) = entry.bytes(bytes, order) {
                        out.extend(from_iptc(payload));
                    }
                }
                TAG_XMP => {
                    if let Some(payload) = entry.bytes(bytes, order) {
                        out.extend(from_xmp(payload));
                    }
                }
                _ => {
                    let known = match table {
                        Table::Tiff => IMAGE_TAGS,
                        Table::Exif => SETTINGS_TAGS,
                        Table::Gps => PLACE_TAGS,
                    };
                    let Some(named) = known.iter().find(|known| known.number == entry.tag) else {
                        continue;
                    };
                    // Two tags can hold the same thing, and a camera that
                    // writes both is a camera that would have said its ISO
                    // twice.
                    let already = group.entries.iter().any(|(name, _)| name == named.name);
                    if already {
                        continue;
                    }
                    if let Some(value) = value_of(entry, bytes, order, named.shape) {
                        group.entries.push((named.name.to_string(), value));
                    }
                }
            }
        }
        // Where it was taken, which takes four tags to say and is one line.
        if table == Table::Gps {
            group
                .entries
                .splice(0..0, where_it_was(&entries, bytes, order));
        }
        out.push(group);

        // The next directory in the chain, which is the thumbnail's. Nothing in
        // it is about the picture, only about the little copy of it, so it is
        // read for its own sake and not shown.
        if let Some(count) = order.short(bytes, at) {
            if let Some(next) = order.long(bytes, at + 2 + count as usize * 12) {
                if next != 0 && seen.len() < 8 {
                    queue.push((next as usize, String::from("Thumbnail"), Table::Tiff));
                }
            }
        }
    }
    out.retain(|group| group.name != "Thumbnail");
    out
}

/// Where the picture was taken, as degrees north and east.
///
/// The file writes a latitude as three numbers and which side of the equator it
/// is on as a separate letter, and the same again for longitude. Four tags for
/// one place, so they are put together here and the parts are not shown.
fn where_it_was(entries: &[preview::Entry], bytes: &[u8], order: Order) -> Vec<(String, String)> {
    let find = |tag: u16| entries.iter().find(|entry| entry.tag == tag);
    let side = |tag: u16| {
        find(tag)
            .and_then(|entry| entry.bytes(bytes, order))
            .and_then(|raw| raw.first().copied())
            .map(|letter| letter as char)
    };
    let degrees = |tag: u16| {
        let entry = find(tag)?;
        let raw = entry.bytes(bytes, order)?;
        if entry.count < 3 {
            return None;
        }
        let part = |index: usize| -> Option<f64> {
            let top = order.long(raw, index * 8)?;
            let bottom = order.long(raw, index * 8 + 4)?;
            (bottom != 0).then(|| top as f64 / bottom as f64)
        };
        Some(part(0)? + part(1)? / 60.0 + part(2)? / 3600.0)
    };

    let mut out = Vec::new();
    if let (Some(north), Some(side)) = (degrees(0x0002), side(0x0001)) {
        out.push((String::from("Latitude"), format!("{north:.5} {side}")));
    }
    if let (Some(east), Some(side)) = (degrees(0x0004), side(0x0003)) {
        out.push((String::from("Longitude"), format!("{east:.5} {side}")));
    }
    if let Some(height) = find(0x0006).and_then(|entry| {
        let raw = entry.bytes(bytes, order)?;
        first_ratio(entry, raw, order)
    }) {
        let below = side(0x0005).is_some_and(|reference| reference as u8 == 1);
        let sea = if below { "below" } else { "above" };
        out.push((
            String::from("Height"),
            format!("{} m {sea} sea level", trimmed(height)),
        ));
    }
    out
}

const TAG_IPTC: u16 = 0x83BB;
const TAG_XMP: u16 = 0x02BC;
const TAG_EXIF_DIRECTORY: u16 = 0x8769;
const TAG_GPS_DIRECTORY: u16 = 0x8825;

/// Which table of names a directory's tags are read against. The same number
/// means different things in each.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum Table {
    Tiff,
    Exif,
    Gps,
}
