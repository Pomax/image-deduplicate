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
    assert!(
        missing_from("café niño.png", &[]).is_empty(),
        "accented Latin is missing"
    );

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
    for name in [
        "Segoe UI",
        "Noto Sans CJK JP",
        "Microsoft YaHei",
        "Yu Gothic",
    ] {
        assert!(
            looks_sans_serif(&named(name)),
            "{name} was not taken as a sans serif"
        );
    }
    for name in ["Times New Roman", "Noto Serif", "MS Mincho", "Comic Script"] {
        assert!(
            !looks_sans_serif(&named(name)),
            "{name} was taken as a sans serif"
        );
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
    assert_eq!(
        &FACE[..4],
        b"\x00\x01\x00\x00",
        "the bundled face is not TrueType"
    );

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
