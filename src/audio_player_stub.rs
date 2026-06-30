use crate::dsp::DspSettings;
use anyhow::Result;
use std::{
    path::{Path, PathBuf},
    time::Instant,
};

#[derive(Clone, Debug)]
pub struct PlaybackInfo {
    pub path: PathBuf,
    pub original_duration_seconds: f32,
    pub rendered_duration_seconds: f32,
    pub input_channels: u16,
    pub sample_rate: u32,
    pub size_bytes: Option<u64>,
    pub waveform: Vec<f32>,
    pub waveform_brightness: Vec<f32>,
    pub silence_ranges: Vec<(f32, f32)>,
}

pub struct PreparedPlayback;

#[derive(Clone, Debug)]
pub struct RadioRecordingInfo {
    pub path: PathBuf,
    pub started_at: Instant,
    pub bytes_written: u64,
}

#[derive(Clone, Copy, Debug)]
pub struct RadioVisualizerBar {
    pub age_seconds: f32,
    pub peak: f32,
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

#[derive(Clone, Debug, Default)]
pub struct RadioVisualizerFrame {
    pub bars: Vec<RadioVisualizerBar>,
    pub bucket_seconds: f32,
}

pub struct AudioPlayer {
    output_device_name: String,
    volume_percent: u8,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        Ok(Self {
            output_device_name: current_default_output_device_name(),
            volume_percent: 100,
        })
    }

    pub fn output_device_name(&self) -> &str {
        &self.output_device_name
    }

    pub fn set_volume_percent(&mut self, volume_percent: u8) {
        self.volume_percent = volume_percent.clamp(0, 100);
    }

    pub fn play_radio_stream(&mut self, _url: &str, _settings: DspSettings) -> Result<()> {
        anyhow::bail!(unsupported_audio_message())
    }

    pub fn is_radio_recording(&self) -> bool {
        false
    }

    pub fn radio_recording_info(&self) -> Option<RadioRecordingInfo> {
        None
    }

    pub fn start_radio_recording(
        &mut self,
        _output_folder: &Path,
        _station_name: &str,
        _stream_title: Option<&str>,
    ) -> Result<PathBuf> {
        anyhow::bail!(unsupported_audio_message())
    }

    pub fn stop_radio_recording(&mut self) -> Result<Option<RadioRecordingInfo>> {
        Ok(None)
    }

    pub fn radio_visualizer_frame(&self, _requested_points: usize) -> RadioVisualizerFrame {
        RadioVisualizerFrame::default()
    }


    pub fn prepare_file(
        _path: PathBuf,
        _settings: DspSettings,
        _start_seconds: f32,
    ) -> Result<PreparedPlayback> {
        anyhow::bail!(unsupported_audio_message())
    }

    pub fn play_prepared(&mut self, _prepared: PreparedPlayback) -> Result<PlaybackInfo> {
        anyhow::bail!(unsupported_audio_message())
    }

    pub fn play_prepared_from_live_position(
        &mut self,
        _prepared: PreparedPlayback,
        _render_elapsed_seconds: f32,
    ) -> Result<PlaybackInfo> {
        anyhow::bail!(unsupported_audio_message())
    }

    pub fn play_file_with_orbit_from(
        &mut self,
        _path: &Path,
        _settings: DspSettings,
        _start_seconds: f32,
    ) -> Result<PlaybackInfo> {
        anyhow::bail!(unsupported_audio_message())
    }

    pub fn crossfade_to_prepared(
        &mut self,
        _prepared: PreparedPlayback,
        _crossfade_seconds: f32,
    ) -> Result<PlaybackInfo> {
        anyhow::bail!(unsupported_audio_message())
    }

    pub fn seek_current(&mut self, _seconds: f32) -> Result<Option<PlaybackInfo>> {
        Ok(None)
    }

    pub fn stop(&mut self) {}

    pub fn pause_or_resume(&mut self) {}

    pub fn is_playing(&self) -> bool {
        false
    }

    pub fn is_paused(&self) -> bool {
        false
    }

    pub fn has_finished(&self) -> bool {
        false
    }

    pub fn playback_position_seconds(&self) -> f32 {
        0.0
    }

    pub fn playback_duration_seconds(&self) -> Option<f32> {
        None
    }

    pub fn current_start_offset_seconds(&self) -> f32 {
        0.0
    }
}

pub fn current_default_output_device_name() -> String {
    "Audio disabled in non-Windows dev preview".to_owned()
}

fn unsupported_audio_message() -> &'static str {
    "Audio playback is available in Windows builds. Non-Windows cargo dev runs the UI with audio disabled so development does not require ALSA/CoreAudio backend packages."
}
