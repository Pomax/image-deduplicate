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

/// The shortlist a walk draws up from names alone. Every format is reachable
/// by each of the extensions it is written under, and a name that claims
/// none of them claims nothing.
#[test]
fn every_extension_names_the_format_it_belongs_to() {
    for format in Format::every() {
        for extension in format.extensions() {
            assert_eq!(
                from_extension(&format!("holiday.{extension}")),
                Some(*format),
                "{extension} did not name {format}"
            );
        }
    }
    for name in [
        "notes.txt",
        "imgdedupe.sqlite",
        "imgdedupe.sqlite-journal",
        "README",
    ] {
        assert_eq!(from_extension(name), None, "{name} was taken for a picture");
    }
}

/// A name is a name however it is typed.
#[test]
fn an_extension_in_capitals_is_the_same_extension() {
    assert_eq!(from_extension("HOLIDAY.JPG"), Some(Format::Jpeg));
    assert_eq!(from_extension("holiday.JpEg"), Some(Format::Jpeg));
    assert_eq!(from_extension("raw.CR2"), Some(Format::Cr2));
}

#[test]
fn detects_each_supported_format() {
    assert_eq!(detect(&[0xFF, 0xD8, 0xFF, 0xE0]), Some(Format::Jpeg));
    assert_eq!(
        detect(b"\x89PNG\r\n\x1a\n\x00\x00\x00\x0d"),
        Some(Format::Png)
    );
    assert_eq!(detect(b"GIF89a\x10\x00\x10\x00"), Some(Format::Gif));
    assert_eq!(detect(b"RIFF\x24\x00\x00\x00WEBPVP8 "), Some(Format::WebP));
    assert_eq!(detect(b"II\x2a\x00\x08\x00\x00\x00"), Some(Format::Tiff));
    assert_eq!(detect(b"MM\x00\x2a\x00\x00\x00\x08"), Some(Format::Tiff));
}

#[test]
fn detects_each_raw_format_and_heic() {
    assert_eq!(
        detect(b"II\x2a\x00\x10\x00\x00\x00CR\x02\x00"),
        Some(Format::Cr2)
    );
    assert_eq!(detect(b"IIU\x00\x18\x00\x00\x00"), Some(Format::Rw2));
    assert_eq!(
        detect(&tiff_with_maker("NIKON CORPORATION")),
        Some(Format::Nef)
    );
    assert_eq!(detect(&tiff_with_maker("SONY")), Some(Format::Arw));
    assert_eq!(detect(b"\x00\x00\x00\x18ftypcrx isom"), Some(Format::Cr3));
    assert_eq!(
        detect(b"\x00\x00\x00\x18ftypheic\x00\x00\x00\x00"),
        Some(Format::Heic)
    );
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
    assert_eq!(
        detect(b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>"),
        None
    );
    assert_eq!(
        detect(b"<?xml version=\"1.0\"?><rss version=\"2.0\"></rss>"),
        None
    );
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
