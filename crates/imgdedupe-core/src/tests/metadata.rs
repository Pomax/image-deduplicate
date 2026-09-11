use super::*;

fn value(groups: &[Group], name: &str) -> Option<String> {
    groups
        .iter()
        .flat_map(|group| group.entries.iter())
        .find(|(key, _)| key == name)
        .map(|(_, value)| value.clone())
}

/// A TIFF directory as a camera writes one: a make, a model, and a rational
/// for the exposure.
fn tiff() -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(b"II\x2a\x00");
    out.extend_from_slice(&8u32.to_le_bytes());
    let entries: [(u16, u16, u32, u32); 4] = [
        (0x010F, 2, 6, 0),
        (0x0110, 2, 6, 0),
        (0x0112, 3, 1, 6),
        (0x8769, 4, 1, 0),
    ];
    out.extend_from_slice(&(entries.len() as u16).to_le_bytes());
    let after = 8 + 2 + entries.len() * 12 + 4;
    let mut values = Vec::new();
    for (index, (tag, kind, count, inline)) in entries.iter().enumerate() {
        out.extend_from_slice(&tag.to_le_bytes());
        out.extend_from_slice(&kind.to_le_bytes());
        out.extend_from_slice(&count.to_le_bytes());
        match index {
            0 => {
                out.extend_from_slice(&((after + values.len()) as u32).to_le_bytes());
                values.extend_from_slice(b"NIKON\0");
            }
            1 => {
                out.extend_from_slice(&((after + values.len()) as u32).to_le_bytes());
                values.extend_from_slice(b"Z6\0\0");
            }
            3 => {
                // The camera directory, written after the values.
                out.extend_from_slice(&((after + values.len() + 10) as u32).to_le_bytes());
            }
            _ => out.extend_from_slice(&inline.to_le_bytes()),
        }
    }
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&values);
    out.extend_from_slice(&[0; 10]);

    // The camera directory: one exposure time, written as a rational after it.
    let camera = out.len();
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&0x829Au16.to_le_bytes());
    out.extend_from_slice(&5u16.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&((camera + 2 + 12 + 4) as u32).to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&1u32.to_le_bytes());
    out.extend_from_slice(&250u32.to_le_bytes());
    out
}

#[test]
fn a_cameras_own_directory_is_read() {
    let groups = read(&tiff(), Format::Nef);
    assert_eq!(value(&groups, "Make").as_deref(), Some("NIKON"));
    assert_eq!(value(&groups, "Model").as_deref(), Some("Z6"));
}

#[test]
fn the_camera_settings_hanging_off_it_are_read_too() {
    let groups = read(&tiff(), Format::Nef);
    assert!(
        groups.iter().any(|group| group.name == "Settings"),
        "the directory of settings was not followed"
    );
    assert_eq!(value(&groups, "Shutter speed").as_deref(), Some("1/250 s"));
}

#[test]
fn the_numbered_fields_of_a_wire_service_are_read() {
    let mut block = Vec::new();
    for (field, text) in [(120u8, "A deer in a garden"), (80, "Pomax"), (25, "deer")] {
        block.push(0x1C);
        block.push(2);
        block.push(field);
        block.extend_from_slice(&(text.len() as u16).to_be_bytes());
        block.extend_from_slice(text.as_bytes());
    }
    let groups = from_iptc(&block);
    assert_eq!(
        value(&groups, "Description").as_deref(),
        Some("A deer in a garden")
    );
    assert_eq!(value(&groups, "Creator").as_deref(), Some("Pomax"));
    assert_eq!(value(&groups, "Keywords").as_deref(), Some("deer"));
}

/// An editor writes hundreds of its own settings into a file: how much
/// clarity was applied, what the highlights were pulled to. None of that is
/// about the photograph, and none of it is shown.
#[test]
fn only_what_a_photographer_would_look_at_is_kept() {
    assert_eq!(xmp_name("dc:title"), Some("Title"));
    assert_eq!(xmp_name("dc:subject"), Some("Keywords"));
    assert_eq!(xmp_name("photoshop:City"), Some("City"));
    assert_eq!(
        xmp_name("rdf:Description"),
        None,
        "the element every property sits inside was read as a property"
    );
    assert_eq!(xmp_name("aux:Lens"), Some("Lens"));
    assert_eq!(xmp_name("crs:Clarity2012"), None);
    assert_eq!(xmp_name("crs:Highlights2012"), None);
    assert_eq!(xmp_name("crs:ToneCurveName2012"), None);
    assert_eq!(xmp_name("xmpMM:InstanceID"), None);
}

#[test]
fn adobes_xml_is_read_in_both_of_its_forms() {
    let xml = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/">
        <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description photoshop:City="Vancouver">
        <dc:title>A deer</dc:title>
        </rdf:Description></rdf:RDF></x:xmpmeta>"#;
    let groups = from_xmp(xml.as_bytes());
    assert_eq!(value(&groups, "City").as_deref(), Some("Vancouver"));
    assert_eq!(value(&groups, "Title").as_deref(), Some("A deer"));
}

/// A file writes a date the way a machine sorts them, on a clock nobody
/// outside an armed force reads. Somebody looking at their photographs
/// reads neither.
#[test]
fn a_date_is_said_the_way_somebody_would_say_it() {
    assert_eq!(
        said_plainly("2026:06:13 04:18:34").as_deref(),
        Some("June 13, 2026, 4:18:34 am")
    );
    assert_eq!(
        said_plainly("2026-06-13T16:05:00").as_deref(),
        Some("June 13, 2026, 4:05:00 pm")
    );
    assert_eq!(
        said_plainly("2026:01:02 00:30:00").as_deref(),
        Some("January 2, 2026, 12:30:00 am")
    );
    assert_eq!(
        said_plainly("2026:07:04 12:00:00").as_deref(),
        Some("July 4, 2026, 12:00:00 pm")
    );
    assert_eq!(said_plainly("2026:06:13").as_deref(), Some("June 13, 2026"));
    // Nothing that is not a date, rather than a wrong one.
    assert_eq!(said_plainly("not a date"), None);
    assert_eq!(said_plainly("0000:00:00 00:00:00"), None);
    assert_eq!(said_plainly(""), None);
}

#[test]
fn a_file_that_says_nothing_about_itself_has_nothing_to_show() {
    assert!(read(b"not a picture", Format::Jpeg).is_empty());
    assert!(read(&[], Format::Nef).is_empty());
    assert!(read(&[0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10], Format::Jpeg).is_empty());
}

/// The files this reads come off other people's cameras. A length that runs
/// past the end of the file is a file to give up on, not to read anyway.
#[test]
fn a_file_that_lies_about_its_own_lengths_is_survived() {
    let mut lying = tiff();
    // The make now claims to be four thousand bytes long.
    lying[10 + 4] = 0xA0;
    lying[10 + 5] = 0x0F;
    let groups = read(&lying, Format::Nef);
    assert!(groups.iter().all(|group| !group.entries.is_empty()));

    let mut truncated = tiff();
    truncated.truncate(20);
    let _ = read(&truncated, Format::Nef);

    let mut iptc = vec![0x1C, 2, 120, 0xFF, 0xFF];
    iptc.extend_from_slice(b"short");
    let _ = from_iptc(&iptc);
}
