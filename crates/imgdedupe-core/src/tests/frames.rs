use super::*;

fn still_gif() -> Vec<u8> {
    let mut out = b"GIF89a".to_vec();
    out.extend_from_slice(&[1, 0, 1, 0, 0x80, 0, 0]); // 1x1, global table of 2 entries
    out.extend_from_slice(&[0, 0, 0, 255, 255, 255]);
    out.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]); // image descriptor
    out.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]); // LZW size, one sub-block, terminator
    out.push(0x3B);
    out
}

#[test]
fn a_still_gif_is_not_animated() {
    assert!(!is_animated(Format::Gif, &still_gif()));
}

#[test]
fn a_gif_with_two_image_descriptors_is_animated() {
    let mut bytes = still_gif();
    bytes.pop(); // drop the trailer
    bytes.extend_from_slice(&[0x2C, 0, 0, 0, 0, 1, 0, 1, 0, 0]);
    bytes.extend_from_slice(&[0x02, 0x02, 0x44, 0x01, 0x00]);
    bytes.push(0x3B);
    assert!(is_animated(Format::Gif, &bytes));
}

fn png_with_chunks(kinds: &[&[u8; 4]]) -> Vec<u8> {
    let mut out = b"\x89PNG\r\n\x1a\n".to_vec();
    for kind in kinds {
        out.extend_from_slice(&0u32.to_be_bytes());
        out.extend_from_slice(*kind);
        out.extend_from_slice(&0u32.to_be_bytes());
    }
    out
}

#[test]
fn a_plain_png_is_not_animated() {
    assert!(!is_animated(
        Format::Png,
        &png_with_chunks(&[b"IHDR", b"IDAT", b"IEND"])
    ));
}

#[test]
fn a_png_with_actl_is_animated() {
    assert!(is_animated(
        Format::Png,
        &png_with_chunks(&[b"IHDR", b"acTL", b"IDAT", b"IEND"])
    ));
}

fn webp_with_vp8x(flags: u8) -> Vec<u8> {
    let mut out = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
    out.extend_from_slice(b"VP8X");
    out.extend_from_slice(&10u32.to_le_bytes());
    out.push(flags);
    out.extend_from_slice(&[0; 9]);
    out
}

#[test]
fn a_still_webp_is_not_animated() {
    let mut out = b"RIFF\x00\x00\x00\x00WEBP".to_vec();
    out.extend_from_slice(b"VP8 ");
    out.extend_from_slice(&4u32.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    assert!(!is_animated(Format::WebP, &out));
    assert!(!is_animated(Format::WebP, &webp_with_vp8x(0x10)));
}

#[test]
fn a_webp_with_the_animation_flag_is_animated() {
    assert!(is_animated(Format::WebP, &webp_with_vp8x(0x02)));
}

#[test]
fn truncated_files_do_not_panic() {
    for len in 0..40 {
        let gif = &still_gif()[..len.min(still_gif().len())];
        let _ = is_animated(Format::Gif, gif);
        let png = png_with_chunks(&[b"IHDR", b"acTL"]);
        let _ = is_animated(Format::Png, &png[..len.min(png.len())]);
        let webp = webp_with_vp8x(0x02);
        let _ = is_animated(Format::WebP, &webp[..len.min(webp.len())]);
    }
}
