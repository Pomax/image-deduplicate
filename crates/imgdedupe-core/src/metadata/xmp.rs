use super::*;

pub(super) const XMP_MARK: &[u8] = b"http://ns.adobe.com/xap/1.0/\0";

/// Whether a property is one a photographer looks at, and what it is called.
///
/// The name that comes back is the property's own, not another word for it: a
/// file that holds `dc:description` says Description, and if it does not hold
/// one, nothing says Description.
///
/// Everything else is left out. An editor writes hundreds of its own settings
/// in here: how much clarity was applied, where the highlights were pulled to,
/// which curve was used. None of that is about the photograph.
pub(super) fn xmp_name(property: &str) -> Option<&'static str> {
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
pub(super) fn from_xmp(bytes: &[u8]) -> Vec<Group> {
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
    vec![Group {
        name: String::from("Description"),
        entries,
    }]
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
