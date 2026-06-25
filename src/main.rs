#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_player;
mod config;
mod dsp;
mod icon;
mod updater;

use crate::{
    audio_player::{current_default_output_device_name, AudioPlayer, PlaybackInfo},
    config::{
        app_data_dir, collect_audio_files_from_folder, display_file_name, export_state_zip,
        import_state_zip, load_state, same_path, save_state, Playlist, PlaylistKind, SavedState,
        Track, FAVORITES_PLAYLIST_NAME,
    },
    dsp::{DspSettings, OrbitMode},
};
use eframe::egui;
use rfd::FileDialog;
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

fn main() -> eframe::Result<()> {
    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([1240.0, 780.0])
        .with_min_inner_size([980.0, 640.0])
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
    active_track_path: Option<PathBuf>,
    last_playback: Option<PlaybackInfo>,
    status_message: String,
    error_message: Option<String>,
    auto_advance: bool,
    show_folder_import_modal: bool,
    pending_folder_path: Option<PathBuf>,
    pending_playlist_name: String,
    pending_folder_depth: usize,
    last_known_output_name: String,
    detected_output_change: Option<String>,
    last_output_check: Instant,
    editing_playlist_index: Option<usize>,
    editing_profile_index: Option<usize>,
    last_update_check: Option<updater::UpdateCheck>,
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
                    active_track_path: None,
                    last_playback: None,
                    status_message: format!("Ready. Output device: {output_name}"),
                    error_message: None,
                    auto_advance: true,
                    show_folder_import_modal: false,
                    pending_folder_path: None,
                    pending_playlist_name,
                    pending_folder_depth: 2,
                    last_known_output_name: output_name,
                    detected_output_change: None,
                    last_output_check: Instant::now(),
                    editing_playlist_index: None,
                    editing_profile_index: None,
                    last_update_check: None,
                }
            }
            Err(error) => Self {
                player: None,
                state,
                selected_track_index: None,
                active_track_index: None,
                active_track_path: None,
                last_playback: None,
                status_message: "No audio output device is available.".to_owned(),
                error_message: Some(error.to_string()),
                auto_advance: true,
                show_folder_import_modal: false,
                pending_folder_path: None,
                pending_playlist_name,
                pending_folder_depth: 2,
                last_known_output_name: current_output_name,
                detected_output_change: None,
                last_output_check: Instant::now(),
                editing_playlist_index: None,
                editing_profile_index: None,
                last_update_check: None,
            },
        }
    }

    fn current_playlist(&self) -> Option<&Playlist> {
        self.state.playlists.get(self.state.selected_playlist_index)
    }

    fn current_playlist_mut(&mut self) -> Option<&mut Playlist> {
        self.state.playlists.get_mut(self.state.selected_playlist_index)
    }

    fn current_settings(&self) -> DspSettings {
        self.state
            .profiles
            .get(self.state.selected_profile_index)
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
        self.active_track_path
            .as_ref()
            .map(|path| display_file_name(path))
            .or_else(|| {
                self.selected_track_path()
                    .as_ref()
                    .map(|path| display_file_name(path))
            })
            .unwrap_or_else(|| "No track playing".to_owned())
    }

    fn add_audio_files(&mut self) {
        let Some(files) = FileDialog::new()
            .add_filter("Audio files", &["mp3", "wav", "flac", "ogg", "opus", "m4a", "mp4", "aac", "aiff", "aif", "ape", "wv"])
            .add_filter("All files", &["*"])
            .pick_files()
        else {
            return;
        };

        let Some(playlist) = self.current_playlist() else {
            return;
        };
        if !playlist.accepts_manual_tracks() {
            self.error_message = Some("Folder playlists are scanner-owned. Add files to a manual playlist or Favorites instead.".to_owned());
            return;
        }

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

    fn export_app_backup(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter("Audio Orbit backup", &["zip"])
            .set_file_name("audio-orbit-backup.zip")
            .save_file()
        else {
            return;
        };

        match export_state_zip(&self.state, &path) {
            Ok(()) => {
                self.status_message = format!("Exported app backup to {}.", path.display());
                self.error_message = None;
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }

    fn import_app_backup(&mut self) {
        let Some(path) = FileDialog::new()
            .add_filter("Audio Orbit backup", &["zip"])
            .pick_file()
        else {
            return;
        };

        match import_state_zip(&path) {
            Ok(mut state) => {
                ensure_state_is_valid(&mut state);
                self.stop();
                self.state = state;
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.status_message = format!("Imported app backup from {}.", path.display());
                self.error_message = None;
                self.save_state_silently();
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
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
        let Some(playlist) = self.current_playlist() else {
            return;
        };

        if !playlist.kind.can_delete() {
            self.error_message = Some("Favorites is built-in and cannot be deleted.".to_owned());
            return;
        }

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

    fn move_playlist(&mut self, from: usize, delta: isize) {
        if from >= self.state.playlists.len() {
            return;
        }
        let to = if delta < 0 {
            from.saturating_sub(1)
        } else {
            (from + 1).min(self.state.playlists.len() - 1)
        };
        if from == to {
            return;
        }
        self.state.playlists.swap(from, to);
        if self.state.selected_playlist_index == from {
            self.state.selected_playlist_index = to;
        } else if self.state.selected_playlist_index == to {
            self.state.selected_playlist_index = from;
        }
        self.save_state_silently();
    }

    fn add_profile(&mut self) {
        let settings = self.current_settings();
        let number = self.state.profiles.len() + 1;
        self.state
            .profiles
            .push(config::DspProfile::new(format!("Profile {number}"), settings));
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
        self.apply_current_profile_live();
    }

    fn play_selected_or_first_track(&mut self) {
        if !self.selected_track_is_visible() {
            self.selected_track_index = self.eligible_track_indexes().first().copied();
        }

        let Some(path) = self.selected_track_path() else {
            self.error_message = Some("Select a track first.".to_owned());
            return;
        };

        self.play_path(path, self.selected_track_index, 0.0);
    }

    fn play_path(&mut self, path: PathBuf, index: Option<usize>, start_seconds: f32) {
        let settings = self.current_settings();
        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        };

        self.status_message = "Decoding and processing audio...".to_owned();
        self.error_message = None;

        match player.play_file_with_orbit_from(&path, settings, start_seconds) {
            Ok(info) => {
                let mode_label = settings.mode.label();
                self.status_message = format!(
                    "Playing {} through {}.",
                    display_file_name(&info.path),
                    mode_label
                );
                self.active_track_index = index;
                self.active_track_path = Some(info.path.clone());
                self.store_playback_metadata(&info);
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
        self.play_path(path, Some(next_index), 0.0);
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
        self.play_path(path, Some(previous_index), 0.0);
    }

    fn seek_current(&mut self, seconds: f32) {
        let Some(player) = &mut self.player else {
            return;
        };

        match player.seek_current(seconds) {
            Ok(Some(info)) => {
                self.status_message = format!("Seeked to {}.", format_duration(seconds));
                self.store_playback_metadata(&info);
                self.last_playback = Some(info);
                self.error_message = None;
            }
            Ok(None) => {}
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }

    fn apply_current_profile_live(&mut self) {
        let settings = self.current_settings();
        let Some(player) = &mut self.player else {
            return;
        };

        if !(player.is_playing() || player.is_paused()) {
            return;
        }

        match player.apply_settings_to_current(settings) {
            Ok(Some(info)) => {
                self.status_message = "Applied sound profile without restarting the track.".to_owned();
                self.store_playback_metadata(&info);
                self.last_playback = Some(info);
                self.error_message = None;
            }
            Ok(None) => {}
            Err(error) => {
                self.error_message = Some(error.to_string());
            }
        }
    }

    fn stop(&mut self) {
        if let Some(player) = &mut self.player {
            player.stop();
        }

        self.active_track_index = None;
        self.active_track_path = None;
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
        let resume_path = self.active_track_path.clone();
        let resume_position = self.player.as_ref().map(AudioPlayer::playback_position_seconds).unwrap_or(0.0);
        let resume_index = self.active_track_index;

        if let Some(player) = &mut self.player {
            player.stop();
        }

        match AudioPlayer::new() {
            Ok(player) => {
                let output_name = player.output_device_name().to_owned();
                self.player = Some(player);
                self.detected_output_change = None;
                self.last_known_output_name = output_name.clone();
                self.status_message = format!("Output refreshed: {output_name}.");
                self.error_message = None;

                if let Some(path) = resume_path {
                    self.play_path(path, resume_index, resume_position);
                }
            }
            Err(error) => {
                self.player = None;
                self.error_message = Some(error.to_string());
                self.status_message = "Could not refresh output device.".to_owned();
            }
        }
    }

    fn check_for_updates(&mut self) {
        match updater::check_for_update(self.state.update_settings.include_prereleases) {
            Ok(check) => {
                if check.is_update_available {
                    self.status_message = format!(
                        "Update available: v{}{}.",
                        check.latest_version,
                        if check.prerelease { " prerelease" } else { "" }
                    );
                } else {
                    self.status_message = format!("Audio Orbit is up to date: v{}.", check.current_version);
                }
                self.last_update_check = Some(check);
                self.error_message = None;
            }
            Err(error) => self.error_message = Some(error.to_string()),
        }
    }

    fn install_update(&mut self) {
        let Some(check) = self.last_update_check.clone() else {
            self.error_message = Some("Check for updates first.".to_owned());
            return;
        };

        if !check.is_update_available {
            self.status_message = "No newer update is available.".to_owned();
            return;
        }

        if let Err(error) = updater::install_update(&check) {
            self.error_message = Some(error.to_string());
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
                self.active_track_path = None;
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
                "Output device changed to {current_output}. Refresh output to continue on the new device."
            );
        }
    }

    fn favorites_index(&self) -> Option<usize> {
        self.state
            .playlists
            .iter()
            .position(|playlist| playlist.kind == PlaylistKind::Favorites)
    }

    fn is_favorite(&self, path: &Path) -> bool {
        self.favorites_index()
            .and_then(|index| self.state.playlists.get(index))
            .map(|playlist| playlist.tracks.iter().any(|track| same_path(&track.path, path)))
            .unwrap_or(false)
    }

    fn toggle_favorite(&mut self, path: PathBuf) {
        let Some(favorites_index) = self.favorites_index() else {
            return;
        };

        let is_favorite = self.is_favorite(&path);
        if let Some(favorites) = self.state.playlists.get_mut(favorites_index) {
            if is_favorite {
                favorites.tracks.retain(|track| !same_path(&track.path, &path));
                self.status_message = "Removed from Favorites.".to_owned();
            } else {
                favorites.add_track_path(path.clone(), None, 0);
                self.status_message = "Added to Favorites.".to_owned();
            }
        }
        self.save_state_silently();
    }

    fn add_track_to_playlist(&mut self, path: PathBuf, playlist_index: usize) {
        let Some(playlist) = self.state.playlists.get_mut(playlist_index) else {
            return;
        };

        if !playlist.accepts_manual_tracks() {
            self.error_message = Some("Folder playlists are scanner-owned and cannot receive manual tracks.".to_owned());
            return;
        }

        let playlist_name = playlist.name.clone();
        if playlist.add_track_path(path, None, 0) {
            self.status_message = format!("Added track to {playlist_name}.");
        } else {
            self.status_message = format!("Track is already in {playlist_name}.");
        }
        self.save_state_silently();
    }

    fn remove_track_from_current_playlist(&mut self, track_index: usize) {
        let Some(remaining_len) = self.current_playlist_mut().and_then(|playlist| {
            if playlist.kind == PlaylistKind::Folder {
                None
            } else if track_index < playlist.tracks.len() {
                playlist.tracks.remove(track_index);
                Some(playlist.tracks.len())
            } else {
                None
            }
        }) else {
            self.error_message = Some("Folder playlist entries are managed by folder rescan.".to_owned());
            return;
        };

        self.selected_track_index = next_valid_track_index(track_index, remaining_len);
        self.save_state_silently();
    }

    fn delete_track_from_disk(&mut self, path: PathBuf) {
        if self.active_track_path.as_ref().map(|active| same_path(active, &path)).unwrap_or(false) {
            self.stop();
        }

        match fs::remove_file(&path) {
            Ok(()) => {
                for playlist in &mut self.state.playlists {
                    playlist.tracks.retain(|track| !same_path(&track.path, &path));
                }
                self.selected_track_index = self.eligible_track_indexes().first().copied();
                self.status_message = format!("Deleted {} from disk.", display_file_name(&path));
                self.error_message = None;
                self.save_state_silently();
            }
            Err(error) => self.error_message = Some(format!("Failed to delete file: {error}")),
        }
    }

    fn reveal_track_in_file_manager(&mut self, path: PathBuf) {
        if let Err(error) = reveal_in_file_manager(&path) {
            self.error_message = Some(error.to_string());
        }
    }

    fn store_playback_metadata(&mut self, info: &PlaybackInfo) {
        for playlist in &mut self.state.playlists {
            for track in &mut playlist.tracks {
                if same_path(&track.path, &info.path) {
                    track.update_playback_metadata(
                        info.original_duration_seconds,
                        info.sample_rate,
                        info.input_channels,
                        info.waveform.clone(),
                    );
                    if track.metadata.size_bytes.is_none() {
                        track.metadata.size_bytes = info.size_bytes;
                    }
                }
            }
        }
        self.save_state_silently();
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
        context.request_repaint_after(Duration::from_millis(180));

        self.update_playback_status();
        self.poll_output_device_change();

        egui::TopBottomPanel::top("now_playing_panel").show(context, |ui| {
            self.render_now_playing_panel(ui);
        });

        egui::SidePanel::left("library_panel")
            .resizable(true)
            .default_width(300.0)
            .show(context, |ui| {
                self.render_library_panel(ui);
            });

        egui::SidePanel::right("profile_panel")
            .resizable(true)
            .default_width(320.0)
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
                        "{} · {} Hz · {} ch · {} · rendered {}",
                        display_parent(&playback.path),
                        playback.sample_rate,
                        playback.input_channels,
                        playback.size_bytes.map(format_file_size).unwrap_or_else(|| "unknown size".to_owned()),
                        format_duration(playback.rendered_duration_seconds)
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
            .or_else(|| self.last_playback.as_ref().map(|playback| playback.original_duration_seconds))
            .unwrap_or(0.0);
        let progress = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };

        let waveform = self
            .last_playback
            .as_ref()
            .map(|playback| playback.waveform.as_slice())
            .unwrap_or(&[]);
        let response = draw_waveform_seek(ui, waveform, progress, format!("{} / {}", format_duration(position), format_duration(duration)));
        if (response.clicked() || response.dragged()) && duration > 0.0 {
            if let Some(pointer) = response.interact_pointer_pos() {
                let next_position = ((pointer.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0) * duration;
                self.seek_current(next_position);
            }
        }

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

        if let Some(output_name) = self.detected_output_change.clone() {
            ui.horizontal(|ui| {
                ui.colored_label(
                    egui::Color32::YELLOW,
                    format!("Output changed to {output_name}."),
                );
                if ui.button("Refresh and continue").clicked() {
                    self.refresh_output_device();
                }
            });
        }

        ui.add_space(6.0);
    }

    fn render_library_panel(&mut self, ui: &mut egui::Ui) {
        ui.heading("Library");
        ui.small("Folder scanners, manual playlists, Favorites, backups, and updates.");
        ui.separator();

        egui::ScrollArea::vertical()
            .max_height(260.0)
            .show(ui, |ui| {
                for index in 0..self.state.playlists.len() {
                    let selected = self.state.selected_playlist_index == index;
                    let playlist = self.state.playlists[index].clone();
                    let row = ui.horizontal(|ui| {
                        if ui.small_button("↑").clicked() {
                            self.move_playlist(index, -1);
                        }
                        if ui.small_button("↓").clicked() {
                            self.move_playlist(index, 1);
                        }

                        let mut clicked_row = false;
                        if self.editing_playlist_index == Some(index) {
                            let mut next_name = playlist.name.clone();
                            let response = ui.text_edit_singleline(&mut next_name);
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
                            let label = format!("{} {}  ·  {}", playlist.kind.icon(), playlist.name, playlist.tracks.len());
                            if ui.selectable_label(selected, label).clicked() {
                                clicked_row = true;
                            }
                        }

                        if ui.small_button("✎").on_hover_text("Rename").clicked() && playlist.kind != PlaylistKind::Favorites {
                            self.editing_playlist_index = Some(index);
                        }

                        clicked_row
                    });

                    if row.inner {
                        self.state.selected_playlist_index = index;
                        self.selected_track_index = self.eligible_track_indexes().first().copied();
                        self.save_state_silently();
                    }
                }
            });

        ui.horizontal(|ui| {
            if ui.button("New playlist").clicked() {
                self.add_playlist();
            }
            let can_remove = self.current_playlist().map(|playlist| playlist.kind.can_delete()).unwrap_or(false);
            if ui.add_enabled(can_remove, egui::Button::new("Remove")).clicked() {
                self.remove_current_playlist();
            }
        });

        ui.separator();
        self.render_current_playlist_controls(ui);

        ui.separator();
        ui.horizontal(|ui| {
            let can_add_files = self.current_playlist().map(|playlist| playlist.accepts_manual_tracks()).unwrap_or(false);
            if ui.add_enabled(can_add_files, egui::Button::new("Add files...")).clicked() {
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
        });

        ui.separator();
        ui.horizontal(|ui| {
            if ui.button("Export backup ZIP").clicked() {
                self.export_app_backup();
            }
            if ui.button("Import backup ZIP").clicked() {
                self.import_app_backup();
            }
        });

        if let Some(path) = app_data_dir() {
            ui.small(format!("Data: {}", path.display()));
        }

        ui.separator();
        ui.heading("Updates");
        let prerelease_changed = ui
            .checkbox(&mut self.state.update_settings.include_prereleases, "Include prereleases")
            .changed();
        if prerelease_changed {
            self.save_state_silently();
        }
        ui.horizontal(|ui| {
            if ui.button("Check").clicked() {
                self.check_for_updates();
            }
            let can_install = self
                .last_update_check
                .as_ref()
                .map(|check| check.is_update_available && check.asset_download_url.is_some())
                .unwrap_or(false);
            if ui.add_enabled(can_install, egui::Button::new("Install")).clicked() {
                self.install_update();
            }
        });
        if ui.button("Open releases").clicked() {
            if let Err(error) = updater::open_releases_page() {
                self.error_message = Some(error.to_string());
            }
        }
        if let Some(check) = &self.last_update_check {
            ui.small(format!(
                "Current v{} · Latest v{}{}",
                check.current_version,
                check.latest_version,
                if check.prerelease { " prerelease" } else { "" }
            ));
        } else {
            ui.small(format!("Repository: {}", updater::repository_label()));
        }
    }

    fn render_current_playlist_controls(&mut self, ui: &mut egui::Ui) {
        let Some(playlist) = self.current_playlist() else {
            return;
        };

        ui.small(format!("Type: {} {}", playlist.kind.icon(), playlist.kind.label()));

        let groups = playlist.folder_groups();
        let selected_group = playlist.selected_group.clone();
        let selected_label = playlist.selected_group_label();
        let source_folder = playlist.source_folder.clone();
        let folder_depth = playlist.folder_depth;

        if let Some(folder) = source_folder {
            ui.small(format!("Folder: {}", folder.display()));
            ui.small(format!("Grouping depth: {folder_depth} folder level(s)"));
        }

        if !groups.is_empty() {
            let mut next_group = selected_group.clone();
            egui::ComboBox::from_id_salt("folder_group_selector")
                .selected_text(selected_label)
                .height(360.0)
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
        let show_group_headers = playlist.selected_group.is_none();

        ui.horizontal(|ui| {
            ui.heading(format!("{} {}", playlist.kind.icon(), playlist_name));
            ui.label(format!("{visible_count}/{total_count} tracks · {selected_group_label}"));
        });
        ui.separator();

        if visible_count == 0 {
            ui.centered_and_justified(|ui| {
                ui.label("No tracks in this view. Add files or import a music folder.");
            });
            return;
        }

        let visible_tracks: Vec<(usize, Track)> = self
            .current_playlist()
            .map(|playlist| {
                filtered_indexes
                    .into_iter()
                    .filter_map(|index| playlist.tracks.get(index).map(|track| (index, track.clone())))
                    .collect()
            })
            .unwrap_or_default();

        let add_targets: Vec<(usize, String, PlaylistKind)> = self
            .state
            .playlists
            .iter()
            .enumerate()
            .map(|(index, playlist)| (index, playlist.name.clone(), playlist.kind.clone()))
            .collect();

        let mut last_group = String::new();
        egui::ScrollArea::vertical().show(ui, |ui| {
            for (index, track) in visible_tracks {
                if show_group_headers && track.group != last_group {
                    ui.add_space(6.0);
                    ui.heading(track.group.clone());
                    ui.separator();
                    last_group = track.group.clone();
                }

                let is_selected = self.selected_track_index == Some(index);
                let is_active = self
                    .active_track_path
                    .as_ref()
                    .map(|active| same_path(active, &track.path))
                    .unwrap_or(false);
                let favorite = self.is_favorite(&track.path);
                let marker = if is_active { "▶" } else { " " };
                let metadata = format_track_metadata(&track);
                let title = format!("{marker} {}", track.title);
                let path = track.path.clone();

                ui.horizontal(|ui| {
                    let heart = if favorite { "♥" } else { "♡" };
                    if ui.button(heart).on_hover_text("Toggle favorite").clicked() {
                        self.toggle_favorite(path.clone());
                    }

                    let response = ui.selectable_label(is_selected, title);
                    if response.clicked() {
                        self.selected_track_index = Some(index);
                    }
                    if response.double_clicked() {
                        self.selected_track_index = Some(index);
                        self.play_path(path.clone(), Some(index), 0.0);
                    }

                    ui.add_space(8.0);
                    ui.small(metadata);
                    ui.add_space(8.0);
                    draw_mini_waveform(ui, &track.waveform);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        ui.menu_button("⋯", |ui| {
                            if ui.button("Play now").clicked() {
                                self.selected_track_index = Some(index);
                                self.play_path(path.clone(), Some(index), 0.0);
                                ui.close_menu();
                            }
                            if ui.button("Show in File Explorer").clicked() {
                                self.reveal_track_in_file_manager(path.clone());
                                ui.close_menu();
                            }

                            ui.menu_button("Add to playlist", |ui| {
                                for (target_index, target_name, kind) in add_targets.clone() {
                                    if kind.accepts_manual_tracks() {
                                        let label = format!("{} {}", kind.icon(), target_name);
                                        if ui.button(label).clicked() {
                                            self.add_track_to_playlist(path.clone(), target_index);
                                            ui.close_menu();
                                        }
                                    }
                                }
                            });

                            let can_remove_from_playlist = self
                                .current_playlist()
                                .map(|playlist| playlist.kind != PlaylistKind::Folder)
                                .unwrap_or(false);
                            if ui.add_enabled(can_remove_from_playlist, egui::Button::new("Remove from playlist")).clicked() {
                                self.remove_track_from_current_playlist(index);
                                ui.close_menu();
                            }
                            if ui.button("Delete from disk").clicked() {
                                self.delete_track_from_disk(path.clone());
                                ui.close_menu();
                            }
                        });
                    });
                });
                ui.separator();
            }
        });
    }

    fn render_profile_panel(&mut self, ui: &mut egui::Ui) {
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
            self.apply_current_profile_live();
        }

        ui.horizontal(|ui| {
            if ui.button("New profile").clicked() {
                self.add_profile();
            }

            if ui.button("Remove").clicked() {
                self.remove_current_profile();
            }

            if ui.small_button("✎").on_hover_text("Rename profile").clicked() {
                self.editing_profile_index = Some(self.state.selected_profile_index);
            }
        });

        ui.separator();

        let mut profile_changed = false;
        if let Some(profile) = self.state.profiles.get_mut(self.state.selected_profile_index) {
            if self.editing_profile_index == Some(self.state.selected_profile_index) {
                ui.label("Profile name");
                profile_changed |= ui.text_edit_singleline(&mut profile.name).changed();
            }

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

            ui.separator();
            profile_changed |= ui
                .checkbox(&mut profile.settings.skip_silence_enabled, "Skip long silence")
                .changed();
            profile_changed |= ui
                .add(
                    egui::Slider::new(&mut profile.settings.silence_threshold_seconds, 1u8..=12u8)
                        .text("Silence threshold (sec)"),
                )
                .changed();
        }

        if profile_changed {
            self.save_state_silently();
            self.apply_current_profile_live();
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
            .default_width(560.0)
            .show(context, |ui| {
                ui.label("Create a scanner-owned playlist from a folder and group tracks by the first N subfolder levels.");
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

                ui.add_space(8.0);
                ui.label("Playlist name");
                ui.text_edit_singleline(&mut self.pending_playlist_name);

                ui.add(
                    egui::Slider::new(&mut self.pending_folder_depth, 0usize..=5usize)
                        .text("Group by folder levels"),
                );
                ui.small("Example: depth 2 groups D:\\mp3\\Artist\\Album\\song.mp3 as Artist / Album.");

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui.button("Import folder").clicked() {
                        close_after_import = self.import_folder_playlist();
                    }
                    if ui.button("Cancel").clicked() {
                        close_after_import = true;
                    }
                });
            });

        if close_after_import {
            is_open = false;
            self.pending_folder_path = None;
        }

        self.show_folder_import_modal = is_open;
    }
}

fn ensure_state_is_valid(state: &mut SavedState) {
    if !state.playlists.iter().any(|playlist| playlist.kind == PlaylistKind::Favorites) {
        state.playlists.insert(0, Playlist::favorites());
    }

    for playlist in &mut state.playlists {
        if playlist.name == FAVORITES_PLAYLIST_NAME {
            playlist.kind = PlaylistKind::Favorites;
        } else if playlist.source_folder.is_some() {
            playlist.kind = PlaylistKind::Folder;
        }
        playlist.sort_tracks();
        playlist.set_selected_group(playlist.selected_group.clone());
    }

    if state.playlists.is_empty() {
        state.playlists.push(Playlist::favorites());
        state.playlists.push(Playlist::new("Local music"));
    }
    if state.selected_playlist_index >= state.playlists.len() {
        state.selected_playlist_index = 0;
    }

    if state.profiles.is_empty() {
        state.profiles.push(config::DspProfile::new("Smooth orbit", DspSettings::default()));
    }
    if state.selected_profile_index >= state.profiles.len() {
        state.selected_profile_index = 0;
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

fn draw_waveform_seek(ui: &mut egui::Ui, waveform: &[f32], progress: f32, label: String) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), 42.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let visuals = ui.visuals();
    let painter = ui.painter();
    painter.rect_filled(rect, 8.0, visuals.extreme_bg_color);

    if waveform.is_empty() {
        painter.text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            label,
            egui::FontId::proportional(13.0),
            visuals.text_color(),
        );
        return response;
    }

    let progress_x = rect.left() + rect.width() * progress.clamp(0.0, 1.0);
    let bar_width = (rect.width() / waveform.len().max(1) as f32).max(1.0);
    for (index, value) in waveform.iter().enumerate() {
        let x = rect.left() + index as f32 * bar_width;
        let height = (rect.height() * value.clamp(0.03, 1.0)).max(2.0);
        let y1 = rect.center().y - height / 2.0;
        let y2 = rect.center().y + height / 2.0;
        let color = if x <= progress_x {
            visuals.selection.bg_fill
        } else {
            visuals.widgets.inactive.fg_stroke.color.linear_multiply(0.55)
        };
        painter.line_segment(
            [egui::pos2(x, y1), egui::pos2(x, y2)],
            egui::Stroke::new(bar_width.min(3.0), color),
        );
    }

    painter.text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        egui::FontId::proportional(13.0),
        visuals.text_color(),
    );
    response
}

fn draw_mini_waveform(ui: &mut egui::Ui, waveform: &[f32]) {
    let desired_size = egui::vec2(96.0, 18.0);
    let (rect, _response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let painter = ui.painter();
    let visuals = ui.visuals();

    if waveform.is_empty() {
        painter.rect_filled(rect, 3.0, visuals.extreme_bg_color);
        return;
    }

    let step = (waveform.len() / 32).max(1);
    let values = waveform.iter().step_by(step).take(32).copied().collect::<Vec<_>>();
    let bar_width = (rect.width() / values.len().max(1) as f32).max(1.0);
    for (index, value) in values.iter().enumerate() {
        let x = rect.left() + index as f32 * bar_width;
        let height = (rect.height() * value.clamp(0.05, 1.0)).max(1.0);
        painter.line_segment(
            [egui::pos2(x, rect.center().y - height / 2.0), egui::pos2(x, rect.center().y + height / 2.0)],
            egui::Stroke::new(bar_width.min(2.0), visuals.widgets.inactive.fg_stroke.color),
        );
    }
}

fn format_track_metadata(track: &Track) -> String {
    let sample_rate = track
        .metadata
        .sample_rate_hz
        .map(|value| format!("{} kHz", value / 1000))
        .unwrap_or_else(|| "? kHz".to_owned());
    let bitrate = track
        .metadata
        .bitrate_kbps
        .map(|value| format!("{value} kbps"))
        .unwrap_or_else(|| "? kbps".to_owned());
    let channels = track
        .metadata
        .channels
        .map(|value| format!("{value} ch"))
        .unwrap_or_else(|| "? ch".to_owned());
    let size = track
        .metadata
        .size_bytes
        .map(format_file_size)
        .unwrap_or_else(|| "? MB".to_owned());
    let duration = track
        .metadata
        .duration_seconds
        .map(format_duration)
        .unwrap_or_else(|| "?:??".to_owned());

    format!("{duration} · {sample_rate} · {bitrate} · {channels} · {size}")
}

fn format_duration(seconds: f32) -> String {
    if !seconds.is_finite() || seconds <= 0.0 {
        return "0:00".to_owned();
    }

    let total_seconds = seconds.round() as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;

    if hours > 0 {
        format!("{hours}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes}:{seconds:02}")
    }
}

fn format_file_size(bytes: u64) -> String {
    let bytes = bytes as f64;
    let kb = bytes / 1024.0;
    let mb = kb / 1024.0;
    let gb = mb / 1024.0;

    if gb >= 1.0 {
        format!("{gb:.2} GB")
    } else if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else if kb >= 1.0 {
        format!("{kb:.0} KB")
    } else {
        format!("{} B", bytes as u64)
    }
}

fn display_parent(path: &Path) -> String {
    path.parent()
        .map(|parent| parent.display().to_string())
        .unwrap_or_else(|| path.display().to_string())
}

fn reveal_in_file_manager(path: &Path) -> anyhow::Result<()> {
    #[cfg(windows)]
    {
        Command::new("explorer")
            .arg(format!("/select,{}", path.display()))
            .spawn()?;
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        let folder = path.parent().unwrap_or(path);
        Command::new("xdg-open").arg(folder).spawn()?;
        Ok(())
    }
}
