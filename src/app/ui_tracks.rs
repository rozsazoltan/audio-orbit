use crate::*;

impl AudioOrbitApp {
    pub(crate) fn render_track_panel(&mut self, ui: &mut egui::Ui) {
        let Some(playlist) = self.current_playlist() else {
            ui.heading("No playlist");
            return;
        };

        let playlist_name = playlist.name.clone();
        let selected_playlist_label = format!("{} {}", playlist.kind.icon(), playlist_name);
        let selected_group_label = playlist.selected_group.clone().unwrap_or_default();
        let folder_group_count = playlist.folder_groups().len();
        let show_group_headers = playlist.selected_group.is_none() && folder_group_count > 1;
        let query = self.track_search_query.trim().to_owned();
        let visible_indexes = self.visible_track_indexes();
        let visible_count = visible_indexes.len();

        let playlist_options: Vec<(usize, String)> = self
            .state
            .playlists
            .iter()
            .enumerate()
            .map(|(index, playlist)| {
                (
                    index,
                    format!("{} {}", playlist.kind.icon(), playlist.name),
                )
            })
            .collect();

        ui.horizontal(|ui| {
            let compact_controls = ui.available_width() < 520.0;
            let selector_width = if compact_controls { 96.0 } else { 120.0 };
            let selected_playlist_short_label = ellipsize_chars(
                &selected_playlist_label,
                if compact_controls { 18 } else { 24 },
            );

            egui::ComboBox::from_id_salt("track_panel_playlist_selector")
                .selected_text(selected_playlist_short_label)
                .width(selector_width)
                .height(520.0)
                .show_ui(ui, |ui| {
                    for (index, label) in playlist_options {
                        if ui
                            .selectable_label(self.state.selected_playlist_index == index, label)
                            .clicked()
                        {
                            self.select_playlist(index);
                        }
                    }
                });
            if folder_group_count > 1 && !selected_group_label.is_empty() && !compact_controls {
                ui.small(ellipsize_chars(&selected_group_label, 24));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let search_label = if self.show_track_search {
                    if compact_controls { ui_icons::icon(Icon::X) } else { self.control_label(Icon::X, "Close search") }
                } else if compact_controls {
                    ui_icons::icon(Icon::Search)
                } else {
                    self.control_label(Icon::Search, "Search")
                };
                if ui.button(search_label)
                    .on_hover_text(if self.show_track_search { "Close search" } else { "Search tracks" })
                    .clicked()
                {
                    self.show_track_search = !self.show_track_search;
                    self.focus_track_search = self.show_track_search;
                    self.state.ui.show_track_search = self.show_track_search;
                    if !self.show_track_search {
                        self.track_search_query.clear();
                        self.search_cursor = 0;
                    }
                    self.save_state_silently();
                }

                if ui.small_button("Z-A").on_hover_text("Sort current playlist Z to A").clicked() {
                    self.sort_current_playlist_by_name(false);
                }
                if ui.small_button("A-Z").on_hover_text("Sort current playlist A to Z").clicked() {
                    self.sort_current_playlist_by_name(true);
                }

                let has_active_source = self.active_track_path.is_some() || self.active_radio_index.is_some();
                let now_playing_label = if compact_controls {
                    ui_icons::icon(Icon::Music)
                } else {
                    ui_icons::label(Icon::Music, "Now playing")
                };
                if ui
                    .add_enabled(has_active_source, egui::Button::new(now_playing_label))
                    .on_hover_text("Switch to the active source and center the currently playing item")
                    .clicked()
                {
                    self.jump_to_now_playing();
                }
            });
        });

        if self.show_track_search {
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                ui.label(ui_icons::icon(Icon::Search));

                let search_input_width = ui.available_width().max(120.0);
                let response = ui.add_sized(
                    egui::vec2(search_input_width, ui.spacing().interact_size.y),
                    egui::TextEdit::singleline(&mut self.track_search_query).hint_text("Search tracks or folders"),
                );
                if self.focus_track_search {
                    response.request_focus();
                    self.focus_track_search = false;
                }
                if response.changed() {
                    self.search_cursor = 0;
                }
            });

            ui.add_space(3.0);
            ui.horizontal_wrapped(|ui| {
                let compact_item_gap = ui.spacing().item_spacing.x.min(6.0);
                ui.spacing_mut().item_spacing.x = compact_item_gap;
                let can_jump = !visible_indexes.is_empty() && !self.track_search_query.trim().is_empty();
                if search_icon_text_button(ui, can_jump, Icon::ArrowDown, "Next").clicked() {
                    let next = visible_indexes[self.search_cursor % visible_indexes.len()];
                    self.selected_track_index = Some(next);
                    self.search_cursor = (self.search_cursor + 1) % visible_indexes.len().max(1);
                }

                if search_icon_text_button(ui, true, Icon::X, "Clear").clicked() {
                    self.track_search_query.clear();
                    self.search_cursor = 0;
                }

                if ui
                    .checkbox(&mut self.search_playback_filtered_only, "Play results")
                    .on_hover_text("When enabled, Next/auto-play stays inside the current search results until search is closed. Turn it off to keep normal playlist playback while searching.")
                    .changed()
                {
                    self.state.ui.search_playback_filtered_only = self.search_playback_filtered_only;
                    self.save_state_silently();
                }
            });
            if !query.is_empty() {
                ui.add_space(2.0);
                let mode = if self.search_playback_filtered_only { "playback is limited to search results" } else { "playback keeps normal playlist order" };
                ui.small(format!("Filtering tracks by: {query} · {mode}"));
            }
        }

        if self.state.playback.repeat_mode == RepeatMode::Selection {
            ui.add_space(4.0);
            let repeat_order = if self.state.playback.shuffle_enabled { "random playback" } else { "playlist order" };
            let helper = format!("Repeat selection mode: tick tracks or whole folders for {repeat_order}.");
            let helper_width = ui.available_width().max(32.0);
            let _ = render_ellipsized_single_line(
                ui,
                &helper,
                helper_width,
                egui::TextStyle::Small.resolve(ui.style()),
                ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.78),
            );
        }

        ui.add_space(4.0);
        ui.separator();

        if visible_count == 0 {
            ui.centered_and_justified(|ui| {
                if self.show_track_search && !self.track_search_query.trim().is_empty() {
                    ui.label("No tracks match the current search.");
                } else {
                    ui.label("No tracks in this view. Add files or import a music folder.");
                }
            });
            return;
        }

        let visible_tracks: Vec<(usize, Track)> = self
            .current_playlist()
            .map(|playlist| {
                visible_indexes
                    .into_iter()
                    .filter_map(|index| playlist.tracks.get(index).map(|track| (index, track.clone())))
                    .collect()
            })
            .unwrap_or_default();

        let group_track_indexes: BTreeMap<String, Vec<usize>> = visible_tracks
            .iter()
            .fold(BTreeMap::new(), |mut groups, (index, track)| {
                groups.entry(track.group.clone()).or_default().push(*index);
                groups
            });

        let add_targets: Vec<(usize, String, PlaylistKind)> = self
            .state
            .playlists
            .iter()
            .enumerate()
            .map(|(index, playlist)| (index, playlist.name.clone(), playlist.kind.clone()))
            .collect();

        let mut last_group = String::new();
        let row_width = (ui.available_width() - 22.0).max(320.0);
        let scroll_height = ui.available_height();
        let mut reorder_track: Option<(usize, usize)> = None;
        let mut next_track_drop_target_index: Option<usize> = self.track_drop_target_index;
        let scroll_output = egui::ScrollArea::vertical()
            .id_salt("track_list_scroll")
            .vertical_scroll_offset(self.current_playlist_scroll_offset_y())
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show_viewport(ui, |ui, _viewport| {
                ui.set_width(row_width);
                let visible_rect = ui.clip_rect();
                let sticky_top = visible_rect.top();
                let mut viewport_group: Option<String> = None;
                let mut next_group_header_top: Option<f32> = None;
                let visible_track_len = visible_tracks.len();
                let track_count = self.current_playlist().map(|playlist| playlist.tracks.len()).unwrap_or(0);
                let pointer_position = ui.input(|input| input.pointer.hover_pos().or(input.pointer.interact_pos()));
                for (visible_row_index, (index, track)) in visible_tracks.iter().cloned().enumerate() {
                    let next_visible_track_index = visible_tracks
                        .get(visible_row_index + 1)
                        .map(|(next_index, _)| *next_index);
                    if show_group_headers && track.group != last_group {
                        ui.add_space(6.0);
                        let group = track.group.clone();
                        let collapsed = self.collapsed_groups.contains(&group);
                        let mut toggle_group = false;
                        let repeat_selection_mode = self.state.playback.repeat_mode == RepeatMode::Selection;
                        let group_indexes = group_track_indexes.get(&group).cloned().unwrap_or_default();
                        let header_response = ui.horizontal(|ui| {
                            let icon = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
                            if ui.small_button(ui_icons::icon(icon)).on_hover_text("Collapse/expand folder").clicked() {
                                toggle_group = true;
                            }
                            if repeat_selection_mode {
                                let mut checked = !group_indexes.is_empty()
                                    && group_indexes.iter().all(|index| self.selected_track_indexes.contains(index));
                                if ui.checkbox(&mut checked, "").on_hover_text("Include this whole folder in repeat selection").changed() {
                                    if checked {
                                        for index in &group_indexes {
                                            self.selected_track_indexes.insert(*index);
                                        }
                                    } else {
                                        for index in &group_indexes {
                                            self.selected_track_indexes.remove(index);
                                        }
                                    }
                                    self.persist_repeat_selection_for_current_playlist();
                                    self.save_state_silently();
                                }
                            }
                            ui.label(egui::RichText::new(group.as_str()).size(13.0).strong());
                        });
                        header_response.response.context_menu(|ui| {
                            if self.state.playback.repeat_mode == RepeatMode::Selection {
                                if ui.button("Select folder for repeat").clicked() {
                                    for index in &group_indexes {
                                        self.selected_track_indexes.insert(*index);
                                    }
                                    self.persist_repeat_selection_for_current_playlist();
                                    self.save_state_silently();
                                    ui.close_menu();
                                }
                                if ui.button("Remove folder from repeat").clicked() {
                                    for index in &group_indexes {
                                        self.selected_track_indexes.remove(index);
                                    }
                                    self.persist_repeat_selection_for_current_playlist();
                                    self.save_state_silently();
                                    ui.close_menu();
                                }
                                ui.separator();
                            }
                            if ui.button("Move folder up").clicked() {
                                self.move_folder_group_in_current_playlist(&group, -1);
                                ui.close_menu();
                            }
                            if ui.button("Move folder down").clicked() {
                                self.move_folder_group_in_current_playlist(&group, 1);
                                ui.close_menu();
                            }
                        });
                        if toggle_group {
                            self.toggle_folder_group_collapsed_and_focus(&group);
                        } else if header_response.response.double_clicked() {
                            self.request_folder_group_top(&group);
                        }
                        if self.scroll_to_folder_group_requested.as_deref() == Some(group.as_str()) {
                            header_response.response.scroll_to_me(Some(egui::Align::Min));
                            self.scroll_to_folder_group_requested = None;
                        }
                        let header_rect = header_response.response.rect;
                        if header_rect.top() <= sticky_top {
                            viewport_group = Some(group.clone());
                        } else if header_rect.top() > sticky_top {
                            next_group_header_top = Some(
                                next_group_header_top
                                    .map(|current| current.min(header_rect.top()))
                                    .unwrap_or(header_rect.top()),
                            );
                        }
                        ui.separator();
                        last_group = group;
                    }

                    if show_group_headers && self.collapsed_groups.contains(&track.group) {
                        continue;
                    }

                    let is_selected = self.selected_track_index == Some(index);
                    let is_active = self
                        .active_track_path
                        .as_ref()
                        .map(|active| same_path(active, &track.path))
                        .unwrap_or(false);
                    let favorite = self.is_favorite(&track.path);
                    let metadata = if self.player_only_mode {
                        format_track_metadata_player_only(&track)
                    } else {
                        format_track_metadata_compact(&track)
                    };
                    let title = if is_active {
                        format!("{} {}", ui_icons::icon(Icon::Play), track.title)
                    } else {
                        track.title.clone()
                    };
                    let path = track.path.clone();
                    let repeat_selection_mode = self.state.playback.repeat_mode == RepeatMode::Selection;

                    let row_hovered = next_row_pointer_hovered(ui, row_width, 32.0);
                    let row_response = ui.allocate_ui_with_layout(
                        egui::vec2(row_width, 32.0),
                        egui::Layout::left_to_right(egui::Align::Center),
                        |ui| {
                            if repeat_selection_mode {
                                let mut checked = self.selected_track_indexes.contains(&index);
                                if ui.checkbox(&mut checked, "").on_hover_text("Include in repeat selection").changed() {
                                    if checked {
                                        self.selected_track_indexes.insert(index);
                                    } else {
                                        self.selected_track_indexes.remove(&index);
                                    }
                                    self.persist_repeat_selection_for_current_playlist();
                                    self.save_state_silently();
                                }
                            }

                            let heart = if favorite {
                                egui::RichText::new("♥").color(egui::Color32::from_rgb(230, 70, 95)).size(15.0)
                            } else {
                                egui::RichText::new("♡").size(15.0)
                            };
                            if ui
                                .add_sized(egui::vec2(28.0, 24.0), egui::Button::new(heart))
                                .on_hover_text("Toggle favorite")
                                .clicked()
                            {
                                self.toggle_favorite(path.clone());
                            }

                            let body_width = ui.available_width().max(160.0);
                            let (body_rect, response) = ui.allocate_exact_size(
                                egui::vec2(body_width, 24.0),
                                egui::Sense::click(),
                            );

                            if is_selected {
                                ui.painter().rect_filled(body_rect, 5.0, ui.visuals().selection.bg_fill);
                            }

                            let body_padding = 8.0;
                            let text_color = if is_selected {
                                ui.visuals().selection.stroke.color
                            } else if is_active {
                                ui.visuals().selection.bg_fill
                            } else {
                                ui.visuals().widgets.inactive.fg_stroke.color
                            };
                            let title_font = egui::FontId::proportional(14.0);
                            let metadata_font = egui::FontId::proportional(12.0);
                            let metadata_color = if is_active || row_hovered {
                                ui.visuals().widgets.inactive.fg_stroke.color
                            } else {
                                ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.50)
                            };
                            let metadata_width = if metadata.is_empty() {
                                0.0
                            } else {
                                text_width(ui, &metadata, metadata_font.clone(), metadata_color).ceil()
                            };
                            let metadata_gap = if metadata.is_empty() { 0.0 } else { 6.0 };
                            let title_left = body_rect.left() + body_padding;
                            let title_right = (body_rect.right() - body_padding - metadata_width - metadata_gap)
                                .max(title_left + 24.0);
                            let title_rect = egui::Rect::from_min_max(
                                egui::pos2(title_left, body_rect.top()),
                                egui::pos2(title_right, body_rect.bottom()),
                            );

                            let title = ellipsize_to_width_exact(ui, &title, title_rect.width(), title_font.clone(), text_color);
                            ui.painter().with_clip_rect(title_rect).text(
                                egui::pos2(title_rect.left(), title_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                title,
                                title_font,
                                text_color,
                            );

                            if !metadata.is_empty() {
                                let metadata_rect = egui::Rect::from_min_max(
                                    egui::pos2(body_rect.right() - body_padding - metadata_width, body_rect.top()),
                                    egui::pos2(body_rect.right() - body_padding, body_rect.bottom()),
                                );
                                ui.painter().with_clip_rect(metadata_rect).text(
                                    egui::pos2(metadata_rect.right(), metadata_rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    metadata,
                                    metadata_font,
                                    metadata_color,
                                );
                            }

                            if response.clicked() {
                                self.selected_track_index = Some(index);
                            }
                            if response.double_clicked() {
                                self.selected_track_index = Some(index);
                                self.play_path(path.clone(), Some(index), 0.0);
                            }
                            if response.secondary_clicked() {
                                self.selected_track_index = Some(index);
                            }
                            response.context_menu(|ui| {
                                self.render_track_row_context_menu(ui, index, path.clone(), &add_targets);
                            });
                        },
                    );
                    let mut context_rect = row_response.response.rect.expand(2.0);
                    context_rect.min.x += if repeat_selection_mode { 62.0 } else { 34.0 };
                    let context_response = ui.interact(
                        context_rect,
                        ui.make_persistent_id(("track_context", index)),
                        egui::Sense::click_and_drag(),
                    );
                    if context_response.clicked() {
                        self.selected_track_index = Some(index);
                    }
                    if context_response.double_clicked() {
                        self.selected_track_index = Some(index);
                        self.play_path(path.clone(), Some(index), 0.0);
                    }
                    if context_response.drag_started() {
                        self.dragging_track_index = Some(index);
                        self.track_drop_target_index = None;
                    }
                    let pointer_over_row = pointer_position
                        .map(|position| row_response.response.rect.expand(2.0).contains(position))
                        .unwrap_or(false);
                    if let Some(from) = self.dragging_track_index {
                        if pointer_over_row {
                            let pointer_y = pointer_position
                                .map(|position| position.y)
                                .unwrap_or(row_response.response.rect.center().y);
                            let drop_after = pointer_y >= row_response.response.rect.center().y;
                            let to = if drop_after {
                                next_visible_track_index.unwrap_or(track_count)
                            } else {
                                index
                            };
                            next_track_drop_target_index = Some(to);
                            if ui.input(|input| input.pointer.any_released()) && Self::valid_drop_target(from, to) {
                                reorder_track = Some((from, to));
                                self.dragging_track_index = None;
                            }
                        }
                    }
                    let track_drop_target_for_paint = next_track_drop_target_index
                        .or(self.track_drop_target_index)
                        .filter(|to| self.dragging_track_index.map(|from| Self::valid_drop_target(from, *to)).unwrap_or(false));
                    if self.dragging_track_index == Some(index) && track_drop_target_for_paint.is_some() {
                        paint_dragged_row_fade(ui, row_response.response.rect);
                    }
                    if visible_row_index == 0 && track_drop_target_for_paint == Some(index) {
                        paint_list_edge_separator(ui, row_response.response.rect, row_width, false);
                    }
                    if row_response.response.secondary_clicked() || context_response.secondary_clicked() {
                        self.selected_track_index = Some(index);
                    }
                    context_response.context_menu(|ui| {
                        self.render_track_row_context_menu(ui, index, path.clone(), &add_targets);
                    });
                    if viewport_group.is_none() && row_response.response.rect.intersects(visible_rect) {
                        viewport_group = Some(track.group.clone());
                    }
                    if self.scroll_to_active_track_requested && is_active {
                        row_response.response.scroll_to_me(Some(egui::Align::Center));
                        self.scroll_to_active_track_requested = false;
                    }
                    if visible_row_index + 1 < visible_track_len {
                        let separator_drop_target = next_visible_track_index.unwrap_or(index + 1);
                        paint_list_separator(ui, row_width, track_drop_target_for_paint == Some(separator_drop_target));
                    } else if track_drop_target_for_paint == Some(track_count) {
                        paint_list_edge_separator(ui, row_response.response.rect, row_width, true);
                    }
                }

                if show_group_headers && self.current_playlist_scroll_offset_y() > 2.0 {
                    if let Some(group) = viewport_group {
                        let sticky_height = 24.0;
                        let push_offset_y = next_group_header_top
                            .map(|next_top| (next_top - sticky_top - sticky_height).min(0.0))
                            .unwrap_or(0.0);
                        let collapsed = self.collapsed_groups.contains(&group);
                        let (sticky_rect, sticky_icon_rect) = paint_sticky_folder_header(
                            ui,
                            visible_rect,
                            &group,
                            collapsed,
                            push_offset_y,
                        );
                        let sticky_response = ui.interact(
                            sticky_rect,
                            ui.make_persistent_id(("sticky_folder_header", group.as_str())),
                            egui::Sense::click(),
                        );
                        let clicked_icon = sticky_response.clicked()
                            && ui
                                .input(|input| input.pointer.interact_pos())
                                .map(|position| sticky_icon_rect.contains(position))
                                .unwrap_or(false);
                        if clicked_icon {
                            self.toggle_folder_group_collapsed_and_focus(&group);
                        } else if sticky_response.double_clicked() {
                            self.request_folder_group_top(&group);
                        }
                    }
                }
            });
        self.remember_current_playlist_scroll_offset(scroll_output.state.offset.y);
        self.track_drop_target_index = next_track_drop_target_index;
        if !ui.input(|input| input.pointer.primary_down()) {
            self.dragging_track_index = None;
            self.track_drop_target_index = None;
        }
        if let Some((from, to)) = reorder_track {
            self.track_drop_target_index = None;
            self.move_track_to_index_in_current_playlist(from, to);
        }
    }
    pub(crate) fn render_track_row_context_menu(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        path: PathBuf,
        add_targets: &[(usize, String, PlaylistKind)],
    ) {
        if ui.button(ui_icons::label(Icon::Play, "Play now")).clicked() {
            self.selected_track_index = Some(index);
            self.play_path(path.clone(), Some(index), 0.0);
            ui.close_menu();
        }
        if ui.button(ui_icons::label(Icon::Info, "Details")).clicked() {
            self.details_modal = Some(DetailsModal::Track(path.clone()));
            ui.close_menu();
        }
        if ui.button("Move up").clicked() {
            self.move_track_in_current_playlist(index, -1);
            ui.close_menu();
        }
        if ui.button("Move down").clicked() {
            self.move_track_in_current_playlist(index, 1);
            ui.close_menu();
        }
        if ui.button(ui_icons::label(Icon::ExternalLink, "Show in File Explorer")).clicked() {
            self.reveal_track_in_file_manager(path.clone());
            ui.close_menu();
        }

        ui.menu_button(ui_icons::label(Icon::ListPlus, "Add to playlist"), |ui| {
            for (target_index, target_name, kind) in add_targets.iter() {
                if kind.accepts_manual_tracks() {
                    let label = format!("{} {}", kind.icon(), target_name);
                    if ui.button(label).clicked() {
                        self.add_track_to_playlist(path.clone(), *target_index);
                        ui.close_menu();
                    }
                }
            }
        });

        let can_remove_from_playlist = self
            .current_playlist()
            .map(|playlist| playlist.kind != PlaylistKind::Folder)
            .unwrap_or(false);
        if ui
            .add_enabled(can_remove_from_playlist, egui::Button::new(ui_icons::label(Icon::ListMinus, "Remove from playlist")))
            .clicked()
        {
            self.remove_track_from_current_playlist(index);
            ui.close_menu();
        }
        if ui.button(ui_icons::label(Icon::Trash2, "Delete from disk")).clicked() {
            self.delete_track_from_disk(path);
            ui.close_menu();
        }
    }
}
