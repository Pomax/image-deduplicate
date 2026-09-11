use std::fmt;

/// The formats this tool treats as images. Anything not in here is not indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    Gif,
    WebP,
    Tiff,
    Heic,
    /// Canon, in the TIFF container their older bodies write.
    Cr2,
    /// Canon, in the ISO base media container their newer bodies write.
    Cr3,
    /// Nikon.
    Nef,
    /// Sony.
    Arw,
    /// Panasonic.
    Rw2,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Jpeg => "jpeg",
            Format::Png => "png",
            Format::Gif => "gif",
            Format::WebP => "webp",
            Format::Tiff => "tiff",
            Format::Heic => "heic",
            Format::Cr2 => "cr2",
            Format::Cr3 => "cr3",
            Format::Nef => "nef",
            Format::Arw => "arw",
            Format::Rw2 => "rw2",
        }
    }

    /// The extensions this format is written under, lower case and without the
    /// dot.
    ///
    /// A name is what a walk has before it has read anything, so it is what
    /// decides whether a file is worth reading at all. What the file turns out
    /// to be is still `detect`'s answer, from the bytes.
    pub fn extensions(self) -> &'static [&'static str] {
        match self {
            Format::Jpeg => &["jpg", "jpeg", "jpe", "jfif"],
            Format::Png => &["png"],
            Format::Gif => &["gif"],
            Format::WebP => &["webp"],
            Format::Tiff => &["tif", "tiff"],
            Format::Heic => &["heic", "heif"],
            Format::Cr2 => &["cr2"],
            Format::Cr3 => &["cr3"],
            Format::Nef => &["nef"],
            Format::Arw => &["arw"],
            Format::Rw2 => &["rw2"],
        }
    }

    /// Every format, for anything that has to go through all of them.
    pub fn every() -> &'static [Format] {
        &[
            Format::Jpeg,
            Format::Png,
            Format::Gif,
            Format::WebP,
            Format::Tiff,
            Format::Heic,
            Format::Cr2,
            Format::Cr3,
            Format::Nef,
            Format::Arw,
            Format::Rw2,
        ]
    }

    /// Whether the encoding discards information. Used by the keep score.
    ///
    /// A raw file is what the sensor recorded, so it counts as lossless however
    /// its own compression works: it is the copy to keep over anything exported
    /// from it.
    pub fn is_lossy(self) -> bool {
        matches!(self, Format::Jpeg | Format::WebP | Format::Heic)
    }

    /// Whether the picture is inside the file rather than being the file: a raw
    /// file holds sensor data no decoder here reads, and a preview of it that
    /// every camera writes beside it.
    pub fn is_raw(self) -> bool {
        matches!(
            self,
            Format::Cr2 | Format::Cr3 | Format::Nef | Format::Arw | Format::Rw2
        )
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The format a name claims, or nothing if the name claims none of them.
///
/// What a file is called, not what it is: this is the shortlist a walk draws up
/// before it reads anything, and `detect` is what then confirms or refuses it.
pub fn from_extension(name: &str) -> Option<Format> {
    let dot = name.rfind('.')?;
    let extension = name[dot + 1..].to_ascii_lowercase();
    Format::every()
        .iter()
        .copied()
        .find(|format| format.extensions().contains(&extension.as_str()))
}

/// How many bytes `detect` needs to reach a verdict on every supported format.
///
/// Most of them are decided by the first dozen bytes. The raw formats that share
/// the TIFF header are told apart by the maker's name, which sits behind the
/// first directory of tags, so the window is as wide as that reaches.
pub const SNIFF_LEN: usize = 4096;

/// Identify a format from the leading bytes of a file. Extensions are never consulted.
pub fn detect(head: &[u8]) -> Option<Format> {
    if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some(Format::Jpeg);
    }
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        return Some(Format::Png);
    }
    if head.starts_with(b"GIF87a") || head.starts_with(b"GIF89a") {
        return Some(Format::Gif);
    }
    if head.len() >= 12 && head.starts_with(b"RIFF") && &head[8..12] == b"WEBP" {
        return Some(Format::WebP);
    }
    if let Some(format) = from_brand(head) {
        return Some(format);
    }
    if let Some(format) = from_tiff(head) {
        return Some(format);
    }
    None
}

/// The ISO base media container names itself in a `ftyp` box at the front. Canon
/// writes their newer raw files in it, and so does HEIC.
fn from_brand(head: &[u8]) -> Option<Format> {
    if head.len() < 12 || &head[4..8] != b"ftyp" {
        return None;
    }
    // The major brand, then the compatible brands that follow it. A file that
    // says anywhere in that list what it is, is that.
    let brands = head[8..].chunks_exact(4).take(16);
    for brand in brands {
        match brand {
            b"crx " => return Some(Format::Cr3),
            b"heic" | b"heix" | b"heim" | b"heis" | b"hevc" | b"hevx" | b"mif1" | b"msf1" => {
                return Some(Format::Heic)
            }
            _ => {}
        }
    }
    None
}

/// TIFF, and the raw formats built on it. Panasonic has a header of their own;
/// Canon marks theirs in the two bytes after it; Nikon and Sony write a plain
/// TIFF header and are told apart by the maker's name inside.
fn from_tiff(head: &[u8]) -> Option<Format> {
    if head.starts_with(b"IIU\x00") {
        return Some(Format::Rw2);
    }
    let order = crate::preview::byte_order(head)?;
    if order.is_little() && head.len() >= 11 && &head[8..10] == b"CR" {
        return Some(Format::Cr2);
    }
    match crate::preview::maker(head) {
        Some(maker) if maker.starts_with("NIKON") => Some(Format::Nef),
        Some(maker) if maker.starts_with("SONY") => Some(Format::Arw),
        _ => Some(Format::Tiff),
    }
}

#[cfg(test)]
#[path = "tests/format.rs"]
mod tests;
