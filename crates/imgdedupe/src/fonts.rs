use std::sync::{Mutex, OnceLock};

use eframe::egui;
use imgdedupe_core::runlog;

/// The interface face, carried in the binary: Open Sans, cut down to the letters
/// the window itself writes and the Latin alphabets a path is usually made of.
/// The licence and the permission to embed are inside the file.
///
/// Nothing is read from the machine to draw this. A file name in a script it
/// does not cover is drawn by `fallback`, which arrives a moment later.
const FACE: &[u8] = include_bytes!("../assets/OpenSans-Regular-subset.ttf");

/// Draw with the bundled face, at a size a person can read.
pub fn install(ctx: &egui::Context) {
    ctx.set_fonts(definitions(Vec::new()));
    ctx.style_mut(set_sizes);
}

/// Make sure `text` can be drawn, and go and find a face for it if it cannot.
///
/// Called with the names the window is about to show, not every frame: it is the
/// answer to a folder being opened or a search coming back, and the machine is
/// only asked when something in those names has no glyph.
pub fn cover(ctx: &egui::Context, text: &str) {
    let missing = missing_from(text, &held_faces());
    if missing.is_empty() {
        return;
    }
    find_a_face(ctx, missing);
}

/// The letters of `text` that neither the bundled face nor anything found so far
/// can draw. Read from the faces themselves rather than from the window, so this
/// answers before there is a window and cannot depend on one.
fn missing_from(text: &str, found: &[Vec<u8>]) -> Vec<char> {
    let bundled = ttf_parser::Face::parse(FACE, 0).ok();
    let others: Vec<ttf_parser::Face<'_>> = found
        .iter()
        .filter_map(|bytes| ttf_parser::Face::parse(bytes, 0).ok())
        .collect();
    let drawn = |letter: char| {
        bundled.as_ref().is_some_and(|face| face.glyph_index(letter).is_some())
            || others.iter().any(|face| face.glyph_index(letter).is_some())
    };

    let mut missing: Vec<char> = text
        .chars()
        .filter(|letter| !letter.is_whitespace() && !drawn(*letter))
        .collect();
    missing.sort_unstable();
    missing.dedup();
    missing
}

/// Copies of the faces found so far, for reading without holding the lock.
fn held_faces() -> Vec<Vec<u8>> {
    FACES
        .get()
        .and_then(|faces| faces.lock().ok())
        .map(|held| held.clone())
        .unwrap_or_default()
}

/// The bundled face first, then whatever was found for the scripts it lacks. A
/// character missing from one face is looked for in the next.
fn definitions(extra: Vec<Vec<u8>>) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::empty();
    let mut order = vec![String::from("ui")];
    fonts
        .font_data
        .insert(String::from("ui"), egui::FontData::from_static(FACE));
    for (index, face) in extra.into_iter().enumerate() {
        let name = format!("system{index}");
        fonts.font_data.insert(name.clone(), egui::FontData::from_owned(face));
        order.push(name);
    }
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts.families.insert(family, order.clone());
    }
    fonts
}

/// Ask the machine for a sans serif that has these letters, on a thread of its
/// own, and hand it to the window when it is found. Reading the system fonts
/// takes a second or more and the window keeps painting throughout.
///
/// Whatever comes back is what the letters are drawn in. It will not match Open
/// Sans, and that is not worth caring about: this window shows file names, and a
/// name that draws is better than a name that does not.
fn find_a_face(ctx: &egui::Context, missing: Vec<char>) {
    let ctx = ctx.clone();
    let faces = FACES.get_or_init(|| Mutex::new(Vec::new()));
    std::thread::spawn(move || {
        let at = std::time::Instant::now();
        let mut database = fontdb::Database::new();
        database.load_system_fonts();
        let Some(bytes) = sans_serif_with(&database, &missing) else {
            runlog::line(&format!(
                "no system face covers {missing:?}, {:.2}s",
                at.elapsed().as_secs_f64()
            ));
            return;
        };
        let Ok(mut held) = faces.lock() else {
            return;
        };
        held.push(bytes);
        runlog::line(&format!(
            "fallback face for {missing:?}: {:.2}s, {} in use",
            at.elapsed().as_secs_f64(),
            held.len()
        ));
        ctx.set_fonts(definitions(held.clone()));
        ctx.request_repaint();
    });
}

/// The faces found so far, kept because handing egui a new set replaces the old
/// one: the second script to turn up must not lose the first.
static FACES: OnceLock<Mutex<Vec<Vec<u8>>>> = OnceLock::new();

/// Whether a face's name says it is a sans serif. The interface faces of the
/// three platforms, and the usual names for one in the scripts that have their
/// own word for it.
fn looks_sans_serif(face: &fontdb::FaceInfo) -> bool {
    const SANS: [&str; 12] = [
        "sans", "gothic", "hei", "grotesk", "arial", "helvetica", "segoe ui", "verdana", "tahoma",
        "roboto", "inter", "ui",
    ];
    const SERIF: [&str; 8] = [
        "serif", "mincho", "song", "times", "georgia", "garamond", "script", "cursive",
    ];
    let name = face
        .families
        .first()
        .map(|(name, _)| name.to_lowercase())
        .unwrap_or_default();
    if SERIF.iter().any(|word| name.contains(word)) {
        return false;
    }
    SANS.iter().any(|word| name.contains(word))
}

/// The first sans serif on the machine that has every one of these letters.
///
/// `fontdb` knows the family and the style of every face but not which letters
/// it holds, so the ones that claim to be sans serif are opened in turn and
/// their character maps are read.
fn sans_serif_with(database: &fontdb::Database, wanted: &[char]) -> Option<Vec<u8>> {
    let mut candidates: Vec<&fontdb::FaceInfo> = database
        .faces()
        .filter(|face| {
            face.style == fontdb::Style::Normal
                && face.weight == fontdb::Weight::NORMAL
                && face.stretch == fontdb::Stretch::Normal
        })
        .collect();
    // Sans serif first. Neither `fontdb` nor `ttf-parser` says what kind of face
    // it is, so this goes on the name, which is what the names are for.
    candidates.sort_by_key(|face| !looks_sans_serif(face));

    for face in candidates {
        let mut covers = false;
        database.with_face_data(face.id, |data, index| {
            covers = ttf_parser::Face::parse(data, index)
                .map(|parsed| wanted.iter().all(|letter| parsed.glyph_index(*letter).is_some()))
                .unwrap_or(false);
        });
        if !covers {
            continue;
        }
        let mut bytes = None;
        database.with_face_data(face.id, |data, _index| bytes = Some(data.to_vec()));
        if bytes.is_some() {
            return bytes;
        }
    }
    None
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled face is cut down to the alphabets a file name is usually made
    /// of. Everything the window itself writes has to be in it, or the interface
    /// depends on what the machine happens to have.
    #[test]
    fn the_bundled_face_has_the_letters_the_window_writes() {
        let ctx = egui::Context::default();
        install(&ctx);
        let _ = ctx.run(Default::default(), |_| {});
        let wanted = egui::FontId::new(SIZE, egui::FontFamily::Proportional);
        let written = "Choose folder previous Include subfolders Save an index database for this \
             folder What counts as a duplicate presets close balanced wide yolo Match colour with \
             grayscale Scan Cancel Find duplicates read indexed found unchanged removed failed to \
             read per second sets duplicates to remove MB to reclaim keep all keep none KEEP \
             Clean up Recycle bin Move Remove dismiss none chosen clear previous locations \
             No duplicates found for current settings 0123456789 %.,:-_()[]/\\'\"";
        ctx.fonts(|fonts| {
            for letter in written.chars().filter(|letter| !letter.is_whitespace()) {
                assert!(
                    fonts.has_glyph(&wanted, letter),
                    "the bundled face has no {letter:?}"
                );
            }
        });
    }

    /// A name in a script the bundled face does not have is what sends anyone
    /// looking. One it can draw asks the machine for nothing.
    #[test]
    fn only_letters_the_bundled_face_lacks_send_anyone_looking() {
        assert!(missing_from("holiday photo (1).jpeg", &[]).is_empty());
        assert!(missing_from("café niño.png", &[]).is_empty(), "accented Latin is missing");

        let mut wanted = vec!['写', '真'];
        wanted.sort_unstable();
        assert_eq!(missing_from("写真.jpg", &[]), wanted);
    }

    /// The name is what says whether a face is a sans serif, and a serif face
    /// whose name also contains a sans serif word is still a serif.
    #[test]
    fn a_face_is_taken_for_a_sans_serif_by_its_name() {
        let named = |name: &str| fontdb::FaceInfo {
            id: fontdb::ID::dummy(),
            source: fontdb::Source::Binary(std::sync::Arc::new(Vec::<u8>::new())),
            index: 0,
            families: vec![(String::from(name), fontdb::Language::English_UnitedStates)],
            post_script_name: String::from(name),
            style: fontdb::Style::Normal,
            weight: fontdb::Weight::NORMAL,
            stretch: fontdb::Stretch::Normal,
            monospaced: false,
        };
        for name in ["Segoe UI", "Noto Sans CJK JP", "Microsoft YaHei", "Yu Gothic"] {
            assert!(looks_sans_serif(&named(name)), "{name} was not taken as a sans serif");
        }
        for name in ["Times New Roman", "Noto Serif", "MS Mincho", "Comic Script"] {
            assert!(!looks_sans_serif(&named(name)), "{name} was taken as a sans serif");
        }
    }

    /// A face is only taken when it has every letter that was missing.
    #[test]
    fn a_face_without_the_letters_is_not_taken() {
        let mut database = fontdb::Database::new();
        database.load_font_data(FACE.to_vec());
        assert!(
            sans_serif_with(&database, &['a', 'b']).is_some(),
            "the bundled face has no a or b"
        );
        assert!(
            sans_serif_with(&database, &['中']).is_none(),
            "a face was taken for a letter it does not have"
        );
    }

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

    /// The face in the binary is a real one, and it is the one that gets used.
    #[test]
    fn the_bundled_face_is_the_only_one_the_window_has() {
        assert!(FACE.len() > 10_000, "the bundled face is not a font file");
        assert_eq!(&FACE[..4], b"\x00\x01\x00\x00", "the bundled face is not TrueType");

        let ctx = egui::Context::default();
        install(&ctx);
        // Fonts are built on the first pass, not when they are set.
        let _ = ctx.run(Default::default(), |_| {});
        ctx.fonts(|fonts| {
            let width = |text: &str| {
                fonts
                    .layout_no_wrap(
                        String::from(text),
                        egui::FontId::new(SIZE, egui::FontFamily::Proportional),
                        egui::Color32::BLACK,
                    )
                    .rect
                    .width()
            };
            assert!(width("imgdedupe") > 0.0, "nothing was laid out at all");
            assert!(
                width("mmmm") > width("iiii"),
                "the text came out of a fallback that has no real glyphs"
            );
        });
    }

    /// Both families are the one face. A second face in the binary is a second
    /// face nobody asked for.
    #[test]
    fn there_is_one_face_and_both_families_use_it() {
        let mut fonts = egui::FontDefinitions::empty();
        fonts
            .font_data
            .insert(String::from("ui"), egui::FontData::from_static(FACE));
        assert_eq!(fonts.font_data.len(), 1);

        let ctx = egui::Context::default();
        install(&ctx);
        // Fonts are built on the first pass, not when they are set.
        let _ = ctx.run(Default::default(), |_| {});
        ctx.fonts(|fonts| {
            let same = |family: egui::FontFamily| {
                fonts
                    .layout_no_wrap(
                        String::from("gjpq"),
                        egui::FontId::new(SIZE, family),
                        egui::Color32::BLACK,
                    )
                    .rect
                    .width()
            };
            assert_eq!(
                same(egui::FontFamily::Proportional),
                same(egui::FontFamily::Monospace),
                "the two families are not the same face"
            );
        });
    }
}
