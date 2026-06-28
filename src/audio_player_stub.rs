use crate::dsp::DspSettings;
use anyhow::Result;
use std::{
    fs::File,
    io::Write,
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

#[derive(Clone, Debug)]
pub struct RecognitionAudioSample {
    pub sample_rate: u32,
    pub channels: u16,
    pub samples: Vec<f32>,
}

impl RecognitionAudioSample {
    pub fn duration_seconds(&self) -> f32 {
        if self.sample_rate == 0 || self.channels == 0 {
            0.0
        } else {
            self.samples.len() as f32 / self.channels as f32 / self.sample_rate as f32
        }
    }

    pub fn write_wav(&self, path: &Path) -> Result<()> {
        if self.samples.is_empty() {
            anyhow::bail!("recognition sample is empty");
        }
        if self.sample_rate == 0 || self.channels == 0 {
            anyhow::bail!("recognition sample has an invalid audio format");
        }

        let mut file = File::create(path)?;
        let bits_per_sample = 16u16;
        let bytes_per_sample = bits_per_sample / 8;
        let block_align = self.channels.saturating_mul(bytes_per_sample);
        let byte_rate = self.sample_rate.saturating_mul(block_align as u32);
        let data_size = (self.samples.len() * bytes_per_sample as usize) as u32;
        let chunk_size = 36u32.saturating_add(data_size);

        file.write_all(b"RIFF")?;
        file.write_all(&chunk_size.to_le_bytes())?;
        file.write_all(b"WAVE")?;
        file.write_all(b"fmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&self.channels.to_le_bytes())?;
        file.write_all(&self.sample_rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&bits_per_sample.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&data_size.to_le_bytes())?;

        for sample in &self.samples {
            let scaled = (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16;
            file.write_all(&scaled.to_le_bytes())?;
        }
        file.flush()?;
        Ok(())
    }
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

    pub fn radio_recognition_sample(&self, _seconds: f32) -> Result<Option<RecognitionAudioSample>> {
        Ok(None)
    }

    pub fn capture_file_recognition_sample(
        _path: &Path,
        _start_seconds: f32,
        _seconds: f32,
    ) -> Result<RecognitionAudioSample> {
        anyhow::bail!(unsupported_audio_message())
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
