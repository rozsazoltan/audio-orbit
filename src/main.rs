#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_player;
mod config;
mod dsp;
mod icon;

use crate::{
    audio_player::{current_default_output_device_name, AudioPlayer, PlaybackInfo},
    config::{
        collect_audio_files_from_folder, export_state_to, import_state_from, load_state, save_state,
        DspProfile, Playlist, SavedState,
    },
    dsp::{DspSettings, OrbitMode},
};
use eframe::egui;
use rfd::FileDialog;
use std::{
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1120.0, 720.0])
        .with_min_inner_size([920.0, 620.0])
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
    show_folder_import_modal: bool,
    pending_folder_path: Option<PathBuf>,
    pending_playlist_name: String,
    pending_folder_depth: usize,
    last_known_output_name: String,
    detected_output_change: Option<String>,
    last_output_check: Instant,
}

impl AudioOrbitApp {
    fn new() -> Self {
        let mut state = load_state();
        ensure_state_is_valid(&mut state);

        let pending_playlist_name = "Local music".to_owned();
        let current_output_name = current_default_output_device_name();

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
                    show_folder_import_modal: false,
                    pending_folder_path: None,
                    pending_playlist_name,
                    pending_folder_depth: 2,
                    last_known_output_name: output_name,
                    detected_output_change: None,
                    last_output_check: Instant::now(),
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
                show_folder_import_modal: false,
                pending_folder_path: None,
                pending_playlist_name,
                pending_folder_depth: 2,
                last_known_output_name: current_output_name,
                detected_output_change: None,
                last_output_check: Instant::now(),
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

    fn eligible_track_indexes(&self) -> Vec<usize> {
        self.current_playlist()
            .map(Playlist::filtered_track_indexes)
            .unwrap_or_default()
    }

    fn selected_track_path(&self) -> Option<PathBuf> {
        let playlist = self.current_playlist()?;
        let index = self.selected_track_index?;
        playlist.tracks.get(index).map(|track| track.path.clone())
    }

    fn active_track_title(&self) -> String {
        self.active_track_index
            .and_then(|index| self.current_playlist()?.tracks.get(index))
            .map(|track| track.title.clone())
            .or_else(|| {
                self.last_playback
                    .as_ref()
                    .map(|playback| display_file_name(&playback.path))
            })
            .unwrap_or_else(|| "No track playing".to_owned())
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
            playlist.add_files(files);
            (start_index, playlist.name.clone())
        }) else {
            return;
        };

        self.selected_track_index = Some(start_index);
        self.ensure_selected_track_visible();
        self.status_message = format!("Added {added_count} track(s) to {playlist_name}.");
        self.error_message = None;
        self.save_state_silently();
    }

    fn open_folder_import_modal(&mut self) {
        self.show_folder_import_modal = true;
        self.error_message = None;
    }

    fn pick_music_folder(&mut self) {
        let Some(folder) = FileDialog::new().pick_folder() else {
            return;
        };

        if self.pending_playlist_name.trim().is_empty() || self.pending_playlist_name == "Local music" {
            self.pending_playlist_name = folder
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Local music".to_owned());
        }

        self.pending_folder_path = Some(folder);
    }

    fn import_folder_playlist(&mut self) -> bool {
        let Some(folder) = self.pending_folder_path.clone() else {
            self.error_message = Some("Choose a music folder first.".to_owned());
            return false;
        };

        let name = if self.pending_playlist_name.trim().is_empty() {
            folder
                .file_name()
                .and_then(|name| name.to_str())
                .filter(|name| !name.trim().is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| "Local music".to_owned())
        } else {
            self.pending_playlist_name.trim().to_owned()
        };

        match collect_audio_files_from_folder(&folder) {
            Ok(files) if files.is_empty() => {
                self.error_message = Some(format!(
                    "No supported audio files were found under {}.",
                    folder.display()
                ));
                false
            }
            Ok(files) => {
                let track_count = files.len();
                let playlist = Playlist::from_folder(name.clone(), folder.clone(), self.pending_folder_depth, files);
                self.state.playlists.push(playlist);
                self.state.selected_playlist_index = self.state.playlists.len() - 1;
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.status_message = format!(
                    "Imported {track_count} track(s) from {} as {name}.",
                    folder.display()
                );
                self.error_message = None;
                self.save_state_silently();
                true
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                false
            }
        }
    }

    fn rescan_current_folder(&mut self) {
        let Some((folder, depth, name)) = self.current_playlist().and_then(|playlist| {
            playlist
                .source_folder
                .clone()
                .map(|folder| (folder, playlist.folder_depth, playlist.name.clone()))
        }) else {
            self.error_message = Some("This playlist was not created from a folder.".to_owned());
            return;
        };

        match collect_audio_files_from_folder(&folder) {
            Ok(files) => {
                let track_count = files.len();
                if let Some(playlist) = self.current_playlist_mut() {
                    playlist.folder_depth = depth;
                    playlist.replace_tracks_from_files(files);
                }
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.status_message = format!("Rescanned {name}: {track_count} track(s) found.");
                self.error_message = None;
                self.save_state_silently();
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }

    fn export_library_backup(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter("Audio Orbit library backup", &["json"])
            .set_file_name("audio-orbit-library.json")
            .save_file()
        else {
            return;
        };

        match export_state_to(&self.state, &path) {
            Ok(()) => {
                self.status_message = format!("Exported library backup to {}.", path.display());
                self.error_message = None;
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }

    fn import_library_backup(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter("Audio Orbit library backup", &["json"])
            .pick_file()
        else {
            return;
        };

        match import_state_from(&path) {
            Ok(mut state) => {
                ensure_state_is_valid(&mut state);
                self.stop();
                self.state = state;
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.status_message = format!("Imported library backup from {}.", path.display());
                self.error_message = None;
                self.save_state_silently();
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }

    fn remove_selected_track(&mut self) {
        let Some(track_index) = self.selected_track_index else {
            return;
        };

        if self.active_track_index == Some(track_index) {
            self.stop();
        }

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

        if let Some(active_index) = self.active_track_index {
            if active_index > track_index {
                self.active_track_index = Some(active_index - 1);
            }
        }

        self.selected_track_index = next_valid_track_index(track_index, remaining_len);
        self.ensure_selected_track_visible();
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

        self.stop();
        self.state.playlists.remove(self.state.selected_playlist_index);
        self.state.selected_playlist_index = self.state.selected_playlist_index.saturating_sub(1);
        self.selected_track_index = self.eligible_track_indexes().first().copied();
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

    fn play_selected_or_first_track(&mut self) {
        if !self.selected_track_is_visible() {
            self.selected_track_index = self.eligible_track_indexes().first().copied();
        }

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
        let indexes = self.eligible_track_indexes();
        if indexes.is_empty() {
            return;
        }

        let current_index = self.active_track_index.or(self.selected_track_index);
        let current_position = current_index.and_then(|index| indexes.iter().position(|candidate| *candidate == index));
        let next_position = current_position.map(|position| (position + 1) % indexes.len()).unwrap_or(0);
        let next_index = indexes[next_position];

        let Some(path) = self
            .current_playlist()
            .and_then(|playlist| playlist.tracks.get(next_index))
            .map(|track| track.path.clone())
        else {
            return;
        };

        self.selected_track_index = Some(next_index);
        self.play_path(path);
    }

    fn play_previous_track(&mut self) {
        let indexes = self.eligible_track_indexes();
        if indexes.is_empty() {
            return;
        }

        let current_index = self.active_track_index.or(self.selected_track_index);
        let current_position = current_index.and_then(|index| indexes.iter().position(|candidate| *candidate == index));
        let previous_position = current_position
            .map(|position| if position == 0 { indexes.len() - 1 } else { position - 1 })
            .unwrap_or(0);
        let previous_index = indexes[previous_position];

        let Some(path) = self
            .current_playlist()
            .and_then(|playlist| playlist.tracks.get(previous_index))
            .map(|track| track.path.clone())
        else {
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
                self.detected_output_change = None;
                self.last_known_output_name = output_name.clone();
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

    fn selected_track_is_visible(&self) -> bool {
        let Some(index) = self.selected_track_index else {
            return false;
        };

        self.current_playlist()
            .and_then(|playlist| playlist.tracks.get(index).map(|track| playlist.track_matches_selected_group(track)))
            .unwrap_or(false)
    }

    fn ensure_selected_track_visible(&mut self) {
        if !self.selected_track_is_visible() {
            self.selected_track_index = self.eligible_track_indexes().first().copied();
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

    fn poll_output_device_change(&mut self) {
        if self.last_output_check.elapsed() < Duration::from_secs(2) {
            return;
        }

        self.last_output_check = Instant::now();
        let current_output = current_default_output_device_name();

        if current_output != self.last_known_output_name {
            self.detected_output_change = Some(current_output.clone());
            self.status_message = format!(
                "Output device changed to {current_output}. Refresh output device before starting the next track."
            );
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
        context.set_visuals(egui::Visuals::dark());
        context.request_repaint_after(Duration::from_millis(250));

        self.update_playback_status();
        self.poll_output_device_change();

        egui::TopBottomPanel::top("now_playing_panel").show(context, |ui| {
            self.render_now_playing_panel(ui);
        });

        egui::SidePanel::left("library_panel")
            .resizable(true)
            .default_width(260.0)
            .show(context, |ui| {
                self.render_library_panel(ui);
            });

        egui::SidePanel::right("profile_panel")
            .resizable(true)
            .default_width(310.0)
            .show(context, |ui| {
                self.render_profile_panel(ui);
            });

        egui::CentralPanel::default().show(context, |ui| {
            self.render_track_panel(ui);
        });

        egui::TopBottomPanel::bottom("status_panel").show(context, |ui| {
            self.render_status_panel(ui);
        });

        if self.show_folder_import_modal {
            self.render_folder_import_window(context);
        }
    }
}

impl AudioOrbitApp {
    fn render_now_playing_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        ui.horizontal(|ui| {
            ui.vertical(|ui| {
                ui.heading(self.active_track_title());

                if let Some(playback) = &self.last_playback {
                    ui.small(format!(
                        "{} · {} Hz · {} channel(s)",
                        display_parent(&playback.path),
                        playback.sample_rate,
                        playback.input_channels
                    ));
                } else {
                    ui.small("Choose a playlist, folder, or track to start playback.");
                }
            });

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if ui.button("Refresh output").clicked() {
                    self.refresh_output_device();
                }
                ui.small(self.last_known_output_name.as_str());
            });
        });

        let position = self
            .player
            .as_ref()
            .map(AudioPlayer::playback_position_seconds)
            .unwrap_or(0.0);
        let duration = self
            .player
            .as_ref()
            .and_then(AudioPlayer::playback_duration_seconds)
            .or_else(|| self.last_playback.as_ref().map(|playback| playback.duration_seconds))
            .unwrap_or(0.0);
        let progress = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };

        ui.add_sized(
            [ui.available_width(), 18.0],
            egui::ProgressBar::new(progress).text(format!(
                "{} / {}",
                format_duration(position),
                format_duration(duration)
            )),
        );

        ui.horizontal(|ui| {
            if ui
                .add_enabled(self.player.is_some(), egui::Button::new("⏮ Previous"))
                .clicked()
            {
                self.play_previous_track();
            }

            let play_label = match self.player.as_ref() {
                Some(player) if player.is_playing() => "⏸ Pause",
                Some(player) if player.is_paused() => "▶ Resume",
                _ => "▶ Play",
            };

            if ui
                .add_enabled(self.player.is_some(), egui::Button::new(play_label))
                .clicked()
            {
                let is_active = self
                    .player
                    .as_ref()
                    .map(|player| player.is_playing() || player.is_paused())
                    .unwrap_or(false);

                if is_active {
                    self.pause_or_resume();
                } else {
                    self.play_selected_or_first_track();
                }
            }

            if ui
                .add_enabled(self.player.is_some(), egui::Button::new("⏹ Stop"))
                .clicked()
            {
                self.stop();
            }

            if ui
                .add_enabled(self.player.is_some(), egui::Button::new("Next ⏭"))
                .clicked()
            {
                self.play_next_track();
            }

            ui.separator();
            ui.checkbox(&mut self.auto_advance, "Auto-play next");
        });

        if self.settings_changed_during_playback {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Sound profile changed. Restart the track to hear the new DSP render.",
            );
        }

        if let Some(output_name) = self.detected_output_change.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("Output changed to {output_name}."),
                );
                if ui.button("Refresh now").clicked() {
                    self.refresh_output_device();
                }
            });
        }

        ui.add_space(6.0);
    }

    fn render_library_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Library");
        ui.small("Playlists, folder imports, and backups.");
        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(220.0)
            .show(ui, |ui| {
                for index in 0..self.state.playlists.len() {
                    let playlist = &self.state.playlists[index];
                    let selected = self.state.selected_playlist_index == index;
                    let label = format!("{}  ·  {} tracks", playlist.name, playlist.tracks.len());

                    if ui.selectable_label(selected, label).clicked() {
                        self.state.selected_playlist_index = index;
                        self.selected_track_index = self.eligible_track_indexes().first().copied();
                        self.save_state_silently();
                    }
                }
            });

        ui.horizontal(|ui| {
            if ui.button("New").clicked() {
                self.add_playlist();
            }
            if ui.button("Remove").clicked() {
                self.remove_current_playlist();
            }
        });

        ui.separator();
        self.render_current_playlist_controls(ui);

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Add files...").clicked() {
                self.add_audio_files();
            }
            if ui.button("Add folder...").clicked() {
                self.open_folder_import_modal();
            }
        });

        ui.horizontal(|ui| {
            let can_rescan = self
                .current_playlist()
                .and_then(|playlist| playlist.source_folder.as_ref())
                .is_some();

            if ui
                .add_enabled(can_rescan, egui::Button::new("Rescan folder"))
                .clicked()
            {
                self.rescan_current_folder();
            }

            if ui
                .add_enabled(self.selected_track_index.is_some(), egui::Button::new("Remove track"))
                .clicked()
            {
                self.remove_selected_track();
            }
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Export backup").clicked() {
                self.export_library_backup();
            }
            if ui.button("Import backup").clicked() {
                self.import_library_backup();
            }
        });
    }

    fn render_current_playlist_controls(&mut self, ui: &mut egui::Ui) {
        let mut playlist_changed = false;
        if let Some(playlist) = self.current_playlist_mut() {
            ui.label("Playlist name");
            playlist_changed |= ui.text_edit_singleline(&mut playlist.name).changed();
        }

        if playlist_changed {
            self.save_state_silently();
        }

        let Some(playlist) = self.current_playlist() else {
            return;
        };

        let groups = playlist.folder_groups();
        let selected_group = playlist.selected_group.clone();
        let selected_label = playlist.selected_group_label();
        let source_folder = playlist.source_folder.clone();
        let folder_depth = playlist.folder_depth;

        if let Some(folder) = source_folder {
            ui.small(format!("Folder: {}", folder.display()));
            ui.small(format!("Grouping depth: {folder_depth} folder level(s)"));
        }

        let mut next_group = selected_group.clone();
        egui::ComboBox::from_id_salt("folder_group_selector")
            .selected_text(selected_label)
            .show_ui(ui, |ui| {
                if ui
                    .selectable_label(next_group.is_none(), "All folders")
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
            if let Some(playlist) = self.current_playlist_mut() {
                playlist.set_selected_group(next_group);
            }
            self.ensure_selected_track_visible();
            self.save_state_silently();
        }
    }

    fn render_track_panel(&mut self, ui: &mut egui::Ui) {
        let Some(playlist) = self.current_playlist() else {
            ui.heading("No playlist");
            return;
        };

        let playlist_name = playlist.name.clone();
        let selected_group_label = playlist.selected_group_label();
        let filtered_indexes = playlist.filtered_track_indexes();
        let visible_count = filtered_indexes.len();
        let total_count = playlist.tracks.len();

        ui.heading(playlist_name);
        ui.label(format!(
            "Showing {visible_count} of {total_count} track(s) · {selected_group_label}"
        ));
        ui.separator();

        if visible_count == 0 {
            ui.centered_and_justified(|ui| {
                ui.label("No tracks in this view. Add files or import a music folder.");
            });
            return;
        }

        let visible_tracks: Vec<(usize, String, String, PathBuf)> = self
            .current_playlist()
            .map(|playlist| {
                filtered_indexes
                    .into_iter()
                    .filter_map(|index| {
                        playlist.tracks.get(index).map(|track| {
                            (index, track.title.clone(), track.group.clone(), track.path.clone())
                        })
                    })
                    .collect()
            })
            .unwrap_or_default();

        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, title, group, path) in visible_tracks {
                let is_selected = self.selected_track_index == Some(index);
                let is_active = self.active_track_index == Some(index);
                let marker = if is_active { "▶" } else { " " };
                let label = format!("{marker} {:03}. {}", index + 1, title);

                ui.group(|ui| {
                    ui.horizontal(|ui| {
                        if ui.selectable_label(is_selected, label).clicked() {
                            self.selected_track_index = Some(index);
                        }

                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            ui.small(group);
                        });
                    });
                    ui.small(path.display().to_string());
                });
            }
        });
    }

    fn render_profile_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Sound profiles");
        ui.small("Changes are saved immediately. Restart the current track to apply them.");
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

        ui.horizontal(|ui| {
            if ui.button("New profile").clicked() {
                self.add_profile();
            }

            if ui.button("Remove").clicked() {
                self.remove_current_profile();
            }
        });

        ui.separator();

        let mut profile_changed = false;
        if let Some(profile) = self.current_profile_mut() {
            ui.label("Profile name");
            profile_changed |= ui.text_edit_singleline(&mut profile.name).changed();

            ui.separator();
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

            ui.separator();
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
                        .text("Motion Smoothness (%)"),
                )
                .changed();
            profile_changed |= ui
                .add(
                    egui::Slider::new(&mut profile.settings.depth_cue_percent, 0u8..=100u8)
                        .text("Surround Cue Strength (%)"),
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

    fn render_status_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(self.status_message.as_str());

            if let Some(error_message) = &self.error_message {
                ui.separator();
                ui.colored_label(egui::Color32::RED, error_message);
            }
        });
    }

    fn render_folder_import_window(&mut self, context: &egui::Context) {
        let mut is_open = self.show_folder_import_modal;
        let mut close_after_import = false;

        egui::Window::new("Add music folder")
            .open(&mut is_open)
            .collapsible(false)
            .resizable(false)
            .default_width(520.0)
            .show(context, |ui| {
                ui.label("Create a playlist from a folder and group tracks by the first N subfolder levels.");
                ui.add_space(8.0);

                ui.horizontal(|ui| {
                    ui.label("Folder:");
                    let folder_label = self
                        .pending_folder_path
                        .as_ref()
                        .map(|path| path.display().to_string())
                        .unwrap_or_else(|| "No folder selected".to_owned());
                    ui.monospace(folder_label);
                });

                if ui.button("Choose folder...").clicked() {
                    self.pick_music_folder();
                }

                ui.separator();
                ui.label("Playlist name");
                ui.text_edit_singleline(&mut self.pending_playlist_name);

                ui.add(
                    egui::Slider::new(&mut self.pending_folder_depth, 0usize..=5usize)
                        .text("Group by folder levels"),
                );
                ui.small("Example: depth 2 turns D:\\mp3\\Artist\\Album\\song.mp3 into Artist / Album.");

                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Import folder as playlist").clicked() {
                        close_after_import = self.import_folder_playlist();
                    }
                    if ui.button("Cancel").clicked() {
                        close_after_import = true;
                    }
                });
            });

        self.show_folder_import_modal = is_open && !close_after_import;
    }
}

fn ensure_state_is_valid(state: &mut SavedState) {
    if state.playlists.is_empty() {
        state.playlists.push(Playlist::new("Local music"));
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

    for playlist in &mut state.playlists {
        playlist.set_selected_group(playlist.selected_group.clone());
    }
}

fn next_valid_track_index(previous_index: usize, remaining_len: usize) -> Option<usize> {
    if remaining_len == 0 {
        None
    } else if previous_index >= remaining_len {
        Some(remaining_len - 1)
    } else {
        Some(previous_index)
    }
}

fn display_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn display_parent(path: &Path) -> String {
    path.parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_else(|| "Unknown folder".to_owned())
}

fn format_duration(seconds: f32) -> String {
    let total_seconds = seconds.max(0.0).round() as u64;
    let minutes = total_seconds / 60;
    let seconds = total_seconds % 60;
    format!("{minutes}:{seconds:02}")
}
