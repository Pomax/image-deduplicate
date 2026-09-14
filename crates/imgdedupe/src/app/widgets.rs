use super::*;

/// Where the list has to be scrolled to for one row to sit in the middle of what
/// is on screen, or nothing when it is already there.
///
/// The row walked to is always brought to the middle, so the rows on either side
/// of it are in sight: walking forward shows what is coming and walking back
/// shows what has been passed. The ends of the list are the only exception, since
/// there is nothing beyond them to scroll into view.
///
/// The rows the list is not drawing cannot be asked to scroll themselves into
/// view, so the place of the one wanted is worked out from its number: every row
/// is the same height, and they are laid out one after another.
pub(super) fn scroll_to_show(
    row: usize,
    rows: usize,
    row_height: f32,
    spacing: f32,
    offset: f32,
    viewport: f32,
) -> Option<f32> {
    let pitch = row_height + spacing;
    let content = (rows as f32 * pitch - spacing).max(0.0);
    let middle = row as f32 * pitch + row_height / 2.0 - viewport / 2.0;
    let wanted = middle.clamp(0.0, (content - viewport).max(0.0));
    ((wanted - offset).abs() > 0.5).then_some(wanted)
}

/// Width of the strip a scroll bar sits in, and of the handle that fills it.
pub(super) const SCROLL_BAR: f32 = 12.0;

/// The triangle on a scroll bar's end button, pointing at the end it scrolls to.
fn arrow(
    painter: &egui::Painter,
    button: egui::Rect,
    towards: f32,
    down: bool,
    ink: egui::Color32,
) {
    let middle = button.center();
    let reach = button.width().min(button.height()) * 0.26;
    let along = |amount: f32| {
        if down {
            egui::vec2(0.0, amount)
        } else {
            egui::vec2(amount, 0.0)
        }
    };
    let across = |amount: f32| {
        if down {
            egui::vec2(amount, 0.0)
        } else {
            egui::vec2(0.0, amount)
        }
    };
    painter.add(egui::Shape::convex_polygon(
        vec![
            middle + along(reach * towards),
            middle + along(-reach * towards) + across(-reach),
            middle + along(-reach * towards) + across(reach),
        ],
        ink,
        egui::Stroke::NONE,
    ));
}

/// Show something that scrolls, with the bar this application draws rather than
/// the toolkit's: a strip taken out of the space, a button at each end of it, and
/// a handle between them.
///
/// `step` is how far one click of an end button moves. `wheel_over` is a
/// rectangle a wheel turned anywhere inside counts as a turn on this list, for a
/// list that is one part of a larger pane. Gives back what was shown, how far
/// along it is, how much of it is on screen, and how much of it there is.
pub(super) fn scrolled<R>(
    ui: &mut egui::Ui,
    id: egui::Id,
    down: bool,
    step: f32,
    wheel_over: Option<egui::Rect>,
    area: egui::ScrollArea,
    show: impl FnOnce(egui::ScrollArea, &mut egui::Ui) -> egui::scroll_area::ScrollAreaOutput<R>,
) -> (R, f32, f32, f32) {
    let room = ui.available_rect_before_wrap();
    let (content_rect, strip) = if down {
        (
            // A gap before the bar as well as the bar itself, so what is in the
            // list stops as far from it as the list stops from the window's edge
            // rather than running up against it.
            room.with_max_x(room.right() - SCROLL_BAR - PAGE_MARGIN),
            egui::Rect::from_min_max(egui::pos2(room.right() - SCROLL_BAR, room.top()), room.max),
        )
    } else {
        (
            room.with_max_y(room.bottom() - SCROLL_BAR),
            egui::Rect::from_min_max(
                egui::pos2(room.left(), room.bottom() - SCROLL_BAR),
                room.max,
            ),
        )
    };

    let pending: Option<f32> = ui.data_mut(|data| data.remove_temp(id));
    let mut area = area.scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
    if let Some(offset) = pending {
        area = if down {
            area.vertical_scroll_offset(offset)
        } else {
            area.horizontal_scroll_offset(offset)
        };
    }

    let output = ui
        .allocate_new_ui(egui::UiBuilder::new().max_rect(content_rect), |ui| {
            // Nothing inside is drawn outside: a list is a window onto its
            // rows, and a row that came out wider than the room it was given
            // paints over whatever is beside the list rather than being cut off
            // at its edge. Cut at the bar rather than at the content, because a
            // line drawn round something is drawn half on either side of its
            // edge, and a clip along that edge takes half the line with it.
            // The whole of the room the list was given, up to the bar: a box in
            // it is drawn with a line round it, and a line sits half on either
            // side of the edge it marks, so a clip along the content itself
            // takes the top off the first box and the bottom off the last.
            let cut = if down {
                egui::Rect::from_min_max(
                    egui::pos2(room.left(), room.top()),
                    egui::pos2(strip.left(), room.bottom()),
                )
            } else {
                egui::Rect::from_min_max(
                    egui::pos2(room.left(), room.top()),
                    egui::pos2(room.right(), strip.top()),
                )
            };
            ui.set_clip_rect(cut.intersect(ui.clip_rect()));
            show(area, ui)
        })
        .inner;

    // Alongside what it scrolls, and nailed to it. The bar marks the room the
    // list is drawn in, which is the same rectangle from one frame to the next:
    // it does not move because the list moved under it. That rectangle is the
    // one the area was given, not the one the rows happen to be at, which is
    // exactly what scrolling changes.
    let strip = if down {
        egui::Rect::from_min_max(
            egui::pos2(strip.left(), content_rect.top()),
            egui::pos2(strip.right(), content_rect.bottom()),
        )
    } else {
        egui::Rect::from_min_max(
            egui::pos2(content_rect.left(), strip.top()),
            egui::pos2(content_rect.right(), strip.bottom()),
        )
    };

    let axis = usize::from(down);
    let content = output.content_size[axis];
    let viewport = output.inner_rect.size()[axis];
    let offset = output.state.offset[axis];
    // The bar first, so a drag on it beats a wheel turned at the same time.
    let wanted = paint_scroll_bar(ui, strip, down, step, content, viewport, offset).or_else(|| {
        let pane = wheel_over?;
        let (wheel, pointer) =
            ui.input(|input| (input.smooth_scroll_delta, input.pointer.latest_pos()));
        let at = pointer?;
        // Over the list itself the toolkit has already applied the wheel, and
        // applying it again here is one turn counted twice.
        if wheel[axis] == 0.0 || !pane.contains(at) || output.inner_rect.contains(at) {
            return None;
        }
        let moved = (offset - wheel[axis]).clamp(0.0, (content - viewport).max(0.0));
        (moved != offset).then_some(moved)
    });
    if let Some(wanted) = wanted {
        ui.data_mut(|data| data.insert_temp(id, wanted));
        ui.ctx().request_repaint();
    }
    (output.inner, offset, viewport, content)
}

/// Draw the bar for one scrolling area, and say where a click or a drag on it
/// wants that area to be.
pub(super) fn paint_scroll_bar(
    ui: &egui::Ui,
    strip: egui::Rect,
    down: bool,
    step: f32,
    content: f32,
    viewport: f32,
    offset: f32,
) -> Option<f32> {
    if viewport <= 0.0 || content <= viewport + 0.5 {
        return None;
    }
    let furthest = content - viewport;
    let thickness = if down { strip.width() } else { strip.height() };
    let length = if down { strip.height() } else { strip.width() };

    let end = |from_start: f32| {
        if down {
            egui::Rect::from_min_size(
                egui::pos2(strip.left(), strip.top() + from_start),
                egui::vec2(thickness, thickness),
            )
        } else {
            egui::Rect::from_min_size(
                egui::pos2(strip.left() + from_start, strip.top()),
                egui::vec2(thickness, thickness),
            )
        }
    };
    let first = end(0.0);
    let last = end(length - thickness);
    let track_length = (length - thickness * 2.0).max(1.0);

    let ink = ui.visuals().text_color();
    let painter = ui.painter();
    painter.rect_filled(strip, 0.0, ui.visuals().extreme_bg_color);
    // Outlined on every side, so the bar reads as a part of the window rather
    // than as a strip of another colour left on the side of it, and so it ends
    // somewhere rather than running off the edge. Half a point in, because a
    // line is drawn either side of where it is put and the outer half of it
    // would be clipped away.
    painter.rect_stroke(
        strip.shrink(0.5),
        0.0,
        egui::Stroke::new(1.0_f32, ui.visuals().widgets.noninteractive.bg_stroke.color),
    );
    arrow(painter, first, -1.0, down, ink);
    arrow(painter, last, 1.0, down, ink);

    let handle_length = (track_length * viewport / content)
        .max(thickness * 2.0)
        .min(track_length);
    let travel = track_length - handle_length;
    let at = (offset / furthest).clamp(0.0, 1.0);
    let handle_start = thickness + at * travel;
    let handle = if down {
        egui::Rect::from_min_size(
            egui::pos2(strip.left(), strip.top() + handle_start),
            egui::vec2(thickness, handle_length),
        )
    } else {
        egui::Rect::from_min_size(
            egui::pos2(strip.left() + handle_start, strip.top()),
            egui::vec2(handle_length, thickness),
        )
    };
    painter.rect_filled(handle, 2.0, ink);

    let base = ui
        .id()
        .with(("scroll bar", strip.left() as i32, strip.top() as i32));
    let stepped = |by: f32| Some((offset + by).clamp(0.0, furthest));
    if ui
        .interact(first, base.with("first"), egui::Sense::click())
        .clicked()
    {
        return stepped(-step);
    }
    if ui
        .interact(last, base.with("last"), egui::Sense::click())
        .clicked()
    {
        return stepped(step);
    }

    let track = ui.interact(strip, base.with("track"), egui::Sense::click_and_drag());
    let grip = base.with("grip");

    // Nothing is being pressed, so there is no grip to hold on to. Clearing it
    // here is what makes the next press take a fresh one.
    let Some(pointer) = track.interact_pointer_pos() else {
        ui.data_mut(|data| data.remove_temp::<f32>(grip));
        return None;
    };
    let along = if down {
        pointer.y - strip.top()
    } else {
        pointer.x - strip.left()
    };

    // Where on the handle the pointer went down, kept for as long as it is held.
    // Taken on the first frame of the press: a drag has not started yet then, and
    // a click is not reported until the button comes back up, so waiting for
    // either of those is what made the handle jump under the pointer.
    let held = ui
        .data_mut(|data| data.get_temp::<f32>(grip))
        .unwrap_or_else(|| {
            let handle_start = thickness + (offset / furthest).clamp(0.0, 1.0) * travel;
            let on_handle = along >= handle_start && along <= handle_start + handle_length;
            // A press on the track outside the handle has nowhere to hold, so it
            // jumps, and the handle arrives centred on the pointer.
            let from_start = if on_handle {
                along - handle_start
            } else {
                handle_length / 2.0
            };
            ui.data_mut(|data| data.insert_temp(grip, from_start));
            from_start
        });

    let wanted = ((along - thickness - held) / travel.max(1.0)).clamp(0.0, 1.0);
    Some(wanted * furthest)
}

/// How the window looks and what it lets the pointer do.
///
/// Scrolling gets a strip of `SCROLL_BAR` points at the edge of anything that
/// scrolls, with the handle filling it when there is something to scroll.
/// `solid` is the preset that takes that space rather than floating over the
/// content. Its handle is drawn in the widget background colour, which is pale
/// grey on a near white track and comes out invisible; the foreground colour is
/// what makes a handle that can be seen.
pub(super) fn install_style(ctx: &egui::Context) {
    // Both of them. `style_mut` changes the theme in use at the time, and the
    // window is handed the machine's theme after this runs, which would leave the
    // other one as egui ships it.
    ctx.all_styles_mut(|style| {
        style.spacing.scroll = egui::style::ScrollStyle::solid();
        style.spacing.scroll.bar_width = SCROLL_BAR;
        style.spacing.scroll.bar_inner_margin = 0.0;
        style.spacing.scroll.bar_outer_margin = 0.0;
        style.spacing.scroll.foreground_color = true;
        // Nothing here is a text field. Labels are what the window says, not
        // something to drag a cursor through.
        style.interaction.selectable_labels = false;
    });
}

/// Spacing used between the sections of a view, so they are consistent.
pub(super) const SECTION_GAP: f32 = 14.0;

/// Border and inner margin `Frame::group` adds around its contents, so the
/// arithmetic below is about outer widths.
pub(super) const FRAME_EXTRA: f32 = 14.0;

/// The largest a picture in a set may be drawn. What the strip of them is tall
/// is worked out from this and the font, in `tile_strip_height`.
pub(super) const TILE: egui::Vec2 = egui::vec2(156.0, 118.0);

/// Space kept clear around a picture for what is drawn around it: the keeper's
/// border, and the ring outside that for the one the preview is showing. The ring
/// sits 3 out from the border and is 3 wide, so it reaches 4.5 past it. Without
/// this the ring is drawn outside the tile and the neighbour clips it.
pub(super) const TILE_RING: f32 = 6.0;

/// The picture's border and the margin inside it, on both sides.
pub(super) const TILE_BORDER: f32 = 4.0;

/// What a picture of these proportions comes out as inside `TILE`.
pub(super) fn fitted(width: u32, height: u32) -> egui::Vec2 {
    let (width, height) = (width.max(1) as f32, height.max(1) as f32);
    let scale = (TILE.x / width).min(TILE.y / height);
    egui::vec2(width * scale, height * scale)
}

/// How wide one tile is: its own picture with room for what is drawn around it.
/// A portrait beside a landscape is a portrait's width, so a strip has no gaps in
/// it where a narrow picture was given a wide picture's column.
pub(super) fn tile_width(member: &imgdedupe_core::matching::Member) -> f32 {
    fitted(member.width, member.height).x.max(1.0) + TILE_BORDER + TILE_RING * 2.0
}

/// Room above and below the buttons inside the band along the bottom of a set,
/// so the line along the top of the band stands clear of the buttons instead of
/// being drawn along their top edge. The space between the buttons is the space
/// the row lays them out with.
pub(super) const BUTTON_ROW_GAP: f32 = 2.0;

/// How far in from the left edge of the box the row of buttons starts.
pub(super) const BUTTON_ROW_INSET: f32 = 5.0;

/// The band those buttons sit on.
pub(super) const BUTTON_ROW_BACKGROUND: egui::Color32 = egui::Color32::from_rgb(0xe8, 0xe8, 0xe8);

/// What the page keeps at its edges, and what a list keeps between what is in it
/// and the scroll bar down its right: the same, so a box in a list stops as far
/// from the bar as the list stops from the edge of the window.
///
/// The review keeps this itself rather than through the panel it is drawn in, so
/// the panels in it can run the width of the window and draw their lines across
/// all of it.
pub(super) const PAGE_MARGIN: f32 = 16.0;

/// Kept clear at the right of the folder row for the button that lists the
/// folders scanned before, so a long path stops short of it.
pub(super) const PREVIOUS_ROOM: f32 = 84.0;

/// What the review has found and what a cleanup would do about it, as one line:
/// how many sets, how many pictures in them are not being kept, and what those
/// come to. The last part is the least of it and is drawn as such.
pub(super) fn count_line(
    ui: &egui::Ui,
    sets: usize,
    duplicates: usize,
    going: usize,
    reclaimable: i64,
) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let strong = egui::TextFormat {
        font_id: font.clone(),
        color: ui.style().visuals.strong_text_color(),
        ..Default::default()
    };
    let weak = egui::TextFormat {
        font_id: font,
        color: ui.style().visuals.weak_text_color(),
        ..Default::default()
    };
    let gap = ui.spacing().item_spacing.x;

    let mut line = egui::text::LayoutJob::default();
    line.append(&counted(sets as u64, "set", "sets"), 0.0, strong.clone());
    line.append(
        &counted(duplicates as u64, "duplicate", "duplicates"),
        gap,
        strong.clone(),
    );
    line.append(&format!("{going} to remove"), gap, strong);
    line.append(
        &format!("{:.1} MB to reclaim", reclaimable as f64 / 1e6),
        gap,
        weak,
    );
    line
}

/// The review toolbar's one row. The checkbox, the counts and the button are
/// laid out over the same rectangle, which has to be as tall as the tallest of
/// them: the button.
pub(super) const TOOLBAR_HEIGHT: f32 = 28.0;

/// Between the buttons at the left of the review toolbar.
pub(super) const TOOLBAR_BUTTON_GAP: f32 = 14.0;

/// The cleanup button at the right of it, which is a fixed width so the counts
/// know how much of the row is left for them.
pub(super) const CLEANUP_BUTTON_WIDTH: f32 = 120.0;

/// Room around a preset's name. Four of these sit under the slider and are read
/// at a glance, so they are no bigger than the words in them.
pub(super) const PRESET_PADDING: egui::Vec2 = egui::vec2(6.0, 2.0);

/// The box holding the percentage beside the slider. Wide enough for the widest
/// value the scale reaches, so the number never changes the width of anything.
pub(super) const VALUE_WIDTH: f32 = 56.0;

/// Whether the slider is sitting on a preset, which is what draws that one as
/// pressed. The slider carries one decimal place, so anything closer than half
/// of that is the same setting.
pub(super) fn on_preset(sensitivity: f64, percent: f64) -> bool {
    (sensitivity - percent).abs() < 0.05
}

/// How many pictures are copies of another: every picture in a set, less the one
/// each set keeps.
pub(super) fn duplicate_count(sets: &[DuplicateSet]) -> usize {
    let pictures: usize = sets.iter().map(|set| set.members.len()).sum();
    pictures - sets.len()
}

/// What a set row takes. The list places the rows it is not drawing by this, and
/// a row is built to exactly it, so a row can never be a few points out and shift
/// the content under a scroll that is already running.
pub(super) fn set_row_height(ui: &egui::Ui) -> f32 {
    // The room kept above the pictures, the strip, the scroll bar under it, the
    // row of buttons under that, the line drawn round the lot, and the space to
    // the next box. The list places the rows it is not drawing by this number,
    // so the space between boxes has to be part of it.
    BOX_PADDING
        + tile_strip_height(ui)
        + SCROLL_BAR
        + STRIP_TO_BAR
        + button_row_height(ui)
        + 2.0 * BOX_EDGE
        + BETWEEN_BOXES
}

/// Kept between the writing under the pictures and the strip's own scroll bar,
/// which sits on the band of buttons below it. Enough to be seen: the bar reads
/// as another line of the writing when the two touch.
pub(super) const STRIP_TO_BAR: f32 = 6.0;

/// Kept between one set and the next.
pub(super) const BETWEEN_BOXES: f32 = 12.0;

/// How much of a set nobody calls a set of copies is drawn: the pictures and
/// every line of writing under them, but not the row of buttons, which is how it
/// stops being ignored.
pub(super) const IGNORED_OPACITY: f32 = 0.25;

/// The line the box is drawn with, on each side.
pub(super) const BOX_EDGE: f32 = 1.0;

/// What a box keeps between its edge and the pictures in it. The band of
/// buttons keeps none: it is the bottom of the box.
pub(super) const BOX_PADDING: f32 = 6.0;

/// What a button has to be to hold any of the words it can say, with the padding
/// around them.
pub(super) fn button_width(ui: &egui::Ui, says: &[&str]) -> f32 {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let widest = says
        .iter()
        .map(|words| {
            ui.fonts(|fonts| {
                fonts
                    .layout_no_wrap((*words).to_string(), font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        })
        .fold(0.0_f32, f32::max);
    widest + 2.0 * PRESET_PADDING.x
}

/// The row of buttons along the bottom of a set, with the space above it.
pub(super) fn button_row_height(ui: &egui::Ui) -> f32 {
    // What the buttons themselves come out as, with the same small space above
    // and below them. They are drawn with the presets' padding, not the style's
    // own, and a band built to the style's is half as tall again as it needs.
    2.0 * BUTTON_ROW_GAP + ui.text_style_height(&egui::TextStyle::Button) + 2.0 * PRESET_PADDING.y
}

/// What one tile in a set takes from top to bottom, which is what the strip of
/// them is tall.
///
/// Worked out from the style rather than kept as a number, because every part of
/// it is a style value: the text under a picture is four lines of whatever the
/// window's font measures, and a number written down here for one font is dead
/// space or a clipped line in another.
pub(super) fn tile_strip_height(ui: &egui::Ui) -> f32 {
    let gap = ui.spacing().item_spacing.y;
    // Room kept clear for the ring, then the picture at its largest inside the
    // border drawn round it.
    let picture = TILE_RING + TILE.y + TILE_BORDER;
    // Under it, five rows with a gap above each: the one that says KEEP, which
    // is kept clear whether or not it says it, and the four lines of text.
    let under = 5.0 * gap
        + ui.spacing().interact_size.y
        + 4.0 * ui.text_style_height(&egui::TextStyle::Body);
    picture + under
}

/// Box widths for a row: each one its measured content, plus an equal share of
/// whatever is left over, so the row fills the width and no box is padded more
/// than any other.
pub(super) fn share_row_width(available: f32, content: &[f32], gap: f32) -> Vec<f32> {
    let boxes = content.len() as f32;
    let used: f32 = content.iter().map(|width| width + FRAME_EXTRA).sum();
    let spare = ((available - used - gap * (boxes - 1.0)) / boxes).max(0.0);
    content.iter().map(|width| width + spare).collect()
}

/// A titled box. Every group of related controls goes in one, so the window reads
/// as parts rather than as one column of widgets.
pub(super) fn section(ui: &mut egui::Ui, title: &str, contents: impl FnOnce(&mut egui::Ui)) {
    let width = ui.available_width() - FRAME_EXTRA;
    sized_section(ui, title, egui::vec2(width, 0.0), contents);
}

/// The same at a fixed height, for boxes standing side by side.
///
/// The height is a constant and not measured. Measuring the boxes and feeding the
/// tallest back in on the next frame ratchets: the measurement includes the height
/// that was just imposed, so it can only grow, and hovering a widget was enough to
/// make the whole row taller and keep it there.
/// Draws the box and returns the width its contents actually wanted.
///
/// The contents are laid out in a child that is never told how wide to be, so the
/// measurement is of the content and not of the box. That is what stops it
/// feeding back on itself: the box is always at least as wide as its contents, so
/// nothing wraps, so the measurement does not change when the box grows.
pub(super) fn sized_section(
    ui: &mut egui::Ui,
    title: &str,
    size: egui::Vec2,
    contents: impl FnOnce(&mut egui::Ui),
) -> egui::Vec2 {
    let mut content = egui::Vec2::ZERO;
    ui.vertical(|ui| {
        ui.label(egui::RichText::new(title).strong());
        ui.add_space(4.0);
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(size.x);
            ui.set_min_height(size.y);
            content = ui.scope(contents).response.rect.size();
        });
    });
    content
}

/// A number with its name under it, for a row of them.
///
/// Neither line wraps. The column is as wide as the number, which is narrower
/// than the word under it, so wrapping breaks the word in half.
pub(super) fn counter(ui: &mut egui::Ui, name: &str, value: u64) {
    ui.vertical(|ui| {
        ui.add(unwrapped(egui::RichText::new(value.to_string()).strong()));
        ui.add(unwrapped(egui::RichText::new(name).weak()));
    });
}

/// A bar with its label inside it, filled up to `fraction`.
///
/// At zero nothing is filled. `egui::ProgressBar` widens its fill to the corner
/// radius so that the rounding has something to round, which draws a bubble on a
/// bar that has made no progress at all.
pub(super) fn progress_bar(
    ui: &mut egui::Ui,
    label: &str,
    fraction: f32,
    width: f32,
) -> egui::Response {
    let fraction = fraction.clamp(0.0, 1.0);
    let height = ui.spacing().interact_size.y;
    let (rect, response) = ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
    if !ui.is_rect_visible(rect) {
        return response;
    }
    let visuals = ui.style().visuals.clone();
    let rounding = height / 2.0;
    let painter = ui.painter();
    painter.rect(rect, rounding, visuals.extreme_bg_color, egui::Stroke::NONE);
    if fraction > 0.0 {
        let filled = (rect.width() * fraction).max(2.0 * rounding);
        painter.rect(
            egui::Rect::from_min_size(rect.min, egui::vec2(filled, height)),
            rounding,
            visuals.selection.bg_fill,
            egui::Stroke::NONE,
        );
    }
    let galley = egui::WidgetText::from(format!("{label} {:.0}%", fraction * 100.0)).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Button,
    );
    let at = rect.left_center() - egui::vec2(0.0, galley.size().y / 2.0)
        + egui::vec2(ui.spacing().item_spacing.x, 0.0);
    let ink = visuals
        .override_text_color
        .unwrap_or(visuals.selection.stroke.color);
    ui.painter().with_clip_rect(rect).galley(at, galley, ink);
    response
}

/// A number and the word for it, which is not the same word when there is one of
/// them.
pub(super) fn counted(how_many: u64, one: &str, more: &str) -> String {
    format!("{how_many} {}", if how_many == 1 { one } else { more })
}

/// A line of text cut to the room it is given.
///
/// `egui::Label` puts the whole string in a tooltip whenever it has to cut one,
/// and nothing in this window explains itself by being hovered over. Painting the
/// text rather than adding a widget leaves nothing to hover over.
pub(super) fn clipped_line(ui: &mut egui::Ui, text: egui::RichText) {
    let room = ui.available_width();
    clipped_line_in(ui, text, room);
}

/// The same, in the room given rather than in whatever is left.
pub(super) fn clipped_line_in(ui: &mut egui::Ui, text: egui::RichText, room: f32) {
    let room = room.max(0.0);
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Truncate),
        room,
        egui::TextStyle::Body,
    );
    let (rect, _) = ui.allocate_exact_size(galley.size(), egui::Sense::hover());
    let ink = ui.visuals().text_color();
    ui.painter()
        .with_clip_rect(rect)
        .galley(rect.min, galley, ink);
}

/// A label that takes the width it needs rather than breaking to fit.
pub(super) fn unwrapped(text: egui::RichText) -> egui::Label {
    egui::Label::new(text).wrap_mode(egui::TextWrapMode::Extend)
}
