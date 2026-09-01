use eframe::egui;

/// System interface faces, best first. egui ships Ubuntu-Light, which is thin and
/// is not what anything else on the machine is set in.
const PREFERRED: [&str; 9] = [
    "Segoe UI",
    "SF Pro Text",
    "Helvetica Neue",
    "Inter",
    "Noto Sans",
    "DejaVu Sans",
    "Liberation Sans",
    "Cantarell",
    "Arial",
];

const MONOSPACE: [&str; 6] = [
    "Cascadia Mono",
    "Consolas",
    "SF Mono",
    "Menlo",
    "DejaVu Sans Mono",
    "Liberation Mono",
];

/// Use the platform's interface font and set text at sizes a person can read.
pub fn install(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mut database = fontdb::Database::new();
    database.load_system_fonts();

    if let Some(data) = first_available(&database, &PREFERRED) {
        fonts
            .font_data
            .insert(String::from("ui"), egui::FontData::from_owned(data));
        fonts
            .families
            .entry(egui::FontFamily::Proportional)
            .or_default()
            .insert(0, String::from("ui"));
    }

    if let Some(data) = first_available(&database, &MONOSPACE) {
        fonts
            .font_data
            .insert(String::from("ui-mono"), egui::FontData::from_owned(data));
        fonts
            .families
            .entry(egui::FontFamily::Monospace)
            .or_default()
            .insert(0, String::from("ui-mono"));
    }

    ctx.set_fonts(fonts);
    ctx.style_mut(set_sizes);
}

/// The one text size. There is no second size: headings, small print and button
/// labels are all this.
pub const SIZE: f32 = 16.0;

fn set_sizes(style: &mut egui::Style) {
    use egui::{FontFamily, FontId, TextStyle};
    let proportional = FontId::new(SIZE, FontFamily::Proportional);
    style.text_styles = [
        (TextStyle::Heading, proportional.clone()),
        (TextStyle::Body, proportional.clone()),
        (TextStyle::Button, proportional.clone()),
        (TextStyle::Small, proportional),
        (TextStyle::Monospace, FontId::new(SIZE, FontFamily::Monospace)),
    ]
    .into();

    style.spacing.button_padding = egui::vec2(12.0, 7.0);
    style.spacing.item_spacing = egui::vec2(9.0, 7.0);
    style.spacing.interact_size.y = 26.0;
}

/// The first of `names` the system actually has, as font bytes.
///
/// A query for normal weight returns the closest face a family has, so a family
/// that only ships Light comes back as Light. Anything lighter than normal is
/// rejected rather than accepted as close enough, which is the whole reason for
/// replacing egui's bundled Ubuntu-Light.
fn first_available(database: &fontdb::Database, names: &[&str]) -> Option<Vec<u8>> {
    for name in names {
        let query = fontdb::Query {
            families: &[fontdb::Family::Name(name)],
            weight: fontdb::Weight::NORMAL,
            style: fontdb::Style::Normal,
            stretch: fontdb::Stretch::Normal,
        };
        let Some(id) = database.query(&query) else {
            continue;
        };
        if !is_normal_weight(database, id) {
            continue;
        }
        let mut bytes = None;
        database.with_face_data(id, |data, _index| bytes = Some(data.to_vec()));
        if bytes.is_some() {
            return bytes;
        }
    }
    None
}

/// Normal, not Light and not Bold. Bold is excluded too: egui emboldens where it
/// wants emphasis, and a bold base face leaves nothing above it.
fn is_normal_weight(database: &fontdb::Database, id: fontdb::ID) -> bool {
    database
        .face(id)
        .map(|face| {
            let weight = face.weight.0;
            (400..600).contains(&weight) && face.style == fontdb::Style::Normal
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_text_style_is_the_same_size() {
        let mut style = egui::Style::default();
        set_sizes(&mut style);
        for name in [
            egui::TextStyle::Heading,
            egui::TextStyle::Body,
            egui::TextStyle::Button,
            egui::TextStyle::Small,
            egui::TextStyle::Monospace,
        ] {
            assert_eq!(
                style.text_styles[&name].size, SIZE,
                "{name:?} is not {SIZE}"
            );
        }
    }

    #[test]
    fn every_proportional_style_is_the_same_face() {
        let mut style = egui::Style::default();
        set_sizes(&mut style);
        for name in [
            egui::TextStyle::Heading,
            egui::TextStyle::Body,
            egui::TextStyle::Button,
            egui::TextStyle::Small,
        ] {
            assert_eq!(
                style.text_styles[&name].family,
                egui::FontFamily::Proportional,
                "{name:?} is not the interface face"
            );
        }
    }

    #[test]
    fn the_size_is_not_scaled_by_anything() {
        // The number here is the number on screen. There was a zoom factor
        // multiplying it, which made 14 mean 21 and made the constant a lie.
        let ctx = egui::Context::default();
        install(&ctx);
        assert_eq!(ctx.zoom_factor(), 1.0, "something is scaling the interface");
        assert_eq!(
            ctx.style().text_styles[&egui::TextStyle::Body].size,
            SIZE,
            "body text is not the size this module sets"
        );
    }

    #[test]
    fn a_system_font_is_found_on_this_machine() {
        // If no face in the list is present, the window falls back to the bundled
        // font rather than failing, but on any normal desktop one of these exists.
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let found = first_available(&database, &PREFERRED);
        assert!(found.is_some(), "none of {PREFERRED:?} were installed");
        assert!(found.unwrap().len() > 1000, "the face data looks empty");
    }

    #[test]
    fn asking_for_a_font_that_does_not_exist_gives_nothing() {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        assert!(first_available(&database, &["No Such Face At All 12345"]).is_none());
    }

    #[test]
    fn a_light_face_is_not_accepted_as_the_interface_font() {
        // A family that only ships Light comes back from a normal-weight query as
        // Light, which is exactly the face this module exists to avoid.
        let mut database = fontdb::Database::new();
        database.load_system_fonts();

        for name in ["Segoe UI Light", "Ubuntu Light", "Helvetica Neue Light"] {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                weight: fontdb::Weight::NORMAL,
                style: fontdb::Style::Normal,
                stretch: fontdb::Stretch::Normal,
            };
            if let Some(id) = database.query(&query) {
                if database.face(id).is_some_and(|face| face.weight.0 < 400) {
                    assert!(
                        first_available(&database, &[name]).is_none(),
                        "{name} was accepted despite being lighter than normal"
                    );
                }
            }
        }
    }

    #[test]
    fn the_chosen_face_is_normal_weight() {
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        for name in PREFERRED {
            let query = fontdb::Query {
                families: &[fontdb::Family::Name(name)],
                weight: fontdb::Weight::NORMAL,
                style: fontdb::Style::Normal,
                stretch: fontdb::Stretch::Normal,
            };
            if let Some(id) = database.query(&query) {
                if first_available(&database, &[name]).is_some() {
                    let face = database.face(id).expect("the face that was just chosen");
                    assert!(
                        (400..600).contains(&face.weight.0),
                        "{name} was chosen at weight {}",
                        face.weight.0
                    );
                    return;
                }
            }
        }
        panic!("no face was chosen at all");
    }
}
