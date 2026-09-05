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
    Group { name: String::from("File"), entries: vec![(name.to_string(), value)] }
}

/// Every directory a TIFF holds: the file's own, the camera settings hanging off
/// it, where the picture was taken, and the one describing the thumbnail.
fn from_tiff(bytes: &[u8]) -> Vec<Group> {
    from_tiff_as(bytes, "Image", Table::Tiff)
}

/// The same, for a directory that arrives on its own rather than at the front of
/// a file, so what it is has to be said rather than assumed. Canon's newer raw
/// files keep each of theirs in a box of its own.
fn from_tiff_as(bytes: &[u8], name: &str, table: Table) -> Vec<Group> {
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

        let mut group = Group { name, entries: Vec::new() };
        for entry in &entries {
            match entry.tag {
                TAG_EXIF_DIRECTORY => queue.push((
                    entry.value as usize,
                    String::from("Settings"),
                    Table::Exif,
                )),
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
            group.entries.splice(0..0, where_it_was(&entries, bytes, order));
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
fn where_it_was(
    entries: &[preview::Entry],
    bytes: &[u8],
    order: Order,
) -> Vec<(String, String)> {
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
        out.push((String::from("Height"), format!("{} m {sea} sea level", trimmed(height))));
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
enum Table {
    Tiff,
    Exif,
    Gps,
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

/// The captions, credits and keywords, in the numbered fields a wire service
/// would have sent them in.
fn from_iptc(bytes: &[u8]) -> Vec<Group> {
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
    vec![Group { name: String::from("Description"), entries }]
}

/// A JPEG keeps IPTC inside a Photoshop block, which is a run of named pieces of
/// which one is the wire service fields.
fn from_photoshop(bytes: &[u8]) -> Vec<Group> {
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

const XMP_MARK: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Whether a property is one a photographer looks at, and what it is called.
///
/// The name that comes back is the property's own, not another word for it: a
/// file that holds `dc:description` says Description, and if it does not hold
/// one, nothing says Description.
///
/// Everything else is left out. An editor writes hundreds of its own settings
/// in here: how much clarity was applied, where the highlights were pulled to,
/// which curve was used. None of that is about the photograph.
fn xmp_name(property: &str) -> Option<&'static str> {
    // The XML's own scaffolding. `rdf:Description` is the element every property
    // sits inside, and its local name is description, so without this the whole
    // block is read as though it were the photograph's description.
    let (space, name) = property.split_once(':').unwrap_or(("", property));
    if matches!(space, "rdf" | "x" | "xmlns") {
        return None;
    }
    let named = match name.to_ascii_lowercase().as_str() {
        "title" => "Title",
        "description" | "caption" => "Description",
        "headline" => "Headline",
        "subject" | "keywords" => "Keywords",
        "creator" | "byline" => "Creator",
        "credit" => "Credit line",
        "source" => "Source",
        "rights" | "copyrightnotice" => "Copyright notice",
        "usageterms" => "Rights usage terms",
        "instructions" => "Instructions",
        "datecreated" => "Date created",
        "city" => "City",
        "state" | "province" => "State or province",
        "country" => "Country",
        "location" | "sublocation" => "Sublocation",
        "label" => "Label",
        "lens" | "lensmodel" => "Lens",
        _ => return None,
    };
    Some(named)
}

/// Adobe's XML. Whatever wrote it chose what to put in, so this takes the
/// handful of things a photographer's panel shows and leaves the rest.
///
/// A property is read on its own: what is inside it and nothing after it. A
/// property holding a list, which is how keywords are written, becomes one line
/// with the list on it rather than the same name over and over.
fn from_xmp(bytes: &[u8]) -> Vec<Group> {
    let text = String::from_utf8_lossy(bytes);
    let mut entries: Vec<(String, String)> = Vec::new();
    let mut add = |name: &'static str, value: String| {
        let value = value.trim().to_string();
        if value.is_empty() {
            return;
        }
        // A date is a date wherever it was written down.
        let value = if name == "Date created" {
            match said_plainly(&value) {
                Some(said) => said,
                None => return,
            }
        } else {
            value
        };
        match entries.iter_mut().find(|(held, _)| held == name) {
            Some((_, held)) => {
                if !held.split(", ").any(|piece| piece == value) {
                    held.push_str(", ");
                    held.push_str(&value);
                }
            }
            None => entries.push((name.to_string(), value)),
        }
    };

    // The short form, where a property is an attribute of the description.
    for piece in text.split_whitespace() {
        // The last attribute of an element carries the element's own closing
        // bracket, and an element with nothing inside it a slash as well.
        let piece = piece.trim_end_matches('>').trim_end_matches('/');
        if let Some((property, value)) = piece.split_once("=\"") {
            if let (Some(name), Some(value)) = (xmp_name(property), value.strip_suffix('"')) {
                add(name, value.to_string());
            }
        }
    }

    // And the long form, where a property is an element with the value inside
    // it. What is inside is read as far as that property's own closing tag and
    // no further.
    let mut rest = text.as_ref();
    while let Some(open) = rest.find('<') {
        let Some(shut) = rest[open..].find('>') else {
            break;
        };
        let tag = &rest[open + 1..open + shut];
        rest = &rest[open + shut + 1..];
        if tag.starts_with('/') || tag.starts_with('?') || tag.ends_with('/') {
            continue;
        }
        let property = tag.split_whitespace().next().unwrap_or(tag);
        let Some(name) = xmp_name(property) else {
            continue;
        };
        let closing = format!("</{property}>");
        let Some(end) = rest.find(&closing) else {
            continue;
        };
        for value in values_inside(&rest[..end]) {
            add(name, value);
        }
        rest = &rest[end + closing.len()..];
    }

    if entries.is_empty() {
        return Vec::new();
    }
    entries.sort();
    vec![Group { name: String::from("Description"), entries }]
}

/// What is inside one property: the text of it, or the items of the list it
/// holds. Keywords are written as a list, and a list is one line.
fn values_inside(body: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = body;
    let mut plain = String::new();
    while let Some(open) = rest.find('<') {
        plain.push_str(&rest[..open]);
        let Some(shut) = rest[open..].find('>') else {
            break;
        };
        let tag = &rest[open + 1..open + shut];
        rest = &rest[open + shut + 1..];
        if !tag.starts_with("rdf:li") {
            continue;
        }
        let Some(end) = rest.find('<') else {
            break;
        };
        let item = rest[..end].trim();
        if !item.is_empty() {
            out.push(item.to_string());
        }
        rest = &rest[end..];
    }
    if out.is_empty() {
        plain.push_str(rest);
        let plain = plain.trim();
        if !plain.is_empty() {
            out.push(plain.to_string());
        }
    }
    out
}

/// PNG keeps text in chunks of its own, and can carry a whole Exif directory in
/// another.
fn from_png(bytes: &[u8]) -> Vec<Group> {
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
        out.push(Group { name: String::from("Description"), entries });
    }
    out
}

/// GIF holds comments in an extension block.
fn from_gif(bytes: &[u8]) -> Vec<Group> {
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
    vec![Group { name: String::from("Description"), entries }]
}

/// WebP is a RIFF, and the Exif and XMP sit in chunks of their own.
fn from_riff(bytes: &[u8]) -> Vec<Group> {
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
fn from_boxes(bytes: &[u8]) -> Vec<Group> {
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

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|window| window == needle)
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

/// What a tag is worth showing as.
///
/// A number that stands for a word is shown as the word: "Metering: 5" tells
/// nobody anything and "Metering: Multi-segment" tells them what the camera was
/// doing. A number with a unit is shown with its unit. Anything whose meaning
/// cannot be given in either of those ways is not in the tables below at all,
/// because a photograph's panel is not the place for the layout of a sensor's
/// colour filters.
#[derive(Clone, Copy)]
enum Shape {
    Text,
    Number,
    /// Shutter speed, which reads as a fraction of a second.
    Seconds,
    /// An aperture, which reads as f/2.8.
    FStop,
    Millimetres,
    Metres,
    /// Exposure compensation, in stops.
    Stops,
    /// A number that stands for a word.
    Words(&'static [(u32, &'static str)]),
    /// The one number whose meaning is spread over its bits.
    Flash,
    /// A comment, which carries eight bytes in front of it naming the alphabet
    /// it was written in. Those are for whoever reads the bytes, not for
    /// whoever reads the photograph.
    Comment,
    /// A date, which a file writes as 2026:06:13 04:18:34 and a person reads as
    /// June 13, 2026, 4:18:34 am.
    Date,
}

/// One tag worth showing: what it is called and how to say what it holds.
struct Named {
    number: u16,
    name: &'static str,
    shape: Shape,
}

const fn tag(number: u16, name: &'static str, shape: Shape) -> Named {
    Named { number, name, shape }
}

/// One tag's value as something worth reading, or nothing when what it holds
/// cannot be said in words.
fn value_of(entry: &preview::Entry, bytes: &[u8], order: Order, shape: Shape) -> Option<String> {
    let raw = entry.bytes(bytes, order)?;
    let value = match shape {
        Shape::Text => {
            let text = text_of(raw);
            if text.is_empty() {
                return None;
            }
            text
        }
        Shape::Number => {
            if entry.count != 1 {
                return None;
            }
            first_number(entry, raw, order)?.to_string()
        }
        Shape::Seconds => {
            let seconds = first_ratio(entry, raw, order)?;
            if seconds >= 1.0 {
                format!("{seconds:.1} s")
            } else if seconds > 0.0 {
                format!("1/{:.0} s", 1.0 / seconds)
            } else {
                return None;
            }
        }
        Shape::FStop => format!("f/{}", trimmed(first_ratio(entry, raw, order)?)),
        Shape::Millimetres => format!("{} mm", trimmed(first_ratio(entry, raw, order)?)),
        Shape::Metres => format!("{} m", trimmed(first_ratio(entry, raw, order)?)),
        Shape::Stops => {
            let stops = first_ratio(entry, raw, order)?;
            if stops == 0.0 {
                String::from("0")
            } else {
                format!("{}{} EV", if stops > 0.0 { "+" } else { "" }, trimmed(stops))
            }
        }
        Shape::Words(words) => {
            let value = first_number(entry, raw, order)?;
            words.iter().find(|(number, _)| *number == value).map(|(_, word)| word.to_string())?
        }
        Shape::Flash => flash(first_number(entry, raw, order)?),
        Shape::Date => said_plainly(&text_of(raw))?,
        Shape::Comment => {
            let said = text_of(raw.get(8..).unwrap_or_default());
            if said.is_empty() {
                return None;
            }
            said
        }
    };
    Some(value)
}

/// The first number of a tag, whichever width it was written in.
fn first_number(entry: &preview::Entry, raw: &[u8], order: Order) -> Option<u32> {
    match entry.kind {
        1 | 6 | 7 => raw.first().map(|byte| *byte as u32),
        3 | 8 => order.short(raw, 0).map(u32::from),
        4 | 9 => order.long(raw, 0),
        _ => None,
    }
}

/// The first rational of a tag, as a number. Exif writes anything that is not a
/// whole number as two numbers, one over the other.
fn first_ratio(entry: &preview::Entry, raw: &[u8], order: Order) -> Option<f64> {
    if !matches!(entry.kind, 5 | 10) {
        return first_number(entry, raw, order).map(f64::from);
    }
    let top = order.long(raw, 0)?;
    let bottom = order.long(raw, 4)?;
    if bottom == 0 {
        return None;
    }
    if entry.kind == 10 {
        return Some(top as i32 as f64 / bottom as i32 as f64);
    }
    Some(top as f64 / bottom as f64)
}

/// A number with no trailing nothing after the point.
fn trimmed(value: f64) -> String {
    format!("{value:.2}").trim_end_matches('0').trim_end_matches('.').to_string()
}

/// A date as somebody would say it.
///
/// A file writes one as 2026:06:13 04:18:34, which is a machine's way of putting
/// it: the parts in the order a computer sorts by, and a clock nobody outside an
/// armed force reads. XMP writes 2026-06-13T04:18:34 instead. Both come out as
/// June 13, 2026, 4:18:34 am.
///
/// Nothing is shown at all when what is there is not a date, because a date
/// nobody can read is worse than no line.
pub fn said_plainly(text: &str) -> Option<String> {
    let numbers: Vec<u32> = text
        .split(|letter: char| !letter.is_ascii_digit())
        .filter(|piece| !piece.is_empty())
        .filter_map(|piece| piece.parse().ok())
        .collect();
    let (year, month, day) = (*numbers.first()?, *numbers.get(1)?, *numbers.get(2)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1800 {
        return None;
    }
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let said = format!("{} {day}, {year}", months[month as usize - 1]);

    let (Some(hour), Some(minute)) = (numbers.get(3), numbers.get(4)) else {
        return Some(said);
    };
    if *hour > 23 || *minute > 59 {
        return Some(said);
    }
    let second = numbers.get(5).copied().unwrap_or(0).min(59);
    let (clock, half) = match hour {
        0 => (12, "am"),
        1..=11 => (*hour, "am"),
        12 => (12, "pm"),
        _ => (hour - 12, "pm"),
    };
    Some(format!("{said}, {clock}:{minute:02}:{second:02} {half}"))
}

/// What the flash did, which one number says in its bits: whether it fired, and
/// whether the camera was trying to keep it from turning eyes red.
fn flash(value: u32) -> String {
    if value & 0x20 != 0 {
        return String::from("No flash on this camera");
    }
    let mut said = String::from(if value & 1 != 0 { "Fired" } else { "Did not fire" });
    if value & 0x40 != 0 {
        said.push_str(", red-eye reduction");
    }
    said
}

/// The picture, as the file describes it.
const IMAGE_TAGS: &[Named] = &[
    tag(0x010F, "Make", Shape::Text),
    tag(0x0110, "Model", Shape::Text),
    tag(0x010E, "Description", Shape::Text),
    tag(0x0132, "Changed", Shape::Date),
    tag(0x013B, "Photographer", Shape::Text),
    tag(0x8298, "Copyright", Shape::Text),
    tag(0x9003, "Taken", Shape::Date),
    tag(0x9004, "Digitised", Shape::Date),
];

/// What the camera was set to.
const SETTINGS_TAGS: &[Named] = &[
    tag(0x9003, "Taken", Shape::Date),
    tag(0x9004, "Digitised", Shape::Date),
    tag(0x829A, "Shutter speed", Shape::Seconds),
    tag(0x829D, "Aperture", Shape::FStop),
    tag(0x8827, "ISO", Shape::Number),
    tag(0x8832, "ISO", Shape::Number),
    tag(0x9204, "Exposure compensation", Shape::Stops),
    tag(0x8822, "Exposure program", Shape::Words(PROGRAMS)),
    tag(0xA402, "Exposure mode", Shape::Words(MODES)),
    tag(0x9207, "Metering", Shape::Words(METERING)),
    tag(0x9209, "Flash", Shape::Flash),
    tag(0x9208, "Light source", Shape::Words(LIGHT)),
    tag(0xA403, "White balance", Shape::Words(BALANCE)),
    tag(0x920A, "Focal length", Shape::Millimetres),
    tag(0xA405, "Focal length on 35mm", Shape::Millimetres),
    tag(0x9206, "Subject distance", Shape::Metres),
    tag(0xA406, "Scene", Shape::Words(SCENES)),
    tag(0xA408, "Contrast", Shape::Words(DEGREES)),
    tag(0xA409, "Saturation", Shape::Words(AMOUNTS)),
    tag(0xA40A, "Sharpness", Shape::Words(DEGREES)),
    tag(0xA001, "Colour space", Shape::Words(COLOUR_SPACES)),
    tag(0xA434, "Lens", Shape::Text),
    tag(0xA433, "Lens make", Shape::Text),
    tag(0xA435, "Lens serial", Shape::Text),
    tag(0xA431, "Body serial", Shape::Text),
    tag(0xA430, "Camera owner", Shape::Text),
    tag(0x9286, "Comment", Shape::Comment),
];

/// Where the picture was taken. The coordinates come from several tags at once,
/// so they are put together elsewhere rather than listed here.
const PLACE_TAGS: &[Named] = &[
    tag(0x0008, "Satellites", Shape::Text),
    tag(0x0012, "Map datum", Shape::Text),
    tag(0x001B, "Found by", Shape::Text),
    tag(0x001D, "Date", Shape::Text),
];


const PROGRAMS: &[(u32, &str)] = &[
    (1, "Manual"),
    (2, "Program"),
    (3, "Aperture priority"),
    (4, "Shutter priority"),
    (5, "Creative"),
    (6, "Action"),
    (7, "Portrait"),
    (8, "Landscape"),
];

const MODES: &[(u32, &str)] = &[(0, "Automatic"), (1, "Manual"), (2, "Automatic bracket")];

const METERING: &[(u32, &str)] = &[
    (1, "Average"),
    (2, "Centre weighted"),
    (3, "Spot"),
    (4, "Multi-spot"),
    (5, "Multi-segment"),
    (6, "Partial"),
];

const LIGHT: &[(u32, &str)] = &[
    (1, "Daylight"),
    (2, "Fluorescent"),
    (3, "Tungsten"),
    (4, "Flash"),
    (9, "Fine weather"),
    (10, "Cloudy"),
    (11, "Shade"),
    (17, "Standard light A"),
    (18, "Standard light B"),
    (19, "Standard light C"),
    (24, "Studio tungsten"),
];

const BALANCE: &[(u32, &str)] = &[(0, "Automatic"), (1, "Manual")];

const SCENES: &[(u32, &str)] = &[(0, "Standard"), (1, "Landscape"), (2, "Portrait"), (3, "Night")];

const DEGREES: &[(u32, &str)] = &[(0, "Normal"), (1, "Soft"), (2, "Hard")];

const AMOUNTS: &[(u32, &str)] = &[(0, "Normal"), (1, "Low"), (2, "High")];

const COLOUR_SPACES: &[(u32, &str)] = &[(1, "sRGB"), (2, "Adobe RGB"), (65535, "Uncalibrated")];

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



#[cfg(test)]
mod tests {
    use super::*;

    fn value(groups: &[Group], name: &str) -> Option<String> {
        groups
            .iter()
            .flat_map(|group| group.entries.iter())
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    }

    /// A TIFF directory as a camera writes one: a make, a model, and a rational
    /// for the exposure.
    fn tiff() -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"II\x2a\x00");
        out.extend_from_slice(&8u32.to_le_bytes());
        let entries: [(u16, u16, u32, u32); 4] = [
            (0x010F, 2, 6, 0),
            (0x0110, 2, 6, 0),
            (0x0112, 3, 1, 6),
            (0x8769, 4, 1, 0),
        ];
        out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
        let after = 8 + 2 + entries.len() * 12 + 4;
        let mut values = Vec::new();
        for (index, (tag, kind, count, inline)) in entries.iter().enumerate() {
            out.extend_from_slice(&tag.to_le_bytes());
            out.extend_from_slice(&kind.to_le_bytes());
            out.extend_from_slice(&count.to_le_bytes());
            match index {
                0 => {
                    out.extend_from_slice(&((after + values.len()) as u32).to_le_bytes());
                    values.extend_from_slice(b"NIKON\0");
                }
                1 => {
                    out.extend_from_slice(&((after + values.len()) as u32).to_le_bytes());
                    values.extend_from_slice(b"Z6\0\0");
                }
                3 => {
                    // The camera directory, written after the values.
                    out.extend_from_slice(&((after + values.len() + 10) as u32).to_le_bytes());
                }
                _ => out.extend_from_slice(&inline.to_le_bytes()),
            }
        }
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&values);
        out.extend_from_slice(&[0; 10]);

        // The camera directory: one exposure time, written as a rational after it.
        let camera = out.len();
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0x829Au16.to_le_bytes());
        out.extend_from_slice(&5u16.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&((camera + 2 + 12 + 4) as u32).to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&1u32.to_le_bytes());
        out.extend_from_slice(&250u32.to_le_bytes());
        out
    }

    #[test]
    fn a_cameras_own_directory_is_read() {
        let groups = read(&tiff(), Format::Nef);
        assert_eq!(value(&groups, "Make").as_deref(), Some("NIKON"));
        assert_eq!(value(&groups, "Model").as_deref(), Some("Z6"));

    }

    #[test]
    fn the_camera_settings_hanging_off_it_are_read_too() {
        let groups = read(&tiff(), Format::Nef);
        assert!(
            groups.iter().any(|group| group.name == "Settings"),
            "the directory of settings was not followed"
        );
        assert_eq!(value(&groups, "Shutter speed").as_deref(), Some("1/250 s"));
    }

    #[test]
    fn the_numbered_fields_of_a_wire_service_are_read() {
        let mut block = Vec::new();
        for (field, text) in [(120u8, "A deer in a garden"), (80, "Pomax"), (25, "deer")] {
            block.push(0x1C);
            block.push(2);
            block.push(field);
            block.extend_from_slice(&(text.len() as u16).to_be_bytes());
            block.extend_from_slice(text.as_bytes());
        }
        let groups = from_iptc(&block);
        assert_eq!(value(&groups, "Description").as_deref(), Some("A deer in a garden"));
        assert_eq!(value(&groups, "Creator").as_deref(), Some("Pomax"));
        assert_eq!(value(&groups, "Keywords").as_deref(), Some("deer"));
    }

    /// An editor writes hundreds of its own settings into a file: how much
    /// clarity was applied, what the highlights were pulled to. None of that is
    /// about the photograph, and none of it is shown.
    #[test]
    fn only_what_a_photographer_would_look_at_is_kept() {
        assert_eq!(xmp_name("dc:title"), Some("Title"));
        assert_eq!(xmp_name("dc:subject"), Some("Keywords"));
        assert_eq!(xmp_name("photoshop:City"), Some("City"));
        assert_eq!(
            xmp_name("rdf:Description"),
            None,
            "the element every property sits inside was read as a property"
        );
        assert_eq!(xmp_name("aux:Lens"), Some("Lens"));
        assert_eq!(xmp_name("crs:Clarity2012"), None);
        assert_eq!(xmp_name("crs:Highlights2012"), None);
        assert_eq!(xmp_name("crs:ToneCurveName2012"), None);
        assert_eq!(xmp_name("xmpMM:InstanceID"), None);
    }

    #[test]
    fn adobes_xml_is_read_in_both_of_its_forms() {
        let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
            <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
            <rdf:Description photoshop:City="Vancouver">
            <dc:title>A deer</dc:title>
            </rdf:Description></rdf:RDF></x:xmpmeta>"#;
        let groups = from_xmp(xml.as_bytes());
        assert_eq!(value(&groups, "City").as_deref(), Some("Vancouver"));
        assert_eq!(value(&groups, "Title").as_deref(), Some("A deer"));
    }

    /// A file writes a date the way a machine sorts them, on a clock nobody
    /// outside an armed force reads. Somebody looking at their photographs
    /// reads neither.
    #[test]
    fn a_date_is_said_the_way_somebody_would_say_it() {
        assert_eq!(
            said_plainly("2026:06:13 04:18:34").as_deref(),
            Some("June 13, 2026, 4:18:34 am")
        );
        assert_eq!(
            said_plainly("2026-06-13T16:05:00").as_deref(),
            Some("June 13, 2026, 4:05:00 pm")
        );
        assert_eq!(said_plainly("2026:01:02 00:30:00").as_deref(), Some("January 2, 2026, 12:30:00 am"));
        assert_eq!(said_plainly("2026:07:04 12:00:00").as_deref(), Some("July 4, 2026, 12:00:00 pm"));
        assert_eq!(said_plainly("2026:06:13").as_deref(), Some("June 13, 2026"));
        // Nothing that is not a date, rather than a wrong one.
        assert_eq!(said_plainly("not a date"), None);
        assert_eq!(said_plainly("0000:00:00 00:00:00"), None);
        assert_eq!(said_plainly(""), None);
    }

    #[test]
    fn a_file_that_says_nothing_about_itself_has_nothing_to_show() {
        assert!(read(b"not a picture", Format::Jpeg).is_empty());
        assert!(read(&[], Format::Nef).is_empty());
        assert!(read(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10], Format::Jpeg).is_empty());
    }

    /// The files this reads come off other people's cameras. A length that runs
    /// past the end of the file is a file to give up on, not to read anyway.
    #[test]
    fn a_file_that_lies_about_its_own_lengths_is_survived() {
        let mut lying = tiff();
        // The make now claims to be four thousand bytes long.
        lying[10 + 4] = 0xA0;
        lying[10 + 5] = 0x0F;
        let groups = read(&lying, Format::Nef);
        assert!(groups.iter().all(|group| !group.entries.is_empty()));

        let mut truncated = tiff();
        truncated.truncate(20);
        let _ = read(&truncated, Format::Nef);

        let mut iptc = vec![0x1C, 2, 120, 0xFF, 0xFF];
        iptc.extend_from_slice(b"short");
        let _ = from_iptc(&iptc);
    }
}
