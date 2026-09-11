use super::*;

fn jpeg(width: u32, height: u32) -> Vec<u8> {
    let picture = image::RgbImage::from_fn(width, height, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 128])
    });
    let mut out = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(picture)
        .write_to(&mut out, image::ImageFormat::Jpeg)
        .expect("encoding a fixture");
    out.into_inner()
}

/// A TIFF holding directories, each a list of tags, with everything the tags
/// point at appended after them. Enough of the format to stand in for the
/// raw files this reads.
struct Builder {
    out: Vec<u8>,
}

struct Tag {
    tag: u16,
    kind: u16,
    count: u32,
    value: u32,
}

fn long(tag: u16, value: u32) -> Tag {
    Tag {
        tag,
        kind: 4,
        count: 1,
        value,
    }
}

fn short(tag: u16, value: u16) -> Tag {
    Tag {
        tag,
        kind: 3,
        count: 1,
        value: value as u32,
    }
}

impl Builder {
    fn new() -> Self {
        let mut out = Vec::new();
        out.extend_from_slice(b"II\x2a\x00");
        out.extend_from_slice(&8u32.to_le_bytes());
        Builder { out }
    }

    /// Put bytes in the file and say where they went.
    fn put(&mut self, bytes: &[u8]) -> u32 {
        let at = self.out.len() as u32;
        self.out.extend_from_slice(bytes);
        at
    }

    /// Write a directory, and say where the next one goes if there is one.
    fn directory(&mut self, tags: &[Tag], next: u32) -> u32 {
        let at = self.out.len() as u32;
        self.out
            .extend_from_slice(&(tags.len() as u16).to_le_bytes());
        for tag in tags {
            self.out.extend_from_slice(&tag.tag.to_le_bytes());
            self.out.extend_from_slice(&tag.kind.to_le_bytes());
            self.out.extend_from_slice(&tag.count.to_le_bytes());
            match tag.kind {
                3 => {
                    self.out
                        .extend_from_slice(&(tag.value as u16).to_le_bytes());
                    self.out.extend_from_slice(&[0, 0]);
                }
                _ => self.out.extend_from_slice(&tag.value.to_le_bytes()),
            }
        }
        self.out.extend_from_slice(&next.to_le_bytes());
        at
    }

    /// Point the file's first directory at `at`.
    fn first(&mut self, at: u32) {
        self.out[4..8].copy_from_slice(&at.to_le_bytes());
    }
}

#[test]
fn the_preview_a_directory_points_at_is_found() {
    let picture = jpeg(64, 48);
    let mut file = Builder::new();
    let where_it_went = file.put(&picture);
    let directory = file.directory(
        &[
            long(TAG_IMAGE_WIDTH, 6000),
            long(TAG_IMAGE_HEIGHT, 4000),
            long(TAG_JPEG_OFFSET, where_it_went),
            long(TAG_JPEG_LENGTH, picture.len() as u32),
        ],
        0,
    );
    file.first(directory);

    let found = find(&file.out).expect("no preview was found");
    assert_eq!(
        found.jpeg, picture,
        "what was found is not the picture that went in"
    );
    assert_eq!(
        found.full,
        Some((6000, 4000)),
        "the sensor's size did not come back"
    );
}

#[test]
fn the_biggest_preview_in_the_file_is_the_one_taken() {
    let small = jpeg(32, 24);
    let large = jpeg(160, 120);
    let mut file = Builder::new();
    let small_at = file.put(&small);
    let large_at = file.put(&large);
    let second = file.directory(
        &[
            long(TAG_JPEG_OFFSET, large_at),
            long(TAG_JPEG_LENGTH, large.len() as u32),
        ],
        0,
    );
    let first = file.directory(
        &[
            long(TAG_JPEG_OFFSET, small_at),
            long(TAG_JPEG_LENGTH, small.len() as u32),
        ],
        second,
    );
    file.first(first);

    let found = find(&file.out).expect("no preview was found");
    assert_eq!(
        found.jpeg, large,
        "the thumbnail was taken over the preview"
    );
}

/// Canon's older bodies write the full-size JPEG where a TIFF keeps pixels.
#[test]
fn a_picture_stored_as_though_it_were_the_files_pixels_is_found() {
    let picture = jpeg(96, 72);
    let mut file = Builder::new();
    let at = file.put(&picture);
    let directory = file.directory(
        &[
            short(TAG_COMPRESSION, 6),
            long(TAG_STRIP_OFFSET, at),
            long(TAG_STRIP_LENGTH, picture.len() as u32),
        ],
        0,
    );
    file.first(directory);

    let found = find(&file.out).expect("no preview was found");
    assert_eq!(found.jpeg, picture);
}

/// Nikon keeps it in a directory that hangs off the first one.
#[test]
fn a_preview_in_a_directory_off_the_first_one_is_found() {
    let picture = jpeg(80, 60);
    let mut file = Builder::new();
    let at = file.put(&picture);
    let sub = file.directory(
        &[
            long(TAG_JPEG_OFFSET, at),
            long(TAG_JPEG_LENGTH, picture.len() as u32),
        ],
        0,
    );
    let first = file.directory(&[long(TAG_SUB_DIRECTORIES, sub)], 0);
    file.first(first);

    let found = find(&file.out).expect("no preview was found");
    assert_eq!(found.jpeg, picture);
}

/// Panasonic writes the picture into a tag rather than pointing at it, and
/// gives the sensor's size rather than the picture's.
#[test]
fn a_preview_written_into_a_tag_is_found() {
    let picture = jpeg(72, 54);
    let mut file = Builder::new();
    let at = file.put(&picture);
    let directory = file.directory(
        &[
            short(TAG_SENSOR_WIDTH, 5184),
            short(TAG_SENSOR_HEIGHT, 3888),
            Tag {
                tag: TAG_PANASONIC_JPEG,
                kind: 7,
                count: picture.len() as u32,
                value: at,
            },
        ],
        0,
    );
    file.first(directory);

    let found = find(&file.out).expect("no preview was found");
    assert_eq!(found.jpeg, picture);
    assert_eq!(found.full, Some((5184, 3888)));
}

/// Canon's newer bodies write boxes inside boxes, and the picture sits in
/// one of them behind a header of its own.
#[test]
fn a_preview_in_a_box_of_its_own_is_found() {
    let picture = jpeg(120, 90);
    let mut inner = Vec::new();
    inner.extend_from_slice(&(picture.len() as u32 + 8 + 12).to_be_bytes());
    inner.extend_from_slice(b"PRVW");
    inner.extend_from_slice(&[0; 12]);
    inner.extend_from_slice(&picture);

    let mut file = Vec::new();
    file.extend_from_slice(&24u32.to_be_bytes());
    file.extend_from_slice(b"ftypcrx isom\x00\x00\x00\x00\x00\x00\x00\x00");
    file.extend_from_slice(&(inner.len() as u32 + 8 + 16).to_be_bytes());
    file.extend_from_slice(b"uuid");
    file.extend_from_slice(&[0xAA; 16]);
    file.extend_from_slice(&inner);

    let found = find(&file).expect("no preview was found");
    assert_eq!(found.jpeg, picture);
}

#[test]
fn a_jpeg_holding_a_smaller_one_is_measured_to_its_own_end() {
    let inner = jpeg(16, 16);
    let mut outer = jpeg(64, 64);
    // A thumbnail of itself, in the segment that metadata goes in, which
    // ends in the same two bytes the file does.
    let mut segment = Vec::new();
    segment.extend_from_slice(&[0xFF, 0xE1]);
    segment.extend_from_slice(&((inner.len() + 2) as u16).to_be_bytes());
    segment.extend_from_slice(&inner);
    outer.splice(2..2, segment);

    assert_eq!(
        jpeg_length(&outer),
        Some(outer.len()),
        "the inner picture ended the outer one"
    );
}

/// What came off the sensor, as Canon stores it: a lossless JPEG of two
/// channels, in the same file, pointed at the same way, and several times
/// the size of the preview. Taking it for the preview means indexing
/// nothing, because nothing here decodes it.
#[test]
fn sensor_data_stored_as_a_lossless_jpeg_is_not_taken_for_the_preview() {
    let picture = jpeg(64, 48);
    let mut sensor = vec![0xFF, 0xD8, 0xFF, 0xC3, 0x00, 0x0E, 0x0E];
    sensor.extend_from_slice(&2624u16.to_be_bytes());
    sensor.extend_from_slice(&3956u16.to_be_bytes());
    sensor.push(2);
    sensor.extend_from_slice(&[0; 6]);
    sensor.resize(picture.len() * 4, 0x5A);

    let mut file = Builder::new();
    let picture_at = file.put(&picture);
    let sensor_at = file.put(&sensor);
    let second = file.directory(
        &[
            short(TAG_COMPRESSION, 6),
            long(TAG_STRIP_OFFSET, sensor_at),
            long(TAG_STRIP_LENGTH, sensor.len() as u32),
        ],
        0,
    );
    let first = file.directory(
        &[
            long(TAG_JPEG_OFFSET, picture_at),
            long(TAG_JPEG_LENGTH, picture.len() as u32),
        ],
        second,
    );
    file.first(first);

    let found = find(&file.out).expect("no preview was found");
    assert_eq!(
        found.jpeg, picture,
        "the sensor data was taken for the picture"
    );
}

/// A box too big for a four byte size says so and writes the size after its
/// name. Canon's newer files put the preview behind one.
#[test]
fn a_box_that_says_its_size_the_long_way_is_still_read() {
    let picture = jpeg(120, 90);
    let mut inner = Vec::new();
    inner.extend_from_slice(&(picture.len() as u32 + 8 + 12).to_be_bytes());
    inner.extend_from_slice(b"PRVW");
    inner.extend_from_slice(&[0; 12]);
    inner.extend_from_slice(&picture);

    let mut file = Vec::new();
    file.extend_from_slice(&24u32.to_be_bytes());
    file.extend_from_slice(b"ftypcrx isom\x00\x00\x00\x00\x00\x00\x00\x00");
    file.extend_from_slice(&1u32.to_be_bytes());
    file.extend_from_slice(b"uuid");
    file.extend_from_slice(&((inner.len() + 16 + 16) as u64).to_be_bytes());
    file.extend_from_slice(&[0xAA; 16]);
    file.extend_from_slice(&inner);

    let found = find(&file).expect("no preview was found");
    assert_eq!(found.jpeg, picture);
}

/// The size of the picture the file is of, when the directories describe
/// pieces of it rather than the whole. Canon writes it with the camera's
/// settings, in a directory of its own.
#[test]
fn the_size_of_the_picture_is_taken_from_the_camera_settings_when_it_is_there() {
    let picture = jpeg(64, 48);
    let mut file = Builder::new();
    let at = file.put(&picture);
    let settings = file.directory(
        &[long(TAG_EXIF_WIDTH, 3456), long(TAG_EXIF_HEIGHT, 2304)],
        0,
    );
    let first = file.directory(
        &[
            long(TAG_IMAGE_WIDTH, 1536),
            long(TAG_IMAGE_HEIGHT, 1024),
            long(TAG_EXIF_DIRECTORY, settings),
            long(TAG_JPEG_OFFSET, at),
            long(TAG_JPEG_LENGTH, picture.len() as u32),
        ],
        0,
    );
    file.first(first);

    let found = find(&file.out).expect("no preview was found");
    assert_eq!(
        found.full,
        Some((3456, 2304)),
        "the picture's own size was not found"
    );
}

#[test]
fn a_file_that_is_not_a_container_holds_nothing() {
    assert!(find(b"not a picture at all").is_none());
    assert!(find(&jpeg(32, 32)).is_none());
}

#[test]
fn a_directory_pointing_outside_the_file_is_not_followed() {
    let mut file = Builder::new();
    let directory = file.directory(
        &[
            long(TAG_JPEG_OFFSET, 900_000),
            long(TAG_JPEG_LENGTH, 40_000),
        ],
        0,
    );
    file.first(directory);
    assert!(
        find(&file.out).is_none(),
        "a preview came back from outside the file"
    );
}

#[test]
fn a_directory_that_points_at_itself_ends() {
    let mut file = Builder::new();
    let at = file.out.len() as u32;
    file.directory(&[long(TAG_SUB_DIRECTORIES, at)], at);
    file.first(at);
    assert!(find(&file.out).is_none());
}

/// A JPEG with a way-up written into it the way a camera writes one: a
/// segment near the front holding a TIFF of its own.
pub(crate) fn jpeg_the_way_up(way_up: u16) -> Vec<u8> {
    let mut tiff = Vec::new();
    tiff.extend_from_slice(b"II\x2a\x00");
    tiff.extend_from_slice(&8u32.to_le_bytes());
    tiff.extend_from_slice(&1u16.to_le_bytes());
    tiff.extend_from_slice(&TAG_ORIENTATION.to_le_bytes());
    tiff.extend_from_slice(&3u16.to_le_bytes());
    tiff.extend_from_slice(&1u32.to_le_bytes());
    tiff.extend_from_slice(&way_up.to_le_bytes());
    tiff.extend_from_slice(&[0, 0]);
    tiff.extend_from_slice(&0u32.to_le_bytes());

    let mut segment = Vec::new();
    segment.extend_from_slice(&[0xFF, 0xE1]);
    segment.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    segment.extend_from_slice(b"Exif\0\0");
    segment.extend_from_slice(&tiff);

    let mut out = jpeg(48, 32);
    out.splice(2..2, segment);
    out
}

#[test]
fn the_way_up_comes_out_of_a_jpegs_own_segment() {
    for way_up in 1..=8 {
        assert_eq!(the_way_up(&jpeg_the_way_up(way_up)), way_up);
    }
}

#[test]
fn the_way_up_comes_out_of_a_raw_files_first_directory() {
    let mut file = Builder::new();
    let directory = file.directory(&[short(TAG_ORIENTATION, 8)], 0);
    file.first(directory);
    assert_eq!(the_way_up(&file.out), 8);
}

#[test]
fn a_file_that_does_not_say_which_way_up_it_goes_is_upright() {
    assert_eq!(
        the_way_up(&jpeg(32, 32)),
        1,
        "a JPEG with no segment for it"
    );
    let mut file = Builder::new();
    let directory = file.directory(&[long(TAG_IMAGE_WIDTH, 100)], 0);
    file.first(directory);
    assert_eq!(the_way_up(&file.out), 1, "a raw file with no tag for it");
    assert_eq!(the_way_up(b"not a picture"), 1);
    assert_eq!(the_way_up(&[]), 1);
}

/// A value outside the eight the standard defines means nothing, and turning
/// a picture by nothing in particular is worse than leaving it.
#[test]
fn a_way_up_that_is_not_one_of_the_eight_is_ignored() {
    assert_eq!(the_way_up(&jpeg_the_way_up(0)), 1);
    assert_eq!(the_way_up(&jpeg_the_way_up(9)), 1);
    assert_eq!(the_way_up(&jpeg_the_way_up(60000)), 1);
}

#[test]
fn the_maker_comes_out_of_the_first_directory() {
    let mut file = Builder::new();
    let name = file.put(b"NIKON CORPORATION\x00");
    let directory = file.directory(
        &[Tag {
            tag: TAG_MAKE,
            kind: 2,
            count: 18,
            value: name,
        }],
        0,
    );
    file.first(directory);
    assert_eq!(maker(&file.out).as_deref(), Some("NIKON CORPORATION"));
}
