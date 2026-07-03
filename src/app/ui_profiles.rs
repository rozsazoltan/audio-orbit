use crate::*;

impl AudioOrbitApp {
    pub(crate) fn render_profile_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Sound profiles");
        ui.small("Changes are applied to the current track without restarting playback.");
        ui.separator();

        let profile_names: Vec<String> = self
            .state
            .profiles
            .iter()
            .map(|profile| profile.name.clone())
            .collect();
        let selected_profile_name = profile_names
            .get(self.state.selected_profile_index)
            .map(String::as_str)
            .unwrap_or("No profile");

        let previous_profile_index = self.state.selected_profile_index;
        egui::ComboBox::from_id_salt("profile_selector")
            .selected_text(selected_profile_name)
            .height(360.0)
            .show_ui(ui, |ui| {
                for (index, profile_name) in profile_names.iter().enumerate() {
                    ui.selectable_value(
                        &mut self.state.selected_profile_index,
                        index,
                        profile_name.as_str(),
                    );
                }
            });
        if previous_profile_index != self.state.selected_profile_index {
            self.save_state_silently();
            self.schedule_current_profile_apply();
        }

        ui.horizontal_wrapped(|ui| {
            if ui.button(ui_icons::label(Icon::Plus, "New profile")).clicked() {
                self.add_profile();
            }

            if ui.button(ui_icons::label(Icon::Trash2, "Remove")).clicked() {
                self.remove_current_profile();
            }

            if ui.small_button(ui_icons::icon(Icon::Pencil)).on_hover_text("Rename profile").clicked() {
                self.editing_profile_index = Some(self.state.selected_profile_index);
            }
        });

        ui.add_space(8.0);

        let mut profile_changed = false;
        if let Some(profile) = self.state.profiles.get_mut(self.state.selected_profile_index) {
            if self.editing_profile_index == Some(self.state.selected_profile_index) {
                ui.label("Profile name");
                profile_changed |= ui.text_edit_singleline(&mut profile.name).changed();
            }

            ui.add_space(6.0);
            profile_changed |= ui
                .checkbox(&mut profile.settings.orbit_enabled, "Enable orbit mode")
                .on_hover_text("Turn off orbit processing and keep normal stereo playback while preserving other playback settings.")
                .changed();

            ui.add_enabled_ui(profile.settings.orbit_enabled, |ui| {
                ui.label("Orbit mode");
                profile_changed |= ui
                    .radio_value(
                        &mut profile.settings.mode,
                        OrbitMode::SmoothStereoOrbit,
                        OrbitMode::SmoothStereoOrbit.label(),
                    )
                    .changed();
                profile_changed |= ui
                    .radio_value(
                        &mut profile.settings.mode,
                        OrbitMode::VirtualEightDirectionOrbit,
                        OrbitMode::VirtualEightDirectionOrbit.label(),
                    )
                    .changed();
                ui.small(profile.settings.mode.description());
            });

            ui.add_space(8.0);
            profile_changed |= ui
                .add(
                    egui::Slider::new(&mut profile.settings.output_level_percent, 1u8..=100u8)
                        .text("Output Level (%)"),
                )
                .changed();
            ui.add_enabled_ui(profile.settings.orbit_enabled, |ui| {
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.stereo_width_percent, 0u8..=100u8)
                            .text("Stereo Width (%)"),
                    )
                    .changed();
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.orbit_speed_percent, 10u8..=200u8)
                            .text("Orbit Speed (%)"),
                    )
                    .changed();
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.transition_smoothness_percent, 0u8..=100u8)
                            .text("Motion Smoothness (%)"),
                    )
                    .changed();
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.depth_cue_percent, 0u8..=100u8)
                            .text("Surround Cue Strength (%)"),
                    )
                    .changed();
            });

        }

        if profile_changed {
            self.schedule_current_profile_apply();
        }

        if self.active_tab != MainContentTab::Radio {
            ui.add_space(12.0);
            self.render_profile_transition_section(ui);
        }

        ui.add_space(12.0);
        ui.separator();
        ui.horizontal_wrapped(|ui| {
            if ui.small_button(ui_icons::icon(Icon::RefreshCw)).on_hover_text("Refresh output device").clicked() {
                self.refresh_output_device();
            }
            ui.label(self.last_known_output_name.as_str());
        });
        ui.add_space(16.0);
    }
    pub(crate) fn render_profile_transition_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Playback transitions");
        ui.small("Crossfade and silence skipping are kept near the active sound profile because they affect how this profile feels during playback. Crossfade also applies when switching between music and internet radio sources.");

        let mut playback_changed = false;
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
}
