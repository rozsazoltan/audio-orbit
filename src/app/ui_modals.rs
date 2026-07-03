use crate::*;

impl AudioOrbitApp {
    pub(crate) fn render_panel_modal(&mut self, context: &egui::Context, panel: AppPanelModal) {
        self.render_modal_backdrop(context, "panel_modal_backdrop");
        let screen_rect = context.screen_rect();
        let outer_padding = Self::modal_outer_padding(screen_rect);
        let footer_height = self.modal_info_footer_reserved_height();
        let content_size = Self::modal_content_size(screen_rect, outer_padding, footer_height);
        let scroll_height = (content_size.y - 78.0).max(120.0);

        egui::Area::new(egui::Id::new("panel_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                Self::modal_panel_frame(outer_padding).show(ui, |ui| {
                    ui.set_min_size(content_size);
                    ui.set_max_width(content_size.x);

                    if Self::render_modal_header(ui, outer_padding.x, panel.icon(), panel.title(), panel.description()) {
                        self.close_panel_modal();
                    }

                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            match panel {
                                AppPanelModal::Settings => self.render_settings_panel_content(ui),
                                AppPanelModal::Updates => Self::render_modal_section(ui, |ui| self.render_updates_section_inner(ui, false)),
                                AppPanelModal::Backup => Self::render_modal_section(ui, |ui| self.render_backup_settings_section_inner(ui, false)),
                                AppPanelModal::About => Self::render_modal_section(ui, |ui| self.render_about_section_inner(ui, false)),
                            }
                        });
                });
            });
        self.render_modal_info_footer_fixed(context, "panel_modal_info_footer", screen_rect);
    }
    pub(crate) fn render_settings_panel_content(&mut self, ui: &mut egui::Ui) {
        Self::render_modal_section(ui, |ui| {
            ui.heading("Panels");
            ui.small("Open a separate panel. Esc or the top-right X returns to the previous panel.");
            ui.horizontal_wrapped(|ui| {
                if ui.button(ui_icons::label(Icon::RefreshCw, "Updates")).clicked() {
                    self.open_panel_modal(AppPanelModal::Updates);
                }
                if ui.button(ui_icons::label(Icon::Archive, "Backup")).clicked() {
                    self.open_panel_modal(AppPanelModal::Backup);
                }
                if ui.button(ui_icons::label(Icon::Info, "About")).clicked() {
                    self.open_panel_modal(AppPanelModal::About);
                }
            });
        });
        ui.add_space(8.0);

        Self::render_modal_section(ui, |ui| {
            self.render_playback_settings_section(ui);
        });
        ui.add_space(8.0);

        Self::render_modal_section(ui, |ui| {
            self.render_recording_settings_section(ui);
        });
        ui.add_space(8.0);

        Self::render_modal_section(ui, |ui| {
            self.render_profile_panel(ui);
        });
        ui.add_space(2.0);
    }
    pub(crate) fn modal_outer_padding(screen_rect: egui::Rect) -> egui::Vec2 {
        let horizontal = if screen_rect.width() < 560.0 { 16.0 } else { 24.0 };
        let vertical = if screen_rect.height() < 520.0 { 14.0 } else { 18.0 };
        egui::vec2(horizontal, vertical)
    }
    pub(crate) fn modal_content_size(screen_rect: egui::Rect, _outer_padding: egui::Vec2, footer_height: f32) -> egui::Vec2 {
        egui::vec2(
            screen_rect.width().max(280.0),
            (screen_rect.height() - footer_height).max(200.0),
        )
    }
    pub(crate) fn modal_panel_frame(_outer_padding: egui::Vec2) -> egui::Frame {
        egui::Frame::new()
            .fill(egui::Color32::from_rgba_unmultiplied(18, 20, 24, 238))
            .corner_radius(egui::CornerRadius::same(0))
            .inner_margin(egui::Margin::same(0))
    }
    pub(crate) fn render_modal_header(ui: &mut egui::Ui, horizontal_padding: f32, icon: Icon, title: &str, description: &str) -> bool {
        let mut close_clicked = false;
        let row_height = 34.0;

        ui.add_space(10.0);
        egui::Frame::new()
            .inner_margin(egui::Margin::symmetric(horizontal_padding as i8, 0))
            .show(ui, |ui| {
                ui.allocate_ui_with_layout(
                    egui::vec2(ui.available_width(), row_height),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.heading(ui_icons::label(icon, title));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            close_clicked = ui
                                .add_sized(
                                    egui::vec2(40.0, 30.0),
                                    egui::Button::new(egui::RichText::new(ui_icons::icon(Icon::X)).size(17.0)),
                                )
                                .on_hover_text("Close")
                                .clicked();
                        });
                    },
                );

                if !description.trim().is_empty() {
                    ui.add_space(14.0);
                    ui.add(egui::Label::new(description).wrap());
                }
            });

        ui.add_space(10.0);
        Self::render_modal_full_width_separator(ui, horizontal_padding);
        close_clicked
    }
    pub(crate) fn render_modal_full_width_separator(ui: &mut egui::Ui, _horizontal_padding: f32) {
        let stroke = ui.visuals().widgets.noninteractive.bg_stroke;
        let y = ui.cursor().top().round();
        let rect = ui.max_rect();
        ui.painter().line_segment([egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)], stroke);
        ui.add_space(1.0);
    }
    pub(crate) fn render_modal_section(ui: &mut egui::Ui, add_contents: impl FnOnce(&mut egui::Ui)) {
        egui::Frame::new()
            .fill(egui::Color32::from_black_alpha(34))
            .stroke(egui::Stroke::new(1.0, egui::Color32::from_black_alpha(58)))
            .corner_radius(egui::CornerRadius::same(0))
            .inner_margin(egui::Margin::symmetric(14, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                add_contents(ui);
            });
    }
    pub(crate) fn render_modal_backdrop(&self, context: &egui::Context, id: &'static str) {
        let screen_rect = context.screen_rect();
        let painter = context.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new(id),
        ));
        painter.rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(156));
    }
    pub(crate) fn render_details_modal(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "details_modal_backdrop");
        let details = self.details_modal.clone();
        let mut is_open = details.is_some();
        let screen_rect = context.screen_rect();
        let outer_padding = Self::modal_outer_padding(screen_rect);
        let footer_height = self.modal_info_footer_reserved_height();
        let content_size = Self::modal_content_size(screen_rect, outer_padding, footer_height);
        let scroll_height = (content_size.y - 78.0).max(120.0);

        egui::Area::new(egui::Id::new("details_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                Self::modal_panel_frame(outer_padding).show(ui, |ui| {
                    ui.set_min_size(content_size);
                    ui.set_max_width(content_size.x);

                    if Self::render_modal_header(ui, outer_padding.x, Icon::Info, "Details", "Selected item metadata and current playback information.") {
                        is_open = false;
                    }

                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            Self::render_modal_section(ui, |ui| {
                                match details {
                                    Some(DetailsModal::Track(path)) => {
                                        if let Some(track) = self.find_track_by_path(&path).cloned() {
                                            detail_row(ui, "Title", &track.title);
                                            detail_row(ui, "File", &track.path.display().to_string());
                                            detail_row(ui, "Folder", &display_parent(&track.path));
                                            detail_row(ui, "Group", &track.group);
                                            detail_row(ui, "Duration", &track.metadata.duration_seconds.map(format_duration).unwrap_or_else(|| "Unknown".to_owned()));
                                            detail_row(ui, "Sample rate", &track.metadata.sample_rate_hz.map(|value| format!("{value} Hz")).unwrap_or_else(|| "Unknown".to_owned()));
                                            detail_row(ui, "Bitrate", &track.metadata.bitrate_kbps.map(|value| format!("{value} kbps")).unwrap_or_else(|| "Unknown".to_owned()));
                                            detail_row(ui, "Channels", &track.metadata.channels.map(|value| value.to_string()).unwrap_or_else(|| "Unknown".to_owned()));
                                            detail_row(ui, "Size", &track.metadata.size_bytes.map(format_file_size).unwrap_or_else(|| "Unknown".to_owned()));
                                            detail_row(ui, "Waveform points", &track.waveform.len().to_string());
                                        } else {
                                            detail_row(ui, "File", &path.display().to_string());
                                            ui.label("This track is no longer present in the current library state.");
                                        }
                                    }
                                    Some(DetailsModal::Radio(index)) => {
                                        if let Some(station) = self.state.radio_stations.get(index) {
                                            detail_row(ui, "Name", &station.name);
                                            detail_row(ui, "Fetched station name", station.last_station_name.as_deref().unwrap_or("Unknown"));
                                            detail_row(ui, "URL", &station.url);
                                            detail_row(ui, "Favorite", if station.favorite { "Yes" } else { "No" });
                                            detail_row(ui, "Last stream title", station.last_stream_title.as_deref().unwrap_or("Unknown"));
                                            let state = if self.active_radio_index == Some(index) { "Playing" } else { "Stopped" };
                                            detail_row(ui, "State", state);
                                            if self.active_radio_index == Some(index) {
                                                detail_row(ui, "Elapsed", &self.radio_elapsed_seconds().map(format_duration).unwrap_or_else(|| "0:00".to_owned()));
                                            }
                                        } else {
                                            ui.label("This radio station is no longer available.");
                                        }
                                    }
                                    None => {}
                                }
                            });
                        });
                });
            });
        self.render_modal_info_footer_fixed(context, "details_modal_info_footer", screen_rect);

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            is_open = false;
        }
        if !is_open {
            self.details_modal = None;
        }
    }
    pub(crate) fn find_track_by_path(&self, path: &Path) -> Option<&Track> {
        self.state
            .playlists
            .iter()
            .flat_map(|playlist| playlist.tracks.iter())
            .find(|track| same_path(&track.path, path))
    }
    pub(crate) fn find_track_location(&self, path: &Path) -> Option<(usize, usize)> {
        self.state
            .playlists
            .iter()
            .enumerate()
            .find_map(|(playlist_index, playlist)| {
                playlist
                    .tracks
                    .iter()
                    .position(|track| same_path(&track.path, path))
                    .map(|track_index| (playlist_index, track_index))
            })
    }
    pub(crate) fn jump_to_now_playing(&mut self) {
        if let Some(radio_index) = self.active_radio_index {
            self.active_tab = MainContentTab::Radio;
            self.state.selected_radio_index = Some(radio_index);
            self.radio_show_favorites_only = false;
            self.radio_search_query.clear();
            self.scroll_to_active_radio_requested = true;
            self.save_state_silently();
            return;
        }

        let Some(active_path) = self.active_track_path.clone() else {
            return;
        };

        let active_location = self
            .active_playlist_index
            .and_then(|playlist_index| {
                self.state.playlists.get(playlist_index).and_then(|playlist| {
                    playlist
                        .tracks
                        .iter()
                        .position(|track| same_path(&track.path, &active_path))
                        .map(|track_index| (playlist_index, track_index))
                })
            })
            .or_else(|| self.find_track_location(&active_path));

        let Some((playlist_index, track_index)) = active_location else {
            return;
        };

        self.active_tab = MainContentTab::Music;
        self.state.selected_playlist_index = playlist_index;
        if let Some(playlist) = self.state.playlists.get_mut(playlist_index) {
            playlist.set_selected_group(None);
        }
        self.selected_track_index = Some(track_index);
        self.track_search_query.clear();
        self.search_cursor = 0;
        self.scroll_to_active_track_requested = true;
        self.save_state_silently();
    }
    pub(crate) fn render_radio_add_modal(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "radio_add_modal_backdrop");
        let mut is_open = self.show_radio_add_modal;
        let screen_rect = context.screen_rect();
        let outer_padding = Self::modal_outer_padding(screen_rect);
        let footer_height = self.modal_info_footer_reserved_height();
        let content_size = Self::modal_content_size(screen_rect, outer_padding, footer_height);
        let scroll_height = (content_size.y - 78.0).max(120.0);

        egui::Area::new(egui::Id::new("radio_add_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                Self::modal_panel_frame(outer_padding).show(ui, |ui| {
                    ui.set_min_size(content_size);
                    ui.set_max_width(content_size.x);

                    if Self::render_modal_header(
                        ui,
                        outer_padding.x,
                        Icon::Radio,
                        "Add internet radio",
                        "Add a stream URL. If the name is empty, Audio Orbit tries to read the station name from stream headers and falls back to the stream host.",
                    ) {
                        is_open = false;
                    }

                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            Self::render_modal_section(ui, |ui| {
                                let form_width = ui.available_width().min(640.0);
                                ui.label("Stream URL");
                                ui.add_sized(
                                    egui::vec2(form_width, 24.0),
                                    egui::TextEdit::singleline(&mut self.pending_radio_url).hint_text("https://..."),
                                );
                                ui.add_space(6.0);
                                ui.label("Name (optional)");
                                ui.add_sized(
                                    egui::vec2(form_width, 24.0),
                                    egui::TextEdit::singleline(&mut self.pending_radio_name).hint_text("Read from stream if empty"),
                                );
                                ui.add_space(10.0);

                                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                    if ui.button(ui_icons::label(Icon::Plus, "Add station")).clicked() {
                                        if self.add_radio_station() {
                                            is_open = false;
                                        }
                                    }
                                });
                            });
                        });
                });
            });
        self.render_modal_info_footer_fixed(context, "radio_add_modal_info_footer", screen_rect);

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            is_open = false;
        }

        self.show_radio_add_modal = is_open;
    }
    pub(crate) fn render_recording_settings_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Recording");
        ui.small("Internet radio recordings are saved from the original stream bytes before volume, orbit, silence skip, or any other playback processing.");
        let folder = self.state.recording.resolved_output_folder();
        ui.label("Radio recording folder");
        ui.horizontal_wrapped(|ui| {
            ui.monospace(folder.display().to_string());
            if ui.button(ui_icons::label(Icon::FolderOpen, "Choose folder...")).clicked() {
                self.choose_recording_folder();
            }
            if ui.button(ui_icons::label(Icon::ExternalLink, "Open current folder")).clicked() {
                self.open_recording_folder();
            }
            if ui.button("Reset default").clicked() {
                self.state.recording.output_folder = None;
                self.status_message = "Radio recording folder reset to .audio-orbit-records next to the executable.".to_owned();
                self.error_message = None;
                self.save_state_silently();
            }
        });
        if let Some(info) = self.player.as_ref().and_then(|player| player.radio_recording_info()) {
            ui.colored_label(
                egui::Color32::RED,
                format!(
                    "Recording: {} · {} · {}",
                    info.path.display(),
                    format_duration(info.started_at.elapsed().as_secs_f32()),
                    format_file_size(info.bytes_written)
                ),
            );
        }
    }
    pub(crate) fn render_playback_settings_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Playback");
        ui.small("Crossfade is a real overlap: the current track fades out while the next track fades in over the configured seconds.");

        let profile_index = self.state.selected_profile_index;
        let mut orbit_profile_changed = false;
        if let Some(profile) = self.state.profiles.get_mut(profile_index) {
            orbit_profile_changed |= ui
                .checkbox(&mut profile.settings.orbit_enabled, "Enable orbit effect")
                .on_hover_text("Disable this to use Audio Orbit as a normal stereo music player.")
                .changed();
            if !profile.settings.orbit_enabled {
                ui.small("Orbit processing is off for the active sound profile. Playback stays in normal stereo.");
            }
        }
        if orbit_profile_changed {
            self.schedule_current_profile_apply();
        }

        let mut playback_changed = false;
        playback_changed |= ui
            .checkbox(&mut self.state.playback.auto_advance, "Auto-play next")
            .changed();
        playback_changed |= ui
            .checkbox(&mut self.state.playback.shuffle_enabled, "Shuffle playback")
            .on_hover_text("Randomizes the next track inside the current playlist or repeat selection.")
            .changed();

        ui.horizontal(|ui| {
            let volume_icon = if self.effective_volume_percent() == 0 { Icon::VolumeX } else { Icon::Volume2 };
            if ui.button(ui_icons::icon(volume_icon)).on_hover_text("Mute / unmute").clicked() {
                self.toggle_mute();
            }
            let mut volume = self.state.playback.volume_percent;
            if ui
                .add(egui::Slider::new(&mut volume, 0u8..=100u8).show_value(true).suffix("%"))
                .on_hover_text("Main playback volume. Also available in player-only mode and on the top player bar mouse wheel.")
                .changed()
            {
                self.set_volume_percent(volume);
            }
        });

        ui.horizontal(|ui| {
            ui.label("Repeat");
            egui::ComboBox::from_id_salt("repeat_mode_selector")
                .selected_text(self.state.playback.repeat_mode.label())
                .show_ui(ui, |ui| {
                    playback_changed |= ui
                        .selectable_value(&mut self.state.playback.repeat_mode, RepeatMode::Off, RepeatMode::Off.label())
                        .changed();
                    playback_changed |= ui
                        .selectable_value(&mut self.state.playback.repeat_mode, RepeatMode::Track, RepeatMode::Track.label())
                        .changed();
                    playback_changed |= ui
                        .selectable_value(&mut self.state.playback.repeat_mode, RepeatMode::Selection, RepeatMode::Selection.label())
                        .changed();
                });
        });
        if self.state.playback.repeat_mode == RepeatMode::Selection {
            let repeat_order = if self.state.playback.shuffle_enabled { "at random" } else { "in playlist order" };
            ui.small(format!(
                "{} selected track(s) will repeat {repeat_order}.",
                self.selected_track_indexes.len()
            ));
        }

        playback_changed |= ui
            .checkbox(&mut self.state.playback.crossfade_enabled, "Crossfade source changes")
            .changed();
        if self.state.playback.crossfade_enabled {
            playback_changed |= ui
                .add(
                    egui::Slider::new(&mut self.state.playback.crossfade_seconds, 1u8..=20u8)
                        .text("Crossfade seconds"),
                )
                .changed();
        }
        if playback_changed {
            self.save_state_silently();
        }

        let profile_index = self.state.selected_profile_index;
        let mut profile_changed = false;
        if let Some(profile) = self.state.profiles.get_mut(profile_index) {
            profile_changed |= ui
                .checkbox(&mut profile.settings.skip_silence_enabled, "Enable silence removal")
                .on_hover_text("AIMP-style silence removal for local music files. Internet radio streams stay live and are not silence-skipped.")
                .changed();
            if profile.settings.skip_silence_enabled {
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.silence_trigger_millis, 250u16..=10000u16)
                            .text("Activation delay (ms)"),
                    )
                    .on_hover_text("A continuous silent section must last at least this long before Audio Orbit removes it. AIMP default: 2000 ms.")
                    .changed();
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.silence_threshold_db, -90i16..=-20i16)
                            .text("Detection threshold (dB)"),
                    )
                    .on_hover_text("Audio below this level is treated as silence. AIMP default shown in your screenshot: -60 dB.")
                    .changed();
                profile_changed |= ui
                    .checkbox(
                        &mut profile.settings.silence_trim_end_regardless_of_duration,
                        "Remove silence at track end regardless of duration",
                    )
                    .on_hover_text("Matches AIMP's option for trimming ending silence even when the silent tail is shorter than the activation delay.")
                    .changed();
            }
        }
        if profile_changed {
            self.schedule_current_profile_apply();
        }
    }
    pub(crate) fn render_updates_section_inner(&mut self, ui: &mut egui::Ui, show_title: bool) {
        if show_title {
            ui.heading("Updates");
        }
        ui.small(format!(
            "Checks GitHub releases for {} and can replace the current Windows executable when a newer release is available.",
            updater::repository_label()
        ));

        if self.state.update_settings.include_prereleases {
            let mut checked = true;
            ui.add_enabled(
                false,
                egui::Checkbox::new(&mut checked, "Watch prerelease builds"),
            )
            .on_hover_text("Prerelease watching can only be turned off by switching back to the latest stable release.");
            ui.small("Prerelease checks use the newest GitHub prerelease build that is newer than the current app version.");
        } else {
            let mut enable_prereleases = false;
            if ui
                .checkbox(&mut enable_prereleases, "Watch prerelease builds")
                .on_hover_text("Enables checks for the latest GitHub prerelease build. After enabling it, use Switch back to stable to return to stable releases.")
                .changed()
                && enable_prereleases
            {
                self.state.update_settings.include_prereleases = true;
                self.last_update_check = None;
                self.save_state_silently();
                self.start_update_check(true);
            }
        }

        ui.add_space(8.0);
        let checking = self.update_check_receiver.is_some();
        let installing = self.update_install_receiver.is_some();
        ui.horizontal_wrapped(|ui| {
            if ui
                .add_enabled(!checking && !installing, egui::Button::new(ui_icons::label(Icon::RefreshCw, "Check now")))
                .clicked()
            {
                self.start_update_check(true);
            }
            if self.state.update_settings.include_prereleases
                && ui
                    .add_enabled(!checking && !installing, egui::Button::new(ui_icons::label(Icon::Download, "Switch back to stable")))
                    .on_hover_text("Disables prerelease watching, downloads the latest stable release, replaces the executable, and restarts Audio Orbit.")
                    .clicked()
            {
                self.start_switch_to_stable_install();
            }
            if ui
                .add_enabled(!installing, egui::Button::new(ui_icons::label(Icon::ExternalLink, "Open releases")))
                .clicked()
            {
                self.open_releases_page();
            }
        });

        if checking {
            let elapsed = self
                .update_check_started_at
                .map(|started| started.elapsed().as_secs())
                .unwrap_or_default();
            ui.small(format!("Checking for updates... {elapsed}s"));
        }
        if installing {
            let elapsed = self
                .update_install_started_at
                .map(|started| started.elapsed().as_secs())
                .unwrap_or_default();
            ui.small(format!("Installing update... {elapsed}s"));
        }

        ui.add_space(8.0);
        let check = self.last_update_check.clone();
        if let Some(check) = check {
            ui.separator();
            ui.label(format!("Current version: v{}", check.current_version));
            ui.label(format!(
                "Latest {} version: v{}",
                if check.prerelease { "prerelease" } else { "stable" },
                check.latest_version
            ));
            ui.label(format!(
                "Release type: {}",
                if check.prerelease { "prerelease" } else { "stable" }
            ));
            if let Some(asset_name) = &check.asset_name {
                ui.label(format!("Asset: {asset_name}"));
            } else {
                ui.colored_label(egui::Color32::YELLOW, "No Windows executable asset was found for this release.");
            }

            if check.is_update_available {
                ui.colored_label(
                    egui::Color32::LIGHT_GREEN,
                    format!(
                        "A newer Audio Orbit {} release is available.",
                        if check.prerelease { "prerelease" } else { "stable" }
                    ),
                );
                if ui
                    .add_enabled(
                        !checking && !installing && check.asset_download_url.is_some(),
                        egui::Button::new(ui_icons::label(Icon::Download, "Download and install")),
                    )
                    .on_hover_text("Downloads the release executable, replaces the current executable, and restarts Audio Orbit.")
                    .clicked()
                {
                    self.start_update_install(check);
                }
            } else {
                ui.colored_label(
                    egui::Color32::LIGHT_GREEN,
                    format!(
                        "No newer {} release is available.",
                        if check.prerelease { "prerelease" } else { "stable" }
                    ),
                );
            }
        } else {
            ui.small("No update check result yet.");
        }
    }
    pub(crate) fn render_backup_settings_section_inner(&mut self, ui: &mut egui::Ui, show_title: bool) {
        if show_title {
            ui.heading("Backup and data");
        }
        ui.small("The ZIP backup stores the full app state: music folders, playlists, Favorites, sound profiles, playback settings, recording settings, and UI settings.");

        ui.horizontal_wrapped(|ui| {
            if ui.button(ui_icons::label(Icon::Download, "Export full backup ZIP")).clicked() {
                self.export_app_backup();
            }
            if ui.button(ui_icons::label(Icon::Upload, "Import backup ZIP")).clicked() {
                self.import_app_backup();
            }
        });

        if let Some(path) = app_data_dir() {
            ui.small(format!("Portable data folder: {}", path.display()));
        }
    }
    pub(crate) fn render_about_section_inner(&mut self, ui: &mut egui::Ui, show_title: bool) {
        if show_title {
            ui.heading("About Audio Orbit");
        }
        ui.add(egui::Label::new("Audio Orbit is a lightweight Windows music player focused on local libraries, folder-based playlists, smooth crossfade playback, silence skipping, and headphone-friendly orbit-style stereo movement.").wrap());
        ui.add_space(8.0);
        ui.add(egui::Label::new(format!("Version: {}", app_version_label())).wrap());
        ui.add(egui::Label::new("Creator: Zoltán Rózsa").wrap());
        ui.add(egui::Label::new("License: GNU Affero General Public License v3.0 (AGPL-3.0)").wrap());
        ui.add(egui::Label::new("This app stores its portable state next to the executable in .audio-orbit-data.").wrap());

        ui.add_space(10.0);
        ui.heading("External components");
        ui.add(egui::Label::new("RustFFT — high-performance pure Rust FFT used for Audio Orbit waveform/spectrum analysis. License: MIT OR Apache-2.0. GitHub: https://github.com/ejmahler/RustFFT").wrap());

        ui.add_space(10.0);
        ui.heading("Keyboard shortcuts");
        ui.small("AIMP-style in-app controls are available while no text field is focused.");
        ui.label("Space — Play / pause");
        ui.label("Enter — Play selected track");
        ui.label("S — Stop");
        ui.label("Left / Right — Seek 10 seconds backward / forward");
        ui.label("Ctrl + Left / Ctrl + Right — Previous / next track");
        ui.label("Ctrl + F — Show or hide track search");
        ui.label("Ctrl + Z — Undo last track/radio order change");
        ui.label("M — Player-only / full layout");
        ui.label("Ctrl + L — Show or hide Library panel");
        ui.label("Ctrl + P — Show or hide Sound profiles panel");
    }
    pub(crate) fn modal_info_footer_reserved_height(&self) -> f32 {
        24.0
    }
    pub(crate) fn render_modal_info_footer_fixed(&self, context: &egui::Context, id: &'static str, modal_rect: egui::Rect) {
        let footer_height = self.modal_info_footer_reserved_height();
        let top_left = egui::pos2(modal_rect.left(), modal_rect.bottom() - footer_height);
        let style = context.style();
        let footer_fill = style.visuals.panel_fill;
        let footer_stroke = style.visuals.widgets.noninteractive.bg_stroke;
        let horizontal_padding = 8.0;
        let vertical_padding = 2.0;

        egui::Area::new(egui::Id::new(id))
            .order(egui::Order::Foreground)
            .fixed_pos(top_left)
            .show(context, |ui| {
                egui::Frame::new()
                    .fill(footer_fill)
                    .stroke(footer_stroke)
                    .corner_radius(egui::CornerRadius::same(0))
                    .inner_margin(egui::Margin::symmetric(horizontal_padding as i8, vertical_padding as i8))
                    .show(ui, |ui| {
                        let content_width = (modal_rect.width() - horizontal_padding * 2.0).max(180.0);
                        let content_height = (footer_height - vertical_padding * 2.0).max(18.0);
                        ui.set_min_size(egui::vec2(content_width, content_height));
                        ui.set_width(content_width);
                        ui.set_height(content_height);
                        self.render_status_panel_contents(ui, true);
                    });
            });
    }
}
