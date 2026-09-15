# Issue 33: move all UI-relevant constants into one file per crate

Issue text: "Things like colours, label strings, defaults, etc. should all live in a single file that is trivially editable by anyone who wants to fix a typo, or improve colour contrast, etc. etc. We can have one file for core and one for the main app, but we should not be sprinkling UI-relevant constants all over the place in dozens of files."

This is a refactor. Nothing the program does changes, and nothing the tests check changes.

## Names

Every constant in the two files is named so that someone reading `constants.rs` can tell what it is for without looking at the code that uses it: what it belongs to, and what about that thing it sets. A name that already says that, such as `TOOLBAR_BUTTON_GAP`, stays as it is. A name that does not, such as `SIZE`, is renamed. New constants follow the same rule.

- One constant holds one value. A constant that holds two things (a width and a height, a border and a margin) is split into one constant per thing, and the code that used the combined value adds them up where it is used.
- A size says which dimension it is: width, height, radius, or per-side margin.
- "scrollbar" is one word.

Renaming or splitting a constant that a test refers to changes that name, or that sum, in the test, and nothing else in it.

## The two files

- `crates/imgdedupe/src/constants.rs`, declared as `mod constants;` in `crates/imgdedupe/src/main.rs`.
- `crates/imgdedupe-core/src/constants.rs`, declared as `pub mod constants;` in `crates/imgdedupe-core/src/lib.rs`.

Both are plain Rust: `const` items and nothing else. No functions, no build script, no data file.

## What goes in

Colours, sizes, margins, gaps, opacities, text size and spacing, window and panel sizes, repaint intervals, default values of settings the user can change, and user-visible label strings.

A user-visible string that is a `format!` template stays a template where it is. The fixed words in it that stand on their own (for example `"cancelled"`, `"reading..."`) become constants.

## What stays where it is

- Metadata tag and value labels in core. Metadata is an independent specification, not this program's UI.
- Error texts from core. They are not UI labels or colours.
- Storage keys: the `notes.rs` keys, `Destination::name`, `INDEX_FILENAME`.
- Log lines and panic messages.
- The debug command line tool's help text.
- The window icon's geometry and colours.
- Thumbnail decode sizes and worker counts.
- Algorithm constants in core.

## Keeping the tests as they are

Where a test reaches a constant through its old module, that module brings it into scope from `constants.rs`, so the test's `use` lines stay as they are:

- `app/mod.rs` pulls in `crate::constants::*`, so `tests/app.rs` finds the layout constants through `use super::*`.
- `fonts.rs` brings the font size into scope for `tests/fonts.rs`.
- `matching/mod.rs` re-exports the sensitivity constants, so `tests/matching.rs`, `tests/app.rs` and every `imgdedupe_core::matching::` path keep working.

Strings that tests compare word for word keep exactly the same text.

## Renames of existing constants

| Now | Becomes | What it is |
|---|---|---|
| `SCROLL_BAR` (12.0) | `SCROLLBAR_STRIP_WIDTH` (12.0) | Width of the strip a scrollbar sits in |
| `SECTION_GAP` (14.0) | `SECTION_SPACING_GAP` (14.0) | Space between the sections of a page |
| `FRAME_EXTRA` (14.0) | `FRAME_GROUP_BORDER_WIDTH` (1.0) and `FRAME_GROUP_INNER_MARGIN` (6.0) | Width of the line `Frame::group` draws, and the margin it keeps inside that line on each side. The old value is both of these on both sides. |
| `TILE` (156.0, 118.0) | `THUMBNAIL_MAX_WIDTH` (156.0) and `THUMBNAIL_MAX_HEIGHT` (118.0) | Widest and tallest a thumbnail is drawn |
| `TILE_RING` (6.0) | `THUMBNAIL_CLEARANCE` (6.0) | Space kept clear on each side of a thumbnail so the selection ring is not clipped |
| `TILE_BORDER` (4.0) | `THUMBNAIL_INNER_MARGIN` (2.0) | Margin between a thumbnail and its border, on each side. The old value is both sides together. |
| `BUTTON_ROW_GAP` (2.0) | `SET_BUTTON_BAND_VERTICAL_PADDING` (2.0) | Space above and below the buttons in the band along the bottom of a set |
| `BUTTON_ROW_INSET` (5.0) | `SET_BUTTON_BAND_LEFT_INSET` (5.0) | Distance from the left edge of a set's box to its first button |
| `BUTTON_ROW_BACKGROUND` | `SET_BUTTON_BAND_BACKGROUND_COLOUR` | Colour of the band along the bottom of a set |
| `PAGE_MARGIN` (16.0) | `CONTENT_MARGIN` (16.0) | Space between a page's content and the window edge, and between a list's content and its scrollbar |
| `PREVIOUS_ROOM` (84.0) | `PREVIOUS_BUTTON_WIDTH` (84.0) | Width kept free at the right of the folder row for the "previous" button |
| `TOOLBAR_HEIGHT` (28.0) | `TOOLBAR_ROW_HEIGHT` (28.0) | Height of the review page's toolbar row |
| `TOOLBAR_BUTTON_GAP` (14.0) | `TOOLBAR_BUTTON_GAP` (14.0) | Unchanged |
| `CLEANUP_BUTTON_WIDTH` (120.0) | `CLEANUP_BUTTON_WIDTH` (120.0) | Unchanged |
| `PRESET_PADDING` (6.0, 2.0) | `SMALL_BUTTON_HORIZONTAL_PADDING` (6.0) and `SMALL_BUTTON_VERTICAL_PADDING` (2.0) | Padding around the text of the sensitivity preset buttons and a set's "keep all", "keep none" and "ignore" buttons |
| `VALUE_WIDTH` (56.0) | `SENSITIVITY_PERCENTAGE_BOX_WIDTH` (56.0) | Width of the percentage box beside the sensitivity slider |
| `STRIP_TO_BAR` (6.0) | `SET_TEXT_TO_SCROLLBAR_GAP` (6.0) | Space between the text under a set's thumbnails and the set's scrollbar |
| `BETWEEN_BOXES` (12.0) | `SET_BOX_VERTICAL_GAP` (12.0) | Space between one set's box and the next |
| `IGNORED_OPACITY` (0.25) | `IGNORED_SET_OPACITY` (0.25) | Opacity of the thumbnails and text of an ignored set |
| `BOX_EDGE` (1.0) | `SET_BOX_BORDER_WIDTH` (1.0) | Width of the line around a set's box |
| `BOX_PADDING` (6.0) | `SET_BOX_INNER_PADDING` (6.0) | Space between the edge of a set's box and its thumbnails |
| `SIZE` (fonts.rs, 16.0) | `FONT_SIZE` (16.0) | Size of all text in the window |
| `RED` (scan_page.rs) | `LAMP_WAITING_COLOUR` | Colour of a scan page lamp for a step that has not happened yet |
| `GREEN` (scan_page.rs) | `LAMP_DONE_COLOUR` | Colour of a scan page lamp for a step that has happened |
| `GREY` (scan_page.rs) | `LAMP_SKIPPED_RING_COLOUR` | Colour of the ring for a step there was nothing to do for |
| `DOT` (scan_page.rs, 5.0) | `LAMP_DOT_RADIUS` (5.0) | Radius of a scan page lamp |
| `MAX_SENSITIVITY` (core, 50.0) | `SENSITIVITY_SLIDER_MAX_PERCENT` (50.0) | Highest percentage the sensitivity slider goes to |
| `DEFAULT_SENSITIVITY` (core, 15.0) | `SENSITIVITY_SLIDER_DEFAULT_PERCENT` (15.0) | Percentage the sensitivity slider starts on |
| `PRESETS` (core) | `SENSITIVITY_SLIDER_PRESETS` | Names and percentages of the preset buttons under the sensitivity slider |

## Procedure for each step

1. Make the move.
2. Run `scripts\test.bat` and match the baseline: 191 (core), 211, 192 (app), 3, 3, 0.
3. Run `scripts\build.bat --test`, then `scripts\build.bat`.
4. Report the test results and ask permission.
5. On a yes: tick the checkbox, run `git add .`, then `git commit -m "<step name>"`.

## Steps

- [x] 1. App `constants.rs`: create it and move the 21 constants from `app/widgets.rs` into it, renamed as in the table above.
- [x] 2. Font size and style spacing: `SIZE` from `fonts.rs` as `FONT_SIZE`, and the values set in `fonts::set_sizes` as `BUTTON_HORIZONTAL_PADDING` (12.0), `BUTTON_VERTICAL_PADDING` (7.0), `WIDGET_HORIZONTAL_SPACING` (9.0), `WIDGET_VERTICAL_SPACING` (7.0) and `CONTROL_MIN_HEIGHT` (26.0).
- [x] 3. Colours: every colour literal in `app/mod.rs`, `app/scan_page.rs`, `app/review_page.rs` and `app/cleanup_page.rs` becomes a named constant: `ERROR_MESSAGE_TEXT_COLOUR` rgb(200, 80, 80), `CLEANUP_BUTTON_FILL_COLOUR` rgb(60, 110, 180), `PERMANENT_DELETE_BUTTON_FILL_COLOUR` rgb(150, 50, 50), `CLEANUP_BUTTON_TEXT_COLOUR` white, `KEPT_THUMBNAIL_MARK_COLOUR` rgb(90, 180, 110) for the keep border and the "KEEP" label, `FULL_WINDOW_PICTURE_BACKDROP_COLOUR` black alpha 240, and the lamp colours and dot radius renamed as in the table above, with `LAMP_SKIPPED_RING_WIDTH` (1.5) for the ring's line. Each value used in more than one place becomes one constant.
- [x] 4. Sizes, gaps and timings: the inline layout numbers and repaint intervals in `app/mod.rs`, `app/scan_page.rs`, `app/review_page.rs`, `app/cleanup_page.rs`, `app/searching.rs` and `app/widgets.rs`. That includes the window sizes, panel and field widths, button sizes, grid spacings, the `add_space` gaps, the preview pane fractions, the stroke widths, and the 50 ms and 100 ms repaint intervals. A literal equal to an existing constant with the same meaning, such as the 16.0 page margin in `app/mod.rs`, uses that constant.
- [x] 5. App defaults: the default values set in `App::from_settings`, in `open_folder` in `app/scan_page.rs`, and in `settings.rs` `impl Default`.
- [x] 6. App label strings: the button labels, section titles, tab names, checkbox labels, status words and messages in the page files, `app/state.rs` (`Destination::label`, `note`, `verb`, the previous session question, `LAMPS`) and `app/widgets.rs`.
- [x] 7. Core `constants.rs`: create it and move `MAX_SENSITIVITY`, `DEFAULT_SENSITIVITY` and `PRESETS` from `matching/mod.rs` into it, renamed as in the table above, and the default switch values used in `Thresholds::at`.
- [ ] 8. Docs: update anything in `docs/` that names these constants or where they live.
