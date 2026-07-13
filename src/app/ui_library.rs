use crate::*;

impl AudioOrbitApp {
    pub(crate) fn render_library_panel(&mut self, ui: &mut egui::Ui) {
        if self.active_tab == MainContentTab::Radio {
            ui.heading("Internet radio");
            ui.add(egui::Label::new("Local music library is not available while browsing internet radio.").wrap());
            ui.small("Use this side panel to switch between all radio stations and favorite stations.");
            ui.separator();
            let favorite_count = self.state.radio_stations.iter().filter(|station| station.favorite).count();
            if ui
                .selectable_label(!self.radio_show_favorites_only, format!("All stations ({})", self.state.radio_stations.len()))
                .clicked()
            {
                self.radio_show_favorites_only = false;
            }
            if ui
                .selectable_label(self.radio_show_favorites_only, format!("Favorite stations ({favorite_count})"))
                .clicked()
            {
                self.radio_show_favorites_only = true;
            }
            ui.separator();
            if ui.button(ui_icons::label(Icon::Settings2, "Settings...")).clicked() {
                self.open_panel_modal(AppPanelModal::Settings);
            }
            ui.add_space(16.0);
            return;
        }

        ui.heading("Library");
        ui.small("Folder scanners, manual playlists, and Favorites.");
        ui.separator();

        let playlist_list_width = (ui.available_width() - 16.0).max(220.0);
        egui::ScrollArea::vertical()
            .max_height(260.0)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(playlist_list_width);
                for index in 0..self.state.playlists.len() {
                    let selected = self.state.selected_playlist_index == index;
                    let playlist = self.state.playlists[index].clone();
                    let show_actions = selected;

                    let row = ui.horizontal(|ui| {
                        let mut clicked_row = false;
                        let action_width = if show_actions && playlist.kind != PlaylistKind::Favorites { 88.0 } else { 0.0 };
                        let name_width = (ui.available_width() - action_width).max(120.0);

                        if self.editing_playlist_index == Some(index) {
                            let mut next_name = playlist.name.clone();
                            let response = ui.add_sized(
                                egui::vec2(name_width, 22.0),
                                egui::TextEdit::singleline(&mut next_name),
                            );
                            if response.changed() {
                                if let Some(target) = self.state.playlists.get_mut(index) {
                                    if target.kind != PlaylistKind::Favorites {
                                        target.name = next_name;
                                    }
                                }
                                self.save_state_silently();
                            }
                            if response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter)) {
                                self.editing_playlist_index = None;
                            }
                        } else {
                            ui.allocate_ui_with_layout(
                                egui::vec2(name_width, 42.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    let label = format!("{} {}", playlist.kind.icon(), playlist.name);
                                    if ui.selectable_label(selected, label).clicked() {
                                        clicked_row = true;
                                    }
                                    ui.small(format!("{} track(s)", playlist.tracks.len()));
                                },
                            );
                        }

                        if show_actions && playlist.kind != PlaylistKind::Favorites {
                            if ui.small_button(ui_icons::icon(Icon::Pencil)).on_hover_text("Rename").clicked() {
                                self.editing_playlist_index = Some(index);
                            }
                            if ui.small_button(ui_icons::icon(Icon::ArrowUp)).on_hover_text("Move up").clicked() {
                                self.move_playlist(index, -1);
                            }
                            if ui.small_button(ui_icons::icon(Icon::ArrowDown)).on_hover_text("Move down").clicked() {
                                self.move_playlist(index, 1);
                            }
                        }

                        clicked_row
                    });

                    if row.inner {
                        self.select_playlist(index);
                    }
                }
            });

        ui.horizontal(|ui| {
            if ui.button(ui_icons::label(Icon::ListPlus, "New playlist")).clicked() {
                self.add_playlist();
            }
            let can_remove = self.current_playlist().map(|playlist| playlist.kind.can_delete()).unwrap_or(false);
            if ui.add_enabled(can_remove, egui::Button::new(ui_icons::label(Icon::Trash2, "Remove"))).clicked() {
                self.remove_current_playlist();
            }
        });

        ui.separator();
        self.render_current_playlist_controls(ui);

        ui.separator();
        ui.horizontal(|ui| {
            let can_add_files = self.current_playlist().map(|playlist| playlist.accepts_manual_tracks()).unwrap_or(false);
            if ui.add_enabled(can_add_files, egui::Button::new(ui_icons::label(Icon::FilePlus2, "Add files..."))).clicked() {
                self.add_audio_files();
            }
            if ui.button(ui_icons::label(Icon::FolderPlus, "Add folder...")).clicked() {
                self.open_folder_import_modal();
            }
        });

        ui.horizontal_wrapped(|ui| {
            let scan_idle = self.pending_folder_scan_receiver.is_none()
                && self.pending_library_sync_receiver.is_none();
            if ui
                .add_enabled(
                    scan_idle,
                    egui::Button::new(ui_icons::label(Icon::RefreshCw, "Sync playlist")),
                )
                .on_hover_text("Check only selected playlist. Folder playlists scan their source folder; manual playlists check saved files only.")
                .clicked()
            {
                self.start_library_sync(LibrarySyncTrigger::Manual, true);
            }
        });

        ui.separator();
        if ui.button(ui_icons::label(Icon::Settings2, "Settings...")).clicked() {
            self.open_panel_modal(AppPanelModal::Settings);
        }
        ui.add_space(16.0);
    }
    pub(crate) fn render_current_playlist_controls(&mut self, ui: &mut egui::Ui) {
        let Some(playlist) = self.current_playlist() else {
            return;
        };

        ui.small(format!("Type: {} {}", playlist.kind.icon(), playlist.kind.label()));

        let groups = playlist.folder_groups();
        let selected_group = playlist.selected_group.clone();
        let selected_label = playlist.selected_group.clone().unwrap_or_else(|| "Folder filter".to_owned());
        let source_folder = playlist.source_folder.clone();
        let folder_depth = playlist.folder_depth;

        if let Some(folder) = source_folder {
            ui.small(format!("Folder: {}", folder.display()));
            ui.small(format!("Grouping depth: {folder_depth} folder level(s)"));
        }

        if groups.len() > 1 {
            let mut next_group = selected_group.clone();
            let group_dropdown_height = ((groups.len() + 1) as f32 * 24.0 + 36.0).clamp(180.0, 640.0);
            egui::ComboBox::from_id_salt("folder_group_selector")
                .selected_text(ellipsize_chars(&selected_label, 22))
                .width(120.0)
                .height(group_dropdown_height)
                .show_ui(ui, |ui| {
                    if ui
                        .selectable_label(next_group.is_none(), "Show all")
                        .clicked()
                    {
                        next_group = None;
                    }

                    for group in groups {
                        let is_selected = next_group.as_ref() == Some(&group);
                        if ui.selectable_label(is_selected, group.as_str()).clicked() {
                            next_group = Some(group);
                        }
                    }
                });

            if next_group != selected_group {
                self.remember_current_playlist_scroll_offset(self.state.ui.playlist_scroll_offset_y);
                if let Some(playlist) = self.current_playlist_mut() {
                    playlist.set_selected_group(next_group);
                }
                let restored_offset = self.current_playlist_scroll_offset_y();
                self.state.ui.playlist_scroll_offset_y = restored_offset;
                self.ensure_selected_track_visible();
                self.save_state_silently();
            }
        }
    }
    pub(crate) fn render_main_content_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.selectable_label(self.active_tab == MainContentTab::Music, ui_icons::label(Icon::Music, "Music")).clicked() {
                self.active_tab = MainContentTab::Music;
                self.save_state_silently();
            }
            if ui.selectable_label(self.active_tab == MainContentTab::Radio, ui_icons::label(Icon::Radio, "Internet radio")).clicked() {
                self.active_tab = MainContentTab::Radio;
                self.save_state_silently();
            }
        });
        ui.separator();

        match self.active_tab {
            MainContentTab::Music => self.render_track_panel(ui),
            MainContentTab::Radio => self.render_radio_panel(ui),
        }
    }
}
