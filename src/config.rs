use crate::dsp::{DspSettings, OrbitMode};
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeSet,
    fs,
    path::{Path, PathBuf},
};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Track {
    pub path: PathBuf,
    pub title: String,
    pub group: String,
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

        Self { path, title, group }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<Track>,
    pub source_folder: Option<PathBuf>,
    pub folder_depth: usize,
    pub selected_group: Option<String>,
}

impl Playlist {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tracks: Vec::new(),
            source_folder: None,
            folder_depth: 2,
            selected_group: None,
        }
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
        };
        playlist.replace_tracks_from_files(files);
        playlist
    }

    pub fn add_files(&mut self, files: Vec<PathBuf>) {
        let root = self.source_folder.as_deref();
        let folder_depth = self.folder_depth;
        self.tracks
            .extend(files.into_iter().map(|path| Track::from_path(path, root, folder_depth)));
        self.sort_tracks();
    }

    pub fn replace_tracks_from_files(&mut self, files: Vec<PathBuf>) {
        let root = self.source_folder.as_deref();
        let folder_depth = self.folder_depth;
        self.tracks = files
            .into_iter()
            .map(|path| Track::from_path(path, root, folder_depth))
            .collect();
        self.sort_tracks();
        self.ensure_selected_group_exists();
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

    pub fn selected_group_label(&self) -> String {
        self.selected_group
            .clone()
            .unwrap_or_else(|| "All folders".to_owned())
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

    fn sort_tracks(&mut self) {
        self.tracks.sort_by(|left, right| {
            left.group
                .to_lowercase()
                .cmp(&right.group.to_lowercase())
                .then_with(|| left.title.to_lowercase().cmp(&right.title.to_lowercase()))
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

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SavedState {
    pub playlists: Vec<Playlist>,
    pub profiles: Vec<DspProfile>,
    pub selected_playlist_index: usize,
    pub selected_profile_index: usize,
}

impl Default for SavedState {
    fn default() -> Self {
        Self {
            playlists: vec![Playlist::new("Local music")],
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
            selected_playlist_index: 0,
            selected_profile_index: 0,
        }
    }
}

pub fn load_state() -> SavedState {
    let Some(path) = state_path() else {
        return SavedState::default();
    };

    let Ok(contents) = fs::read_to_string(path) else {
        return SavedState::default();
    };

    serde_json::from_str(&contents).unwrap_or_default()
}

pub fn save_state(state: &SavedState) -> Result<()> {
    let path = state_path().context("could not resolve the configuration path")?;
    write_state_to_path(state, &path)
}

pub fn export_state_to(state: &SavedState, path: &Path) -> Result<()> {
    write_state_to_path(state, path)
}

pub fn import_state_from(path: &Path) -> Result<SavedState> {
    let contents = fs::read_to_string(path)
        .with_context(|| format!("failed to read library backup: {}", path.display()))?;
    let state = serde_json::from_str(&contents)
        .with_context(|| format!("failed to parse library backup: {}", path.display()))?;
    Ok(state)
}

pub fn collect_audio_files_from_folder(root: &Path) -> Result<Vec<PathBuf>> {
    let mut files = Vec::new();
    let mut directories = vec![root.to_path_buf()];

    while let Some(directory) = directories.pop() {
        let entries = fs::read_dir(&directory)
            .with_context(|| format!("failed to read folder: {}", directory.display()))?;

        for entry in entries {
            let entry = entry.with_context(|| {
                format!("failed to inspect a folder entry under {}", directory.display())
            })?;
            let path = entry.path();
            let metadata = entry.metadata().with_context(|| {
                format!("failed to read file metadata: {}", path.display())
            })?;

            if metadata.is_dir() {
                directories.push(path);
            } else if metadata.is_file() && is_supported_audio_file(&path) {
                files.push(path);
            }
        }
    }

    files.sort_by(|left, right| left.to_string_lossy().to_lowercase().cmp(&right.to_string_lossy().to_lowercase()));
    Ok(files)
}

pub fn is_supported_audio_file(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .map(|extension| matches!(extension.to_lowercase().as_str(), "mp3" | "wav" | "flac" | "ogg"))
        .unwrap_or(false)
}

fn write_state_to_path(state: &SavedState, path: &Path) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create configuration directory: {}", parent.display()))?;
    }

    let contents = serde_json::to_string_pretty(state)
        .context("failed to serialize application state")?;
    fs::write(path, contents)
        .with_context(|| format!("failed to save application state: {}", path.display()))?;

    Ok(())
}

fn state_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "AudioOrbit", "Audio Orbit")
        .map(|dirs| dirs.config_dir().join("state.json"))
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

fn display_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}
