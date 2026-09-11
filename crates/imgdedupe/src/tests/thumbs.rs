use super::*;
use image::{DynamicImage, RgbImage};

fn write(path: &Path, width: u32, height: u32) {
    let image = RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 120])
    });
    DynamicImage::ImageRgb8(image)
        .save_with_format(path, image::ImageFormat::Png)
        .expect("writing a fixture");
}

/// A camera held on its side writes a wide picture and a number saying to
/// turn it. Both the tiles and the preview come through here, so this is
/// where the turn has to happen, and a picture that arrives on its end is
/// the proof it did.
#[test]
fn a_picture_the_file_says_to_turn_arrives_turned() {
    let dir = tempfile::tempdir().expect("tempdir");
    let wide = image::RgbImage::from_fn(400, 200, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    });
    let mut bytes = std::io::Cursor::new(Vec::new());
    DynamicImage::ImageRgb8(wide)
        .write_to(&mut bytes, image::ImageFormat::Jpeg)
        .expect("encoding a fixture");
    let mut bytes = bytes.into_inner();

    // The segment a camera writes to say which way up the picture goes: a
    // quarter turn clockwise.
    let mut segment = vec![0xFF, 0xE1, 0x00, 0x20];
    segment.extend_from_slice(b"Exif\0\0II\x2a\x00");
    segment.extend_from_slice(&8u32.to_le_bytes());
    segment.extend_from_slice(&1u16.to_le_bytes());
    segment.extend_from_slice(&0x0112u16.to_le_bytes());
    segment.extend_from_slice(&3u16.to_le_bytes());
    segment.extend_from_slice(&1u32.to_le_bytes());
    segment.extend_from_slice(&6u16.to_le_bytes());
    segment.extend_from_slice(&[0, 0]);
    segment.extend_from_slice(&0u32.to_le_bytes());
    bytes.splice(2..2, segment);

    let path = dir.path().join("sideways.jpg");
    std::fs::write(&path, &bytes).expect("writing a fixture");

    let shown = load(&path, THUMB_EDGE).expect("decoded");
    assert!(
        shown.size[1] > shown.size[0],
        "a picture the file says to stand on its end came back lying down: {:?}",
        shown.size
    );
}

#[test]
fn loading_reduces_to_the_preview_size() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.png");
    write(&path, 900, 600);

    let image = load(&path, THUMB_EDGE).expect("decoded");
    assert_eq!(image.size[0], THUMB_EDGE as usize);
    assert_eq!(image.size[1], (THUMB_EDGE * 2 / 3) as usize);
}

/// A photograph is 3000x4000 or thereabouts, and the JPEG decoder can hand
/// back a half, a quarter or an eighth of that without reading the whole
/// thing. The preview edge has to stay under half the long edge, or every
/// preview decodes twelve million pixels and the pane sits empty while it
/// does. This is the check on that.
#[test]
fn the_preview_edge_leaves_a_photograph_on_the_half_scale_path() {
    let (width, height) = (3000u32, 4000u32);
    assert!(
        LARGE_EDGE <= height / 2,
        "asking for {LARGE_EDGE} of a {width}x{height} picture decodes it whole"
    );

    // And still larger than a pane can show, or the preview would be soft.
    assert!(
        LARGE_EDGE >= 1200,
        "{LARGE_EDGE} is smaller than the pane it fills"
    );
}

/// The picture beside the list is decoded from the same file at a larger
/// edge, so a thumbnail is not what gets stretched across the pane.
#[test]
fn the_large_edge_gives_a_bigger_image_than_the_thumbnail_edge() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("a.png");
    write(&path, 3000, 2000);

    let thumb = load(&path, THUMB_EDGE).expect("decoded");
    let large = load(&path, LARGE_EDGE).expect("decoded");
    assert_eq!(thumb.size[0], THUMB_EDGE as usize);
    assert_eq!(large.size[0], LARGE_EDGE as usize);
}

/// The list does not wait for a picture to be scrolled to. Everything is
/// asked for as soon as the sets are known, and nothing that has been read
/// is thrown away again.
#[test]
fn priming_reads_every_picture_and_keeps_them() {
    let dir = tempfile::tempdir().expect("tempdir");
    let mut wanted = Vec::new();
    for index in 0..12 {
        let name = format!("{index}.png");
        write(&dir.path().join(&name), 300, 200);
        wanted.push((index as i64, name));
    }

    let ctx = egui::Context::default();
    let mut thumbs = Thumbnails::new();
    thumbs.prime(
        dir.path(),
        wanted.iter().map(|(id, name)| (*id, name.as_str())),
        THUMB_EDGE,
    );

    let first = &wanted[0];
    while !thumbs.pending.is_empty() {
        thumbs.collect(&ctx);
        thumbs.get(first.0, THUMB_EDGE, dir.path(), &first.1);
    }
    assert_eq!(thumbs.textures.len() + thumbs.ready.len(), wanted.len());
    assert_eq!(thumbs.tally.failed, 0);
}

/// What is being drawn is read before what is not. Everything is asked for
/// at once, then the picture at the far end of the list is drawn, and it has
/// to come back near the front of the answers instead of last.
#[test]
fn a_picture_being_drawn_is_read_before_the_ones_that_are_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let count = 400usize;
    let mut wanted = Vec::new();
    for index in 0..count {
        let name = format!("{index}.png");
        write(&dir.path().join(&name), 300, 200);
        wanted.push((index as i64, name));
    }

    let ctx = egui::Context::default();
    let mut thumbs = Thumbnails::new();
    thumbs.prime(
        dir.path(),
        wanted.iter().map(|(id, name)| (*id, name.as_str())),
        THUMB_EDGE,
    );

    let last = &wanted[count - 1];
    let (took, _) = fill(&mut thumbs, &ctx, dir.path(), std::slice::from_ref(last));
    let others = thumbs.textures.len() - 1;
    assert!(
        others < 50,
        "the picture on screen arrived in {took:.2}s, behind {others} that are not on screen"
    );
}

/// A file id names a file only for as long as the index it came from is the
/// current one. A new result takes everything read under the old one with
/// it, including whatever a worker was in the middle of.
#[test]
fn a_new_result_keeps_nothing_the_last_one_read() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(&dir.path().join("slow.png"), 3000, 2000);
    write(&dir.path().join("quick.png"), 300, 200);
    let slow = (1i64, String::from("slow.png"));
    let quick = (2i64, String::from("quick.png"));

    let ctx = egui::Context::default();
    let mut thumbs = Thumbnails::new();

    // Read one picture right through, and start a slow one.
    let (_, _) = fill(&mut thumbs, &ctx, dir.path(), std::slice::from_ref(&quick));
    assert_eq!(thumbs.textures.len(), 1);
    thumbs.collect(&ctx);
    assert!(thumbs
        .get(slow.0, THUMB_EDGE, dir.path(), &slow.1)
        .is_none());
    thumbs.collect(&ctx);

    thumbs.forget();
    assert!(
        thumbs.textures.is_empty(),
        "a picture from the last result was kept"
    );
    assert!(thumbs.ready.is_empty());
    assert!(thumbs.pending.is_empty());

    // Whatever the workers were inside arrives after the result changed, and
    // is thrown away rather than filed under a number that means something
    // else now.
    let until = std::time::Instant::now() + std::time::Duration::from_millis(1500);
    while std::time::Instant::now() < until {
        thumbs.collect(&ctx);
    }
    assert!(
        thumbs.ready.is_empty() && thumbs.textures.is_empty(),
        "a picture read for the last result was kept for this one"
    );
}

/// A decoded picture becomes a texture on the frame that draws it, not on the
/// frame it arrives. A pass over thousands of pictures must not spend the
/// window's frames uploading ones nobody is looking at.
#[test]
fn a_picture_becomes_a_texture_when_it_is_drawn_and_not_before() {
    let dir = tempfile::tempdir().expect("tempdir");
    let count = 12usize;
    let mut wanted = Vec::new();
    for index in 0..count {
        let name = format!("{index}.png");
        write(&dir.path().join(&name), 300, 200);
        wanted.push((index as i64, name));
    }

    let ctx = egui::Context::default();
    let mut thumbs = Thumbnails::new();
    thumbs.prime(
        dir.path(),
        wanted.iter().map(|(id, name)| (*id, name.as_str())),
        THUMB_EDGE,
    );

    let first = &wanted[0];
    while !thumbs.pending.is_empty() {
        thumbs.collect(&ctx);
        thumbs.get(first.0, THUMB_EDGE, dir.path(), &first.1);
    }

    assert_eq!(
        thumbs.textures.len(),
        1,
        "a picture nobody drew was uploaded"
    );
    assert_eq!(thumbs.ready.len(), count - 1);

    let other = &wanted[5];
    assert!(thumbs
        .get(other.0, THUMB_EDGE, dir.path(), &other.1)
        .is_some());
    assert_eq!(thumbs.textures.len(), 2);
}

/// A picture put in front is left in the queue behind as well. That place
/// must not turn into a second read of the same file.
#[test]
fn a_picture_put_in_front_is_still_only_read_once() {
    let dir = tempfile::tempdir().expect("tempdir");
    let count = 40usize;
    let mut wanted = Vec::new();
    for index in 0..count {
        let name = format!("{index}.png");
        write(&dir.path().join(&name), 300, 200);
        wanted.push((index as i64, name));
    }

    let ctx = egui::Context::default();
    let mut thumbs = Thumbnails::new();
    thumbs.prime(
        dir.path(),
        wanted.iter().map(|(id, name)| (*id, name.as_str())),
        THUMB_EDGE,
    );
    let last = &wanted[count - 1];
    while !thumbs.pending.is_empty() {
        thumbs.collect(&ctx);
        thumbs.get(last.0, THUMB_EDGE, dir.path(), &last.1);
    }

    assert_eq!(thumbs.textures.len() + thumbs.ready.len(), count);
    assert_eq!(
        thumbs.tally.arrived, count as u64,
        "a picture was read twice"
    );
}

/// Scrolling away from a tile before a worker has reached it takes it off
/// them. What the eye is on now is read instead, and does not wait behind it.
#[test]
fn a_tile_that_scrolled_away_does_not_hold_up_the_one_on_screen() {
    let dir = tempfile::tempdir().expect("tempdir");
    write(&dir.path().join("big.png"), 3000, 2000);
    write(&dir.path().join("small.png"), 300, 200);
    let big = (0i64, String::from("big.png"));
    let small = (1i64, String::from("small.png"));

    let ctx = egui::Context::default();
    let mut thumbs = Thumbnails::new();

    // The frame the big picture is on screen for, and the frame after it has
    // been scrolled past.
    thumbs.collect(&ctx);
    assert!(thumbs.get(big.0, THUMB_EDGE, dir.path(), &big.1).is_none());
    thumbs.collect(&ctx);
    assert!(thumbs
        .get(small.0, THUMB_EDGE, dir.path(), &small.1)
        .is_none());

    let (took, _) = fill(&mut thumbs, &ctx, dir.path(), std::slice::from_ref(&small));
    assert!(
        !thumbs.textures.contains_key(&(big.0, THUMB_EDGE)),
        "the picture on screen only arrived once the one scrolled past had, in {took:.2}s"
    );
}

/// Draw the given tiles frame after frame until every one of them has a
/// picture, the way the window does: collect what has arrived, then ask for
/// what is on screen and still missing.
fn fill(
    thumbs: &mut Thumbnails,
    ctx: &egui::Context,
    root: &Path,
    on_screen: &[(i64, String)],
) -> (f64, u32) {
    let started = std::time::Instant::now();
    let mut frames = 0;
    loop {
        thumbs.collect(ctx);
        frames += 1;
        let missing = on_screen
            .iter()
            .filter(|(id, path)| thumbs.get(*id, THUMB_EDGE, root, path).is_none())
            .count();
        if missing == 0 {
            return (started.elapsed().as_secs_f64(), frames);
        }
    }
}

#[test]
fn loading_something_that_is_not_an_image_gives_nothing_rather_than_panicking() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("notes.txt");
    std::fs::write(&path, b"just text").unwrap();
    assert!(load(&path, THUMB_EDGE).is_none());

    let broken = dir.path().join("bad.png");
    std::fs::write(&broken, b"\x89PNG\r\n\x1a\ncut").unwrap();
    assert!(load(&broken, THUMB_EDGE).is_none());

    assert!(load(&dir.path().join("absent.png"), THUMB_EDGE).is_none());
}

#[test]
fn a_small_image_is_not_enlarged() {
    let dir = tempfile::tempdir().expect("tempdir");
    let path = dir.path().join("small.png");
    write(&path, 40, 30);
    let image = load(&path, THUMB_EDGE).expect("decoded");
    assert_eq!(image.size, [40, 30]);

    let large = load(&path, LARGE_EDGE).expect("decoded");
    assert_eq!(large.size, [40, 30]);
}
