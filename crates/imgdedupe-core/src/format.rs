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
        matches!(self, Format::Cr2 | Format::Cr3 | Format::Nef | Format::Arw | Format::Rw2)
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
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
mod tests {
    use super::*;

    /// A TIFF header with one directory holding a maker's name, which is what
    /// tells a Nikon or Sony raw file from any other TIFF.
    fn tiff_with_maker(maker: &str) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(b"II\x2a\x00");
        out.extend_from_slice(&8u32.to_le_bytes());
        // One entry: Make, ASCII, as many bytes as the name and its terminator,
        // stored after the directory because it does not fit in four.
        out.extend_from_slice(&1u16.to_le_bytes());
        out.extend_from_slice(&0x010Fu16.to_le_bytes());
        out.extend_from_slice(&2u16.to_le_bytes());
        out.extend_from_slice(&((maker.len() + 1) as u32).to_le_bytes());
        out.extend_from_slice(&26u32.to_le_bytes());
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(maker.as_bytes());
        out.push(0);
        out
    }

    #[test]
    fn detects_each_supported_format() {
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0d"), Some(Format::Png));
        assert_eq!(detect(b"GIF89a\x10\x00\x10\x00"), Some(Format::Gif));
        assert_eq!(detect(b"RIFF\x24\x00\x00\x00WEBPVP8 "), Some(Format::WebP));
        assert_eq!(detect(b"II\x2a\x00\x08\x00\x00\x00"), Some(Format::Tiff));
        assert_eq!(detect(b"MM\x00\x2a\x00\x00\x00\x08"), Some(Format::Tiff));
    }

    #[test]
    fn detects_each_raw_format_and_heic() {
        assert_eq!(detect(b"II\x2a\x00\x10\x00\x00\x00CR\x02\x00"), Some(Format::Cr2));
        assert_eq!(detect(b"IIU\x00\x18\x00\x00\x00"), Some(Format::Rw2));
        assert_eq!(detect(&tiff_with_maker("NIKON CORPORATION")), Some(Format::Nef));
        assert_eq!(detect(&tiff_with_maker("SONY")), Some(Format::Arw));
        assert_eq!(detect(b"\x00\x00\x00\x18ftypcrx isom"), Some(Format::Cr3));
        assert_eq!(detect(b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00"), Some(Format::Heic));
        assert_eq!(detect(b"\x00\x00\x00\x18ftypmif1heic"), Some(Format::Heic));
    }

    #[test]
    fn a_maker_this_tool_has_no_name_for_is_a_tiff() {
        assert_eq!(detect(&tiff_with_maker("Canon")), Some(Format::Tiff));
        assert_eq!(detect(&tiff_with_maker("Hasselblad")), Some(Format::Tiff));
    }

    #[test]
    fn rejects_formats_that_are_not_images() {
        // Project files, render data, icon containers and plain text.
        assert_eq!(detect(b"8BPS\x00\x01\x00\x00"), None); // PSD
        assert_eq!(detect(b"%PDF-1.7\n"), None);
        assert_eq!(detect(b"\x00\x00\x01\x00\x02\x00"), None); // ICO
        assert_eq!(detect(b"DDS \x7c\x00\x00\x00"), None);
        assert_eq!(detect(b"#?RADIANCE\n"), None); // Radiance HDR
        assert_eq!(detect(b"P6\n64 64\n255\n"), None); // PNM
        assert_eq!(detect(b"qoif\x00\x00\x01\x00"), None);
        assert_eq!(detect(b"hello, this is a text file"), None);
        assert_eq!(detect(b""), None);
        assert_eq!(detect(b"BM\x36\x00\x00\x00"), None); // BMP
        assert_eq!(detect(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"), None);
        assert_eq!(detect(b"<?xml version=\"1.0\"?><rss version=\"2.0\"></rss>"), None);
    }

    #[test]
    fn a_container_this_tool_does_not_read_is_not_an_image() {
        // Video and audio in the same container HEIC and CR3 use.
        assert_eq!(detect(b"\x00\x00\x00\x18ftypmp42isom"), None);
        assert_eq!(detect(b"\x00\x00\x00\x18ftypqt  \x00\x00\x00\x00"), None);
        assert_eq!(detect(b"\x00\x00\x00\x18ftypM4A \x00\x00\x00\x00"), None);
    }

    #[test]
    fn riff_that_is_not_webp_is_not_an_image() {
        assert_eq!(detect(b"RIFF\x24\x00\x00\x00WAVEfmt "), None);
        assert_eq!(detect(b"RIFF\x24\x00\x00\x00AVI LIST"), None);
    }

    #[test]
    fn lossiness_matches_the_format() {
        assert!(Format::Jpeg.is_lossy());
        assert!(Format::WebP.is_lossy());
        assert!(Format::Heic.is_lossy());
        assert!(!Format::Png.is_lossy());
        assert!(!Format::Gif.is_lossy());
        assert!(!Format::Tiff.is_lossy());
        assert!(!Format::Cr2.is_lossy());
        assert!(!Format::Cr3.is_lossy());
        assert!(!Format::Nef.is_lossy());
        assert!(!Format::Arw.is_lossy());
        assert!(!Format::Rw2.is_lossy());
    }
}
