use super::*;

impl App {
    pub(super) fn review_view(&mut self, ui: &mut egui::Ui) {
        let Some(root) = self.folder.clone() else {
            ui.label("no folder chosen");
            return;
        };

        let visible: Vec<usize> = (0..self.sets.len()).collect();
        let duplicates = duplicate_count(&self.sets);
        let (going, reclaimable) = self.selected_for_removal();

        egui::TopBottomPanel::top("review toolbar").show_inside(ui, |ui| {
            ui.add_space(4.0);
            // Three lots that share one row: the checkbox against the left edge,
            // the counts in the middle of the window, and the button against the
            // right edge. They are laid out over the same rectangle, each with
            // the layout that puts it where it belongs, so the counts sit in the
            // centre of the row rather than in the centre of what is left of it.
            // The panel runs the width of the window, so its own line does too.
            // What is in it keeps the page's margin.
            let row = egui::vec2(ui.available_width(), TOOLBAR_ROW_HEIGHT);
            let (rect, _) = ui.allocate_exact_size(row, egui::Sense::hover());
            let rect = rect.shrink2(egui::vec2(CONTENT_MARGIN, 0.0));

            let mut left = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
            );
            if left.button("unmark all").clicked() {
                self.unmark_everything();
            }
            left.add_space(TOOLBAR_BUTTON_GAP);
            if left.button("mark all").clicked() {
                self.keep_everything();
            }
            left.add_space(TOOLBAR_BUTTON_GAP);
            if left.button("auto-mark to keep").clicked() {
                self.auto_mark_to_keep();
            }

            // In the middle of what is left between the buttons, not the middle
            // of the window: the buttons take the ends of the row, and centring
            // on the window puts the counts over them as soon as there are
            // enough of them.
            let between = egui::Rect::from_min_max(
                egui::pos2(left.min_rect().right() + TOOLBAR_BUTTON_GAP, rect.top()),
                egui::pos2(
                    rect.right() - CLEANUP_BUTTON_WIDTH - TOOLBAR_BUTTON_GAP,
                    rect.bottom(),
                ),
            );
            let mut middle = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(between)
                    .layout(egui::Layout::top_down(egui::Align::Center)),
            );
            // One line rather than four labels beside each other: a row of
            // widgets is laid out from where the row starts, and only a single
            // thing can be put in the middle of the space it is given.
            let counts = count_line(&middle, visible.len(), duplicates, going, reclaimable);
            middle.add(egui::Label::new(counts).wrap_mode(egui::TextWrapMode::Extend));

            let mut right = ui.new_child(
                egui::UiBuilder::new()
                    .max_rect(rect)
                    .layout(egui::Layout::right_to_left(egui::Align::Center)),
            );
            let go = egui::Button::new(
                egui::RichText::new("Clean up")
                    .strong()
                    .color(CLEANUP_BUTTON_TEXT_COLOUR),
            )
            .fill(CLEANUP_BUTTON_FILL_COLOUR)
            .min_size(egui::vec2(CLEANUP_BUTTON_WIDTH, 28.0));
            if right.add_enabled(going > 0, go).clicked() {
                self.view = View::Cleanup;
            }
            ui.add_space(4.0);
        });

        for (key, direction) in [
            (egui::Key::ArrowRight, Direction::Forward),
            (egui::Key::ArrowLeft, Direction::Back),
            (egui::Key::ArrowDown, Direction::NextSet),
            (egui::Key::ArrowUp, Direction::PreviousSet),
        ] {
            if ui.input(|input| input.key_pressed(key)) {
                self.walk(&visible, direction);
            }
        }
        if ui.input(|input| input.key_pressed(egui::Key::Space)) {
            if ui.input(|input| input.modifiers.shift) {
                self.keep_only_selected();
            } else {
                self.keep_selected();
            }
        }

        self.preview_pane(ui, &root);

        // Only the rows on screen are built, so a folder with thousands of sets
        // costs the same per frame as one with ten.
        let row_height = set_row_height(ui);
        let spacing = ui.spacing().item_spacing.y;
        let mut list = egui::ScrollArea::vertical().auto_shrink([false, false]);
        if let Some(row) = self.scroll_to.take() {
            if let Some(offset) = scroll_to_show(
                row,
                visible.len(),
                row_height,
                spacing,
                self.list_offset,
                self.list_viewport,
            ) {
                list = list.vertical_scroll_offset(offset);
            }
        }
        if std::mem::take(&mut self.list_to_top) {
            list = list.vertical_scroll_offset(0.0);
        }

        // The page's margin down the left of the list, kept here rather than by
        // the panel, so the panels above can run the width of the window and
        // draw their lines across all of it. The top is the line under the
        // toolbar: the bar down the side of the list starts there, against the
        // line, and the gap above the first set is put inside the list instead.
        let room = ui.available_rect_before_wrap();
        let room = egui::Rect::from_min_max(
            egui::pos2(room.left() + CONTENT_MARGIN, room.top()),
            room.max,
        );
        // What a row comes out as, worked out here where the list's own room is
        // known: inside a scroll area nothing is told where the area ends.
        let row_width = (room.width() - SCROLLBAR_STRIP_WIDTH - CONTENT_MARGIN).max(0.0);

        let (_, offset, viewport, _) = ui
            .allocate_new_ui(egui::UiBuilder::new().max_rect(room), |ui| {
                let list_id = egui::Id::new("review list");
                scrolled(
                    ui,
                    list_id,
                    true,
                    row_height + spacing,
                    None,
                    list,
                    |list, ui| {
                        // The gap above the first set, taken here rather than
                        // from the rectangle the list was given: the bar down
                        // the side of the list comes from that rectangle and has
                        // to start at the line above it, not at the first box.
                        ui.add_space(SECTION_SPACING_GAP);
                        list.show_rows(ui, row_height, visible.len(), |ui, range| {
                            for position in range {
                                let index = visible[position];
                                self.set_row(ui, index, &root, row_width);
                            }
                        })
                    },
                )
            })
            .inner;
        self.list_offset = offset;
        self.list_viewport = viewport;
    }

    /// The picture that was clicked, at a size worth looking at. Sits beside the
    /// list rather than over it, so clicking through a set is one click each.
    fn preview_pane(&mut self, ui: &mut egui::Ui, root: &Path) {
        let find = |file_id: i64| {
            self.sets
                .iter()
                .flat_map(|set| set.members.iter().map(move |member| (set.set_id, member)))
                .find(|(_, member)| member.file_id == file_id)
                .map(|(set_id, member)| (set_id, member.clone()))
        };
        let chosen = self.selected.and_then(find);
        let held = self.showing.and_then(find);

        let width = self.preview_width.unwrap_or(ui.available_width() * 0.42);
        let pane = egui::SidePanel::right("preview")
            .resizable(true)
            .default_width(width)
            .min_width(260.0)
            .show_inside(ui, |ui| {
                let Some((set_id, member)) = chosen else {
                    ui.centered_and_justified(|ui| {
                        ui.label(egui::RichText::new("click a picture to see it here").weak());
                    });
                    return;
                };

                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let keeping = keeps(self.keep.get(&set_id), member.file_id);
                    if ui
                        .add_enabled(!keeping, egui::Button::new("Keep this one"))
                        .clicked()
                    {
                        let came_off: Vec<i64> = self
                            .keep
                            .get(&set_id)
                            .map(Keep::marked)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|kept| *kept != member.file_id)
                            .collect();
                        self.keep.insert(set_id, Keep::One(member.file_id));
                        self.unmarked(&came_off);
                        self.marked(&[member.file_id]);
                    }
                    ui.label(
                        egui::RichText::new(format!(
                            "{}x{}  {}  {:.1} MB",
                            member.width,
                            member.height,
                            member.format,
                            member.size_bytes as f64 / 1_000_000.0
                        ))
                        .weak(),
                    );
                });
                ui.add(unwrapped(egui::RichText::new(&member.rel_path).weak()));
                ui.add_space(4.0);

                // The picture takes the top of the pane and what the file says
                // about itself takes the rest, so there is always a picture and
                // always somewhere for the words to go.
                let room = egui::vec2(ui.available_width(), ui.available_height() * 0.62);
                let wanted =
                    self.thumbs
                        .get(member.file_id, thumbs::LARGE_EDGE, root, &member.rel_path);
                if wanted.is_some() {
                    self.showing = self.selected;
                }
                // The one being read is not on screen yet, so what is on screen
                // stays there. Asking for it again is what keeps it alive.
                let drawing = wanted.or_else(|| {
                    let (_, held) = held?;
                    self.thumbs
                        .get(held.file_id, thumbs::LARGE_EDGE, root, &held.rel_path)
                });

                ui.allocate_ui(room, |ui| {
                    ui.centered_and_justified(|ui| match drawing {
                        Some(texture) => {
                            let shown = ui.add(
                                egui::Image::new(&texture)
                                    .max_size(room)
                                    .maintain_aspect_ratio(true)
                                    .sense(egui::Sense::click()),
                            );
                            // A hand over it, because a picture that does
                            // something when it is clicked has to look like one.
                            if shown.hovered() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            if shown.clicked() {
                                self.filling_the_window = Some(member.file_id);
                            }
                        }
                        None => {
                            ui.label(egui::RichText::new("reading...").weak());
                        }
                    });
                });
                ui.add_space(6.0);
                self.written_beside_it(ui, &member, root);
            });
        self.preview_width = Some(pane.response.rect.width());
    }

    /// Everything the file says about itself, under the picture: what the camera
    /// was set to, when and where it was taken, and whatever anybody wrote into
    /// it since.
    ///
    /// The file is read on a thread of its own, because a raw file is tens of
    /// megabytes and often on another machine, and the window carries on drawing
    /// while it arrives.
    fn written_beside_it(
        &mut self,
        ui: &mut egui::Ui,
        member: &imgdedupe_core::matching::Member,
        root: &std::path::Path,
    ) {
        let path = root.join(&member.rel_path);
        let groups = self.metadata.get(member.file_id, path, ui.ctx());
        if groups.is_empty() {
            let waiting = self.metadata.reading();
            ui.label(
                egui::RichText::new(if waiting {
                    "reading..."
                } else {
                    "this file says nothing about itself"
                })
                .weak(),
            );
            return;
        }

        let lines: Vec<(Option<String>, String, String)> = groups
            .iter()
            .flat_map(|group| {
                std::iter::once((Some(group.name.clone()), String::new(), String::new())).chain(
                    group
                        .entries
                        .iter()
                        .map(|(name, value)| (None, name.clone(), value.clone())),
                )
            })
            .collect();

        let line = ui.text_style_height(&egui::TextStyle::Body) + ui.spacing().item_spacing.y;
        let names = ui.available_width() * 0.38;
        // What is left of the pane, and no more. Without a height of its own the
        // list takes whatever it asks for, runs off the bottom of the window, and
        // the part of it below the edge cannot be reached by anything.
        let room =
            (ui.max_rect().bottom() - ui.cursor().top() - SCROLLBAR_STRIP_WIDTH).max(line * 3.0);
        let bar = egui::Id::new(("what it says", member.file_id));
        // The whole pane, not only the part the list covers: a scroll wheel
        // turned over the picture is a scroll wheel turned over this pane, and
        // the list is the only thing in it that can move.
        let where_it_is = ui.max_rect();
        scrolled(
            ui,
            bar,
            true,
            line * 3.0,
            Some(where_it_is),
            egui::ScrollArea::vertical()
                .id_salt(("metadata", member.file_id))
                .max_height(room)
                .auto_shrink([false, false]),
            |area, ui| {
                area.show_rows(ui, line, lines.len(), |ui, rows| {
                    for (heading, name, value) in &lines[rows] {
                        match heading {
                            Some(heading) => {
                                // A band the width of the pane, so a heading is
                                // where one part ends and the next begins rather
                                // than another line of text among the lines.
                                let (rect, _) = ui.allocate_exact_size(
                                    egui::vec2(ui.available_width(), line),
                                    egui::Sense::hover(),
                                );
                                ui.painter().rect_filled(
                                    rect,
                                    2.0,
                                    ui.visuals().widgets.inactive.weak_bg_fill,
                                );
                                let ink = ui.visuals().strong_text_color();
                                let words = ui.painter().layout_no_wrap(
                                    heading.clone(),
                                    egui::TextStyle::Body.resolve(ui.style()),
                                    ink,
                                );
                                let down = (rect.height() - words.size().y) / 2.0;
                                ui.painter().galley(
                                    rect.left_top() + egui::vec2(6.0, down),
                                    words,
                                    ink,
                                );
                            }
                            None => {
                                ui.horizontal(|ui| {
                                    let room = ui.available_width();
                                    clipped_line_in(ui, egui::RichText::new(name).weak(), names);
                                    clipped_line_in(ui, egui::RichText::new(value), room - names);
                                });
                            }
                        }
                    }
                })
            },
        );
    }

    /// The picture, filling the window, over everything else.
    ///
    /// Asked for at the size of the window rather than at the size of the pane
    /// it came from, so it is the picture and not a blown up thumbnail of it. A
    /// click anywhere on it, or the escape key, puts it away.
    pub(super) fn filling_the_window(&mut self, ctx: &egui::Context) {
        let Some(file_id) = self.filling_the_window else {
            return;
        };
        let Some(root) = self.folder.clone() else {
            self.filling_the_window = None;
            return;
        };
        let Some(member) = self
            .sets
            .iter()
            .flat_map(|set| set.members.iter())
            .find(|member| member.file_id == file_id)
            .cloned()
        else {
            self.filling_the_window = None;
            return;
        };

        if ctx.input(|input| input.key_pressed(egui::Key::Escape)) {
            self.filling_the_window = None;
            return;
        }

        let screen = ctx.screen_rect();
        let edge = (screen.width().max(screen.height()) * ctx.pixels_per_point()) as u32;
        let picture = self.thumbs.get(file_id, edge, &root, &member.rel_path);

        egui::Area::new(egui::Id::new("filling the window"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen.min)
            .show(ctx, |ui| {
                ui.set_min_size(screen.size());
                // Everything behind it is covered, so what is on screen is the
                // picture and nothing else.
                ui.painter()
                    .rect_filled(screen, 0.0, FULL_WINDOW_PICTURE_BACKDROP_COLOUR);
                let taken = ui.allocate_rect(screen, egui::Sense::click());
                match picture {
                    Some(texture) => {
                        let size = texture.size_vec2();
                        let room = screen.size() * 0.98;
                        let scale = (room.x / size.x).min(room.y / size.y).min(1.0);
                        let shown = egui::Rect::from_center_size(screen.center(), size * scale);
                        egui::Image::new(&texture).paint_at(ui, shown);
                    }
                    None => {
                        ui.painter().text(
                            screen.center(),
                            egui::Align2::CENTER_CENTER,
                            "reading...",
                            egui::TextStyle::Body.resolve(ui.style()),
                            ui.visuals().weak_text_color(),
                        );
                    }
                }
                if taken.clicked() {
                    self.filling_the_window = None;
                }
            });
    }

    /// `width` is what the box around the set comes out as. The list works it
    /// out from its own room, because what is inside a scroll area is not told
    /// where the area ends.
    pub(super) fn set_row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        root: &std::path::Path,
        width: f32,
    ) {
        let set_id = self.sets[index].set_id;
        let members = self.sets[index].members.clone();
        let keeping = self.keep.get(&set_id).cloned();

        // Built to a fixed height rather than measured afterwards. The list places
        // the rows it is not drawing by this number, and a row that came out any
        // other height would move the content under a scroll already in progress.
        //
        // The width is the room less what the frame drawn round the row adds to
        // it, and less the margin the window keeps at its edges, so the box ends
        // as far from the list's scroll bar as it begins from the window's edge.
        let size = egui::vec2(width.max(0.0), set_row_height(ui));
        let ignored = self.is_ignored(&self.sets[index]);
        // The row is the box and the space kept under it. The box is the top of
        // the row; the space below it is what separates one set from the next.
        let room = egui::Rect::from_min_size(ui.next_widget_position(), size);
        // The line round the box is drawn half on either side of the rectangle
        // it is given, so that rectangle is set in by the width of the line: the
        // box then ends exactly at the row's edges, and the gap from the window
        // to its left edge is the gap from its right edge to the scroll bar.
        let inside = egui::Rect::from_min_max(
            room.min,
            egui::pos2(room.right(), room.bottom() - SET_BOX_VERTICAL_GAP),
        )
        .shrink(SET_BOX_BORDER_WIDTH);
        ui.allocate_new_ui(egui::UiBuilder::new().max_rect(room), |ui| {
            ui.set_min_size(size);
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(inside), |ui| {
                egui::Frame::group(ui.style())
                    .inner_margin(egui::Margin::ZERO)
                    .show(ui, |ui| {
                        let gap = ui.spacing().item_spacing.x;
                        // The room for the strip's own scroll bar is kept whether or not
                        // this set has one, so every box is the height the list places
                        // its rows at and one box follows the next with the same space
                        // between them.
                        ui.set_height(inside.height());
                        // The strip and the buttons are both drawn into rectangles of
                        // their own, which claim no width for the box around them. Left
                        // at that the box ends up as wide as the row of buttons.
                        ui.set_min_width(inside.width());

                        // The strip has the top of the box, down to where the buttons
                        // begin. It runs the whole width of the box, so the bar under it
                        // does too, the way the bar beside the list runs the height of
                        // the list. The pictures keep the box's padding on either side;
                        // that is theirs, not the bar's. Given the whole box the strip
                        // would put its bar under the buttons, at the very bottom.
                        let strip = egui::Rect::from_min_size(
                            egui::pos2(inside.left(), inside.top() + SET_BOX_INNER_PADDING),
                            egui::vec2(
                                inside.width(),
                                tile_strip_height(ui)
                                    + SET_TEXT_TO_SCROLLBAR_GAP
                                    + SCROLLBAR_STRIP_WIDTH,
                            ),
                        );

                        // One tile's width is what a click on the strip's scroll bar
                        // moves by, and the first tile's is as good a step as any.
                        let step = members.first().map_or(THUMBNAIL_MAX_WIDTH, tile_width);
                        let bar = egui::Id::new(("set bar", set_id));
                        let (_, offset, viewport, _) = ui
                            .allocate_new_ui(egui::UiBuilder::new().max_rect(strip), |ui| {
                                // A set nobody calls a set of copies is barely there:
                                // everything above the buttons, the pictures and every
                                // line of writing under them, at a quarter of its
                                // opacity. The buttons are how it stops being ignored, so
                                // they are not faded with the rest of it.
                                if ignored {
                                    ui.set_opacity(IGNORED_SET_OPACITY);
                                }
                                scrolled(
                                    ui,
                                    bar,
                                    false,
                                    step,
                                    None,
                                    egui::ScrollArea::horizontal().id_salt(("set", set_id)),
                                    |area, ui| {
                                        // The pictures keep the box's padding on either
                                        // side of them, inside a strip that is the whole
                                        // width of the box. Their own room, clipped to
                                        // it, so a picture scrolled up against the edge
                                        // stops there rather than in the padding.
                                        let room = ui
                                            .max_rect()
                                            .shrink2(egui::vec2(SET_BOX_INNER_PADDING, 0.0));
                                        let mut pictures =
                                            ui.new_child(egui::UiBuilder::new().max_rect(room));
                                        pictures.set_clip_rect(room.intersect(ui.clip_rect()));
                                        area.show(&mut pictures, |ui| {
                                            ui.horizontal_top(|ui| {
                                                for member in &members {
                                                    let width = tile_width(member);
                                                    self.member_tile(
                                                        ui,
                                                        member,
                                                        keeping.as_ref(),
                                                        root,
                                                        width,
                                                        ignored,
                                                    );
                                                }
                                            });
                                        })
                                    },
                                )
                            })
                            .inner;

                        // The cursor keys move the preview along the strip, and the strip
                        // follows: as far as it takes to bring the picture on screen and
                        // no further, so the set does not jump about under a selection
                        // that was already in view.
                        let holds_it = members
                            .iter()
                            .any(|member| Some(member.file_id) == self.selected);
                        if self.show_selected && holds_it {
                            if let Some(wanted) = self.strip_offset(&members, gap, offset, viewport)
                            {
                                ui.data_mut(|data| data.insert_temp(bar, wanted));
                                ui.ctx().request_repaint();
                            }
                            self.show_selected = false;
                        }

                        // What the whole set can be told to do, in a row along the
                        // bottom of it: everything, nothing, or that it is not a set of
                        // copies at all. Under the pictures rather than over them,
                        // because the pictures are what a set is.
                        // A band of its own under the pictures, so the row reads as the
                        // foot of the set rather than as three buttons adrift in it. It
                        // is the bottom of the box, corner to corner: the box keeps no
                        // margin, so there is nothing between the two.
                        let band = egui::Rect::from_min_max(
                            egui::pos2(inside.left(), inside.bottom() - button_row_height(ui)),
                            inside.max,
                        );
                        ui.painter()
                            .rect_filled(band, 0.0, SET_BUTTON_BAND_BACKGROUND_COLOUR);
                        // A line of its own along the top of the band, in the grey the
                        // scroll bar is drawn round, which the bar above it now sits on.
                        ui.painter().hline(
                            band.x_range(),
                            band.top(),
                            egui::Stroke::new(
                                1.0_f32,
                                ui.visuals().widgets.noninteractive.bg_stroke.color,
                            ),
                        );

                        let mut pressed = None;
                        ui.allocate_new_ui(
                            egui::UiBuilder::new()
                                // In from the left edge of the box, so the line round the
                                // first button is drawn rather than clipped away by it.
                                .max_rect(band.with_min_x(band.left() + SET_BUTTON_BAND_LEFT_INSET))
                                .layout(egui::Layout::left_to_right(egui::Align::Center)),
                            |ui| {
                                // The presets under the slider are a row of buttons the
                                // size of their own words, spaced by the style, with the
                                // one that is on drawn as pressed. These are the same,
                                // because they are the same kind of thing.
                                ui.spacing_mut().button_padding = egui::vec2(
                                    SMALL_BUTTON_HORIZONTAL_PADDING,
                                    SMALL_BUTTON_VERTICAL_PADDING,
                                );
                                for (label, what, says) in [
                                    ("keep all", SetAction::KeepAll, ["keep all"; 2]),
                                    ("keep none", SetAction::KeepNone, ["keep none"; 2]),
                                    // The third one undoes itself: a set that has been
                                    // ignored is one press away from being a set again.
                                    (
                                        if ignored { "ignored" } else { "ignore" },
                                        if ignored {
                                            SetAction::Unignore
                                        } else {
                                            SetAction::Ignore
                                        },
                                        ["ignore", "ignored"],
                                    ),
                                ] {
                                    // An ignored set keeps nothing, so the two about
                                    // keeping mean nothing until it is a set again.
                                    let usable = !ignored || what == SetAction::Unignore;
                                    // As wide as the widest thing it can ever say. A
                                    // button that fits its current word changes width
                                    // when the word changes, and takes every button to
                                    // the right of it along with it.
                                    let button = egui::Button::new(label)
                                        .wrap_mode(egui::TextWrapMode::Extend)
                                        .min_size(egui::vec2(button_width(ui, &says), 0.0));
                                    if ui.add_enabled(usable, button).clicked() {
                                        pressed = Some(what);
                                    }
                                }
                            },
                        );
                        match pressed {
                            // Every picture in the set marked, and shown as marked. A
                            // cleanup takes nothing from it, the same as a set nobody has
                            // reached, but the set says so rather than looking untouched.
                            Some(SetAction::KeepAll) => {
                                let all: Vec<i64> =
                                    members.iter().map(|member| member.file_id).collect();
                                // The ones that were not marked already: a row is written
                                // for what changed, not for the set.
                                let already =
                                    self.keep.get(&set_id).map(Keep::marked).unwrap_or_default();
                                let went_on: Vec<i64> = all
                                    .iter()
                                    .copied()
                                    .filter(|id| !already.contains(id))
                                    .collect();
                                match as_keep(all) {
                                    Some(keep) => self.keep.insert(set_id, keep),
                                    None => self.keep.remove(&set_id),
                                };
                                self.marked(&went_on);
                            }
                            // Nothing in the set is marked to keep any more. By the rule
                            // above that leaves the set losing nothing, the same as one
                            // nobody has reached.
                            Some(SetAction::KeepNone) => {
                                let came_off = self
                                    .keep
                                    .remove(&set_id)
                                    .map(|keep| keep.marked())
                                    .unwrap_or_default();
                                self.unmarked(&came_off);
                            }
                            Some(SetAction::Ignore) => self.ignore_set(set_id),
                            Some(SetAction::Unignore) => self.unignore_set(set_id),
                            None => {}
                        }
                    });
            });
        });
    }

    /// How far along a set's strip has to be for the picture the preview is
    /// showing to be on it, or `None` if it is on it already.
    ///
    /// As little movement as the job takes: a picture off the left brings the
    /// strip back to its left edge, one off the right brings it just far enough
    /// to end at the right edge, and one already on screen moves nothing. The
    /// strip is a row of tiles with a gap between them, so where a tile starts is
    /// what the tiles before it took.
    fn strip_offset(
        &self,
        members: &[imgdedupe_core::matching::Member],
        gap: f32,
        offset: f32,
        viewport: f32,
    ) -> Option<f32> {
        let selected = self.selected?;
        let mut start = 0.0;
        for member in members {
            let width = tile_width(member);
            if member.file_id == selected {
                if start < offset {
                    return Some(start);
                }
                if start + width > offset + viewport {
                    return Some(start + width - viewport);
                }
                return None;
            }
            start += width + gap;
        }
        None
    }

    /// One image in a set: the picture, whether it is the one being kept, and the
    /// two facts that decide it.
    fn member_tile(
        &mut self,
        ui: &mut egui::Ui,
        member: &imgdedupe_core::matching::Member,
        keeping: Option<&Keep>,
        root: &std::path::Path,
        width: f32,
        ignored: bool,
    ) {
        // A set nobody calls a set of copies keeps nothing and shows nothing as
        // kept: no border and no ring. Half showing is the strip's business and
        // is done to the whole of it at once.
        let kept = !ignored && keeps(keeping, member.file_id);
        let showing = !ignored && self.selected == Some(member.file_id);
        let keep_colour = KEPT_THUMBNAIL_MARK_COLOUR;

        let tall = tile_strip_height(ui);
        ui.allocate_ui(egui::vec2(width, tall), |ui| {
            ui.set_height(tall);
            // A set is a strip that scrolls sideways, and the ones off the end of
            // it are not on screen however much of the set is. Asking for them
            // would put a hundred pictures nobody can see in front of the next
            // set's, which is what made the sets below the first one wait.
            let on_screen = ui.is_rect_visible(ui.max_rect());
            ui.vertical(|ui| {
                ui.add_space(THUMBNAIL_CLEARANCE);
                // The keeper's border stays on the picture. Being the one on the
                // right is a second thing, drawn as a ring outside it, so a
                // picture that is both shows both.
                let frame = egui::Frame::none()
                    .stroke(if kept {
                        egui::Stroke::new(3.0_f32, keep_colour)
                    } else {
                        egui::Stroke::new(
                            1.0_f32,
                            ui.style().visuals.widgets.noninteractive.bg_stroke.color,
                        )
                    })
                    .inner_margin(THUMBNAIL_INNER_MARGIN)
                    .outer_margin(egui::Margin::symmetric(THUMBNAIL_CLEARANCE, 0.0));

                let framed = frame.show(ui, |ui| {
                    // The same picture whether or not the set is ignored, which
                    // is drawn at half its opacity rather than read again.
                    let picture = on_screen
                        .then(|| {
                            self.thumbs.get(
                                member.file_id,
                                thumbs::THUMB_EDGE,
                                root,
                                &member.rel_path,
                            )
                        })
                        .flatten();
                    match picture {
                        // Half showing when the set is ignored, but not by this:
                        // the whole strip is drawn at half its opacity.
                        Some(texture) => ui.add(
                            egui::Image::new(&texture)
                                .fit_to_exact_size(egui::vec2(
                                    THUMBNAIL_MAX_WIDTH,
                                    THUMBNAIL_MAX_HEIGHT,
                                ))
                                .sense(egui::Sense::click()),
                        ),
                        // The same space the picture will take, so nothing moves
                        // when it arrives.
                        None => ui.add_sized(
                            fitted(member.width, member.height),
                            egui::Label::new(egui::RichText::new("...").weak())
                                .sense(egui::Sense::click()),
                        ),
                    }
                });
                // A frame's response covers its outer margin as well, and the ring
                // goes around the picture, not around the space kept clear for it.
                let bordered = framed
                    .response
                    .rect
                    .shrink2(egui::vec2(THUMBNAIL_CLEARANCE, 0.0));
                if showing {
                    // Inside the keep border, not around it. Drawn outside, the
                    // ring for the picture being looked at sits over the border
                    // that says the picture is being kept, and the one thing a
                    // person needs to see about the picture in front of them is
                    // hidden by the fact that they are looking at it.
                    ui.painter().rect_stroke(
                        bordered.shrink(3.0),
                        2.0,
                        egui::Stroke::new(3.0_f32, ui.style().visuals.selection.bg_fill),
                    );
                }
                let picked = framed.inner;
                if picked.clicked() {
                    self.selected = Some(member.file_id);
                }
                // Twice on a picture keeps it, which is the space bar on the one
                // being shown, including that it takes the mark off again and
                // that holding shift marks it and nothing else.
                if picked.double_clicked() {
                    self.selected = Some(member.file_id);
                    if ui.input(|input| input.modifiers.shift) {
                        self.keep_only_selected();
                    } else {
                        self.keep_selected();
                    }
                }

                // Centred on the picture this tile is about. The picture sits in
                // the middle of the column, so the column's width is its width.
                // The space is taken either way, so the rows under it line up
                // across a set whether or not anything is marked.
                let over_the_picture = egui::vec2(width, ui.spacing().interact_size.y);
                ui.allocate_ui_with_layout(
                    over_the_picture,
                    egui::Layout::top_down(egui::Align::Center),
                    |ui| {
                        ui.set_min_size(over_the_picture);
                        if kept {
                            ui.label(egui::RichText::new("KEEP").strong().color(keep_colour));
                        }
                    },
                );
                // Four lines, each of them one line. A tile is a column as wide
                // as its picture, and a line long enough to wrap in it would
                // make that tile taller than every other one in the strip.
                clipped_line_in(
                    ui,
                    egui::RichText::new(format!("{}x{}", member.width, member.height)),
                    width,
                );
                clipped_line_in(
                    ui,
                    egui::RichText::new(format!(
                        "{}  {:.1} MB",
                        member.format,
                        member.size_bytes as f64 / 1_000_000.0
                    ))
                    .weak(),
                    width,
                );
                clipped_line_in(
                    ui,
                    egui::RichText::new(file_date(member.mtime_seconds)).weak(),
                    width,
                );
                clipped_line_in(ui, egui::RichText::new(&member.rel_path).weak(), width);
            });
        });
    }
}
