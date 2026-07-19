#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_player;
mod config;
mod dj_mix;
mod dsp;
mod icon;
mod folder_watcher;
#[cfg(windows)]
mod file_associations;
mod media_keys;
mod single_instance;
mod spectrum_waveform;
mod ui_icons;
mod updater;
mod app;

#[cfg(debug_assertions)]
use crate::app::dev_metrics::{DevMetricsNativeWindowHandle, DevMetricsPanelState};

use crate::{
    audio_player::{current_default_output_device_name, AudioPlayer, PlaybackInfo, PreparedPlayback, RadioVisualizerFrame},
    config::{
        app_data_dir, app_version_label, default_backup_file_name, display_file_name, export_state_zip,
        import_state_zip, is_recursive_scan_link, is_supported_audio_file, load_state, path_is_same_or_descendant,
        path_key, same_path, save_state, scan_audio_folder, LastPlayedTrack,
        PlaybackSession, Playlist, PlaylistKind, RadioStation, RepeatMode, SavedState, Track, WindowGeometry, FAVORITES_PLAYLIST_NAME,
    },
    dsp::{DspSettings, OrbitMode},
};
use eframe::egui;
use lucide_icons::Icon;
use rfd::FileDialog;
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::Read,
    path::{Path, PathBuf},
    process::Command,
    sync::{atomic::AtomicBool, mpsc, Arc},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

// Radio waveform is a rolling live overview, not a zoomed-in oscilloscope.
// Keep scroll speed constant, but show enough seconds for real music dynamics to
// appear instead of a solid high-frequency block.
const RADIO_WAVEFORM_PIXELS_PER_SECOND: f32 = 30.0;
const RADIO_WAVEFORM_BAR_PITCH_PIXELS: f32 = 3.0;
const WAVEFORM_BAR_WIDTH_PIXELS: f32 = 1.0;
const RADIO_WAVEFORM_MAX_VISIBLE_SECONDS: f32 = 180.0;
const RADIO_METADATA_REFRESH_INTERVAL_SECONDS: u64 = 5;
const ACTIVE_INPUT_REPAINT_INTERVAL: Duration = Duration::from_millis(16);
const WAVEFORM_LOADING_REPAINT_INTERVAL: Duration = Duration::from_millis(80);
const RADIO_REPAINT_INTERVAL: Duration = Duration::from_millis(40);
const PLAYBACK_REPAINT_INTERVAL: Duration = Duration::from_millis(250);
const BACKGROUND_WORK_REPAINT_INTERVAL: Duration = Duration::from_millis(160);
const STATUS_REPAINT_INTERVAL: Duration = Duration::from_millis(500);
const IDLE_REPAINT_INTERVAL: Duration = Duration::from_millis(1000);
const FOLDER_WATCH_DEBOUNCE: Duration = Duration::from_millis(900);
const SEEK_PREPARE_DEBOUNCE: Duration = Duration::from_millis(700);
const FAST_SEEK_COALESCE_INTERVAL: Duration = Duration::from_millis(140);

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
    #[cfg(debug_assertions)]
    if let Some(config) = app::dev_metrics::dev_metrics_process_config_from_args() {
        return app::dev_metrics::run_dev_metrics_process(config);
    }

    let startup_audio_files = command_line_audio_files();
    let _single_instance_guard = match single_instance::acquire(&startup_audio_files) {
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
        &format!("Audio Orbit {}", app_version_label()),
        options,
        Box::new(move |creation_context| {
            ui_icons::install(&creation_context.egui_ctx);
            configure_app_style(&creation_context.egui_ctx);
            let mut app = AudioOrbitApp::new(state);
            if !startup_audio_files.is_empty() {
                app.open_audio_files_in_temporary_playlist(startup_audio_files.clone(), true);
            }
            Ok(Box::new(app))
        }),
    )
}

fn command_line_audio_files() -> Vec<PathBuf> {
    std::env::args_os()
        .skip(1)
        .map(PathBuf::from)
        .filter(|path| path.is_file() && is_supported_audio_file(path))
        .collect()
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
    playlist_index: usize,
    index: Option<usize>,
    info: PlaybackInfo,
}

struct PreparedTrackPlayback {
    playlist_index: usize,
    index: Option<usize>,
    crossfade_seconds: f32,
    live_position_compensation: bool,
    background_upgrade: bool,
    prepared: PreparedPlayback,
    requested_at: Instant,
}

#[derive(Clone, Debug)]
struct PendingSeekPrepare {
    run_after: Instant,
    requested_at: Instant,
    playlist_index: usize,
    index: Option<usize>,
    path: PathBuf,
    start_seconds: f32,
    settings: DspSettings,
    cached_waveform: Option<(Vec<f32>, Vec<f32>)>,
    cached_silence_ranges: Option<Vec<(f32, f32)>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct SilenceSettingsFingerprint {
    skip_silence_enabled: bool,
    trigger_millis: u16,
    threshold_db: i16,
    trim_end_regardless_of_duration: bool,
}

#[derive(Clone, Debug)]
struct SilenceAnalysisCacheEntry {
    file_len: Option<u64>,
    modified_nanos: Option<u128>,
    settings: SilenceSettingsFingerprint,
    ranges: Vec<(f32, f32)>,
}

#[derive(Clone, Debug)]
struct PendingFastSeek {
    run_after: Instant,
    playlist_index: usize,
    index: Option<usize>,
    path: PathBuf,
    position_seconds: f32,
    settings: DspSettings,
    known_duration_seconds: Option<f32>,
    prepare_after_streaming: bool,
}

#[derive(Clone, Debug)]
enum PendingFolderScanKind {
    Import {
        name: String,
        folder: PathBuf,
        depth: usize,
    },
}

#[derive(Clone, Debug)]
struct PendingFolderScanResult {
    kind: PendingFolderScanKind,
    files: Vec<PathBuf>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum LibrarySyncTrigger {
    Startup,
    Manual,
    Automatic,
}

#[derive(Clone, Debug)]
struct PendingFolderWatchChange {
    path: PathBuf,
    scan_directory_if_present: bool,
}

#[derive(Clone, Debug)]
struct FolderLibrarySyncTarget {
    playlist_index: usize,
    playlist_name: String,
    source_folder: PathBuf,
}

#[derive(Clone, Debug)]
enum FolderLibrarySyncOutcome {
    Scanned {
        files: Arc<Vec<PathBuf>>,
    },
    Incremental {
        present_files: Arc<Vec<PathBuf>>,
        missing_roots: Arc<Vec<PathBuf>>,
        errors: Vec<String>,
    },
}

#[derive(Clone, Debug)]
struct FolderLibrarySyncResult {
    target: FolderLibrarySyncTarget,
    outcome: Result<FolderLibrarySyncOutcome, String>,
}

#[derive(Clone, Debug)]
struct PendingLibrarySyncResult {
    trigger: LibrarySyncTrigger,
    playlist_index: usize,
    playlist_name: String,
    playlist_kind: PlaylistKind,
    source_folder: Option<PathBuf>,
    availability: BTreeMap<String, bool>,
    folder_results: Vec<FolderLibrarySyncResult>,
}


#[derive(Clone, Debug)]
enum DetailsModal {
    Track(PathBuf),
    Radio(usize),
}

#[derive(Clone, Debug)]
struct PendingTrackDeleteConfirmation {
    paths: Vec<PathBuf>,
    title: String,
    description: String,
}

#[derive(Clone, Debug)]
enum TrackFileOperationResult {
    Copy {
        destination: PathBuf,
        requested: usize,
        copied: usize,
        skipped_missing: usize,
        errors: Vec<String>,
    },
    Delete {
        requested: usize,
        deleted: usize,
        already_missing: usize,
        removed_paths: Vec<PathBuf>,
        errors: Vec<String>,
    },
}

#[derive(Clone, Debug)]
struct DjMixTrack {
    path: PathBuf,
    title: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DjMixStyle {
    Crossfade,
    SmartDj,
}

#[derive(Clone, Copy, Debug)]
struct DjMixOptions {
    style: DjMixStyle,
    smart_order: bool,
    normalize_loudness: bool,
    bass_swap: bool,
    transition_beats: u32,
    bitrate_kbps: u32,
}

impl Default for DjMixOptions {
    fn default() -> Self {
        Self {
            style: DjMixStyle::SmartDj,
            smart_order: true,
            normalize_loudness: true,
            bass_swap: true,
            transition_beats: 16,
            bitrate_kbps: 256,
        }
    }
}

#[derive(Clone, Debug)]
struct DjMixModalState {
    tracks: Vec<DjMixTrack>,
    options: DjMixOptions,
    stage: String,
    progress: f32,
    output_path: Option<PathBuf>,
    completed: bool,
}

#[derive(Clone, Debug)]
enum DjMixEvent {
    Progress {
        stage: String,
        progress: f32,
    },
    Completed {
        output_path: PathBuf,
        track_count: usize,
    },
    Cancelled,
    Failed(String),
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
            Self::Updates => "Check GitHub releases, install a newer Windows build, or open the release page.",
            Self::Backup => "Export and import the complete Audio Orbit state, including folders, playlists, radio stations, profiles, playback, and UI settings.",
            Self::About => "Purpose, licensing, author information, and app shortcuts.",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Self::Settings => Icon::Settings2,
            Self::Updates => Icon::RefreshCw,
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

#[derive(Clone, Debug)]
enum OrderUndoSnapshot {
    Playlist {
        playlist_index: usize,
        playlist: Playlist,
        selected_track_index: Option<usize>,
        selected_track_indexes: BTreeSet<usize>,
        active_playlist_index: Option<usize>,
        active_track_index: Option<usize>,
        active_track_path: Option<PathBuf>,
    },
    Radio {
        stations: Vec<RadioStation>,
        selected_radio_index: Option<usize>,
        active_radio_index: Option<usize>,
        radio_selection_was_user_set: bool,
    },
}

struct AudioOrbitApp {
    player: Option<AudioPlayer>,
    state: SavedState,
    selected_track_index: Option<usize>,
    selected_track_indexes: BTreeSet<usize>,
    multi_selected_track_indexes: BTreeSet<usize>,
    track_selection_anchor_index: Option<usize>,
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
    pending_seek_prepare: Option<PendingSeekPrepare>,
    pending_fast_seek: Option<PendingFastSeek>,
    last_fast_seek_started_at: Option<Instant>,
    silence_analysis_cache: BTreeMap<PathBuf, SilenceAnalysisCacheEntry>,
    pending_folder_scan_receiver: Option<mpsc::Receiver<Result<PendingFolderScanResult, String>>>,
    pending_library_sync_receiver: Option<mpsc::Receiver<PendingLibrarySyncResult>>,
    pending_track_file_operation_receiver: Option<mpsc::Receiver<TrackFileOperationResult>>,
    dj_mix_event_receiver: Option<mpsc::Receiver<DjMixEvent>>,
    dj_mix_cancel_flag: Option<Arc<AtomicBool>>,
    folder_watcher: Option<folder_watcher::FolderWatcher>,
    folder_watcher_target_key: Option<String>,
    pending_folder_watch_sync_at: Option<Instant>,
    pending_folder_watch_paths: BTreeMap<String, PendingFolderWatchChange>,
    pending_folder_watch_full_rescan: bool,
    pending_profile_apply_at: Option<Instant>,
    profile_apply_applied_until: Option<Instant>,
    waveform_drag_position_seconds: Option<f32>,
    suppress_window_geometry_save_until: Option<Instant>,
    show_folder_import_modal: bool,
    show_radio_add_modal: bool,
    show_new_playlist_modal: bool,
    pending_new_playlist_name: String,
    pending_new_playlist_tracks: Vec<PathBuf>,
    pending_track_delete_confirmation: Option<PendingTrackDeleteConfirmation>,
    pending_track_delete_confirmation_text: String,
    dj_mix_modal: Option<DjMixModalState>,
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
    scroll_to_track_path_requested: Option<PathBuf>,
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
    dragging_track_index: Option<usize>,
    dragging_radio_index: Option<usize>,
    track_drop_target_index: Option<usize>,
    radio_drop_target_index: Option<usize>,
    order_undo_stack: Vec<OrderUndoSnapshot>,
    collapsed_groups: BTreeSet<String>,
    pending_folder_path: Option<PathBuf>,
    pending_playlist_name: String,
    pending_folder_depth: usize,
    last_known_output_name: String,
    detected_output_change: Option<String>,
    last_output_check: Instant,
    editing_playlist_index: Option<usize>,
    editing_profile_index: Option<usize>,
    pending_clipboard_text: Option<String>,
    media_key_receiver: Option<mpsc::Receiver<media_keys::MediaKeyEvent>>,
    media_key_status: String,
    update_check_receiver: Option<mpsc::Receiver<Result<updater::UpdateCheck, String>>>,
    update_install_receiver: Option<mpsc::Receiver<Result<(), String>>>,
    last_update_check: Option<updater::UpdateCheck>,
    update_check_started_at: Option<Instant>,
    update_install_started_at: Option<Instant>,
    last_external_open_request_poll: Instant,
    #[cfg(windows)]
    file_associations_registered: bool,
    #[cfg(debug_assertions)]
    dev_metrics: DevMetricsPanelState,
    #[cfg(debug_assertions)]
    show_dev_metrics_window: bool,
    #[cfg(debug_assertions)]
    dev_metrics_window: Option<DevMetricsNativeWindowHandle>,
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
        .map(|value| decode_radio_text_bytes(value.as_bytes()))
        .map(|value| clean_radio_metadata_value(&value))
        .filter(|value| !value.is_empty());

    let stream_title = headers
        .get("icy-title")
        .map(|value| decode_radio_text_bytes(value.as_bytes()))
        .map(|value| clean_radio_metadata_value(&value))
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

    // Some stations return an empty first ICY metadata block. Read a few blocks
    // from the metadata request so the visible title can update while radio keeps
    // playing, without mixing ICY bytes into the playback stream.
    let mut audio_buffer = vec![0_u8; metadata_interval];
    for _ in 0..6 {
        reader.read_exact(&mut audio_buffer).ok()?;

        let mut length_byte = [0_u8; 1];
        reader.read_exact(&mut length_byte).ok()?;
        let metadata_length = length_byte[0] as usize * 16;
        if metadata_length == 0 {
            continue;
        }
        if metadata_length > 4096 {
            return None;
        }

        let mut metadata = vec![0_u8; metadata_length];
        reader.read_exact(&mut metadata).ok()?;
        let metadata = decode_radio_text_bytes(&metadata);
        if let Some(title) = parse_icy_stream_title(&metadata) {
            return Some(title);
        }
    }

    None
}

fn playlist_scroll_key(index: usize, playlist: &Playlist) -> String {
    let group = playlist.selected_group.as_deref().unwrap_or("__all__");
    let source = playlist
        .source_folder
        .as_ref()
        .map(|path| path.to_string_lossy().into_owned())
        .unwrap_or_default();
    format!("{index}|{}|{}|{}", playlist.name, source, group)
}

fn decode_radio_text_bytes(bytes: &[u8]) -> String {
    if let Ok(value) = std::str::from_utf8(bytes) {
        return value.to_owned();
    }

    bytes
        .iter()
        .map(|byte| match *byte {
            0x80 => '€',
            0x82 => '‚',
            0x84 => '„',
            0x85 => '…',
            0x86 => '†',
            0x87 => '‡',
            0x89 => '‰',
            0x8A => 'Š',
            0x8B => '‹',
            0x8C => 'Ś',
            0x8D => 'Ť',
            0x8E => 'Ž',
            0x8F => 'Ź',
            0x91 => '‘',
            0x92 => '’',
            0x93 => '“',
            0x94 => '”',
            0x95 => '•',
            0x96 => '–',
            0x97 => '—',
            0x99 => '™',
            0x9A => 'š',
            0x9B => '›',
            0x9C => 'ś',
            0x9D => 'ť',
            0x9E => 'ž',
            0x9F => 'ź',
            0xA1 => 'ˇ',
            0xA2 => '˘',
            0xA3 => 'Ł',
            0xA5 => 'Ą',
            0xAA => 'Ş',
            0xAF => 'Ż',
            0xB2 => '˛',
            0xB3 => 'ł',
            0xB9 => 'ą',
            0xBA => 'ş',
            0xBC => 'Ľ',
            0xBD => '˝',
            0xBE => 'ľ',
            0xBF => 'ż',
            0xC0 => 'Ŕ',
            0xC1 => 'Á',
            0xC2 => 'Â',
            0xC3 => 'Ă',
            0xC4 => 'Ä',
            0xC5 => 'Ĺ',
            0xC6 => 'Ć',
            0xC7 => 'Ç',
            0xC8 => 'Č',
            0xC9 => 'É',
            0xCA => 'Ę',
            0xCB => 'Ë',
            0xCC => 'Ě',
            0xCD => 'Í',
            0xCE => 'Î',
            0xCF => 'Ď',
            0xD0 => 'Đ',
            0xD1 => 'Ń',
            0xD2 => 'Ň',
            0xD3 => 'Ó',
            0xD4 => 'Ô',
            0xD5 => 'Ő',
            0xD6 => 'Ö',
            0xD7 => '×',
            0xD8 => 'Ř',
            0xD9 => 'Ů',
            0xDA => 'Ú',
            0xDB => 'Ű',
            0xDC => 'Ü',
            0xDD => 'Ý',
            0xDE => 'Ţ',
            0xDF => 'ß',
            0xE0 => 'ŕ',
            0xE1 => 'á',
            0xE2 => 'â',
            0xE3 => 'ă',
            0xE4 => 'ä',
            0xE5 => 'ĺ',
            0xE6 => 'ć',
            0xE7 => 'ç',
            0xE8 => 'č',
            0xE9 => 'é',
            0xEA => 'ę',
            0xEB => 'ë',
            0xEC => 'ě',
            0xED => 'í',
            0xEE => 'î',
            0xEF => 'ď',
            0xF0 => 'đ',
            0xF1 => 'ń',
            0xF2 => 'ň',
            0xF3 => 'ó',
            0xF4 => 'ô',
            0xF5 => 'ő',
            0xF6 => 'ö',
            0xF7 => '÷',
            0xF8 => 'ř',
            0xF9 => 'ů',
            0xFA => 'ú',
            0xFB => 'ű',
            0xFC => 'ü',
            0xFD => 'ý',
            0xFE => 'ţ',
            0xFF => '˙',
            0x00..=0x7F => *byte as char,
            _ => ' ',
        })
        .collect()
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


fn search_icon_text_button(ui: &mut egui::Ui, enabled: bool, icon: Icon, text: &str) -> egui::Response {
    let icon_text = ui_icons::icon(icon);
    let icon_font = egui::FontId::proportional(14.0);
    let text_font = egui::TextStyle::Button.resolve(ui.style());
    let button_height = ui.spacing().interact_size.y;
    let horizontal_padding = ui.spacing().button_padding.x.max(6.0);
    let icon_width = text_width(
        ui,
        &icon_text,
        icon_font.clone(),
        ui.visuals().widgets.inactive.fg_stroke.color,
    ).ceil();
    let text_width = text_width(
        ui,
        text,
        text_font.clone(),
        ui.visuals().widgets.inactive.fg_stroke.color,
    ).ceil();
    let icon_gap = 5.0;
    let button_width = (horizontal_padding * 2.0 + icon_width + icon_gap + text_width).ceil().max(44.0);

    let response = ui.add_enabled(
        enabled,
        egui::Button::new("").min_size(egui::vec2(button_width, button_height)),
    );
    let visuals = ui.style().interact(&response);
    let text_color = if enabled {
        visuals.fg_stroke.color
    } else {
        ui.visuals().weak_text_color()
    };
    let icon_left = response.rect.left() + horizontal_padding;
    let text_left = icon_left + icon_width + icon_gap;
    let center_y = response.rect.center().y;

    ui.painter().text(
        egui::pos2(icon_left, center_y),
        egui::Align2::LEFT_CENTER,
        icon_text,
        icon_font,
        text_color,
    );
    ui.painter().text(
        egui::pos2(text_left, center_y),
        egui::Align2::LEFT_CENTER,
        text,
        text_font,
        text_color,
    );

    response
}

fn render_ellipsized_single_line(
    ui: &mut egui::Ui,
    value: &str,
    width: f32,
    font_id: egui::FontId,
    color: egui::Color32,
) -> egui::Response {
    let trimmed = value.trim();
    let desired_size = egui::vec2(width, ui.spacing().interact_size.y);
    if trimmed.is_empty() {
        return ui.allocate_response(desired_size, egui::Sense::hover());
    }

    let clipped = ellipsize_to_width_exact(ui, trimmed, width, font_id.clone(), color);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    ui.painter().text(
        rect.left_center(),
        egui::Align2::LEFT_CENTER,
        clipped.as_str(),
        font_id,
        color,
    );

    if clipped != trimmed {
        return response.on_hover_text(trimmed);
    }
    response
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

fn normalize_search_text(value: &str) -> String {
    value
        .chars()
        .map(|ch| {
            if ch.is_alphanumeric() {
                ch.to_lowercase().next().unwrap_or(ch)
            } else {
                ' '
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn track_matches_search_query(track: &Track, query: &str) -> bool {
    let query = normalize_search_text(query);
    if query.is_empty() {
        return true;
    }

    let haystack = normalize_search_text(&format!(
        "{} {} {} {}",
        track.title,
        track.group,
        track.path.display(),
        display_parent(&track.path)
    ));

    query
        .split_whitespace()
        .all(|token| haystack.contains(token))
}

fn naturalish_key(value: &str) -> String {
    normalize_search_text(value)
}

fn same_text(left: &str, right: &str) -> bool {
    left.eq_ignore_ascii_case(right)
}

fn ensure_state_is_valid(state: &mut SavedState) {
    state
        .playlists
        .retain(|playlist| playlist.kind != PlaylistKind::Temporary);
    if !state.playlists.iter().any(|playlist| playlist.kind == PlaylistKind::Favorites) {
        state.playlists.insert(0, Playlist::favorites());
    }

    for playlist in &mut state.playlists {
        if playlist.name == FAVORITES_PLAYLIST_NAME {
            playlist.kind = PlaylistKind::Favorites;
        } else if playlist.source_folder.is_some() {
            playlist.kind = PlaylistKind::Folder;
        }
        playlist.ensure_favorite_added_sequences();
        playlist.ensure_folder_group_contiguity();
        playlist.set_selected_group(playlist.selected_group.clone());
        let track_paths: Vec<PathBuf> = playlist.tracks.iter().map(|track| track.path.clone()).collect();
        playlist
            .repeat_selection
            .retain(|selected_path| track_paths.iter().any(|track_path| same_path(track_path, selected_path)));
        let mut deduped_repeat_selection: Vec<PathBuf> = Vec::new();
        for selected_path in playlist.repeat_selection.drain(..) {
            if !deduped_repeat_selection
                .iter()
                .any(|existing_path| same_path(existing_path.as_path(), selected_path.as_path()))
            {
                deduped_repeat_selection.push(selected_path);
            }
        }
        playlist.repeat_selection = deduped_repeat_selection;
    }

    if state.playlists.is_empty() {
        state.playlists.push(Playlist::favorites());
        state.playlists.push(Playlist::new("Local music"));
    }
    if state.selected_playlist_index >= state.playlists.len() {
        state.selected_playlist_index = 0;
    }
    state.playlists.push(Playlist::temporary());

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


    if !state.playback_session.position_seconds.is_finite() || state.playback_session.position_seconds < 0.0 {
        state.playback_session.position_seconds = 0.0;
    }
    if !matches!(state.playback_session.source.as_str(), "music" | "track" | "radio") {
        state.playback_session = PlaybackSession::default();
    }
    if let Some(index) = state.playback_session.radio_index {
        if index >= state.radio_stations.len() {
            state.playback_session.radio_index = None;
            state.playback_session.was_active = false;
        }
    }

    if !state.ui.playlist_scroll_offset_y.is_finite() || state.ui.playlist_scroll_offset_y < 0.0 {
        state.ui.playlist_scroll_offset_y = 0.0;
    }
    state
        .ui
        .playlist_scroll_offsets
        .retain(|_, offset| offset.is_finite() && *offset >= 0.0);
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

fn paint_dragged_row_fade(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, egui::Color32::from_black_alpha(150));
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, ui.visuals().widgets.inactive.bg_stroke.color.linear_multiply(0.70)),
        egui::StrokeKind::Inside,
    );
}

fn paint_list_separator_line(ui: &egui::Ui, left: f32, right: f32, y: f32, highlighted: bool) {
    let color = if highlighted {
        egui::Color32::from_rgb(78, 148, 255)
    } else {
        ui.visuals().widgets.noninteractive.bg_stroke.color
    };
    let stroke_width = if highlighted { 2.0 } else { 1.0 };
    let y = y.round() + 0.5;
    ui.painter().line_segment(
        [egui::pos2(left, y), egui::pos2(right, y)],
        egui::Stroke::new(stroke_width, color),
    );
}

fn paint_list_separator(ui: &mut egui::Ui, width: f32, highlighted: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 1.0), egui::Sense::hover());
    paint_list_separator_line(ui, rect.left(), rect.right(), rect.center().y, highlighted);
}

fn paint_list_edge_separator(ui: &egui::Ui, row_rect: egui::Rect, width: f32, after: bool) {
    let y = if after { row_rect.bottom() } else { row_rect.top() - 1.0 };
    paint_list_separator_line(ui, row_rect.left(), row_rect.left() + width, y, true);
}

fn draw_radio_waveform_strip(ui: &mut egui::Ui, frame: &RadioVisualizerFrame) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width().max(96.0).floor(), 46.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::hover());
    let painter = ui.painter();

    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(210));

    if frame.bars.is_empty() {
        return response;
    }

    let center_y = rect.center().y.round();
    let live_color = egui::Color32::from_rgb(78, 148, 255);
    let start_x = rect.left().round() + 0.5;
    let right_x = rect.right().round() - 0.5;
    let stroke = egui::Stroke::new(WAVEFORM_BAR_WIDTH_PIXELS, live_color);

    for (bar_index, bar) in frame.bars.iter().enumerate() {
        let x = start_x + bar_index as f32 * RADIO_WAVEFORM_BAR_PITCH_PIXELS;
        if x > right_x {
            break;
        }

        let value = bar.peak.clamp(0.0, 1.0);
        if value <= 0.004 {
            continue;
        }

        let eased = value.powf(1.12);
        let height = (rect.height() * 0.72 * eased)
            .max(1.5)
            .min(rect.height() - 5.0);
        painter.line_segment(
            [
                egui::pos2(x, center_y - height * 0.5),
                egui::pos2(x, center_y + height * 0.5),
            ],
            stroke,
        );
    }

    response
}

fn sample_waveform_column(waveform: &[f32], column: usize, columns: usize) -> f32 {
    if waveform.is_empty() || columns == 0 {
        return 0.0;
    }

    let len = waveform.len();
    let start = ((column as f32 / columns as f32) * len as f32)
        .floor()
        .clamp(0.0, len.saturating_sub(1) as f32) as usize;
    let end = (((column + 1) as f32 / columns as f32) * len as f32)
        .ceil()
        .clamp((start + 1) as f32, len as f32) as usize;
    let slice = &waveform[start..end];
    let stride = (slice.len() / 24).max(1);
    slice.iter().step_by(stride).copied().fold(0.0_f32, f32::max)
}

fn draw_waveform_seek(
    ui: &mut egui::Ui,
    waveform: &[f32],
    _waveform_brightness: &[f32],
    progress: f32,
    silence_ranges: &[(f32, f32)],
    duration_seconds: f32,
    show_loading_wave: bool,
) -> egui::Response {
    let desired_size = egui::vec2(ui.available_width().max(96.0).floor(), 46.0);
    let (rect, response) = ui.allocate_exact_size(desired_size, egui::Sense::click_and_drag());
    let painter = ui.painter();

    painter.rect_filled(rect, 0.0, egui::Color32::from_black_alpha(210));

    if waveform.is_empty() {
        if show_loading_wave {
            paint_waveform_loading_wave(ui, rect);
            ui.ctx().request_repaint_after(WAVEFORM_LOADING_REPAINT_INTERVAL);
        }
        return response;
    }

    let progress = progress.clamp(0.0, 1.0);
    let progress_x = rect.left() + rect.width() * progress;
    let column_count = (rect.width() / RADIO_WAVEFORM_BAR_PITCH_PIXELS)
        .floor()
        .max(1.0) as usize + 1;
    let mut values = Vec::with_capacity(column_count);
    for column in 0..column_count {
        values.push(sample_waveform_column(waveform, column, column_count));
    }
    let peak = values.iter().copied().fold(0.0_f32, f32::max).max(0.08);
    let dynamic_range = peak.max(0.05);

    let unplayed_color = egui::Color32::from_rgb(92, 98, 110);
    let played_color = egui::Color32::from_rgb(78, 148, 255);
    let silence_color = egui::Color32::from_rgb(238, 194, 74);
    let center_y = rect.center().y.round();
    let start_x = rect.left().round() + 0.5;
    let right_x = rect.right().round() - 0.5;

    for (bar_index, value) in values.iter().enumerate() {
        let normalized = (*value / dynamic_range).clamp(0.018, 1.0);
        let eased = normalized.powf(1.08);
        let x = start_x + bar_index as f32 * RADIO_WAVEFORM_BAR_PITCH_PIXELS;
        if x > right_x {
            break;
        }

        let height = (rect.height() * 0.84 * eased).max(2.0).min(rect.height() - 4.0);
        let bar_start_seconds = if duration_seconds > 0.0 {
            bar_index as f32 / column_count.max(1) as f32 * duration_seconds
        } else {
            0.0
        };
        let bar_end_seconds = if duration_seconds > 0.0 {
            (bar_index + 1) as f32 / column_count.max(1) as f32 * duration_seconds
        } else {
            0.0
        };
        let is_silence = duration_seconds > 0.0
            && silence_ranges.iter().any(|(start, end)| *end > bar_start_seconds && *start < bar_end_seconds);

        let color = if is_silence {
            silence_color
        } else if x <= progress_x {
            played_color
        } else {
            unplayed_color
        };
        painter.line_segment(
            [
                egui::pos2(x, center_y - height * 0.5),
                egui::pos2(x, center_y + height * 0.5),
            ],
            egui::Stroke::new(WAVEFORM_BAR_WIDTH_PIXELS, color),
        );
    }

    if response.dragged() {
        painter.line_segment(
            [
                egui::pos2(progress_x.round() + 0.5, rect.top() + 4.0),
                egui::pos2(progress_x.round() + 0.5, rect.bottom() - 4.0),
            ],
            egui::Stroke::new(1.0, egui::Color32::WHITE.linear_multiply(0.75)),
        );
    }

    response
}


fn paint_waveform_loading_wave(ui: &egui::Ui, rect: egui::Rect) {
    let painter = ui.painter();
    let time = ui.input(|input| input.time) as f32;
    let center_y = rect.center().y.round();
    let left = rect.left() + 10.0;
    let right = rect.right() - 10.0;
    let width = (right - left).max(1.0);
    let amplitude = (rect.height() * 0.18).clamp(4.0, 10.0);
    let points = 72usize;
    let base_color = egui::Color32::from_rgb(72, 86, 108).linear_multiply(0.72);
    let accent_color = egui::Color32::from_rgb(78, 148, 255).linear_multiply(0.85);

    let mut previous: Option<egui::Pos2> = None;
    for index in 0..points {
        let t = index as f32 / (points.saturating_sub(1).max(1)) as f32;
        let x = left + width * t;
        let phase = t * std::f32::consts::TAU * 2.2 + time * 3.2;
        let envelope = (std::f32::consts::PI * t).sin().clamp(0.0, 1.0);
        let y = center_y + phase.sin() * amplitude * envelope;
        let current = egui::pos2(x, y);
        if let Some(previous) = previous {
            painter.line_segment([previous, current], egui::Stroke::new(1.0, base_color));
        }
        previous = Some(current);
    }

    let pulse_center = (time * 0.42).fract();
    let pulse_half_width = 0.18;
    let mut previous: Option<egui::Pos2> = None;
    for index in 0..points {
        let t = index as f32 / (points.saturating_sub(1).max(1)) as f32;
        let distance = (t - pulse_center).abs().min((t - pulse_center + 1.0).abs()).min((t - pulse_center - 1.0).abs());
        if distance > pulse_half_width {
            previous = None;
            continue;
        }
        let x = left + width * t;
        let phase = t * std::f32::consts::TAU * 2.2 + time * 3.2;
        let envelope = (std::f32::consts::PI * t).sin().clamp(0.0, 1.0);
        let y = center_y + phase.sin() * amplitude * envelope;
        let current = egui::pos2(x, y);
        if let Some(previous) = previous {
            painter.line_segment([previous, current], egui::Stroke::new(1.25, accent_color));
        }
        previous = Some(current);
    }
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
    let target = if path.is_absolute() {
        path.to_path_buf()
    } else {
        fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
    };
    let looks_like_file = target.is_file() || (!target.is_dir() && path.extension().is_some());
    let folder = if looks_like_file {
        target.parent().map(Path::to_path_buf).unwrap_or_else(|| target.clone())
    } else {
        target.clone()
    };

    #[cfg(windows)]
    {
        let explorer_folder = explorer_compatible_path(&folder);
        if looks_like_file {
            let explorer_target = explorer_compatible_path(&target);
            Command::new("explorer.exe")
                .current_dir(&explorer_folder)
                .arg("/select,")
                .arg(&explorer_target)
                .spawn()?;
        } else {
            Command::new("explorer.exe")
                .current_dir(&explorer_folder)
                .arg(&explorer_folder)
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

#[cfg(windows)]
fn explorer_compatible_path(path: &Path) -> PathBuf {
    let path_text = path.as_os_str().to_string_lossy();
    if let Some(stripped) = path_text.strip_prefix(r"\\?\UNC\") {
        return PathBuf::from(format!(r"\\{stripped}"));
    }
    if let Some(stripped) = path_text.strip_prefix(r"\\?\") {
        return PathBuf::from(stripped);
    }
    path.to_path_buf()
}

