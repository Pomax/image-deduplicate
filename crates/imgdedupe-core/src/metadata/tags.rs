use super::*;

/// What a tag is worth showing as.
///
/// A number that stands for a word is shown as the word: "Metering: 5" tells
/// nobody anything and "Metering: Multi-segment" tells them what the camera was
/// doing. A number with a unit is shown with its unit. Anything whose meaning
/// cannot be given in either of those ways is not in the tables below at all,
/// because a photograph's panel is not the place for the layout of a sensor's
/// colour filters.
#[derive(Clone, Copy)]
pub(super) enum Shape {
    Text,
    Number,
    /// Shutter speed, which reads as a fraction of a second.
    Seconds,
    /// An aperture, which reads as f/2.8.
    FStop,
    Millimetres,
    Metres,
    /// Exposure compensation, in stops.
    Stops,
    /// A number that stands for a word.
    Words(&'static [(u32, &'static str)]),
    /// The one number whose meaning is spread over its bits.
    Flash,
    /// A comment, which carries eight bytes in front of it naming the alphabet
    /// it was written in. Those are for whoever reads the bytes, not for
    /// whoever reads the photograph.
    Comment,
    /// A date, which a file writes as 2026:06:13 04:18:34 and a person reads as
    /// June 13, 2026, 4:18:34 am.
    Date,
}

/// One tag worth showing: what it is called and how to say what it holds.
pub(super) struct Named {
    pub(super) number: u16,
    pub(super) name: &'static str,
    pub(super) shape: Shape,
}

const fn tag(number: u16, name: &'static str, shape: Shape) -> Named {
    Named {
        number,
        name,
        shape,
    }
}

/// One tag's value as something worth reading, or nothing when what it holds
/// cannot be said in words.
pub(super) fn value_of(
    entry: &preview::Entry,
    bytes: &[u8],
    order: Order,
    shape: Shape,
) -> Option<String> {
    let raw = entry.bytes(bytes, order)?;
    let value = match shape {
        Shape::Text => {
            let text = text_of(raw);
            if text.is_empty() {
                return None;
            }
            text
        }
        Shape::Number => {
            if entry.count != 1 {
                return None;
            }
            first_number(entry, raw, order)?.to_string()
        }
        Shape::Seconds => {
            let seconds = first_ratio(entry, raw, order)?;
            if seconds >= 1.0 {
                format!("{seconds:.1} s")
            } else if seconds > 0.0 {
                format!("1/{:.0} s", 1.0 / seconds)
            } else {
                return None;
            }
        }
        Shape::FStop => format!("f/{}", trimmed(first_ratio(entry, raw, order)?)),
        Shape::Millimetres => format!("{} mm", trimmed(first_ratio(entry, raw, order)?)),
        Shape::Metres => format!("{} m", trimmed(first_ratio(entry, raw, order)?)),
        Shape::Stops => {
            let stops = first_ratio(entry, raw, order)?;
            if stops == 0.0 {
                String::from("0")
            } else {
                format!(
                    "{}{} EV",
                    if stops > 0.0 { "+" } else { "" },
                    trimmed(stops)
                )
            }
        }
        Shape::Words(words) => {
            let value = first_number(entry, raw, order)?;
            words
                .iter()
                .find(|(number, _)| *number == value)
                .map(|(_, word)| word.to_string())?
        }
        Shape::Flash => flash(first_number(entry, raw, order)?),
        Shape::Date => said_plainly(&text_of(raw))?,
        Shape::Comment => {
            let said = text_of(raw.get(8..).unwrap_or_default());
            if said.is_empty() {
                return None;
            }
            said
        }
    };
    Some(value)
}

/// The first number of a tag, whichever width it was written in.
fn first_number(entry: &preview::Entry, raw: &[u8], order: Order) -> Option<u32> {
    match entry.kind {
        1 | 6 | 7 => raw.first().map(|byte| *byte as u32),
        3 | 8 => order.short(raw, 0).map(u32::from),
        4 | 9 => order.long(raw, 0),
        _ => None,
    }
}

/// The first rational of a tag, as a number. Exif writes anything that is not a
/// whole number as two numbers, one over the other.
pub(super) fn first_ratio(entry: &preview::Entry, raw: &[u8], order: Order) -> Option<f64> {
    if !matches!(entry.kind, 5 | 10) {
        return first_number(entry, raw, order).map(f64::from);
    }
    let top = order.long(raw, 0)?;
    let bottom = order.long(raw, 4)?;
    if bottom == 0 {
        return None;
    }
    if entry.kind == 10 {
        return Some(top as i32 as f64 / bottom as i32 as f64);
    }
    Some(top as f64 / bottom as f64)
}

/// A number with no trailing nothing after the point.
pub(super) fn trimmed(value: f64) -> String {
    format!("{value:.2}")
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_string()
}

/// A date as somebody would say it.
///
/// A file writes one as 2026:06:13 04:18:34, which is a machine's way of putting
/// it: the parts in the order a computer sorts by, and a clock nobody outside an
/// armed force reads. XMP writes 2026-06-13T04:18:34 instead. Both come out as
/// June 13, 2026, 4:18:34 am.
///
/// Nothing is shown at all when what is there is not a date, because a date
/// nobody can read is worse than no line.
pub fn said_plainly(text: &str) -> Option<String> {
    let numbers: Vec<u32> = text
        .split(|letter: char| !letter.is_ascii_digit())
        .filter(|piece| !piece.is_empty())
        .filter_map(|piece| piece.parse().ok())
        .collect();
    let (year, month, day) = (*numbers.first()?, *numbers.get(1)?, *numbers.get(2)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || year < 1800 {
        return None;
    }
    let months = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let said = format!("{} {day}, {year}", months[month as usize - 1]);

    let (Some(hour), Some(minute)) = (numbers.get(3), numbers.get(4)) else {
        return Some(said);
    };
    if *hour > 23 || *minute > 59 {
        return Some(said);
    }
    let second = numbers.get(5).copied().unwrap_or(0).min(59);
    let (clock, half) = match hour {
        0 => (12, "am"),
        1..=11 => (*hour, "am"),
        12 => (12, "pm"),
        _ => (hour - 12, "pm"),
    };
    Some(format!("{said}, {clock}:{minute:02}:{second:02} {half}"))
}

/// What the flash did, which one number says in its bits: whether it fired, and
/// whether the camera was trying to keep it from turning eyes red.
fn flash(value: u32) -> String {
    if value & 0x20 != 0 {
        return String::from("No flash on this camera");
    }
    let mut said = String::from(if value & 1 != 0 {
        "Fired"
    } else {
        "Did not fire"
    });
    if value & 0x40 != 0 {
        said.push_str(", red-eye reduction");
    }
    said
}

/// The picture, as the file describes it.
pub(super) const IMAGE_TAGS: &[Named] = &[
    tag(0x010F, "Make", Shape::Text),
    tag(0x0110, "Model", Shape::Text),
    tag(0x010E, "Description", Shape::Text),
    tag(0x0132, "Changed", Shape::Date),
    tag(0x013B, "Photographer", Shape::Text),
    tag(0x8298, "Copyright", Shape::Text),
    tag(0x9003, "Taken", Shape::Date),
    tag(0x9004, "Digitised", Shape::Date),
];

/// What the camera was set to.
pub(super) const SETTINGS_TAGS: &[Named] = &[
    tag(0x9003, "Taken", Shape::Date),
    tag(0x9004, "Digitised", Shape::Date),
    tag(0x829A, "Shutter speed", Shape::Seconds),
    tag(0x829D, "Aperture", Shape::FStop),
    tag(0x8827, "ISO", Shape::Number),
    tag(0x8832, "ISO", Shape::Number),
    tag(0x9204, "Exposure compensation", Shape::Stops),
    tag(0x8822, "Exposure program", Shape::Words(PROGRAMS)),
    tag(0xA402, "Exposure mode", Shape::Words(MODES)),
    tag(0x9207, "Metering", Shape::Words(METERING)),
    tag(0x9209, "Flash", Shape::Flash),
    tag(0x9208, "Light source", Shape::Words(LIGHT)),
    tag(0xA403, "White balance", Shape::Words(BALANCE)),
    tag(0x920A, "Focal length", Shape::Millimetres),
    tag(0xA405, "Focal length on 35mm", Shape::Millimetres),
    tag(0x9206, "Subject distance", Shape::Metres),
    tag(0xA406, "Scene", Shape::Words(SCENES)),
    tag(0xA408, "Contrast", Shape::Words(DEGREES)),
    tag(0xA409, "Saturation", Shape::Words(AMOUNTS)),
    tag(0xA40A, "Sharpness", Shape::Words(DEGREES)),
    tag(0xA001, "Colour space", Shape::Words(COLOUR_SPACES)),
    tag(0xA434, "Lens", Shape::Text),
    tag(0xA433, "Lens make", Shape::Text),
    tag(0xA435, "Lens serial", Shape::Text),
    tag(0xA431, "Body serial", Shape::Text),
    tag(0xA430, "Camera owner", Shape::Text),
    tag(0x9286, "Comment", Shape::Comment),
];

/// Where the picture was taken. The coordinates come from several tags at once,
/// so they are put together elsewhere rather than listed here.
pub(super) const PLACE_TAGS: &[Named] = &[
    tag(0x0008, "Satellites", Shape::Text),
    tag(0x0012, "Map datum", Shape::Text),
    tag(0x001B, "Found by", Shape::Text),
    tag(0x001D, "Date", Shape::Text),
];

const PROGRAMS: &[(u32, &str)] = &[
    (1, "Manual"),
    (2, "Program"),
    (3, "Aperture priority"),
    (4, "Shutter priority"),
    (5, "Creative"),
    (6, "Action"),
    (7, "Portrait"),
    (8, "Landscape"),
];

const MODES: &[(u32, &str)] = &[(0, "Automatic"), (1, "Manual"), (2, "Automatic bracket")];

const METERING: &[(u32, &str)] = &[
    (1, "Average"),
    (2, "Centre weighted"),
    (3, "Spot"),
    (4, "Multi-spot"),
    (5, "Multi-segment"),
    (6, "Partial"),
];

const LIGHT: &[(u32, &str)] = &[
    (1, "Daylight"),
    (2, "Fluorescent"),
    (3, "Tungsten"),
    (4, "Flash"),
    (9, "Fine weather"),
    (10, "Cloudy"),
    (11, "Shade"),
    (17, "Standard light A"),
    (18, "Standard light B"),
    (19, "Standard light C"),
    (24, "Studio tungsten"),
];

const BALANCE: &[(u32, &str)] = &[(0, "Automatic"), (1, "Manual")];

const SCENES: &[(u32, &str)] = &[
    (0, "Standard"),
    (1, "Landscape"),
    (2, "Portrait"),
    (3, "Night"),
];

const DEGREES: &[(u32, &str)] = &[(0, "Normal"), (1, "Soft"), (2, "Hard")];

const AMOUNTS: &[(u32, &str)] = &[(0, "Normal"), (1, "Low"), (2, "High")];

const COLOUR_SPACES: &[(u32, &str)] = &[(1, "sRGB"), (2, "Adobe RGB"), (65535, "Uncalibrated")];
