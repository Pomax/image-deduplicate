use super::*;

fn picture(path: &std::path::Path) {
    let image = image::RgbImage::from_fn(32, 24, |x, y| {
        image::Rgb([(x * 8) as u8, (y * 8) as u8, 90])
    });
    image::DynamicImage::ImageRgb8(image)
        .save_with_format(path, image::ImageFormat::Png)
        .expect("writing a fixture");
}

/// Nothing is read on the thread that draws: asking is asking, and the answer
/// turns up later. The window has to keep drawing while a file arrives off
/// another machine.
#[test]
fn asking_gives_nothing_back_at_once_and_something_back_later() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.png");
    picture(&path);
    // A PNG with something in it to say. The text goes in as the format keeps
    // it: a name, a zero, and the words.
    let mut bytes = std::fs::read(&path).expect("read");
    let mut chunk: Vec<u8> = Vec::new();
    chunk.extend_from_slice(b"tEXtComment\0a deer in a garden");
    let length = (chunk.len() - 4) as u32;
    let sum = crc(&chunk);
    let mut piece = length.to_be_bytes().to_vec();
    piece.append(&mut chunk);
    piece.extend_from_slice(&sum.to_be_bytes());
    let end = bytes.len() - 12;
    bytes.splice(end..end, piece);
    std::fs::write(&path, &bytes).expect("write");

    let ctx = egui::Context::default();
    let mut held = Metadata::default();
    assert!(
        held.get(1, path.clone(), &ctx).is_empty(),
        "the file was read on this thread"
    );
    assert!(held.reading());

    let waited = std::time::Instant::now();
    loop {
        let groups = held.get(1, path.clone(), &ctx);
        if !groups.is_empty() {
            let text = groups
                .iter()
                .flat_map(|group| group.entries.iter())
                .find(|(name, _)| name == "Comment")
                .map(|(_, value)| value.clone());
            assert_eq!(text.as_deref(), Some("a deer in a garden"));
            break;
        }
        assert!(waited.elapsed().as_secs() < 10, "nothing was ever read");
        std::thread::yield_now();
    }
}

/// The check every PNG chunk carries. Written here because the fixture is a
/// chunk this test adds by hand.
fn crc(bytes: &[u8]) -> u32 {
    let mut value = 0xFFFF_FFFFu32;
    for byte in bytes {
        value ^= *byte as u32;
        for _ in 0..8 {
            value = if value & 1 != 0 {
                0xEDB8_8320 ^ (value >> 1)
            } else {
                value >> 1
            };
        }
    }
    value ^ 0xFFFF_FFFF
}
