use crate::*;

impl AudioOrbitApp {
    pub(crate) fn render_radio_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.heading(ui_icons::label(Icon::Radio, "Internet radio"));
            ui.label(format!("{} station(s)", self.state.radio_stations.len()));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui
                    .button(ui_icons::icon(Icon::Plus))
                    .on_hover_text("Add internet radio station")
                    .clicked()
                {
                    self.show_radio_add_modal = true;
                }

                let search_icon = if self.show_radio_search { Icon::X } else { Icon::Search };
                if ui
                    .button(ui_icons::icon(search_icon))
                    .on_hover_text(if self.show_radio_search { "Hide radio search" } else { "Search radio stations" })
                    .clicked()
                {
                    self.show_radio_search = !self.show_radio_search;
                    self.focus_radio_search = self.show_radio_search;
                    if !self.show_radio_search {
                        self.radio_search_query.clear();
                    }
                }

                if ui
                    .add_enabled(self.active_radio_index.is_some(), egui::Button::new(ui_icons::label(Icon::Music, "Now playing")))
                    .on_hover_text("Center the currently playing radio station")
                    .clicked()
                {
                    self.jump_to_now_playing();
                }
            });
        });
        ui.add(egui::Label::new("Radio streams are live sources. They ignore shuffle, repeat, auto-play next, and silence skipping. Crossfade is used when switching sources if enabled.").wrap());
        ui.add_space(8.0);

        ui.horizontal(|ui| {
            if ui
                .selectable_label(!self.radio_show_favorites_only, format!("All ({})", self.state.radio_stations.len()))
                .clicked()
            {
                self.radio_show_favorites_only = false;
            }
            let favorite_count = self.state.radio_stations.iter().filter(|station| station.favorite).count();
            if ui
                .selectable_label(self.radio_show_favorites_only, format!("Favorites ({favorite_count})"))
                .clicked()
            {
                self.radio_show_favorites_only = true;
            }
        });

        ui.horizontal(|ui| {
            if ui.small_button("A-Z").on_hover_text("Sort radio stations A to Z").clicked() {
                self.sort_radio_stations_by_name(true);
            }
            if ui.small_button("Z-A").on_hover_text("Sort radio stations Z to A").clicked() {
                self.sort_radio_stations_by_name(false);
            }
        });

        if self.show_radio_search {
            ui.horizontal(|ui| {
                ui.label(ui_icons::icon(Icon::Search));
                let response = ui.add_sized(
                    egui::vec2((ui.available_width() - 92.0).max(180.0), 22.0),
                    egui::TextEdit::singleline(&mut self.radio_search_query).hint_text("Search by station name, URL, or stream title"),
                );
                if self.focus_radio_search {
                    response.request_focus();
                    self.focus_radio_search = false;
                }
                if ui.button(ui_icons::label(Icon::X, "Clear")).clicked() {
                    self.radio_search_query.clear();
                }
            });
        }

        if self.state.radio_stations.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("No internet radio stations yet. Use the + button to add a stream URL.");
            });
            return;
        }

        let query = self.radio_search_query.trim().to_lowercase();
        let visible_stations: Vec<(usize, RadioStation)> = self
            .state
            .radio_stations
            .iter()
            .cloned()
            .enumerate()
            .filter(|(_, station)| {
                if self.radio_show_favorites_only && !station.favorite {
                    return false;
                }
                if query.is_empty() {
                    return true;
                }
                station.name.to_lowercase().contains(&query)
                    || station.url.to_lowercase().contains(&query)
                    || station
                        .last_station_name
                        .as_deref()
                        .map(|name| name.to_lowercase().contains(&query))
                        .unwrap_or(false)
                    || station
                        .last_stream_title
                        .as_deref()
                        .map(|title| title.to_lowercase().contains(&query))
                        .unwrap_or(false)
            })
            .collect();

        ui.small(format!(
            "Showing {}/{} station(s).",
            visible_stations.len(),
            self.state.radio_stations.len()
        ));
        ui.separator();

        if visible_stations.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("No radio stations match this view.");
            });
            return;
        }

        let row_width = (ui.available_width() - 22.0).max(320.0);
        let scroll_height = ui.available_height();
        let mut remove_radio_index: Option<usize> = None;
        let mut play_radio_index: Option<usize> = None;
        let mut favorite_toggle_index: Option<usize> = None;
        let mut reorder_radio_station: Option<(usize, usize)> = None;
        let mut next_radio_drop_target_index: Option<usize> = self.radio_drop_target_index;

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show(ui, |ui| {
                ui.set_width(row_width);
                let visible_station_len = visible_stations.len();
                let station_count = self.state.radio_stations.len();
                let pointer_position = ui.input(|input| input.pointer.hover_pos().or(input.pointer.interact_pos()));
                for (visible_row_index, (index, station)) in visible_stations.iter().cloned().enumerate() {
                    let next_visible_station_index = visible_stations
                        .get(visible_row_index + 1)
                        .map(|(next_index, _)| *next_index);
                    let active = self.active_radio_index == Some(index);
                    let selected = active || (self.radio_selection_was_user_set && self.state.selected_radio_index == Some(index));
                    let display_stream_title = station
                        .last_stream_title
                        .as_deref()
                        .filter(|title| !title.trim().is_empty() && !title.eq_ignore_ascii_case(&station.name));
                    let primary_title = display_stream_title.unwrap_or(station.name.as_str());
                    let station_title = if active {
                        format!("{} {}", ui_icons::icon(Icon::Play), primary_title)
                    } else {
                        primary_title.to_owned()
                    };
                    let station_info = if display_stream_title.is_some() {
                        station.name.clone()
                    } else {
                        String::new()
                    };

                    let row_hovered = next_row_pointer_hovered(ui, row_width, 34.0);
                    let row_response = ui.allocate_ui_with_layout(
                        egui::vec2(row_width, 34.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            let heart = if station.favorite {
                                egui::RichText::new("♥").color(egui::Color32::from_rgb(230, 70, 95)).size(15.0)
                            } else {
                                egui::RichText::new("♡").size(15.0)
                            };
                            if ui
                                .add_sized(egui::vec2(28.0, 24.0), egui::Button::new(heart))
                                .on_hover_text("Toggle favorite radio station")
                                .clicked()
                            {
                                favorite_toggle_index = Some(index);
                            }

                            let body_width = ui.available_width().max(160.0);
                            let (body_rect, title_response) = ui.allocate_exact_size(
                                egui::vec2(body_width, 24.0),
                                egui::Sense::click(),
                            );

                            if selected {
                                ui.painter().rect_filled(body_rect, 5.0, ui.visuals().selection.bg_fill);
                            }

                            let body_padding = 8.0;
                            let text_color = if selected {
                                ui.visuals().selection.stroke.color
                            } else if active {
                                ui.visuals().selection.bg_fill
                            } else {
                                ui.visuals().widgets.inactive.fg_stroke.color
                            };
                            let title_font = egui::FontId::proportional(14.0);
                            let info_font = egui::FontId::proportional(12.0);
                            let info_color = if active || row_hovered {
                                ui.visuals().widgets.inactive.fg_stroke.color
                            } else {
                                ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.50)
                            };
                            let info_width = if station_info.is_empty() {
                                0.0
                            } else {
                                text_width(ui, &station_info, info_font.clone(), info_color).ceil()
                            };
                            let info_gap = if station_info.is_empty() { 0.0 } else { 6.0 };
                            let title_left = body_rect.left() + body_padding;
                            let title_right = (body_rect.right() - body_padding - info_width - info_gap)
                                .max(title_left + 24.0);
                            let title_rect = egui::Rect::from_min_max(
                                egui::pos2(title_left, body_rect.top()),
                                egui::pos2(title_right, body_rect.bottom()),
                            );

                            let station_title = ellipsize_to_width_exact(ui, &station_title, title_rect.width(), title_font.clone(), text_color);
                            ui.painter().with_clip_rect(title_rect).text(
                                egui::pos2(title_rect.left(), title_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                station_title,
                                title_font,
                                text_color,
                            );

                            if info_width > 0.0 {
                                let info_rect = egui::Rect::from_min_max(
                                    egui::pos2(body_rect.right() - body_padding - info_width, body_rect.top()),
                                    egui::pos2(body_rect.right() - body_padding, body_rect.bottom()),
                                );
                                ui.painter().with_clip_rect(info_rect).text(
                                    egui::pos2(info_rect.right(), info_rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    station_info,
                                    info_font,
                                    info_color,
                                );
                            }

                            if title_response.clicked() {
                                self.state.selected_radio_index = Some(index);
                                self.radio_selection_was_user_set = true;
                                self.save_state_silently();
                            }
                            if title_response.double_clicked() {
                                play_radio_index = Some(index);
                            }
                        },
                    );
                    let mut context_rect = row_response.response.rect.expand(2.0);
                    context_rect.min.x += 34.0;
                    let context_response = ui.interact(
                        context_rect,
                        ui.make_persistent_id(("radio_station_context", index)),
                        egui::Sense::click_and_drag(),
                    );
                    if context_response.clicked() {
                        self.state.selected_radio_index = Some(index);
                        self.radio_selection_was_user_set = true;
                        self.save_state_silently();
                    }
                    if context_response.double_clicked() {
                        self.state.selected_radio_index = Some(index);
                        self.radio_selection_was_user_set = true;
                        play_radio_index = Some(index);
                    }
                    if context_response.drag_started() {
                        self.dragging_radio_index = Some(index);
                        self.radio_drop_target_index = None;
                    }
                    let pointer_over_row = pointer_position
                        .map(|position| row_response.response.rect.expand(2.0).contains(position))
                        .unwrap_or(false);
                    if let Some(from) = self.dragging_radio_index {
                        if pointer_over_row {
                            let pointer_y = pointer_position
                                .map(|position| position.y)
                                .unwrap_or(row_response.response.rect.center().y);
                            let drop_after = pointer_y >= row_response.response.rect.center().y;
                            let to = if drop_after {
                                next_visible_station_index.unwrap_or(station_count)
                            } else {
                                index
                            };
                            next_radio_drop_target_index = Some(to);
                            if ui.input(|input| input.pointer.any_released()) && Self::valid_drop_target(from, to) {
                                reorder_radio_station = Some((from, to));
                                self.dragging_radio_index = None;
                            }
                        }
                    }
                    let radio_drop_target_for_paint = next_radio_drop_target_index
                        .or(self.radio_drop_target_index)
                        .filter(|to| self.dragging_radio_index.map(|from| Self::valid_drop_target(from, *to)).unwrap_or(false));
                    if self.dragging_radio_index == Some(index) && radio_drop_target_for_paint.is_some() {
                        paint_dragged_row_fade(ui, row_response.response.rect);
                    }
                    if visible_row_index == 0 && radio_drop_target_for_paint == Some(index) {
                        paint_list_edge_separator(ui, row_response.response.rect, row_width, false);
                    }
                    if row_response.response.secondary_clicked() || context_response.secondary_clicked() {
                        self.state.selected_radio_index = Some(index);
                        self.radio_selection_was_user_set = true;
                        self.save_state_silently();
                    }
                    context_response.context_menu(|ui| {
                        if ui.button(ui_icons::label(Icon::Play, "Play station")).clicked() {
                            play_radio_index = Some(index);
                            ui.close_menu();
                        }
                        if ui.button(ui_icons::label(Icon::Info, "Details")).clicked() {
                            self.details_modal = Some(DetailsModal::Radio(index));
                            ui.close_menu();
                        }
                        if ui.button("Move up").clicked() {
                            self.move_radio_station(index, -1);
                            ui.close_menu();
                        }
                        if ui.button("Move down").clicked() {
                            self.move_radio_station(index, 1);
                            ui.close_menu();
                        }
                        if ui.button(ui_icons::label(Icon::Trash2, "Remove station")).clicked() {
                            remove_radio_index = Some(index);
                            ui.close_menu();
                        }
                    });
                    if self.scroll_to_active_radio_requested && active {
                        row_response.response.scroll_to_me(Some(egui::Align::Center));
                        self.scroll_to_active_radio_requested = false;
                    }
                    if visible_row_index + 1 < visible_station_len {
                        let separator_drop_target = next_visible_station_index.unwrap_or(index + 1);
                        paint_list_separator(ui, row_width, radio_drop_target_for_paint == Some(separator_drop_target));
                    } else if radio_drop_target_for_paint == Some(station_count) {
                        paint_list_edge_separator(ui, row_response.response.rect, row_width, true);
                    }
                }
            });

        self.radio_drop_target_index = next_radio_drop_target_index;
        if !ui.input(|input| input.pointer.primary_down()) {
            self.dragging_radio_index = None;
            self.radio_drop_target_index = None;
        }
        if let Some((from, to)) = reorder_radio_station {
            self.radio_drop_target_index = None;
            self.move_radio_station_to_index(from, to);
        }
        if let Some(index) = favorite_toggle_index {
            if let Some(station) = self.state.radio_stations.get_mut(index) {
                station.favorite = !station.favorite;
                self.state.selected_radio_index = Some(index);
                self.radio_selection_was_user_set = true;
                self.save_state_silently();
            }
        }
        if let Some(index) = play_radio_index {
            self.state.selected_radio_index = Some(index);
            self.radio_selection_was_user_set = true;
            self.play_radio_station(index);
        }
        if let Some(index) = remove_radio_index {
            self.state.selected_radio_index = Some(index);
            self.radio_selection_was_user_set = true;
            self.remove_selected_radio_station();
        }
    }
    pub(crate) fn request_folder_group_top(&mut self, group: &str) {
        self.scroll_to_folder_group_requested = Some(group.to_owned());
    }
    pub(crate) fn toggle_folder_group_collapsed_and_focus(&mut self, group: &str) {
        if self.collapsed_groups.contains(group) {
            self.collapsed_groups.remove(group);
        } else {
            self.collapsed_groups.insert(group.to_owned());
        }
        self.request_folder_group_top(group);
    }
}
