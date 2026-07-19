use crate::dsp::{DspSettings, OrbitMode};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use lofty::file::AudioFile;
use lucide_icons::Icon;
use serde::{Deserialize, Serialize};
use std::{
    cmp::Reverse,
    collections::{BTreeMap, BTreeSet},
    fs::{self, File},
    io::{Read, Write},
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use zip::{write::SimpleFileOptions, ZipArchive, ZipWriter};

pub const FAVORITES_PLAYLIST_NAME: &str = "Favorites";
pub const TEMPORARY_PLAYLIST_NAME: &str = "Temporary playback";
const BACKUP_STATE_ENTRY: &str = "audio-orbit/state.json";
const BACKUP_META_ENTRY: &str = "audio-orbit/backup.json";

pub fn app_version_label() -> &'static str {
    if cfg!(debug_assertions) {
        "dev"
    } else {
        concat!("v", env!("CARGO_PKG_VERSION"))
    }
}

pub fn default_backup_file_name() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = utc_timestamp_parts(seconds);
    let version = sanitize_file_name_segment(app_version_label());
    format!(
        "audio-orbit-backup-{version}-{year:04}-{month:02}-{day:02}-{hour:02}-{minute:02}-{second:02}.zip"
    )
}

fn sanitize_file_name_segment(value: &str) -> String {
    let sanitized = value
        .chars()
        .map(|character| match character {
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*' => '-',
            character if character.is_control() => '-',
            character => character,
        })
        .collect::<String>()
        .trim_matches(|character| character == '.' || character == ' ')
        .to_owned();

    if sanitized.is_empty() {
        "unknown".to_owned()
    } else {
        sanitized
    }
}

fn utc_timestamp_parts(seconds: u64) -> (i32, u32, u32, u32, u32, u32) {
    let days = (seconds / 86_400) as i64;
    let seconds_of_day = seconds % 86_400;
    let hour = (seconds_of_day / 3_600) as u32;
    let minute = ((seconds_of_day % 3_600) / 60) as u32;
    let second = (seconds_of_day % 60) as u32;

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let mut year = yoe as i32 + era as i32 * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = (mp + if mp < 10 { 3 } else { -9 }) as u32;
    if month <= 2 {
        year += 1;
    }

    (year, month, day, hour, minute, second)
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaylistKind {
    Favorites,
    Manual,
    Folder,
    Temporary,
}

impl Default for PlaylistKind {
    fn default() -> Self {
        Self::Manual
    }
}

impl PlaylistKind {
    pub fn icon(&self) -> String {
        let icon = match self {
            Self::Favorites => Icon::Heart,
            Self::Manual => Icon::ListMusic,
            Self::Folder => Icon::Folder,
            Self::Temporary => Icon::ListMusic,
        };

        char::from(icon).to_string()
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Favorites => "Favorites",
            Self::Manual => "Manual playlist",
            Self::Folder => "Folder playlist",
            Self::Temporary => "Temporary playback",
        }
    }

    pub fn accepts_manual_tracks(&self) -> bool {
        matches!(self, Self::Favorites | Self::Manual)
    }

    pub fn can_delete(&self) -> bool {
        !matches!(self, Self::Favorites | Self::Temporary)
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct TrackMetadata {
    pub size_bytes: Option<u64>,
    pub duration_seconds: Option<f32>,
    pub sample_rate_hz: Option<u32>,
    pub channels: Option<u8>,
    pub bitrate_kbps: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub group: String,
    #[serde(default)]
    pub metadata: TrackMetadata,
    #[serde(default)]
    pub waveform: Vec<f32>,
    #[serde(default)]
    pub waveform_brightness: Vec<f32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub favorite_added_sequence: Option<u64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub missing: bool,
}

impl Track {
    pub fn from_path(path: PathBuf, root: Option<&Path>, folder_depth: usize) -> Self {
        let title = path
            .file_stem()
            .and_then(|name| name.to_str())
            .filter(|name| !name.trim().is_empty())
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| display_file_name(&path));

        let group = folder_group_for_path(&path, root, folder_depth);
        // Keep large folder imports responsive: expensive decoder/tag metadata is filled
        // lazily from playback results instead of being read for every scanned file.
        let file_metadata = fs::metadata(&path).ok().filter(|metadata| metadata.is_file());
        let metadata = TrackMetadata {
            size_bytes: file_metadata.as_ref().map(|metadata| metadata.len()),
            ..Default::default()
        };

        Self {
            path,
            title,
            group,
            metadata,
            waveform: Vec::new(),
            waveform_brightness: Vec::new(),
            favorite_added_sequence: None,
            missing: file_metadata.is_none(),
        }
    }

    pub fn update_playback_metadata(
        &mut self,
        duration_seconds: f32,
        sample_rate_hz: u32,
        channels: u16,
        waveform: Vec<f32>,
        waveform_brightness: Vec<f32>,
    ) {
        self.metadata.duration_seconds = Some(duration_seconds);
        self.metadata.sample_rate_hz = Some(sample_rate_hz);
        self.metadata.channels = Some(channels.min(u8::MAX as u16) as u8);
        self.missing = false;
        if self.metadata.size_bytes.is_none() {
            self.metadata.size_bytes = fs::metadata(&self.path).ok().map(|metadata| metadata.len());
        }
        if !waveform.is_empty() && !waveform_brightness.is_empty() {
            self.waveform = waveform;
            self.waveform_brightness = waveform_brightness;
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RadioStation {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub last_station_name: Option<String>,
    #[serde(default)]
    pub last_stream_title: Option<String>,
    #[serde(default)]
    pub favorite: bool,
}

impl RadioStation {
    pub fn new(name: impl Into<String>, url: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            url: url.into(),
            last_station_name: None,
            last_stream_title: None,
            favorite: false,
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct LastPlayedTrack {
    pub playlist_index: usize,
    pub track_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaybackSession {
    #[serde(default)]
    pub was_active: bool,
    #[serde(default)]
    pub was_paused: bool,
    #[serde(default = "default_playback_session_source")]
    pub source: String,
    #[serde(default)]
    pub playlist_index: Option<usize>,
    #[serde(default)]
    pub track_path: Option<PathBuf>,
    #[serde(default)]
    pub position_seconds: f32,
    #[serde(default)]
    pub radio_index: Option<usize>,
}

impl Default for PlaybackSession {
    fn default() -> Self {
        Self {
            was_active: false,
            was_paused: false,
            source: default_playback_session_source(),
            playlist_index: None,
            track_path: None,
            position_seconds: 0.0,
            radio_index: None,
        }
    }
}

fn default_playback_session_source() -> String {
    "music".to_owned()
}


#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackAvailabilityStats {
    pub restored: usize,
    pub newly_missing: usize,
    pub missing_total: usize,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct FolderSyncStats {
    pub added: usize,
    pub restored: usize,
    pub newly_missing: usize,
    pub missing_total: usize,
    pub present_total: usize,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<Track>,
    pub source_folder: Option<PathBuf>,
    pub folder_depth: usize,
    pub selected_group: Option<String>,
    #[serde(default)]
    pub repeat_selection: Vec<PathBuf>,
    #[serde(default)]
    pub kind: PlaylistKind,
}

impl Playlist {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tracks: Vec::new(),
            source_folder: None,
            folder_depth: 2,
            selected_group: None,
            repeat_selection: Vec::new(),
            kind: PlaylistKind::Manual,
        }
    }

    pub fn favorites() -> Self {
        Self {
            name: FAVORITES_PLAYLIST_NAME.to_owned(),
            tracks: Vec::new(),
            source_folder: None,
            folder_depth: 0,
            selected_group: None,
            repeat_selection: Vec::new(),
            kind: PlaylistKind::Favorites,
        }
    }

    pub fn temporary() -> Self {
        Self {
            name: TEMPORARY_PLAYLIST_NAME.to_owned(),
            tracks: Vec::new(),
            source_folder: None,
            folder_depth: 0,
            selected_group: None,
            repeat_selection: Vec::new(),
            kind: PlaylistKind::Temporary,
        }
    }

    pub fn add_temporary_files(&mut self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        if self.kind != PlaylistKind::Temporary {
            return Vec::new();
        }

        let mut added = Vec::new();
        for path in files {
            if self.tracks.iter().any(|track| same_path(&track.path, &path)) {
                continue;
            }
            self.tracks.push(Track::from_path(path.clone(), None, 0));
            added.push(path);
        }
        added
    }

    pub fn from_folder(
        name: impl Into<String>,
        source_folder: PathBuf,
        folder_depth: usize,
        files: Vec<PathBuf>,
    ) -> Self {
        let mut playlist = Self {
            name: name.into(),
            tracks: Vec::new(),
            source_folder: Some(source_folder),
            folder_depth,
            selected_group: None,
            repeat_selection: Vec::new(),
            kind: PlaylistKind::Folder,
        };
        playlist.replace_tracks_from_files(files);
        playlist
    }

    pub fn accepts_manual_tracks(&self) -> bool {
        self.kind.accepts_manual_tracks()
    }

    pub fn add_files(&mut self, files: Vec<PathBuf>) -> Vec<PathBuf> {
        if !self.accepts_manual_tracks() {
            return Vec::new();
        }

        let root = self.source_folder.clone();
        let folder_depth = self.folder_depth;
        let mut added_paths = Vec::new();
        for path in files {
            if self.add_track_path(path.clone(), root.as_deref(), folder_depth) {
                added_paths.push(path);
            }
        }
        added_paths
    }

    pub fn add_track_path(&mut self, path: PathBuf, root: Option<&Path>, folder_depth: usize) -> bool {
        if self.tracks.iter().any(|track| same_path(&track.path, &path)) {
            return false;
        }

        let mut track = Track::from_path(path, root, folder_depth);
        if self.kind == PlaylistKind::Favorites {
            self.ensure_favorite_added_sequences();
            track.favorite_added_sequence = Some(
                self.tracks
                    .iter()
                    .filter_map(|track| track.favorite_added_sequence)
                    .max()
                    .unwrap_or(0)
                    .saturating_add(1),
            );
            self.tracks.insert(0, track);
        } else {
            self.tracks.push(track);
            self.sort_tracks();
        }
        true
    }

    pub fn replace_tracks_from_files(&mut self, files: Vec<PathBuf>) {
        let root = self.source_folder.clone();
        let folder_depth = self.folder_depth;
        self.tracks = files
            .into_iter()
            .map(|path| {
                let mut track = Track::from_path(path, root.as_deref(), folder_depth);
                track.missing = false;
                track
            })
            .collect();
        self.sort_tracks();
        self.ensure_selected_group_exists();
    }

    pub fn apply_track_availability(
        &mut self,
        availability: &BTreeMap<String, bool>,
    ) -> TrackAvailabilityStats {
        let mut stats = TrackAvailabilityStats::default();

        for track in &mut self.tracks {
            let Some(is_present) = availability.get(&path_key(&track.path)).copied() else {
                if track.missing {
                    stats.missing_total += 1;
                }
                continue;
            };

            let was_missing = track.missing;
            track.missing = !is_present;
            if was_missing && is_present {
                stats.restored += 1;
            } else if !was_missing && !is_present {
                stats.newly_missing += 1;
            }
            if track.missing {
                stats.missing_total += 1;
            }
        }

        stats
    }

    pub fn merge_tracks_from_folder_scan(&mut self, files: &[PathBuf]) -> FolderSyncStats {
        let root = self.source_folder.clone();
        let folder_depth = self.folder_depth;
        let mut scanned = BTreeMap::new();
        for path in files {
            scanned
                .entry(path_key(path))
                .or_insert_with(|| path.clone());
        }

        let mut stats = FolderSyncStats::default();
        for track in &mut self.tracks {
            if scanned.remove(&path_key(&track.path)).is_some() {
                if track.missing {
                    stats.restored += 1;
                }
                track.missing = false;
                stats.present_total += 1;
            } else {
                if !track.missing {
                    stats.newly_missing += 1;
                }
                track.missing = true;
                stats.missing_total += 1;
            }
        }

        self.ensure_folder_group_contiguity();
        let mut additions_by_group = BTreeMap::<String, Vec<Track>>::new();
        let mut new_group_order = Vec::<String>::new();
        for path in scanned.into_values() {
            let mut track = Track::from_path(path, root.as_deref(), folder_depth);
            track.missing = false;
            if !additions_by_group.contains_key(&track.group) {
                new_group_order.push(track.group.clone());
            }
            additions_by_group
                .entry(track.group.clone())
                .or_default()
                .push(track);
            stats.added += 1;
            stats.present_total += 1;
        }

        if !additions_by_group.is_empty() {
            let old_tracks = std::mem::take(&mut self.tracks);
            let mut merged = Vec::with_capacity(old_tracks.len().saturating_add(stats.added));
            let mut current_group: Option<String> = None;

            for track in old_tracks {
                if current_group.as_deref() != Some(track.group.as_str()) {
                    if let Some(group) = current_group.take() {
                        if let Some(mut additions) = additions_by_group.remove(&group) {
                            merged.append(&mut additions);
                        }
                    }
                    current_group = Some(track.group.clone());
                }
                merged.push(track);
            }
            if let Some(group) = current_group {
                if let Some(mut additions) = additions_by_group.remove(&group) {
                    merged.append(&mut additions);
                }
            }
            for group in new_group_order {
                if let Some(mut additions) = additions_by_group.remove(&group) {
                    merged.append(&mut additions);
                }
            }
            self.tracks = merged;
        }

        self.ensure_selected_group_exists();
        stats
    }

    pub fn merge_tracks_from_folder_changes(
        &mut self,
        present_files: &[PathBuf],
        missing_roots: &[PathBuf],
    ) -> FolderSyncStats {
        let root = self.source_folder.clone();
        let folder_depth = self.folder_depth;
        let mut present = BTreeMap::<String, PathBuf>::new();
        for path in present_files {
            present
                .entry(path_key(path))
                .or_insert_with(|| path.clone());
        }

        let mut stats = FolderSyncStats::default();
        let mut matched_present_keys = BTreeSet::new();
        for track in &mut self.tracks {
            let key = path_key(&track.path);
            if present.contains_key(&key) {
                matched_present_keys.insert(key);
                if track.missing {
                    stats.restored += 1;
                }
                track.missing = false;
            } else if missing_roots
                .iter()
                .any(|missing_root| path_is_same_or_descendant(&track.path, missing_root))
            {
                if !track.missing {
                    stats.newly_missing += 1;
                }
                track.missing = true;
            }

            if track.missing {
                stats.missing_total += 1;
            } else {
                stats.present_total += 1;
            }
        }
        for key in matched_present_keys {
            present.remove(&key);
        }

        let mut additions_by_group = BTreeMap::<String, Vec<Track>>::new();
        let mut new_group_order = Vec::<String>::new();
        for path in present.into_values() {
            let mut track = Track::from_path(path, root.as_deref(), folder_depth);
            track.missing = false;
            if !additions_by_group.contains_key(&track.group) {
                new_group_order.push(track.group.clone());
            }
            additions_by_group
                .entry(track.group.clone())
                .or_default()
                .push(track);
            stats.added += 1;
            stats.present_total += 1;
        }

        if !additions_by_group.is_empty() {
            self.ensure_folder_group_contiguity();
            let old_tracks = std::mem::take(&mut self.tracks);
            let mut merged = Vec::with_capacity(old_tracks.len().saturating_add(stats.added));
            let mut current_group: Option<String> = None;

            for track in old_tracks {
                if current_group.as_deref() != Some(track.group.as_str()) {
                    if let Some(group) = current_group.take() {
                        if let Some(mut additions) = additions_by_group.remove(&group) {
                            merged.append(&mut additions);
                        }
                    }
                    current_group = Some(track.group.clone());
                }
                merged.push(track);
            }
            if let Some(group) = current_group {
                if let Some(mut additions) = additions_by_group.remove(&group) {
                    merged.append(&mut additions);
                }
            }
            for group in new_group_order {
                if let Some(mut additions) = additions_by_group.remove(&group) {
                    merged.append(&mut additions);
                }
            }
            self.tracks = merged;
        }

        self.ensure_selected_group_exists();
        stats
    }

    pub fn ensure_folder_group_contiguity(&mut self) {
        if self.kind != PlaylistKind::Folder || self.tracks.len() < 2 {
            return;
        }

        let old_tracks = std::mem::take(&mut self.tracks);
        let mut group_indexes = BTreeMap::<String, usize>::new();
        let mut grouped_tracks = Vec::<Vec<Track>>::new();
        for track in old_tracks {
            let group = track.group.clone();
            let group_index = if let Some(index) = group_indexes.get(&group).copied() {
                index
            } else {
                let index = grouped_tracks.len();
                group_indexes.insert(group, index);
                grouped_tracks.push(Vec::new());
                index
            };
            grouped_tracks[group_index].push(track);
        }

        self.tracks = grouped_tracks.into_iter().flatten().collect();
    }

    pub fn folder_groups(&self) -> Vec<String> {
        self.tracks
            .iter()
            .map(|track| track.group.clone())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn filtered_track_indexes(&self) -> Vec<usize> {
        self.tracks
            .iter()
            .enumerate()
            .filter_map(|(index, track)| {
                if self.track_matches_selected_group(track) {
                    Some(index)
                } else {
                    None
                }
            })
            .collect()
    }

    pub fn track_matches_selected_group(&self, track: &Track) -> bool {
        self.selected_group
            .as_ref()
            .map(|group| &track.group == group)
            .unwrap_or(true)
    }

    pub fn set_selected_group(&mut self, group: Option<String>) {
        self.selected_group = group;
        self.ensure_selected_group_exists();
    }

    fn ensure_selected_group_exists(&mut self) {
        let Some(selected_group) = self.selected_group.clone() else {
            return;
        };

        if !self.tracks.iter().any(|track| track.group == selected_group) {
            self.selected_group = None;
        }
    }

    pub fn ensure_favorite_added_sequences(&mut self) {
        if self.kind != PlaylistKind::Favorites {
            for track in &mut self.tracks {
                track.favorite_added_sequence = None;
            }
            return;
        }

        let known_sequences = self
            .tracks
            .iter()
            .filter_map(|track| track.favorite_added_sequence)
            .collect::<Vec<_>>();
        if known_sequences.is_empty() {
            let track_count = self.tracks.len() as u64;
            for (index, track) in self.tracks.iter_mut().enumerate() {
                track.favorite_added_sequence = Some(track_count.saturating_sub(index as u64));
            }
            return;
        }

        let mut next_unknown_sequence = known_sequences
            .into_iter()
            .min()
            .unwrap_or(1)
            .saturating_sub(1);
        for track in &mut self.tracks {
            if track.favorite_added_sequence.is_none() {
                track.favorite_added_sequence = Some(next_unknown_sequence);
                next_unknown_sequence = next_unknown_sequence.saturating_sub(1);
            }
        }
    }

    pub fn sort_favorites_by_added(&mut self) {
        if self.kind != PlaylistKind::Favorites {
            return;
        }

        self.ensure_favorite_added_sequences();
        self.tracks.sort_by_key(|track| Reverse(track.favorite_added_sequence.unwrap_or(0)));
    }

    pub fn sort_tracks(&mut self) {
        self.tracks.sort_by(|left, right| {
            natural_key(&left.group)
                .cmp(&natural_key(&right.group))
                .then_with(|| natural_key(&left.title).cmp(&natural_key(&right.title)))
                .then_with(|| left.path.cmp(&right.path))
        });
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct DspProfile {
    pub name: String,
    pub settings: DspSettings,
}

impl DspProfile {
    pub fn new(name: impl Into<String>, settings: DspSettings) -> Self {
        Self {
            name: name.into(),
            settings,
        }
    }
}



#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct RecordingSettings {
    #[serde(default)]
    pub output_folder: Option<PathBuf>,
}

impl RecordingSettings {
    pub fn resolved_output_folder(&self) -> PathBuf {
        self.output_folder
            .clone()
            .or_else(default_recording_output_folder)
            .unwrap_or_else(|| PathBuf::from(".audio-orbit-records"))
    }
}

pub fn default_recording_output_folder() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(".audio-orbit-records")))
}


#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct LibrarySettings {
    #[serde(default, alias = "auto_sync_folder_playlists")]
    pub auto_sync_selected_playlist: bool,
}

impl Default for LibrarySettings {
    fn default() -> Self {
        Self {
            auto_sync_selected_playlist: false,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UpdateSettings {
    #[serde(default)]
    pub include_prereleases: bool,
    #[serde(default)]
    pub last_auto_check_unix_seconds: u64,
}

impl Default for UpdateSettings {
    fn default() -> Self {
        Self {
            include_prereleases: false,
            last_auto_check_unix_seconds: 0,
        }
    }
}


#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum RepeatMode {
    Off,
    Track,
    Selection,
}

impl Default for RepeatMode {
    fn default() -> Self {
        Self::Off
    }
}

impl RepeatMode {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Off => "Repeat off",
            Self::Track => "Repeat track",
            Self::Selection => "Repeat selection",
        }
    }

    pub fn next(self) -> Self {
        match self {
            Self::Off => Self::Track,
            Self::Track => Self::Selection,
            Self::Selection => Self::Off,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PlaybackSettings {
    #[serde(default = "default_auto_advance")]
    pub auto_advance: bool,
    #[serde(default)]
    pub auto_switch_output_device: bool,
    #[serde(default)]
    pub crossfade_enabled: bool,
    #[serde(default = "default_crossfade_seconds")]
    pub crossfade_seconds: u8,
    #[serde(default)]
    pub repeat_mode: RepeatMode,
    #[serde(default)]
    pub shuffle_enabled: bool,
    #[serde(default = "default_volume_percent")]
    pub volume_percent: u8,
    #[serde(default)]
    pub muted: bool,
}

impl Default for PlaybackSettings {
    fn default() -> Self {
        Self {
            auto_advance: true,
            auto_switch_output_device: false,
            crossfade_enabled: false,
            crossfade_seconds: default_crossfade_seconds(),
            repeat_mode: RepeatMode::default(),
            shuffle_enabled: false,
            volume_percent: default_volume_percent(),
            muted: false,
        }
    }
}

fn default_auto_advance() -> bool {
    true
}

fn default_crossfade_seconds() -> u8 {
    5
}

fn default_volume_percent() -> u8 {
    100
}



#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize)]
pub struct WindowGeometry {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl WindowGeometry {
    pub fn is_valid(&self) -> bool {
        self.width >= 320.0
            && self.height >= 180.0
            && self.width.is_finite()
            && self.height.is_finite()
            && self.x.is_finite()
            && self.y.is_finite()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct UiSettings {
    #[serde(default = "default_true")]
    pub show_library_panel: bool,
    #[serde(default = "default_true")]
    pub show_profile_panel: bool,
    #[serde(default)]
    pub player_only_mode: bool,
    #[serde(default)]
    pub show_track_search: bool,
    #[serde(default = "default_true")]
    pub search_playback_filtered_only: bool,
    #[serde(default)]
    pub window_geometry: Option<WindowGeometry>,
    #[serde(default)]
    pub full_layout_window_geometry: Option<WindowGeometry>,
    #[serde(default)]
    pub player_only_window_geometry: Option<WindowGeometry>,
    #[serde(default)]
    pub playlist_scroll_offset_y: f32,
    #[serde(default)]
    pub playlist_scroll_offsets: BTreeMap<String, f32>,
}

impl Default for UiSettings {
    fn default() -> Self {
        Self {
            show_library_panel: true,
            show_profile_panel: true,
            player_only_mode: false,
            show_track_search: false,
            search_playback_filtered_only: true,
            window_geometry: None,
            full_layout_window_geometry: None,
            player_only_window_geometry: None,
            playlist_scroll_offset_y: 0.0,
            playlist_scroll_offsets: BTreeMap::new(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedState {
    pub playlists: Vec<Playlist>,
    pub profiles: Vec<DspProfile>,
    pub selected_playlist_index: usize,
    pub selected_profile_index: usize,
    #[serde(default)]
    pub radio_stations: Vec<RadioStation>,
    #[serde(default)]
    pub selected_radio_index: Option<usize>,
    #[serde(default)]
    pub last_played_track: Option<LastPlayedTrack>,
    #[serde(default)]
    pub update_settings: UpdateSettings,
    #[serde(default)]
    pub playback_session: PlaybackSession,
    #[serde(default)]
    pub playback: PlaybackSettings,
    #[serde(default)]
    pub recording: RecordingSettings,
    #[serde(default)]
    pub library: LibrarySettings,
    #[serde(default)]
    pub ui: UiSettings,
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            playlists: vec![Playlist::favorites(), Playlist::new("Local music")],
            profiles: vec![
                DspProfile::new("Smooth orbit", DspSettings::default()),
                DspProfile::new(
                    "Headphone surround",
                    DspSettings {
                        depth_cue_percent: 85,
                        mode: OrbitMode::VirtualEightDirectionOrbit,
                        ..DspSettings::default()
                    },
                ),
            ],
            selected_playlist_index: 1,
            selected_profile_index: 0,
            radio_stations: Vec::new(),
            selected_radio_index: None,
            last_played_track: None,
            update_settings: UpdateSettings::default(),
            playback_session: PlaybackSession::default(),
            playback: PlaybackSettings::default(),
            recording: RecordingSettings::default(),
            library: LibrarySettings::default(),
            ui: UiSettings::default(),
        }
    }
}

pub fn app_data_dir() -> Option<PathBuf> {
    std::env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(|parent| parent.join(".audio-orbit-data")))
}

pub fn state_path() -> Option<PathBuf> {
    app_data_dir().map(|dir| dir.join("state.json"))
}

fn legacy_state_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "AudioOrbit", "Audio Orbit")
        .map(|dirs| dirs.data_local_dir().join("state.json"))
}

pub fn load_state() -> SavedState {
    let Some(path) = state_path() else {
        return SavedState::default();
    };

    if let Ok(contents) = fs::read_to_string(&path) {
        return serde_json::from_str(&contents).unwrap_or_default();
    }

    if let Some(legacy_path) = legacy_state_path() {
        if let Ok(contents) = fs::read_to_string(&legacy_path) {
            let state = serde_json::from_str(&contents).unwrap_or_default();
            let _ = write_state_to_path(&state, &path);
            return state;
        }
    }

    SavedState::default()
}

fn persisted_state(state: &SavedState) -> SavedState {
    let mut persisted = state.clone();
    let temporary_index = persisted
        .playlists
        .iter()
        .position(|playlist| playlist.kind == PlaylistKind::Temporary);

    if let Some(index) = temporary_index {
        persisted.playlists.remove(index);

        if persisted.selected_playlist_index == index {
            persisted.selected_playlist_index = persisted
                .playlists
                .iter()
                .position(|playlist| playlist.kind == PlaylistKind::Manual)
                .unwrap_or(0);
        } else if persisted.selected_playlist_index > index {
            persisted.selected_playlist_index -= 1;
        }

        if persisted
            .last_played_track
            .as_ref()
            .map(|track| track.playlist_index == index)
            .unwrap_or(false)
        {
            persisted.last_played_track = None;
        } else if let Some(track) = persisted.last_played_track.as_mut() {
            if track.playlist_index > index {
                track.playlist_index -= 1;
            }
        }

        if persisted.playback_session.playlist_index == Some(index) {
            persisted.playback_session = PlaybackSession::default();
        } else if let Some(playlist_index) = persisted.playback_session.playlist_index.as_mut() {
            if *playlist_index > index {
                *playlist_index -= 1;
            }
        }
    }

    if persisted.playlists.is_empty() {
        persisted.playlists.push(Playlist::favorites());
        persisted.playlists.push(Playlist::new("Local music"));
    }
    if persisted.selected_playlist_index >= persisted.playlists.len() {
        persisted.selected_playlist_index = 0;
    }

    persisted
}

pub fn save_state(state: &SavedState) -> Result<()> {
    let path = state_path().context("could not resolve the application data path")?;
    write_state_to_path(&persisted_state(state), &path)
}

pub fn export_state_zip(state: &SavedState, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create backup directory: {}", parent.display()))?;
    }

    let file = File::create(path)
        .with_context(|| format!("failed to create backup zip: {}", path.display()))?;
    let mut zip = ZipWriter::new(file);
    let options = SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated);

    let meta = serde_json::json!({
        "app": "Audio Orbit",
        "version": app_version_label(),
        "type": "full-app-state-backup"
    });
    zip.start_file(BACKUP_META_ENTRY, options)?;
    zip.write_all(serde_json::to_string_pretty(&meta)?.as_bytes())?;

    zip.start_file(BACKUP_STATE_ENTRY, options)?;
    zip.write_all(serde_json::to_string_pretty(&persisted_state(state))?.as_bytes())?;
    zip.finish()?;
    Ok(())
}

pub fn import_state_zip(path: &Path) -> Result<SavedState> {
    let file = File::open(path)
        .with_context(|| format!("failed to open backup zip: {}", path.display()))?;
    let mut zip = ZipArchive::new(file)
        .with_context(|| format!("failed to read backup zip: {}", path.display()))?;
    let mut entry = zip
        .by_name(BACKUP_STATE_ENTRY)
        .context("backup zip does not contain audio-orbit/state.json")?;
    let mut contents = String::new();
    entry.read_to_string(&mut contents)?;
    let state = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse backup state from {}", path.display()))?;
    Ok(state)
}

#[derive(Clone, Debug)]
pub struct FolderScanResult {
    pub files: Vec<PathBuf>,
}

pub fn scan_audio_folder(root: &Path) -> Result<FolderScanResult> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];

    while let Some(directory) = directories.pop() {
        let metadata = fs::metadata(&directory)
            .with_context(|| format!("failed to inspect folder: {}", directory.display()))?;
        if !metadata.is_dir() {
            anyhow::bail!("folder path is not a directory: {}", directory.display());
        }

        let entries = fs::read_dir(&directory)
            .with_context(|| format!("failed to read folder: {}", directory.display()))?;

        for entry in entries {
            let entry = entry.with_context(|| {
                format!("failed to inspect a folder entry under {}", directory.display())
            })?;
            let path = entry.path();
            let file_type = entry.file_type().with_context(|| {
                format!("failed to read file type: {}", path.display())
            })?;

            // Never follow symbolic links or Windows reparse points during
            // recursive scans. This prevents cycles and avoids leaving the
            // user-selected folder through junctions or mounted paths.
            if is_recursive_scan_link(&path, &file_type) {
                continue;
            }
            if file_type.is_dir() {
                directories.push(path);
            } else if file_type.is_file() && is_supported_audio_file(&path) {
                files.push(path);
            }
        }
    }

    files.sort_by(|left, right| natural_key(&left.to_string_lossy()).cmp(&natural_key(&right.to_string_lossy())));
    Ok(FolderScanResult { files })
}

pub fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| {
            matches!(
                extension.to_lowercase().as_str(),
                "mp3" | "wav" | "flac" | "ogg" | "opus" | "m4a" | "mp4" | "aac" | "aiff" | "aif" | "ape" | "wv"
            )
        })
        .unwrap_or(false)
}

pub fn is_recursive_scan_link(path: &Path, file_type: &fs::FileType) -> bool {
    if file_type.is_symlink() {
        return true;
    }

    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;

        const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
        return fs::symlink_metadata(path)
            .map(|metadata| metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0)
            .unwrap_or(true);
    }

    #[cfg(not(windows))]
    {
        let _ = path;
        false
    }
}

pub fn path_key(path: &Path) -> String {
    path.to_string_lossy().to_lowercase()
}

pub fn same_path(left: &Path, right: &Path) -> bool {
    path_key(left) == path_key(right)
}

pub fn path_is_same_or_descendant(path: &Path, root: &Path) -> bool {
    let path = path_key(path);
    let root = path_key(root);

    if path == root {
        return true;
    }

    if root.ends_with('\\') || root.ends_with('/') {
        return path.starts_with(&root);
    }

    path.strip_prefix(&root)
        .map(|suffix| suffix.starts_with('\\') || suffix.starts_with('/'))
        .unwrap_or(false)
}

pub fn display_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}

fn write_state_to_path(state: &SavedState, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create application data directory: {}", parent.display()))?;
    }

    let contents = serde_json::to_string_pretty(state)
        .context("failed to serialize application state")?;
    fs::write(path, contents)
        .with_context(|| format!("failed to save application state: {}", path.display()))?;

    Ok(())
}

#[allow(dead_code)]
fn read_track_metadata(path: &Path) -> Result<TrackMetadata> {
    let file_size = fs::metadata(path).ok().map(|metadata| metadata.len());
    let tagged_file = lofty::read_from_path(path)
        .with_context(|| format!("failed to read audio metadata: {}", path.display()))?;
    let properties = tagged_file.properties();

    Ok(TrackMetadata {
        size_bytes: file_size,
        duration_seconds: Some(properties.duration().as_secs_f32()).filter(|value| *value > 0.0),
        sample_rate_hz: properties.sample_rate(),
        channels: properties.channels(),
        bitrate_kbps: properties.audio_bitrate().or_else(|| properties.overall_bitrate()),
    })
}

fn folder_group_for_path(path: &Path, root: Option<&Path>, folder_depth: usize) -> String {
    if folder_depth == 0 {
        return "Root".to_owned();
    }

    if let Some(root) = root {
        if let Ok(relative_path) = path.strip_prefix(root) {
            if let Some(parent) = relative_path.parent() {
                let parts: Vec<String> = parent
                    .components()
                    .filter_map(|component| component.as_os_str().to_str().map(ToOwned::to_owned))
                    .filter(|part| !part.trim().is_empty())
                    .take(folder_depth)
                    .collect();

                if !parts.is_empty() {
                    return parts.join(" / ");
                }
            }
        }
    }

    path.parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        .filter(|name| !name.trim().is_empty())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| "Root".to_owned())
}

fn natural_key(input: &str) -> String {
    input.to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track_paths(playlist: &Playlist) -> Vec<String> {
        playlist
            .tracks
            .iter()
            .map(|track| track.path.to_string_lossy().into_owned())
            .collect()
    }

    fn owned_paths(paths: &[&PathBuf]) -> Vec<String> {
        paths
            .iter()
            .map(|path| path.to_string_lossy().into_owned())
            .collect()
    }

    #[test]
    fn playback_settings_keep_auto_output_switch_disabled_for_existing_state() {
        let settings: PlaybackSettings = serde_json::from_str("{}").unwrap();

        assert!(!settings.auto_switch_output_device);
    }

    #[test]
    fn favorites_insert_new_tracks_at_top_and_restore_added_order() {
        let alpha = PathBuf::from("C:/Music/Alpha.mp3");
        let beta = PathBuf::from("C:/Music/Beta.mp3");
        let charlie = PathBuf::from("C:/Music/Charlie.mp3");
        let mut favorites = Playlist::favorites();

        assert!(favorites.add_track_path(alpha.clone(), None, 0));
        assert!(favorites.add_track_path(beta.clone(), None, 0));
        assert_eq!(track_paths(&favorites), owned_paths(&[&beta, &alpha]));

        favorites.sort_tracks();
        assert_eq!(track_paths(&favorites), owned_paths(&[&alpha, &beta]));

        assert!(favorites.add_track_path(charlie.clone(), None, 0));
        assert_eq!(track_paths(&favorites), owned_paths(&[&charlie, &alpha, &beta]));
        assert!(!favorites.add_track_path(alpha.clone(), None, 0));

        favorites.sort_favorites_by_added();
        assert_eq!(
            track_paths(&favorites),
            owned_paths(&[&charlie, &beta, &alpha])
        );
    }

    #[test]
    fn favorites_keep_manual_order_across_serialization() {
        let alpha = PathBuf::from("C:/Music/Alpha.mp3");
        let beta = PathBuf::from("C:/Music/Beta.mp3");
        let mut favorites = Playlist::favorites();
        favorites.add_track_path(alpha.clone(), None, 0);
        favorites.add_track_path(beta.clone(), None, 0);
        favorites.tracks.swap(0, 1);

        let serialized = serde_json::to_string(&favorites).unwrap();
        let mut restored: Playlist = serde_json::from_str(&serialized).unwrap();

        assert_eq!(track_paths(&restored), owned_paths(&[&alpha, &beta]));
        restored.sort_favorites_by_added();
        assert_eq!(track_paths(&restored), owned_paths(&[&beta, &alpha]));
    }

    #[test]
    fn favorites_migrate_missing_added_sequence_without_reordering_tracks() {
        let alpha = PathBuf::from("C:/Music/Alpha.mp3");
        let beta = PathBuf::from("C:/Music/Beta.mp3");
        let mut favorites = Playlist::favorites();
        favorites.tracks = vec![
            Track::from_path(beta.clone(), None, 0),
            Track::from_path(alpha.clone(), None, 0),
        ];

        let serialized = serde_json::to_string(&favorites).unwrap();
        let mut restored: Playlist = serde_json::from_str(&serialized).unwrap();
        restored.ensure_favorite_added_sequences();

        assert_eq!(track_paths(&restored), owned_paths(&[&beta, &alpha]));
        assert_eq!(
            restored
                .tracks
                .iter()
                .map(|track| track.favorite_added_sequence)
                .collect::<Vec<_>>(),
            vec![Some(2), Some(1)]
        );
    }

    #[test]
    fn non_favorite_playlists_do_not_persist_favorite_sequences() {
        let mut playlist = Playlist::new("Manual");
        let mut track = Track::from_path(PathBuf::from("C:/Music/Alpha.mp3"), None, 0);
        track.favorite_added_sequence = Some(42);
        playlist.tracks.push(track);

        playlist.ensure_favorite_added_sequences();

        assert_eq!(playlist.tracks[0].favorite_added_sequence, None);
        assert!(!serde_json::to_string(&playlist)
            .unwrap()
            .contains("favorite_added_sequence"));
    }

    #[test]
    fn library_settings_keep_automatic_sync_disabled_for_existing_state() {
        let settings: LibrarySettings = serde_json::from_str("{}").unwrap();

        assert!(!settings.auto_sync_selected_playlist);
    }

    #[test]
    fn library_settings_migrate_previous_automatic_sync_key() {
        let settings: LibrarySettings = serde_json::from_str(
            r#"{"auto_sync_folder_playlists":true}"#,
        )
        .unwrap();

        assert!(settings.auto_sync_selected_playlist);
    }

    #[test]
    fn tracks_from_existing_state_default_to_available() {
        let track: Track = serde_json::from_str(
            r#"{"path":"C:/Music/Track.mp3","title":"Track","group":"Root"}"#,
        )
        .unwrap();

        assert!(!track.missing);
    }

    #[test]
    fn path_descendant_check_respects_component_boundaries() {
        let root = PathBuf::from("C:/Music");

        assert!(path_is_same_or_descendant(&root, &root));
        assert!(path_is_same_or_descendant(
            &PathBuf::from("C:/Music/Album/Track.mp3"),
            &root,
        ));
        assert!(!path_is_same_or_descendant(
            &PathBuf::from("C:/Music Archive/Track.mp3"),
            &root,
        ));
        assert!(path_is_same_or_descendant(
            &PathBuf::from("C:/Music/Track.mp3"),
            &PathBuf::from("C:/"),
        ));
    }

    #[test]
    fn incremental_folder_sync_touches_only_changed_paths() {
        let root = PathBuf::from("C:/Music");
        let alpha = root.join("Album/Alpha.mp3");
        let beta = root.join("Album/Beta.mp3");
        let gamma = root.join("Other/Gamma.mp3");
        let delta = root.join("Album/Delta.mp3");
        let mut playlist = Playlist::from_folder(
            "Folder",
            root,
            2,
            vec![alpha.clone(), beta.clone(), gamma.clone()],
        );

        let stats = playlist.merge_tracks_from_folder_changes(
            &[beta.clone(), delta.clone()],
            std::slice::from_ref(&alpha),
        );

        assert_eq!(stats.added, 1);
        assert_eq!(stats.newly_missing, 1);
        assert_eq!(stats.present_total, 3);
        assert_eq!(stats.missing_total, 1);
        assert!(playlist
            .tracks
            .iter()
            .find(|track| same_path(&track.path, &alpha))
            .unwrap()
            .missing);
        assert!(!playlist
            .tracks
            .iter()
            .find(|track| same_path(&track.path, &gamma))
            .unwrap()
            .missing);
        assert!(playlist.tracks.iter().any(|track| same_path(&track.path, &delta)));
    }

    #[test]
    fn folder_sync_retains_missing_tracks_and_adds_new_files() {
        let root = PathBuf::from("C:/Music");
        let alpha = root.join("Alpha.mp3");
        let beta = root.join("Beta.mp3");
        let gamma = root.join("Gamma.mp3");
        let mut playlist = Playlist::from_folder(
            "Folder",
            root,
            1,
            vec![alpha.clone(), beta.clone()],
        );

        let stats = playlist.merge_tracks_from_folder_scan(&[beta.clone(), gamma.clone()]);

        assert_eq!(stats.added, 1);
        assert_eq!(stats.newly_missing, 1);
        assert_eq!(stats.missing_total, 1);
        assert_eq!(track_paths(&playlist), owned_paths(&[&alpha, &beta, &gamma]));
        assert!(playlist.tracks[0].missing);
        assert!(!playlist.tracks[1].missing);
        assert!(!playlist.tracks[2].missing);

        let restored = playlist.merge_tracks_from_folder_scan(&[alpha.clone(), beta, gamma]);
        assert_eq!(restored.restored, 1);
        assert_eq!(restored.missing_total, 0);
        assert!(playlist.tracks.iter().all(|track| !track.missing));
    }

    #[test]
    fn folder_sync_keeps_manual_order_and_appends_new_track_inside_group() {
        let root = PathBuf::from("C:/Music");
        let alpha = root.join("Artist").join("Alpha.mp3");
        let beta = root.join("Artist").join("Beta.mp3");
        let gamma = root.join("Artist").join("Gamma.mp3");
        let other = root.join("Other").join("Track.mp3");
        let mut playlist = Playlist::from_folder(
            "Folder",
            root,
            1,
            vec![alpha.clone(), beta.clone(), other.clone()],
        );
        playlist.tracks.swap(0, 1);

        playlist.merge_tracks_from_folder_scan(&[
            alpha.clone(),
            beta.clone(),
            gamma.clone(),
            other.clone(),
        ]);

        assert_eq!(
            track_paths(&playlist),
            owned_paths(&[&beta, &alpha, &gamma, &other])
        );
    }

    #[test]
    fn folder_group_normalization_preserves_group_and_track_order() {
        let root = PathBuf::from("C:/Music");
        let artist_a_one = root.join("Artist A").join("One.mp3");
        let artist_b_one = root.join("Artist B").join("One.mp3");
        let artist_a_two = root.join("Artist A").join("Two.mp3");
        let mut playlist = Playlist::from_folder(
            "Folder",
            root,
            1,
            vec![artist_a_one.clone(), artist_b_one.clone(), artist_a_two.clone()],
        );
        playlist.tracks = vec![
            Track::from_path(artist_a_one.clone(), playlist.source_folder.as_deref(), 1),
            Track::from_path(artist_b_one.clone(), playlist.source_folder.as_deref(), 1),
            Track::from_path(artist_a_two.clone(), playlist.source_folder.as_deref(), 1),
        ];

        playlist.ensure_folder_group_contiguity();

        assert_eq!(
            track_paths(&playlist),
            owned_paths(&[&artist_a_one, &artist_a_two, &artist_b_one])
        );
    }

    #[test]
    fn folder_sync_preserves_existing_order_when_only_availability_changes() {
        let root = PathBuf::from("C:/Music");
        let alpha = root.join("Alpha.mp3");
        let beta = root.join("Beta.mp3");
        let mut playlist = Playlist::from_folder(
            "Folder",
            root,
            1,
            vec![alpha.clone(), beta.clone()],
        );
        playlist.tracks.swap(0, 1);

        playlist.merge_tracks_from_folder_scan(std::slice::from_ref(&beta));
        assert_eq!(track_paths(&playlist), owned_paths(&[&beta, &alpha]));
        assert!(playlist.tracks[1].missing);

        playlist.merge_tracks_from_folder_scan(&[alpha.clone(), beta.clone()]);
        assert_eq!(track_paths(&playlist), owned_paths(&[&beta, &alpha]));
        assert!(playlist.tracks.iter().all(|track| !track.missing));
    }

    #[cfg(unix)]
    #[test]
    fn folder_scan_does_not_follow_directory_symlinks() {
        use std::os::unix::fs::symlink;

        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let test_root = std::env::temp_dir().join(format!(
            "audio-orbit-folder-scan-{}-{unique}",
            std::process::id()
        ));
        let library = test_root.join("library");
        let outside = test_root.join("outside");
        fs::create_dir_all(&library).unwrap();
        fs::create_dir_all(&outside).unwrap();
        let inside_track = library.join("inside.mp3");
        fs::write(&inside_track, b"").unwrap();
        fs::write(outside.join("outside.mp3"), b"").unwrap();
        symlink(&outside, library.join("linked")).unwrap();

        let files = scan_audio_folder(&library).unwrap().files;

        assert_eq!(files, vec![inside_track]);
        fs::remove_dir_all(test_root).unwrap();
    }

    #[test]
    fn availability_check_marks_and_restores_manual_tracks_without_reordering() {
        let alpha = PathBuf::from("C:/Music/Alpha.mp3");
        let beta = PathBuf::from("C:/Music/Beta.mp3");
        let mut playlist = Playlist::new("Manual");
        playlist.tracks = vec![
            Track::from_path(alpha.clone(), None, 0),
            Track::from_path(beta.clone(), None, 0),
        ];
        for track in &mut playlist.tracks {
            track.missing = false;
        }

        let availability = BTreeMap::from([
            (path_key(&alpha), false),
            (path_key(&beta), true),
        ]);
        let missing = playlist.apply_track_availability(&availability);

        assert_eq!(missing.newly_missing, 1);
        assert_eq!(missing.missing_total, 1);
        assert_eq!(track_paths(&playlist), owned_paths(&[&alpha, &beta]));

        let restored = playlist.apply_track_availability(&BTreeMap::from([
            (path_key(&alpha), true),
            (path_key(&beta), true),
        ]));
        assert_eq!(restored.restored, 1);
        assert_eq!(restored.missing_total, 0);
    }

    #[test]
    fn temporary_playlist_is_runtime_only_and_read_only() {
        let mut state = SavedState::default();
        state.playlists.push(Playlist::temporary());
        state.selected_playlist_index = state.playlists.len() - 1;
        state.last_played_track = Some(LastPlayedTrack {
            playlist_index: state.selected_playlist_index,
            track_path: PathBuf::from("C:/Music/mix.mp3"),
        });
        state.playback_session.playlist_index = Some(state.selected_playlist_index);
        state.playback_session.track_path = Some(PathBuf::from("C:/Music/mix.mp3"));
        state.playback_session.was_active = true;

        let persisted = persisted_state(&state);

        assert!(persisted
            .playlists
            .iter()
            .all(|playlist| playlist.kind != PlaylistKind::Temporary));
        assert!(persisted.last_played_track.is_none());
        assert!(persisted.playback_session.playlist_index.is_none());
        assert!(!PlaylistKind::Temporary.accepts_manual_tracks());
        assert!(!PlaylistKind::Temporary.can_delete());
    }
}
