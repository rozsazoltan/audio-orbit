#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_player;
mod config;
mod dsp;
mod icon;
mod media_keys;
mod recognition;
mod single_instance;
mod spectrum_waveform;
mod ui_icons;
mod updater;

use crate::{
    audio_player::{current_default_output_device_name, AudioPlayer, PlaybackInfo, PreparedPlayback, RadioVisualizerFrame},
    config::{
        app_data_dir, collect_audio_files_from_folder, display_file_name, export_state_zip, external_tools_dir,
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
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::mpsc,
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const UPDATE_CHECKS_BEFORE_CONFIRMATION: u8 = 2;
const AUTOMATIC_UPDATE_CHECK_INTERVAL_SECONDS: u64 = 60 * 60;
const AUTOMATIC_SONGREC_CHECK_INTERVAL_SECONDS: u64 = 24 * 60 * 60;

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

fn recognize_audio_sample_with_songrec(
    sample: audio_player::RecognitionAudioSample,
    command: Option<PathBuf>,
) -> anyhow::Result<recognition::RecognitionResult> {
    let sample_path = recognition::temporary_sample_path();
    sample.write_wav(&sample_path)?;
    recognition::ensure_sample_exists(&sample_path)?;
    let result = recognition::recognize_with_songrec(command, &sample_path);
    recognition::cleanup_sample(&sample_path);
    result
}


#[derive(Clone, Debug)]
struct PendingTrackSwitch {
    switch_at: Instant,
    started_at: Instant,
    previous_position: f32,
    previous_duration: f32,
    playlist_index: usize,
    index: Option<usize>,
    info: PlaybackInfo,
}

struct PreparedTrackPlayback {
    playlist_index: usize,
    index: Option<usize>,
    crossfade_seconds: f32,
    live_position_compensation: bool,
    previous_position: Option<f32>,
    previous_duration: Option<f32>,
    prepared: PreparedPlayback,
    requested_at: Instant,
}

#[derive(Clone, Debug)]
enum DetailsModal {
    Track(PathBuf),
    Radio(usize),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MainContentTab {
    Music,
    Radio,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AppPanelModal {
    Settings,
    Updates,
    Backup,
    About,
}

impl AppPanelModal {
    fn title(self) -> &'static str {
        match self {
            Self::Settings => "Settings",
            Self::Updates => "Updates",
            Self::Backup => "Backup",
            Self::About => "About",
        }
    }

    fn description(self) -> &'static str {
        match self {
            Self::Settings => "Playback, profiles, shortcuts, and links to the app panels.",
            Self::Updates => "Check, review, and install GitHub release updates.",
            Self::Backup => "Export and import the complete Audio Orbit state, including folders, playlists, radio stations, profiles, playback, and UI settings.",
            Self::About => "Purpose, licensing, author information, and app shortcuts.",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Settings => Icon::Settings2,
            Self::Updates => Icon::Download,
            Self::Backup => Icon::Archive,
            Self::About => Icon::Info,
        }
    }
}

#[derive(Clone, Debug, Default)]
struct RadioStreamMetadata {
    station_name: Option<String>,
    stream_title: Option<String>,
}

struct AudioOrbitApp {
    player: Option<AudioPlayer>,
    state: SavedState,
    selected_track_index: Option<usize>,
    selected_track_indexes: BTreeSet<usize>,
    active_track_index: Option<usize>,
    active_playlist_index: Option<usize>,
    active_track_path: Option<PathBuf>,
    last_playback: Option<PlaybackInfo>,
    status_message: String,
    status_last_seen: String,
    status_updated_at: Instant,
    error_message: Option<String>,
    error_last_seen: Option<String>,
    error_updated_at: Instant,
    crossfade_started_for_path: Option<PathBuf>,
    pending_track_switch: Option<PendingTrackSwitch>,
    pending_prepared_track_receiver: Option<mpsc::Receiver<Result<PreparedTrackPlayback, String>>>,
    pending_profile_apply_at: Option<Instant>,
    suppress_window_geometry_save_until: Option<Instant>,
    show_folder_import_modal: bool,
    show_radio_add_modal: bool,
    active_panel_modal: Option<AppPanelModal>,
    panel_modal_history: Vec<AppPanelModal>,
    details_modal: Option<DetailsModal>,
    show_library_panel: bool,
    show_profile_panel: bool,
    player_only_mode: bool,
    show_track_search: bool,
    show_radio_search: bool,
    search_playback_filtered_only: bool,
    focus_track_search: bool,
    focus_radio_search: bool,
    scroll_to_active_track_requested: bool,
    scroll_to_active_radio_requested: bool,
    scroll_to_folder_group_requested: Option<String>,
    active_tab: MainContentTab,
    track_search_query: String,
    search_cursor: usize,
    pending_radio_name: String,
    pending_radio_url: String,
    radio_search_query: String,
    radio_show_favorites_only: bool,
    active_radio_index: Option<usize>,
    radio_selection_was_user_set: bool,
    active_radio_station_name: Option<String>,
    active_radio_title: Option<String>,
    radio_started_at: Option<Instant>,
    last_radio_title_lookup_at: Option<Instant>,
    radio_title_receiver: Option<mpsc::Receiver<(usize, Option<RadioStreamMetadata>)>>,
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
    show_update_check_confirmation: bool,
    last_songrec_tool_status: Option<recognition::SongRecToolStatus>,
    songrec_tool_receiver: Option<mpsc::Receiver<Result<recognition::SongRecToolStatus, String>>>,
    songrec_install_receiver: Option<mpsc::Receiver<Result<recognition::InstalledSongRec, String>>>,
    recognition_receiver: Option<mpsc::Receiver<Result<recognition::RecognitionResult, String>>>,
    recognition_started_at: Option<Instant>,
    pending_clipboard_text: Option<String>,
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
        let search_playback_filtered_only = state.ui.search_playback_filtered_only;

        let mut app = match AudioPlayer::new() {
            Ok(player) => {
                let output_name = player.output_device_name().to_owned();
                Self {
                    player: Some(player),
                    state,
                    selected_track_index: None,
                    selected_track_indexes: BTreeSet::new(),
                    active_track_index: None,
                    active_playlist_index: None,
                    active_track_path: None,
                    last_playback: None,
                    status_message: format!("Ready. Output device: {output_name}"),
                    status_last_seen: String::new(),
                    status_updated_at: Instant::now(),
                    error_message: None,
                    error_last_seen: None,
                    error_updated_at: Instant::now(),
                    crossfade_started_for_path: None,
                    pending_track_switch: None,
                    pending_prepared_track_receiver: None,
                    pending_profile_apply_at: None,
                    suppress_window_geometry_save_until: None,
                    show_folder_import_modal: false,
                    show_radio_add_modal: false,
                    active_panel_modal: None,
                    panel_modal_history: Vec::new(),
                    details_modal: None,
                    show_library_panel,
                    show_profile_panel,
                    player_only_mode,
                    show_track_search,
                    show_radio_search: false,
                    search_playback_filtered_only,
                    focus_track_search: false,
                    focus_radio_search: false,
                    scroll_to_active_track_requested: false,
                    scroll_to_active_radio_requested: false,
                    scroll_to_folder_group_requested: None,
                    active_tab: MainContentTab::Music,
                    track_search_query: String::new(),
                    search_cursor: 0,
                    pending_radio_name: String::new(),
                    pending_radio_url: String::new(),
                    radio_search_query: String::new(),
                    radio_show_favorites_only: false,
                    active_radio_index: None,
                    radio_selection_was_user_set: false,
                    active_radio_station_name: None,
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
                    show_update_check_confirmation: false,
                    last_songrec_tool_status: None,
                    songrec_tool_receiver: None,
                    songrec_install_receiver: None,
                    recognition_receiver: None,
                    recognition_started_at: None,
                    pending_clipboard_text: None,
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
                active_playlist_index: None,
                active_track_path: None,
                last_playback: None,
                status_message: "No audio output device is available.".to_owned(),
                status_last_seen: String::new(),
                status_updated_at: Instant::now(),
                error_message: Some(error.to_string()),
                error_last_seen: Some(error.to_string()),
                error_updated_at: Instant::now(),
                crossfade_started_for_path: None,
                pending_track_switch: None,
                pending_prepared_track_receiver: None,
                pending_profile_apply_at: None,
                suppress_window_geometry_save_until: None,
                show_folder_import_modal: false,
                show_radio_add_modal: false,
                active_panel_modal: None,
                panel_modal_history: Vec::new(),
                details_modal: None,
                show_library_panel,
                show_profile_panel,
                player_only_mode,
                show_track_search,
                show_radio_search: false,
                search_playback_filtered_only,
                focus_track_search: false,
                focus_radio_search: false,
                scroll_to_active_track_requested: false,
                scroll_to_active_radio_requested: false,
                scroll_to_folder_group_requested: None,
                active_tab: MainContentTab::Music,
                track_search_query: String::new(),
                search_cursor: 0,
                pending_radio_name: String::new(),
                pending_radio_url: String::new(),
                radio_search_query: String::new(),
                radio_show_favorites_only: false,
                active_radio_index: None,
                radio_selection_was_user_set: false,
                active_radio_station_name: None,
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
                show_update_check_confirmation: false,
                last_songrec_tool_status: None,
                songrec_tool_receiver: None,
                songrec_install_receiver: None,
                recognition_receiver: None,
                recognition_started_at: None,
                pending_clipboard_text: None,
                media_key_receiver: None,
                media_key_status: "Media keys: unavailable".to_owned(),
            },
        };

        let initial_volume_percent = app.effective_volume_percent();
        if let Some(player) = &mut app.player {
            player.set_volume_percent(initial_volume_percent);
        }

        let media_keys = media_keys::start_listener();
        app.media_key_receiver = media_keys.receiver;
        app.media_key_status = media_keys.status_message;
        app.restore_last_played_track_selection();
        app.state.selected_radio_index = None;
        app.radio_selection_was_user_set = false;
        app.start_automatic_update_check_if_due();
        app.start_automatic_songrec_check_if_due();
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


    fn open_panel_modal(&mut self, panel: AppPanelModal) {
        if self.active_panel_modal == Some(panel) {
            return;
        }

        if let Some(current) = self.active_panel_modal {
            self.panel_modal_history.push(current);
        }
        self.active_panel_modal = Some(panel);
    }

    fn close_panel_modal(&mut self) {
        self.active_panel_modal = self.panel_modal_history.pop();
    }


    fn close_track_search(&mut self) {
        if self.show_track_search {
            self.show_track_search = false;
            self.state.ui.show_track_search = false;
            self.track_search_query.clear();
            self.search_cursor = 0;
            self.save_state_silently();
        }
    }

    fn close_radio_search(&mut self) {
        if self.show_radio_search {
            self.show_radio_search = false;
            self.radio_search_query.clear();
        }
    }

    fn process_escape_navigation(&mut self, context: &egui::Context) {
        if !context.input(|input| input.key_pressed(egui::Key::Escape)) {
            return;
        }

        if self.show_update_check_confirmation {
            self.show_update_check_confirmation = false;
        } else if self.active_panel_modal.is_some() {
            self.close_panel_modal();
        } else if self.show_folder_import_modal {
            self.show_folder_import_modal = false;
        } else if self.show_radio_add_modal {
            self.show_radio_add_modal = false;
        } else if self.details_modal.is_some() {
            self.details_modal = None;
        } else if self.show_track_search {
            self.close_track_search();
        } else if self.show_radio_search {
            self.close_radio_search();
        }
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
        let restrict_to_search = self.show_track_search
            && self.search_playback_filtered_only
            && !self.track_search_query.trim().is_empty();
        let indexes = if restrict_to_search {
            self.visible_track_indexes()
        } else {
            self.eligible_track_indexes()
        };

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
            playlist_index: self.active_playlist_index.unwrap_or(self.state.selected_playlist_index),
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
                    .filter(|title| !title.trim().is_empty() && !title.eq_ignore_ascii_case(&station.name));
                let station_name = self
                    .active_radio_station_name
                    .clone()
                    .or_else(|| station.last_station_name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| station.name.clone());

                return stream_title.unwrap_or(station_name);
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
                let station_name = self
                    .active_radio_station_name
                    .clone()
                    .or_else(|| station.last_station_name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| station.name.clone());
                let elapsed = self.radio_elapsed_seconds().map(format_duration).unwrap_or_else(|| "0:00".to_owned());
                format!("{station_name} · live for {elapsed} · {}", station.url)
            });
        }

        self.last_playback.as_ref().map(|playback| {
            if self.player_only_mode {
                format!(
                    "{} · {}",
                    format_duration(playback.rendered_duration_seconds),
                    playback.size_bytes.map(format_file_size).unwrap_or_else(|| "unknown size".to_owned())
                )
            } else {
                format!(
                    "{} · {} Hz · {} ch · {} · rendered {}",
                    display_parent(&playback.path),
                    playback.sample_rate,
                    playback.input_channels,
                    playback.size_bytes.map(format_file_size).unwrap_or_else(|| "unknown size".to_owned()),
                    format_duration(playback.rendered_duration_seconds)
                )
            }
        })
    }

    fn active_track_time_label(&self) -> String {
        if self.active_radio_index.is_some() {
            return self
                .radio_elapsed_seconds()
                .map(|elapsed| format!("Live stream · {}", format_duration(elapsed)))
                .unwrap_or_else(|| "Live stream · 0:00".to_owned());
        }

        if self.active_track_path.is_some() || self.pending_track_switch.is_some() {
            let position = self.displayed_playback_position_seconds();
            let duration = self.displayed_playback_duration_seconds();
            return format!("{} / {}", format_duration(position), format_duration(duration));
        }

        String::new()
    }

    fn radio_elapsed_seconds(&self) -> Option<f32> {
        self.radio_started_at.map(|started_at| started_at.elapsed().as_secs_f32())
    }

    fn add_radio_station(&mut self) -> bool {
        let typed_name = self.pending_radio_name.trim().to_owned();
        let url = self.pending_radio_url.trim().to_owned();
        if url.is_empty() {
            self.error_message = Some("Radio stream URL is required.".to_owned());
            return false;
        }
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            self.error_message = Some("Internet radio stream URL must start with http:// or https://.".to_owned());
            return false;
        }

        let metadata = if typed_name.is_empty() {
            self.status_message = "Reading internet radio station name...".to_owned();
            fetch_radio_stream_metadata(&url)
        } else {
            None
        };
        let station_name = if typed_name.is_empty() {
            metadata
                .as_ref()
                .and_then(|metadata| metadata.station_name.clone())
                .unwrap_or_else(|| fallback_radio_station_name(&url))
        } else {
            typed_name
        };

        let mut station = RadioStation::new(station_name, url);
        if let Some(metadata) = metadata {
            station.last_station_name = metadata.station_name;
            station.last_stream_title = metadata.stream_title;
        }
        self.state.radio_stations.push(station);
        self.state.selected_radio_index = None;
        self.radio_selection_was_user_set = false;
        self.pending_radio_name.clear();
        self.pending_radio_url.clear();
        self.status_message = "Added internet radio station.".to_owned();
        self.error_message = None;
        self.save_state_silently();
        true
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
        let settings = self.current_settings();
        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        };

        self.status_message = format!("Opening internet radio: {}...", station.name);
        self.error_message = None;
        match player.play_radio_stream(&station.url, settings) {
            Ok(()) => {
                self.active_tab = MainContentTab::Radio;
                self.active_radio_index = Some(index);
                self.active_radio_station_name = station.last_station_name.clone();
                self.active_radio_title = station.last_stream_title.clone();
                self.radio_started_at = Some(Instant::now());
                self.last_radio_title_lookup_at = Some(Instant::now());
                self.active_track_index = None;
                self.active_playlist_index = None;
                self.active_track_path = None;
                self.last_playback = None;
                self.pending_track_switch = None;
                self.state.selected_radio_index = Some(index);
                self.radio_selection_was_user_set = true;
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
            let metadata = fetch_radio_stream_metadata(&url);
            let _ = sender.send((index, metadata));
        });
    }

    fn process_radio_title_events(&mut self) {
        let Some(receiver) = &self.radio_title_receiver else {
            return;
        };
        let events = receiver.try_iter().collect::<Vec<_>>();
        let mut completed = false;
        for (index, metadata) in events {
            completed = true;
            if let Some(metadata) = metadata {
                if self.active_radio_index == Some(index) {
                    self.active_radio_station_name = metadata.station_name.clone().or_else(|| self.active_radio_station_name.clone());
                    self.active_radio_title = metadata.stream_title.clone().or_else(|| self.active_radio_title.clone());
                }
                if let Some(station) = self.state.radio_stations.get_mut(index) {
                    if metadata.station_name.is_some() {
                        station.last_station_name = metadata.station_name;
                    }
                    if metadata.stream_title.is_some() {
                        station.last_stream_title = metadata.stream_title;
                    }
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
        let removed_index = self.state.selected_playlist_index;
        let Some(playlist) = self.state.playlists.get(removed_index) else {
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

        let removed_playing_playlist = self.active_playlist_index == Some(removed_index);
        if removed_playing_playlist {
            self.stop();
        }

        self.state.playlists.remove(removed_index);
        if let Some(active_playlist_index) = self.active_playlist_index {
            if active_playlist_index > removed_index {
                self.active_playlist_index = Some(active_playlist_index - 1);
            }
        }
        self.state.selected_playlist_index = removed_index.saturating_sub(1).min(self.state.playlists.len() - 1);
        self.selected_track_indexes.clear();
        self.selected_track_index = self.eligible_track_indexes().first().copied();
        self.status_message = if removed_playing_playlist {
            "Removed playlist and stopped its active playback.".to_owned()
        } else {
            "Removed playlist. Current playback was left running.".to_owned()
        };
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
        self.prepare_track_playback(path, index, start_seconds, crossfade_seconds, false);
    }

    fn prepare_track_playback(
        &mut self,
        path: PathBuf,
        index: Option<usize>,
        start_seconds: f32,
        crossfade_seconds: f32,
        live_position_compensation: bool,
    ) {
        if self.player.is_none() {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        }

        let settings = self.current_settings();
        let playlist_index = self.state.selected_playlist_index;
        let previous_position = if crossfade_seconds > 0.05 {
            Some(self.displayed_playback_position_seconds())
        } else {
            None
        };
        let previous_duration = if crossfade_seconds > 0.05 {
            Some(self.displayed_playback_duration_seconds())
        } else {
            None
        };
        let requested_at = Instant::now();
        let (sender, receiver) = mpsc::channel();
        let path_for_thread = path.clone();

        self.pending_prepared_track_receiver = Some(receiver);
        self.status_message = if live_position_compensation {
            "Preparing updated playback without stopping the current audio...".to_owned()
        } else if crossfade_seconds > 0.05 {
            format!("Preparing crossfade for {:.1} second(s)...", crossfade_seconds)
        } else {
            format!("Preparing {}...", display_file_name(&path))
        };
        self.error_message = None;

        thread::spawn(move || {
            let result = AudioPlayer::prepare_file(path_for_thread, settings, start_seconds)
                .map(|prepared| PreparedTrackPlayback {
                    playlist_index,
                    index,
                    crossfade_seconds,
                    live_position_compensation,
                    previous_position,
                    previous_duration,
                    prepared,
                    requested_at,
                })
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });
    }

    fn process_prepared_track_playback(&mut self) {
        let Some(receiver) = &self.pending_prepared_track_receiver else {
            return;
        };

        let message = match receiver.try_recv() {
            Ok(message) => message,
            Err(mpsc::TryRecvError::Empty) => return,
            Err(mpsc::TryRecvError::Disconnected) => {
                self.pending_prepared_track_receiver = None;
                return;
            }
        };

        self.pending_prepared_track_receiver = None;
        let prepared = match message {
            Ok(prepared) => prepared,
            Err(error) => {
                self.error_message = Some(error);
                self.status_message = "Playback preparation failed.".to_owned();
                return;
            }
        };

        let PreparedTrackPlayback {
            playlist_index,
            index,
            crossfade_seconds,
            live_position_compensation,
            previous_position,
            previous_duration,
            prepared: prepared_audio,
            requested_at,
        } = prepared;

        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            return;
        };

        let render_elapsed_seconds = requested_at.elapsed().as_secs_f32();
        let was_playing = player.is_playing();
        let result = if crossfade_seconds > 0.05 {
            player.crossfade_to_prepared(prepared_audio, crossfade_seconds)
        } else if live_position_compensation && was_playing {
            player.play_prepared_from_live_position(prepared_audio, render_elapsed_seconds)
        } else {
            player.play_prepared(prepared_audio)
        };

        match result {
            Ok(info) => {
                let settings = self.current_settings();
                let mode_label = if settings.orbit_enabled {
                    settings.mode.label()
                } else {
                    "normal stereo playback"
                };
                self.active_tab = MainContentTab::Music;
                self.active_radio_index = None;
                self.active_radio_station_name = None;
                self.active_radio_title = None;
                self.radio_started_at = None;
                self.last_radio_title_lookup_at = None;
                self.radio_title_receiver = None;
                self.active_playlist_index = Some(playlist_index);
                self.selected_track_index = index;

                if crossfade_seconds > 0.05 {
                    let started_at = Instant::now();
                    let switch_after = Duration::from_secs_f32((crossfade_seconds * 0.5).max(0.1));
                    self.pending_track_switch = Some(PendingTrackSwitch {
                        switch_at: started_at + switch_after,
                        started_at,
                        previous_position: previous_position.unwrap_or_else(|| self.displayed_playback_position_seconds()),
                        previous_duration: previous_duration.unwrap_or_else(|| self.displayed_playback_duration_seconds()),
                        playlist_index,
                        index,
                        info: info.clone(),
                    });
                    self.status_message = format!(
                        "Crossfading to {} through {}; display switches halfway through the mix.",
                        display_file_name(&info.path),
                        mode_label
                    );
                } else {
                    self.active_track_index = index;
                    self.active_track_path = Some(info.path.clone());
                    self.pending_track_switch = None;
                    self.crossfade_started_for_path = None;
                    self.store_playback_metadata(&info);
                    self.remember_last_played_track(index, &info.path);
                    self.last_playback = Some(info.clone());
                    self.status_message = if live_position_compensation {
                        format!("Applied sound profile and continued {} through {}.", display_file_name(&info.path), mode_label)
                    } else {
                        format!("Playing {} through {}.", display_file_name(&info.path), mode_label)
                    };
                }
                self.error_message = None;
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
        let position = self.displayed_playback_position_seconds();
        let Some(path) = self.active_track_path.clone() else {
            return;
        };
        let Some(player) = &self.player else {
            return;
        };

        if !(player.is_playing() || player.is_paused()) {
            return;
        }

        let live_position_compensation = player.is_playing();
        self.prepare_track_playback(path, self.active_track_index, position, 0.0, live_position_compensation);
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
        self.active_playlist_index = Some(pending.playlist_index);
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

        let Some(player) = self.player.as_ref() else {
            return 0.0;
        };

        let rendered_position = player.playback_position_seconds();
        let render_start = player.current_start_offset_seconds();
        if let Some(playback) = &self.last_playback {
            rendered_to_original_position(rendered_position, render_start, playback)
        } else {
            rendered_position
        }
    }

    fn displayed_playback_duration_seconds(&self) -> f32 {
        if let Some(pending) = &self.pending_track_switch {
            return pending.previous_duration;
        }

        self.last_playback
            .as_ref()
            .map(|playback| playback.original_duration_seconds)
            .or_else(|| self.player.as_ref().and_then(AudioPlayer::playback_duration_seconds))
            .unwrap_or(0.0)
    }

    fn process_keyboard_shortcuts(&mut self, context: &egui::Context) {
        if self.show_folder_import_modal
            || self.active_panel_modal.is_some()
            || self.show_update_check_confirmation
            || self.show_radio_add_modal
            || self.details_modal.is_some()
            || context.wants_keyboard_input()
        {
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
            if self.active_tab == MainContentTab::Radio {
                self.show_radio_search = !self.show_radio_search;
                self.focus_radio_search = self.show_radio_search;
                if !self.show_radio_search {
                    self.radio_search_query.clear();
                }
            } else {
                self.show_track_search = !self.show_track_search;
                self.focus_track_search = self.show_track_search;
                self.state.ui.show_track_search = self.show_track_search;
                if !self.show_track_search {
                    self.track_search_query.clear();
                    self.search_cursor = 0;
                }
                self.save_state_silently();
            }
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
        self.active_playlist_index = None;
        self.active_track_path = None;
        self.active_radio_index = None;
        self.active_radio_station_name = None;
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

    fn check_for_updates(&mut self, confirmed_after_limit: bool) {
        if self.update_check_receiver.is_some() {
            self.error_message = Some("An update check is already running.".to_owned());
            return;
        }

        if self.update_check_count >= UPDATE_CHECKS_BEFORE_CONFIRMATION && !confirmed_after_limit {
            self.show_update_check_confirmation = true;
            self.error_message = None;
            self.status_message = "Confirm the extra release check before contacting GitHub again.".to_owned();
            return;
        }

        self.show_update_check_confirmation = false;
        self.update_check_count += 1;

        match updater::check_for_update(self.state.update_settings.include_prereleases) {
            Ok(check) => {
                self.handle_update_check_result(check, false);
            }
            Err(error) => self.error_message = Some(error.to_string()),
        }
    }

    fn start_automatic_update_check_if_due(&mut self) {
        if self.update_check_receiver.is_some()
            || self.update_check_count >= UPDATE_CHECKS_BEFORE_CONFIRMATION
        {
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

    fn start_automatic_songrec_check_if_due(&mut self) {
        if !self.state.recognition.enabled
            || !self.state.recognition.manage_songrec_automatically
            || !self.state.recognition.auto_update_songrec
            || self.songrec_tool_receiver.is_some()
            || self.songrec_install_receiver.is_some()
        {
            return;
        }

        let now = current_unix_seconds();
        let last_check = self.state.recognition.last_songrec_auto_check_unix_seconds;
        if now.saturating_sub(last_check) < AUTOMATIC_SONGREC_CHECK_INTERVAL_SECONDS {
            return;
        }

        self.state.recognition.last_songrec_auto_check_unix_seconds = now;
        self.save_state_silently();
        self.start_songrec_tool_check(false);
    }

    fn start_songrec_tool_check(&mut self, manual: bool) {
        if self.songrec_tool_receiver.is_some() {
            self.status_message = "SongRec release check is already running.".to_owned();
            return;
        }

        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = recognition::check_songrec_tool(false).map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        self.songrec_tool_receiver = Some(receiver);
        if manual {
            self.status_message = "Checking SongRec releases...".to_owned();
            self.error_message = None;
        }
    }

    fn process_songrec_tool_events(&mut self) {
        let Some(receiver) = &self.songrec_tool_receiver else {
            return;
        };

        match receiver.try_recv() {
            Ok(Ok(status)) => {
                self.songrec_tool_receiver = None;
                self.handle_songrec_tool_status(status);
            }
            Ok(Err(error)) => {
                self.songrec_tool_receiver = None;
                self.error_message = Some(format!("SongRec release check failed: {error}"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.songrec_tool_receiver = None;
                self.error_message = Some("SongRec release check stopped unexpectedly.".to_owned());
            }
        }
    }

    fn handle_songrec_tool_status(&mut self, status: recognition::SongRecToolStatus) {
        if let Some(version) = status.installed_version.clone() {
            self.state.recognition.installed_songrec_version = Some(version);
        }

        let status_message = if !status.is_installed() && status.asset_download_url.is_some() {
            "SongRec can be installed by Audio Orbit.".to_owned()
        } else if status.is_update_available {
            format!(
                "SongRec update available: v{}.",
                status.latest_version.as_deref().unwrap_or("unknown")
            )
        } else if let Some(path) = &status.executable_path {
            format!("SongRec is ready: {}.", path.display())
        } else {
            "No official Windows SongRec asset was found on the selected release. You can still set an executable manually.".to_owned()
        };

        self.last_songrec_tool_status = Some(status);
        self.status_message = status_message;
        self.error_message = None;
        self.save_state_silently();
    }

    fn install_or_update_songrec_now(&mut self) {
        if self.songrec_install_receiver.is_some() {
            self.status_message = "SongRec install/update is already running.".to_owned();
            return;
        }

        let status = self.last_songrec_tool_status.clone();
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let status = match status {
                Some(status) if status.asset_download_url.is_some() => status,
                _ => recognition::check_songrec_tool(false).map_err(|error| error.to_string())?,
            };

            let Some(download_url) = status.asset_download_url.as_deref() else {
                return Err("No official Windows SongRec downloadable asset was found for the selected release.".to_owned());
            };

            recognition::install_or_update_songrec(
                download_url,
                status.latest_version.as_deref(),
                status.asset_name.as_deref(),
            )
            .map_err(|error| error.to_string())
        });

        self.songrec_install_receiver = Some(receiver);
        self.status_message = "Installing SongRec into .audio-orbit-dll...".to_owned();
        self.error_message = None;
    }

    fn process_songrec_install_events(&mut self) {
        let Some(receiver) = &self.songrec_install_receiver else {
            return;
        };

        match receiver.try_recv() {
            Ok(Ok(installed)) => {
                self.songrec_install_receiver = None;
                self.state.recognition.songrec_command = None;
                self.state.recognition.installed_songrec_version = installed.version.clone();
                self.status_message = format!(
                    "SongRec is installed and ready: {}{}.",
                    installed.executable_path.display(),
                    installed.version.as_deref().map(|version| format!(" · v{version}")).unwrap_or_default()
                );
                self.error_message = None;
                self.save_state_silently();
                self.start_songrec_tool_check(false);
            }
            Ok(Err(error)) => {
                self.songrec_install_receiver = None;
                self.error_message = Some(format!("SongRec install/update failed: {error}"));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.songrec_install_receiver = None;
                self.error_message = Some("SongRec install/update stopped unexpectedly.".to_owned());
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
                self.open_panel_modal(AppPanelModal::Updates);
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

    fn current_radio_recording_name(&self) -> String {
        if let Some(index) = self.active_radio_index {
            if let Some(station) = self.state.radio_stations.get(index) {
                return self
                    .active_radio_station_name
                    .clone()
                    .or_else(|| station.last_station_name.clone())
                    .filter(|name| !name.trim().is_empty())
                    .unwrap_or_else(|| station.name.clone());
            }
        }
        "internet-radio".to_owned()
    }

    fn toggle_radio_recording(&mut self) {
        let is_recording = match self.player.as_ref() {
            Some(player) => player.is_radio_recording(),
            None => {
                self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
                return;
            }
        };

        if is_recording {
            let result = self
                .player
                .as_mut()
                .map(|player| player.stop_radio_recording());
            match result {
                Some(Ok(Some(info))) => {
                    self.status_message = format!(
                        "Saved radio recording: {} ({}).",
                        info.path.display(),
                        format_file_size(info.bytes_written)
                    );
                    self.error_message = None;
                }
                Some(Ok(None)) => {
                    self.status_message = "No active radio recording.".to_owned();
                }
                Some(Err(error)) => {
                    self.error_message = Some(error.to_string());
                }
                None => {
                    self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
                }
            }
            return;
        }

        if self.active_radio_index.is_none() {
            self.error_message = Some("Start an internet radio station before recording.".to_owned());
            return;
        }

        let folder = self.state.recording.resolved_output_folder();
        let station_name = self.current_radio_recording_name();
        let stream_title = self.active_radio_title.clone();
        let result = self
            .player
            .as_mut()
            .map(|player| player.start_radio_recording(&folder, &station_name, stream_title.as_deref()));
        match result {
            Some(Ok(path)) => {
                self.status_message = format!("Recording internet radio to {}.", path.display());
                self.error_message = None;
            }
            Some(Err(error)) => {
                self.error_message = Some(error.to_string());
            }
            None => {
                self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
            }
        }
    }

    fn choose_recording_folder(&mut self) {
        let initial_dir = self.state.recording.resolved_output_folder();
        let mut dialog = FileDialog::new();
        if initial_dir.exists() {
            dialog = dialog.set_directory(initial_dir);
        }
        if let Some(path) = dialog.pick_folder() {
            self.state.recording.output_folder = Some(path.clone());
            self.status_message = format!("Radio recordings folder set to {}.", path.display());
            self.error_message = None;
            self.save_state_silently();
        }
    }

    fn open_recording_folder(&mut self) {
        let folder = self.state.recording.resolved_output_folder();
        if let Err(error) = fs::create_dir_all(&folder).and_then(|_| reveal_in_file_manager(&folder).map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()))) {
            self.error_message = Some(format!("Failed to open recording folder: {error}"));
        } else {
            self.status_message = format!("Opened radio recordings folder: {}.", folder.display());
            self.error_message = None;
        }
    }
    fn open_external_tools_folder(&mut self) {
        let folder = external_tools_dir();
        if let Err(error) = fs::create_dir_all(&folder).and_then(|_| reveal_in_file_manager(&folder).map_err(|error| std::io::Error::new(std::io::ErrorKind::Other, error.to_string()))) {
            self.error_message = Some(format!("Failed to open Audio Orbit tools folder: {error}"));
        } else {
            self.status_message = format!("Opened Audio Orbit tools folder: {}.", folder.display());
            self.error_message = None;
        }
    }

    fn open_songrec_releases(&mut self) {
        if let Err(error) = open_url("https://github.com/marin-m/SongRec/releases") {
            self.error_message = Some(format!("Failed to open SongRec releases: {error}"));
        } else {
            self.status_message = "Opened SongRec releases.".to_owned();
            self.error_message = None;
        }
    }


    fn recognize_current_audio(&mut self) {
        if !self.state.recognition.enabled {
            self.status_message = "Recognition is turned off. Enable it in Settings > Recognition.".to_owned();
            self.open_panel_modal(AppPanelModal::Settings);
            return;
        }

        if self.recognition_receiver.is_some() {
            self.status_message = "Audio recognition is already running.".to_owned();
            return;
        }

        if self.state.recognition.prefer_stream_metadata {
            if let Some(title) = self
                .active_radio_title
                .clone()
                .filter(|title| !title.trim().is_empty())
            {
                self.pending_clipboard_text = Some(title.clone());
                self.status_message = format!("Radio stream title copied: {title}.");
                self.error_message = None;
                return;
            }
        }

        let sample_seconds = self.state.recognition.clamped_sample_seconds() as f32;
        let command = self.state.recognition.songrec_command.clone();
        if command.is_none()
            && self.state.recognition.manage_songrec_automatically
            && recognition::installed_songrec_executable().is_none()
        {
            self.install_or_update_songrec_now();
            self.status_message = "Installing SongRec first. Run recognition again when installation is ready.".to_owned();
            return;
        }

        if self.active_radio_index.is_some() {
            let Some(player) = &self.player else {
                self.error_message = Some("No audio output device is available. Try Refresh output device.".to_owned());
                return;
            };

            match player.radio_recognition_sample(sample_seconds) {
                Ok(Some(sample)) => {
                    self.start_recognition_worker_from_sample(sample, command, "internet radio");
                }
                Ok(None) => {
                    self.error_message = Some("Start an internet radio station before recognition.".to_owned());
                }
                Err(error) => {
                    self.error_message = Some(error.to_string());
                }
            }
            return;
        }

        if let Some(path) = self.active_track_path.clone() {
            let position = self.displayed_playback_position_seconds();
            let start_seconds = (position - 4.0).max(0.0);
            self.start_recognition_worker_from_file(path, start_seconds, sample_seconds, command);
            return;
        }

        self.error_message = Some("Start a track or internet radio station before recognition.".to_owned());
    }

    fn start_recognition_worker_from_sample(
        &mut self,
        sample: audio_player::RecognitionAudioSample,
        command: Option<PathBuf>,
        source_label: &'static str,
    ) {
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = recognize_audio_sample_with_songrec(sample, command)
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        self.recognition_receiver = Some(receiver);
        self.recognition_started_at = Some(Instant::now());
        self.status_message = format!("Identifying {source_label} with free SongRec-compatible recognition...");
        self.error_message = None;
    }

    fn start_recognition_worker_from_file(
        &mut self,
        path: PathBuf,
        start_seconds: f32,
        sample_seconds: f32,
        command: Option<PathBuf>,
    ) {
        let title = display_file_name(&path);
        let (sender, receiver) = mpsc::channel();
        thread::spawn(move || {
            let result = AudioPlayer::capture_file_recognition_sample(&path, start_seconds, sample_seconds)
                .and_then(|sample| recognize_audio_sample_with_songrec(sample, command))
                .map_err(|error| error.to_string());
            let _ = sender.send(result);
        });

        self.recognition_receiver = Some(receiver);
        self.recognition_started_at = Some(Instant::now());
        self.status_message = format!("Identifying {title} with free SongRec-compatible recognition...");
        self.error_message = None;
    }

    fn process_recognition_events(&mut self, _context: &egui::Context) {
        let Some(receiver) = &self.recognition_receiver else {
            return;
        };

        match receiver.try_recv() {
            Ok(Ok(result)) => {
                self.recognition_receiver = None;
                self.recognition_started_at = None;
                let label = result.display_label();
                self.pending_clipboard_text = Some(label.clone());
                self.status_message = format!("Recognized and copied: {label}.");
                self.error_message = None;
            }
            Ok(Err(error)) => {
                self.recognition_receiver = None;
                self.recognition_started_at = None;
                self.error_message = Some(error);
            }
            Err(mpsc::TryRecvError::Empty) => {
                if let Some(started_at) = self.recognition_started_at {
                    self.status_message = format!(
                        "Identifying audio... {}",
                        format_duration(started_at.elapsed().as_secs_f32())
                    );
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.recognition_receiver = None;
                self.recognition_started_at = None;
                self.error_message = Some("Audio recognition worker stopped unexpectedly.".to_owned());
            }
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
        } else if !self.status_message.is_empty() && self.status_updated_at.elapsed() >= Duration::from_secs(10) {
            self.status_message.clear();
            self.status_last_seen.clear();
            self.status_updated_at = Instant::now();
        }

        if self.error_message != self.error_last_seen {
            self.error_last_seen = self.error_message.clone();
            self.error_updated_at = Instant::now();
        } else if self.error_message.is_some() && self.error_updated_at.elapsed() >= Duration::from_secs(10) {
            self.error_message = None;
            self.error_last_seen = None;
            self.error_updated_at = Instant::now();
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

        let current = self.displayed_playback_position_seconds();
        let duration = self.displayed_playback_duration_seconds().max(current.max(0.0));
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
                self.active_playlist_index = None;
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
                playlist_index: self.active_playlist_index.unwrap_or(self.state.selected_playlist_index),
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
        let repaint_interval = if self.active_radio_index.is_some() {
            Duration::from_millis(16)
        } else {
            Duration::from_millis(33)
        };
        context.request_repaint_after(repaint_interval);
        self.remember_window_geometry(context);

        self.process_media_key_events();
        self.process_update_check_events();
        self.process_songrec_tool_events();
        self.process_songrec_install_events();
        self.process_recognition_events(context);
        self.process_escape_navigation(context);
        self.process_keyboard_shortcuts(context);
        self.process_radio_title_events();
        self.refresh_radio_title_periodically();
        self.process_pending_profile_apply();
        self.process_prepared_track_playback();
        self.process_pending_track_switch();
        self.update_playback_status();
        self.poll_output_device_change();
        self.sync_status_lifetime();
        if let Some(text) = self.pending_clipboard_text.take() {
            context.copy_text(text);
        }

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
                    egui::ScrollArea::vertical()
                        .id_salt("library_panel_outer_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.render_library_panel(ui);
                        });
                });
        }

        if !self.player_only_mode && self.show_profile_panel {
            egui::SidePanel::right("profile_panel")
                .resizable(true)
                .default_width(340.0)
                .width_range(280.0..=520.0)
                .show(context, |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("profile_panel_outer_scroll")
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            self.render_profile_panel(ui);
                        });
                });
        }

        if !self.status_message.is_empty() || !self.media_key_status.is_empty() || self.playlist_count_label().is_some() {
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

        if let Some(panel) = self.active_panel_modal {
            self.render_panel_modal(context, panel);
        }

        if self.show_update_check_confirmation {
            self.render_update_check_confirmation_modal(context);
        }

        if self.show_radio_add_modal {
            self.render_radio_add_modal(context);
        }

        if self.details_modal.is_some() {
            self.render_details_modal(context);
        }

        self.render_error_toast(context);
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
            let requested_points = (ui.available_width() / 2.8).round().clamp(80.0, 900.0) as usize;
            let frame = self
                .player
                .as_ref()
                .map(|player| player.radio_visualizer_frame(requested_points))
                .unwrap_or_default();
            let response = draw_radio_visualizer(ui, &frame);
            response.on_hover_text("Internet radio streams are live: new decoded audio enters on the right, older audio moves left, and connection gaps stay empty.");
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
            let response = draw_waveform_seek(ui, waveform, progress, silence_ranges, marker_duration);
            if (response.clicked() || response.drag_stopped()) && duration > 0.0 {
                if let Some(pointer) = response.interact_pointer_pos() {
                    let next_position = ((pointer.x - response.rect.left()) / response.rect.width()).clamp(0.0, 1.0) * duration;
                    self.seek_current(next_position);
                }
            }
        } else {
            let response = draw_waveform_seek(ui, &[], 0.0, &[], 0.0);
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

        ui.horizontal_wrapped(|ui| {
            ui.horizontal(|ui| {
                if !radio_controls_active {
                    if ui
                        .add_enabled(self.player.is_some(), egui::Button::new(self.control_label(Icon::SkipBack, "Previous")))
                        .clicked()
                    {
                        self.play_previous_track();
                    }
                }

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
                }
            });

            if !radio_controls_active {
                ui.horizontal(|ui| {
                    self.render_compact_playback_toggles(ui);
                });
            } else {
                ui.horizontal(|ui| {
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
                            .min_size(egui::vec2(28.0, 24.0))
                            .fill(egui::Color32::from_rgb(82, 24, 28))
                    } else {
                        egui::Button::new(record_text).min_size(egui::vec2(28.0, 24.0))
                    };
                    let record_response = ui
                        .add(record_button)
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
                });
            }

            ui.horizontal(|ui| {
                let recognition_running = self.recognition_receiver.is_some();
                let recognition_enabled = self.state.recognition.enabled;
                let can_recognize = recognition_enabled && has_now_playing && !recognition_running;
                let recognition_icon = if recognition_running { Icon::RefreshCw } else { Icon::Search };
                let recognition_color = if recognition_running {
                    ui.visuals().selection.bg_fill
                } else {
                    ui.visuals().widgets.inactive.fg_stroke.color
                };
                if ui
                    .add_enabled(
                        can_recognize,
                        egui::Button::new(
                            egui::RichText::new(ui_icons::icon(recognition_icon))
                                .size(14.0)
                                .color(recognition_color),
                        )
                        .min_size(egui::vec2(28.0, 24.0)),
                    )
                    .on_hover_text(if recognition_enabled {
                        "Identify the current song with free SongRec-compatible recognition"
                    } else {
                        "Recognition is off. Enable it in Settings > Recognition."
                    })
                    .clicked()
                {
                    self.recognize_current_audio();
                }
            });

            ui.horizontal(|ui| {
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
            if ui.button(ui_icons::label(Icon::Settings2, "Settings...")).clicked() {
                self.open_panel_modal(AppPanelModal::Settings);
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
        if ui.button(ui_icons::label(Icon::Settings2, "Settings...")).clicked() {
            self.open_panel_modal(AppPanelModal::Settings);
        }
    }

    fn render_current_playlist_controls(&mut self, ui: &mut egui::Ui) {
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

    fn render_radio_panel(&mut self, ui: &mut egui::Ui) {
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
        ui.add(egui::Label::new("Radio streams are live sources. They ignore shuffle, repeat, auto-play next, crossfade, silence skipping, and playback transitions.").wrap());
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

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show(ui, |ui| {
                ui.set_width(row_width);
                for (index, station) in visible_stations {
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
                    let context_response = ui.interact(
                        row_response.response.rect.expand(2.0),
                        ui.make_persistent_id(("radio_station_context", index)),
                        egui::Sense::click(),
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
                        if ui.button(ui_icons::label(Icon::Trash2, "Remove station")).clicked() {
                            remove_radio_index = Some(index);
                            ui.close_menu();
                        }
                    });
                    if self.scroll_to_active_radio_requested && active {
                        row_response.response.scroll_to_me(Some(egui::Align::Center));
                        self.scroll_to_active_radio_requested = false;
                    }
                    ui.separator();
                }
            });

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

    fn request_folder_group_top(&mut self, group: &str) {
        self.scroll_to_folder_group_requested = Some(group.to_owned());
    }

    fn toggle_folder_group_collapsed_and_focus(&mut self, group: &str) {
        if self.collapsed_groups.contains(group) {
            self.collapsed_groups.remove(group);
        } else {
            self.collapsed_groups.insert(group.to_owned());
        }
        self.request_folder_group_top(group);
    }

    fn render_track_panel(&mut self, ui: &mut egui::Ui) {
        let Some(playlist) = self.current_playlist() else {
            ui.heading("No playlist");
            return;
        };

        let playlist_name = playlist.name.clone();
        let selected_playlist_label = format!("{} {}", playlist.kind.icon(), playlist_name);
        let selected_playlist_short_label = ellipsize_chars(&selected_playlist_label, 24);
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
            egui::ComboBox::from_id_salt("track_panel_playlist_selector")
                .selected_text(selected_playlist_short_label)
                .width(120.0)
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
            if folder_group_count > 1 && !selected_group_label.is_empty() {
                ui.small(ellipsize_chars(&selected_group_label, 28));
            }

            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let has_active_source = self.active_track_path.is_some() || self.active_radio_index.is_some();
                if ui
                    .add_enabled(has_active_source, egui::Button::new(ui_icons::label(Icon::Music, "Now playing")))
                    .on_hover_text("Switch to the active source and center the currently playing item")
                    .clicked()
                {
                    self.jump_to_now_playing();
                }

                let search_label = if self.show_track_search {
                    self.control_label(Icon::X, "Close search")
                } else {
                    self.control_label(Icon::Search, "Search")
                };
                if ui.button(search_label).clicked() {
                    self.show_track_search = !self.show_track_search;
                    self.focus_track_search = self.show_track_search;
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
                if self.focus_track_search {
                    response.request_focus();
                    self.focus_track_search = false;
                }
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
                ui.separator();
                if ui
                    .checkbox(&mut self.search_playback_filtered_only, "Play results only")
                    .on_hover_text("When enabled, Next/auto-play stays inside the current search results. Turn it off to keep normal playlist playback while searching.")
                    .changed()
                {
                    self.state.ui.search_playback_filtered_only = self.search_playback_filtered_only;
                    self.save_state_silently();
                }
            });
            if !query.is_empty() {
                let mode = if self.search_playback_filtered_only { "playback is limited to search results" } else { "playback keeps normal playlist order" };
                ui.small(format!("Filtering tracks by: {query} · {mode}"));
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
        let scroll_output = egui::ScrollArea::vertical()
            .id_salt("track_list_scroll")
            .vertical_scroll_offset(self.state.ui.playlist_scroll_offset_y.max(0.0))
            .auto_shrink([false, false])
            .max_height(scroll_height)
            .show_viewport(ui, |ui, _viewport| {
                ui.set_width(row_width);
                let visible_rect = ui.clip_rect();
                let sticky_top = visible_rect.top();
                let mut viewport_group: Option<String> = None;
                let mut next_group_header_top: Option<f32> = None;
                for (index, track) in visible_tracks {
                    if show_group_headers && track.group != last_group {
                        ui.add_space(6.0);
                        let group = track.group.clone();
                        let collapsed = self.collapsed_groups.contains(&group);
                        let mut toggle_group = false;
                        let header_response = ui.horizontal(|ui| {
                            let icon = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
                            if ui.small_button(ui_icons::icon(icon)).on_hover_text("Collapse/expand folder").clicked() {
                                toggle_group = true;
                            }
                            ui.label(egui::RichText::new(group.as_str()).size(13.0).strong());
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
                        egui::Sense::click(),
                    );
                    if context_response.clicked() {
                        self.selected_track_index = Some(index);
                    }
                    if context_response.double_clicked() {
                        self.selected_track_index = Some(index);
                        self.play_path(path.clone(), Some(index), 0.0);
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
                    ui.separator();
                }

                if show_group_headers {
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
        self.state.ui.playlist_scroll_offset_y = scroll_output.state.offset.y.max(0.0);
    }

    fn render_track_row_context_menu(
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
                        egui::Slider::new(&mut profile.settings.silence_threshold_seconds, 2u8..=30u8)
                            .text("Skip gaps longer than (sec)"),
                    )
                    .on_hover_text("One continuous near-silent gap must last this long before Audio Orbit skips it.")
                    .changed();
                ui.small("Audio Orbit detects the near-silence level automatically, so this behaves like AIMP-style gap skipping without a manual gate control.");
            }
        }
        if profile_changed {
            self.schedule_current_profile_apply();
        }
    }



    fn render_panel_modal(&mut self, context: &egui::Context, panel: AppPanelModal) {
        self.render_modal_backdrop(context, "panel_modal_backdrop");
        let screen_rect = context.screen_rect();
        let outer_padding = egui::vec2(28.0, 22.0);
        let content_size = egui::vec2(
            (screen_rect.width() - outer_padding.x * 2.0).max(280.0),
            (screen_rect.height() - outer_padding.y * 2.0).max(200.0),
        );
        let scroll_height = (content_size.y - 76.0).max(180.0);

        egui::Area::new(egui::Id::new("panel_modal"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(244))
                    .inner_margin(egui::Margin::symmetric(outer_padding.x as i8, outer_padding.y as i8))
                    .show(ui, |ui| {
                        ui.set_min_size(content_size);
                        ui.set_max_width(content_size.x);

                        ui.horizontal(|ui| {
                            ui.heading(ui_icons::label(panel.icon(), panel.title()));
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                                if ui
                                    .add_sized(egui::vec2(42.0, 34.0), egui::Button::new(egui::RichText::new(ui_icons::icon(Icon::X)).size(18.0)))
                                    .on_hover_text("Close")
                                    .clicked()
                                {
                                    self.close_panel_modal();
                                }
                            });
                        });
                        ui.add_space(1.0);
                        ui.add(egui::Label::new(panel.description()).wrap());
                        ui.add_space(8.0);

                        egui::ScrollArea::vertical()
                            .max_height(scroll_height)
                            .auto_shrink([false, false])
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                match panel {
                                    AppPanelModal::Settings => self.render_settings_panel_content(ui),
                                    AppPanelModal::Updates => self.render_update_settings_section(ui, false),
                                    AppPanelModal::Backup => self.render_backup_settings_section_inner(ui, false),
                                    AppPanelModal::About => self.render_about_section_inner(ui, false),
                                }
                            });
                        self.render_modal_info_footer(ui);
                    });
            });
    }


    fn render_settings_panel_content(&mut self, ui: &mut egui::Ui) {
        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.heading("Panels");
            ui.small("Open a separate panel. Esc or the top-right X returns to the previous panel.");
            ui.horizontal_wrapped(|ui| {
                if ui.button(ui_icons::label(Icon::Download, "Updates")).clicked() {
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
        ui.add_space(12.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.render_playback_settings_section(ui);
        });
        ui.add_space(12.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.render_recording_settings_section(ui);
        });
        ui.add_space(12.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.render_recognition_settings_section(ui);
        });
        ui.add_space(12.0);

        egui::Frame::group(ui.style()).show(ui, |ui| {
            ui.set_width(ui.available_width());
            self.render_profile_panel(ui);
        });
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
        painter.rect_filled(screen_rect, 0.0, egui::Color32::from_black_alpha(230));
    }


    fn render_details_modal(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "details_modal_backdrop");
        let details = self.details_modal.clone();
        let mut is_open = details.is_some();
        let modal_size = self.responsive_modal_size(context, 680.0, 560.0);
        let scroll_height = (modal_size.y - 74.0).max(160.0);

        egui::Area::new(egui::Id::new("details_modal"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(context, |ui| {
                egui::Frame::window(ui.style()).show(ui, |ui| {
                    ui.set_min_size(modal_size);
                    ui.set_max_width(modal_size.x);

                    ui.horizontal(|ui| {
                        ui.heading(ui_icons::label(Icon::Info, "Details"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(ui_icons::icon(Icon::X)).on_hover_text("Close").clicked() {
                                is_open = false;
                            }
                        });
                    });
                    ui.separator();

                    egui::ScrollArea::vertical()
                        .max_height(scroll_height)
                        .auto_shrink([false, false])
                        .show(ui, |ui| {
                            ui.set_width(ui.available_width());
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
                    self.render_modal_info_footer(ui);

                });
            });

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            is_open = false;
        }
        if !is_open {
            self.details_modal = None;
        }
    }

    fn find_track_by_path(&self, path: &Path) -> Option<&Track> {
        self.state
            .playlists
            .iter()
            .flat_map(|playlist| playlist.tracks.iter())
            .find(|track| same_path(&track.path, path))
    }

    fn find_track_location(&self, path: &Path) -> Option<(usize, usize)> {
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

    fn jump_to_now_playing(&mut self) {
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

    fn render_update_check_confirmation_modal(&mut self, context: &egui::Context) {
        let mut is_open = self.show_update_check_confirmation;
        let screen_rect = context.screen_rect();
        let card_width = (screen_rect.width() - 48.0).clamp(300.0, 560.0);

        egui::Area::new(egui::Id::new("update_check_confirmation_scrim"))
            .order(egui::Order::Foreground)
            .fixed_pos(screen_rect.left_top())
            .show(context, |ui| {
                let local_rect = egui::Rect::from_min_size(egui::Pos2::ZERO, screen_rect.size());
                let response = ui.allocate_rect(local_rect, egui::Sense::click());
                ui.painter().rect_filled(local_rect, 0.0, egui::Color32::from_black_alpha(188));
                response.on_hover_text("Confirm or cancel the release check.");
            });

        egui::Area::new(egui::Id::new("update_check_confirmation_modal"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(context, |ui| {
                egui::Frame::window(ui.style())
                    .inner_margin(egui::Margin::symmetric(18, 16))
                    .show(ui, |ui| {
                        ui.set_min_width(card_width);
                        ui.set_max_width(card_width);

                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.heading(ui_icons::label(Icon::Info, "Confirm release check"));
                                ui.add_space(2.0);
                                ui.add(
                                    egui::Label::new("GitHub release checks are still allowed, but Audio Orbit asks before making another request after repeated checks.")
                                        .wrap(),
                                );
                            });
                            ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
                                if ui
                                    .add_sized(egui::vec2(36.0, 30.0), egui::Button::new(ui_icons::icon(Icon::X)))
                                    .on_hover_text("Cancel")
                                    .clicked()
                                {
                                    is_open = false;
                                }
                            });
                        });

                        ui.add_space(12.0);
                        egui::Frame::group(ui.style()).show(ui, |ui| {
                            ui.set_width(ui.available_width());
                            ui.label(format!(
                                "Checks this session: {}",
                                self.update_check_count
                            ));
                            ui.small("Too many repeated GitHub API requests can be temporarily rate limited. Confirming only performs one additional release lookup now.");
                        });

                        ui.add_space(14.0);
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            let can_confirm = self.update_check_receiver.is_none();
                            if ui
                                .add_enabled(can_confirm, egui::Button::new(ui_icons::label(Icon::Search, "Check again")))
                                .clicked()
                            {
                                self.check_for_updates(true);
                                is_open = false;
                            }

                            if ui.button("Cancel").clicked() {
                                is_open = false;
                            }
                        });
                    });
            });

        self.show_update_check_confirmation = is_open;
    }

    fn render_radio_add_modal(&mut self, context: &egui::Context) {
        self.render_modal_backdrop(context, "radio_add_modal_backdrop");
        let mut is_open = self.show_radio_add_modal;
        let modal_size = self.responsive_modal_size(context, 560.0, 360.0);

        egui::Area::new(egui::Id::new("radio_add_modal"))
            .order(egui::Order::Foreground)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(context, |ui| {
                egui::Frame::window(ui.style()).show(ui, |ui| {
                    ui.set_min_size(modal_size);
                    ui.set_max_width(modal_size.x);

                    ui.horizontal(|ui| {
                        ui.heading(ui_icons::label(Icon::Radio, "Add internet radio"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui.small_button(ui_icons::icon(Icon::X)).on_hover_text("Close").clicked() {
                                is_open = false;
                            }
                        });
                    });
                    ui.add(egui::Label::new("Add a stream URL. If the name is empty, Audio Orbit tries to read the station name from stream headers and falls back to the stream host.").wrap());
                    ui.separator();

                    ui.label("Stream URL");
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 24.0),
                        egui::TextEdit::singleline(&mut self.pending_radio_url).hint_text("https://..."),
                    );
                    ui.add_space(8.0);
                    ui.label("Name (optional)");
                    ui.add_sized(
                        egui::vec2(ui.available_width(), 24.0),
                        egui::TextEdit::singleline(&mut self.pending_radio_name).hint_text("Read from stream if empty"),
                    );
                    ui.add_space(14.0);

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button(ui_icons::label(Icon::Plus, "Add station")).clicked() {
                            if self.add_radio_station() {
                                is_open = false;
                            }
                        }
                    });
                    self.render_modal_info_footer(ui);
                });
            });

        if context.input(|input| input.key_pressed(egui::Key::Escape)) {
            is_open = false;
        }

        self.show_radio_add_modal = is_open;
    }

    fn render_recognition_settings_section(&mut self, ui: &mut egui::Ui) {
        ui.heading("Recognition");
        ui.small("Optional, fully free recognition. It is off by default; when enabled, Audio Orbit can manage SongRec inside .audio-orbit-dll so the app stays installer-free.");

        let enabled_changed = ui
            .checkbox(&mut self.state.recognition.enabled, "Enable recognition")
            .on_hover_text("When off, the top-bar recognition button is disabled and Audio Orbit does not call SongRec.")
            .changed();
        if enabled_changed {
            self.save_state_silently();
            if self.state.recognition.enabled
                && self.state.recognition.manage_songrec_automatically
                && self.state.recognition.auto_update_songrec
            {
                self.start_automatic_songrec_check_if_due();
            }
        }

        ui.add_enabled_ui(self.state.recognition.enabled, |ui| {
            let metadata_first_changed = ui
                .checkbox(
                    &mut self.state.recognition.prefer_stream_metadata,
                    "Use radio StreamTitle metadata first",
                )
                .on_hover_text("When internet radio already provides the current track title, Audio Orbit returns that instantly and avoids an external lookup.")
                .changed();
            if metadata_first_changed {
                self.save_state_silently();
            }

            let manage_changed = ui
                .checkbox(
                    &mut self.state.recognition.manage_songrec_automatically,
                    "Let Audio Orbit install and manage SongRec",
                )
                .on_hover_text("Downloads SongRec into the managed .audio-orbit-dll folder instead of asking the user to place files manually.")
                .changed();
            if manage_changed {
                self.save_state_silently();
            }

            let auto_update_changed = ui
                .add_enabled(
                    self.state.recognition.manage_songrec_automatically,
                    egui::Checkbox::new(
                        &mut self.state.recognition.auto_update_songrec,
                        "Check SongRec updates once per day on startup",
                    ),
                )
                .on_hover_text("Only runs when recognition and automatic SongRec management are enabled.")
                .changed();
            if auto_update_changed {
                self.save_state_silently();
            }

            ui.add_space(6.0);
            ui.label("SongRec executable");
            let tools_folder = external_tools_dir();
            let installed_path = recognition::installed_songrec_executable();
            ui.horizontal_wrapped(|ui| {
                ui.monospace(self.state.recognition.command_label());
                if ui.button(ui_icons::label(Icon::Search, "Check SongRec")).clicked() {
                    self.start_songrec_tool_check(true);
                }
                if ui
                    .add_enabled(
                        self.state.recognition.manage_songrec_automatically && self.songrec_install_receiver.is_none(),
                        egui::Button::new(ui_icons::label(Icon::Download, "Install / update")),
                    )
                    .clicked()
                {
                    self.install_or_update_songrec_now();
                }
                if ui.button(ui_icons::label(Icon::FolderOpen, "Choose...")) .clicked() {
                    if let Some(path) = FileDialog::new().pick_file() {
                        self.state.recognition.songrec_command = Some(path.clone());
                        self.status_message = format!("SongRec executable set to {}.", path.display());
                        self.error_message = None;
                        self.save_state_silently();
                    }
                }
                if ui.button(ui_icons::label(Icon::FolderOpen, "Open .audio-orbit-dll")).clicked() {
                    self.open_external_tools_folder();
                }
                if ui.button(ui_icons::label(Icon::ExternalLink, "SongRec releases")).clicked() {
                    self.open_songrec_releases();
                }
                if ui.button("Auto lookup").clicked() {
                    self.state.recognition.songrec_command = None;
                    self.status_message = "SongRec executable reset to automatic lookup.".to_owned();
                    self.error_message = None;
                    self.save_state_silently();
                }
            });

            if let Some(path) = installed_path {
                ui.small(format!("Managed SongRec: {}", path.display()));
            } else {
                ui.small(format!("Managed SongRec is not installed yet. Audio Orbit will install it into {} when requested.", tools_folder.display()));
            }

            if let Some(status) = &self.last_songrec_tool_status {
                ui.small(format!(
                    "Latest checked SongRec: {} · asset: {}",
                    status.latest_version.as_deref().unwrap_or("unknown"),
                    status.asset_name.as_deref().unwrap_or("none")
                ));
            }

            let mut sample_seconds = self.state.recognition.clamped_sample_seconds();
            if ui
                .add(egui::Slider::new(&mut sample_seconds, 6..=20).text("sample seconds"))
                .on_hover_text("Longer samples can improve recognition but take slightly longer to process.")
                .changed()
            {
                self.state.recognition.sample_seconds = sample_seconds;
                self.save_state_silently();
            }
        });

        if !self.state.recognition.enabled {
            ui.small("Recognition is disabled. The app will not use SongRec or make SongRec release checks until you enable this option.");
        }

        ui.small("SongRec is GPL-3.0 and optional. Audio Orbit uses it as an external helper executable for Shazam-compatible recognition, not as a required runtime dependency.");
    }

    fn render_recording_settings_section(&mut self, ui: &mut egui::Ui) {
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
                        egui::Slider::new(&mut profile.settings.silence_threshold_seconds, 2u8..=30u8)
                            .text("Skip gaps longer than (sec)"),
                    )
                    .on_hover_text("One continuous near-silent gap must last this long before Audio Orbit skips it.")
                    .changed();
                ui.small("Audio Orbit detects the near-silence level automatically, so this behaves like AIMP-style gap skipping without a manual gate control.");
            }
        }
        if profile_changed {
            self.schedule_current_profile_apply();
        }
    }

    fn render_backup_settings_section_inner(&mut self, ui: &mut egui::Ui, show_title: bool) {
        if show_title {
            ui.heading("Backup and data");
        }
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
        if show_modal_button {
            ui.heading("Updates");
        }
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
                format!(
                    "Checks this session: {} · checking...",
                    self.update_check_count
                )
            } else if self.update_check_count >= UPDATE_CHECKS_BEFORE_CONFIRMATION {
                format!(
                    "Checks this session: {} · confirmation required for each extra check",
                    self.update_check_count
                )
            } else {
                format!(
                    "Checks this session: {} · confirmation starts after {}",
                    self.update_check_count,
                    UPDATE_CHECKS_BEFORE_CONFIRMATION
                )
            };
            ui.label(check_status);

            let can_check = self.update_check_receiver.is_none();
            if ui
                .add_enabled(can_check, egui::Button::new(ui_icons::label(Icon::Search, "Check releases")))
                .clicked()
            {
                self.check_for_updates(false);
            }

            if show_modal_button && ui.button(ui_icons::label(Icon::Download, "Open Updates panel")).clicked() {
                self.open_panel_modal(AppPanelModal::Updates);
            }

            if ui.button(ui_icons::label(Icon::ExternalLink, "Open releases" )).clicked() {
                if let Err(error) = updater::open_releases_page() {
                    self.error_message = Some(error.to_string());
                }
            }
        });

        if self.update_check_count >= UPDATE_CHECKS_BEFORE_CONFIRMATION {
            ui.colored_label(
                egui::Color32::YELLOW,
                "Further release checks are allowed, but Audio Orbit will ask before contacting GitHub again because repeated requests may trigger temporary API rate limiting.",
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

    fn render_about_section_inner(&mut self, ui: &mut egui::Ui, show_title: bool) {
        if show_title {
            ui.heading("About Audio Orbit");
        }
        ui.add(egui::Label::new("Audio Orbit is a lightweight Windows music player focused on local libraries, folder-based playlists, smooth crossfade playback, silence skipping, and headphone-friendly orbit-style stereo movement.").wrap());
        ui.add_space(8.0);
        ui.add(egui::Label::new(format!("Version: v{}", env!("CARGO_PKG_VERSION"))).wrap());
        ui.add(egui::Label::new("Creator: Zoltán Rózsa").wrap());
        ui.add(egui::Label::new("License: GNU Affero General Public License v3.0 (AGPL-3.0)").wrap());
        ui.add(egui::Label::new("This app stores its portable state next to the executable in .audio-orbit-data.").wrap());

        ui.add_space(10.0);
        ui.heading("External components");
        ui.add(egui::Label::new("RustFFT — high-performance pure Rust FFT used for Audio Orbit waveform/spectrum analysis. License: MIT OR Apache-2.0. GitHub: https://github.com/ejmahler/RustFFT").wrap());
        ui.add(egui::Label::new("SongRec — optional free/open-source Shazam-compatible recognizer executable. License: GPL-3.0. GitHub: https://github.com/marin-m/SongRec").wrap());
        ui.add(egui::Label::new(format!("Managed optional helpers are installed by Audio Orbit into {} when enabled.", external_tools_dir().display())).wrap());
        if let Some(path) = recognition::installed_songrec_executable() {
            ui.add(egui::Label::new(format!("Managed SongRec executable: {}", path.display())).wrap());
        }

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

    fn render_modal_info_footer(&mut self, ui: &mut egui::Ui) {
        if self.status_message.is_empty() && self.error_message.is_none() {
            return;
        }

        ui.add_space(8.0);
        ui.separator();
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(egui::Color32::from_black_alpha(92))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                if let Some(error) = &self.error_message {
                    ui.colored_label(egui::Color32::LIGHT_RED, error.as_str());
                } else {
                    ui.label(self.status_message.as_str());
                }
            });
    }

    fn render_inline_status_strip(&mut self, ui: &mut egui::Ui) {
        if self.status_message.is_empty() {
            return;
        }
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(egui::Color32::from_black_alpha(92))
            .corner_radius(egui::CornerRadius::same(6))
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal_wrapped(|ui| {
                    ui.label(self.status_message.as_str());
                });
            });
        ui.add_space(4.0);
    }

    fn playlist_count_label(&self) -> Option<String> {
        if self.active_tab != MainContentTab::Music {
            return None;
        }
        let playlist = self.current_playlist()?;
        let total = playlist.tracks.len();
        if total == 0 {
            return None;
        }
        let visible = self.visible_track_indexes().len();
        if self.show_track_search && !self.track_search_query.trim().is_empty() && visible != total {
            Some(format!("{visible}/{total} tracks"))
        } else {
            Some(format!("{total} tracks"))
        }
    }

    fn render_status_panel(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            if !self.status_message.is_empty() {
                ui.label(self.status_message.as_str());
                if !self.media_key_status.is_empty() {
                    ui.separator();
                }
            }
            if !self.media_key_status.is_empty() {
                ui.small(self.media_key_status.as_str());
            }

            if let Some(count_label) = self.playlist_count_label() {
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.small(count_label);
                });
            }
        });
    }

    fn render_error_toast(&mut self, context: &egui::Context) {
        let Some(error_message) = self.error_message.clone() else {
            return;
        };

        let screen_rect = context.screen_rect();
        let horizontal_margin = 16.0;
        let width = (screen_rect.width() - horizontal_margin * 2.0).max(240.0);
        egui::Area::new(egui::Id::new("error_toast_overlay"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -12.0])
            .show(context, |ui| {
                ui.set_width(width);
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(238))
                    .stroke(egui::Stroke::new(1.0, egui::Color32::from_rgb(150, 54, 62)))
                    .corner_radius(egui::CornerRadius::same(8))
                    .inner_margin(egui::Margin::symmetric(12, 7))
                    .show(ui, |ui| {
                        ui.set_width(width);
                        ui.horizontal_wrapped(|ui| {
                            ui.colored_label(egui::Color32::from_rgb(255, 112, 112), error_message.as_str());
                        });
                    });
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

fn fetch_radio_stream_metadata(url: &str) -> Option<RadioStreamMetadata> {
    let client = reqwest::blocking::Client::builder()
        .user_agent("Audio-Orbit-Radio-Metadata")
        .timeout(Duration::from_secs(6))
        .build()
        .ok()?;

    let mut response = client
        .get(url)
        .header("Icy-MetaData", "1")
        .send()
        .ok()?;

    let headers = response.headers().clone();
    let station_name = headers
        .get("icy-name")
        .or_else(|| headers.get("x-audiocast-name"))
        .or_else(|| headers.get("icy-description"))
        .and_then(|value| value.to_str().ok())
        .map(clean_radio_metadata_value)
        .filter(|value| !value.is_empty());

    let stream_title = headers
        .get("icy-title")
        .and_then(|value| value.to_str().ok())
        .map(clean_radio_metadata_value)
        .filter(|value| !value.is_empty())
        .or_else(|| {
            let metadata_interval = headers
                .get("icy-metaint")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.parse::<usize>().ok())?;
            read_icy_stream_title(&mut response, metadata_interval)
        });

    if station_name.is_none() && stream_title.is_none() {
        None
    } else {
        Some(RadioStreamMetadata {
            station_name,
            stream_title,
        })
    }
}

fn read_icy_stream_title<R: Read>(reader: &mut R, metadata_interval: usize) -> Option<String> {
    if metadata_interval == 0 || metadata_interval > 2_000_000 {
        return None;
    }

    let mut audio_buffer = vec![0_u8; metadata_interval];
    reader.read_exact(&mut audio_buffer).ok()?;

    let mut length_byte = [0_u8; 1];
    reader.read_exact(&mut length_byte).ok()?;
    let metadata_length = length_byte[0] as usize * 16;
    if metadata_length == 0 || metadata_length > 4096 {
        return None;
    }

    let mut metadata = vec![0_u8; metadata_length];
    reader.read_exact(&mut metadata).ok()?;
    let metadata = String::from_utf8_lossy(&metadata);
    parse_icy_stream_title(&metadata)
}

fn parse_icy_stream_title(metadata: &str) -> Option<String> {
    let marker = "StreamTitle='";
    let start = metadata.find(marker)? + marker.len();
    let rest = &metadata[start..];
    let end = rest.find("';").or_else(|| rest.find('\''))?;
    Some(clean_radio_metadata_value(&rest[..end])).filter(|value| !value.is_empty())
}

fn clean_radio_metadata_value(value: &str) -> String {
    value
        .trim_matches(char::from(0))
        .trim()
        .trim_matches('\'')
        .trim_matches('"')
        .trim()
        .to_owned()
}


fn text_width(ui: &egui::Ui, value: &str, font_id: egui::FontId, color: egui::Color32) -> f32 {
    if value.trim().is_empty() {
        return 0.0;
    }

    ui.painter()
        .layout_no_wrap(value.to_owned(), font_id, color)
        .rect
        .width()
}

fn ellipsize_to_width_exact(ui: &egui::Ui, value: &str, width: f32, font_id: egui::FontId, color: egui::Color32) -> String {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if text_width(ui, trimmed, font_id.clone(), color) <= width {
        return trimmed.to_owned();
    }

    let ellipsis = "…";
    let ellipsis_width = text_width(ui, ellipsis, font_id.clone(), color);
    let available_width = (width - ellipsis_width).max(0.0);
    if available_width <= 0.0 {
        return ellipsis.to_owned();
    }

    let chars: Vec<char> = trimmed.chars().collect();
    let mut low = 0usize;
    let mut high = chars.len();

    while low < high {
        let mid = (low + high + 1) / 2;
        let candidate: String = chars.iter().take(mid).collect();
        if text_width(ui, &candidate, font_id.clone(), color) <= available_width {
            low = mid;
        } else {
            high = mid - 1;
        }
    }

    if low == 0 {
        ellipsis.to_owned()
    } else {
        format!("{}{}", chars.iter().take(low).collect::<String>(), ellipsis)
    }
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

    if !state.ui.playlist_scroll_offset_y.is_finite() || state.ui.playlist_scroll_offset_y < 0.0 {
        state.ui.playlist_scroll_offset_y = 0.0;
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


fn paint_sticky_folder_header(
    ui: &egui::Ui,
    visible_rect: egui::Rect,
    group: &str,
    collapsed: bool,
    push_offset_y: f32,
) -> (egui::Rect, egui::Rect) {
    let header_height = 24.0;
    let rect = egui::Rect::from_min_max(
        egui::pos2(visible_rect.left(), visible_rect.top() + push_offset_y),
        egui::pos2(visible_rect.right(), visible_rect.top() + push_offset_y + header_height),
    );
    let painter = ui.painter().with_clip_rect(visible_rect);
    let visuals = ui.visuals();
    let background = visuals.widgets.noninteractive.bg_fill;
    let stroke = visuals.widgets.noninteractive.bg_stroke;
    painter.rect_filled(rect, 0.0, background);
    painter.line_segment([rect.left_bottom(), rect.right_bottom()], stroke);

    let icon = if collapsed { Icon::ChevronRight } else { Icon::ChevronDown };
    let icon_rect = egui::Rect::from_min_size(
        egui::pos2(rect.left() + 8.0, rect.top() + 3.0),
        egui::vec2(18.0, header_height - 6.0),
    );
    let text_left = icon_rect.right() + 4.0;
    let text_width = (rect.right() - text_left - 10.0).max(24.0);
    let text_color = visuals.widgets.inactive.fg_stroke.color.linear_multiply(0.92);

    painter.text(
        icon_rect.center(),
        egui::Align2::CENTER_CENTER,
        ui_icons::icon(icon),
        egui::FontId::proportional(11.0),
        text_color,
    );
    painter.text(
        egui::pos2(text_left, rect.center().y),
        egui::Align2::LEFT_CENTER,
        ellipsize_to_width(group, text_width, 12.0),
        egui::FontId::proportional(12.0),
        text_color,
    );

    (rect, icon_rect)
}

fn draw_radio_visualizer(ui: &mut egui::Ui, frame: &RadioVisualizerFrame) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), 46.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let visuals = ui.visuals();
    let painter = ui.painter();

    painter.rect_filled(rect, 8.0, egui::Color32::from_black_alpha(210));

    let bar_count = (rect.width() / 2.8).round().clamp(80.0, 900.0) as usize;
    let gap = 0.75;
    let bar_width = ((rect.width() - gap * bar_count.saturating_sub(1) as f32) / bar_count.max(1) as f32)
        .clamp(0.65, 2.2);
    let pitch = bar_width + gap;
    let bucket_seconds = if frame.bucket_seconds > 0.0 { frame.bucket_seconds } else { 1.0 / 24.0 };

    let zero_color = visuals.widgets.inactive.fg_stroke.color.linear_multiply(0.14);
    let center_y = rect.center().y;
    for index in 0..bar_count {
        let x1 = rect.left() + index as f32 * pitch;
        let x2 = (x1 + bar_width).min(rect.right());
        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x1, center_y - 0.35), egui::pos2(x2, center_y + 0.35)),
            bar_width / 2.0,
            zero_color,
        );
    }

    for bar in &frame.bars {
        let offset = bar.age_seconds.max(0.0) / bucket_seconds;
        let x2 = rect.right() - offset * pitch;
        let x1 = x2 - bar_width;
        if x1 >= rect.right() || x2 <= rect.left() {
            continue;
        }

        let value = bar.peak.clamp(0.0, 1.0);
        if value <= 0.006 {
            continue;
        }
        let normalized = value.powf(0.92);
        let height = (rect.height() * 0.88 * normalized).clamp(2.0, rect.height() * 0.92);
        let y1 = rect.center().y - height / 2.0;
        let y2 = rect.center().y + height / 2.0;

        painter.rect_filled(
            egui::Rect::from_min_max(egui::pos2(x1.max(rect.left()), y1), egui::pos2(x2.min(rect.right()), y2)),
            bar_width / 2.0,
            visuals.selection.bg_fill,
        );
    }

    response
}


fn draw_waveform_seek(ui: &mut egui::Ui, waveform: &[f32], progress: f32, silence_ranges: &[(f32, f32)], duration_seconds: f32) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width(), 46.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let visuals = ui.visuals();
    let painter = ui.painter();

    painter.rect_filled(rect, 8.0, egui::Color32::from_black_alpha(210));

    if waveform.is_empty() {
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
        let bar_start_seconds = if duration_seconds > 0.0 {
            (bar_index * step) as f32 / waveform.len().max(1) as f32 * duration_seconds
        } else {
            0.0
        };
        let bar_end_seconds = if duration_seconds > 0.0 {
            ((bar_index + 1) * step).min(waveform.len()) as f32 / waveform.len().max(1) as f32 * duration_seconds
        } else {
            0.0
        };
        let is_silence = duration_seconds > 0.0
            && silence_ranges.iter().any(|(start, end)| *end > bar_start_seconds && *start < bar_end_seconds);
        let color = if is_silence {
            egui::Color32::from_rgb(238, 194, 74)
        } else if x1 <= progress_x {
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

    response
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


fn format_track_metadata_player_only(track: &Track) -> String {
    track
        .metadata
        .duration_seconds
        .map(format_duration)
        .unwrap_or_else(|| "?:??".to_owned())
}

fn next_row_pointer_hovered(ui: &egui::Ui, width: f32, height: f32) -> bool {
    let rect = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(width, height));
    ui.input(|input| {
        input
            .pointer
            .hover_pos()
            .map(|position| rect.contains(position))
            .unwrap_or(false)
    })
}

fn rendered_to_original_position(rendered_position: f32, render_start: f32, playback: &PlaybackInfo) -> f32 {
    if playback.silence_ranges.is_empty() {
        return rendered_position.clamp(0.0, playback.original_duration_seconds.max(0.0));
    }

    let mut original = rendered_position.max(render_start).max(0.0);
    for _ in 0..8 {
        let previous = original;
        for (start, end) in &playback.silence_ranges {
            let effective_start = (*start).max(render_start);
            if *end > effective_start && original >= effective_start && original < *end {
                original = *end;
            }
        }

        let skipped_before = playback
            .silence_ranges
            .iter()
            .filter_map(|(start, end)| {
                let effective_start = (*start).max(render_start);
                if *end <= effective_start || *end > original {
                    None
                } else {
                    Some(*end - effective_start)
                }
            })
            .sum::<f32>();

        original = (rendered_position + skipped_before).min(playback.original_duration_seconds.max(0.0));
        if (original - previous).abs() < 0.001 {
            break;
        }
    }

    original.clamp(0.0, playback.original_duration_seconds.max(0.0))
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


fn detail_row(ui: &mut egui::Ui, label: &str, value: &str) {
    let available_width = ui.available_width().max(260.0);
    let label_width = available_width.min(170.0);
    let value_width = (available_width - label_width - 14.0).max(120.0);

    ui.horizontal_top(|ui| {
        ui.set_width(available_width);
        ui.vertical(|ui| {
            ui.set_min_width(label_width);
            ui.set_max_width(label_width);
            ui.label(
                egui::RichText::new(label)
                    .strong()
                    .color(ui.visuals().widgets.inactive.fg_stroke.color.linear_multiply(0.72)),
            );
        });
        ui.vertical(|ui| {
            ui.set_min_width(value_width);
            ui.set_max_width(value_width);
            ui.add(egui::Label::new(value).wrap());
        });
    });
    ui.add_space(4.0);
}

fn reveal_in_file_manager(path: &Path) -> anyhow::Result<()> {
    let target = fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    let folder = if target.is_file() {
        target.parent().map(Path::to_path_buf).unwrap_or_else(|| target.clone())
    } else {
        target.clone()
    };

    #[cfg(windows)]
    {
        if target.is_file() {
            Command::new("explorer.exe")
                .arg(format!("/select,{}", target.display()))
                .spawn()?;
        } else {
            Command::new("explorer.exe")
                .arg(folder)
                .spawn()?;
        }
        return Ok(());
    }

    #[cfg(not(windows))]
    {
        Command::new("xdg-open").arg(folder).spawn()?;
        Ok(())
    }
}

fn open_url(url: &str) -> std::io::Result<()> {
    #[cfg(target_os = "windows")]
    {
        Command::new("cmd")
            .args(["/C", "start", "", url])
            .spawn()?;
        return Ok(());
    }

    #[cfg(target_os = "macos")]
    {
        Command::new("open").arg(url).spawn()?;
        return Ok(());
    }

    #[cfg(all(unix, not(target_os = "macos")))]
    {
        Command::new("xdg-open").arg(url).spawn()?;
        return Ok(());
    }
}
