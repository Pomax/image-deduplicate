use std::fmt;

/// The formats this tool treats as images. Anything not in here is not indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Format {
    Jpeg,
    Png,
    Gif,
    WebP,
}

impl Format {
    pub fn as_str(self) -> &'static str {
        match self {
            Format::Jpeg => "jpeg",
            Format::Png => "png",
            Format::Gif => "gif",
            Format::WebP => "webp",
        }
    }

    /// Whether the encoding discards information. Used by the keep score.
    pub fn is_lossy(self) -> bool {
        matches!(self, Format::Jpeg | Format::WebP)
    }
}

impl fmt::Display for Format {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// How many bytes `detect` needs to reach a verdict on every supported format.
pub const SNIFF_LEN: usize = 512;

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
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_each_supported_format() {
        assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
        assert_eq!(detect(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0d"), Some(Format::Png));
        assert_eq!(detect(b"GIF89a\x10\x00\x10\x00"), Some(Format::Gif));
        assert_eq!(detect(b"RIFF\x24\x00\x00\x00WEBPVP8 "), Some(Format::WebP));
    }

    #[test]
    fn rejects_formats_that_are_not_images() {
        // RAW, project files, render data, icon containers and plain text.
        assert_eq!(detect(b"8BPS\x00\x01\x00\x00"), None); // PSD
        assert_eq!(detect(b"%PDF-1.7\n"), None);
        assert_eq!(detect(b"\x00\x00\x01\x00\x02\x00"), None); // ICO
        assert_eq!(detect(b"DDS \x7c\x00\x00\x00"), None);
        assert_eq!(detect(b"#?RADIANCE\n"), None); // Radiance HDR
        assert_eq!(detect(b"P6\n64 64\n255\n"), None); // PNM
        assert_eq!(detect(b"qoif\x00\x00\x01\x00"), None);
        assert_eq!(detect(b"hello, this is a text file"), None);
        assert_eq!(detect(b""), None);
        assert_eq!(detect(b"II\x2a\x00\x08\x00\x00\x00"), None); // TIFF
        assert_eq!(detect(b"MM\x00\x2a\x00\x00\x00\x08"), None);
        assert_eq!(detect(b"BM\x36\x00\x00\x00"), None); // BMP
        assert_eq!(detect(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"), None);
        assert_eq!(detect(b"<?xml version=\"1.0\"?><rss version=\"2.0\"></rss>"), None);
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
        assert!(!Format::Png.is_lossy());
        assert!(!Format::Gif.is_lossy());
    }
}
