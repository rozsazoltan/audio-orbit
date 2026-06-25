use crate::dsp::DspSettings;
use anyhow::{Context, Result};
use directories::ProjectDirs;
use serde::{Deserialize, Serialize};
use std::{fs, path::PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<PathBuf>,
}

impl Playlist {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            tracks: Vec::new(),
        }
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
            playlists: vec![Playlist::new("Main playlist")],
            profiles: vec![
                DspProfile::new("Smooth orbit", DspSettings::default()),
                DspProfile::new("Wide virtual 8-direction", DspSettings {
                    depth_cue_percent: 75,
                    mode: crate::dsp::OrbitMode::VirtualEightDirectionOrbit,
                    ..DspSettings::default()
                }),
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

    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .with_context(|| format!("failed to create configuration directory: {}", parent.display()))?;
    }

    let contents = serde_json::to_string_pretty(state)
        .context("failed to serialize application state")?;
    fs::write(&path, contents)
        .with_context(|| format!("failed to save application state: {}", path.display()))?;

    Ok(())
}

fn state_path() -> Option<PathBuf> {
    ProjectDirs::from("dev", "AudioOrbit", "Audio Orbit")
        .map(|dirs| dirs.config_dir().join("state.json"))
}
