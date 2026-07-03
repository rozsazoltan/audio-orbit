use crate::*;

impl AudioOrbitApp {
    pub(crate) fn control_label(&self, icon: Icon, text: &str) -> String {
        if self.player_only_mode {
            ui_icons::icon(icon)
        } else {
            ui_icons::label(icon, text)
        }
    }
    pub(crate) fn render_transport_controls(
        &mut self,
        ui: &mut egui::Ui,
        radio_controls_active: bool,
        context_is_active: bool,
        play_label: String,
        transport_button_size: egui::Vec2,
        play_button_size: egui::Vec2,
        stop_button_size: egui::Vec2,
    ) {
        if !radio_controls_active {
            if ui
                .add_enabled(
                    self.player.is_some(),
                    egui::Button::new(self.control_label(Icon::SkipBack, "Previous")).min_size(transport_button_size),
                )
                .clicked()
            {
                self.play_previous_track();
            }
        }

        if ui
            .add_enabled(
                self.player.is_some(),
                egui::Button::new(play_label).min_size(play_button_size),
            )
            .clicked()
        {
            if context_is_active {
                self.pause_or_resume();
            } else if radio_controls_active {
                if let Some(index) = self.state.selected_radio_index {
                    self.play_radio_station(index);
                }
            } else {
                self.play_selected_or_first_track();
            }
        }

        if ui
            .add_enabled(
                self.player.is_some(),
                egui::Button::new(self.control_label(Icon::Square, "Stop")).min_size(stop_button_size),
            )
            .clicked()
        {
            self.stop();
        }

        if !radio_controls_active {
            if ui
                .add_enabled(
                    self.player.is_some(),
                    egui::Button::new(self.control_label(Icon::SkipForward, "Next")).min_size(transport_button_size),
                )
                .clicked()
            {
                self.play_next_track();
            }
        }
    }
    pub(crate) fn render_playback_mode_or_recording_controls(
        &mut self,
        ui: &mut egui::Ui,
        radio_controls_active: bool,
        icon_button_size: egui::Vec2,
    ) {
        if !radio_controls_active {
            self.render_compact_playback_toggles(ui);
            return;
        }

        let is_recording = self.player.as_ref().map(|player| player.is_radio_recording()).unwrap_or(false);
        let blink_on = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| (duration.as_millis() / 500) % 2 == 0)
            .unwrap_or(true);
        let record_icon_color = if is_recording && blink_on {
            egui::Color32::from_rgb(255, 72, 72)
        } else if is_recording {
            egui::Color32::from_rgb(180, 54, 54)
        } else {
            ui.visuals().widgets.inactive.fg_stroke.color
        };
        let record_text = egui::RichText::new(ui_icons::icon(Icon::Mic))
            .size(14.0)
            .color(record_icon_color);
        let record_button = if is_recording {
            egui::Button::new(record_text)
                .min_size(icon_button_size)
                .fill(egui::Color32::from_rgb(82, 24, 28))
        } else {
            egui::Button::new(record_text).min_size(icon_button_size)
        };
        let record_response = ui
            .add_sized(icon_button_size, record_button)
            .on_hover_text(if is_recording {
                "Stop and save radio recording · Right-click to open the recordings folder"
            } else {
                "Record original internet radio stream · Right-click to open the recordings folder"
            });
        if record_response.clicked() {
            self.toggle_radio_recording();
        }
        if record_response.secondary_clicked() {
            self.open_recording_folder();
        }
    }
    pub(crate) fn copy_current_radio_title(&mut self) {
        let Some(radio_index) = self.active_radio_index else {
            self.error_message = Some("Start an internet radio station before copying track info.".to_owned());
            return;
        };

        let text = self
            .active_radio_title
            .clone()
            .filter(|title| !title.trim().is_empty())
            .or_else(|| {
                self.active_radio_station_name
                    .clone()
                    .filter(|name| !name.trim().is_empty())
            })
            .or_else(|| self.state.radio_stations.get(radio_index).map(|station| station.name.clone()))
            .unwrap_or_else(|| "Internet radio".to_owned());

        self.pending_clipboard_text = Some(text.clone());
        self.status_message = format!("Radio info copied: {text}.");
        self.error_message = None;
    }
    pub(crate) fn render_copy_and_volume_controls(&mut self, ui: &mut egui::Ui, _has_now_playing: bool, icon_button_size: egui::Vec2) {
        if self.active_radio_index.is_some() {
            let copy_button_size = egui::vec2(if self.player_only_mode { 42.0 } else { 58.0 }, icon_button_size.y);
            if ui
                .add_sized(copy_button_size, egui::Button::new("Copy"))
                .on_hover_text("Copy the current radio stream title. Falls back to the station name when no title is available.")
                .clicked()
            {
                self.copy_current_radio_title();
            }
        }

        let volume_icon = if self.effective_volume_percent() == 0 { Icon::VolumeX } else { Icon::Volume2 };
        if ui
            .add_sized(icon_button_size, egui::Button::new(egui::RichText::new(ui_icons::icon(volume_icon)).size(14.0)))
            .on_hover_text("Mute / unmute")
            .clicked()
        {
            self.toggle_mute();
        }
        let mut volume = self.state.playback.volume_percent;
        let slider_width = if self.player_only_mode { 86.0 } else { 128.0 };
        if ui
            .add_sized(
                egui::vec2(slider_width, 18.0),
                egui::Slider::new(&mut volume, 0u8..=100u8)
                    .show_value(true)
                    .suffix("%"),
            )
            .on_hover_text("Volume. You can also use the mouse wheel over the top player bar.")
            .changed()
        {
            self.set_volume_percent(volume);
        }
    }
    pub(crate) fn render_now_playing_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let has_now_playing = self.active_track_path.is_some()
            || self.pending_track_switch.is_some()
            || self.active_radio_index.is_some();

        ui.horizontal(|ui| {
            let controls_width = if self.player_only_mode { 74.0 } else { 164.0 };
            let title_width = (ui.available_width() - controls_width).max(140.0);
            let (title_rect, title_response) = ui.allocate_exact_size(
                egui::vec2(title_width, 54.0),
                egui::Sense::hover(),
            );
            let title_available_width = title_width - 10.0;
            let active_title_color = ui.visuals().widgets.inactive.fg_stroke.color;
            let active_detail_color = ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.76);
            let active_time_color = ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.68);
            let placeholder_title_color = ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.82);
            let placeholder_detail_color = ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.62);
            let placeholder_time_color = ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.52);
            let title_font = egui::FontId::proportional(15.0);
            let detail_font = egui::FontId::proportional(11.5);
            let time_font = egui::FontId::proportional(11.5);
            let (title, detail, time_label, title_color, detail_color, time_color) = if has_now_playing {
                (
                    ellipsize_to_width_exact(ui, &self.active_track_title(), title_available_width, title_font.clone(), active_title_color),
                    self.active_track_detail()
                        .map(|value| ellipsize_to_width_exact(ui, &value, title_available_width, detail_font.clone(), active_detail_color))
                        .unwrap_or_default(),
                    ellipsize_to_width_exact(ui, &self.active_track_time_label(), title_available_width, time_font.clone(), active_time_color),
                    active_title_color,
                    active_detail_color,
                    active_time_color,
                )
            } else {
                (
                    ellipsize_to_width_exact(ui, "Audio Orbit is ready", title_available_width, title_font.clone(), placeholder_title_color),
                    ellipsize_to_width_exact(ui, "Choose a song, start a playlist, or tune in to internet radio.", title_available_width, detail_font.clone(), placeholder_detail_color),
                    ellipsize_to_width_exact(ui, "Local music · Live radio · Sound profiles", title_available_width, time_font.clone(), placeholder_time_color),
                    placeholder_title_color,
                    placeholder_detail_color,
                    placeholder_time_color,
                )
            };

            let painter = ui.painter();
            painter.text(
                egui::pos2(title_rect.left() + 2.0, title_rect.top() + 10.0),
                egui::Align2::LEFT_CENTER,
                title,
                title_font,
                title_color,
            );
            if !detail.is_empty() {
                painter.text(
                    egui::pos2(title_rect.left() + 2.0, title_rect.top() + 25.0),
                    egui::Align2::LEFT_CENTER,
                    detail,
                    detail_font,
                    detail_color,
                );
            }
            if !time_label.is_empty() {
                painter.text(
                    egui::pos2(title_rect.left() + 2.0, title_rect.top() + 40.0),
                    egui::Align2::LEFT_CENTER,
                    time_label,
                    time_font,
                    time_color,
                );
            }
            title_response.on_hover_text("Mouse wheel over the top player bar adjusts volume.");

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let settings_label = self.control_label(Icon::Settings2, "Settings");
                if ui.button(settings_label).on_hover_text("Settings").clicked() {
                    self.open_panel_modal(AppPanelModal::Settings);
                }

                let player_only_label = if self.player_only_mode {
                    self.control_label(Icon::ListMusic, "Full layout")
                } else {
                    self.control_label(Icon::Music, "Player only")
                };
                if ui.button(player_only_label).clicked() {
                    self.toggle_player_only_mode(ui.ctx());
                }

                if !self.player_only_mode {
                    let profile_label = if self.show_profile_panel {
                        ui_icons::label(Icon::Settings2, "Hide profiles")
                    } else {
                        ui_icons::label(Icon::Settings2, "Show profiles")
                    };
                    if ui.button(profile_label).clicked() {
                        self.show_profile_panel = !self.show_profile_panel;
                        self.state.ui.show_profile_panel = self.show_profile_panel;
                        self.save_state_silently();
                    }

                    let library_label = if self.show_library_panel {
                        ui_icons::label(Icon::ListMusic, "Hide library")
                    } else {
                        ui_icons::label(Icon::ListMusic, "Show library")
                    };
                    if ui.button(library_label).clicked() {
                        self.show_library_panel = !self.show_library_panel;
                        self.state.ui.show_library_panel = self.show_library_panel;
                        self.save_state_silently();
                    }
                }

            });
        });

        if self.active_radio_index.is_some() {
            let available_width = ui.available_width().max(96.0);
            let visible_seconds = (available_width / RADIO_WAVEFORM_PIXELS_PER_SECOND)
                .clamp(1.0, RADIO_WAVEFORM_MAX_VISIBLE_SECONDS);
            let requested_points = (available_width / RADIO_WAVEFORM_BAR_PITCH_PIXELS)
                .floor()
                .clamp(1.0, 900.0) as usize + 1;
            let frame = self
                .player
                .as_ref()
                .map(|player| player.radio_visualizer_frame(requested_points, visible_seconds))
                .unwrap_or_default();
            let response = draw_radio_waveform_strip(ui, &frame);
            response.on_hover_text(format!(
                "Internet radio streams are live: this is a rolling waveform history of about {:.0}s. It is not seekable.",
                visible_seconds
            ));
        } else if has_now_playing {
            let position = self.displayed_playback_position_seconds();
            let duration = self.displayed_playback_duration_seconds();
            let visual_position = self.waveform_drag_position_seconds.unwrap_or(position);
            let progress = if duration > 0.0 {
                (visual_position / duration).clamp(0.0, 1.0)
            } else {
                0.0
            };

            let waveform = self
                .last_playback
                .as_ref()
                .map(|playback| playback.waveform.as_slice())
                .unwrap_or(&[]);
            let waveform_brightness = self
                .last_playback
                .as_ref()
                .map(|playback| playback.waveform_brightness.as_slice())
                .unwrap_or(&[]);
            let silence_ranges = self
                .last_playback
                .as_ref()
                .map(|playback| playback.silence_ranges.as_slice())
                .unwrap_or(&[]);
            let marker_duration = self
                .last_playback
                .as_ref()
                .map(|playback| playback.original_duration_seconds)
                .unwrap_or(duration);
            let response = draw_waveform_seek(
                ui,
                waveform,
                waveform_brightness,
                progress,
                silence_ranges,
                marker_duration,
                waveform.is_empty(),
            );
            if duration > 0.0 {
                let pointer_position = response.interact_pointer_pos();
                if response.drag_started() || response.dragged() {
                    if let Some(pointer) = pointer_position {
                        let next_position = ((pointer.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0) * duration;
                        self.waveform_drag_position_seconds = Some(next_position);
                    }
                }

                if response.drag_stopped() {
                    let next_position = self.waveform_drag_position_seconds.or_else(|| {
                        pointer_position.map(|pointer| {
                            ((pointer.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0) * duration
                        })
                    });
                    if let Some(next_position) = next_position {
                        self.seek_current(next_position);
                    }
                    self.waveform_drag_position_seconds = None;
                } else if response.clicked() {
                    if let Some(pointer) = pointer_position {
                        let next_position = ((pointer.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0) * duration;
                        self.seek_current(next_position);
                    }
                    self.waveform_drag_position_seconds = None;
                } else if !response.hovered() && !ui.input(|input| input.pointer.primary_down()) {
                    self.waveform_drag_position_seconds = None;
                }
            }
        } else {
            let response = draw_waveform_seek(ui, &[], &[], 0.0, &[], 0.0, false);
            response.on_hover_text("No track is currently playing.");
        }

        let radio_controls_active = self.active_tab == MainContentTab::Radio;
        let context_is_active = self
            .player
            .as_ref()
            .map(|player| {
                let matching_source_active = if radio_controls_active {
                    self.active_radio_index.is_some()
                } else {
                    self.active_track_path.is_some() || self.pending_track_switch.is_some()
                };
                matching_source_active && (player.is_playing() || player.is_paused())
            })
            .unwrap_or(false);

        let play_label = match self.player.as_ref() {
            Some(player) if context_is_active && player.is_playing() => self.control_label(Icon::Pause, "Pause"),
            Some(player) if context_is_active && player.is_paused() => self.control_label(Icon::Play, "Resume"),
            _ => self.control_label(Icon::Play, "Play"),
        };

        let icon_button_size = egui::vec2(24.0, 24.0);
        let transport_button_size = if self.player_only_mode { icon_button_size } else { egui::vec2(84.0, 24.0) };
        let play_button_size = if self.player_only_mode { icon_button_size } else { egui::vec2(70.0, 24.0) };
        let stop_button_size = if self.player_only_mode { icon_button_size } else { egui::vec2(64.0, 24.0) };

        let control_width = ui.available_width();
        if control_width < 360.0 {
            ui.vertical(|ui| {
                ui.horizontal(|ui| {
                    self.render_transport_controls(
                        ui,
                        radio_controls_active,
                        context_is_active,
                        play_label.clone(),
                        transport_button_size,
                        play_button_size,
                        stop_button_size,
                    );
                });
                ui.horizontal(|ui| {
                    self.render_playback_mode_or_recording_controls(ui, radio_controls_active, icon_button_size);
                });
                ui.horizontal(|ui| {
                    self.render_copy_and_volume_controls(ui, has_now_playing, icon_button_size);
                });
            });
        } else if control_width < 620.0 {
            ui.vertical(|ui| {
                ui.horizontal_wrapped(|ui| {
                    self.render_transport_controls(
                        ui,
                        radio_controls_active,
                        context_is_active,
                        play_label.clone(),
                        transport_button_size,
                        play_button_size,
                        stop_button_size,
                    );
                    self.render_playback_mode_or_recording_controls(ui, radio_controls_active, icon_button_size);
                });
                ui.horizontal(|ui| {
                    self.render_copy_and_volume_controls(ui, has_now_playing, icon_button_size);
                });
            });
        } else {
            ui.horizontal_wrapped(|ui| {
                self.render_transport_controls(
                    ui,
                    radio_controls_active,
                    context_is_active,
                    play_label,
                    transport_button_size,
                    play_button_size,
                    stop_button_size,
                );
                self.render_playback_mode_or_recording_controls(ui, radio_controls_active, icon_button_size);
                self.render_copy_and_volume_controls(ui, has_now_playing, icon_button_size);
            });
        }

        if let Some(output_name) = self.detected_output_change.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("Output changed to {output_name}."),
                );
                if ui.button(ui_icons::label(Icon::RefreshCw, "Refresh and continue")).clicked() {
                    self.refresh_output_device();
                }
            });
        }

        ui.add_space(6.0);
    }
    pub(crate) fn render_compact_playback_toggles(&mut self, ui: &mut egui::Ui) {
        let mut playback_changed = false;
        let repeat_active = self.state.playback.repeat_mode != RepeatMode::Off;
        let repeat_button_label = if self.player_only_mode {
            "↻".to_owned()
        } else {
            format!("↻ {}", self.state.playback.repeat_mode.label())
        };
        let repeat_button = if repeat_active {
            egui::Button::new(egui::RichText::new(repeat_button_label).strong())
                .fill(ui.visuals().selection.bg_fill)
                .stroke(ui.visuals().selection.stroke)
        } else {
            egui::Button::new(repeat_button_label)
        };
        if ui
            .add(repeat_button)
            .on_hover_text(format!("Cycle repeat mode. Current: {}", self.state.playback.repeat_mode.label()))
            .clicked()
        {
            self.state.playback.repeat_mode = self.state.playback.repeat_mode.next();
            playback_changed = true;
        }
        playback_changed |= ui
            .checkbox(&mut self.state.playback.auto_advance, if self.player_only_mode { "Auto" } else { "Auto-play next" })
            .changed();
        playback_changed |= ui
            .checkbox(&mut self.state.playback.shuffle_enabled, "Shuffle")
            .on_hover_text("Pick a random next track from the active playlist or repeat selection.")
            .changed();
        if playback_changed {
            self.save_state_silently();
        }

    }
}
