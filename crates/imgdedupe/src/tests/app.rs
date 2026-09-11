use super::*;
use imgdedupe_core::matching::Member;

/// A context set up the way the window sets one up: the face it carries and
/// the style it installs. A bare context has no font at all, so text measures
/// as nothing and every layout a test looks at is a different layout from the
/// one on screen.
fn window() -> egui::Context {
    let ctx = egui::Context::default();
    crate::fonts::install(&ctx);
    install_style(&ctx);
    ctx
}

/// The ring drawn around the picture being looked at, which is a different
/// thing from the border that says a picture is being kept.
fn selection_colour() -> egui::Color32 {
    window().style().visuals.selection.bg_fill
}

fn member(id: i64, path: &str, size: i64) -> Member {
    Member {
        file_id: id,
        rel_path: path.to_string(),
        width: 100,
        height: 100,
        format: "jpeg".to_string(),
        channels: 3,
        size_bytes: size,
        mtime_seconds: 1_700_000_000,
        auto_keep: false,
    }
}

/// The date on a tile is the file's own timestamp, turned into a date without
/// a calendar library, so the arithmetic is what gets checked.
#[test]
fn a_file_stamp_becomes_the_date_and_time_it_stands_for() {
    assert_eq!(file_date(0), "1970-01-01 00:00");
    assert_eq!(file_date(1), "1970-01-01 00:00");
    assert_eq!(file_date(86_399), "1970-01-01 23:59");
    assert_eq!(file_date(86_400), "1970-01-02 00:00");

    // 2024-02-29, a leap day in a year that is a multiple of four.
    assert_eq!(file_date(1_709_164_800), "2024-02-29 00:00");
    // 2000-02-29: a multiple of a hundred that is still a leap year.
    assert_eq!(file_date(951_782_400), "2000-02-29 00:00");
    // 1900 was not one, being a multiple of a hundred but not four hundred,
    // so the day after the 28th of February is the first of March.
    assert_eq!(file_date(-2_203_977_600), "1900-02-28 00:00");
    assert_eq!(file_date(-2_203_891_200), "1900-03-01 00:00");

    assert_eq!(file_date(1_700_000_000), "2023-11-14 22:13");
    assert_eq!(file_date(2_000_000_000), "2033-05-18 03:33");
}

/// The space bar keeps whatever the preview is showing, in whichever set it
/// belongs to.
#[test]
fn the_space_bar_keeps_the_picture_the_preview_is_showing() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 2, "the two pairs were not found");

    let (first, second) = (app.sets[0].set_id, app.sets[1].set_id);
    let other_in_first = app.sets[0].members[1].file_id;
    let one_in_second = app.sets[1].members[1].file_id;

    app.selected = Some(other_in_first);
    app.keep_selected();
    assert_eq!(
        app.keep.get(&first),
        Some(&Keep::One(other_in_first)),
        "the picture the preview was on was not marked"
    );

    app.selected = Some(one_in_second);
    app.keep_selected();
    assert_eq!(
        app.keep.get(&second),
        Some(&Keep::One(one_in_second)),
        "the other set was not given the keeper it was asked for"
    );
    assert_eq!(
        app.keep.get(&first),
        Some(&Keep::One(other_in_first)),
        "it changed a set it was not on"
    );

    app.selected = None;
    app.keep_selected();
    assert_eq!(
        app.keep.len(),
        2,
        "nothing was selected and something changed"
    );

    // Again on the one already kept takes the mark off, and again puts it
    // back, so the key is a toggle rather than a one way door.
    app.selected = Some(other_in_first);
    app.keep_selected();
    assert_eq!(app.keep.get(&first), None, "the mark did not come off");
    assert_eq!(
        app.keep.get(&second),
        Some(&Keep::One(one_in_second)),
        "it changed a set it was not on"
    );
    app.keep_selected();
    assert_eq!(
        app.keep.get(&first),
        Some(&Keep::One(other_in_first)),
        "the mark did not go back on"
    );
}

/// The escape key on the scan page is the Cancel button: it stops whatever
/// that button would stop, and where the button is greyed out it does
/// nothing, so the key never means something the page does not show.
#[test]
fn escape_on_the_scan_page_is_the_cancel_button() {
    let scanned = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(scanned.path().to_path_buf());

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1100.0, 700.0));
    let mut clock = 0.0;
    let mut escape = |app: &mut App| {
        clock += 0.1;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(clock),
                events: vec![egui::Event::Key {
                    key: egui::Key::Escape,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: Default::default(),
                }],
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.scan_view(ui));
            },
        );
    };

    // Nothing is running, so the button is greyed out and the key does
    // nothing: not a folder forgotten, not a page changed, nothing.
    let folder = app.folder.clone();
    assert!(
        !app.can_cancel(),
        "there was something to cancel before anything started"
    );
    escape(&mut app);
    assert_eq!(
        app.folder, folder,
        "escape did something on a page with nothing to stop"
    );
    assert!(app.error.is_none());

    // A pass running is what the button is for, and the key stops it.
    app.start_scan();
    assert!(
        app.can_cancel(),
        "a pass that is running cannot be cancelled"
    );
    escape(&mut app);
    assert!(app.running.is_none(), "escape did not stop the pass");
    assert!(!app.can_cancel(), "there is still something to cancel");
    // And the page is back to before the run, which is what the button does:
    // a cancelled pass leaves counts and lamps that are true of nothing.
    assert_eq!(
        app.scan.total, 0,
        "the cancelled run left its numbers on the page"
    );
    assert!(app.lit.is_empty(), "the cancelled run left its lamps lit");
    assert!(
        app.images.is_none(),
        "the cancelled run left half an index in memory"
    );
}

/// Starting a second pass while one is going means nothing, so everything
/// that would start one is off while any of the three is running.
#[test]
fn nothing_that_starts_work_is_offered_while_work_is_going() {
    let scanned = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    assert!(!app.busy(), "a window that has done nothing is busy");

    app.open_folder(scanned.path().to_path_buf());
    app.start_scan();
    assert!(app.busy(), "a pass over the folder does not count as busy");
    settle(&mut app);
    assert!(
        !app.busy(),
        "the window is still busy with a pass that finished"
    );

    app.load_sets();
    assert!(
        app.busy(),
        "the search for duplicates does not count as busy"
    );
    settle(&mut app);

    app.destination = Destination::Delete;
    let plan = app.build_plan();
    app.run_cleanup(&plan);
    assert!(app.busy(), "removing files does not count as busy");
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.removing.is_some() && std::time::Instant::now() < until {
        app.pump_cleanup(&ctx);
    }
    assert!(
        !app.busy(),
        "the window is still busy with a cleanup that finished"
    );
}

/// A cleanup where nothing could be removed leaves everything as it was, on
/// the page where another destination can be chosen, and says which files.
#[test]
fn a_cleanup_that_removed_nothing_stays_put_and_names_the_files() {
    let scanned = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(scanned.path().to_path_buf());
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);
    // Marked, so there is something for the cleanup to fail at.
    app.auto_mark_to_keep();
    let keeping = app.keep.clone();

    // The file the plan is about is taken away before the cleanup runs, so
    // the removal fails the way a locked or missing file does.
    app.destination = Destination::Delete;
    let plan = app.build_plan();
    let going = plan.removals[0].rel_path.clone();
    std::fs::remove_file(scanned.path().join(&going)).expect("take the file away");

    app.view = View::Cleanup;
    app.run_cleanup(&plan);
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.removing.is_some() && std::time::Instant::now() < until {
        app.pump_cleanup(&ctx);
    }

    assert_eq!(
        app.view,
        View::Cleanup,
        "it left the page with the destinations on it"
    );
    assert_eq!(
        app.sets.len(),
        1,
        "the sets went even though the files did not"
    );
    assert_eq!(app.keep, keeping, "the keeper was thrown away");
    assert_eq!(app.cleanup_failures.len(), 1);
    assert_eq!(app.cleanup_failures[0].0, going);
}

/// A cleanup that removed everything is over: the sets are gone and so is the
/// page for them.
#[test]
fn a_cleanup_that_removed_everything_leaves_the_page() {
    let scanned = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(scanned.path().to_path_buf());
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);
    // Marked, so there is a cleanup to carry out.
    app.auto_mark_to_keep();
    assert_eq!(app.sets.len(), 1, "the two copies were not found");

    // Delete outright, so the test does not depend on a recycle bin.
    app.destination = Destination::Delete;
    let plan = app.build_plan();
    assert_eq!(
        plan.files(),
        1,
        "the plan is not the copy the set does not keep"
    );
    let going = plan.removals[0].rel_path.clone();
    app.view = View::Cleanup;
    app.run_cleanup(&plan);
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.removing.is_some() && std::time::Instant::now() < until {
        app.pump_cleanup(&ctx);
    }

    assert!(
        !scanned.path().join(&going).exists(),
        "{going} is still on disk"
    );
    assert_eq!(app.view, View::Scan);
    assert!(app.sets.is_empty());
    assert!(app.cleanup_failures.is_empty());
}

/// Some went and some did not. What went comes out of the sets, what did not
/// stays on screen to be tried another way.
#[test]
fn a_cleanup_that_half_worked_keeps_what_is_still_there() {
    let dir = tempfile::tempdir().expect("tempdir");
    let picture = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
        image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
    }));
    for name in ["one.png", "two.png", "three.png"] {
        picture
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
    }

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(dir.path().to_path_buf());
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);
    // Marked, so two of the three are going.
    app.auto_mark_to_keep();
    assert_eq!(
        app.sets[0].members.len(),
        3,
        "the three copies were not one set"
    );

    // One of the two the plan would remove is taken away first, so that one
    // fails and the other goes.
    app.destination = Destination::Delete;
    let plan = app.build_plan();
    assert_eq!(plan.files(), 2);
    let fails = plan.removals[0].rel_path.clone();
    let goes = plan.removals[1].rel_path.clone();
    std::fs::remove_file(dir.path().join(&fails)).expect("take the file away");

    app.view = View::Cleanup;
    app.run_cleanup(&plan);
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.removing.is_some() && std::time::Instant::now() < until {
        app.pump_cleanup(&ctx);
    }

    assert_eq!(app.view, View::Cleanup);
    assert_eq!(
        app.sets.len(),
        1,
        "the set lost more than the file that went"
    );
    let left: Vec<&str> = app.sets[0]
        .members
        .iter()
        .map(|member| member.rel_path.as_str())
        .collect();
    assert!(
        !left.contains(&goes.as_str()),
        "the file that went is still listed"
    );
    assert!(
        left.contains(&fails.as_str()),
        "the file that would not go was dropped"
    );
    assert_eq!(app.cleanup_failures.len(), 1);
}

/// How the last pass ended is about that pass. Starting another clears it, or
/// a new search sits under the word "cancelled" from the one before it.
#[test]
fn starting_a_pass_clears_what_the_last_one_ended_with() {
    let scanned = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(scanned.path().to_path_buf());

    // A finished pass and search leave their outcome on screen.
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);
    app.scan.finished = Some(String::from("cancelled"));

    app.load_sets();
    assert_eq!(
        app.scan.finished, None,
        "the search kept the last outcome on screen"
    );
    assert_eq!(app.error, None, "the search kept the last error on screen");
    settle(&mut app);

    app.start_scan();
    assert_eq!(
        app.scan.finished, None,
        "the scan kept the last outcome on screen"
    );
    settle(&mut app);
}

/// A search that finds nothing leaves the window where it is. There is
/// nothing to review, and the Review tab stays shut.
#[test]
fn finding_no_duplicates_does_not_open_the_review() {
    let dir = tempfile::tempdir().expect("tempdir");
    let picture = |seed: u32| {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([
                ((x * 7 + seed) % 256) as u8,
                ((y * 11 + seed) % 256) as u8,
                20,
            ])
        }))
    };
    for (name, seed) in [("a.png", 0), ("b.png", 128)] {
        picture(seed)
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
    }

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(dir.path().to_path_buf());
    app.sensitivity = 0.5;
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);

    assert_eq!(
        app.view,
        View::Scan,
        "it went to the review with nothing in it"
    );
    assert!(!app.have_sets(), "the tabs would be open on an empty list");
    assert_eq!(app.selected, None);
    assert_eq!(
        app.scan.finished.as_deref(),
        Some("No duplicates found for current settings")
    );
}

/// The preview pane opens on a picture of the first set rather than on an
/// empty pane: what it marks if it marks anything, and its first picture
/// otherwise, which is what a review that has just been found looks like.
#[test]
fn the_first_sets_keeper_is_what_the_preview_starts_on() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 2, "the two pairs were not found");

    // Nothing is marked when a review arrives, so it opens on the first
    // picture of the first set.
    let first = app.sets[0].members[0].file_id;
    assert_eq!(
        app.selected,
        Some(first),
        "the preview did not open on a picture"
    );

    let marked = app.sets[0].members[1].file_id;
    app.keep.insert(app.sets[0].set_id, Keep::One(marked));
    app.preselect_first_keeper();
    assert_eq!(
        app.selected,
        Some(marked),
        "the preview passed over the marked picture"
    );

    app.keep.remove(&app.sets[0].set_id);
    app.preselect_first_keeper();
    assert_eq!(
        app.selected,
        Some(first),
        "a first set with no mark showed nothing"
    );

    app.sets.clear();
    app.preselect_first_keeper();
    assert_eq!(app.selected, None);
}

/// The review opens on the first set that is a set of copies. A set nobody
/// calls a set of copies is not somewhere to start: it keeps nothing, shows
/// nothing as kept, and the cursor keys would be starting from a set they are
/// only going to step out of.
#[test]
fn the_preview_does_not_open_inside_an_ignored_set() {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.sets = vec![
        DuplicateSet {
            set_id: 1,
            members: vec![member(1, "a.jpg", 10), member(2, "b.jpg", 10)],
        },
        DuplicateSet {
            set_id: 2,
            members: vec![member(3, "c.jpg", 10), member(4, "d.jpg", 10)],
        },
    ];
    // The first set is not a set of copies.
    app.ignored.insert(db::pair(1, 2));

    app.preselect_first_keeper();
    assert_eq!(
        app.selected,
        Some(3),
        "the review opened inside the ignored set"
    );

    // What the second set keeps, when it keeps something.
    app.keep.insert(2, Keep::One(4));
    app.preselect_first_keeper();
    assert_eq!(
        app.selected,
        Some(4),
        "the review did not open on what the set keeps"
    );

    // Nothing but ignored sets is nowhere to open.
    app.ignored.insert(db::pair(3, 4));
    app.preselect_first_keeper();
    assert_eq!(
        app.selected, None,
        "the review opened inside an ignored set anyway"
    );
}

#[test]
fn right_and_left_run_through_the_whole_list_and_stop_at_its_ends() {
    let counts = [3, 1, 2];

    assert_eq!(step(&counts, (0, 0), Direction::Forward), Some((0, 1)));
    assert_eq!(step(&counts, (0, 2), Direction::Forward), Some((1, 0)));
    assert_eq!(step(&counts, (1, 0), Direction::Forward), Some((2, 0)));
    assert_eq!(step(&counts, (2, 1), Direction::Forward), None);

    assert_eq!(step(&counts, (2, 1), Direction::Back), Some((2, 0)));
    assert_eq!(step(&counts, (2, 0), Direction::Back), Some((1, 0)));
    assert_eq!(step(&counts, (1, 0), Direction::Back), Some((0, 2)));
    assert_eq!(step(&counts, (0, 0), Direction::Back), None);
}

#[test]
fn up_and_down_move_a_set_at_a_time_and_keep_the_place_in_it() {
    let counts = [4, 2, 5];

    assert_eq!(step(&counts, (0, 1), Direction::NextSet), Some((1, 1)));
    assert_eq!(step(&counts, (1, 1), Direction::PreviousSet), Some((0, 1)));

    // The set arrived at is shorter, so it lands on the last picture in it.
    assert_eq!(step(&counts, (0, 3), Direction::NextSet), Some((1, 1)));
    assert_eq!(step(&counts, (2, 4), Direction::PreviousSet), Some((1, 1)));

    assert_eq!(step(&counts, (2, 0), Direction::NextSet), None);
    assert_eq!(step(&counts, (0, 0), Direction::PreviousSet), None);
}

#[test]
fn a_list_with_nothing_in_it_moves_nowhere() {
    for direction in [
        Direction::Forward,
        Direction::Back,
        Direction::NextSet,
        Direction::PreviousSet,
    ] {
        assert_eq!(step(&[], (0, 0), direction), None, "on {direction:?}");
        assert_eq!(step(&[1], (0, 0), direction), None, "on {direction:?}");
    }
}

/// A cursor key moves the preview from where it is now, which is a file id,
/// not a place in the list.
#[test]
fn walking_moves_the_preview_to_the_next_picture() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 2, "the two pairs were not found");
    let ids: Vec<Vec<i64>> = app
        .sets
        .iter()
        .map(|set| set.members.iter().map(|member| member.file_id).collect())
        .collect();
    let visible = vec![0, 1];

    app.selected = Some(ids[0][0]);
    app.walk(&visible, Direction::Forward);
    assert_eq!(app.selected, Some(ids[0][1]));

    app.walk(&visible, Direction::Forward);
    assert_eq!(
        app.selected,
        Some(ids[1][0]),
        "the end of a set did not cross into the next"
    );

    app.walk(&visible, Direction::Forward);
    app.walk(&visible, Direction::Forward);
    assert_eq!(
        app.selected,
        Some(ids[1][1]),
        "the end of the list moved somewhere"
    );

    app.walk(&visible, Direction::PreviousSet);
    assert_eq!(app.selected, Some(ids[0][1]));

    app.selected = None;
    app.walk(&visible, Direction::Forward);
    assert_eq!(
        app.selected, None,
        "nothing was selected and something moved"
    );
}

/// Forgetting a folder takes the index and everything beside it, and says
/// how many pictures went with it. The count used to be a hardcoded nought,
/// because the window removed the file itself and had nothing left to count.
#[test]
fn a_folder_forgotten_leaves_no_index_and_says_how_many_rows_went() {
    let scanned = folder_with_a_duplicate();
    let db_path = headless::default_db_path(scanned.path());
    let app = reviewing(scanned.path());
    app.index.synced().expect("wait for the file");
    assert!(db_path.is_file(), "the pass wrote no index");

    let went = discard_index(&app.index);
    assert_eq!(
        went, 3,
        "the count of what went is not what the folder held"
    );
    assert!(!db_path.exists(), "{} was left behind", db_path.display());
    assert!(
        app.index.open_index_path().is_none(),
        "the manager still has an index open that is gone"
    );
}

/// The index holds a row for every file the pass has been through, pictures
/// or not. What went with it is a count of pictures, because that is what
/// "the index was deleted and N rows went with it" is read as.
#[test]
fn what_went_with_a_forgotten_index_is_counted_in_pictures() {
    let dir = tempfile::tempdir().expect("tempdir");
    for name in ["a.png", "b.png"] {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(48, 32, |x, y| {
            image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
        }))
        .save_with_format(dir.path().join(name), image::ImageFormat::Png)
        .expect("a fixture");
    }
    std::fs::write(dir.path().join("pretend.png"), b"not a picture").expect("a fixture");

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(dir.path().to_path_buf());
    app.start_scan();
    settle(&mut app);
    app.index.synced().expect("wait for the file");

    assert_eq!(
        discard_index(&app.index),
        2,
        "the file that is not a picture was counted as one"
    );
}

/// A folder whose index cannot be read stops there: the lamp for reading the
/// index stays red, what went wrong is on screen, and nothing else about the
/// folder is attempted. What to do about it is the user's to decide.
#[test]
fn a_broken_index_leaves_the_lamp_red_and_the_window_stopped() {
    let found = folder_with_a_duplicate();
    let db_path = headless::default_db_path(found.path());
    std::fs::write(&db_path, b"not a database, just some bytes").expect("a broken index");
    let was = std::fs::read(&db_path).expect("read it");

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(found.path().to_path_buf());
    settle(&mut app);

    assert!(
        app.error.is_some(),
        "nothing said the index could not be read"
    );
    assert_eq!(
        app.how_it_went(Lamp::CheckedForIndexFile),
        Went::Waiting,
        "the lamp for the index went green over an index that cannot be read"
    );
    assert_eq!(
        app.how_it_went(Lamp::LoadedIndexIntoMemory),
        Went::Waiting,
        "the window went on to read an index it could not open"
    );
    assert!(
        app.images.is_none(),
        "pictures came out of an index that cannot be read"
    );
    assert!(
        app.running.is_none(),
        "a pass was started on a folder that stopped"
    );
    assert_eq!(
        std::fs::read(&db_path).expect("read"),
        was,
        "the broken index was written to"
    );
}

/// A folder whose index was written by an older build opens: the manager
/// brings the file to the shape this build reads before anything is served
/// from it, so the window comes up on it without a pass having to happen
/// first.
#[test]
fn a_folder_whose_index_is_in_an_older_shape_still_opens() {
    let found = folder_with_a_duplicate();
    let db_path = headless::default_db_path(found.path());
    {
        let scanned = reviewing(found.path());
        scanned.index.synced().expect("wait for the file");
        assert_eq!(
            scanned.sets.len(),
            1,
            "the fixture found nothing to begin with"
        );
        scanned.index.close().expect("close the index");
    }

    // The index as an older build left it.
    let older = rusqlite::Connection::open(&db_path).expect("the index file");
    older
        .execute_batch(
            "DROP VIEW IF EXISTS indexed_images;
             ALTER TABLE fingerprints DROP COLUMN corners;",
        )
        .expect("making an older index");
    drop(older);

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(found.path().to_path_buf());
    settle(&mut app);
    assert!(
        app.error.is_none(),
        "the folder would not open: {:?}",
        app.error
    );
    app.load_sets();
    settle(&mut app);
    assert_eq!(
        app.sets.len(),
        1,
        "the older index gave nothing without a pass"
    );

    // And the file itself is in the current shape, not only what the manager
    // is holding.
    let conn = index_file(&app, &db_path);
    let corners: i64 = conn
        .query_row(
            "SELECT count(*) FROM pragma_table_info('fingerprints') WHERE name = 'corners'",
            [],
            |row| row.get(0),
        )
        .expect("looking for the column");
    assert_eq!(corners, 1, "the file was left in the older shape");
}

/// With the checkbox unticked, the index is deleted once the cleanup is over,
/// along with everything the manager left beside it. The next run opens the
/// folder with no index and a scan builds it again.
#[test]
fn an_unticked_folder_loses_its_index_when_the_cleanup_is_done() {
    let scanned = folder_with_a_duplicate();
    let db_path = headless::default_db_path(scanned.path());
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(scanned.path().to_path_buf());
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);
    assert!(db_path.is_file(), "the pass wrote no index");
    assert!(!app.keep_index, "a fresh folder is not remembered");

    app.destination = Destination::Delete;
    let plan = app.build_plan();
    app.view = View::Cleanup;
    app.run_cleanup(&plan);
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.removing.is_some() && std::time::Instant::now() < until {
        app.pump_cleanup(&ctx);
    }

    assert!(!db_path.exists(), "{} was left behind", db_path.display());

    // And the window is back on the scan with nothing on it, so the only way
    // on is another folder or another scan.
    assert_eq!(app.view, View::Scan);
    assert!(app.sets.is_empty());
    assert_eq!(
        app.scan.total, 0,
        "the last scan's numbers are still on screen"
    );
}

/// Dropping the removed files rewrites the index, which is seconds of work on
/// a large folder. It happens on the removal's thread, and the frame that
/// takes the outcome is handed the count rather than working it out, or the
/// window stops painting at exactly the moment the cleanup looks finished.
#[test]
fn taking_the_outcome_does_not_touch_the_index() {
    let scanned = folder_with_a_duplicate();
    let db_path = headless::default_db_path(scanned.path());
    let mut app = reviewing(scanned.path());
    // Marked, so there is a cleanup to carry out at all.
    app.auto_mark_to_keep();
    // Kept, so the cleanup drops the rows rather than deleting the index.
    app.keep_index = true;
    app.destination = Destination::Delete;
    let plan = app.build_plan();
    let going = plan.removals[0].rel_path.clone();

    app.view = View::Cleanup;
    app.run_cleanup(&plan);
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.removing.is_some() && std::time::Instant::now() < until {
        app.pump_cleanup(&ctx);
    }

    let conn = index_file(&app, &db_path);
    let left: i64 = conn
        .query_row(
            "SELECT count(*) FROM files WHERE rel_path = ?1",
            [&going],
            |row| row.get(0),
        )
        .expect("count");
    assert_eq!(left, 0, "the removed file is still in the index");

    let said = app.cleanup_result.clone().expect("a result");
    assert!(said.contains("1 dropped from the index"), "{said}");
}

/// Files that were not pictures, and files that could not be read, are not
/// pictures this pass found. A folder whose only unindexed files are of those
/// two kinds has found nothing, however many times it reads them.
///
/// A file whose name claims no picture format is not one of those two: it is
/// never looked at, so it is not among the files the pass went through.
#[test]
fn what_was_skipped_and_what_broke_are_not_counted_as_found() {
    // A folder of pictures, one of which is not a picture at all, read twice:
    // the first pass finds them, the second finds nothing new.
    let dir = tempfile::tempdir().expect("tempdir");
    for name in ["a.png", "b.png"] {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(48, 32, |x, y| {
            image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
        }))
        .save_with_format(dir.path().join(name), image::ImageFormat::Png)
        .expect("a fixture");
    }
    // Named as no format at all: not read, not counted, not anything.
    std::fs::write(dir.path().join("notes.txt"), b"not a picture").expect("a fixture");
    // Named as a picture and not one: read, and refused by its first bytes.
    std::fs::write(dir.path().join("pretend.png"), b"not a picture").expect("a fixture");
    std::fs::write(dir.path().join("broken.png"), b"\x89PNG\r\n\x1a\ncut").expect("a fixture");

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(dir.path().to_path_buf());
    app.start_scan();
    settle(&mut app);

    assert_eq!(
        app.scan.done, 4,
        "the pass looked at a file that claims no format"
    );
    assert_eq!(
        app.scan.ignored, 1,
        "the file that is not a picture was not counted"
    );
    assert_eq!(
        app.scan.failures.len(),
        1,
        "the broken file is its own number"
    );
    assert_eq!(app.scan.found(), 2, "found is the pictures, not the files");

    app.start_scan();
    settle(&mut app);
    // All four, not just the two pictures: the file that is not one and the
    // broken one were looked at and written down as looked at, so the second
    // pass has nothing to read at all.
    assert_eq!(
        app.scan.unchanged, 4,
        "a file the last pass looked at was read again"
    );
    assert_eq!(
        app.scan.done, app.scan.unchanged,
        "the second pass read a file, where everything in the folder was known"
    );
    assert_eq!(
        app.scan.found(),
        0,
        "a pass with nothing new found something"
    );
}

/// A set's tiles are as wide as that set's own widest picture. Nothing about
/// another set reaches into this one.
#[test]
fn a_set_of_portraits_is_not_given_the_width_of_a_landscape() {
    let portrait = fitted(900, 1200);
    let landscape = fitted(1600, 900);
    assert!(
        portrait.x < landscape.x,
        "the two shapes fitted to the same width"
    );
    assert!(
        portrait.y <= TILE.y && landscape.y <= TILE.y,
        "a picture came out too tall"
    );
    assert!(
        portrait.x <= TILE.x && landscape.x <= TILE.x,
        "a picture came out too wide"
    );

    let of = |width: u32, height: u32| {
        let mut member = member(1, "a.jpg", 100);
        member.width = width;
        member.height = height;
        member
    };
    // The column is the picture plus room for the border and the ring drawn
    // around it, or the neighbouring tile clips them.
    let around = TILE_BORDER + TILE_RING * 2.0;
    assert!(
        around >= 12.0,
        "there is not enough room around a picture for its ring"
    );
    assert_eq!(tile_width(&of(900, 1200)), portrait.x + around);
    assert_eq!(
        tile_width(&of(1600, 900)),
        landscape.x + around,
        "a tile is the width of its own picture"
    );
    assert!(
        tile_width(&of(900, 1200)) < tile_width(&of(1600, 900)),
        "a portrait was given a landscape's column, which is the gap beside it"
    );
}

/// The tally beside the set and duplicate counts: exactly what a cleanup
/// would take. It follows the marks, so it moves as the marks do.
#[test]
fn the_selected_tally_is_every_picture_a_cleanup_would_take() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 2, "the two pairs were not found");
    let bytes_of = |app: &App, set: usize, keeper: i64| -> i64 {
        app.sets[set]
            .members
            .iter()
            .filter(|member| member.file_id != keeper)
            .map(|member| member.size_bytes)
            .sum()
    };
    // Four pictures and nothing marked, so all four are going.
    let (going, bytes) = app.selected_for_removal();
    assert_eq!(going, 4, "an untouched review was keeping something");
    assert_eq!(bytes, bytes_of(&app, 0, -1) + bytes_of(&app, 1, -1));

    // One marked in each set leaves the other picture of each going. Marked
    // the way a person marks one, because the tally follows from the marking
    // and not from the field it lands in.
    let kept_first = app.sets[0].members[0].file_id;
    let kept_second = app.sets[1].members[0].file_id;
    app.selected = Some(kept_first);
    app.keep_selected();
    app.selected = Some(kept_second);
    app.keep_selected();
    let (going, bytes) = app.selected_for_removal();
    assert_eq!(going, 2);
    assert_eq!(
        bytes,
        bytes_of(&app, 0, kept_first) + bytes_of(&app, 1, kept_second)
    );

    // Taking one set's marks off puts the whole set back in the tally.
    app.keep_selected();
    let (going, bytes) = app.selected_for_removal();
    assert_eq!(going, 3);
    assert_eq!(bytes, bytes_of(&app, 0, kept_first) + bytes_of(&app, 1, -1));

    // And it is the same count the plan carries out.
    assert_eq!(app.build_plan().files(), 3);
}

/// What the toolbar counts: pictures that would go if every set kept one, not
/// pictures that are in a set.
#[test]
fn the_duplicate_count_is_every_picture_but_the_one_each_set_keeps() {
    assert_eq!(duplicate_count(&[]), 0);

    // Five pictures in two sets: a pair and a triple.
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, seed) in [
        ("a1.png", 0),
        ("a2.png", 0),
        ("b1.png", 120),
        ("b2.png", 120),
        ("b3.png", 120),
    ] {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([
                ((x * 3 + seed) % 256) as u8,
                ((y * 5 + seed) % 256) as u8,
                40,
            ])
        }))
        .save_with_format(dir.path().join(name), image::ImageFormat::Png)
        .expect("a fixture");
    }

    let app = reviewing(dir.path());
    assert_eq!(
        app.sets.len(),
        2,
        "the pair and the triple were not two sets"
    );
    assert_eq!(
        duplicate_count(&app.sets),
        3,
        "five pictures in two sets is three copies"
    );
}

/// The keys follow what is on screen. A set that is not in the list the walk
/// was given is not somewhere the preview can go.
#[test]
fn walking_only_visits_the_sets_it_was_given() {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, seed) in [
        ("a1.png", 0),
        ("a2.png", 0),
        ("b1.png", 90),
        ("b2.png", 90),
        ("c1.png", 180),
        ("c2.png", 180),
    ] {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([
                ((x * 3 + seed) % 256) as u8,
                ((y * 5 + seed) % 256) as u8,
                40,
            ])
        }))
        .save_with_format(dir.path().join(name), image::ImageFormat::Png)
        .expect("a fixture");
    }

    let mut app = reviewing(dir.path());
    assert_eq!(app.sets.len(), 3, "the three pairs were not three sets");
    let last_of_first = app.sets[0].members[1].file_id;
    let first_of_third = app.sets[2].members[0].file_id;

    app.selected = Some(last_of_first);
    app.walk(&[0, 2], Direction::Forward);
    assert_eq!(
        app.selected,
        Some(first_of_third),
        "the walk went into a hidden set"
    );
}

/// A row the cursor keys reached must be brought into sight, and one already
/// in sight must not shift the list under the pointer.
#[test]
fn the_row_walked_to_is_brought_to_the_middle() {
    let (height, spacing, viewport) = (240.0, 8.0, 700.0);
    let rows = 40;
    let show =
        |row: usize, offset: f32| scroll_to_show(row, rows, height, spacing, offset, viewport);

    // Rows are 248 apart and 700 is on screen, so a row in the middle sits
    // with its own middle at 350. Row one's middle is 368, so the list moves
    // by 18 even though the whole row was already in sight.
    assert_eq!(
        show(1, 0.0),
        Some(18.0),
        "a row in sight but off centre did not move"
    );
    assert_eq!(show(2, 0.0), Some(616.0 - 350.0));
    assert_eq!(show(3, 0.0), Some(864.0 - 350.0));

    // Where it was reached from makes no difference: the row ends up in the
    // same place walking down to it or up to it.
    assert_eq!(show(3, 0.0), show(3, 5_000.0));

    // Nothing to do when it is already in the middle.
    assert_eq!(
        show(1, 18.0),
        None,
        "the list was moved to where it already was"
    );

    // The ends are the exception. Row zero's middle is 120 and half a screen
    // above that is off the top, so it stops there.
    assert_eq!(
        show(0, 100.0),
        Some(0.0),
        "the first row scrolled past the top"
    );
    let content = rows as f32 * (height + spacing) - spacing;
    assert_eq!(
        show(rows - 1, 0.0),
        Some(content - viewport),
        "the last row pulled the list past its own end"
    );
}

#[test]
fn walking_asks_for_the_row_it_moved_to_to_be_shown() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 2, "the two pairs were not found");
    let visible = vec![0, 1];

    // From the last picture of the first set into the second set.
    app.selected = Some(app.sets[0].members[1].file_id);
    app.scroll_to = None;
    app.walk(&visible, Direction::Forward);
    assert_eq!(app.scroll_to, Some(1));

    // And from the last picture of the list, which has nowhere to go.
    app.selected = Some(app.sets[1].members[1].file_id);
    app.scroll_to = None;
    app.walk(&visible, Direction::Forward);
    assert_eq!(
        app.scroll_to, None,
        "the end of the list asked for a scroll"
    );
}

/// The list places every row it is not drawing at a multiple of
/// `SET_ROW_HEIGHT`. A row that comes out any other height moves the content
/// under a scroll that is already running, which reads as the list trembling
/// and jumping back the way it came.
#[test]
fn a_set_row_takes_exactly_the_height_the_list_places_it_at() {
    let ctx = window();
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    let root = found.path().to_path_buf();
    assert_eq!(app.sets.len(), 2, "the two pairs were not found");

    let mut taken = Vec::new();
    let _ = ctx.run(Default::default(), |ctx| {
        egui::CentralPanel::default().show(ctx, |ui| {
            for index in 0..app.sets.len() {
                let placed = set_row_height(ui);
                let before = ui.next_widget_position().y;
                app.set_row(ui, index, &root, ui.available_width());
                let spacing = ui.spacing().item_spacing.y;
                taken.push((ui.next_widget_position().y - before - spacing, placed));
            }
        });
    });

    for (index, (height, placed)) in taken.iter().enumerate() {
        assert!(
            (height - placed).abs() < 0.5,
            "row {index} took {height}, the list places rows every {placed}"
        );
    }
}

/// Where duplicates go is a fact about the folder they were found in, so it
/// lives in that folder's index and not in the application's settings.
#[test]
fn the_cleanup_choice_is_kept_with_the_folders_index() {
    // A folder the window scanned, with a destination chosen for it.
    let scanned = folder_with_a_duplicate();
    let held = tempfile::tempdir().expect("tempdir");
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(scanned.path().to_path_buf());
    app.start_scan();
    settle(&mut app);
    app.destination = Destination::MoveTo;
    app.move_dir = held.path().display().to_string();
    app.remember_disposal();

    // Opened again from nothing, as the next run of the window would.
    closed(&app);
    let mut opened = App::from_settings(crate::settings::Settings::default());
    assert_eq!(
        opened.destination,
        Destination::Trash,
        "the test started from the default"
    );
    opened.open_folder(scanned.path().to_path_buf());
    settle(&mut opened);
    assert_eq!(opened.destination, Destination::MoveTo);
    assert_eq!(opened.move_dir, held.path().display().to_string());
}

/// How far down the folder an index reaches is a fact about the index, not a
/// preference. Opening it with the box clear would drop every row under a
/// subfolder as vanished on the next pass.
#[test]
fn an_index_built_over_the_subfolders_opens_with_the_box_ticked() {
    // A folder with a picture in a subfolder, scanned with the box ticked.
    let dir = tempfile::tempdir().expect("tempdir");
    std::fs::create_dir(dir.path().join("under")).expect("mkdir");
    for name in ["top.png", "under/deep.png"] {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(48, 32, |x, y| {
            image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
        }))
        .save_with_format(dir.path().join(name), image::ImageFormat::Png)
        .expect("a fixture");
    }

    let mut app = App::from_settings(crate::settings::Settings::default());
    assert!(!app.recurse, "the window starts on the folder itself");
    app.open_folder(dir.path().to_path_buf());
    app.recurse = true;
    app.start_scan();
    settle(&mut app);
    assert_eq!(app.scan.done, 2, "the pass did not go into the subfolder");

    // Opened again from nothing, as the next run of the window would.
    closed(&app);
    let mut opened = App::from_settings(crate::settings::Settings::default());
    opened.open_folder(dir.path().to_path_buf());
    settle(&mut opened);
    assert!(
        opened.recurse,
        "the index reaches into the subfolders and the box does not"
    );

    // And a pass over the folder alone puts the box back down.
    opened.recurse = false;
    opened.start_scan();
    settle(&mut opened);
    closed(&app);
    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(dir.path().to_path_buf());
    settle(&mut again);
    assert!(
        !again.recurse,
        "the index is the folder itself and the box says otherwise"
    );
}

#[test]
fn an_index_that_has_never_been_cleaned_up_keeps_the_safe_default() {
    let scanned = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(scanned.path().to_path_buf());
    app.start_scan();
    settle(&mut app);

    closed(&app);
    let mut opened = App::from_settings(crate::settings::Settings::default());
    opened.open_folder(scanned.path().to_path_buf());
    settle(&mut opened);
    assert_eq!(opened.destination, Destination::Trash);
    assert_eq!(opened.move_dir, "");
}

/// Moving files to a folder is not removing them, and the button that does it
/// says so.
#[test]
fn the_button_says_what_the_chosen_destination_actually_does() {
    assert_eq!(Destination::MoveTo.verb(), "Move");
    assert_eq!(Destination::Trash.verb(), "Remove");
    assert_eq!(Destination::Delete.verb(), "Remove");
}

#[test]
fn every_cleanup_choice_survives_being_written_and_read_back() {
    for choice in [Destination::Trash, Destination::MoveTo, Destination::Delete] {
        assert_eq!(Destination::from_name(choice.name()), Some(choice));
    }
    assert_eq!(Destination::from_name("something else"), None);
}

/// What was actually painted, so a scroll bar that is reserved but drawn in a
/// colour nobody can see counts as missing. Twice now it has been invisible
/// while the space for it was there.
fn painted_rects() -> Vec<egui::epaint::RectShape> {
    let ctx = window();
    install_style(&ctx);
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(400.0, 300.0),
        )),
        ..Default::default()
    };

    let mut shapes = Vec::new();
    // A scroll area only knows on its second frame whether what it holds is
    // taller than it is, and then animates the bar to its full width over
    // several more. What is wanted here is where it settles.
    for _ in 0..30 {
        shapes = crate::shot::frame("a_list_that_scrolls", &ctx, input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                scrolled(
                    ui,
                    egui::Id::new("a list"),
                    true,
                    20.0,
                    None,
                    egui::ScrollArea::vertical().auto_shrink([false, false]),
                    |area, ui| {
                        area.show(ui, |ui| {
                            for row in 0..200 {
                                ui.label(format!("row {row}"));
                            }
                        })
                    },
                );
            });
        });
    }

    shapes
        .into_iter()
        .filter_map(|clipped| match clipped.shape {
            egui::Shape::Rect(rect) => Some(rect),
            _ => None,
        })
        .collect()
}

/// How far one turn of the wheel asks for, in the checks below.
const WHEEL_TURN: f32 = 40.0;

/// A point in the strip above the list, and a point in the list itself.
const BESIDE_THE_LIST: egui::Pos2 = egui::pos2(200.0, 40.0);
const ON_THE_LIST: egui::Pos2 = egui::pos2(150.0, 250.0);

/// A list with a strip above it standing in for the picture, the two of them
/// making up a pane, with the wheel counted over the whole pane.
///
/// `turns` wheel events arrive with the pointer at `at`, and then the frames
/// run on without any until the list settles, because a turn arrives spread
/// over the frames that follow it. Gives back where the list ended up, how
/// much there is of it and how much of it shows.
fn wheeled(at: egui::Pos2, turns: usize) -> (f32, f32, f32) {
    let ctx = window();
    install_style(&ctx);
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(400.0, 300.0));
    let mut out = (0.0, 0.0, 0.0);
    let mut clock = 0.0;
    for frame in 0..turns + 40 {
        clock += 0.1;
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(clock),
            ..Default::default()
        };
        input.events.push(egui::Event::PointerMoved(at));
        if frame < turns {
            input.events.push(egui::Event::MouseWheel {
                unit: egui::MouseWheelUnit::Point,
                delta: egui::vec2(0.0, -WHEEL_TURN),
                modifiers: egui::Modifiers::NONE,
            });
        }
        crate::shot::frame("a_wheel_over_a_list", &ctx, input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let pane = ui.max_rect();
                ui.allocate_space(egui::vec2(ui.available_width(), 100.0));
                let (_, offset, viewport, content) = scrolled(
                    ui,
                    egui::Id::new("a wheeled list"),
                    true,
                    WHEEL_TURN,
                    Some(pane),
                    egui::ScrollArea::vertical().auto_shrink([false, false]),
                    |area, ui| {
                        area.show_rows(ui, 20.0, 200, |ui, rows| {
                            for row in rows {
                                ui.label(format!("row {row}"));
                            }
                        })
                    },
                );
                out = (offset, content, viewport);
            });
        });
    }
    out
}

/// The wheel over the list is the toolkit's to apply, and applying it a
/// second time by hand is what made the list jump.
#[test]
fn a_wheel_over_the_list_moves_it_once() {
    let (offset, _, _) = wheeled(ON_THE_LIST, 1);
    assert!(
        (offset - WHEEL_TURN).abs() < 1.0,
        "one turn of the wheel moved the list {offset} rather than {WHEEL_TURN}"
    );
}

/// Turning the wheel until it stops moving arrives at the end of the list,
/// rather than somewhere short of it. From beside the list, which is the
/// side this works out for itself rather than leaving to the toolkit.
#[test]
fn the_wheel_reaches_the_bottom_of_the_list() {
    let (offset, content, viewport) = wheeled(BESIDE_THE_LIST, 200);
    let furthest = content - viewport;
    assert!(
        (offset - furthest).abs() < 1.0,
        "the wheel stopped at {offset} of {furthest}"
    );
}

/// The list is one part of a pane, and a wheel turned over the rest of that
/// pane is a wheel turned on the only thing in it that moves.
#[test]
fn a_wheel_beside_the_list_still_scrolls_it() {
    let (offset, _, _) = wheeled(BESIDE_THE_LIST, 1);
    assert!(offset > 0.0, "the list did not move for a wheel beside it");
}

/// The review view as it is really built, panels and all, rather than a bare
/// scroll area that proves nothing about it.
fn review_rects(visuals: egui::Visuals) -> Vec<egui::epaint::RectShape> {
    review_rects_sized(visuals, egui::vec2(1200.0, 800.0), None)
}

fn review_rects_sized(
    visuals: egui::Visuals,
    screen: egui::Vec2,
    preview_width: Option<f32>,
) -> Vec<egui::epaint::RectShape> {
    let ctx = window();
    ctx.set_visuals(visuals);

    let mut app = App::from_settings(crate::settings::Settings {
        folder: Some(PathBuf::from(".")),
        preview_width,
        ..crate::settings::Settings::default()
    });
    app.view = View::Review;
    app.sets = (0..40)
        .map(|index| DuplicateSet {
            set_id: index,
            members: vec![
                member(index * 2, "a.jpg", 500),
                member(index * 2 + 1, "b.jpg", 300),
            ],
        })
        .collect();

    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(egui::pos2(0.0, 0.0), screen)),
        ..Default::default()
    };

    let mut shapes = Vec::new();
    for _ in 0..30 {
        shapes = crate::shot::frame("the_review_list_bar", &ctx, input.clone(), |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
        });
    }

    shapes
        .into_iter()
        .filter_map(|clipped| match clipped.shape {
            egui::Shape::Rect(rect) => Some(rect),
            _ => None,
        })
        .collect()
}

/// The strip belongs to the list, so it sits at the left of whatever divides
/// the list from the preview, not at the window edge.
fn scroll_strip(rects: &[egui::epaint::RectShape]) -> Vec<&egui::epaint::RectShape> {
    rects
        .iter()
        .filter(|shape| {
            (shape.rect.width() - SCROLL_BAR).abs() < 0.5 && shape.rect.height() > SCROLL_BAR
        })
        .collect()
}

/// How far apart two colours are, so "visible" is a number and not a claim.
fn contrast(a: egui::Color32, b: egui::Color32) -> i32 {
    let channel =
        |c: egui::Color32| (c.r() as i32 * 299 + c.g() as i32 * 587 + c.b() as i32 * 114) / 1000;
    (channel(a) - channel(b)).abs()
}

#[test]
fn the_review_list_paints_a_twelve_point_bar_beside_it_in_either_theme() {
    for (name, visuals) in [
        ("light", egui::Visuals::light()),
        ("dark", egui::Visuals::dark()),
    ] {
        let rects = review_rects(visuals);
        let strip = scroll_strip(&rects);
        assert!(
            strip.len() >= 2,
            "{name}: no {SCROLL_BAR} point track and handle in the review view, \
             the tall narrow rects were {:?}",
            rects
                .iter()
                .filter(|shape| shape.rect.width() < 40.0 && shape.rect.height() > 100.0)
                .map(|shape| (shape.rect, shape.fill))
                .collect::<Vec<_>>()
        );

        let tallest = strip
            .iter()
            .max_by(|a, b| a.rect.height().total_cmp(&b.rect.height()))
            .unwrap();
        let handle = strip
            .iter()
            .min_by(|a, b| a.rect.height().total_cmp(&b.rect.height()))
            .unwrap();
        println!(
            "{name}: track {:?} {:?}, handle {:?} {:?}",
            tallest.rect, tallest.fill, handle.rect, handle.fill
        );

        assert!(
            handle.rect.height() < tallest.rect.height(),
            "{name}: no handle in the track"
        );
        assert!(
            handle.fill.a() > 0 && tallest.fill.a() > 0,
            "{name}: painted transparent"
        );
        let apart = contrast(handle.fill, tallest.fill);
        assert!(
            apart >= 40,
            "{name}: the handle is {apart} apart from its track, invisible"
        );
    }
}

/// The bar is drawn here rather than by the toolkit, so everything on it has
/// to be checked: a track, a handle inside it, and a triangle at each end.
#[test]
fn the_bar_has_a_track_a_handle_and_a_button_at_each_end() {
    let ctx = window();
    ctx.set_visuals(egui::Visuals::light());
    install_style(&ctx);

    let strip = egui::Rect::from_min_size(egui::pos2(388.0, 0.0), egui::vec2(12.0, 300.0));
    let shapes = crate::shot::frame(
        "the_bar_has_a_track_a_handle_and_a_button_at_each_end",
        &ctx,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                paint_scroll_bar(ui, strip, true, 60.0, 3000.0, 300.0, 0.0);
            });
        },
    );

    let mut track = None;
    let mut handle = None;
    let mut triangles = 0;
    for clipped in shapes {
        match clipped.shape {
            egui::Shape::Rect(rect) if rect.rect == strip => track = Some(rect),
            egui::Shape::Rect(rect) if rect.rect.width() == strip.width() => {
                handle = Some(rect);
            }
            egui::Shape::Path(path) if path.points.len() == 3 => triangles += 1,
            _ => {}
        }
    }

    let track = track.expect("no track filling the strip");
    let handle = handle.expect("no handle in the track");
    assert_eq!(triangles, 2, "expected a triangle at each end of the bar");
    assert!(
        handle.rect.height() < track.rect.height(),
        "the handle is not shorter than its track"
    );
    assert!(
        handle.rect.top() >= track.rect.top() + strip.width(),
        "the handle overlaps the button at the top"
    );
    assert!(
        handle.rect.bottom() <= track.rect.bottom() - strip.width(),
        "the handle overlaps the button at the bottom"
    );
    assert!(
        contrast(handle.fill, track.fill) >= 40,
        "the handle cannot be told from its track"
    );
}

/// Pressing the handle holds it where it is, and dragging moves it by how far
/// the pointer moved. It must not jump so its middle is under the pointer.
#[test]
fn pressing_the_handle_holds_it_where_it_was_and_drags_from_there() {
    let ctx = window();
    ctx.set_visuals(egui::Visuals::light());

    let strip = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(12.0, 312.0));
    let (content, viewport, step) = (3000.0, 300.0, 60.0);

    // A third of the way down, and where the handle sits when it is, worked
    // out the same way the bar does rather than written down here.
    let offset = (content - viewport) / 3.0;
    let track_length = strip.height() - strip.width() * 2.0;
    let handle_length = (track_length * viewport / content).max(strip.width() * 2.0);
    let travel = track_length - handle_length;
    let handle_start = strip.width() + (offset / (content - viewport)) * travel;

    let press_at = |y: f32, offset: f32| {
        let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(200.0, 400.0));
        let at = egui::pos2(6.0, y);
        let wanted = std::cell::RefCell::new(None);

        // The pointer moves on one frame, goes down on the next, and comes up
        // on the last. A widget has to have been there on a previous frame
        // before egui reports the pointer as being on it, and the release is
        // what lets go of the grip for whatever presses next.
        for button in [None, Some(true), Some(false)] {
            let mut input = egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            };
            input.events.push(egui::Event::PointerMoved(at));
            if let Some(pressed) = button {
                input.events.push(egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
            // What the frame drew is not looked at here: this drives the bar
            // and reads what it decided, which comes back through `wanted`.
            // The picture is kept anyway, for when it decides something else.
            let _ = crate::shot::frame(
                "pressing_the_handle_holds_it_where_it_was",
                &ctx,
                input,
                |ctx| {
                    egui::CentralPanel::default().show(ctx, |ui| {
                        let moved =
                            paint_scroll_bar(ui, strip, true, step, content, viewport, offset);
                        if button == Some(true) {
                            *wanted.borrow_mut() = moved;
                        }
                    });
                },
            );
        }
        wanted.into_inner()
    };

    // Anywhere on the handle: it stays where it is, rather than jumping so
    // its middle is under the pointer.
    for (name, at) in [
        ("its middle", handle_start + handle_length / 2.0),
        ("its top", handle_start + 2.0),
        ("its bottom", handle_start + handle_length - 2.0),
    ] {
        let held = press_at(at, offset).expect("the press did nothing");
        assert!(
            (held - offset).abs() < 1.0,
            "pressing {name} moved the handle from {offset} to {held}"
        );
    }

    // The track outside the handle has nothing to hold, so it jumps.
    let jumped =
        press_at(handle_start + handle_length + 40.0, offset).expect("the press did nothing");
    assert!(
        jumped > offset,
        "pressing below the handle did not move down: {jumped}"
    );
}

/// The same bar lies on its side for anything that scrolls sideways, and its
/// buttons point the way they scroll.
#[test]
fn a_sideways_bar_has_the_same_parts_lying_down() {
    let ctx = window();
    ctx.set_visuals(egui::Visuals::light());

    let strip = egui::Rect::from_min_size(egui::pos2(0.0, 288.0), egui::vec2(400.0, 12.0));
    let shapes = crate::shot::frame(
        "a_sideways_bar_has_the_same_parts_lying_down",
        &ctx,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(400.0, 300.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                paint_scroll_bar(ui, strip, false, 60.0, 4000.0, 400.0, 0.0);
            });
        },
    );

    let mut track = None;
    let mut handle = None;
    let mut triangles = Vec::new();
    for clipped in shapes {
        match clipped.shape {
            egui::Shape::Rect(rect) if rect.rect == strip => track = Some(rect),
            egui::Shape::Rect(rect) if rect.rect.height() == strip.height() => {
                handle = Some(rect);
            }
            egui::Shape::Path(path) if path.points.len() == 3 => triangles.push(path),
            _ => {}
        }
    }

    let track = track.expect("no track filling the strip");
    let handle = handle.expect("no handle in the track");
    assert_eq!(triangles.len(), 2, "expected a button at each end");
    assert!(
        handle.rect.width() < track.rect.width(),
        "the handle is as wide as its track"
    );
    assert!(
        handle.rect.left() >= track.rect.left() + strip.height(),
        "over the left button"
    );

    // One triangle points left and one right, or both buttons look the same.
    let widest = |path: &egui::epaint::PathShape| {
        let xs: Vec<f32> = path.points.iter().map(|point| point.x).collect();
        xs.iter().cloned().fold(f32::MIN, f32::max) - xs.iter().cloned().fold(f32::MAX, f32::min)
    };
    assert!(
        triangles.iter().all(|path| widest(path) > 0.0),
        "the triangles have no width, so they are not pointing sideways"
    );
}

/// With the preview pane dragged wide, the list is a narrow column. The bar
/// still belongs to it and still has to be there.
#[test]
fn the_bar_is_there_when_the_preview_has_taken_most_of_the_window() {
    let rects = review_rects_sized(
        egui::Visuals::light(),
        egui::vec2(1523.0, 1067.0),
        Some(1162.0),
    );
    let strip = scroll_strip(&rects);
    for shape in &strip {
        println!("narrow list: bar {:?} filled {:?}", shape.rect, shape.fill);
    }
    assert!(
        strip.len() >= 2,
        "no bar beside a list only {} points wide, the tall narrow rects were {:?}",
        1523.0 - 1162.0,
        rects
            .iter()
            .filter(|shape| shape.rect.width() < 40.0 && shape.rect.height() > 100.0)
            .map(|shape| (shape.rect, shape.fill))
            .collect::<Vec<_>>()
    );
}

#[test]
fn a_list_that_scrolls_paints_a_twelve_point_handle_at_its_right_edge() {
    let rects = painted_rects();
    let right = 400.0;

    let strip: Vec<&egui::epaint::RectShape> = rects
        .iter()
        .filter(|shape| {
            (shape.rect.right() - right).abs() < 12.0
                && (shape.rect.width() - SCROLL_BAR).abs() < 0.5
        })
        .collect();
    assert!(
        strip.len() >= 2,
        "no {SCROLL_BAR} point track and handle at the right edge, found {:?}",
        rects.iter().map(|shape| shape.rect).collect::<Vec<_>>()
    );

    // The track and the handle have to be told apart, or the bar is a strip
    // of one flat colour and says nothing about where the list is.
    let colours: std::collections::HashSet<[u8; 4]> =
        strip.iter().map(|shape| shape.fill.to_array()).collect();
    assert!(
        colours.len() >= 2,
        "the handle is the same colour as the track it sits in: {colours:?}"
    );
    assert!(
        strip.iter().all(|shape| shape.fill.a() > 0),
        "the bar was painted fully transparent: {colours:?}"
    );

    // And the handle is shorter than the track, or it is not showing how much
    // of the list is on screen.
    let tallest = strip
        .iter()
        .map(|shape| shape.rect.height())
        .fold(0.0f32, f32::max);
    let shortest = strip
        .iter()
        .map(|shape| shape.rect.height())
        .fold(f32::MAX, f32::min);
    assert!(
        shortest < tallest,
        "the handle fills the whole track: {shortest} of {tallest}"
    );
}

/// A mark is what puts the rest of its set in the plan.
#[test]
fn a_set_removes_everything_but_the_marked_file() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let kept = app.sets[0].members[0].file_id;
    app.keep.insert(app.sets[0].set_id, Keep::One(kept));
    let plan = app.build_plan();

    assert_eq!(plan.files(), 1);
    let staying = app.sets[0]
        .members
        .iter()
        .find(|member| member.file_id == kept)
        .expect("the keeper is in the set");
    assert_ne!(
        plan.removals[0].rel_path, staying.rel_path,
        "the keeper is in the plan"
    );
    assert_eq!(plan.bytes(), plan.removals[0].size_bytes);
}

#[test]
fn moving_the_keep_mark_moves_what_gets_removed() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let set_id = app.sets[0].set_id;
    app.keep
        .insert(set_id, Keep::One(app.sets[0].members[0].file_id));
    let was_going = app.build_plan().removals[0].rel_path.clone();
    let moving_to = app.sets[0]
        .members
        .iter()
        .find(|member| member.rel_path == was_going)
        .expect("the plan removes a picture in the set")
        .file_id;

    // Marking the one that was going and unmarking the one that was staying
    // swaps them over, since a mark now only ever speaks for itself.
    app.selected = Some(moving_to);
    app.keep_selected();
    app.selected = Some(app.sets[0].members[0].file_id);
    app.keep_selected();

    let plan = app.build_plan();
    assert_eq!(plan.files(), 1);
    assert_ne!(
        plan.removals[0].rel_path, was_going,
        "the mark did not move"
    );
}

#[test]
fn keeping_everything_in_a_set_removes_nothing_from_it() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let members: Vec<i64> = app.sets[0]
        .members
        .iter()
        .map(|member| member.file_id)
        .collect();
    app.keep.insert(app.sets[0].set_id, Keep::Several(members));
    assert_eq!(
        app.build_plan().files(),
        0,
        "a set keeping all of it produced removals"
    );
}

/// Everywhere a label was drawn, so a test can press a button or measure a
/// row without working the layout out a second time here.
fn label_rects(shapes: &[egui::epaint::ClippedShape], label: &str) -> Vec<egui::Rect> {
    fn walk(shape: &egui::Shape, label: &str, found: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Text(text) if text.galley.text() == label => {
                found.push(text.galley.rect.translate(text.pos.to_vec2()));
            }
            egui::Shape::Vec(inner) => {
                for shape in inner {
                    walk(shape, label, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, label, &mut found);
    }
    found
}

fn label_rect(shapes: &[egui::epaint::ClippedShape], label: &str) -> Option<egui::Rect> {
    label_rects(shapes, label).into_iter().next()
}

/// Every line of text that was painted, with where it went.
/// The box a set is drawn in: the widest rectangle painted with an outline,
/// which is the group frame around the row.
fn box_around_the_set(shapes: &[egui::epaint::ClippedShape]) -> Option<egui::Rect> {
    fn walk(shape: &egui::Shape, found: &mut Vec<egui::Rect>) {
        match shape {
            egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => found.push(rect.rect),
            egui::Shape::Vec(inner) => {
                for shape in inner {
                    walk(shape, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut found);
    }
    found.into_iter().max_by(|one, other| {
        one.width()
            .partial_cmp(&other.width())
            .unwrap_or(std::cmp::Ordering::Equal)
    })
}

fn texts(shapes: &[egui::epaint::ClippedShape]) -> Vec<(String, egui::Rect)> {
    fn walk(shape: &egui::Shape, found: &mut Vec<(String, egui::Rect)>) {
        match shape {
            egui::Shape::Text(text) => found.push((
                text.galley.text().to_string(),
                text.galley.rect.translate(text.pos.to_vec2()),
            )),
            egui::Shape::Vec(inner) => {
                for shape in inner {
                    walk(shape, found);
                }
            }
            _ => {}
        }
    }
    let mut found = Vec::new();
    for clipped in shapes {
        walk(&clipped.shape, &mut found);
    }
    found
}

/// A set drawn from a really scanned folder, with one picture marked to keep
/// and one not. Only the marked one says anything, and the space is taken
/// either way, so the facts under the pictures stay on one line across the
/// set.
#[test]
fn only_the_picture_being_kept_is_labelled_and_the_others_keep_the_space() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    // Nothing is labelled until something is marked.
    app.auto_mark_to_keep();
    let members = &app.sets[0].members;
    assert_eq!(members.len(), 2, "the two copies were not found");
    let size = format!("{}x{}", members[0].width, members[0].height);
    assert_eq!(
        size,
        format!("{}x{}", members[1].width, members[1].height),
        "the fixture pictures are not the same shape"
    );
    assert!(
        app.keep.get(&app.sets[0].set_id).is_some(),
        "the search marked nothing to keep"
    );

    let root = found.path().to_path_buf();
    let ctx = window();
    let shapes = crate::shot::frame(
        "only_the_picture_being_kept_is_labelled_and_the_others_keep_the_space",
        &ctx,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(900.0, 500.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
        },
    );

    assert_eq!(
        label_rects(&shapes, "KEEP").len(),
        1,
        "the one picture being kept is not the only one saying so"
    );
    assert!(
        label_rects(&shapes, "keep this").is_empty(),
        "a picture that is not being kept was labelled anyway"
    );

    let sizes = label_rects(&shapes, &size);
    assert_eq!(
        sizes.len(),
        2,
        "both pictures should say what shape they are"
    );
    assert!(
        (sizes[0].top() - sizes[1].top()).abs() < 0.5,
        "the unmarked picture pulled its text up: {:?} against {:?}",
        sizes[0],
        sizes[1]
    );
}

/// One of something is not "1 sets". The counts above the review list say
/// what they are counting, and the word changes with the number.
#[test]
fn one_of_something_is_written_in_the_singular() {
    assert_eq!(counted(1, "set", "sets"), "1 set");
    assert_eq!(counted(1, "duplicate", "duplicates"), "1 duplicate");
    assert_eq!(counted(0, "set", "sets"), "0 sets");
    assert_eq!(counted(2, "duplicate", "duplicates"), "2 duplicates");
}

/// The box around a set is as tall as what it holds. The strip is a fixed
/// height, and any of it the tiles do not use is empty space under the file
/// names in every row of the list.
#[test]
fn a_set_box_is_not_taller_than_the_tiles_in_it() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let root = found.path().to_path_buf();

    let ctx = window();
    let mut taken = 0.0;
    let mut buttons = 0.0;
    let shapes = crate::shot::frame(
        "a_set_box_is_not_taller_than_the_tiles_in_it",
        &ctx,
        egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(900.0, 500.0),
            )),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                buttons = button_row_height(ui);
                let before = ui.next_widget_position().y;
                app.set_row(ui, 0, &root, ui.available_width());
                taken = ui.next_widget_position().y - before;
            });
        },
    );

    fn lowest(shape: &egui::Shape, so_far: &mut f32) {
        match shape {
            egui::Shape::Text(text) => {
                *so_far = so_far.max(text.pos.y + text.galley.rect.height());
            }
            egui::Shape::Vec(inner) => {
                for shape in inner {
                    lowest(shape, so_far);
                }
            }
            _ => {}
        }
    }
    let mut bottom = 0.0_f32;
    for clipped in &shapes {
        lowest(&clipped.shape, &mut bottom);
    }
    assert!(bottom > 0.0, "the row painted no text at all");

    // What is under the last line: the strip's scroll bar, the row of
    // buttons, the padding the frame draws inside its own edge, and the
    // space to the next row. Nothing else.
    let spare = taken - bottom;
    assert!(
        spare < SCROLL_BAR + buttons + BOX_PADDING + 2.0 * BOX_EDGE + 8.0,
        "the box is {spare} points taller than the tiles in it"
    );
}

/// Nothing in the window explains itself by being hovered over. What a
/// control does is written on it, or it does not belong there.
#[test]
fn nothing_in_the_window_shows_a_tooltip() {
    // Split so this test does not find itself.
    let hover = concat!("on_hover", "_text");
    let window = include_str!("../app.rs");
    assert!(
        !window.contains(hover),
        "the window has gone back to explaining itself in tooltips"
    );
    // A label that has to cut its text puts the whole string in a tooltip of
    // egui's own making, which is why the ones here paint the text instead.
    assert!(
        !window.contains(concat!(".trunc", "ate()")),
        "a label is cutting its text, which egui explains in a tooltip"
    );
}

/// Dragging across the file names really tried, in a scanned folder. Nothing
/// is a text field: no line highlights, and the pointer never becomes a
/// text cursor.
#[test]
fn dragging_across_the_window_selects_no_text() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let root = found.path().to_path_buf();

    let ctx = window();
    install_style(&ctx);
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let frame = |app: &mut App, at: egui::Pos2, pressed: Option<bool>| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        input.events.push(egui::Event::PointerMoved(at));
        if let Some(pressed) = pressed {
            input.events.push(egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: Default::default(),
            });
        }
        ctx.run(input, |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
        })
    };

    // Both themes: the machine's is applied after the window is set up, and
    // one of the two going unchanged is what shipped a selectable review tab.
    for theme in [egui::Theme::Dark, egui::Theme::Light] {
        assert!(
            !ctx.style_of(theme).interaction.selectable_labels,
            "labels are still text to be selected in the {theme:?} theme"
        );
    }

    // Press on the first line under a picture and drag across the rest.
    let start = egui::pos2(20.0, 200.0);
    frame(&mut app, start, None);
    frame(&mut app, start, Some(true));
    for step in 1..8 {
        frame(&mut app, start + egui::vec2(step as f32 * 20.0, 8.0), None);
    }
    let output = frame(&mut app, start + egui::vec2(160.0, 8.0), Some(false));

    assert!(
        output.platform_output.cursor_icon != egui::CursorIcon::Text,
        "the pointer turned into a text cursor over the window"
    );
}

/// The pointer really held over every part of a set, in a folder whose file
/// names are far too long for a tile. Nothing pops up: not over the picture,
/// not over the name that had to be cut, not over the buttons.
#[test]
fn holding_the_pointer_over_a_set_pops_nothing_up() {
    let dir = tempfile::tempdir().expect("tempdir");
    let long = "a_file_name_far_too_long_to_fit_under_a_picture_in_a_tile";
    for name in [format!("{long}_one.png"), format!("{long}_two.png")] {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
        }))
        .save_with_format(dir.path().join(name), image::ImageFormat::Png)
        .expect("a fixture");
    }
    let mut app = reviewing(dir.path());
    let root = dir.path().to_path_buf();
    assert_eq!(app.sets.len(), 1, "the two copies were not found");

    let ctx = window();
    install_style(&ctx);
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let frame = |app: &mut App, at: Option<egui::Pos2>, time: f64| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            ..Default::default()
        };
        if let Some(pos) = at {
            input.events.push(egui::Event::PointerMoved(pos));
        }
        crate::shot::frame("holding_the_pointer_over_a_set", &ctx, input, |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
        })
    };

    // Every place a pointer can rest inside the row, one point apart, and
    // twice each: egui shows a tooltip on the frame after the pointer
    // arrives, never on the same one.
    let drawn = frame(&mut app, None, 0.0);
    let row = texts(&drawn)
        .iter()
        .map(|(_, rect)| *rect)
        .fold(egui::Rect::NOTHING, |all, rect| all.union(rect));
    let mut clock = 1.0;
    let mut y = row.top();
    while y < row.bottom() {
        let mut x = row.left();
        while x < row.right() {
            let at = egui::pos2(x, y);
            // Arrive, then wait: a tooltip is held back until the pointer has
            // been still for a moment.
            frame(&mut app, Some(at), clock);
            clock += 2.0;
            let painted = frame(&mut app, Some(at), clock);
            clock += 2.0;
            // One line per tile and no more. A tooltip would be a third
            // drawing of the name, over the top of the two.
            let names = texts(&painted)
                .into_iter()
                .filter(|(text, _)| text.contains(long))
                .count();
            assert_eq!(
                names, 2,
                "the pointer at {at:?} left {names} copies of the name on screen"
            );
            x += 12.0;
        }
        y += 12.0;
    }
}

/// A set wider than the window, walked along with the cursor keys. The strip
/// follows the selection: the picture the preview is showing is on screen,
/// whichever end of the set it is at.
#[test]
fn walking_along_a_long_set_brings_the_selected_picture_into_view() {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.view = View::Review;
    app.sets = vec![DuplicateSet {
        set_id: 7,
        members: (0..24).map(|index| member(index, "a.jpg", 500)).collect(),
    }];
    app.selected = Some(0);
    let root = PathBuf::from(".");
    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let bar = egui::Id::new(("set bar", 7));

    // What the strip is asked to do on the frame after a key press. It is
    // taken up by the scrolling itself on the frame after that, so it is read
    // straight away or not at all.
    let frame = |app: &mut App| {
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            },
        );
        ctx.data(|data| data.get_temp::<f32>(bar))
    };

    frame(&mut app);
    // Walked along the set a picture at a time, the way the right cursor key
    // walks it.
    let mut asked: Vec<Option<f32>> = Vec::new();
    for _ in 0..23 {
        app.walk(&[0], Direction::Forward);
        asked.push(frame(&mut app));
    }
    assert_eq!(
        app.selected,
        Some(23),
        "the walk did not reach the end of the set"
    );

    assert!(
        asked[..2].iter().all(Option::is_none),
        "the strip moved for pictures that were already on it: {:?}",
        &asked[..2]
    );
    let moved: Vec<f32> = asked.iter().flatten().copied().collect();
    assert!(
        !moved.is_empty(),
        "the strip never moved for a picture off the end of it"
    );
    assert!(
        moved.windows(2).all(|pair| pair[1] > pair[0]),
        "the strip did not follow the selection along: {moved:?}"
    );

    // And back to the near end, one picture at a time.
    for _ in 0..23 {
        app.walk(&[0], Direction::Back);
        frame(&mut app);
    }
    assert_eq!(app.selected, Some(0));
    assert_eq!(
        frame(&mut app),
        None,
        "the first picture was on screen and the strip was asked to move anyway"
    );
}

/// Two of the buttons change what they say. Each is drawn to the widest
/// thing it can ever say, so the word changes and nothing moves: a button
/// sized to its current word takes every button right of it along when that
/// word gets shorter.
#[test]
fn a_button_that_changes_its_word_does_not_move_the_ones_beside_it() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let root = found.path().to_path_buf();
    let set_id = app.sets[0].set_id;

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let draw = |app: &mut App| {
        crate::shot::frame(
            "a_button_that_changes_its_word_does_not_move_the_ones_beside_it",
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            },
        )
    };
    // Every button as it is drawn, by the rounded rectangle under its word
    // rather than by the word, which is centred in it and moves when the
    // word changes length even though the button has not.
    let buttons = |drawn: &[egui::epaint::ClippedShape]| -> Vec<egui::Rect> {
        let mut found: Vec<egui::Rect> = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.rect.height() > 8.0
                        && rect.rect.height() < 40.0
                        && rect.rect.width() < 120.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        found.sort_by(|a, b| a.left().total_cmp(&b.left()));
        found
    };

    let before = buttons(&draw(&mut app));
    assert_eq!(before.len(), 3, "three buttons were not drawn: {before:?}");

    // "ignore" becomes "ignored", which is a character longer.
    app.ignore_set(set_id);
    draw(&mut app);
    let after = buttons(&draw(&mut app));

    assert_eq!(after.len(), 3, "three buttons were not drawn: {after:?}");
    for (was, now) in before.iter().zip(&after) {
        assert!(
            (was.left() - now.left()).abs() < 0.51 && (was.width() - now.width()).abs() < 0.51,
            "a button was {was:?} and became {now:?} when one of them changed its word"
        );
    }
}

/// The three buttons sit in a row along the bottom of a set, in order and
/// with space between them, under the pictures rather than over them. The
/// first two decide what the set keeps and the third says it is not a set of
/// copies at all.
#[test]
fn a_set_has_its_buttons_in_a_row_along_the_bottom() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let root = found.path().to_path_buf();

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let drawn = crate::shot::frame(
        "a_set_has_its_buttons_in_a_row_along_the_bottom",
        &ctx,
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
        },
    );

    let painted = texts(&drawn);
    let where_it_is = |label: &str| {
        painted
            .iter()
            .find(|(text, _)| text == label)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("no {label} button was drawn"))
    };
    let all = where_it_is("keep all");
    let none = where_it_is("keep none");
    let ignore = where_it_is("ignore");

    // In that order, left to right, with room between them.
    assert!(
        all.right() < none.left(),
        "keep none is not right of keep all"
    );
    assert!(
        none.right() < ignore.left(),
        "ignore is not right of keep none"
    );
    assert!(
        none.left() - all.right() > 4.0,
        "the buttons are not spaced apart"
    );
    // The same space after each of them. Measured between the buttons rather
    // than between their words: a button is as wide as the widest thing it
    // can say, and the word it is saying now sits centred in that.
    let boxes = {
        let mut found: Vec<egui::Rect> = drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if rect.rect.height() > 8.0
                        && rect.rect.height() < 40.0
                        && rect.rect.width() < 120.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .collect();
        found.sort_by(|a, b| a.left().total_cmp(&b.left()));
        found
    };
    assert_eq!(boxes.len(), 3, "three buttons were not drawn: {boxes:?}");
    // Inside the box, not against it. A button drawn hard against the box's
    // edge has the line round it drawn half outside, and the clip along that
    // edge takes that half away, so the button looks cut off down its side.
    let box_rect = drawn
        .iter()
        .find_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect)
                if rect.stroke.width > 0.0
                    && rect.rect.width() > 200.0
                    && rect.rect.height() > 100.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .expect("the set was not drawn in a box");
    assert!(
        boxes[0].left() > box_rect.left() + 2.0,
        "the first button starts at {} inside a box that starts at {}",
        boxes[0].left(),
        box_rect.left()
    );
    let first = boxes[1].left() - boxes[0].right();
    let second = boxes[2].left() - boxes[1].right();
    assert!(
        (first - second).abs() < 2.0,
        "the gaps between the buttons are {first} and {second}"
    );
    assert!(
        (all.center().y - ignore.center().y).abs() < 2.0,
        "the buttons are not on one row"
    );

    // Under the pictures: below the lowest line of text in the tiles.
    let lowest = painted
        .iter()
        .filter(|(text, _)| !["keep all", "keep none", "ignore"].contains(&text.as_str()))
        .map(|(_, rect)| rect.bottom())
        .fold(0.0_f32, f32::max);
    assert!(
        all.top() >= lowest,
        "the buttons are over the pictures rather than under them"
    );
}

/// A set row is as tall as what it holds, and as wide as the room it is
/// given without running under the list's scroll bar. The box around a set
/// is drawn by a frame that adds its own margin, so a row built to the whole
/// of the available width comes out that much wider than the room for it.
#[test]
fn a_set_row_fits_the_room_it_is_given() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let root = found.path().to_path_buf();

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let mut bar_at = 0.0_f32;
    let mut list_at = 0.0_f32;
    let mut buttons = 0.0_f32;
    let drawn = crate::shot::frame(
        "a_set_row_fits_the_room_it_is_given",
        &ctx,
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                buttons = button_row_height(ui);
                // As the list gives it: the page's margin down the left, and
                // the width less the scroll bar it paints down the right and
                // the gap kept before that bar.
                let whole = ui.available_rect_before_wrap();
                let room = egui::Rect::from_min_max(
                    egui::pos2(whole.left() + PAGE_MARGIN, whole.top()),
                    whole.max,
                );
                let inside = room.with_max_x(room.right() - SCROLL_BAR - PAGE_MARGIN);
                list_at = whole.left();
                bar_at = room.right() - SCROLL_BAR;
                let wide = inside.width();
                ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inside), |ui| {
                    app.set_row(ui, 0, &root, wide)
                });
            });
        },
    );

    let outline = box_around_the_set(&drawn).expect("no box was drawn around the set");
    let painted = texts(&drawn);
    let last = painted
        .iter()
        .filter(|(text, _)| !text.starts_with("keep"))
        .map(|(_, rect)| rect.bottom())
        .fold(0.0_f32, f32::max);

    // The gap under the last line is the strip's own scroll bar, the row of
    // buttons, and the margin the frame draws with, and nothing else.
    let under = outline.bottom() - last;
    assert!(
        under < SCROLL_BAR + buttons + 14.0,
        "the box goes {under} past the last line under the pictures"
    );
    // The left of the box sits a margin in from where the list begins, and
    // its right leaves the same margin before the list's scroll bar.
    let left = outline.left() - list_at;
    let right = bar_at - outline.right();
    assert!(
        (left - right).abs() < 3.0,
        "the box is {left} from the left edge and {right} from the scroll bar"
    );
}

/// Two clicks on a picture in a really scanned folder do what the space bar
/// does on it: keep that one, and on a second pair of clicks let it go again.
#[test]
fn two_clicks_on_a_picture_keep_it_the_way_the_space_bar_does() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let root = found.path().to_path_buf();
    let set_id = app.sets[0].set_id;
    let other = app.sets[0]
        .members
        .iter()
        .map(|member| member.file_id)
        .find(|file_id| app.keep.get(&set_id) != Some(&Keep::One(*file_id)))
        .expect("both pictures are the keeper");

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    // The clock has to move between the two pairs, or four clicks that close
    // together are one gesture rather than two.
    let frame = |app: &mut App, at: Option<egui::Pos2>, clicks: usize, time: f64| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            ..Default::default()
        };
        if let Some(pos) = at {
            input.events.push(egui::Event::PointerMoved(pos));
            for _ in 0..clicks {
                for pressed in [true, false] {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
        }
        crate::shot::frame("two_clicks_on_a_picture", &ctx, input, |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
        })
    };

    // The pictures are the tile sized rectangles, and the one wanted here is
    // the one that is not already the keeper: the tiles are drawn in the
    // order the set holds them.
    let drawn = frame(&mut app, None, 0, 0.0);
    let mut pictures: Vec<egui::Rect> = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            // Not the ring around the picture being looked at: that is drawn
            // inside the tile now, so it is close to the tile's own size.
            egui::Shape::Rect(rect)
                if rect.stroke.color != selection_colour()
                    && (rect.rect.width() - TILE.x).abs() < 6.0
                    && (rect.rect.height() - TILE.y).abs() < 6.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect();
    pictures.sort_by(|a, b| a.left().total_cmp(&b.left()));
    assert_eq!(
        pictures.len(),
        2,
        "the two pictures were not drawn: {pictures:?}"
    );
    let index = app.sets[0]
        .members
        .iter()
        .position(|member| member.file_id == other)
        .expect("the set lost a picture");
    let at = pictures[index].center();

    frame(&mut app, Some(at), 0, 0.1);
    frame(&mut app, Some(at), 2, 0.2);
    assert_eq!(
        app.keep.get(&set_id),
        Some(&Keep::One(other)),
        "two clicks did not keep the picture they were on"
    );

    frame(&mut app, Some(at), 0, 2.0);
    frame(&mut app, Some(at), 2, 2.1);
    assert_eq!(
        app.keep.get(&set_id),
        None,
        "twice more did not let it go again"
    );
}

/// A cleanup removes files the window chose itself, so afterwards the folder
/// is what it was less that list, and the pictures held in memory say so
/// without the folder being read or the index converted again. Searching
/// again finds what is left, and no pass runs.
#[test]
fn what_a_cleanup_took_is_out_of_the_pictures_held_in_memory() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    assert_eq!(app.images.as_ref().expect("pictures").len(), 3);
    app.auto_mark_to_keep();
    app.keep_index = true;
    app.destination = Destination::Delete;
    let plan = app.plan.clone();
    let going = plan.removals[0].rel_path.clone();

    app.view = View::Cleanup;
    app.run_cleanup(&plan);
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.removing.is_some() && std::time::Instant::now() < until {
        app.pump_cleanup(&ctx);
    }

    let left = app.images.as_ref().expect("the pictures were thrown away");
    assert_eq!(
        left.len(),
        2,
        "the picture the cleanup took is still in memory"
    );

    // And searching again runs on those, with no pass over the folder.
    app.load_sets();
    settle(&mut app);
    assert!(
        app.running.is_none(),
        "the folder was read again after a cleanup"
    );
    assert!(app.sets.is_empty(), "the copies survived the cleanup");
    let _ = going;
}

/// "unmark all" takes every mark off, so somebody can choose again from
/// nothing. A set nobody calls a set of copies keeps what it was keeping,
/// which is what taking it back gives back.
#[test]
fn unmarking_all_leaves_nothing_marked_but_what_an_ignored_set_was_keeping() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    let (first, second) = (app.sets[0].set_id, app.sets[1].set_id);
    app.selected = Some(app.sets[1].members[0].file_id);
    app.keep_selected();
    app.ignore_set(second);
    app.keep_everything();
    assert!(
        app.keep.contains_key(&first),
        "the set that is a set of copies marks nothing"
    );

    app.unmark_everything();

    assert!(
        app.keep.get(&first).is_none(),
        "a mark survived unmarking everything"
    );
    assert!(
        app.keep.get(&second).is_some(),
        "an ignored set lost what it was keeping before it was ignored"
    );
    assert_eq!(
        app.plan.files(),
        2,
        "what a cleanup would take did not follow the marks"
    );
}

/// "keep everything" marks every picture in every set that is a set of
/// copies, so a cleanup takes nothing. A set nobody calls a set of copies is
/// left alone, the way it is everywhere else.
#[test]
fn keeping_everything_marks_every_picture_that_is_in_a_set_of_copies() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 2, "the two sets were not found");
    let ignored = app.sets[1].set_id;
    app.ignore_set(ignored);
    // One picture already marked, to show that marking the rest disturbs it.
    app.selected = Some(app.sets[0].members[0].file_id);
    app.keep_selected();

    app.keep_everything();

    let marked = app
        .keep
        .get(&app.sets[0].set_id)
        .expect("the set marks nothing")
        .marked();
    let all: Vec<i64> = app.sets[0]
        .members
        .iter()
        .map(|member| member.file_id)
        .collect();
    assert_eq!(marked, all, "not every picture in the set was marked");
    assert!(
        app.keep.get(&ignored).is_none(),
        "a set that is not a set of copies was marked"
    );
    assert_eq!(app.plan.files(), 0, "a cleanup would still take something");
}

/// The toolbar over the review holds three things in one row, and each is in
/// its own place: the marking button against the left edge, the counts in
/// the middle of the window, and the cleanup button against the right edge.
#[test]
fn the_review_toolbar_holds_marking_left_the_counts_centred_and_cleanup_right() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    // Marked, so the counts on the toolbar are counting something.
    app.auto_mark_to_keep();

    let ctx = window();
    // Wide enough for the row: two buttons at the left, the counts in the
    // middle and the cleanup button at the right come to more than a narrow
    // window has.
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 500.0));
    let shapes = crate::shot::frame(
        "the_review_toolbar_holds_the_box_left_the_counts_centred_and_the_button_right",
        &ctx,
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
        },
    );

    // The toolbar is the top of the window, so what is drawn below it is the
    // review itself and no part of this.
    let painted: Vec<(String, egui::Rect)> = texts(&shapes)
        .into_iter()
        .filter(|(_, rect)| rect.top() < 40.0)
        .collect();
    let one = |wanted: &str| {
        painted
            .iter()
            .find(|(text, _)| text == wanted)
            .map(|(_, rect)| *rect)
            .unwrap_or_else(|| panic!("{wanted} was not drawn in the toolbar: {painted:?}"))
    };
    let unmark = one("unmark all");
    let everything = one("mark all");
    let marking = one("auto-mark to keep");
    let button = one("Clean up");
    let counts = painted
        .iter()
        .filter(|(text, _)| {
            text != "unmark all"
                && text != "mark all"
                && text != "auto-mark to keep"
                && text != "Clean up"
        })
        .map(|(_, rect)| *rect)
        .reduce(|all, rect| all.union(rect))
        .expect("no counts were drawn");

    assert!(
        unmark.left() < 60.0,
        "the first button is not against the left edge: {unmark:?}"
    );
    assert!(
        everything.left() > unmark.right(),
        "marking all is not to the right of unmarking all"
    );
    assert!(
        marking.left() > everything.right(),
        "auto-marking is not to the right of marking all"
    );
    assert!(
        button.right() > screen.right() - 60.0,
        "the button is not against the right edge: {button:?}"
    );
    // In the middle of what is left between the buttons. Centred on the
    // window instead, they sit over the buttons as soon as there are enough
    // of them, which there now are.
    let middle_of_the_gap = (marking.right() + button.left()) / 2.0;
    assert!(
        (counts.center().x - middle_of_the_gap).abs() < 12.0,
        "the counts are centred on {} and the space between the buttons on {middle_of_the_gap}",
        counts.center().x,
    );
    assert!(
        counts.left() > marking.right() && counts.right() < button.left(),
        "the counts run into the checkbox or the button: {counts:?}"
    );
}

/// What the file says about itself is under the picture in the preview: the
/// name of each thing on the left and what it says on the right. It is read
/// off the file on another thread, so it arrives a frame or two after the
/// picture is clicked rather than with it.
#[test]
fn the_preview_shows_what_the_file_says_about_itself() {
    let found = folder_with_a_duplicate();
    // A comment written into one of them, the way a PNG carries text: the
    // name, a zero, and the words.
    let first = std::fs::read_dir(found.path())
        .expect("folder")
        .filter_map(|entry| entry.ok().map(|entry| entry.path()))
        .find(|path| path.extension().is_some_and(|ext| ext == "png"))
        .expect("a picture");
    let mut bytes = std::fs::read(&first).expect("read");
    let mut chunk = b"tEXtNotes\0taken in a garden".to_vec();
    let length = (chunk.len() - 4) as u32;
    let sum = png_check(&chunk);
    let mut piece = length.to_be_bytes().to_vec();
    piece.append(&mut chunk);
    piece.extend_from_slice(&sum.to_be_bytes());
    let end = bytes.len() - 12;
    bytes.splice(end..end, piece);
    std::fs::write(&first, &bytes).expect("write");

    let mut app = reviewing(found.path());
    app.selected = app.sets[0]
        .members
        .iter()
        .find(|member| first.ends_with(&member.rel_path))
        .map(|member| member.file_id);

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1100.0, 700.0));
    let waited = std::time::Instant::now();
    loop {
        let shapes = crate::shot::frame(
            "the_preview_shows_what_the_file_says_about_itself",
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
            },
        );
        let painted = texts(&shapes);
        let said = |wanted: &str| painted.iter().any(|(text, _)| text == wanted);
        if said("Notes") && said("taken in a garden") {
            break;
        }
        assert!(
            waited.elapsed().as_secs() < 20,
            "the preview never showed what the file says: {:?}",
            painted
                .iter()
                .map(|(text, _)| text.as_str())
                .collect::<Vec<&str>>()
        );
        std::thread::yield_now();
    }
}

/// A click on the preview fills the window with the picture, and the escape
/// key puts it back. The picture is asked for at the size of the window
/// rather than at the size of the pane it came from.
#[test]
fn a_click_on_the_preview_fills_the_window_and_escape_puts_it_back() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let first = app.sets[0].members[0].file_id;
    app.selected = Some(first);

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1100.0, 700.0));
    let mut clock = 0.0;
    let mut frame = |app: &mut App, events: Vec<egui::Event>| {
        clock += 0.1;
        let _ = ctx.run(
            egui::RawInput {
                screen_rect: Some(screen),
                time: Some(clock),
                events,
                ..Default::default()
            },
            |ctx| {
                // What the workers have read becomes a texture on the frame
                // that draws it, which is the window's own job.
                app.thumbs.collect(ctx);
                egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
                app.filling_the_window(ctx);
            },
        );
    };

    // The picture in the pane, once it has been read.
    let waited = std::time::Instant::now();
    while app.showing.is_none() {
        frame(&mut app, Vec::new());
        assert!(waited.elapsed().as_secs() < 20, "the preview never arrived");
        std::thread::yield_now();
    }
    assert_eq!(
        app.filling_the_window, None,
        "it started out filling the window"
    );

    // A click in the middle of the pane, which is where the picture is.
    let at = egui::pos2(screen.right() - 230.0, 220.0);
    frame(&mut app, vec![egui::Event::PointerMoved(at)]);
    frame(
        &mut app,
        vec![
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ],
    );
    assert_eq!(
        app.filling_the_window,
        Some(first),
        "a click on the preview did not fill the window with it"
    );

    frame(
        &mut app,
        vec![egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }],
    );
    assert_eq!(
        app.filling_the_window, None,
        "escape did not put the picture back"
    );
}

/// The check every PNG chunk carries, for the fixture above.
fn png_check(bytes: &[u8]) -> u32 {
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

/// The two ways of matching are on the page, in the box that says what
/// counts as a duplicate, and clicking one turns it off.
#[test]
fn the_ways_of_matching_are_boxes_on_the_page_that_can_be_clicked() {
    let ctx = egui::Context::default();
    let mut app = App::from_settings(crate::settings::Settings::default());
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let frame = |app: &mut App, at: Option<egui::Pos2>, pressed: Option<bool>| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        if let Some(pos) = at {
            input.events.push(egui::Event::PointerMoved(pos));
            if let Some(pressed) = pressed {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
        }
        crate::shot::frame("the_ways_of_matching_are_boxes", &ctx, input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.matching_section(ui, 600.0);
            });
        })
    };

    let drawn = frame(&mut app, None, None);
    let whole = label_rect(&drawn, "Match whole pictures")
        .expect("the box for matching whole pictures was not drawn");
    let crops =
        label_rect(&drawn, "Match partials").expect("the box for matching partials was not drawn");
    let colour = label_rect(&drawn, "Match colour with grayscale")
        .expect("the box for matching colour with grayscale was not drawn");
    // The ways of matching first, then the colour box, which changes how the
    // first of them decides rather than being a way of its own.
    assert!(
        whole.top() < crops.top() && crops.top() < colour.top(),
        "the boxes are in the wrong order: {whole:?}, {crops:?}, {colour:?}"
    );

    // The tick itself sits to the left of the words.
    let at = egui::pos2(crops.left() - 8.0, crops.center().y);
    frame(&mut app, Some(at), Some(true));
    frame(&mut app, Some(at), Some(false));
    assert!(
        !app.match_corners,
        "clicking the box did not switch matching crops off"
    );
    assert!(
        app.match_whole_frame,
        "clicking one box switched the other off"
    );
}

/// Click where the index box is drawn, and say whether it went on.
fn ticks_the_index_box(app: &mut App, ctx: &egui::Context, screen: egui::Rect) -> bool {
    let drawn = crate::shot::frame(
        "ticks_the_index_box",
        ctx,
        egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        },
        |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.folder_section(ui, 700.0);
            });
        },
    );
    let Some(label) = label_rect(&drawn, "Save an index database for this folder") else {
        return false;
    };
    let at = egui::pos2(label.left() - 8.0, label.center().y);
    for pressed in [true, false] {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        input.events.push(egui::Event::PointerMoved(at));
        input.events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.folder_section(ui, 700.0);
            });
        });
    }
    app.keep_index
}

/// Two of the boxes are about the box above them, and mean nothing without
/// it. Both are off and out of reach while what they depend on is off, and
/// keeping an index asks for a rescan on opening by itself.
#[test]
fn a_box_that_depends_on_another_is_off_and_out_of_reach_without_it() {
    let ctx = window();
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let draw = |app: &mut App| {
        crate::shot::frame(
            "a_box_that_depends_on_another",
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    app.folder_section(ui, 700.0);
                });
            },
        )
    };

    // All three are ticked, and all three lose what they depend on.
    app.recurse = true;
    app.within_a_folder = true;
    app.keep_index = true;
    app.auto_rescan = true;
    app.auto_mark = true;
    app.recurse = false;
    app.keep_index = false;
    app.settle_the_boxes();
    assert!(
        !app.within_a_folder,
        "matching within folders survived the subfolders going"
    );
    assert!(
        !app.auto_rescan,
        "running on opening survived the index going"
    );
    assert!(
        !app.auto_mark,
        "marking on opening survived the rescan going"
    );

    // And with what they depend on off, they are drawn but cannot be
    // reached: clicking where they are changes nothing.
    let drawn = draw(&mut app);
    let apart = label_rect(&drawn, "Only match within folders").expect("no box for that");
    let opening =
        label_rect(&drawn, "Automatically rescan when opening this index").expect("no box");
    let marking = label_rect(&drawn, "Automatically mark to keep").expect("no box");
    for box_at in [apart, opening, marking] {
        let at = egui::pos2(box_at.left() - 8.0, box_at.center().y);
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        input.events.push(egui::Event::PointerMoved(at));
        input.events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: Default::default(),
        });
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.folder_section(ui, 700.0);
            });
        });
    }
    assert!(!app.within_a_folder, "a box nobody can reach was ticked");
    assert!(!app.auto_rescan, "a box nobody can reach was ticked");
    assert!(!app.auto_mark, "a box nobody can reach was ticked");

    // And saying yes to keeping an index says yes to rescanning on opening,
    // which is what the box under it is for.
    assert!(
        ticks_the_index_box(&mut app, &ctx, screen),
        "the index box was not ticked"
    );
    assert!(
        app.auto_rescan,
        "ticking the index box did not ask for a rescan"
    );

    // With that on, the box under it can be reached and ticked.
    let drawn = draw(&mut app);
    let marking = label_rect(&drawn, "Automatically mark to keep").expect("no box");
    let at = egui::pos2(marking.left() - 8.0, marking.center().y);
    for pressed in [true, false] {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        input.events.push(egui::Event::PointerMoved(at));
        input.events.push(egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        });
        let _ = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.folder_section(ui, 700.0);
            });
        });
    }
    assert!(
        app.auto_mark,
        "the box could not be ticked with a rescan asked for"
    );
}

/// The box is a fact about the folder: the index keeps it, and opening that
/// folder again comes back with it.
#[test]
fn the_index_keeps_whether_marking_on_opening_was_ticked() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    assert!(
        !app.auto_mark,
        "marking on opening was on before anything asked for it"
    );

    app.keep_index = true;
    app.auto_rescan = true;
    app.auto_mark = true;
    app.remember_ways_of_matching();

    closed(&app);
    let again = reviewing(found.path());
    assert!(
        again.auto_mark,
        "the folder was opened again without the box ticked"
    );
}

/// With the box on, a pass leaves every set marking its best copy, so a
/// folder opens with the obvious answers already filled in.
#[test]
fn a_pass_with_marking_on_leaves_every_set_marked() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert!(
        app.keep.is_empty(),
        "the search marked something on its own"
    );
    app.auto_mark = true;

    app.start_scan();
    settle(&mut app);

    assert_eq!(app.sets.len(), 2, "the two pairs were not found again");
    for set in &app.sets {
        let best = set
            .members
            .iter()
            .find(|member| member.auto_keep)
            .expect("a best copy");
        let keeping = app.keep.get(&set.set_id).expect("a set came back unmarked");
        assert!(
            keeping.keeps(best.file_id),
            "the best copy of a set was not marked"
        );
    }
}

/// Both ways of matching are on to begin with, and switching one off is a
/// fact about the folder: the index keeps it and opening that folder again
/// comes back with it.
#[test]
fn the_index_keeps_which_ways_of_matching_were_ticked() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    assert!(
        app.match_whole_frame,
        "whole pictures were not being matched to begin with"
    );
    assert!(
        app.match_corners,
        "crops were not being matched to begin with"
    );

    // Unticking it writes nothing on its own: until a search has run that
    // way, the folder has not been searched that way.
    app.match_corners = false;
    assert_eq!(
        crate::notes::read(&app.index).match_corners,
        Some(true),
        "unticking the box wrote it down before it had been used"
    );

    // Searching is using it, and that is what the index takes.
    app.load_sets();
    settle(&mut app);

    closed(&app);
    let again = reviewing(found.path());
    assert!(
        again.match_whole_frame,
        "whole pictures came back switched off"
    );
    assert!(
        !again.match_corners,
        "the folder was opened again still matching crops"
    );
}

/// The whole review page: the list keeps to its own side of the window,
/// whatever the preview pane beside it is dragged to. Nothing in it is
/// painted over the pane, and no set runs under the scroll bar.
#[test]
fn the_review_list_keeps_to_its_own_side_of_the_window() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    app.selected = app.sets[0].members.first().map(|member| member.file_id);

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
    // The window's own panels, in the order the window puts them: the tabs
    // across the top, then the page with no margin of its own, which is what
    // lets the toolbar's line run the width of the window.
    let draw = |app: &mut App| {
        crate::shot::frame(
            "the_review_list_keeps_to_its_own_side_of_the_window",
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("1  Scan");
                    });
                    ui.add_space(6.0);
                });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::ZERO),
                    )
                    .show(ctx, |ui| app.review_view(ui));
            },
        )
    };
    // The panel settles on its width over a frame or two.
    draw(&mut app);
    draw(&mut app);
    let drawn = draw(&mut app);

    let pane = app.preview_width.expect("the preview pane was not drawn");
    // The first set starts below the toolbar rather than against it, and the
    // list's scroll bar starts level with it rather than above or below it.
    let bar = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect)
                if (rect.rect.width() - SCROLL_BAR).abs() < 1.0 && rect.rect.height() > 100.0 =>
            {
                Some(rect.rect.top())
            }
            _ => None,
        })
        .fold(f32::MAX, f32::min);
    // The first set starts below the toolbar rather than against it.
    let toolbar = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.rect.width() > screen.width() - 1.0 => {
                Some(rect.rect.bottom())
            }
            _ => None,
        })
        .filter(|bottom| *bottom < screen.height() / 2.0)
        .fold(0.0_f32, f32::max);
    let first = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect)
                if rect.stroke.width > 0.0
                    && rect.rect.width() > 200.0
                    && rect.rect.height() > 100.0 =>
            {
                Some(rect.rect.top())
            }
            _ => None,
        })
        .fold(f32::MAX, f32::min);
    assert!(
        first - toolbar >= SECTION_GAP - 1.0,
        "the first set starts {} under the toolbar",
        first - toolbar
    );
    // The bar starts at the line under the toolbar, not at the first set:
    // the list runs from that line down, and the gap above the first set is
    // inside the list.
    assert!(
        (bar - toolbar).abs() < 1.01,
        "the scroll bar starts at {bar} and the line under the toolbar is at {toolbar}"
    );
    let outlined: Vec<egui::Rect> = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.stroke.width > 0.0 => Some(rect.rect),
            _ => None,
        })
        .collect();
    let boxes: Vec<&egui::Rect> = outlined
        .iter()
        .filter(|rect| rect.width() > 200.0 && rect.height() > 100.0)
        .collect();
    assert!(!boxes.is_empty(), "no set was drawn");
    let pane_starts = screen.right() - pane;
    for set in boxes {
        assert!(
            set.right() < pane_starts,
            "a set reaches {} and the preview pane starts at {pane_starts}",
            set.right()
        );
    }

    // And whatever the list draws is cut off at the list's own edge rather
    // than painted over the pane beside it. The page behind everything is
    // clipped to the whole window, which is not the list.
    for clipped in &drawn {
        let clip = clipped.clip_rect;
        let of_the_list = clip != screen && clip.left() < pane_starts && clip.top() > 56.0;
        if of_the_list {
            assert!(
                clip.right() <= pane_starts + 0.5,
                "something in the list is clipped to {clip:?}, which reaches over the pane"
            );
        }
    }
}

/// A set's own scroll bar runs the width of the box, edge to edge inside the
/// line round it, and the band of buttons under it has a line of its own
/// along the top.
#[test]
fn a_sets_bar_runs_the_width_of_the_box_and_the_band_has_a_line_on_it() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    // More pictures than fit across the box, so the strip has a bar at all.
    app.sets[0].members = (100..124).map(|id| member(id, "a.jpg", 500)).collect();

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
    let draw = |app: &mut App| {
        crate::shot::frame(
            "a_sets_bar_runs_the_width_of_the_box_and_the_band_has_a_line_on_it",
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::ZERO),
                    )
                    .show(ctx, |ui| app.review_view(ui));
            },
        )
    };
    draw(&mut app);
    draw(&mut app);
    let drawn = draw(&mut app);

    // The first box: the topmost outlined rectangle as wide as a set.
    let (set, stroke) = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect)
                if rect.stroke.width > 0.0
                    && rect.rect.width() > 200.0
                    && rect.rect.height() > 100.0 =>
            {
                Some((rect.rect, rect.stroke.width))
            }
            _ => None,
        })
        .fold(
            (egui::Rect::NOTHING, 0.0),
            |first: (egui::Rect, f32), it| {
                if it.0.top() < first.0.top() {
                    it
                } else {
                    first
                }
            },
        );
    assert!(set.is_finite(), "no set was drawn");
    let inside = set.shrink(stroke / 2.0);

    // The strip's own bar: as tall as a bar, lying down, inside this box.
    let bar = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect)
                if (rect.rect.height() - SCROLL_BAR).abs() < 1.0
                    && rect.rect.width() > 100.0
                    && inside.contains(rect.rect.center()) =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .fold(egui::Rect::NOTHING, |widest, rect| {
            if rect.width() > widest.width() {
                rect
            } else {
                widest
            }
        });
    assert!(bar.is_finite(), "the set's own scroll bar was not drawn");
    assert!(
        (bar.left() - inside.left()).abs() < 0.51 && (bar.right() - inside.right()).abs() < 0.51,
        "the bar runs {:?} inside a box that runs {:?}",
        bar.x_range(),
        inside.x_range()
    );

    // And the line along the top of the band under it.
    let edge = ctx.style().visuals.widgets.noninteractive.bg_stroke.color;
    let lines: Vec<(egui::Pos2, egui::Pos2)> = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::LineSegment { points, stroke }
                if stroke.color == egui::epaint::ColorMode::Solid(edge) =>
            {
                Some((points[0], points[1]))
            }
            _ => false.then_some((egui::Pos2::ZERO, egui::Pos2::ZERO)),
        })
        .collect();
    // Corner to corner of the box, which is the rectangle the line round it
    // was given rather than that rectangle less half of the line.
    let band_line = lines.iter().any(|(from, to)| {
        (to.x - from.x - set.width()).abs() < 0.51
            && (from.y - bar.bottom()).abs() < 0.51
            && from.y < inside.bottom()
    });
    assert!(
        band_line,
        "the band of buttons has no line along the top of it: lines {lines:?}, \
         box {inside:?}, bar bottom {}",
        bar.bottom()
    );
}

/// The bar beside the list marks the room the list is drawn in, so it is in
/// the same place whatever the list is scrolled to. Only the handle inside it
/// moves.
#[test]
fn the_scroll_bar_beside_the_list_stays_where_it_is_when_the_list_moves() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
    let draw = |app: &mut App| {
        crate::shot::frame(
            "the_scroll_bar_beside_the_list_stays_where_it_is_when_the_list_moves",
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::ZERO),
                    )
                    .show(ctx, |ui| app.review_view(ui));
            },
        )
    };
    // The track: as wide as a bar and as tall as the list. The handle inside
    // it is the same width and shorter, so the tallest of them is the track.
    let track = |drawn: &[egui::epaint::ClippedShape]| -> egui::Rect {
        drawn
            .iter()
            .filter_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect)
                    if (rect.rect.width() - SCROLL_BAR).abs() < 1.0
                        && rect.rect.height() > 100.0 =>
                {
                    Some(rect.rect)
                }
                _ => None,
            })
            .fold(egui::Rect::NOTHING, |tallest, rect| {
                if rect.height() > tallest.height() {
                    rect
                } else {
                    tallest
                }
            })
    };

    draw(&mut app);
    let before = track(&draw(&mut app));
    assert!(before.is_finite(), "the list's scroll bar was not drawn");
    let was = app.list_offset;

    // Down to the second set, the way the cursor keys take it there.
    app.scroll_to = Some(1);
    draw(&mut app);
    let after = track(&draw(&mut app));
    assert!(
        app.list_offset > was,
        "the list did not move, so this measures nothing"
    );
    assert_eq!(before, after, "the bar moved with the list it is beside");
}

/// Every set is drawn whole: the line round it is inside what the list is
/// allowed to paint in, top and bottom, so no box is sliced by the edge of
/// the list. One box stands as far from the next as `BETWEEN_BOXES`, the gap
/// from the window's edge to a box's left edge is the gap from its right
/// edge to the scroll bar, and what is inside a box keeps the same room on
/// either side.
#[test]
fn the_set_boxes_are_drawn_whole_and_evenly_spaced() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert!(
        app.sets.len() >= 2,
        "two sets were needed and {} were found",
        app.sets.len()
    );

    let ctx = window();
    // Short enough that the list has more in it than fits, so the bar beside
    // it is drawn and can be measured against the boxes.
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
    let draw = |app: &mut App| {
        crate::shot::frame(
            "the_set_boxes_are_drawn_whole_and_evenly_spaced",
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label("1  Scan");
                    });
                    ui.add_space(6.0);
                });
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::ZERO),
                    )
                    .show(ctx, |ui| app.review_view(ui));
            },
        )
    };
    draw(&mut app);
    draw(&mut app);
    let drawn = draw(&mut app);

    // The boxes: the only outlined rectangles as wide as a set and as tall.
    let mut boxes: Vec<(egui::Rect, egui::Rect, f32)> = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect)
                if rect.stroke.width > 0.0
                    && rect.rect.width() > 200.0
                    && rect.rect.height() > 100.0 =>
            {
                Some((rect.rect, clipped.clip_rect, rect.stroke.width))
            }
            _ => None,
        })
        .collect();
    boxes.sort_by(|left, right| left.0.top().total_cmp(&right.0.top()));
    assert!(boxes.len() >= 2, "{} sets were drawn, not two", boxes.len());

    // Whole, not sliced: the line is drawn half on either side of the
    // rectangle, and all of it has to be inside what the list may paint in.
    for (set, clip, stroke) in &boxes {
        let line = set.expand(stroke / 2.0);
        // The bottom-most box runs off the end of the window, which is what
        // a list does; none of them is cut off at the top of it.
        assert!(
            clip.top() <= line.top() + 0.01,
            "a set drawn at {set:?} has its top cut off by the clip at {clip:?}"
        );
        assert!(
            clip.left() <= line.left() + 0.01 && clip.right() >= line.right() - 0.01,
            "a set drawn at {set:?} is cut off sideways by the clip at {clip:?}"
        );
    }

    // Spaced, not stacked.
    let gap = boxes[1].0.top() - boxes[0].0.bottom();
    assert!(
        gap >= BETWEEN_BOXES - 0.01,
        "one set ends at {} and the next starts at {}, {gap} apart",
        boxes[0].0.bottom(),
        boxes[1].0.top()
    );

    // The same room on either side of a box: the window's margin on the
    // left, and the same again between the box and the bar beside the list.
    let bar = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect)
                if (rect.rect.width() - SCROLL_BAR).abs() < 1.0 && rect.rect.height() > 100.0 =>
            {
                Some(rect.rect.left())
            }
            _ => None,
        })
        .fold(f32::MAX, f32::min);
    assert!(bar < screen.right(), "the list's scroll bar was not drawn");
    let (first, _, stroke) = boxes[0];
    let left = first.left() - stroke / 2.0;
    let right = bar - (first.right() + stroke / 2.0);
    assert!(
        (left - right).abs() < 0.51,
        "a set has {left} to the left of it and {right} to the right of it"
    );

    // And inside a box, the pictures keep the same room on either side: what
    // the strip is clipped to sits `BOX_PADDING` inside the box at both
    // edges, so the first picture starts as far in as the last one ends.
    let strip = drawn
        .iter()
        .map(|clipped| clipped.clip_rect)
        .filter(|clip| {
            clip.top() > first.top()
                && clip.bottom() < first.bottom()
                && clip.width() > 100.0
                && clip.right() < bar
        })
        .fold(egui::Rect::NOTHING, |widest, clip| {
            if clip.width() > widest.width() {
                clip
            } else {
                widest
            }
        });
    assert!(
        strip.is_finite(),
        "the pictures in the first set were not drawn"
    );
    let inside = first.shrink(stroke / 2.0);
    let (before, after) = (strip.left() - inside.left(), inside.right() - strip.right());
    assert!(
        (before - after).abs() < 0.51,
        "the pictures start {before} inside the box and end {after} inside it"
    );
}

/// Every line of a given colour drawn round something, however faded.
///
/// A faded set is drawn at a quarter, and fading multiplies every channel of
/// a colour, alpha included. So the colour looked for is faded the same way
/// before it is compared, by however much the painted one was: an exact
/// comparison against the unfaded colour matches a set at full strength and
/// misses the faded one, which is an assertion that cannot fail.
fn outlined(drawn: &[egui::epaint::ClippedShape], colour: egui::Color32) -> usize {
    let near = |painted: egui::Color32| {
        let faded = colour.gamma_multiply(f32::from(painted.a()) / f32::from(colour.a()));
        let (painted, faded) = (painted.to_array(), faded.to_array());
        (0..3).all(|channel| painted[channel].abs_diff(faded[channel]) <= 2)
    };
    drawn
        .iter()
        .filter(|clipped| match &clipped.shape {
            egui::Shape::Rect(rect) if rect.stroke.width >= 2.0 => near(rect.stroke.color),
            _ => false,
        })
        .count()
}

/// Draw one set over a really scanned folder, twice, and hand back the second
/// frame: what a picture shows takes a frame to settle.
fn set_frames(
    app: &mut App,
    ctx: &egui::Context,
    root: &std::path::Path,
) -> Vec<egui::epaint::ClippedShape> {
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let mut draw = || {
        crate::shot::frame(
            "a_set_drawn_for_what_it_shows",
            ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, root, ui.available_width()));
            },
        )
    };
    draw();
    draw()
}

/// Keeping a picture is two things on screen: a green border round it and
/// the word KEEP under it. A set that marks nothing draws neither, and the
/// marks are what decides it.
#[test]
fn a_marked_picture_is_drawn_with_a_border_and_an_unmarked_one_is_not() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let set_id = app.sets[0].set_id;

    let root = found.path().to_path_buf();
    let ctx = window();
    let keep_colour = egui::Color32::from_rgb(90, 180, 110);

    // A review arrives marking nothing.
    let plain = set_frames(&mut app, &ctx, &root);
    assert_eq!(
        outlined(&plain, keep_colour),
        0,
        "something was drawn as kept already"
    );
    assert!(
        label_rects(&plain, "KEEP").is_empty(),
        "something said KEEP already"
    );

    app.keep
        .insert(set_id, Keep::One(app.sets[0].members[0].file_id));
    let marked = set_frames(&mut app, &ctx, &root);
    assert_eq!(
        outlined(&marked, keep_colour),
        1,
        "the marked picture had no border"
    );
    assert_eq!(label_rects(&marked, "KEEP").len(), 1, "nothing said KEEP");

    // Which is what keep none takes off again.
    app.keep.remove(&set_id);
    let cleared = set_frames(&mut app, &ctx, &root);
    assert_eq!(
        outlined(&cleared, keep_colour),
        0,
        "the border stayed on an unmarked picture"
    );
    assert!(
        label_rects(&cleared, "KEEP").is_empty(),
        "an unmarked picture said KEEP"
    );
}

/// The ring round the picture the preview is on says where the cursor keys
/// are and nothing about what the set keeps: a picture can have one, the
/// other, both or neither. Taking the marks off leaves the ring where it is.
#[test]
fn the_ring_round_the_picture_shown_is_not_the_keep_border() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let set_id = app.sets[0].set_id;
    let first = app.sets[0].members[0].file_id;
    app.selected = Some(first);

    let root = found.path().to_path_buf();
    let ctx = window();
    let ring_colour = ctx.style().visuals.selection.bg_fill;
    let keep_colour = egui::Color32::from_rgb(90, 180, 110);

    // Shown and not marked: a ring and no border.
    let showing = set_frames(&mut app, &ctx, &root);
    assert_eq!(
        outlined(&showing, ring_colour),
        1,
        "the picture shown had no ring"
    );
    assert_eq!(
        outlined(&showing, keep_colour),
        0,
        "an unmarked picture had a keep border"
    );

    // Marked as well: both, on the same picture.
    app.keep.insert(set_id, Keep::One(first));
    let both = set_frames(&mut app, &ctx, &root);
    assert_eq!(
        outlined(&both, ring_colour),
        1,
        "the ring went when the picture was marked"
    );
    assert_eq!(
        outlined(&both, keep_colour),
        1,
        "the marked picture had no border"
    );

    // And the ring is inside the border, not over it. Drawn outside, the ring
    // for the picture being looked at covers the border that says the picture
    // is being kept, and that is the one thing somebody needs to see about
    // the picture in front of them.
    let inside = |drawn: &[egui::epaint::ClippedShape], colour: egui::Color32| -> egui::Rect {
        drawn
            .iter()
            .find_map(|clipped| match &clipped.shape {
                egui::Shape::Rect(rect) if rect.stroke.width >= 2.0 => {
                    let painted = rect.stroke.color;
                    let faded =
                        colour.gamma_multiply(f32::from(painted.a()) / f32::from(colour.a()));
                    let (painted, faded) = (painted.to_array(), faded.to_array());
                    (0..3)
                        .all(|channel| painted[channel].abs_diff(faded[channel]) <= 2)
                        .then_some(rect.rect)
                }
                _ => None,
            })
            .expect("nothing was drawn in that colour")
    };
    let ring = inside(&both, ring_colour);
    let border = inside(&both, keep_colour);
    assert!(
        border.contains_rect(ring),
        "the ring is not inside the keep border: ring {ring:?}, border {border:?}"
    );

    // Marks off: the ring stays, because it was never about the marks.
    app.keep.remove(&set_id);
    let after = set_frames(&mut app, &ctx, &root);
    assert_eq!(
        outlined(&after, ring_colour),
        1,
        "taking the marks off took the ring with them"
    );
    assert_eq!(
        outlined(&after, keep_colour),
        0,
        "the border stayed on an unmarked picture"
    );
}

/// A set nobody calls a set of copies is barely drawn: the pictures and every
/// line of writing under them at `IGNORED_OPACITY`. The buttons are not,
/// because they are how it stops being ignored.
#[test]
fn an_ignored_set_is_drawn_faded_and_its_buttons_are_not() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let set_id = app.sets[0].set_id;

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 700.0));
    let draw = |app: &mut App| {
        crate::shot::frame(
            "an_ignored_set_is_drawn_faded_and_its_buttons_are_not",
            &ctx,
            egui::RawInput {
                screen_rect: Some(screen),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default()
                    .frame(
                        egui::Frame::central_panel(&ctx.style()).inner_margin(egui::Margin::ZERO),
                    )
                    .show(ctx, |ui| app.review_view(ui));
            },
        )
    };
    // What one line of writing in the list is drawn in. The file names under
    // the pictures are the strip; "keep all" is the row of buttons. Only what
    // is in the list: the pane beside it names the same file and is not part
    // of any set.
    let alpha_of =
        |drawn: &[egui::epaint::ClippedShape], words: &str, list_ends: f32| -> Option<u8> {
            drawn.iter().find_map(|clipped| match &clipped.shape {
                egui::Shape::Text(text)
                    if text.pos.x < list_ends && text.galley.text().contains(words) =>
                {
                    Some(text.fallback_color.a())
                }
                _ => None,
            })
        };

    draw(&mut app);
    let plain = draw(&mut app);
    let list_ends = screen.right() - app.preview_width.expect("the preview pane was not drawn");
    let alpha_of = |drawn: &[egui::epaint::ClippedShape], words: &str| -> Option<u8> {
        alpha_of(drawn, words, list_ends)
    };
    let strip = alpha_of(&plain, "one.png").expect("no file name was drawn under a picture");
    let buttons = alpha_of(&plain, "keep all").expect("no buttons were drawn under the set");

    app.ignore_set(set_id);
    assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");
    draw(&mut app);
    let faded = draw(&mut app);

    let now = alpha_of(&faded, "one.png").expect("the file name went when the set was ignored");
    let wanted = f32::from(strip) * IGNORED_OPACITY;
    assert!(
        (f32::from(now) - wanted).abs() <= 2.0,
        "the writing under the pictures went from {strip} to {now}, and not to {wanted}"
    );
    assert_eq!(
        alpha_of(&faded, "keep all"),
        Some(buttons),
        "the buttons under an ignored set were faded with the rest of it"
    );
}

/// Ignoring a set says none of its pictures are copies of each other. The
/// set stays on the review page, nothing in it is kept or dropped, the
/// folder's index remembers it, and a review holding nothing else has
/// nowhere for a cleanup to go.
#[test]
fn an_ignored_set_is_shown_and_left_alone() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    // A review arrives with nothing marked, so there is nothing to be left
    // alone until something is marked.
    app.auto_mark_to_keep();
    assert_eq!(app.sets.len(), 1, "the copies were not found");
    let set_id = app.sets[0].set_id;
    assert!(
        !app.is_ignored(&app.sets[0]),
        "a set was ignored before anybody said so"
    );
    let (going, _) = app.selected_for_removal();
    assert!(going > 0, "nothing was going to be removed to begin with");

    app.ignore_set(set_id);

    assert_eq!(
        app.sets.len(),
        1,
        "the set went instead of being left alone"
    );
    assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");
    assert_eq!(
        app.selected_for_removal().0,
        0,
        "an ignored set still had something going"
    );
    assert!(
        app.build_plan().files() == 0,
        "the cleanup still had something to do"
    );
    // What it kept is still written down, and counts for nothing while it is
    // ignored. That is what taking it back gives back.
    assert!(
        app.keep.contains_key(&set_id),
        "an ignored set forgot what it had kept"
    );

    // Written down, so the next run knows it too.
    closed(&app);
    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    again.load_sets();
    settle(&mut again);
    assert_eq!(again.sets.len(), 1, "the set was not found again");
    assert!(
        again.is_ignored(&again.sets[0]),
        "the folder forgot that the set was ignored"
    );
}

/// Ignoring is one press, and so is taking it back: the pairs go from the
/// index, the set is a set again, and what it keeps works as it did.
#[test]
fn a_set_can_be_unignored_again() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    // Marked, so the set has something to go back to being.
    app.auto_mark_to_keep();
    let set_id = app.sets[0].set_id;

    app.ignore_set(set_id);
    assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");
    app.unignore_set(set_id);
    assert!(!app.is_ignored(&app.sets[0]), "the set is still ignored");
    assert!(
        app.selected_for_removal().0 > 0,
        "the set is not being cleaned up again"
    );

    // And the index no longer holds any of it.
    let db_path = app.db_path.clone().expect("a db");
    let conn = index_file(&app, &db_path);
    assert!(
        db::ignored(&conn).expect("read").is_empty(),
        "the index still holds the pairs"
    );
    let _ = conn.close();

    // Opened again from nothing, the set is a set.
    closed(&app);
    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    again.load_sets();
    settle(&mut again);
    assert!(
        !again.is_ignored(&again.sets[0]),
        "the folder came back with it still ignored"
    );
}

/// A window opened on the folder it was left on comes up knowing which of its
/// sets are not sets of copies. That folder is never chosen: it is set before
/// the first frame, so whatever reads a folder's index has to read the
/// ignored pairs too, or a set ignored last time comes back as a set.
#[test]
fn a_folder_the_window_opens_on_comes_up_with_its_ignored_sets_ignored() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 1, "the copies were not found");
    app.ignore_set(app.sets[0].set_id);
    assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");

    // The next run of the window, opened on the folder it was left on rather
    // than on one somebody picked.
    let saved = crate::settings::Settings {
        folder: Some(found.path().to_path_buf()),
        ..crate::settings::Settings::default()
    };
    closed(&app);
    let mut again = App::from_settings(saved);
    again.open_what_was_left_open();
    settle(&mut again);

    assert!(
        !again.ignored.is_empty(),
        "the folder came up knowing nothing was ignored"
    );
    again.load_sets();
    settle(&mut again);
    assert_eq!(again.sets.len(), 1, "the set was not found again");
    assert!(
        again.is_ignored(&again.sets[0]),
        "the set came back as a set of copies"
    );
}

/// Taking a set back gives back the picture it was keeping. Ignoring it does
/// not throw the mark away: while a set is ignored the mark means nothing and
/// the cleanup passes over it, and the moment it is a set again the mark is
/// where it was left.
#[test]
fn unignoring_a_set_gives_back_the_picture_it_was_keeping() {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.sets = vec![DuplicateSet {
        set_id: 1,
        members: vec![member(1, "a.jpg", 300), member(2, "b.jpg", 200)],
    }];
    app.selected = Some(2);
    app.keep_selected();
    assert_eq!(
        app.keep.get(&1),
        Some(&Keep::One(2)),
        "the picture was not kept"
    );

    app.ignore_set(1);
    assert!(app.is_ignored(&app.sets[0]), "the set was not ignored");
    assert_eq!(
        app.selected_for_removal().0,
        0,
        "an ignored set still had something going"
    );
    // And nothing in it can be marked or unmarked while it is ignored.
    app.selected = Some(1);
    app.keep_selected();

    app.unignore_set(1);
    assert_eq!(
        app.keep.get(&1),
        Some(&Keep::One(2)),
        "the set came back keeping something other than what it had kept"
    );
    assert_eq!(
        app.selected_for_removal().0,
        1,
        "the set is not being cleaned up again"
    );
}

/// Ignoring the set the preview is in leaves the preview where it was, so the
/// cursor keys have somewhere to walk from: left and up to the set before it,
/// right and down to the set after it.
#[test]
fn ignoring_the_set_the_preview_is_in_leaves_the_keys_somewhere_to_go() {
    let sets = || {
        vec![
            DuplicateSet {
                set_id: 1,
                members: vec![member(1, "a.jpg", 10), member(2, "b.jpg", 10)],
            },
            DuplicateSet {
                set_id: 2,
                members: vec![member(3, "c.jpg", 10), member(4, "d.jpg", 10)],
            },
            DuplicateSet {
                set_id: 3,
                members: vec![member(5, "e.jpg", 10), member(6, "f.jpg", 10)],
            },
        ]
    };
    let visible: Vec<usize> = (0..3).collect();

    // Left and right cross into the set at the picture nearest to where they
    // left; up and down keep their place in the set they land in.
    for (direction, wanted, what) in [
        (Direction::Back, 2, "left"),
        (Direction::PreviousSet, 2, "up"),
        (Direction::Forward, 5, "right"),
        (Direction::NextSet, 6, "down"),
    ] {
        let mut app = App::from_settings(crate::settings::Settings::default());
        app.sets = sets();
        // The preview is on the second picture of the middle set, and that
        // set is the one being ignored.
        app.selected = Some(4);
        app.ignore_set(2);
        assert!(app.is_ignored(&app.sets[1]), "the set was not ignored");
        assert_eq!(
            app.selected,
            Some(4),
            "{what}: ignoring took the preview away"
        );

        app.walk(&visible, direction);
        assert_eq!(
            app.selected,
            Some(wanted),
            "{what} did not leave the ignored set"
        );
    }
}

/// The cursor keys step over a set nobody calls a set of copies: on to the
/// next set that is one, and nowhere at all when there is none.
#[test]
fn the_cursor_keys_step_over_ignored_sets() {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.sets = vec![
        DuplicateSet {
            set_id: 1,
            members: vec![member(1, "a.jpg", 10), member(2, "b.jpg", 10)],
        },
        DuplicateSet {
            set_id: 2,
            members: vec![member(3, "c.jpg", 10), member(4, "d.jpg", 10)],
        },
        DuplicateSet {
            set_id: 3,
            members: vec![member(5, "e.jpg", 10), member(6, "f.jpg", 10)],
        },
    ];
    // The middle set is not a set of copies.
    app.ignored.insert(db::pair(3, 4));
    let visible: Vec<usize> = (0..app.sets.len()).collect();

    // Walking forward off the end of the first set lands in the third.
    app.selected = Some(2);
    app.walk(&visible, Direction::Forward);
    assert_eq!(
        app.selected,
        Some(5),
        "forward did not step over the ignored set"
    );

    // And back again the same way.
    app.walk(&visible, Direction::Back);
    assert_eq!(
        app.selected,
        Some(2),
        "back did not step over the ignored set"
    );

    // A set at a time, the same.
    app.selected = Some(1);
    app.walk(&visible, Direction::NextSet);
    assert_eq!(app.selected, Some(5), "the next set was the ignored one");

    // With nothing but ignored sets beyond it, the keys do nothing.
    app.ignored.insert(db::pair(5, 6));
    app.selected = Some(2);
    app.walk(&visible, Direction::Forward);
    assert_eq!(app.selected, Some(2), "the keys moved into an ignored set");
    app.walk(&visible, Direction::NextSet);
    assert_eq!(app.selected, Some(2), "the keys moved into an ignored set");
}

/// Ignoring one pair of a larger set is not ignoring the set: the rest of
/// them are still copies of each other.
#[test]
fn a_set_is_only_ignored_when_every_pair_in_it_is() {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.sets = vec![DuplicateSet {
        set_id: 1,
        members: vec![
            member(1, "a.jpg", 100),
            member(2, "b.jpg", 100),
            member(3, "c.jpg", 100),
        ],
    }];
    app.ignored.insert(db::pair(1, 2));
    assert!(
        !app.is_ignored(&app.sets[0]),
        "one pair of three was enough to ignore the set"
    );

    app.ignored.insert(db::pair(1, 3));
    app.ignored.insert(db::pair(2, 3));
    assert!(
        app.is_ignored(&app.sets[0]),
        "every pair was ignored and the set was not"
    );
}

/// The numbers beside the steps are the run's own clock: how long since the
/// Scan button was pressed, not how long the window has been open. A window
/// left open for an hour and then told to scan reports milliseconds, not an
/// hour.
#[test]
fn the_times_beside_the_steps_are_measured_from_the_scan_button() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    // As if the window had been sitting there for a while before the press.
    app.started = std::time::Instant::now() - std::time::Duration::from_secs(3600);

    app.start_scan();
    settle(&mut app);

    let lit: Vec<(Lamp, u128)> = LAMPS
        .iter()
        .filter_map(|(lamp, _)| self_lit(&app, *lamp).map(|at| (*lamp, at)))
        .collect();
    assert!(!lit.is_empty(), "the pass lit nothing");
    for (lamp, at) in lit {
        assert!(
            at < 60_000,
            "{lamp:?} says {at} ms, which is the window's clock"
        );
    }
}

fn self_lit(app: &App, lamp: Lamp) -> Option<u128> {
    app.lit.get(&lamp).copied()
}

/// A step nothing was going to do is not a step that failed. A pass over a
/// folder where nothing has changed reads no files and indexes none, and
/// those four steps end as passed over rather than as still to happen.
#[test]
fn the_steps_a_pass_had_nothing_to_do_are_marked_as_passed_over() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());

    // Every file was read the first time round, so all four turned.
    for lamp in [
        Lamp::StartedReadingNewFiles,
        Lamp::FinishedReadingNewFiles,
        Lamp::StartedIndexingNewFiles,
        Lamp::FinishedIndexingNewFiles,
    ] {
        assert_eq!(
            app.how_it_went(lamp),
            Went::Happened,
            "{lamp:?} did not happen"
        );
    }

    // A second pass over the same folder has nothing to read or index.
    app.start_scan();
    settle(&mut app);
    assert!(
        app.scan.unchanged > 0,
        "the second pass read the folder again"
    );
    for lamp in [
        Lamp::StartedReadingNewFiles,
        Lamp::FinishedReadingNewFiles,
        Lamp::StartedIndexingNewFiles,
        Lamp::FinishedIndexingNewFiles,
    ] {
        assert_eq!(
            app.how_it_went(lamp),
            Went::Skipped,
            "{lamp:?} was not passed over"
        );
    }
    // And the ones that did happen still say so.
    assert_eq!(app.how_it_went(Lamp::CheckedForIndexFile), Went::Happened);
}

/// Opening a folder that has been scanned before reads its index into
/// memory whatever the rescan box says. That is what the index is for: the
/// pictures are known, so Find duplicates costs the comparing and nothing
/// else. Only the pass over the files is what the box decides.
#[test]
fn opening_a_folder_reads_its_index_without_scanning_it() {
    let found = folder_with_a_duplicate();
    let app = reviewing(found.path());
    assert!(
        !app.auto_rescan,
        "the folder asked to be rescanned on its own"
    );

    closed(&app);
    let mut again = App::from_settings(crate::settings::Settings::default());
    assert!(
        again.images.is_none(),
        "a window with no folder open holds pictures"
    );
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);

    assert!(
        again.running.is_none(),
        "the folder was scanned although the box is off"
    );
    let held = again
        .images
        .as_ref()
        .expect("the index was not read into memory");
    assert_eq!(
        held.len(),
        3,
        "the index came back holding {} pictures",
        held.len()
    );
    assert!(
        again.lit.contains_key(&Lamp::LoadedIndexIntoMemory),
        "nothing said the index had been read"
    );

    // And it can be searched straight away, without a pass.
    again.load_sets();
    settle(&mut again);
    assert_eq!(
        again.sets.len(),
        1,
        "the copies were not found from the index alone"
    );
}

/// The other two boxes are kept with the folder as well: whether it is
/// searched one folder at a time, and whether opening it runs a pass.
#[test]
fn the_index_keeps_matching_within_folders_and_running_on_opening() {
    let found = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(found.path().to_path_buf());
    assert!(
        !app.within_a_folder,
        "folders were being kept apart to begin with"
    );
    assert!(
        !app.auto_rescan,
        "opening the folder was going to run a pass to begin with"
    );

    // Matching within folders needs the folder to be scanned with its
    // subfolders, which is a fact the index holds, the pass writes, and a
    // later pass over the same index does not argue with. So it is set
    // before the folder has an index at all.
    app.recurse = true;
    app.start_scan();
    settle(&mut app);
    app.within_a_folder = true;
    app.auto_rescan = true;
    // Running on opening is a choice about the folder and is written where it
    // is made. Matching within folders is a search setting, so it waits for a
    // search to use it.
    app.remember_ways_of_matching();
    let clicked = crate::notes::read(&app.index);
    assert_eq!(
        (
            clicked.recurse,
            clicked.within_a_folder,
            clicked.auto_rescan
        ),
        (Some(true), Some(false), Some(true)),
        "a search setting was written before a search had used it"
    );
    app.load_sets();
    settle(&mut app);
    let written = crate::notes::read(&app.index);
    assert_eq!(
        (
            written.recurse,
            written.within_a_folder,
            written.auto_rescan
        ),
        (Some(true), Some(true), Some(true)),
        "the index did not come out of that holding all three"
    );

    closed(&app);
    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    assert!(
        again.within_a_folder,
        "the folder came back without folders kept apart"
    );
    assert!(
        again.auto_rescan,
        "the folder came back without running on opening"
    );
}

/// Shift says which picture rather than toggling one: whatever the set was
/// marking, afterwards it marks that one and nothing else. Pressed again on
/// the same picture it stays marked, because it is not a toggle.
#[test]
fn shift_marks_the_selected_picture_and_unmarks_the_rest() {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.sets = vec![DuplicateSet {
        set_id: 1,
        members: vec![
            member(1, "a.jpg", 500),
            member(2, "b.jpg", 400),
            member(3, "c.jpg", 300),
        ],
    }];
    let set_id = app.sets[0].set_id;
    app.keep.insert(set_id, Keep::Several(vec![1, 2]));

    app.selected = Some(3);
    app.keep_only_selected();
    assert_eq!(
        app.keep.get(&set_id),
        Some(&Keep::One(3)),
        "the other marks were left where they were"
    );

    app.keep_only_selected();
    assert_eq!(
        app.keep.get(&set_id),
        Some(&Keep::One(3)),
        "a second press took the mark off, which is what the toggle does"
    );
}

/// A mark says to keep one picture and says nothing about any other, so
/// marks add up. Taking them off again one at a time can leave the set
/// marked with nothing at all, which is where it started.
#[test]
fn marks_add_up_and_come_off_one_at_a_time() {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.sets = vec![DuplicateSet {
        set_id: 1,
        members: vec![
            member(1, "a.jpg", 500),
            member(2, "b.jpg", 400),
            member(3, "c.jpg", 300),
        ],
    }];
    let set_id = app.sets[0].set_id;

    app.selected = Some(1);
    app.keep_selected();
    app.selected = Some(3);
    app.keep_selected();
    assert_eq!(
        app.keep.get(&set_id),
        Some(&Keep::Several(vec![1, 3])),
        "the second mark did not join the first"
    );

    app.selected = Some(1);
    app.keep_selected();
    assert_eq!(
        app.keep.get(&set_id),
        Some(&Keep::One(3)),
        "taking one off left the wrong picture"
    );

    app.selected = Some(3);
    app.keep_selected();
    assert_eq!(
        app.keep.get(&set_id),
        None,
        "the set is still keeping something"
    );
}

/// Two clicks on a second picture keep it as well as the one already marked,
/// rather than in place of it.
#[test]
fn two_clicks_on_a_second_picture_keep_both() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let root = found.path().to_path_buf();
    let set_id = app.sets[0].set_id;
    let first = app.sets[0].members[0].file_id;
    app.keep.insert(set_id, Keep::One(first));
    let other = app.sets[0]
        .members
        .iter()
        .map(|member| member.file_id)
        .find(|file_id| *file_id != first)
        .expect("the set holds one picture");

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let frame = |app: &mut App, at: Option<egui::Pos2>, clicks: usize, time: f64| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            time: Some(time),
            ..Default::default()
        };
        if let Some(pos) = at {
            input.events.push(egui::Event::PointerMoved(pos));
            for _ in 0..clicks {
                for pressed in [true, false] {
                    input.events.push(egui::Event::PointerButton {
                        pos,
                        button: egui::PointerButton::Primary,
                        pressed,
                        modifiers: Default::default(),
                    });
                }
            }
        }
        crate::shot::frame("two_clicks_with_multi_selected", &ctx, input, |ctx| {
            egui::CentralPanel::default()
                .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
        })
    };

    let drawn = frame(&mut app, None, 0, 0.0);
    let mut pictures: Vec<egui::Rect> = drawn
        .iter()
        .filter_map(|clipped| match &clipped.shape {
            // Not the ring around the picture being looked at: that is drawn
            // inside the tile now, so it is close to the tile's own size.
            egui::Shape::Rect(rect)
                if rect.stroke.color != selection_colour()
                    && (rect.rect.width() - TILE.x).abs() < 6.0
                    && (rect.rect.height() - TILE.y).abs() < 6.0 =>
            {
                Some(rect.rect)
            }
            _ => None,
        })
        .collect();
    pictures.sort_by(|a, b| a.left().total_cmp(&b.left()));
    let index = app.sets[0]
        .members
        .iter()
        .position(|member| member.file_id == other)
        .expect("the set lost a picture");
    let at = pictures[index].center();

    frame(&mut app, Some(at), 0, 0.1);
    frame(&mut app, Some(at), 2, 0.2);

    let keeping = app.keep.get(&set_id).expect("the set is keeping nothing");
    assert!(
        keeping.keeps(first),
        "the picture kept before the clicks was let go"
    );
    assert!(
        keeping.keeps(other),
        "the picture that was clicked twice is not kept"
    );
}

/// The two buttons on a set really pressed, in a really scanned folder. Keep
/// none takes every picture of the set into the plan, and keep all clears
/// the marks, which takes them all back out.
#[test]
fn the_buttons_on_a_set_decide_all_of_it_or_none_of_it() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 1, "the two copies were not found");
    app.keep.insert(
        app.sets[0].set_id,
        Keep::One(app.sets[0].members[0].file_id),
    );
    assert_eq!(
        app.build_plan().files(),
        1,
        "one mark should leave one picture going"
    );

    let root = found.path().to_path_buf();
    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let frame = |app: &mut App, at: Option<egui::Pos2>, pressed: Option<bool>| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        if let Some(pos) = at {
            input.events.push(egui::Event::PointerMoved(pos));
            if let Some(pressed) = pressed {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
        }
        crate::shot::frame(
            "the_buttons_on_a_set_decide_all_or_none",
            &ctx,
            input,
            |ctx| {
                egui::CentralPanel::default()
                    .show(ctx, |ui| app.set_row(ui, 0, &root, ui.available_width()));
            },
        )
    };

    // The pointer has to have been over a widget on an earlier frame before
    // egui reports a click on it, so each press takes three.
    let drawn = frame(&mut app, None, None);
    let none_at = label_rect(&drawn, "keep none")
        .expect("no keep none button was drawn on the set")
        .center();
    let all_at = label_rect(&drawn, "keep all")
        .expect("no keep all button was drawn on the set")
        .center();

    // Keep all marks every picture in the set, which takes all of it out of
    // the plan.
    frame(&mut app, Some(all_at), None);
    frame(&mut app, Some(all_at), Some(true));
    frame(&mut app, Some(all_at), Some(false));
    let keeping = app
        .keep
        .get(&app.sets[0].set_id)
        .expect("keep all marked nothing");
    for member in &app.sets[0].members {
        assert!(
            keeping.keeps(member.file_id),
            "keep all left {} unmarked",
            member.rel_path
        );
    }
    assert_eq!(
        app.build_plan().files(),
        0,
        "keep all still gave the set up"
    );

    // Keep none takes every mark off, which puts every picture of the set
    // into the plan.
    frame(&mut app, Some(none_at), None);
    frame(&mut app, Some(none_at), Some(true));
    frame(&mut app, Some(none_at), Some(false));
    assert_eq!(
        app.keep.get(&app.sets[0].set_id),
        None,
        "keep none left a mark behind"
    );
    assert_eq!(
        app.build_plan().files(),
        2,
        "keep none left a picture behind"
    );
}

/// Auto-marking only ever adds. A set with nothing marked gets the best
/// copy; a set already marking something else keeps that and gains it.
#[test]
fn auto_marking_adds_the_best_copy_and_disturbs_nothing() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    assert_eq!(app.sets.len(), 2, "the two pairs were not found");
    let (first, second) = (app.sets[0].set_id, app.sets[1].set_id);
    let best_of = |app: &App, set: usize| {
        app.sets[set]
            .members
            .iter()
            .find(|member| member.auto_keep)
            .expect("a best copy")
            .file_id
    };
    let (best_first, best_second) = (best_of(&app, 0), best_of(&app, 1));

    // The second set already marks the copy that is not the best one.
    let other_in_second = app.sets[1]
        .members
        .iter()
        .map(|member| member.file_id)
        .find(|file_id| *file_id != best_second)
        .expect("the set holds two pictures");
    app.keep.insert(second, Keep::One(other_in_second));

    app.auto_mark_to_keep();

    assert_eq!(
        app.keep.get(&first),
        Some(&Keep::One(best_first)),
        "the best copy was not marked"
    );
    let keeping = app.keep.get(&second).expect("the second set lost its mark");
    assert!(
        keeping.keeps(other_in_second),
        "the mark that was already there was taken off"
    );
    assert!(
        keeping.keeps(best_second),
        "the best copy was not added beside it"
    );
}

/// A set nobody calls a set of copies is an answer already given, and
/// auto-marking does not answer again.
#[test]
fn auto_marking_leaves_ignored_sets_alone() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    let ignored = app.sets[0].set_id;
    app.ignore_set(ignored);

    app.auto_mark_to_keep();

    assert_eq!(app.keep.get(&ignored), None, "an ignored set was marked");
}

/// What is marked is kept, so a set that marks nothing keeps nothing and all
/// of it is there to be cleaned up. A review arrives marking nothing, so
/// that is every picture of every set it found.
#[test]
fn a_set_marked_with_nothing_loses_all_of_it() {
    let found = folder_with_a_duplicate();
    let app = reviewing(found.path());
    assert!(
        app.keep.is_empty(),
        "the search marked something on its own"
    );
    assert_eq!(
        app.build_plan().files(),
        app.sets[0].members.len(),
        "a set marking nothing was not all going"
    );
}

#[test]
fn a_mark_reaches_the_index_and_leaves_the_pictures_alone() {
    let found = folder_with_a_duplicate();
    let db_path = headless::default_db_path(found.path());
    let mut app = reviewing(found.path());

    let marked = app.sets[0].members[1].file_id;
    app.selected = Some(marked);
    app.keep_selected();
    assert_eq!(app.plan.files(), 1);
    assert_eq!(app.keep.len(), 1);

    let conn = index_file(&app, &db_path);
    assert_eq!(
        db::kept(&conn).expect("marks"),
        vec![marked],
        "the mark is not in the index"
    );
    let rows: i64 = conn
        .query_row("SELECT count(*) FROM files", [], |row| row.get(0))
        .expect("count");
    assert_eq!(
        rows, 3,
        "marking a picture changed what the index knows about the folder"
    );
}

/// A real pass at the top of the scale, and then the window closed and
/// opened again from what it wrote down. The folder comes back, the slider
/// does not: what counts as a duplicate is decided against the pictures on
/// screen and never carried over from a run that is over.
#[test]
fn the_setting_for_what_counts_as_a_duplicate_is_not_kept_across_a_restart() {
    let folder = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(folder.path().to_path_buf());
    app.keep_index = true;
    app.sensitivity = matching::MAX_SENSITIVITY;
    app.ignore_colour = true;
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);
    assert!(!app.sets.is_empty(), "the pass found nothing to review");

    let next = App::from_settings(app.settings());
    assert_eq!(
        next.sensitivity,
        matching::DEFAULT_SENSITIVITY,
        "the last run's sensitivity came back"
    );
    assert_eq!(next.folder, app.folder, "the folder was not remembered");
    assert!(next.ignore_colour, "the colour setting was not remembered");
}

/// A folder of one picture under the given name, so a test can say what
/// order two of them come out in.
fn folder_named(root: &std::path::Path, name: &str) -> PathBuf {
    let dir = root.join(name);
    std::fs::create_dir(&dir).expect("mkdir");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
        image::Rgb([((x * 3) % 256) as u8, ((y * 5) % 256) as u8, 40])
    }))
    .save_with_format(dir.join("one.png"), image::ImageFormat::Png)
    .expect("a fixture");
    dir
}

/// A folder dropped on the window is opened, scanned, and searched, without
/// anything being pressed, from whichever tab the window happened to be on.
#[test]
fn a_folder_dropped_on_the_window_is_scanned_and_searched() {
    let reviewed = folder_with_two_sets();
    let mut app = reviewing(reviewed.path());
    assert_eq!(
        app.view,
        View::Review,
        "the fixture did not reach the review"
    );

    let folder = folder_with_a_duplicate();
    let ctx = window();

    let input = egui::RawInput {
        dropped_files: vec![egui::DroppedFile {
            path: Some(folder.path().to_path_buf()),
            ..Default::default()
        }],
        ..Default::default()
    };
    let _ = ctx.run(input, |ctx| app.take_dropped_folder(ctx));

    assert_eq!(
        app.folder.as_deref(),
        Some(folder.path()),
        "the folder was not opened"
    );
    assert!(app.running.is_some(), "the drop did not start a scan");
    assert_eq!(
        app.view,
        View::Scan,
        "the drop left the window on the old tab"
    );

    settle(&mut app);
    assert_eq!(app.sets.len(), 1, "the search did not follow the scan");
    assert_eq!(
        app.view,
        View::Review,
        "the window did not move on to the review"
    );
}

/// Only folders. A file dropped on the window is not a folder to scan, and
/// neither is a drop while a pass is already running.
#[test]
fn dropping_anything_but_a_folder_does_nothing() {
    let folder = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    let ctx = window();

    let drop = |app: &mut App, path: PathBuf| {
        let input = egui::RawInput {
            dropped_files: vec![egui::DroppedFile {
                path: Some(path),
                ..Default::default()
            }],
            ..Default::default()
        };
        let _ = ctx.run(input, |ctx| app.take_dropped_folder(ctx));
    };

    drop(&mut app, folder.path().join("one.png"));
    assert!(
        app.folder.is_none(),
        "a dropped file was taken for a folder"
    );
    assert!(app.running.is_none(), "a dropped file started a scan");

    // Now with a pass under way: the second folder is not taken up.
    drop(&mut app, folder.path().to_path_buf());
    assert!(app.running.is_some(), "the folder was not scanned");
    let second = folder_with_a_duplicate();
    drop(&mut app, second.path().to_path_buf());
    assert_eq!(
        app.folder.as_deref(),
        Some(folder.path()),
        "a drop during a pass changed the folder"
    );
    settle(&mut app);
}

/// Two folders really scanned, one of them twice, and a third only opened.
/// The list offers what was scanned, in alphabetical order, once each.
#[test]
fn a_folder_joins_the_previous_list_by_being_scanned_and_not_by_being_opened() {
    let root = tempfile::tempdir().expect("tempdir");
    let zebra = folder_named(root.path(), "Zebra");
    let apple = folder_named(root.path(), "apple");
    let passed_over = folder_named(root.path(), "middle");

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(zebra.clone());
    assert!(
        app.previous.is_empty(),
        "choosing a folder was enough to list it"
    );

    app.start_scan();
    settle(&mut app);
    app.open_folder(apple.clone());
    app.start_scan();
    settle(&mut app);
    app.open_folder(passed_over);
    assert_eq!(
        app.previous,
        vec![apple.clone(), zebra.clone()],
        "the two scanned folders are not listed alphabetically"
    );

    app.open_folder(zebra.clone());
    app.start_scan();
    settle(&mut app);
    assert_eq!(
        app.previous,
        vec![apple, zebra],
        "scanning again listed it twice"
    );
}

/// The last entry of the list really clicked, in a window that has scanned
/// two folders. It empties the list, and the box goes with it.
#[test]
fn the_last_entry_of_the_previous_list_empties_it() {
    let root = tempfile::tempdir().expect("tempdir");
    let first = folder_named(root.path(), "Zebra");
    let second = folder_named(root.path(), "apple");

    let mut app = App::from_settings(crate::settings::Settings::default());
    for folder in [first, second] {
        app.open_folder(folder);
        app.start_scan();
        settle(&mut app);
    }
    assert_eq!(app.previous.len(), 2, "the scanned folders were not listed");

    let ctx = window();
    let screen = egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(900.0, 500.0));
    let frame = |app: &mut App, at: Option<egui::Pos2>, pressed: Option<bool>| {
        let mut input = egui::RawInput {
            screen_rect: Some(screen),
            ..Default::default()
        };
        if let Some(pos) = at {
            input.events.push(egui::Event::PointerMoved(pos));
            if let Some(pressed) = pressed {
                input.events.push(egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                });
            }
        }
        crate::shot::frame("the_last_entry_of_the_previous_list", &ctx, input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                app.folder_section(ui, 600.0);
            });
        })
    };

    let drawn = frame(&mut app, None, None);
    let box_rect =
        label_rect(&drawn, "previous").expect("no previous box was drawn beside the folder");
    let button = label_rect(&drawn, "Choose folder").expect("no folder button was drawn");
    assert!(
        box_rect.left() > button.right(),
        "the box is not right of the folder picker: {box_rect:?} against {button:?}"
    );
    let path = label_rect(&drawn, &app.folder.as_ref().unwrap().display().to_string())
        .expect("the folder was not shown");
    assert!(
        box_rect.left() >= path.right(),
        "the box is not right of the folder it belongs to: {box_rect:?} against {path:?}"
    );
    // The row was given 600 points, and the button belongs against the far
    // edge of it rather than trailing the path.
    assert!(
        box_rect.right() > 560.0,
        "the box is not against the right edge: {box_rect:?}"
    );
    let box_at = box_rect.center();
    frame(&mut app, Some(box_at), None);
    frame(&mut app, Some(box_at), Some(true));
    frame(&mut app, Some(box_at), Some(false));
    // The list is a popup, and it is drawn on the frame after the one that
    // opened it.
    let opened = frame(&mut app, Some(box_at), None);

    let clear_at = label_rect(&opened, "clear previous locations")
        .expect("the list has no entry to clear it with")
        .center();
    frame(&mut app, Some(clear_at), None);
    frame(&mut app, Some(clear_at), Some(true));
    frame(&mut app, Some(clear_at), Some(false));

    assert!(app.previous.is_empty(), "the list was not emptied");
    let after = frame(&mut app, None, None);
    assert!(
        label_rect(&after, "previous").is_none(),
        "an empty list still offers a box to pick from"
    );
}

#[test]
fn saved_settings_reach_the_window() {
    let saved = crate::settings::Settings {
        folder: Some(PathBuf::from("/photos")),
        previous: vec![PathBuf::from("/photos"), PathBuf::from("/more photos")],
        recurse: false,
        ignore_colour: true,
        window: Some(crate::settings::Window {
            x: 40.0,
            y: 80.0,
            width: 1200.0,
            height: 800.0,
            maximized: false,
        }),
        preview_width: Some(520.0),
    };
    let app = App::from_settings(saved.clone());
    assert_eq!(
        app.window, saved.window,
        "the window place was not restored"
    );
    assert_eq!(
        app.preview_width,
        Some(520.0),
        "the divider was not restored"
    );
    assert_eq!(app.folder, Some(PathBuf::from("/photos")));
    assert!(!app.recurse, "the subfolder setting was not restored");
    assert!(app.ignore_colour, "the colour setting was not restored");
    assert_eq!(
        app.previous,
        vec![PathBuf::from("/more photos"), PathBuf::from("/photos")],
        "the folders scanned before were not restored in order"
    );
    assert!(app.db_path.is_some(), "the index path was not derived");
}

/// Building the window asks the file system nothing: the folder is looked in
/// once the window is up, and the checkbox is ticked by the answer. A folder
/// with no index leaves it unticked. The settings file has no say in this.
#[test]
fn a_folder_with_an_index_is_asked_about_on_opening_and_one_without_is_not() {
    let dir = tempfile::tempdir().expect("tempdir");
    let folder = dir.path().to_path_buf();
    let settings = |folder: &std::path::Path| crate::settings::Settings {
        folder: Some(folder.to_path_buf()),
        ..crate::settings::Settings::default()
    };

    let mut app = App::from_settings(settings(&folder));
    assert!(
        !app.keep_index,
        "the checkbox was ticked before the folder was looked in"
    );
    app.open_what_was_left_open();
    settle(&mut app);
    assert!(
        !app.keep_index,
        "the checkbox was ticked although there is no index"
    );

    // Scan the folder so that it has an index. A pass writes one when it has
    // read something, so there has to be something in the folder to read.
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 40])
    }))
    .save_with_format(folder.join("a.png"), image::ImageFormat::Png)
    .expect("a fixture");
    let mut built = App::from_settings(crate::settings::Settings::default());
    built.open_folder(folder.clone());
    built.start_scan();
    settle(&mut built);
    assert!(
        headless::default_db_path(&folder).is_file(),
        "the scan did not write an index"
    );

    let mut app = App::from_settings(settings(&folder));
    app.open_what_was_left_open();
    settle(&mut app);
    assert!(
        app.keep_index,
        "an index exists but the checkbox was not ticked"
    );
}

/// Opening a folder reads its index on a thread of its own, and pressing
/// Scan starts a pass that reads the same folder again. Both are answered by
/// the one thing that owns the index, so the opening read can come back
/// after the pass has finished, describing the folder as it was before it.
///
/// The pass's pictures are the newer ones and they stay. Taking the older
/// ones left the window holding an empty folder, and the search that ran on
/// them found nothing.
#[test]
fn the_pictures_read_on_opening_do_not_replace_a_finished_pass() {
    let scanned = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(scanned.path().to_path_buf());
    app.start_scan();
    settle(&mut app);
    let found = app.images.clone().expect("the pass read the folder");
    assert_eq!(found.len(), 3, "the pass did not read the folder");

    // The opening read, arriving now: an index with nothing in it, because it
    // was read before the pass had written anything.
    let (send, receive) = std::sync::mpsc::channel();
    send.send(Opened::Index(std::sync::Arc::new(Vec::new())))
        .expect("the answer");
    drop(send);
    app.asking = Some(receive);
    app.hear_the_index(&window());

    let held = app
        .images
        .clone()
        .expect("the window let go of the pictures");
    assert_eq!(
        held.len(),
        3,
        "the opening read replaced the pass's pictures"
    );

    // And the search that runs on them still finds the copies.
    app.load_sets();
    settle(&mut app);
    assert_eq!(app.sets.len(), 1, "the search found nothing to review");
}

/// The point of the whole thing: a review is not lost by closing the window.
/// What was marked comes back marked, on the same sets, without the folder
/// being read or searched again.
#[test]
fn a_review_survives_the_folder_changing_and_being_scanned_again() {
    let found = folder_with_a_duplicate();
    let db_path = headless::default_db_path(found.path());
    let mut app = reviewing(found.path());
    let marked = app.sets[0].members[1].file_id;
    let ignored_pair = app.sets[0].set_id;
    app.selected = Some(marked);
    app.keep_selected();
    // And a set said not to be copies, which is the other half of what a
    // review is.
    let second = folder_with_a_duplicate();
    let _ = second;
    app.index.synced().expect("wait for the file");
    {
        let conn = db::open_and_migrate(&db_path).expect("read the index file");
        assert_eq!(
            db::kept(&conn).expect("marks"),
            vec![marked],
            "the mark never reached the index file"
        );
    }
    closed(&app);

    // Something added since, which is what this folder does every time: the
    // comparison finds a difference and a pass runs before the review opens.
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(30, 20, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    }))
    .save_with_format(found.path().join("new.png"), image::ImageFormat::Png)
    .expect("a fixture");

    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    again.auto_rescan = true;
    settle(&mut again);
    settle_the_question(&mut again);

    assert!(
        again.have_sets(),
        "the folder was not searched after the pass"
    );
    let still: Vec<i64> = again.keep.values().flat_map(Keep::marked).collect();
    assert_eq!(
        still,
        vec![marked],
        "the mark did not come back onto the sets the new search found"
    );
    assert_eq!(
        again.plan.files(),
        1,
        "what a cleanup would take did not come back"
    );
    let _ = ignored_pair;
}

/// A set somebody said is not a set of copies is part of the review too, and
/// comes back with it.
#[test]
fn an_ignored_set_comes_back_ignored_after_a_rescan() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    let set_id = app.sets[0].set_id;
    app.ignore_set(set_id);
    closed(&app);

    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(30, 20, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    }))
    .save_with_format(found.path().join("new.png"), image::ImageFormat::Png)
    .expect("a fixture");

    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    again.auto_rescan = true;
    settle(&mut again);
    settle_the_question(&mut again);

    let same = again
        .sets
        .iter()
        .find(|set| set.set_id == set_id)
        .expect("the set came back");
    assert!(
        again.is_ignored(same),
        "the set came back as a set of copies"
    );
}

#[test]
fn a_review_is_still_there_when_the_folder_is_opened_again() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let set_id = app.sets[0].set_id;
    let marked = app.sets[0].members[1].file_id;
    app.selected = Some(marked);
    app.keep_selected();
    closed(&app);

    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    settle_the_question(&mut again);

    assert_eq!(
        again.view,
        View::Review,
        "the window did not go back to the review"
    );
    assert!(again.running.is_none(), "the folder was scanned again");
    assert!(again.searching.is_none(), "the folder was searched again");
    assert_eq!(again.sets.len(), 1, "the sets did not come back");
    assert_eq!(again.sets[0].set_id, set_id, "a different set came back");
    assert_eq!(
        again.keep.get(&set_id),
        Some(&Keep::One(marked)),
        "the picture that was marked came back unmarked"
    );
    assert_eq!(
        again.plan.files(),
        1,
        "what a cleanup would take did not come back with it"
    );
}

/// A mark is answered when the copy in memory has it, not when the file does:
/// the file is caught up on a thread of its own. So the window has to close
/// the index on the way out, because closing it is what waits for that
/// thread. Ending without it is ending with the last of the review still on
/// the queue.
///
/// What is checked is that the window closed the index, not that the mark
/// happened to be there: on a small folder the writer wins that race anyway,
/// and a test that reads the file proves nothing about a folder where it does
/// not. That closing waits is `letting_go_of_a_folder_waits_for_the_file_to_
/// catch_up`, in the manager.
#[test]
fn the_window_closes_the_index_on_the_way_out() {
    let found = folder_with_a_duplicate();
    let db_path = headless::default_db_path(found.path());
    let mut app = reviewing(found.path());
    let marked = app.sets[0].members[1].file_id;
    app.selected = Some(marked);
    app.keep_selected();
    assert!(
        app.index.open_index_path().is_some(),
        "the folder was not open to begin with"
    );

    eframe::App::on_exit(&mut app, None);

    assert!(
        app.index.open_index_path().is_none(),
        "the window ended without closing the index, so whatever was still being written went \
         with it"
    );
    let conn = db::open_and_migrate(&db_path).expect("read the index file");
    assert_eq!(
        db::kept(&conn).expect("marks"),
        vec![marked],
        "the mark is not in the file"
    );
}

/// A file added since is a folder the stored sets do not describe, so the
/// person is asked rather than shown a review of what was there before.
/// Nothing happens until they answer.
#[test]
fn a_folder_that_changed_asks_before_its_saved_review_is_opened() {
    let found = folder_with_a_duplicate();
    let app = reviewing(found.path());
    closed(&app);

    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(30, 20, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    }))
    .save_with_format(found.path().join("new.png"), image::ImageFormat::Png)
    .expect("a fixture");

    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    settle_the_question(&mut again);

    assert_eq!(
        again.question,
        Some(Question::TheFolderChanged),
        "the folder changed and nothing was asked"
    );
    assert!(
        again.sets.is_empty(),
        "the review opened before anybody answered"
    );
    assert!(
        again.running.is_none(),
        "a pass started before anybody answered"
    );
    assert_eq!(again.view, View::Scan);
}

/// A folder set to rescan itself is asked about too, when there is a review
/// saved for it: the pass it asks for is what would throw that away. Saying
/// so opens the review, and the folder still rescans itself next time.
#[test]
fn a_folder_that_rescans_itself_with_nothing_to_rescan_opens_its_review() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let marked = app.sets[0].members[1].file_id;
    app.selected = Some(marked);
    app.keep_selected();
    app.keep_index = true;
    app.auto_rescan = true;
    app.remember_ways_of_matching();
    closed(&app);

    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    settle_the_question(&mut again);

    assert_eq!(
        again.question, None,
        "a pass with no work to do was asked about"
    );
    assert!(
        again.running.is_none(),
        "a pass ran over a folder that had not changed"
    );
    assert_eq!(again.view, View::Review, "the review did not open");
    assert!(
        again.auto_rescan,
        "the folder stopped asking to be rescanned"
    );
}

/// The box says what to do when something has moved: bring it up to date,
/// without asking, saved review or not.
#[test]
fn a_folder_that_rescans_itself_is_rescanned_when_something_changed() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    app.keep_index = true;
    app.auto_rescan = true;
    app.remember_ways_of_matching();
    closed(&app);

    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(30, 20, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    }))
    .save_with_format(found.path().join("new.png"), image::ImageFormat::Png)
    .expect("a fixture");

    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    settle_the_question(&mut again);

    assert_eq!(
        again.question, None,
        "the folder was asked about although the box is ticked"
    );
    assert_eq!(
        again.scan.total, 4,
        "the pass did not read the folder as it is now"
    );
}

/// With no saved review there is nothing to protect and nothing to ask
/// about, so a folder that has changed is brought up to date either way.
#[test]
fn a_changed_folder_with_no_saved_review_is_rescanned_without_asking() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    app.keep_index = true;
    app.give_up_the_saved_review();
    closed(&app);

    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(30, 20, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    }))
    .save_with_format(found.path().join("new.png"), image::ImageFormat::Png)
    .expect("a fixture");

    let mut again = App::from_settings(crate::settings::Settings::default());
    assert!(
        !again.auto_rescan,
        "the box was ticked before the folder was opened"
    );
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    settle_the_question(&mut again);

    assert_eq!(
        again.question, None,
        "a folder with nothing saved was asked about"
    );
    assert_eq!(
        again.scan.total, 4,
        "the pass did not read the folder as it is now"
    );
}

/// A folder with an index, nothing changed and no saved review was opened to
/// be searched, and the pictures are already in memory, so it is searched.
#[test]
fn an_unchanged_folder_with_no_saved_review_is_searched_without_a_pass() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    app.give_up_the_saved_review();
    closed(&app);

    let mut again = App::from_settings(crate::settings::Settings::default());
    again.open_folder(found.path().to_path_buf());
    settle(&mut again);
    settle_the_question(&mut again);

    assert!(
        again.running.is_none(),
        "a folder that had not changed was scanned"
    );
    assert_eq!(again.sets.len(), 1, "the folder was not searched");
}

/// Choosing a folder that has no index is not asking for anything to happen
/// to it. Nothing is compared, nothing is scanned, nothing is searched.
#[test]
fn a_folder_with_no_index_is_left_alone_when_it_is_chosen() {
    let dir = tempfile::tempdir().expect("tempdir");
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(30, 20, |x, y| {
        image::Rgb([(x % 256) as u8, (y % 256) as u8, 90])
    }))
    .save_with_format(dir.path().join("a.png"), image::ImageFormat::Png)
    .expect("a fixture");

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(dir.path().to_path_buf());
    settle(&mut app);
    settle_the_question(&mut app);

    assert!(app.running.is_none(), "choosing a folder scanned it");
    assert!(app.sets.is_empty(), "choosing a folder searched it");
    assert_eq!(app.question, None);
    assert_eq!(app.view, View::Scan);
}

/// Answering "rescan" gives the saved review up: the sets go out of the
/// index, and the pass the folder was asking for runs.
#[test]
fn answering_rescan_leaves_no_stored_sets_and_starts_a_pass() {
    let found = folder_with_a_duplicate();
    let mut app = reviewing(found.path());
    let db_path = headless::default_db_path(found.path());
    app.keep_index = true;

    app.give_up_the_saved_review();
    {
        let conn = index_file(&app, &db_path);
        assert!(
            db::stored_sets(&conn).expect("sets").is_empty(),
            "the sets nobody stands behind are still in the index"
        );
    }

    // And the pass runs, and the search after it writes down what it found,
    // which is what the index holds from then on.
    app.start_scan();
    settle(&mut app);
    assert_eq!(app.sets.len(), 1, "the pass and its search found nothing");
    let conn = index_file(&app, &db_path);
    assert_eq!(
        db::stored_sets(&conn).expect("sets").len(),
        1,
        "the new sets were not written down"
    );
}

/// A cleanup is the review being carried out, so the review is over: the
/// sets and the marks both go out of the index.
#[test]
fn a_cleanup_leaves_no_review_in_the_index() {
    let scanned = folder_with_a_duplicate();
    let db_path = headless::default_db_path(scanned.path());
    let mut app = reviewing(scanned.path());
    app.auto_mark_to_keep();
    app.keep_index = true;
    app.destination = Destination::Delete;
    let plan = app.plan.clone();
    assert_eq!(plan.files(), 1, "there was nothing for the cleanup to do");

    app.view = View::Cleanup;
    app.run_cleanup(&plan);
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while app.removing.is_some() && std::time::Instant::now() < until {
        app.pump_cleanup(&ctx);
    }

    let conn = index_file(&app, &db_path);
    assert!(
        db::stored_sets(&conn).expect("sets").is_empty(),
        "the sets outlived the cleanup"
    );
    assert!(
        db::kept(&conn).expect("marks").is_empty(),
        "the marks outlived the cleanup"
    );
}

/// What a cleanup would take is held rather than worked out where it is
/// drawn, so every interaction that changes it has to work it out again.
/// This is what catches one that forgot: after each, the held plan is
/// compared with one derived then and there.
#[test]
fn the_held_plan_follows_every_interaction() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    let in_step = |app: &App, after: &str| {
        let derived = app.build_plan();
        assert_eq!(
            (app.plan.files(), app.plan.bytes()),
            (derived.files(), derived.bytes()),
            "the plan was not worked out again after {after}"
        );
    };
    in_step(&app, "the search came back");

    app.selected = Some(app.sets[0].members[0].file_id);
    app.keep_selected();
    in_step(&app, "a mark went on");
    app.keep_selected();
    in_step(&app, "a mark came off");
    app.keep_only_selected();
    in_step(&app, "shift said which picture");
    app.auto_mark_to_keep();
    in_step(&app, "the best copies were marked");

    let set_id = app.sets[1].set_id;
    app.ignore_set(set_id);
    in_step(&app, "a set was ignored");
    app.unignore_set(set_id);
    in_step(&app, "a set was taken back");

    app.forget_members(&[app.sets[0].members[0].rel_path.clone()]);
    in_step(&app, "a picture was removed");

    app.cancel_work();
    in_step(&app, "the work was cancelled");
    app.start_scan();
    in_step(&app, "a pass started");
}

/// Drawing the review is not an interaction. The plan it draws from is the
/// one it was handed, and a frame does not change it.
#[test]
fn drawing_the_review_does_not_change_the_plan() {
    let found = folder_with_two_sets();
    let mut app = reviewing(found.path());
    app.auto_mark_to_keep();
    let before = (app.plan.files(), app.plan.bytes());

    let ctx = window();
    let input = egui::RawInput {
        screen_rect: Some(egui::Rect::from_min_size(
            egui::pos2(0.0, 0.0),
            egui::vec2(1200.0, 700.0),
        )),
        ..Default::default()
    };
    for _ in 0..3 {
        crate::shot::frame(
            "drawing_does_not_change_the_plan",
            &ctx,
            input.clone(),
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.review_view(ui));
            },
        );
    }
    assert_eq!(
        (app.plan.files(), app.plan.bytes()),
        before,
        "a frame changed the plan"
    );
}

/// A cleanup that took some of what it planned to leaves marks naming files
/// that are gone. Each surviving set's marks are cut down to what is still in
/// it, so no mark names a file that went and no `Several` holds one id.
#[test]
fn marks_do_not_outlive_the_pictures_a_cleanup_took() {
    // Three copies, so the set is still a set once one of them goes.
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.sets = vec![DuplicateSet {
        set_id: 1,
        members: vec![
            member(1, "a.jpg", 300),
            member(2, "b.jpg", 200),
            member(3, "c.jpg", 100),
        ],
    }];
    app.selected = Some(1);
    app.keep_selected();
    app.selected = Some(2);
    app.keep_selected();
    assert_eq!(app.keep.get(&1), Some(&Keep::Several(vec![1, 2])));

    app.forget_members(&[String::from("a.jpg")]);
    assert_eq!(
        app.keep.get(&1),
        Some(&Keep::One(2)),
        "a mark outlived the picture it was on"
    );
}

/// Run the window's own frame loop until the folder has been listed and
/// compared with its index, which is what decides between opening a saved
/// review and asking about it.
fn settle_the_question(app: &mut App) {
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < until {
        app.hear_the_index(&ctx);
        app.pump_indexer(&ctx);
        app.pump_search(&ctx);
        if app.asking.is_none() && app.running.is_none() && app.searching.is_none() {
            return;
        }
    }
    panic!("the folder was never decided about");
}

/// A folder searched in an earlier run and opened again with nothing added
/// to it. The review comes back out of the index without a search running,
/// and the bar that measures finding duplicates is full: that work was done,
/// in the past, and this open confirmed there is nothing new to do it to.
#[test]
fn the_duplicates_bar_is_full_for_a_review_that_came_back_from_the_index() {
    let found = folder_with_a_duplicate();
    let first = reviewing(found.path());
    assert!(
        !first.sets.is_empty(),
        "the first run found nothing to leave behind"
    );
    closed(&first);

    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(found.path().to_path_buf());
    settle_the_question(&mut app);

    assert!(
        app.question.is_none(),
        "the folder was asked about, so files had moved"
    );
    assert!(!app.sets.is_empty(), "the saved review did not come back");
    assert!(
        app.search.done,
        "the duplicates bar was emptied for work already done"
    );
}

/// The checkbox belongs to the folder that is open. Opening a different
/// folder resets it and the subfolder setting; opening the same folder again
/// leaves them alone; and opening a folder that contains an index ticks the
/// checkbox whatever it was before.
#[test]
fn the_checkbox_follows_the_folder_that_is_opened() {
    // Scan a folder so that it contains an index.
    let indexed = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(indexed.path().to_path_buf());
    app.keep_index = true;
    app.recurse = true;
    app.start_scan();
    settle(&mut app);

    let fresh = tempfile::tempdir().expect("tempdir");
    app.open_folder(fresh.path().to_path_buf());
    assert!(
        !app.keep_index,
        "the checkbox was carried over from the last folder"
    );
    assert!(
        !app.recurse,
        "the subfolder setting was carried over from the last folder"
    );

    app.keep_index = true;
    app.open_folder(fresh.path().to_path_buf());
    assert!(
        app.keep_index,
        "opening the same folder again cleared the checkbox"
    );

    app.keep_index = false;
    app.open_folder(indexed.path().to_path_buf());
    settle(&mut app);
    assert!(
        app.keep_index,
        "the folder contains an index but the checkbox was not ticked"
    );
}

/// A folder holding two pairs, so a real pass finds two sets.
fn folder_with_two_sets() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    for (name, seed) in [
        ("a1.png", 0),
        ("a2.png", 0),
        ("b1.png", 120),
        ("b2.png", 120),
    ] {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([
                ((x * 3 + seed) % 256) as u8,
                ((y * 5 + seed) % 256) as u8,
                40,
            ])
        }))
        .save_with_format(dir.path().join(name), image::ImageFormat::Png)
        .expect("a fixture");
    }
    dir
}

/// A window that has really scanned a folder and found what is in it.
fn reviewing(folder: &std::path::Path) -> App {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(folder.to_path_buf());
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);
    app
}

/// The window closing on a folder.
///
/// A caller is answered when the manager has its change, not when the disk
/// does, so a second window opened on the same folder in the same test can
/// otherwise read the file before the first one's writing has landed. Ending
/// the process is what waits for that, and this is a test saying the first
/// run ended.
fn closed(app: &App) {
    app.index.close().expect("close the index");
}

/// The folder's index as it is on disk, once the file has caught up with
/// what the window's manager holds. Tests read this rather than the manager
/// because the file is what the next run of the window opens.
fn index_file(app: &App, db_path: &std::path::Path) -> db::Connection {
    app.index.synced().expect("wait for the file");
    db::open_and_migrate(db_path).expect("read the index file")
}

/// A folder of pictures, two of them the same, so a real pass over it has
/// something to find.
fn folder_with_a_duplicate() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("tempdir");
    let picture = |seed: u32| {
        image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(64, 48, |x, y| {
            image::Rgb([((x * 3 + seed) % 256) as u8, ((y * 5) % 256) as u8, 40])
        }))
    };
    for (name, seed) in [("one.png", 0), ("two.png", 0), ("other.png", 90)] {
        picture(seed)
            .save_with_format(dir.path().join(name), image::ImageFormat::Png)
            .expect("a fixture");
    }
    dir
}

/// Run the window's own frame loop until the pass and the search it starts
/// have both finished, or give up rather than hang.
fn settle(app: &mut App) {
    let ctx = window();
    let until = std::time::Instant::now() + std::time::Duration::from_secs(20);
    while std::time::Instant::now() < until {
        // What the folder's index says about itself arrives on a thread of
        // its own, and may start a pass, so it is waited for as well.
        app.hear_the_index(&ctx);
        app.pump_indexer(&ctx);
        app.pump_search(&ctx);
        if app.asking.is_none() && app.running.is_none() && app.searching.is_none() {
            return;
        }
    }
    panic!("the pass never finished");
}

/// The window after really using it: a folder scanned, its duplicates found,
/// one of them chosen. Picking another folder leaves none of it behind.
#[test]
fn a_folder_picked_after_a_real_pass_leaves_nothing_of_the_last_one() {
    let scanned = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());

    app.open_folder(scanned.path().to_path_buf());
    app.start_scan();
    settle(&mut app);
    assert_eq!(app.scan.done, 3, "the pass did not read the folder");
    assert_eq!(app.scan.indexed, 3, "the pass did not index the folder");

    app.load_sets();
    settle(&mut app);
    assert_eq!(app.sets.len(), 1, "the two copies were not found");
    app.selected = app.sets[0].members.first().map(|member| member.file_id);
    assert!(app.selected.is_some());

    let next = tempfile::tempdir().expect("tempdir");
    app.open_folder(next.path().to_path_buf());

    assert_eq!(app.folder.as_deref(), Some(next.path()));
    assert!(
        app.sets.is_empty(),
        "the sets from the last folder are still here"
    );
    assert_eq!(app.selected, None);
    assert_eq!(
        app.scan.done, 0,
        "the last folder's counters are still on screen"
    );
    assert_eq!(app.scan.indexed, 0);
    assert_eq!(app.scan.total, 0);
    assert_eq!(
        app.scan.finished, None,
        "the last folder's outcome is still on screen"
    );
    assert!(
        app.running.is_none(),
        "a folder with no index was scanned unasked"
    );
}

/// The scan button pressed, and the window painted before a single file has
/// been counted. A bar at nothing paints nothing: a rounded cap around an
/// empty span is still a bubble on screen saying work has begun.
#[test]
fn a_bar_with_nothing_done_paints_no_fill_at_all() {
    let folder = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(folder.path().to_path_buf());
    app.start_scan();
    assert_eq!(app.scan.done, 0, "the pass had already counted something");
    assert_eq!(app.scan.total, 0);

    fn fills(app: &mut App) -> Vec<egui::Rect> {
        let ctx = window();
        let fill = ctx.style().visuals.selection.bg_fill;
        let shapes = crate::shot::frame(
            "a_bar_with_nothing_done",
            &ctx,
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(700.0, 400.0),
                )),
                ..Default::default()
            },
            |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| app.progress_section(ui));
            },
        );
        shapes
            .into_iter()
            .filter_map(|clipped| match clipped.shape {
                egui::Shape::Rect(rect) if rect.fill == fill => Some(rect.rect),
                _ => None,
            })
            .collect()
    }

    let empty = fills(&mut app);
    assert!(
        empty.is_empty(),
        "a bar with no progress painted {} filled rects: {empty:?}",
        empty.len()
    );

    settle(&mut app);
    assert_eq!(app.scan.done, app.scan.total, "the pass did not finish");
    assert_eq!(
        fills(&mut app).len(),
        3,
        "the read, indexed and duplicates bars are not all filled once the pass is done"
    );
}

/// The Finding duplicates is the stages that are going to happen and no
/// others. A search straight after a pass reads no index, because the pass
/// built what it searches, and a bar that gave that stage a share of itself
/// would open a third full for work nobody is going to do.
#[test]
fn a_search_that_reads_no_index_does_not_start_its_bar_part_full() {
    // What a search after a pass reports first: everything is in memory
    // already, so it says how many pictures there are and starts work.
    let after_a_pass = SearchState {
        stage: Some("comparing"),
        loaded: 900,
        to_load: 900,
        ..SearchState::default()
    };
    let (done, of) = after_a_pass.progress();
    assert_eq!(
        done, 0,
        "the bar opened {done} of {of} before anything was done"
    );

    // The same search once the shortlist is half drawn up: half of the
    // first of its two stages.
    let halfway = SearchState {
        shortlisted: 450,
        to_shortlist: 900,
        ..after_a_pass.clone()
    };
    let (done, of) = halfway.progress();
    assert_eq!(
        (done, of),
        (500, 2000),
        "half the shortlist is not a quarter of the bar"
    );

    // A search that does read the index has three stages, and reading half
    // of it is half of the first of them.
    let reading = SearchState {
        stage: Some("reading the index"),
        reads_the_index: true,
        loaded: 450,
        to_load: 900,
        ..SearchState::default()
    };
    let (done, of) = reading.progress();
    assert_eq!(
        (done, of),
        (500, 3000),
        "half the reading is not a sixth of the bar"
    );
}

/// A folder whose index already holds every file in it is fully read and
/// fully indexed, and both bars say so. Nothing has to run for that to be
/// true, and a bar left empty because no work happened is a lie about a
/// folder that is entirely done.
#[test]
fn a_pass_with_nothing_left_to_do_fills_both_bars_anyway() {
    let folder = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(folder.path().to_path_buf());
    app.start_scan();
    settle(&mut app);

    // Nothing has changed in the folder, so this pass reads nothing.
    app.start_scan();
    settle(&mut app);
    assert!(
        app.scan.unchanged > 0,
        "the second pass read the folder again"
    );
    assert_eq!(app.scan.reading, Stage::Over, "the read bar is not full");
    assert_eq!(app.scan.writing, Stage::Over, "the indexed bar is not full");
}

/// The listing is not the reading. A pass that is still working out what is
/// in the folder has read nothing, and the bar for reading says nothing.
#[test]
fn listing_the_folder_does_not_move_the_read_bar() {
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.scan.listing = Some(4000);
    app.scan.done = 0;
    app.scan.total = 0;
    assert_eq!(
        app.scan.reading,
        Stage::Waiting,
        "the reading had not begun"
    );

    // What the pass says when the listing is over: a total to measure the
    // reading against, and nothing read yet.
    app.scan.total = 4000;
    assert_eq!(
        app.scan.reading,
        Stage::Waiting,
        "a total to read is not the same as having read any of it"
    );
}

/// A different folder is different pictures, so what counted as a duplicate
/// in the last one does not carry over. Opening the same folder again leaves
/// the setting alone.
#[test]
fn a_different_folder_starts_on_the_default_setting() {
    // A folder really scanned at the top of the scale, with a cleanup
    // destination chosen for it.
    let found = folder_with_a_duplicate();
    let mut app = App::from_settings(crate::settings::Settings::default());
    app.open_folder(found.path().to_path_buf());
    app.sensitivity = matching::MAX_SENSITIVITY;
    app.ignore_colour = true;
    app.destination = Destination::Delete;
    app.start_scan();
    settle(&mut app);
    app.load_sets();
    settle(&mut app);
    assert!(!app.sets.is_empty(), "the pass found nothing to review");

    let dir = tempfile::tempdir().expect("tempdir");
    app.open_folder(dir.path().to_path_buf());
    assert_eq!(
        app.sensitivity,
        matching::DEFAULT_SENSITIVITY,
        "the last folder's sensitivity was carried over"
    );
    assert!(
        !app.ignore_colour,
        "the last folder's colour setting was carried over"
    );
    assert_eq!(
        app.destination,
        Destination::Trash,
        "the last folder's cleanup choice was carried over"
    );
    assert!(
        app.move_dir.is_empty(),
        "the last folder's move folder was carried over"
    );

    app.sensitivity = matching::MAX_SENSITIVITY;
    app.open_folder(dir.path().to_path_buf());
    assert_eq!(
        app.sensitivity,
        matching::MAX_SENSITIVITY,
        "opening the same folder again threw away the setting chosen for it"
    );
}

#[test]
fn no_saved_settings_leaves_the_window_empty() {
    let app = App::from_settings(crate::settings::Settings::default());
    assert_eq!(app.folder, None);
    assert_eq!(app.db_path, None);
    assert!(
        !app.recurse,
        "a folder is the folder, not everything under it"
    );
}

/// The row says which preset the setting is on, and only that one. A setting
/// between two of them lights up neither.
#[test]
fn the_preset_row_marks_the_one_the_slider_is_on() {
    for (name, percent) in matching::PRESETS {
        let lit: Vec<&str> = matching::PRESETS
            .iter()
            .filter(|(_, other)| on_preset(percent, *other))
            .map(|(other, _)| *other)
            .collect();
        assert_eq!(lit, vec![name], "{name} at {percent} lit up {lit:?}");
    }

    let between = 20.0;
    assert!(
        !matching::PRESETS
            .iter()
            .any(|(_, percent)| on_preset(between, *percent)),
        "a setting between the presets was drawn as one of them"
    );
    assert!(
        matching::PRESETS
            .iter()
            .all(|(_, percent)| *percent <= matching::MAX_SENSITIVITY),
        "a preset sits past the end of the slider"
    );
}

#[test]
fn the_slider_widens_what_counts_as_a_duplicate() {
    assert!(Thresholds::at(4.0).max_bits < Thresholds::at(30.0).max_bits);
    assert!(Thresholds::at(30.0).max_bits < Thresholds::at(50.0).max_bits);
    assert!(Thresholds::at(4.0).max_ring < Thresholds::at(50.0).max_ring);
}

#[test]
fn the_app_starts_on_the_default_setting() {
    // Not `App::default`, which reads whatever this machine was last left
    // set to and would pass or fail depending on it.
    let app = App::from_settings(crate::settings::Settings::default());
    assert_eq!(app.sensitivity, matching::DEFAULT_SENSITIVITY);
    assert!(matching::DEFAULT_SENSITIVITY <= matching::MAX_SENSITIVITY);
}

#[test]
fn a_row_fills_the_width_it_is_given() {
    let content = [200.0, 320.0, 180.0];
    let available = 1067.0;
    let widths = share_row_width(available, &content, SECTION_GAP);
    let used: f32 = widths.iter().map(|w| w + FRAME_EXTRA).sum::<f32>() + SECTION_GAP * 2.0;
    assert!((used - available).abs() < 0.5, "used {used} of {available}");
}

#[test]
fn every_box_gets_the_same_share_of_the_leftover() {
    let content = [200.0, 320.0, 180.0];
    let widths = share_row_width(1067.0, &content, SECTION_GAP);
    let shares: Vec<f32> = widths
        .iter()
        .zip(content.iter())
        .map(|(width, natural)| width - natural)
        .collect();
    assert!((shares[0] - shares[1]).abs() < 0.5, "{shares:?}");
    assert!((shares[1] - shares[2]).abs() < 0.5, "{shares:?}");
    assert!(shares[0] > 0.0, "nothing was shared out");
}

#[test]
fn a_wider_box_stays_wider_than_a_narrow_one() {
    // Sharing the leftover equally keeps the differences between the boxes,
    // which is what splitting the row into equal columns threw away.
    let content = [200.0, 320.0, 180.0];
    let widths = share_row_width(1067.0, &content, SECTION_GAP);
    assert!(widths[1] > widths[0]);
    assert!(widths[0] > widths[2]);
}

#[test]
fn a_row_too_narrow_for_its_content_shares_nothing() {
    let content = [200.0, 320.0, 180.0];
    let widths = share_row_width(300.0, &content, SECTION_GAP);
    assert_eq!(widths, content.to_vec());
}

#[test]
fn cleanup_starts_on_the_recycle_bin() {
    assert_eq!(App::default().disposal(), Disposal::Trash);
}

#[test]
fn exactly_one_destination_is_selected_at_a_time() {
    // Held as one value, so the three cannot all read as chosen. They were
    // three separate booleans and did.
    let mut app = App::default();
    for choice in [Destination::Trash, Destination::MoveTo, Destination::Delete] {
        app.destination = choice;
        let selected = [Destination::Trash, Destination::MoveTo, Destination::Delete]
            .iter()
            .filter(|other| **other == app.destination)
            .count();
        assert_eq!(selected, 1, "{:?} did not select exactly one", choice);
    }
}

#[test]
fn each_destination_maps_to_what_the_cleanup_layer_expects() {
    let mut app = App::default();
    app.destination = Destination::Delete;
    assert_eq!(app.disposal(), Disposal::Delete);

    app.destination = Destination::MoveTo;
    app.move_dir = String::from("held");
    assert_eq!(app.disposal(), Disposal::MoveTo(PathBuf::from("held")));
}

#[test]
fn every_destination_has_a_label_and_a_note() {
    for choice in [Destination::Trash, Destination::MoveTo, Destination::Delete] {
        assert!(!choice.label().is_empty());
        assert!(!choice.note().is_empty());
    }
    assert!(
        Destination::Delete.note().contains("cannot be undone"),
        "the permanent option does not say so"
    );
}
