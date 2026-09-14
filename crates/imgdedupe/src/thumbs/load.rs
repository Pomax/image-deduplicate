use super::*;

pub(super) fn load(path: &Path, edge: u32) -> Option<ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    let head = &bytes[..bytes.len().min(SNIFF_LEN)];
    let format = format::detect(head)?;
    let decoded = decode_at_most(format, &bytes, edge).ok()?;
    // A camera held on its side writes the picture the way the sensor read it
    // and a number saying which way up it goes. Everything that draws it has to
    // do that turn, and this is the one place either the tiles or the preview
    // gets a picture from.
    let upright = turn_upright(decoded.small, the_way_up(&bytes, format));
    let size = [upright.width() as usize, upright.height() as usize];
    Some(ColorImage::from_rgb(size, upright.as_raw()))
}

/// Which way up the file says its picture goes.
///
/// A raw file is shown through the preview inside it, and the preview is written
/// the way the sensor read it like everything else in there, so the number in
/// the raw's own directory is the one that applies to it.
fn the_way_up(bytes: &[u8], format: Format) -> u16 {
    match format {
        Format::Cr3 | Format::Heic => 1,
        _ => preview::the_way_up(bytes),
    }
}
