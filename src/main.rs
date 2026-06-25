#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_player;
mod config;
mod dsp;
mod icon;

use crate::{
    audio_player::{AudioPlayer, PlaybackInfo},
    config::{load_state, save_state, DspProfile, Playlist, SavedState},
    dsp::{DspSettings, OrbitMode},
};
use eframe::egui;
use rfd::FileDialog;
use std::path::{Path, PathBuf};

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([780.0, 650.0])
        .with_min_inner_size([720.0, 600.0])
        .with_resizable(true);

    if let Some(icon) = icon::load_window_icon() {
        viewport = viewport.with_icon(icon);
    }

    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        &format!("Audio Orbit v{}", env!("CARGO_PKG_VERSION")),
        options,
        Box::new(|_creation_context| Ok(Box::new(AudioOrbitApp::new()))),
    )
}

struct AudioOrbitApp {
    player: Option<AudioPlayer>,
    state: SavedState,
    selected_track_index: Option<usize>,
    active_track_index: Option<usize>,
    active_settings: Option<DspSettings>,
    last_playback: Option<PlaybackInfo>,
    status_message: String,
    error_message: Option<String>,
    settings_changed_during_playback: bool,
    auto_advance: bool,
}

impl AudioOrbitApp {
    fn new() -> Self {
        let mut state = load_state();
        ensure_state_is_valid(&mut state);

        match AudioPlayer::new() {
            Ok(player) => {
                let output_name = player.output_device_name().to_owned();
                Self {
                    player: Some(player),
                    state,
                    selected_track_index: None,
                    active_track_index: None,
                    active_settings: None,
                    last_playback: None,
                    status_message: format!("Ready. Output device: {output_name}"),
                    error_message: None,
                    settings_changed_during_playback: false,
                    auto_advance: true,
                }
            }
            Err(error) => Self {
                player: None,
                state,
                selected_track_index: None,
                active_track_index: None,
                active_settings: None,
                last_playback: None,
                status_message: "No audio output device is available.".to_owned(),
                error_message: Some(error.to_string()),
                settings_changed_during_playback: false,
                auto_advance: true,
            },
        }
    }

    fn current_playlist(&self) -> Option<&Playlist> {
        self.state.playlists.get(self.state.selected_playlist_index)
    }

    fn current_playlist_mut(&mut self) -> Option<&mut Playlist> {
        self.state.playlists.get_mut(self.state.selected_playlist_index)
    }

    fn current_profile(&self) -> Option<&DspProfile> {
        self.state.profiles.get(self.state.selected_profile_index)
    }

    fn current_profile_mut(&mut self) -> Option<&mut DspProfile> {
        self.state.profiles.get_mut(self.state.selected_profile_index)
    }

    fn current_settings(&self) -> DspSettings {
        self.current_profile()
            .map(|profile| profile.settings)
            .unwrap_or_default()
    }

    fn selected_track_path(&self) -> Option<PathBuf> {
        let playlist = self.current_playlist()?;
        let index = self.selected_track_index?;
        playlist.tracks.get(index).cloned()
    }

    fn add_audio_files(&mut self) {
        let picked_files = FileDialog::new()
            .add_filter("Audio files", &["mp3", "wav", "flac", "ogg"])
            .add_filter("All files", &["*"])
            .pick_files();

        let Some(files) = picked_files else {
            return;
        };

        let added_count = files.len();
        let Some((start_index, playlist_name)) = self.current_playlist_mut().map(|playlist| {
            let start_index = playlist.tracks.len();
            playlist.tracks.extend(files);
            (start_index, playlist.name.clone())
        }) else {
            return;
        };

        self.selected_track_index = Some(start_index);
        self.status_message = format!("Added {added_count} track(s) to {playlist_name}.");
        self.error_message = None;
        self.save_state_silently();
    }

    fn remove_selected_track(&mut self) {
        let Some(track_index) = self.selected_track_index else {
            return;
        };

        let Some(remaining_len) = self.current_playlist_mut().and_then(|playlist| {
            if track_index < playlist.tracks.len() {
                playlist.tracks.remove(track_index);
                Some(playlist.tracks.len())
            } else {
                None
            }
        }) else {
            return;
        };

        self.selected_track_index = next_valid_track_index(track_index, remaining_len);
        self.status_message = "Removed selected track from playlist.".to_owned();
        self.save_state_silently();
    }

    fn add_playlist(&mut self) {
        let number = self.state.playlists.len() + 1;
        self.state.playlists.push(Playlist::new(format!("Playlist {number}")));
        self.state.selected_playlist_index = self.state.playlists.len() - 1;
        self.selected_track_index = None;
        self.status_message = "Created a new playlist.".to_owned();
        self.save_state_silently();
    }

    fn remove_current_playlist(&mut self) {
        if self.state.playlists.len() <= 1 {
            self.error_message = Some("At least one playlist is required.".to_owned());
            return;
        }

        self.state.playlists.remove(self.state.selected_playlist_index);
        self.state.selected_playlist_index = self.state.selected_playlist_index.saturating_sub(1);
        self.selected_track_index = None;
        self.status_message = "Removed playlist.".to_owned();
        self.save_state_silently();
    }

    fn add_profile(&mut self) {
        let settings = self.current_settings();
        let number = self.state.profiles.len() + 1;
        self.state
            .profiles
            .push(DspProfile::new(format!("Profile {number}"), settings));
        self.state.selected_profile_index = self.state.profiles.len() - 1;
        self.status_message = "Created a new sound profile from the current settings.".to_owned();
        self.save_state_silently();
    }

    fn remove_current_profile(&mut self) {
        if self.state.profiles.len() <= 1 {
            self.error_message = Some("At least one sound profile is required.".to_owned());
            return;
        }

        self.state.profiles.remove(self.state.selected_profile_index);
        self.state.selected_profile_index = self.state.selected_profile_index.saturating_sub(1);
        self.status_message = "Removed sound profile.".to_owned();
        self.save_state_silently();
    }

    fn play_selected_track(&mut self) {
        let Some(path) = self.selected_track_path() else {
            self.error_message = Some("Select a track first.".to_owned());
            return;
        };

        self.play_path(path);
    }

    fn play_path(&mut self, path: PathBuf) {
        let settings = self.current_settings();
        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        };

        self.status_message = "Decoding and processing audio...".to_owned();
        self.error_message = None;
        self.settings_changed_during_playback = false;

        match player.play_file_with_orbit(&path, settings) {
            Ok(info) => {
                let mode_label = settings.mode.label();
                self.status_message = format!(
                    "Playing {} through {}.",
                    display_file_name(&info.path),
                    mode_label
                );
                self.active_settings = Some(settings);
                self.active_track_index = self.selected_track_index;
                self.last_playback = Some(info);
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = "Playback failed.".to_owned();
            }
        }
    }

    fn play_next_track(&mut self) {
        let Some((next_index, path)) = self.current_playlist().and_then(|playlist| {
            if playlist.tracks.is_empty() {
                return None;
            }

            let next_index = match self.active_track_index.or(self.selected_track_index) {
                Some(index) => (index + 1) % playlist.tracks.len(),
                None => 0,
            };

            playlist.tracks.get(next_index).cloned().map(|path| (next_index, path))
        }) else {
            return;
        };

        self.selected_track_index = Some(next_index);
        self.play_path(path);
    }

    fn play_previous_track(&mut self) {
        let Some((previous_index, path)) = self.current_playlist().and_then(|playlist| {
            if playlist.tracks.is_empty() {
                return None;
            }

            let previous_index = match self.active_track_index.or(self.selected_track_index) {
                Some(0) | None => playlist.tracks.len() - 1,
                Some(index) => index.saturating_sub(1),
            };

            playlist
                .tracks
                .get(previous_index)
                .cloned()
                .map(|path| (previous_index, path))
        }) else {
            return;
        };

        self.selected_track_index = Some(previous_index);
        self.play_path(path);
    }

    fn stop(&mut self) {
        if let Some(player) = &mut self.player {
            player.stop();
        }

        self.active_track_index = None;
        self.active_settings = None;
        self.settings_changed_during_playback = false;
        self.status_message = "Playback stopped.".to_owned();
    }

    fn pause_or_resume(&mut self) {
        if let Some(player) = &mut self.player {
            player.pause_or_resume();

            if player.is_paused() {
                self.status_message = "Playback paused.".to_owned();
            } else if player.is_playing() {
                self.status_message = "Playback resumed.".to_owned();
            }
        }
    }

    fn refresh_output_device(&mut self) {
        if let Some(player) = &mut self.player {
            player.stop();
        }

        match AudioPlayer::new() {
            Ok(player) => {
                let output_name = player.output_device_name().to_owned();
                self.player = Some(player);
                self.active_track_index = None;
                self.active_settings = None;
                self.settings_changed_during_playback = false;
                self.status_message = format!("Output refreshed: {output_name}. Start playback again.");
                self.error_message = None;
            }
            Err(error) => {
                self.player = None;
                self.error_message = Some(error.to_string());
                self.status_message = "Could not refresh output device.".to_owned();
            }
        }
    }

    fn save_state_silently(&mut self) {
        if let Err(error) = save_state(&self.state) {
            self.error_message = Some(error.to_string());
        }
    }

    fn update_playback_status(&mut self) {
        let is_active = self
            .player
            .as_ref()
            .map(|player| player.is_playing() || player.is_paused())
            .unwrap_or(false);

        if is_active {
            if let Some(active_settings) = self.active_settings {
                self.settings_changed_during_playback = active_settings != self.current_settings();
            }
        }

        let finished = self
            .player
            .as_ref()
            .map(AudioPlayer::has_finished)
            .unwrap_or(false);

        if finished && self.active_track_index.is_some() {
            if self.auto_advance {
                self.play_next_track();
            } else {
                self.active_track_index = None;
                self.active_settings = None;
                self.settings_changed_during_playback = false;
            }
        }
    }
}

impl Drop for AudioOrbitApp {
    fn drop(&mut self) {
        if let Some(player) = &mut self.player {
            player.stop();
        }
        let _ = save_state(&self.state);
    }
}

impl eframe::App for AudioOrbitApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_playback_status();

        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("Audio Orbit");
            ui.label("A local DSP audio player for smooth stereo orbit and experimental virtual 8-direction headphone cues.");
            ui.label("It processes files it plays itself. System-wide Spotify/YouTube/game audio needs a WASAPI loopback + virtual device/APO architecture.");
            ui.separator();

            self.render_playlist_panel(ui);
            ui.separator();
            self.render_profile_panel(ui);
            ui.separator();
            self.render_transport_panel(ui);
            ui.separator();
            self.render_status_panel(ui);
        });
    }
}

impl AudioOrbitApp {
    fn render_playlist_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Playlists");

            let playlist_names: Vec<String> = self
                .state
                .playlists
                .iter()
                .map(|playlist| playlist.name.clone())
                .collect();
            let selected_playlist_name = playlist_names
                .get(self.state.selected_playlist_index)
                .map(String::as_str)
                .unwrap_or("No playlist");

            egui::ComboBox::from_id_salt("playlist_selector")
                .selected_text(selected_playlist_name)
                .show_ui(ui, |ui| {
                    for (index, playlist_name) in playlist_names.iter().enumerate() {
                        if ui
                            .selectable_value(
                                &mut self.state.selected_playlist_index,
                                index,
                                playlist_name.as_str(),
                            )
                            .clicked()
                        {
                            self.selected_track_index = None;
                        }
                    }
                });

            if ui.button("New playlist").clicked() {
                self.add_playlist();
            }

            if ui.button("Remove playlist").clicked() {
                self.remove_current_playlist();
            }
        });

        let mut playlist_name_changed = false;
        if let Some(playlist) = self.current_playlist_mut() {
            ui.horizontal(|ui| {
                ui.label("Name:");
                playlist_name_changed |= ui.text_edit_singleline(&mut playlist.name).changed();
            });
        }
        if playlist_name_changed {
            self.save_state_silently();
        }

        ui.horizontal(|ui| {
            if ui.button("Add audio files...").clicked() {
                self.add_audio_files();
            }

            if ui
                .add_enabled(self.selected_track_index.is_some(), egui::Button::new("Remove selected track"))
                .clicked()
            {
                self.remove_selected_track();
            }

            ui.checkbox(&mut self.auto_advance, "Auto-play next track");
        });

        let tracks: Vec<PathBuf> = self
            .current_playlist()
            .map(|playlist| playlist.tracks.clone())
            .unwrap_or_default();

        egui::ScrollArea::vertical()
            .max_height(150.0)
            .show(ui, |ui| {
                if tracks.is_empty() {
                    ui.small("No tracks yet. Add multiple files to build a playlist.");
                }

                for (index, track) in tracks.iter().enumerate() {
                    let label = format!("{}  {}", index + 1, display_file_name(track));
                    let selected = self.selected_track_index == Some(index);
                    if ui.selectable_label(selected, label).clicked() {
                        self.selected_track_index = Some(index);
                    }
                }
            });
    }

    fn render_profile_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.strong("Sound profiles");

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

            egui::ComboBox::from_id_salt("profile_selector")
                .selected_text(selected_profile_name)
                .show_ui(ui, |ui| {
                    for (index, profile_name) in profile_names.iter().enumerate() {
                        ui.selectable_value(
                            &mut self.state.selected_profile_index,
                            index,
                            profile_name.as_str(),
                        );
                    }
                });

            if ui.button("New profile").clicked() {
                self.add_profile();
            }

            if ui.button("Remove profile").clicked() {
                self.remove_current_profile();
            }
        });

        let mut profile_changed = false;
        if let Some(profile) = self.current_profile_mut() {
            ui.horizontal(|ui| {
                ui.label("Name:");
                profile_changed |= ui.text_edit_singleline(&mut profile.name).changed();
            });

            ui.horizontal(|ui| {
                ui.label("Mode:");
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
            });

            ui.small(profile.settings.mode.description());

            profile_changed |= ui
                .add(
                    egui::Slider::new(&mut profile.settings.output_level_percent, 1u8..=100u8)
                        .text("Output Level (%)"),
                )
                .changed();
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
                        .text("Transition Smoothness (%)"),
                )
                .changed();
            profile_changed |= ui
                .add(
                    egui::Slider::new(&mut profile.settings.depth_cue_percent, 0u8..=100u8)
                        .text("Front/Back Cue Strength (%)"),
                )
                .changed();
        }

        if profile_changed {
            self.save_state_silently();
            if self.active_settings.is_some() && self.active_settings != Some(self.current_settings()) {
                self.settings_changed_during_playback = true;
            }
        }
    }

    fn render_transport_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui
                .add_enabled(
                    self.player.is_some() && self.selected_track_path().is_some(),
                    egui::Button::new("Play selected"),
                )
                .clicked()
            {
                self.play_selected_track();
            }

            if ui
                .add_enabled(self.player.is_some(), egui::Button::new("Previous"))
                .clicked()
            {
                self.play_previous_track();
            }

            if ui
                .add_enabled(self.player.is_some(), egui::Button::new("Next"))
                .clicked()
            {
                self.play_next_track();
            }

            if ui
                .add_enabled(self.player.is_some(), egui::Button::new("Pause / Resume"))
                .clicked()
            {
                self.pause_or_resume();
            }

            if ui
                .add_enabled(self.player.is_some(), egui::Button::new("Stop"))
                .clicked()
            {
                self.stop();
            }

            if ui.button("Refresh output device").clicked() {
                self.refresh_output_device();
            }
        });

        let output_name = self
            .player
            .as_ref()
            .map(|player| player.output_device_name().to_owned())
            .unwrap_or_else(|| "No output device".to_owned());
        ui.small(format!("Output: {output_name}"));
    }

    fn render_status_panel(&mut self, ui: &mut egui::Ui) {
        ui.strong("Status");
        ui.label(&self.status_message);

        if self.settings_changed_during_playback {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Settings changed. Restart playback to hear the updated DSP render.",
            );
        }

        if let Some(info) = &self.last_playback {
            ui.small(format!(
                "Source: {} channel(s), {} Hz, {:.1}s. Rendered stereo samples: {}.",
                info.input_channels,
                info.sample_rate,
                info.duration_seconds,
                info.output_samples
            ));
        }

        if let Some(error_message) = &self.error_message {
            ui.colored_label(egui::Color32::RED, error_message);
        }

        ui.add_space(8.0);
        ui.small("Virtual 8-direction mode is a stereo headphone illusion, not real Dolby Atmos/HRTF. For true system-wide audio processing, the next architecture step is WASAPI loopback capture plus a virtual output device or APO.");
    }
}

fn ensure_state_is_valid(state: &mut SavedState) {
    if state.playlists.is_empty() {
        state.playlists.push(Playlist::new("Main playlist"));
    }

    if state.profiles.is_empty() {
        state
            .profiles
            .push(DspProfile::new("Smooth orbit", DspSettings::default()));
    }

    if state.selected_playlist_index >= state.playlists.len() {
        state.selected_playlist_index = 0;
    }

    if state.selected_profile_index >= state.profiles.len() {
        state.selected_profile_index = 0;
    }
}

fn next_valid_track_index(previous_index: usize, remaining_len: usize) -> Option<usize> {
    if remaining_len == 0 {
        None
    } else {
        Some(previous_index.min(remaining_len - 1))
    }
}

fn display_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}
