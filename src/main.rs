#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_player;
mod config;
mod dsp;
mod icon;
mod media_keys;
mod single_instance;
mod ui_icons;
mod updater;

use crate::{
    audio_player::{current_default_output_device_name, AudioPlayer, PlaybackInfo},
    config::{
        app_data_dir, collect_audio_files_from_folder, display_file_name, export_state_zip,
        import_state_zip, load_state, same_path, save_state, LastPlayedTrack, Playlist, PlaylistKind, RadioStation, RepeatMode, SavedState,
        Track, WindowGeometry, FAVORITES_PLAYLIST_NAME,
    },
    dsp::{DspSettings, OrbitMode},
};
use eframe::egui;
use lucide_icons::Icon;
use rfd::FileDialog;
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const MAX_UPDATE_CHECKS_PER_SESSION: u8 = 2;
const AUTOMATIC_UPDATE_CHECK_INTERVAL_SECONDS: u64 = 60 * 60;

fn min_window_size_for_mode(player_only_mode: bool) -> egui::Vec2 {
    if player_only_mode {
        egui::vec2(380.0, 220.0)
    } else {
        egui::vec2(900.0, 560.0)
    }
}

fn default_window_size_for_mode(player_only_mode: bool) -> egui::Vec2 {
    if player_only_mode {
        egui::vec2(520.0, 300.0)
    } else {
        egui::vec2(1240.0, 780.0)
    }
}

fn saved_window_geometry_for_mode(state: &SavedState, player_only_mode: bool) -> Option<WindowGeometry> {
    let geometry = if player_only_mode {
        state.ui.player_only_window_geometry
    } else {
        state.ui.full_layout_window_geometry
    };

    geometry
        .or(state.ui.window_geometry)
        .filter(WindowGeometry::is_valid)
}

fn main() -> eframe::Result<()> {
    let _single_instance_guard = match single_instance::acquire() {
        Ok(Some(guard)) => guard,
        Ok(None) => return Ok(()),
        Err(error) => {
            eprintln!("{error}");
            return Ok(());
        }
    };

    let mut state = load_state();
    ensure_state_is_valid(&mut state);

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size(initial_window_size(&state))
        .with_min_inner_size(min_window_size_for_mode(state.ui.player_only_mode))
        .with_resizable(true);

    if let Some(position) = initial_window_position(&state) {
        viewport = viewport.with_position(position);
    }

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
        Box::new(move |creation_context| {
            ui_icons::install(&creation_context.egui_ctx);
            configure_app_style(&creation_context.egui_ctx);
            Ok(Box::new(AudioOrbitApp::new(state)))
        }),
    )
}

fn configure_app_style(context: &egui::Context) {
    let mut style = (*context.style()).clone();
    style
        .text_styles
        .insert(egui::TextStyle::Small, egui::FontId::proportional(12.0));
    context.set_style(style);
}

fn initial_window_size(state: &SavedState) -> egui::Vec2 {
    let player_only_mode = state.ui.player_only_mode;
    let min_size = min_window_size_for_mode(player_only_mode);

    saved_window_geometry_for_mode(state, player_only_mode)
        .map(|geometry| egui::vec2(geometry.width.max(min_size.x), geometry.height.max(min_size.y)))
        .unwrap_or_else(|| default_window_size_for_mode(player_only_mode))
}

fn initial_window_position(state: &SavedState) -> Option<egui::Pos2> {
    saved_window_geometry_for_mode(state, state.ui.player_only_mode)
        .or_else(|| saved_window_geometry_for_mode(state, !state.ui.player_only_mode))
        .map(|geometry| egui::pos2(geometry.x, geometry.y))
}


#[derive(Clone, Debug)]
struct PendingTrackSwitch {
    switch_at: Instant,
    started_at: Instant,
    previous_position: f32,
    previous_duration: f32,
    index: Option<usize>,
    info: PlaybackInfo,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MainContentTab {
    Music,
    Radio,
}

struct AudioOrbitApp {
    player: Option<AudioPlayer>,
    state: SavedState,
    selected_track_index: Option<usize>,
    selected_track_indexes: BTreeSet<usize>,
    active_track_index: Option<usize>,
    active_track_path: Option<PathBuf>,
    last_playback: Option<PlaybackInfo>,
    status_message: String,
    status_last_seen: String,
    status_updated_at: Instant,
    error_message: Option<String>,
    crossfade_started_for_path: Option<PathBuf>,
    pending_track_switch: Option<PendingTrackSwitch>,
    pending_profile_apply_at: Option<Instant>,
    suppress_window_geometry_save_until: Option<Instant>,
    show_folder_import_modal: bool,
    show_settings_modal: bool,
    show_release_modal: bool,
    show_library_panel: bool,
    show_profile_panel: bool,
    player_only_mode: bool,
    show_track_search: bool,
    active_tab: MainContentTab,
    track_search_query: String,
    search_cursor: usize,
    pending_radio_name: String,
    pending_radio_url: String,
    radio_search_query: String,
    radio_show_favorites_only: bool,
    active_radio_index: Option<usize>,
    active_radio_title: Option<String>,
    radio_started_at: Option<Instant>,
    last_radio_title_lookup_at: Option<Instant>,
    radio_title_receiver: Option<mpsc::Receiver<(usize, Option<String>)>>,
    collapsed_groups: BTreeSet<String>,
    pending_folder_path: Option<PathBuf>,
    pending_playlist_name: String,
    pending_folder_depth: usize,
    last_known_output_name: String,
    detected_output_change: Option<String>,
    last_output_check: Instant,
    editing_playlist_index: Option<usize>,
    editing_profile_index: Option<usize>,
    last_update_check: Option<updater::UpdateCheck>,
    update_check_receiver: Option<mpsc::Receiver<Result<updater::UpdateCheck, String>>>,
    update_check_count: u8,
    media_key_receiver: Option<mpsc::Receiver<media_keys::MediaKeyEvent>>,
    media_key_status: String,
}

impl AudioOrbitApp {
    fn new(state: SavedState) -> Self {
        let pending_playlist_name = "Local music".to_owned();
        let current_output_name = current_default_output_device_name();
        let show_library_panel = state.ui.show_library_panel;
        let show_profile_panel = state.ui.show_profile_panel;
        let player_only_mode = state.ui.player_only_mode;
        let show_track_search = state.ui.show_track_search;

        let mut app = match AudioPlayer::new() {
            Ok(player) => {
                let output_name = player.output_device_name().to_owned();
                Self {
                    player: Some(player),
                    state,
                    selected_track_index: None,
                    selected_track_indexes: BTreeSet::new(),
                    active_track_index: None,
                    active_track_path: None,
                    last_playback: None,
                    status_message: format!("Ready. Output device: {output_name}"),
                    status_last_seen: String::new(),
                    status_updated_at: Instant::now(),
                    error_message: None,
                    crossfade_started_for_path: None,
                    pending_track_switch: None,
                    pending_profile_apply_at: None,
                    suppress_window_geometry_save_until: None,
                    show_folder_import_modal: false,
                    show_settings_modal: false,
                    show_release_modal: false,
                    show_library_panel,
                    show_profile_panel,
                    player_only_mode,
                    show_track_search,
                    active_tab: MainContentTab::Music,
                    track_search_query: String::new(),
                    search_cursor: 0,
                    pending_radio_name: String::new(),
                    pending_radio_url: String::new(),
                    radio_search_query: String::new(),
                    radio_show_favorites_only: false,
                    active_radio_index: None,
                    active_radio_title: None,
                    radio_started_at: None,
                    last_radio_title_lookup_at: None,
                    radio_title_receiver: None,
                    collapsed_groups: BTreeSet::new(),
                    pending_folder_path: None,
                    pending_playlist_name,
                    pending_folder_depth: 2,
                    last_known_output_name: output_name,
                    detected_output_change: None,
                    last_output_check: Instant::now(),
                    editing_playlist_index: None,
                    editing_profile_index: None,
                    last_update_check: None,
                    update_check_receiver: None,
                    update_check_count: 0,
                    media_key_receiver: None,
                    media_key_status: "Media keys: unavailable".to_owned(),
                }
            }
            Err(error) => Self {
                player: None,
                state,
                selected_track_index: None,
                selected_track_indexes: BTreeSet::new(),
                active_track_index: None,
                active_track_path: None,
                last_playback: None,
                status_message: "No audio output device is available.".to_owned(),
                status_last_seen: String::new(),
                status_updated_at: Instant::now(),
                error_message: Some(error.to_string()),
                crossfade_started_for_path: None,
                pending_track_switch: None,
                pending_profile_apply_at: None,
                suppress_window_geometry_save_until: None,
                show_folder_import_modal: false,
                show_settings_modal: false,
                show_release_modal: false,
                show_library_panel,
                show_profile_panel,
                player_only_mode,
                show_track_search,
                active_tab: MainContentTab::Music,
                track_search_query: String::new(),
                search_cursor: 0,
                pending_radio_name: String::new(),
                pending_radio_url: String::new(),
                radio_search_query: String::new(),
                radio_show_favorites_only: false,
                active_radio_index: None,
                active_radio_title: None,
                radio_started_at: None,
                last_radio_title_lookup_at: None,
                radio_title_receiver: None,
                collapsed_groups: BTreeSet::new(),
                pending_folder_path: None,
                pending_playlist_name,
                pending_folder_depth: 2,
                last_known_output_name: current_output_name,
                detected_output_change: None,
                last_output_check: Instant::now(),
                editing_playlist_index: None,
                editing_profile_index: None,
                last_update_check: None,
                update_check_receiver: None,
                update_check_count: 0,
                media_key_receiver: None,
                media_key_status: "Media keys: unavailable".to_owned(),
            },
        };

        if let Some(player) = &mut app.player {
            player.set_volume_percent(app.effective_volume_percent());
        }

        let media_keys = media_keys::start_listener();
        app.media_key_receiver = media_keys.receiver;
        app.media_key_status = media_keys.status_message;
        app.restore_last_played_track_selection();
        app.start_automatic_update_check_if_due();
        app
    }

    fn remember_window_geometry(&mut self, context: &egui::Context) {
        if self
            .suppress_window_geometry_save_until
            .map(|blocked_until| blocked_until > Instant::now())
            .unwrap_or(false)
        {
            return;
        }

        let (inner_rect, outer_rect) = context.input(|input| {
            let viewport = input.viewport();
            (viewport.inner_rect, viewport.outer_rect)
        });

        let Some(inner_rect) = inner_rect else {
            return;
        };

        if inner_rect.width() < 320.0 || inner_rect.height() < 180.0 {
            return;
        }

        let position = outer_rect.map(|rect| rect.min).unwrap_or(inner_rect.min);
        let geometry = WindowGeometry {
            x: position.x,
            y: position.y,
            width: inner_rect.width(),
            height: inner_rect.height(),
        };

        if self.player_only_mode {
            self.state.ui.player_only_window_geometry = Some(geometry);
        } else {
            self.state.ui.full_layout_window_geometry = Some(geometry);
        }
        self.state.ui.window_geometry = Some(geometry);
    }

    fn saved_window_size_for_mode(&self, player_only_mode: bool) -> egui::Vec2 {
        let min_size = min_window_size_for_mode(player_only_mode);
        let geometry = if player_only_mode {
            self.state.ui.player_only_window_geometry
        } else {
            self.state.ui.full_layout_window_geometry
        };

        geometry
            .filter(WindowGeometry::is_valid)
            .map(|geometry| egui::vec2(geometry.width.max(min_size.x), geometry.height.max(min_size.y)))
            .unwrap_or_else(|| default_window_size_for_mode(player_only_mode))
    }

    fn apply_window_mode_size(&self, context: &egui::Context, player_only_mode: bool) {
        context.send_viewport_cmd(egui::ViewportCommand::MinInnerSize(min_window_size_for_mode(player_only_mode)));
        context.send_viewport_cmd(egui::ViewportCommand::InnerSize(self.saved_window_size_for_mode(player_only_mode)));
    }

    fn toggle_player_only_mode(&mut self, context: &egui::Context) {
        self.remember_window_geometry(context);
        self.player_only_mode = !self.player_only_mode;
        self.state.ui.player_only_mode = self.player_only_mode;
        self.apply_window_mode_size(context, self.player_only_mode);
        self.suppress_window_geometry_save_until = Some(Instant::now() + Duration::from_millis(450));
        self.save_state_silently();
    }

    fn current_playlist(&self) -> Option<&Playlist> {
        self.state.playlists.get(self.state.selected_playlist_index)
    }

    fn current_playlist_mut(&mut self) -> Option<&mut Playlist> {
        self.state.playlists.get_mut(self.state.selected_playlist_index)
    }

    fn select_playlist(&mut self, index: usize) {
        if index >= self.state.playlists.len() {
            return;
        }

        self.state.selected_playlist_index = index;
        self.selected_track_indexes.clear();
        self.selected_track_index = self.eligible_track_indexes().first().copied();
        self.collapsed_groups.clear();
        self.search_cursor = 0;
        self.save_state_silently();
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

    fn visible_track_indexes(&self) -> Vec<usize> {
        let query = self.track_search_query.trim().to_lowercase();
        let Some(playlist) = self.current_playlist() else {
            return Vec::new();
        };

        playlist
            .filtered_track_indexes()
            .into_iter()
            .filter(|index| {
                playlist
                    .tracks
                    .get(*index)
                    .map(|track| {
                        query.is_empty()
                            || track.title.to_lowercase().contains(&query)
                            || track.group.to_lowercase().contains(&query)
                            || track.path.to_string_lossy().to_lowercase().contains(&query)
                    })
                    .unwrap_or(false)
            })
            .collect()
    }

    fn playback_sequence_indexes(&self) -> Vec<usize> {
        let indexes = self.eligible_track_indexes();
        if self.state.playback.repeat_mode == RepeatMode::Selection && !self.selected_track_indexes.is_empty() {
            indexes
                .into_iter()
                .filter(|index| self.selected_track_indexes.contains(index))
                .collect()
        } else {
            indexes
        }
    }

    fn selected_track_path(&self) -> Option<PathBuf> {
        let playlist = self.current_playlist()?;
        let index = self.selected_track_index?;
        playlist.tracks.get(index).map(|track| track.path.clone())
    }

    fn remember_last_played_track(&mut self, index: Option<usize>, path: &Path) {
        let Some(track_index) = index else {
            return;
        };
        self.state.last_played_track = Some(LastPlayedTrack {
            playlist_index: self.state.selected_playlist_index,
            track_path: path.to_path_buf(),
        });
        self.selected_track_index = Some(track_index);
        self.save_state_silently();
    }

    fn restore_last_played_track_selection(&mut self) {
        let Some(last_played) = self.state.last_played_track.clone() else {
            return;
        };
        let Some(playlist) = self.state.playlists.get(last_played.playlist_index) else {
            return;
        };
        let Some(track_index) = playlist
            .tracks
            .iter()
            .position(|track| same_path(&track.path, &last_played.track_path))
        else {
            return;
        };
        self.state.selected_playlist_index = last_played.playlist_index;
        self.selected_track_index = Some(track_index);
    }

    fn active_track_title(&self) -> String {
        if let Some(radio_index) = self.active_radio_index {
            if let Some(station) = self.state.radio_stations.get(radio_index) {
                let stream_title = self
                    .active_radio_title
                    .clone()
                    .or_else(|| station.last_stream_title.clone())
                    .filter(|title| !title.trim().is_empty());

                return match stream_title {
                    Some(title) if !title.eq_ignore_ascii_case(&station.name) => {
                        format!("{} — {}", station.name, title)
                    }
                    _ => station.name.clone(),
                };
            }
        }

        self.active_track_path
            .as_ref()
            .map(|path| display_file_name(path))
            .unwrap_or_else(|| "No track playing".to_owned())
    }

    fn active_track_detail(&self) -> Option<String> {
        if let Some(radio_index) = self.active_radio_index {
            return self.state.radio_stations.get(radio_index).map(|station| {
                let elapsed = self.radio_elapsed_seconds().map(format_duration).unwrap_or_else(|| "0:00".to_owned());
                format!("Internet radio · playing for {elapsed} · {}", station.url)
            });
        }

        self.last_playback.as_ref().map(|playback| {
            format!(
                "{} · {} Hz · {} ch · {} · rendered {}",
                display_parent(&playback.path),
                playback.sample_rate,
                playback.input_channels,
                playback.size_bytes.map(format_file_size).unwrap_or_else(|| "unknown size".to_owned()),
                format_duration(playback.rendered_duration_seconds)
            )
        })
    }

    fn radio_elapsed_seconds(&self) -> Option<f32> {
        self.radio_started_at.map(|started_at| started_at.elapsed().as_secs_f32())
    }

    fn add_radio_station(&mut self) {
        let typed_name = self.pending_radio_name.trim().to_owned();
        let url = self.pending_radio_url.trim().to_owned();
        if url.is_empty() {
            self.error_message = Some("Radio stream URL is required.".to_owned());
            return;
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            self.error_message = Some("Internet radio stream URL must start with http:// or https://.".to_owned());
            return;
        }

        let station_name = if typed_name.is_empty() {
            self.status_message = "Reading internet radio station name...".to_owned();
            fetch_radio_stream_title(&url).unwrap_or_else(|| fallback_radio_station_name(&url))
        } else {
            typed_name
        };

        self.state.radio_stations.push(RadioStation::new(station_name, url));
        self.state.selected_radio_index = Some(self.state.radio_stations.len() - 1);
        self.pending_radio_name.clear();
        self.pending_radio_url.clear();
        self.status_message = "Added internet radio station.".to_owned();
        self.error_message = None;
        self.save_state_silently();
    }

    fn remove_selected_radio_station(&mut self) {
        let Some(index) = self.state.selected_radio_index else {
            return;
        };
        if index >= self.state.radio_stations.len() {
            return;
        }
        if self.active_radio_index == Some(index) {
            self.stop();
        }
        self.state.radio_stations.remove(index);
        self.state.selected_radio_index = if self.state.radio_stations.is_empty() {
            None
        } else {
            Some(index.min(self.state.radio_stations.len() - 1))
        };
        self.save_state_silently();
    }

    fn play_radio_station(&mut self, index: usize) {
        let Some(station) = self.state.radio_stations.get(index).cloned() else {
            return;
        };
        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        };

        self.status_message = format!("Opening internet radio: {}...", station.name);
        self.error_message = None;
        match player.play_radio_stream(&station.url) {
            Ok(()) => {
                self.active_tab = MainContentTab::Radio;
                self.active_radio_index = Some(index);
                self.active_radio_title = station.last_stream_title.clone();
                self.radio_started_at = Some(Instant::now());
                self.last_radio_title_lookup_at = Some(Instant::now());
                self.active_track_index = None;
                self.active_track_path = None;
                self.last_playback = None;
                self.pending_track_switch = None;
                self.state.selected_radio_index = Some(index);
                self.status_message = format!("Playing internet radio: {}.", station.name);
                self.save_state_silently();
                self.start_radio_title_lookup(index, station.url);
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = "Internet radio playback failed.".to_owned();
            }
        }
    }

    fn start_radio_title_lookup(&mut self, index: usize, url: String) {
        let (sender, receiver) = mpsc::channel();
        self.radio_title_receiver = Some(receiver);
        thread::spawn(move || {
            let title = fetch_radio_stream_title(&url);
            let _ = sender.send((index, title));
        });
    }

    fn process_radio_title_events(&mut self) {
        let Some(receiver) = &self.radio_title_receiver else {
            return;
        };
        let events = receiver.try_iter().collect::<Vec<_>>();
        let mut completed = false;
        for (index, title) in events {
            completed = true;
            if let Some(title) = title {
                if self.active_radio_index == Some(index) {
                    self.active_radio_title = Some(title.clone());
                }
                if let Some(station) = self.state.radio_stations.get_mut(index) {
                    station.last_stream_title = Some(title);
                }
                self.save_state_silently();
            }
        }
        if completed {
            self.radio_title_receiver = None;
        }
    }

    fn refresh_radio_title_periodically(&mut self) {
        let Some(index) = self.active_radio_index else {
            return;
        };
        if self.radio_title_receiver.is_some() {
            return;
        }
        let Some(last_lookup) = self.last_radio_title_lookup_at else {
            return;
        };
        if last_lookup.elapsed() < Duration::from_secs(15) {
            return;
        }
        let Some(station) = self.state.radio_stations.get(index) else {
            return;
        };
        self.last_radio_title_lookup_at = Some(Instant::now());
        self.start_radio_title_lookup(index, station.url.clone());
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
                self.selected_track_indexes.clear();
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
                self.show_library_panel = self.state.ui.show_library_panel;
                self.show_profile_panel = self.state.ui.show_profile_panel;
                self.player_only_mode = self.state.ui.player_only_mode;
                self.selected_track_indexes.clear();
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
        self.selected_track_indexes.clear();
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
        self.selected_track_indexes.clear();
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
        self.schedule_current_profile_apply();
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
        let crossfade_seconds = if start_seconds <= 0.05 {
            self.configured_manual_crossfade_seconds()
        } else {
            0.0
        };
        self.play_path_with_crossfade(path, index, start_seconds, crossfade_seconds);
    }

    fn play_path_with_crossfade(
        &mut self,
        path: PathBuf,
        index: Option<usize>,
        start_seconds: f32,
        crossfade_seconds: f32,
    ) {
        let settings = self.current_settings();
        let crossfade_ui_delay = if crossfade_seconds > 0.05 && self.active_track_path.is_some() {
            let previous_position = self.displayed_playback_position_seconds();
            let previous_duration = self.displayed_playback_duration_seconds();
            Some((previous_position, previous_duration, Instant::now()))
        } else {
            None
        };

        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        };

        self.status_message = if crossfade_seconds > 0.05 {
            format!("Crossfading for {:.1} second(s)...", crossfade_seconds)
        } else {
            "Decoding and processing audio...".to_owned()
        };
        self.error_message = None;

        let result = if crossfade_seconds > 0.05 {
            player.crossfade_to_file_with_orbit_from(&path, settings, start_seconds, crossfade_seconds)
        } else {
            player.play_file_with_orbit_from(&path, settings, start_seconds)
        };

        match result {
            Ok(info) => {
                let mode_label = if settings.orbit_enabled {
                    settings.mode.label()
                } else {
                    "normal stereo playback"
                };
                self.active_tab = MainContentTab::Music;
                self.active_radio_index = None;
                self.active_radio_title = None;
                self.radio_started_at = None;
                self.last_radio_title_lookup_at = None;
                self.radio_title_receiver = None;
                if let Some((previous_position, previous_duration, started_at)) = crossfade_ui_delay {
                    let switch_after = Duration::from_secs_f32((crossfade_seconds * 0.5).max(0.1));
                    self.pending_track_switch = Some(PendingTrackSwitch {
                        switch_at: started_at + switch_after,
                        started_at,
                        previous_position,
                        previous_duration,
                        index,
                        info: info.clone(),
                    });
                    self.selected_track_index = index;
                    self.status_message = format!(
                        "Crossfading to {} through {}; display switches halfway through the mix.",
                        display_file_name(&info.path),
                        mode_label
                    );
                } else {
                    self.status_message = if crossfade_seconds > 0.05 {
                        format!(
                            "Crossfading to {} through {}.",
                            display_file_name(&info.path),
                            mode_label
                        )
                    } else {
                        format!(
                            "Playing {} through {}.",
                            display_file_name(&info.path),
                            mode_label
                        )
                    };
                    self.active_track_index = index;
                    self.active_track_path = Some(info.path.clone());
                    self.pending_track_switch = None;
                    self.crossfade_started_for_path = None;
                    self.store_playback_metadata(&info);
                    self.remember_last_played_track(index, &info.path);
                    self.last_playback = Some(info);
                }
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = "Playback failed.".to_owned();
            }
        }
    }

    fn play_next_track(&mut self) {
        let crossfade_seconds = self.configured_manual_crossfade_seconds();
        self.play_next_track_with_crossfade(crossfade_seconds);
    }

    fn play_next_track_with_crossfade(&mut self, crossfade_seconds: f32) {
        let Some((next_index, path)) = self.next_track_candidate() else {
            return;
        };

        self.selected_track_index = Some(next_index);
        self.play_path_with_crossfade(path, Some(next_index), 0.0, crossfade_seconds);
    }

    fn next_track_candidate(&self) -> Option<(usize, PathBuf)> {
        let indexes = self.playback_sequence_indexes();
        if indexes.is_empty() {
            return None;
        }

        let current_index = self.active_track_index.or(self.selected_track_index);
        let next_index = if self.state.playback.repeat_mode == RepeatMode::Track {
            current_index.unwrap_or(indexes[0])
        } else if self.state.playback.shuffle_enabled {
            self.random_sequence_index(&indexes, current_index)?
        } else {
            let current_position = current_index.and_then(|index| indexes.iter().position(|candidate| *candidate == index));
            let next_position = current_position.map(|position| (position + 1) % indexes.len()).unwrap_or(0);
            indexes[next_position]
        };

        let path = self
            .current_playlist()?
            .tracks
            .get(next_index)?
            .path
            .clone();

        Some((next_index, path))
    }

    fn configured_manual_crossfade_seconds(&self) -> f32 {
        let is_currently_playing = self
            .player
            .as_ref()
            .map(AudioPlayer::is_playing)
            .unwrap_or(false);
        let is_local_track_playing = self.active_radio_index.is_none() && self.active_track_path.is_some();

        if self.state.playback.crossfade_enabled && is_currently_playing && is_local_track_playing {
            self.state.playback.crossfade_seconds.max(1) as f32
        } else {
            0.0
        }
    }

    fn play_previous_track(&mut self) {
        let indexes = self.playback_sequence_indexes();
        if indexes.is_empty() {
            return;
        }

        let current_index = self.active_track_index.or(self.selected_track_index);
        let previous_index = if self.state.playback.repeat_mode == RepeatMode::Track {
            current_index.unwrap_or(indexes[0])
        } else {
            let current_position = current_index.and_then(|index| indexes.iter().position(|candidate| *candidate == index));
            let previous_position = current_position
                .map(|position| if position == 0 { indexes.len() - 1 } else { position - 1 })
                .unwrap_or(0);
            indexes[previous_position]
        };

        let Some(path) = self
            .current_playlist()
            .and_then(|playlist| playlist.tracks.get(previous_index))
            .map(|track| track.path.clone())
        else {
            return;
        };

        self.selected_track_index = Some(previous_index);
        let crossfade_seconds = self.configured_manual_crossfade_seconds();
        self.play_path_with_crossfade(path, Some(previous_index), 0.0, crossfade_seconds);
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

    fn schedule_current_profile_apply(&mut self) {
        self.pending_profile_apply_at = Some(Instant::now() + Duration::from_secs(3));
    }

    fn process_pending_profile_apply(&mut self) {
        let Some(apply_at) = self.pending_profile_apply_at else {
            return;
        };

        if Instant::now() < apply_at {
            return;
        }

        self.pending_profile_apply_at = None;
        self.save_state_silently();
        self.apply_current_profile_live();
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
                self.status_message = "Applied sound profile after settings settled.".to_owned();
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

    fn process_pending_track_switch(&mut self) {
        let Some(pending) = self.pending_track_switch.clone() else {
            return;
        };

        if Instant::now() < pending.switch_at {
            return;
        }

        self.pending_track_switch = None;
        self.active_track_index = pending.index;
        self.active_track_path = Some(pending.info.path.clone());
        self.selected_track_index = pending.index;
        self.crossfade_started_for_path = None;
        self.remember_last_played_track(pending.index, &pending.info.path);
        self.store_playback_metadata(&pending.info);
        self.last_playback = Some(pending.info);
    }

    fn displayed_playback_position_seconds(&self) -> f32 {
        if let Some(pending) = &self.pending_track_switch {
            let elapsed = pending.started_at.elapsed().as_secs_f32();
            return (pending.previous_position + elapsed).min(pending.previous_duration);
        }

        self.player
            .as_ref()
            .map(AudioPlayer::playback_position_seconds)
            .unwrap_or(0.0)
    }

    fn displayed_playback_duration_seconds(&self) -> f32 {
        if let Some(pending) = &self.pending_track_switch {
            return pending.previous_duration;
        }

        self.player
            .as_ref()
            .and_then(AudioPlayer::playback_duration_seconds)
            .or_else(|| self.last_playback.as_ref().map(|playback| playback.original_duration_seconds))
            .unwrap_or(0.0)
    }

    fn process_keyboard_shortcuts(&mut self, context: &egui::Context) {
        if self.show_folder_import_modal || self.show_settings_modal || self.show_release_modal || context.wants_keyboard_input() {
            return;
        }

        let (space, enter, stop, next, previous, seek_forward, seek_backward, search, player_only, library, profiles) = context.input(|input| {
            (
                input.key_pressed(egui::Key::Space),
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::S),
                input.key_pressed(egui::Key::ArrowRight) && input.modifiers.ctrl,
                input.key_pressed(egui::Key::ArrowLeft) && input.modifiers.ctrl,
                input.key_pressed(egui::Key::ArrowRight) && !input.modifiers.ctrl,
                input.key_pressed(egui::Key::ArrowLeft) && !input.modifiers.ctrl,
                input.key_pressed(egui::Key::F) && input.modifiers.ctrl,
                input.key_pressed(egui::Key::M),
                input.key_pressed(egui::Key::L) && input.modifiers.ctrl,
                input.key_pressed(egui::Key::P) && input.modifiers.ctrl,
            )
        });

        if space {
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

        if enter {
            self.play_selected_or_first_track();
        }

        if stop {
            self.stop();
        }

        if next {
            self.play_next_track();
        }

        if previous {
            self.play_previous_track();
        }

        if seek_forward {
            self.seek_relative(10.0);
        }

        if seek_backward {
            self.seek_relative(-10.0);
        }

        if search {
            self.show_track_search = !self.show_track_search;
            self.state.ui.show_track_search = self.show_track_search;
            if !self.show_track_search {
                self.track_search_query.clear();
                self.search_cursor = 0;
            }
            self.save_state_silently();
        }

        if player_only {
            self.toggle_player_only_mode(context);
        }

        if library && !self.player_only_mode {
            self.show_library_panel = !self.show_library_panel;
            self.state.ui.show_library_panel = self.show_library_panel;
            self.save_state_silently();
        }

        if profiles && !self.player_only_mode {
            self.show_profile_panel = !self.show_profile_panel;
            self.state.ui.show_profile_panel = self.show_profile_panel;
            self.save_state_silently();
        }
    }

    fn process_media_key_events(&mut self) {
        let events = match &self.media_key_receiver {
            Some(receiver) => receiver.try_iter().collect::<Vec<_>>(),
            None => return,
        };

        for event in events {
            match event {
                media_keys::MediaKeyEvent::Ready { registered, failed } => {
                    self.media_key_status = media_key_status_message(&registered, &failed);
                }
                media_keys::MediaKeyEvent::Command(command) => {
                    self.handle_media_key_command(command);
                }
            }
        }
    }

    fn handle_media_key_command(&mut self, command: media_keys::MediaKeyCommand) {
        match command {
            media_keys::MediaKeyCommand::Previous => self.play_previous_track(),
            media_keys::MediaKeyCommand::PlayPause => {
                let is_active = self
                    .player
                    .as_ref()
                    .map(|player| player.is_playing() || player.is_paused())
                    .unwrap_or(false);

                if is_active {
                    self.pause_or_resume();
                } else if self.active_tab == MainContentTab::Radio {
                    if let Some(index) = self.state.selected_radio_index {
                        self.play_radio_station(index);
                    }
                } else {
                    self.play_selected_or_first_track();
                }
            }
            media_keys::MediaKeyCommand::Stop => self.stop(),
            media_keys::MediaKeyCommand::Next => self.play_next_track(),
        }
    }

    fn stop(&mut self) {
        if let Some(player) = &mut self.player {
            player.stop();
        }

        self.active_track_index = None;
        self.active_track_path = None;
        self.active_radio_index = None;
        self.active_radio_title = None;
        self.radio_started_at = None;
        self.last_radio_title_lookup_at = None;
        self.radio_title_receiver = None;
        self.pending_track_switch = None;
        self.crossfade_started_for_path = None;
        self.last_playback = None;
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
            Ok(mut player) => {
                player.set_volume_percent(self.effective_volume_percent());
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
        if self.update_check_receiver.is_some() {
            self.error_message = Some("An update check is already running.".to_owned());
            return;
        }

        if self.update_check_count >= MAX_UPDATE_CHECKS_PER_SESSION {
            self.error_message = Some(
                "Update check limit reached for this app session. Restart Audio Orbit before checking again."
                    .to_owned(),
            );
            return;
        }

        self.update_check_count += 1;

        match updater::check_for_update(self.state.update_settings.include_prereleases) {
            Ok(check) => {
                self.handle_update_check_result(check, false);
            }
            Err(error) => self.error_message = Some(error.to_string()),
        }
    }

    fn start_automatic_update_check_if_due(&mut self) {
        if self.update_check_receiver.is_some() || self.update_check_count >= MAX_UPDATE_CHECKS_PER_SESSION {
            return;
        }

        let now = current_unix_seconds();
        let last_check = self.state.update_settings.last_auto_check_unix_seconds;
        if now.saturating_sub(last_check) < AUTOMATIC_UPDATE_CHECK_INTERVAL_SECONDS {
            return;
        }

        self.state.update_settings.last_auto_check_unix_seconds = now;
        self.update_check_count += 1;
        self.save_state_silently();

        let include_prereleases = self.state.update_settings.include_prereleases;
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = updater::check_for_update(include_prereleases).map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        self.update_check_receiver = Some(receiver);
    }

    fn process_update_check_events(&mut self) {
        let Some(receiver) = &self.update_check_receiver else {
            return;
        };

        match receiver.try_recv() {
            Ok(Ok(check)) => {
                self.update_check_receiver = None;
                self.handle_update_check_result(check, true);
            }
            Ok(Err(error)) => {
                self.update_check_receiver = None;
                self.error_message = Some(format!("Automatic update check failed: {error}"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.update_check_receiver = None;
            }
        }
    }

    fn handle_update_check_result(&mut self, check: updater::UpdateCheck, automatic: bool) {
        if check.is_update_available {
            self.status_message = format!(
                "Update available: v{}{}.",
                check.latest_version,
                if check.prerelease { " prerelease" } else { "" }
            );
            if automatic {
                self.show_release_modal = true;
            }
        } else if automatic {
            self.status_message = format!("Audio Orbit is already on the latest release: v{}.", check.current_version);
        } else {
            self.status_message = format!("Audio Orbit is already on the latest release: v{}.", check.current_version);
        }

        self.last_update_check = Some(check);
        self.error_message = None;
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

    fn sync_status_lifetime(&mut self) {
        if self.status_message != self.status_last_seen {
            self.status_last_seen = self.status_message.clone();
            self.status_updated_at = Instant::now();
            return;
        }

        if !self.status_message.is_empty() && self.status_updated_at.elapsed() >= Duration::from_secs(10) {
            self.status_message.clear();
            self.status_last_seen.clear();
            self.status_updated_at = Instant::now();
        }
    }

    fn effective_volume_percent(&self) -> u8 {
        if self.state.playback.muted {
            0
        } else {
            self.state.playback.volume_percent
        }
    }

    fn apply_effective_volume_to_player(&mut self) {
        let effective_volume = self.effective_volume_percent();
        if let Some(player) = &mut self.player {
            player.set_volume_percent(effective_volume);
        }
    }

    fn set_volume_percent(&mut self, volume_percent: u8) {
        let next_volume = volume_percent.clamp(0, 100);
        let next_muted = next_volume == 0;
        if self.state.playback.volume_percent == next_volume && self.state.playback.muted == next_muted {
            return;
        }

        self.state.playback.volume_percent = next_volume;
        self.state.playback.muted = next_muted;
        self.apply_effective_volume_to_player();
        self.status_message = if self.state.playback.muted {
            "Volume muted.".to_owned()
        } else {
            format!("Volume: {next_volume}%.")
        };
        self.save_state_silently();
    }

    fn toggle_mute(&mut self) {
        if self.state.playback.muted || self.state.playback.volume_percent == 0 {
            if self.state.playback.volume_percent == 0 {
                self.state.playback.volume_percent = 50;
            }
            self.state.playback.muted = false;
            self.status_message = format!("Volume: {}%.", self.state.playback.volume_percent);
        } else {
            self.state.playback.muted = true;
            self.status_message = "Volume muted.".to_owned();
        }
        self.apply_effective_volume_to_player();
        self.save_state_silently();
    }

    fn adjust_volume(&mut self, delta_percent: i16) {
        let current = if self.state.playback.muted {
            0
        } else {
            self.state.playback.volume_percent as i16
        };
        let next = (current + delta_percent).clamp(0, 100) as u8;
        self.set_volume_percent(next);
    }

    fn handle_top_panel_volume_wheel(&mut self, response: &egui::Response, context: &egui::Context) {
        if !response.hovered() {
            return;
        }

        let scroll_y = context.input(|input| input.raw_scroll_delta.y + input.smooth_scroll_delta.y);
        if scroll_y.abs() < 0.5 {
            return;
        }

        let steps = (scroll_y / 80.0).round() as i16;
        let steps = if steps == 0 { scroll_y.signum() as i16 } else { steps };
        self.adjust_volume(steps * 2);
    }

    fn seek_relative(&mut self, delta_seconds: f32) {
        let Some(player) = self.player.as_ref() else {
            return;
        };

        if !(player.is_playing() || player.is_paused()) {
            return;
        }

        let current = player.playback_position_seconds();
        let duration = player.playback_duration_seconds().unwrap_or(current.max(0.0));
        let next = (current + delta_seconds).clamp(0.0, duration.max(0.0));
        self.seek_current(next);
    }

    fn random_sequence_index(&self, indexes: &[usize], current_index: Option<usize>) -> Option<usize> {
        if indexes.is_empty() {
            return None;
        }

        if indexes.len() == 1 {
            return indexes.first().copied();
        }

        let candidates = indexes
            .iter()
            .copied()
            .filter(|index| Some(*index) != current_index)
            .collect::<Vec<_>>();
        let candidates = if candidates.is_empty() { indexes.to_vec() } else { candidates };
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos() as usize)
            .unwrap_or(0);
        let seed = nanos ^ self.status_updated_at.elapsed().as_nanos() as usize ^ candidates.len();
        candidates.get(seed % candidates.len()).copied()
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
        if self.maybe_start_crossfade_to_next_track() {
            return;
        }

        let finished = self
            .player
            .as_ref()
            .map(AudioPlayer::has_finished)
            .unwrap_or(false);

        if finished && self.active_track_index.is_some() {
            if self.state.playback.auto_advance || self.state.playback.repeat_mode != RepeatMode::Off {
                self.play_next_track_with_crossfade(0.0);
            } else {
                self.active_track_index = None;
                self.active_track_path = None;
                self.crossfade_started_for_path = None;
            }
        }
    }

    fn maybe_start_crossfade_to_next_track(&mut self) -> bool {
        if !(self.state.playback.auto_advance || self.state.playback.repeat_mode != RepeatMode::Off)
            || !self.state.playback.crossfade_enabled
        {
            return false;
        }

        let Some(player) = self.player.as_ref() else {
            return false;
        };
        if !player.is_playing() {
            return false;
        }

        let Some(active_path) = self.active_track_path.clone() else {
            return false;
        };
        if self
            .crossfade_started_for_path
            .as_ref()
            .map(|path| same_path(path, &active_path))
            .unwrap_or(false)
        {
            return false;
        }

        let Some(duration) = player.playback_duration_seconds() else {
            return false;
        };
        let position = player.playback_position_seconds();
        if duration <= 0.0 || position <= 0.25 {
            return false;
        }

        let requested_fade = self.state.playback.crossfade_seconds.max(1) as f32;
        let effective_fade = requested_fade.min((duration * 0.45).max(0.25));
        let remaining = duration - position;

        if remaining <= effective_fade {
            self.crossfade_started_for_path = Some(active_path);
            self.play_next_track_with_crossfade(effective_fade);
            return true;
        }

        false
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
        if let (Some(index), Some(path)) = (self.active_track_index, self.active_track_path.clone()) {
            self.state.last_played_track = Some(LastPlayedTrack {
                playlist_index: self.state.selected_playlist_index,
                track_path: path,
            });
            self.selected_track_index = Some(index);
        }
        if let Some(player) = &mut self.player {
            player.stop();
        }
        let _ = save_state(&self.state);
    }
}

impl eframe::App for AudioOrbitApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        context.set_visuals(egui::Visuals::dark());
        context.request_repaint_after(Duration::from_millis(33));
        self.remember_window_geometry(context);

        self.process_media_key_events();
        self.process_update_check_events();
        self.process_keyboard_shortcuts(context);
        self.process_radio_title_events();
        self.refresh_radio_title_periodically();
        self.process_pending_track_switch();
        self.process_pending_profile_apply();
        self.update_playback_status();
        self.poll_output_device_change();
        self.sync_status_lifetime();

        let now_playing_response = egui::TopBottomPanel::top("now_playing_panel").show(context, |ui| {
            self.render_now_playing_panel(ui);
        });
        self.handle_top_panel_volume_wheel(&now_playing_response.response, context);

        if !self.player_only_mode && self.show_library_panel {
            egui::SidePanel::left("library_panel")
                .resizable(true)
                .default_width(340.0)
                .width_range(260.0..=520.0)
                .show(context, |ui| {
                    self.render_library_panel(ui);
                });
        }

        if !self.player_only_mode && self.show_profile_panel {
            egui::SidePanel::right("profile_panel")
                .resizable(true)
                .default_width(340.0)
                .width_range(280.0..=520.0)
                .show(context, |ui| {
                    self.render_profile_panel(ui);
                });
        }

        if !self.status_message.is_empty() || self.error_message.is_some() {
            egui::TopBottomPanel::bottom("status_panel").show(context, |ui| {
                self.render_status_panel(ui);
            });
        }

        egui::CentralPanel::default().show(context, |ui| {
            self.render_main_content_panel(ui);
        });

        if self.show_folder_import_modal {
            self.render_folder_import_window(context);
        }

        if self.show_settings_modal {
            self.render_settings_modal(context);
        }

        if self.show_release_modal {
            self.render_release_modal(context);
        }
    }
}

impl AudioOrbitApp {
    fn control_label(&self, icon: Icon, text: &str) -> String {
        if self.player_only_mode {
            ui_icons::icon(icon)
        } else {
            ui_icons::label(icon, text)
        }
    }

    fn render_now_playing_panel(&mut self, ui: &mut egui::Ui) {
        ui.add_space(8.0);
        let has_now_playing = self.active_track_path.is_some()
            || self.pending_track_switch.is_some()
            || self.active_radio_index.is_some();

        ui.horizontal(|ui| {
            let controls_width = if self.player_only_mode { 92.0 } else { 240.0 };
            let title_width = (ui.available_width() - controls_width).max(140.0);
            ui.allocate_ui_with_layout(
                egui::vec2(title_width, 60.0),
                egui::Layout::top_down(egui::Align::Min),
                |ui| {
                    if has_now_playing {
                        let title = ellipsize_to_width(&self.active_track_title(), title_width - 8.0, 18.0);
                        let title_response = ui.add_sized(
                            egui::vec2(title_width, 24.0),
                            egui::Label::new(egui::RichText::new(title).size(18.0).strong()),
                        );
                        title_response.on_hover_text("Mouse wheel over the top player bar adjusts volume.");

                        if let Some(detail) = self.active_track_detail() {
                            ui.add_sized(
                                egui::vec2(title_width, 34.0),
                                egui::Label::new(egui::RichText::new(detail).size(12.0)).wrap(),
                            );
                        } else {
                            ui.add_space(34.0);
                        }
                    } else {
                        ui.add_space(58.0);
                    }
                },
            );

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let settings_label = self.control_label(Icon::Settings2, "Settings");
                if ui.button(settings_label).on_hover_text("Settings").clicked() {
                    self.show_settings_modal = true;
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
            let elapsed = self.radio_elapsed_seconds().unwrap_or(0.0);
            let response = draw_radio_visualizer(ui, elapsed, format!("Live stream · {}", format_duration(elapsed)));
            response.on_hover_text("Internet radio streams are live: this shows a live visualizer and elapsed listening time, not a seekable timeline.");
        } else if has_now_playing {
            let position = self.displayed_playback_position_seconds();
            let duration = self.displayed_playback_duration_seconds();
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
            if (response.clicked() || response.drag_stopped()) && duration > 0.0 {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let next_position = ((pointer.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0) * duration;
                    self.seek_current(next_position);
                }
            }
        } else {
            let response = draw_waveform_seek(ui, &[], 0.0, "".to_owned());
            response.on_hover_text("No track is currently playing.");
        }

        let radio_controls_active = self.active_tab == MainContentTab::Radio;
        ui.horizontal(|ui| {
            if !radio_controls_active {
                if ui
                    .add_enabled(self.player.is_some(), egui::Button::new(self.control_label(Icon::SkipBack, "Previous")))
                    .clicked()
                {
                    self.play_previous_track();
                }
            }

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

            if ui
                .add_enabled(self.player.is_some(), egui::Button::new(play_label))
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
                .add_enabled(self.player.is_some(), egui::Button::new(self.control_label(Icon::Square, "Stop")))
                .clicked()
            {
                self.stop();
            }

            if !radio_controls_active {
                if ui
                    .add_enabled(self.player.is_some(), egui::Button::new(self.control_label(Icon::SkipForward, "Next")))
                    .clicked()
                {
                    self.play_next_track();
                }

                ui.separator();
                self.render_compact_playback_toggles(ui);

                ui.separator();
            } else {
                ui.separator();
            }
            let volume_icon = if self.effective_volume_percent() == 0 { Icon::VolumeX } else { Icon::Volume2 };
            if ui
                .button(ui_icons::icon(volume_icon))
                .on_hover_text("Mute / unmute")
                .clicked()
            {
                self.toggle_mute();
            }
            let mut volume = self.state.playback.volume_percent;
            let slider_width = if self.player_only_mode { 92.0 } else { 140.0 };
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
        });

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

    fn render_compact_playback_toggles(&mut self, ui: &mut egui::Ui) {
        let mut playback_changed = false;
        let repeat_button_label = if self.player_only_mode {
            "↻".to_owned()
        } else {
            format!("↻ {}", self.state.playback.repeat_mode.label())
        };
        if ui
            .button(repeat_button_label)
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

    fn render_library_panel(&mut self, ui: &mut egui::Ui) {
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
            if ui.button(ui_icons::label(Icon::Settings2, "Settings, backups, updates...")).clicked() {
                self.show_settings_modal = true;
            }
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

        ui.horizontal(|ui| {
            let can_rescan = self
                .current_playlist()
                .and_then(|playlist| playlist.source_folder.as_ref())
                .is_some();

            if ui
                .add_enabled(can_rescan, egui::Button::new(ui_icons::label(Icon::FolderSync, "Rescan folder")))
                .clicked()
            {
                self.rescan_current_folder();
            }
        });

        ui.separator();
        if ui.button(ui_icons::label(Icon::Settings2, "Settings, backups, updates...")).clicked() {
            self.show_settings_modal = true;
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

        if groups.len() > 1 {
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

    fn render_main_content_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if ui.selectable_label(self.active_tab == MainContentTab::Music, ui_icons::label(Icon::Music, "Music")).clicked() {
                self.active_tab = MainContentTab::Music;
            }
            if ui.selectable_label(self.active_tab == MainContentTab::Radio, ui_icons::label(Icon::Radio, "Internet radio")).clicked() {
                self.active_tab = MainContentTab::Radio;
            }
        });
        ui.separator();

        match self.active_tab {
            MainContentTab::Music => self.render_track_panel(ui),
            MainContentTab::Radio => self.render_radio_panel(ui),
        }
    }

    fn render_radio_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.heading(ui_icons::label(Icon::Radio, "Internet radio"));
            ui.label(format!("{} station(s)", self.state.radio_stations.len()));
        });
        ui.add(egui::Label::new("Add a stream URL and play it directly. Radio streams ignore shuffle, repeat, auto-play next, crossfade, silence skipping, and playback transitions.").wrap());
        ui.add_space(8.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            let compact = ui.available_width() < 620.0;
            if compact {
                ui.label("Stream URL");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 22.0),
                    egui::TextEdit::singleline(&mut self.pending_radio_url).hint_text("https://..."),
                );
                ui.label("Name (optional)");
                ui.add_sized(
                    egui::vec2(ui.available_width(), 22.0),
                    egui::TextEdit::singleline(&mut self.pending_radio_name).hint_text("Read from stream if empty"),
                );
                if ui.button(ui_icons::label(Icon::Plus, "Add station")).clicked() {
                    self.add_radio_station();
                }
            } else {
                ui.horizontal(|ui| {
                    ui.label("Stream URL");
                    let url_width = (ui.available_width() - 280.0).max(240.0);
                    ui.add_sized(
                        egui::vec2(url_width, 22.0),
                        egui::TextEdit::singleline(&mut self.pending_radio_url).hint_text("https://..."),
                    );
                    ui.label("Name");
                    ui.add_sized(
                        egui::vec2(170.0, 22.0),
                        egui::TextEdit::singleline(&mut self.pending_radio_name).hint_text("optional"),
                    );
                    if ui.button(ui_icons::label(Icon::Plus, "Add")).clicked() {
                        self.add_radio_station();
                    }
                });
            }
            ui.small("If name is empty, Audio Orbit tries to read the station name from stream headers; otherwise it uses the stream host as fallback.");
        });

        ui.add_space(10.0);
        ui.horizontal(|ui| {
            ui.label(ui_icons::icon(Icon::Search));
            ui.add_sized(
                egui::vec2((ui.available_width() - 180.0).max(160.0), 22.0),
                egui::TextEdit::singleline(&mut self.radio_search_query).hint_text("Search radio stations"),
            );
            if ui
                .selectable_label(!self.radio_show_favorites_only, "All")
                .clicked()
            {
                self.radio_show_favorites_only = false;
            }
            if ui
                .selectable_label(self.radio_show_favorites_only, "Favorites")
                .clicked()
            {
                self.radio_show_favorites_only = true;
            }
        });

        if self.state.radio_stations.is_empty() {
            ui.centered_and_justified(|ui| {
                ui.label("No internet radio stations yet. Add a stream URL above.");
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

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show(ui, |ui| {
                ui.set_width(row_width);
                for (index, station) in visible_stations {
                    let selected = self.state.selected_radio_index == Some(index);
                    let active = self.active_radio_index == Some(index);
                    let station_title = if active {
                        format!("{} {}", ui_icons::icon(Icon::Play), station.name)
                    } else {
                        station.name.clone()
                    };
                    let station_info = station
                        .last_stream_title
                        .as_deref()
                        .filter(|title| !title.eq_ignore_ascii_case(&station.name))
                        .unwrap_or(station.url.as_str())
                        .to_owned();

                    ui.allocate_ui_with_layout(
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

                            let actions_width = 70.0;
                            let info_width = if row_width < 620.0 { 150.0 } else { 260.0 };
                            let title_width = (ui.available_width() - actions_width - info_width).max(110.0);
                            let (title_rect, title_response) = ui.allocate_exact_size(
                                egui::vec2(title_width, 24.0),
                                egui::Sense::click(),
                            );

                            if selected {
                                ui.painter().rect_filled(title_rect, 5.0, ui.visuals().selection.bg_fill);
                            }
                            let text_color = if selected {
                                ui.visuals().selection.stroke.color
                            } else {
                                ui.visuals().widgets.inactive.fg_stroke.color
                            };
                            let station_title = ellipsize_to_width(&station_title, title_width - 14.0, 14.0);
                            ui.painter().with_clip_rect(title_rect).text(
                                egui::pos2(title_rect.left() + 8.0, title_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                station_title,
                                egui::FontId::proportional(14.0),
                                text_color,
                            );

                            if title_response.clicked() {
                                self.state.selected_radio_index = Some(index);
                                self.save_state_silently();
                            }
                            if title_response.double_clicked() {
                                play_radio_index = Some(index);
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui.small_button(ui_icons::icon(Icon::Trash2)).on_hover_text("Remove station").clicked() {
                                    remove_radio_index = Some(index);
                                }
                                if ui.small_button(ui_icons::icon(Icon::Play)).on_hover_text("Play station").clicked() {
                                    play_radio_index = Some(index);
                                }
                                let (info_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(info_width, 20.0),
                                    egui::Sense::hover(),
                                );
                                let station_info = ellipsize_to_width(&station_info, info_width - 8.0, 12.0);
                                ui.painter().with_clip_rect(info_rect).text(
                                    egui::pos2(info_rect.right() - 4.0, info_rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    station_info,
                                    egui::FontId::proportional(12.0),
                                    ui.visuals().widgets.inactive.fg_stroke.color,
                                );
                            });
                        },
                    );
                    ui.separator();
                }
            });

        if let Some(index) = favorite_toggle_index {
            if let Some(station) = self.state.radio_stations.get_mut(index) {
                station.favorite = !station.favorite;
                self.state.selected_radio_index = Some(index);
                self.save_state_silently();
            }
        }
        if let Some(index) = play_radio_index {
            self.state.selected_radio_index = Some(index);
            self.play_radio_station(index);
        }
        if let Some(index) = remove_radio_index {
            self.state.selected_radio_index = Some(index);
            self.remove_selected_radio_station();
        }
    }

    fn render_track_panel(&mut self, ui: &mut egui::Ui) {
        let Some(playlist) = self.current_playlist() else {
            ui.heading("No playlist");
            return;
        };

        let playlist_name = playlist.name.clone();
        let selected_playlist_label = format!("{} {}", playlist.kind.icon(), playlist_name);
        let selected_group_label = playlist.selected_group_label();
        let total_count = playlist.tracks.len();
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
            egui::ComboBox::from_id_salt("track_panel_playlist_selector")
                .selected_text(selected_playlist_label)
                .width(260.0)
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
            let group_summary = if folder_group_count > 1 {
                format!(" · {selected_group_label}")
            } else {
                String::new()
            };
            ui.label(format!("{visible_count}/{total_count} tracks{group_summary}"));

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let search_label = if self.show_track_search {
                    self.control_label(Icon::X, "Close search")
                } else {
                    self.control_label(Icon::Search, "Search")
                };
                if ui.button(search_label).clicked() {
                    self.show_track_search = !self.show_track_search;
                    self.state.ui.show_track_search = self.show_track_search;
                    if !self.show_track_search {
                        self.track_search_query.clear();
                        self.search_cursor = 0;
                    }
                    self.save_state_silently();
                }
            });
        });

        if self.show_track_search {
            ui.horizontal(|ui| {
                ui.label(ui_icons::icon(Icon::Search));
                let response = ui.text_edit_singleline(&mut self.track_search_query);
                if response.changed() {
                    self.search_cursor = 0;
                }

                let can_jump = !visible_indexes.is_empty() && !self.track_search_query.trim().is_empty();
                if ui
                    .add_enabled(can_jump, egui::Button::new(ui_icons::label(Icon::ArrowDown, "Next result")))
                    .clicked()
                {
                    let next = visible_indexes[self.search_cursor % visible_indexes.len()];
                    self.selected_track_index = Some(next);
                    self.search_cursor = (self.search_cursor + 1) % visible_indexes.len().max(1);
                }

                if ui.button(ui_icons::label(Icon::X, "Clear")).clicked() {
                    self.track_search_query.clear();
                    self.search_cursor = 0;
                }
            });
            if !query.is_empty() {
                ui.small(format!("Filtering tracks by: {query}"));
            }
        }

        if self.state.playback.repeat_mode == RepeatMode::Selection {
            let repeat_order = if self.state.playback.shuffle_enabled { "random playback" } else { "playlist order" };
            ui.small(format!("Repeat selection mode: tick the tracks that should repeat in {repeat_order}."));
        }

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
        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show(ui, |ui| {
                ui.set_width(row_width);
                for (index, track) in visible_tracks {
                    if show_group_headers && track.group != last_group {
                        ui.add_space(6.0);
                        let group = track.group.clone();
                        let collapsed = self.collapsed_groups.contains(&group);
                        ui.horizontal(|ui| {
                            let icon = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
                            if ui.small_button(ui_icons::icon(icon)).on_hover_text("Collapse/expand folder").clicked() {
                                if collapsed {
                                    self.collapsed_groups.remove(&group);
                                } else {
                                    self.collapsed_groups.insert(group.clone());
                                }
                            }
                            ui.heading(group.as_str());
                        });
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
                    let metadata = format_track_metadata_compact(&track);
                    let title = if is_active {
                        format!("{} {}", ui_icons::icon(Icon::Play), track.title)
                    } else {
                        track.title.clone()
                    };
                    let path = track.path.clone();
                    let repeat_selection_mode = self.state.playback.repeat_mode == RepeatMode::Selection;

                    ui.allocate_ui_with_layout(
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

                            let metadata_width = ((metadata.chars().count() as f32 * 6.2) + 16.0)
                                .clamp(150.0, (row_width * 0.48).max(150.0));
                            let right_reserved_width = metadata_width + 38.0;
                            let title_width = (ui.available_width() - right_reserved_width).max(96.0);
                            let (title_rect, response) = ui.allocate_exact_size(
                                egui::vec2(title_width, 24.0),
                                egui::Sense::click(),
                            );

                            if is_selected {
                                ui.painter().rect_filled(title_rect, 5.0, ui.visuals().selection.bg_fill);
                            }
                            let text_color = if is_selected {
                                ui.visuals().selection.stroke.color
                            } else {
                                ui.visuals().widgets.inactive.fg_stroke.color
                            };
                            let title = ellipsize_to_width(&title, title_width - 14.0, 14.0);
                            ui.painter().with_clip_rect(title_rect).text(
                                egui::pos2(title_rect.left() + 8.0, title_rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                title,
                                egui::FontId::proportional(14.0),
                                text_color,
                            );

                            if response.clicked() {
                                self.selected_track_index = Some(index);
                            }
                            if response.double_clicked() {
                                self.selected_track_index = Some(index);
                                self.play_path(path.clone(), Some(index), 0.0);
                            }

                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                ui.menu_button(ui_icons::icon(Icon::Ellipsis), |ui| {
                                    if ui.button(ui_icons::label(Icon::Play, "Play now")).clicked() {
                                        self.selected_track_index = Some(index);
                                        self.play_path(path.clone(), Some(index), 0.0);
                                        ui.close_menu();
                                    }
                                    if ui.button(ui_icons::label(Icon::ExternalLink, "Show in File Explorer")).clicked() {
                                        self.reveal_track_in_file_manager(path.clone());
                                        ui.close_menu();
                                    }

                                    ui.menu_button(ui_icons::label(Icon::ListPlus, "Add to playlist"), |ui| {
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
                                    if ui.add_enabled(can_remove_from_playlist, egui::Button::new(ui_icons::label(Icon::ListMinus, "Remove from playlist"))).clicked() {
                                        self.remove_track_from_current_playlist(index);
                                        ui.close_menu();
                                    }
                                    if ui.button(ui_icons::label(Icon::Trash2, "Delete from disk")).clicked() {
                                        self.delete_track_from_disk(path.clone());
                                        ui.close_menu();
                                    }
                                });
                                let (metadata_rect, _) = ui.allocate_exact_size(
                                    egui::vec2(metadata_width, 20.0),
                                    egui::Sense::hover(),
                                );
                                ui.painter().with_clip_rect(metadata_rect).text(
                                    egui::pos2(metadata_rect.right() - 4.0, metadata_rect.center().y),
                                    egui::Align2::RIGHT_CENTER,
                                    metadata,
                                    egui::FontId::proportional(12.0),
                                    ui.visuals().widgets.inactive.fg_stroke.color,
                                );
                            });
                        },
                    );
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
    }

    fn render_profile_transition_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Playback transitions");
        ui.small("Crossfade and silence skipping are kept near the active sound profile because they affect how this profile feels during playback.");

        let mut playback_changed = false;
        playback_changed |= ui
            .checkbox(&mut self.state.playback.crossfade_enabled, "Crossfade tracks")
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
                .checkbox(&mut profile.settings.skip_silence_enabled, "Skip long silence")
                .changed();
            if profile.settings.skip_silence_enabled {
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.silence_threshold_seconds, 1u8..=12u8)
                            .text("Minimum silence length (sec)"),
                    )
                    .changed();
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.silence_level_threshold_percent, 1u8..=20u8)
                            .text("Silence level threshold (%)"),
                    )
                    .on_hover_text("Audio below this level is treated as silence. Useful when the file has noise floor instead of absolute zero.")
                    .changed();
            }
        }
        if profile_changed {
            self.schedule_current_profile_apply();
        }
    }


    fn responsive_modal_size(&self, context: &egui::Context, max_width: f32, max_height: f32) -> egui::Vec2 {
        let screen_rect = context.screen_rect();
        let horizontal_margin = if screen_rect.width() < 520.0 { 12.0 } else { 32.0 };
        let vertical_margin = if screen_rect.height() < 420.0 { 12.0 } else { 48.0 };
        let available_width = (screen_rect.width() - horizontal_margin).max(260.0);
        let available_height = (screen_rect.height() - vertical_margin).max(190.0);

        egui::vec2(available_width.min(max_width), available_height.min(max_height))
    }

    fn render_modal_backdrop(&self, context: &egui::Context, id: &'static str) {
        let screen_rect = context.screen_rect();
        let painter = context.layer_painter(egui::LayerId::new(
            egui::Order::Middle,
            egui::Id::new(id),
        ));
        painter.rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(215));
    }

    fn render_release_modal(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "release_modal_backdrop");
        let mut is_open = self.show_release_modal;
        let modal_size = self.responsive_modal_size(context, 680.0, 560.0);
        let scroll_height = (modal_size.y - 118.0).max(160.0);

        egui::Area::new(egui::Id::new("release_modal"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(context, |ui| {
                egui::Frame::window(ui.style()).show(ui, |ui| {
                    ui.set_min_size(modal_size);
                    ui.set_max_width(modal_size.x);

                    ui.horizontal(|ui| {
                        ui.heading("Release watcher");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(ui_icons::icon(Icon::X)).on_hover_text("Close").clicked() {
                                is_open = false;
                            }
                        });
                    });
                    ui.add(egui::Label::new("Checks GitHub releases with a strict per-session limit to avoid API rate limiting.").wrap());
                    ui.add_space(10.0);

                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.render_update_settings_section(ui, false);
                            });
                        });

                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(ui_icons::label(Icon::X, "Close")).clicked() {
                            is_open = false;
                        }
                    });
                });
            });

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            is_open = false;
        }

        self.show_release_modal = is_open;
    }

    fn render_settings_modal(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "settings_modal_backdrop");
        let mut is_open = self.show_settings_modal;
        let modal_size = self.responsive_modal_size(context, 900.0, 760.0);
        let scroll_height = (modal_size.y - 124.0).max(180.0);

        egui::Area::new(egui::Id::new("settings_modal"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(context, |ui| {
                egui::Frame::window(ui.style()).show(ui, |ui| {
                    ui.set_min_size(modal_size);
                    ui.set_max_width(modal_size.x);

                    ui.horizontal(|ui| {
                        ui.heading("Settings");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(ui_icons::icon(Icon::X)).on_hover_text("Close").clicked() {
                                is_open = false;
                            }
                        });
                    });
                    ui.add(egui::Label::new("Playback, sound profiles, backups, updates, and portable data.").wrap());
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.render_playback_settings_section(ui);
                            });
                            ui.add_space(12.0);

                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.render_profile_panel(ui);
                            });
                            ui.add_space(12.0);

                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.render_backup_settings_section(ui);
                            });
                            ui.add_space(12.0);

                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.render_update_settings_section(ui, true);
                            });
                            ui.add_space(12.0);

                            egui::Frame::group(ui.style()).show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                self.render_about_section(ui);
                            });
                        });

                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(ui_icons::label(Icon::X, "Close")).clicked() {
                            is_open = false;
                        }
                    });
                });
            });

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            is_open = false;
        }

        self.show_settings_modal = is_open;
    }

    fn render_playback_settings_section(&mut self, ui: &mut egui::Ui) {
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
            .checkbox(&mut self.state.playback.crossfade_enabled, "Crossfade tracks")
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
                .checkbox(&mut profile.settings.skip_silence_enabled, "Skip long silence")
                .changed();
            if profile.settings.skip_silence_enabled {
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.silence_threshold_seconds, 1u8..=12u8)
                            .text("Silence threshold seconds"),
                    )
                    .changed();
                profile_changed |= ui
                    .add(
                        egui::Slider::new(&mut profile.settings.silence_level_threshold_percent, 1u8..=20u8)
                            .text("Silence level threshold (%)"),
                    )
                    .on_hover_text("Audio below this level is treated as silence, so low noise floors can be skipped too.")
                    .changed();
            }
        }
        if profile_changed {
            self.schedule_current_profile_apply();
        }
    }

    fn render_backup_settings_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Backup and data");
        ui.small("The ZIP backup stores the full app state: music folders, playlists, Favorites, sound profiles, playback settings, and update settings.");

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

    fn render_update_settings_section(&mut self, ui: &mut egui::Ui, show_modal_button: bool) {
        ui.heading("Updates");
        let prerelease_changed = ui
            .checkbox(
                &mut self.state.update_settings.include_prereleases,
                "Also watch prereleases",
            )
            .changed();
        if prerelease_changed {
            self.save_state_silently();
        }

        ui.small(if self.state.update_settings.include_prereleases {
            "Mode: stable releases and prereleases."
        } else {
            "Mode: latest stable release only."
        });

        ui.horizontal_wrapped(|ui| {
            let check_status = if self.update_check_receiver.is_some() {
                format!("Checks used: {}/{} · checking...", self.update_check_count, MAX_UPDATE_CHECKS_PER_SESSION)
            } else {
                format!("Checks used: {}/{}", self.update_check_count, MAX_UPDATE_CHECKS_PER_SESSION)
            };
            ui.label(check_status);

            let can_check = self.update_check_count < MAX_UPDATE_CHECKS_PER_SESSION && self.update_check_receiver.is_none();
            if ui
                .add_enabled(can_check, egui::Button::new(ui_icons::label(Icon::Search, "Check releases")))
                .clicked()
            {
                self.check_for_updates();
            }

            if show_modal_button && ui.button(ui_icons::label(Icon::Bell, "Open release watcher modal")).clicked() {
                self.show_release_modal = true;
            }

            if ui.button(ui_icons::label(Icon::ExternalLink, "Open releases" )).clicked() {
                if let Err(error) = updater::open_releases_page() {
                    self.error_message = Some(error.to_string());
                }
            }
        });

        if self.update_check_count >= MAX_UPDATE_CHECKS_PER_SESSION {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Check limit reached. Restart the app before checking again.",
            );
        }

        if let Some(check) = self.last_update_check.clone() {
            ui.label(format!("Current version: v{}", check.current_version));
            ui.label(format!(
                "Latest version: v{}{}",
                check.latest_version,
                if check.prerelease { " prerelease" } else { "" }
            ));

            if check.is_update_available {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "A newer executable is available.");
            } else {
                ui.colored_label(egui::Color32::LIGHT_GREEN, "Latest is OK. No update is required.");
            }

            if let Some(asset_name) = &check.asset_name {
                ui.small(format!("Asset: {asset_name}"));
            } else {
                ui.small("No Windows executable asset was found on the selected release.");
            }

            let can_install = check.is_update_available && check.asset_download_url.is_some();
            if ui
                .add_enabled(can_install, egui::Button::new(ui_icons::label(Icon::Download, "Replace current executable")))
                .clicked()
            {
                self.install_update();
            }
        } else {
            ui.small(format!("Repository: {}", updater::repository_label()));
            ui.small("No release check has been run in this app session.");
        }
    }

    fn render_about_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("About Audio Orbit");
        ui.add(egui::Label::new("Audio Orbit is a lightweight Windows music player focused on local libraries, folder-based playlists, smooth crossfade playback, silence skipping, and headphone-friendly orbit-style stereo movement.").wrap());
        ui.add_space(8.0);
        ui.add(egui::Label::new(format!("Version: v{}", env!("CARGO_PKG_VERSION"))).wrap());
        ui.add(egui::Label::new("Creator: Zoltán Rózsa").wrap());
        ui.add(egui::Label::new("License: GNU Affero General Public License v3.0 (AGPL-3.0)").wrap());
        ui.add(egui::Label::new("This app stores its portable state next to the executable in .audio-orbit-data.").wrap());

        ui.add_space(10.0);
        ui.heading("Keyboard shortcuts");
        ui.small("AIMP-style in-app controls are available while no text field is focused.");
        ui.label("Space — Play / pause");
        ui.label("Enter — Play selected track");
        ui.label("S — Stop");
        ui.label("Left / Right — Seek 10 seconds backward / forward");
        ui.label("Ctrl + Left / Ctrl + Right — Previous / next track");
        ui.label("Ctrl + F — Show or hide track search");
        ui.label("M — Player-only / full layout");
        ui.label("Ctrl + L — Show or hide Library panel");
        ui.label("Ctrl + P — Show or hide Sound profiles panel");
    }

    fn render_status_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if !self.status_message.is_empty() {
                ui.label(self.status_message.as_str());
                ui.separator();
            }
            ui.small(self.media_key_status.as_str());

            if let Some(error_message) = &self.error_message {
                ui.separator();
                ui.colored_label(egui::Color32::RED, error_message);
            }
        });
    }

    fn render_folder_import_window(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "folder_import_modal_backdrop");
        let mut is_open = self.show_folder_import_modal;
        let mut close_after_import = false;
        let modal_size = self.responsive_modal_size(context, 640.0, 520.0);
        let scroll_height = (modal_size.y - 112.0).max(160.0);

        egui::Area::new(egui::Id::new("folder_import_modal"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(context, |ui| {
                egui::Frame::window(ui.style()).show(ui, |ui| {
                    ui.set_min_size(modal_size);
                    ui.set_max_width(modal_size.x);

                    ui.horizontal(|ui| {
                        ui.heading("Add music folder");
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(ui_icons::icon(Icon::X)).on_hover_text("Close").clicked() {
                                close_after_import = true;
                            }
                        });
                    });

                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.add(egui::Label::new("Create a scanner-owned playlist from a folder and group tracks by the first N subfolder levels.").wrap());
                            ui.add_space(8.0);

                            ui.label("Folder");
                            let folder_label = self
                                .pending_folder_path
                                .as_ref()
                                .map(|path| path.display().to_string())
                                .unwrap_or_else(|| "No folder selected".to_owned());
                            ui.add(egui::Label::new(folder_label).wrap());

                            if ui.button(ui_icons::label(Icon::FolderOpen, "Choose folder...")).clicked() {
                                self.pick_music_folder();
                            }

                            ui.add_space(8.0);
                            ui.label("Playlist name");
                            ui.text_edit_singleline(&mut self.pending_playlist_name);

                            ui.add(
                                egui::Slider::new(&mut self.pending_folder_depth, 0usize..=5usize)
                                    .text("Group by folder levels"),
                            );
                            ui.add(egui::Label::new("Example: depth 2 groups D:\\mp3\\Artist\\Album\\song.mp3 as Artist / Album.").wrap());
                        });

                    ui.separator();
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(ui_icons::label(Icon::X, "Cancel")).clicked() {
                            close_after_import = true;
                        }
                        if ui.button(ui_icons::label(Icon::FolderInput, "Import folder")).clicked() {
                            close_after_import = self.import_folder_playlist();
                        }
                    });
                });
            });

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            close_after_import = true;
        }

        if close_after_import {
            is_open = false;
            self.pending_folder_path = None;
        }

        self.show_folder_import_modal = is_open;
    }

}


fn current_unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

fn media_key_status_message(
    registered: &[media_keys::MediaKeyCommand],
    failed: &[media_keys::MediaKeyCommand],
) -> String {
    if registered.is_empty() {
        return "Media keys: unavailable".to_owned();
    }

    if failed.is_empty() {
        return "Media keys: enabled".to_owned();
    }

    let failed_labels = failed
        .iter()
        .map(|command| command.label())
        .collect::<Vec<_>>()
        .join(", ");

    format!("Media keys: partially enabled; unavailable: {failed_labels}")
}

fn fetch_radio_stream_title(url: &str) -> Option<String> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Audio-Orbit-Radio-Metadata")
        .timeout(Duration::from_secs(5))
        .build()
        .ok()?;

    let response = client
        .get(url)
        .header("Icy-MetaData", "1")
        .send()
        .ok()?;

    let headers = response.headers();
    let title = headers
        .get("icy-name")
        .or_else(|| headers.get("icy-description"))
        .or_else(|| headers.get("x-audiocast-name"))
        .and_then(|value| value.to_str().ok())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    title
}


fn ellipsize_to_width(value: &str, width: f32, font_size: f32) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    let average_char_width = (font_size * 0.56).max(5.0);
    let max_chars = (width.max(24.0) / average_char_width).floor() as usize;
    ellipsize_chars(trimmed, max_chars)
}

fn ellipsize_chars(value: &str, max_chars: usize) -> String {
    let char_count = value.chars().count();
    if char_count <= max_chars {
        return value.to_owned();
    }
    if max_chars <= 1 {
        return "…".to_owned();
    }
    if max_chars <= 4 {
        return format!("{}…", value.chars().take(max_chars - 1).collect::<String>());
    }

    format!("{}…", value.chars().take(max_chars - 1).collect::<String>())
}

fn fallback_radio_station_name(url: &str) -> String {
    let without_scheme = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"))
        .unwrap_or(url);
    let host = without_scheme
        .split('/')
        .next()
        .unwrap_or("Internet radio")
        .trim();

    if host.is_empty() {
        "Internet radio".to_owned()
    } else {
        host.to_owned()
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

    if let Some(index) = state.selected_radio_index {
        if index >= state.radio_stations.len() {
            state.selected_radio_index = None;
        }
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


fn draw_radio_visualizer(ui: &mut egui::Ui, elapsed_seconds: f32, label: String) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), 46.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let visuals = ui.visuals();
    let painter = ui.painter();

    painter.rect_filled(rect, 8.0, egui::Color32::from_black_alpha(210));

    let bar_count = (rect.width() / 5.2).round().clamp(36.0, 180.0) as usize;
    let gap = 1.2;
    let bar_width = ((rect.width() - gap * bar_count.saturating_sub(1) as f32) / bar_count.max(1) as f32)
        .clamp(1.2, 4.0);
    let phase = elapsed_seconds.max(0.0);

    for index in 0..bar_count {
        let x1 = rect.left() + index as f32 * (bar_width + gap);
        let x2 = (x1 + bar_width).min(rect.right());
        if x1 >= rect.right() {
            break;
        }

        let wave_a = ((index as f32 * 0.33) + phase * 3.4).sin().abs();
        let wave_b = ((index as f32 * 0.13) - phase * 1.7).cos().abs();
        let wave_c = ((index as f32 * 0.07) + phase * 0.9).sin().abs();
        let level = (0.18 + wave_a * 0.45 + wave_b * 0.26 + wave_c * 0.18).clamp(0.12, 1.0);
        let height = (rect.height() * 0.78 * level).max(4.0);
        let y1 = rect.center().y - height / 2.0;
        let y2 = rect.center().y + height / 2.0;
        let color = if index % 4 == 0 {
            visuals.selection.bg_fill
        } else {
            visuals.widgets.inactive.fg_stroke.color.linear_multiply(0.75)
        };
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x1, y1), egui::pos2(x2, y2)),
            bar_width / 2.0,
            color,
        );
    }

    paint_seek_label(painter, rect, &label);
    response
}

fn draw_waveform_seek(ui: &mut egui::Ui, waveform: &[f32], progress: f32, label: String) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), 46.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let visuals = ui.visuals();
    let painter = ui.painter();

    painter.rect_filled(rect, 8.0, egui::Color32::from_black_alpha(210));

    if waveform.is_empty() {
        paint_seek_label(painter, rect, &label);
        return response;
    }

    let progress = progress.clamp(0.0, 1.0);
    let progress_x = rect.left() + rect.width() * progress;
    let target_points = (rect.width() / 1.65).round().clamp(140.0, 2200.0) as usize;
    let step = (waveform.len() as f32 / target_points.max(1) as f32).ceil().max(1.0) as usize;
    let rendered_points = (waveform.len() + step - 1) / step;
    let gap = 0.85;
    let bar_width = ((rect.width() - gap * rendered_points.saturating_sub(1) as f32) / rendered_points.max(1) as f32)
        .clamp(0.75, 2.2);
    let peak = waveform
        .iter()
        .copied()
        .fold(0.0_f32, f32::max)
        .max(0.08);
    let mut sorted = waveform.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let floor_index = ((sorted.len().saturating_sub(1)) as f32 * 0.18) as usize;
    let noise_floor = sorted.get(floor_index).copied().unwrap_or(0.0).min(peak * 0.65);
    let dynamic_range = (peak - noise_floor).max(0.04);

    for (bar_index, chunk) in waveform.chunks(step).enumerate() {
        let value = chunk
            .iter()
            .copied()
            .fold(0.0_f32, f32::max);
        let normalized = ((value - noise_floor) / dynamic_range).clamp(0.025, 1.0);
        let eased = normalized.powf(0.72);
        let x1 = rect.left() + bar_index as f32 * (bar_width + gap);
        let x2 = (x1 + bar_width).min(rect.right());
        if x1 >= rect.right() {
            break;
        }

        let height = (rect.height() * 0.82 * eased).max(4.0);
        let y1 = rect.center().y - height / 2.0;
        let y2 = rect.center().y + height / 2.0;
        let color = if x1 <= progress_x {
            visuals.selection.bg_fill
        } else {
            visuals.widgets.inactive.fg_stroke.color.linear_multiply(0.72)
        };
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x1, y1), egui::pos2(x2, y2)),
            bar_width / 2.0,
            color,
        );
    }

    let playhead = egui::Rect::from_min_max(
        egui::pos2(progress_x - 1.0, rect.top() + 4.0),
        egui::pos2(progress_x + 1.0, rect.bottom() - 4.0),
    );
    painter.rect_filled(playhead, 1.0, egui::Color32::WHITE.linear_multiply(0.85));

    paint_seek_label(painter, rect, &label);
    response
}

fn paint_seek_label(painter: &egui::Painter, rect: egui::Rect, label: &str) {
    let font = egui::FontId::proportional(14.0);
    let label_width = (label.chars().count() as f32 * 8.0 + 22.0).clamp(88.0, 190.0);
    let label_rect = egui::Rect::from_center_size(rect.center(), egui::vec2(label_width, 24.0));
    painter.rect_filled(label_rect, 6.0, egui::Color32::from_black_alpha(185));
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        label,
        font,
        egui::Color32::WHITE,
    );
}


fn format_track_metadata_compact(track: &Track) -> String {
    let sample_rate = track
        .metadata
        .sample_rate_hz
        .map(|value| format!("{}k", value / 1000))
        .unwrap_or_else(|| "?k".to_owned());
    let bitrate = track
        .metadata
        .bitrate_kbps
        .map(|value| format!("{value}k"))
        .unwrap_or_else(|| "?k".to_owned());
    let channels = track
        .metadata
        .channels
        .map(|value| format!("{value}ch"))
        .unwrap_or_else(|| "?ch".to_owned());
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
        Command::new("explorer.exe")
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
