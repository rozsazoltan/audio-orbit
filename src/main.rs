#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod audio_player;
mod dsp;

use crate::{
    audio_player::{AudioPlayer, PlaybackInfo},
    dsp::{DspSettings, OrbitMode},
};
use eframe::egui;
use rfd::FileDialog;
use std::path::{Path, PathBuf};

fn main() -> eframe::Result<()> {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_inner_size([620.0, 480.0])
            .with_min_inner_size([560.0, 440.0])
            .with_resizable(false),
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
    selected_file: Option<PathBuf>,
    last_playback: Option<PlaybackInfo>,
    settings: DspSettings,
    status_message: String,
    error_message: Option<String>,
}

impl AudioOrbitApp {
    fn new() -> Self {
        match AudioPlayer::new() {
            Ok(player) => Self {
                player: Some(player),
                selected_file: None,
                last_playback: None,
                settings: DspSettings::default(),
                status_message: "Open an audio file, then play it through the orbit DSP.".to_owned(),
                error_message: None,
            },
            Err(error) => Self {
                player: None,
                selected_file: None,
                last_playback: None,
                settings: DspSettings::default(),
                status_message: "No audio output device is available.".to_owned(),
                error_message: Some(error.to_string()),
            },
        }
    }

    fn open_file(&mut self) {
        let picked_file = FileDialog::new()
            .add_filter("Audio files", &["mp3", "wav", "flac", "ogg"])
            .add_filter("All files", &["*"])
            .pick_file();

        if let Some(path) = picked_file {
            self.selected_file = Some(path.clone());
            self.status_message = format!("Selected: {}", display_file_name(&path));
            self.error_message = None;
        }
    }

    fn play_selected_file(&mut self) {
        let Some(path) = self.selected_file.clone() else {
            self.error_message = Some("Select an audio file first.".to_owned());
            return;
        };

        let Some(player) = &mut self.player else {
            self.error_message = Some("No audio output device is available.".to_owned());
            return;
        };

        self.status_message = "Decoding and processing audio...".to_owned();
        self.error_message = None;

        match player.play_file_with_orbit(&path, self.settings) {
            Ok(info) => {
                self.status_message = format!(
                    "Playing {} through {}.",
                    display_file_name(&info.path),
                    self.settings.mode.label()
                );
                self.last_playback = Some(info);
            }
            Err(error) => {
                self.error_message = Some(error.to_string());
                self.status_message = "Playback failed.".to_owned();
            }
        }
    }

    fn stop(&mut self) {
        if let Some(player) = &mut self.player {
            player.stop();
        }

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
}

impl Drop for AudioOrbitApp {
    fn drop(&mut self) {
        self.stop();
    }
}

impl eframe::App for AudioOrbitApp {
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        egui::CentralPanel::default().show(context, |ui| {
            ui.heading("Audio Orbit");
            ui.label("A local audio player that moves the decoded music signal across the stereo field.");
            ui.label("It processes the selected file itself; it does not control Spotify, YouTube, or other apps.");
            ui.separator();

            ui.horizontal(|ui| {
                if ui.button("Open audio file...").clicked() {
                    self.open_file();
                }

                let selected = self
                    .selected_file
                    .as_ref()
                    .map(|path| display_file_name(path))
                    .unwrap_or_else(|| "No file selected".to_owned());
                ui.label(selected);
            });

            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Mode:");
                ui.radio_value(
                    &mut self.settings.mode,
                    OrbitMode::SmoothLeftRight,
                    OrbitMode::SmoothLeftRight.label(),
                );
                ui.radio_value(
                    &mut self.settings.mode,
                    OrbitMode::EightStepOrbit,
                    OrbitMode::EightStepOrbit.label(),
                );
            });

            ui.add(
                egui::Slider::new(&mut self.settings.output_level_percent, 1u8..=100u8)
                    .text("Output Level (%)"),
            );
            ui.add(
                egui::Slider::new(&mut self.settings.stereo_width_percent, 0u8..=100u8)
                    .text("Stereo Width (%)"),
            );
            ui.add(
                egui::Slider::new(&mut self.settings.orbit_speed_percent, 10u8..=200u8)
                    .text("Orbit Speed (%)"),
            );

            ui.add_space(10.0);
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        self.player.is_some() && self.selected_file.is_some(),
                        egui::Button::new("Play with orbit DSP"),
                    )
                    .clicked()
                {
                    self.play_selected_file();
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
            });

            ui.separator();
            ui.strong("Status");
            ui.label(&self.status_message);

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
            ui.small("For a strong test, use headphones, set Stereo Width to 100%, and try Smooth left/right sweep first. The 8-step mode is a stereo cue, not true HRTF surround.");
        });
    }
}

fn display_file_name(path: &Path) -> String {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(ToOwned::to_owned)
        .unwrap_or_else(|| path.display().to_string())
}
