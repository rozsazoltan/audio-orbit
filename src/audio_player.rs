use crate::dsp::{render_orbit_to_stereo, DspSettings, RenderInfo};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{buffer::SamplesBuffer, Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::{
    fs,
    fs::File,
    io::BufReader,
    path::{Path, PathBuf},
    thread,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct PlaybackInfo {
    pub path: PathBuf,
    pub original_duration_seconds: f32,
    pub rendered_duration_seconds: f32,
    pub input_channels: u16,
    pub sample_rate: u32,
    pub output_samples: usize,
    pub size_bytes: Option<u64>,
    pub waveform: Vec<f32>,
}

pub struct AudioPlayer {
    _stream: OutputStream,
    stream_handle: OutputStreamHandle,
    sink: Option<Sink>,
    output_device_name: String,
    started_at: Option<Instant>,
    paused_at: Option<Instant>,
    accumulated_pause: Duration,
    current_duration: Option<Duration>,
    current_start_offset_seconds: f32,
    current_path: Option<PathBuf>,
    current_settings: Option<DspSettings>,
}

impl AudioPlayer {
    pub fn new() -> Result<Self> {
        let output_device_name = current_default_output_device_name();
        let (_stream, stream_handle) = OutputStream::try_default()
            .context("failed to open the default audio output device")?;

        Ok(Self {
            _stream,
            stream_handle,
            sink: None,
            output_device_name,
            started_at: None,
            paused_at: None,
            accumulated_pause: Duration::ZERO,
            current_duration: None,
            current_start_offset_seconds: 0.0,
            current_path: None,
            current_settings: None,
        })
    }

    pub fn output_device_name(&self) -> &str {
        &self.output_device_name
    }

    pub fn play_file_with_orbit(&mut self, path: &Path, settings: DspSettings) -> Result<PlaybackInfo> {
        self.play_file_with_orbit_from(path, settings, 0.0)
    }

    pub fn play_file_with_orbit_from(
        &mut self,
        path: &Path,
        settings: DspSettings,
        start_seconds: f32,
    ) -> Result<PlaybackInfo> {
        let (processed_samples, render_info, sample_rate) = self.render_file(path, settings, start_seconds)?;

        self.stop();
        let rendered_duration = Duration::from_secs_f32(render_info.rendered_duration_seconds.max(0.0));
        self.play_processed_samples(processed_samples, sample_rate, rendered_duration, path, settings, start_seconds)?;

        Ok(playback_info(path, render_info))
    }

    pub fn crossfade_to_file_with_orbit_from(
        &mut self,
        path: &Path,
        settings: DspSettings,
        start_seconds: f32,
        crossfade_seconds: f32,
    ) -> Result<PlaybackInfo> {
        let (mut processed_samples, render_info, sample_rate) = self.render_file(path, settings, start_seconds)?;
        let fade_seconds = crossfade_seconds
            .max(0.0)
            .min(render_info.rendered_duration_seconds.max(0.0));

        apply_fade_in(&mut processed_samples, sample_rate, fade_seconds);

        if fade_seconds > 0.05 {
            if let Some(old_sink) = self.sink.take() {
                fade_out_and_stop(old_sink, fade_seconds);
            }
        } else {
            self.stop();
        }

        let rendered_duration = Duration::from_secs_f32(render_info.rendered_duration_seconds.max(0.0));
        self.play_processed_samples(processed_samples, sample_rate, rendered_duration, path, settings, start_seconds)?;

        Ok(playback_info(path, render_info))
    }

    fn render_file(
        &self,
        path: &Path,
        settings: DspSettings,
        start_seconds: f32,
    ) -> Result<(Vec<f32>, RenderInfo, u32)> {
        let file = File::open(path)
            .with_context(|| format!("failed to open audio file: {}", path.display()))?;
        let decoder = Decoder::new(BufReader::new(file))
            .with_context(|| format!("failed to decode audio file: {}", path.display()))?;

        let input_channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        if sample_rate == 0 {
            anyhow::bail!("the selected audio file reported an invalid sample rate");
        }

        let input_samples: Vec<f32> = decoder.convert_samples().collect();
        if input_samples.is_empty() {
            anyhow::bail!("the selected audio file did not contain any decoded samples");
        }

        let (processed_samples, render_info) =
            render_orbit_to_stereo(&input_samples, input_channels, sample_rate, settings, start_seconds);

        if processed_samples.is_empty() {
            anyhow::bail!("the rendered audio was empty after processing; disable silence skip or seek earlier in the track");
        }

        Ok((processed_samples, render_info, sample_rate))
    }

    pub fn seek_current(&mut self, seconds: f32) -> Result<Option<PlaybackInfo>> {
        let Some(path) = self.current_path.clone() else {
            return Ok(None);
        };
        let Some(settings) = self.current_settings else {
            return Ok(None);
        };

        self.play_file_with_orbit_from(&path, settings, seconds).map(Some)
    }

    pub fn apply_settings_to_current(&mut self, settings: DspSettings) -> Result<Option<PlaybackInfo>> {
        let position = self.playback_position_seconds();
        let Some(path) = self.current_path.clone() else {
            return Ok(None);
        };

        self.play_file_with_orbit_from(&path, settings, position).map(Some)
    }

    pub fn stop(&mut self) {
        if let Some(sink) = self.sink.take() {
            sink.stop();
        }

        self.started_at = None;
        self.paused_at = None;
        self.accumulated_pause = Duration::ZERO;
        self.current_duration = None;
        self.current_start_offset_seconds = 0.0;
        self.current_path = None;
        self.current_settings = None;
    }

    pub fn pause_or_resume(&mut self) {
        let Some(sink) = &self.sink else {
            return;
        };

        if sink.is_paused() {
            if let Some(paused_at) = self.paused_at.take() {
                self.accumulated_pause += paused_at.elapsed();
            }
            sink.play();
        } else {
            self.paused_at = Some(Instant::now());
            sink.pause();
        }
    }

    pub fn is_playing(&self) -> bool {
        self.sink
            .as_ref()
            .map(|sink| !sink.empty() && !sink.is_paused())
            .unwrap_or(false)
    }

    pub fn is_paused(&self) -> bool {
        self.sink
            .as_ref()
            .map(|sink| !sink.empty() && sink.is_paused())
            .unwrap_or(false)
    }

    pub fn has_finished(&self) -> bool {
        self.sink.as_ref().map(|sink| sink.empty()).unwrap_or(false)
    }

    pub fn playback_position_seconds(&self) -> f32 {
        let Some(started_at) = self.started_at else {
            return 0.0;
        };

        let now = self.paused_at.unwrap_or_else(Instant::now);
        let elapsed = now
            .saturating_duration_since(started_at)
            .saturating_sub(self.accumulated_pause);

        let position = self.current_start_offset_seconds + elapsed.as_secs_f32();

        match self.current_duration {
            Some(duration) => position.min(self.current_start_offset_seconds + duration.as_secs_f32()),
            None => position,
        }
    }

    pub fn playback_duration_seconds(&self) -> Option<f32> {
        self.current_duration
            .map(|duration| self.current_start_offset_seconds + duration.as_secs_f32())
    }

    fn play_processed_samples(
        &mut self,
        samples: Vec<f32>,
        sample_rate: u32,
        duration: Duration,
        path: &Path,
        settings: DspSettings,
        start_seconds: f32,
    ) -> Result<()> {
        let sink = Sink::try_new(&self.stream_handle)
            .context("failed to create audio playback sink")?;
        let source = SamplesBuffer::new(2, sample_rate, samples);

        sink.append(source);
        sink.play();
        self.sink = Some(sink);
        self.started_at = Some(Instant::now());
        self.paused_at = None;
        self.accumulated_pause = Duration::ZERO;
        self.current_duration = Some(duration);
        self.current_start_offset_seconds = start_seconds.max(0.0);
        self.current_path = Some(path.to_path_buf());
        self.current_settings = Some(settings);

        Ok(())
    }
}

fn apply_fade_in(samples: &mut [f32], sample_rate: u32, fade_seconds: f32) {
    if fade_seconds <= 0.0 || sample_rate == 0 {
        return;
    }

    let frame_count = samples.len() / 2;
    let fade_frames = ((fade_seconds * sample_rate as f32) as usize).min(frame_count);
    if fade_frames == 0 {
        return;
    }

    for frame in 0..fade_frames {
        let gain = frame as f32 / fade_frames as f32;
        let left = frame * 2;
        let right = left + 1;
        samples[left] *= gain;
        samples[right] *= gain;
    }
}

fn fade_out_and_stop(sink: Sink, fade_seconds: f32) {
    let steps = ((fade_seconds * 30.0) as usize).clamp(8, 180);
    let sleep_duration = Duration::from_secs_f32((fade_seconds / steps as f32).max(0.005));

    thread::spawn(move || {
        for step in 0..steps {
            let remaining = 1.0 - (step as f32 / steps as f32);
            sink.set_volume(remaining.max(0.0));
            thread::sleep(sleep_duration);
        }
        sink.stop();
    });
}

pub fn current_default_output_device_name() -> String {
    let host = cpal::default_host();

    host.default_output_device()
        .and_then(|device| device.name().ok())
        .unwrap_or_else(|| "Default output device".to_owned())
}

fn playback_info(path: &Path, render_info: RenderInfo) -> PlaybackInfo {
    PlaybackInfo {
        path: path.to_path_buf(),
        original_duration_seconds: render_info.original_duration_seconds,
        rendered_duration_seconds: render_info.rendered_duration_seconds,
        input_channels: render_info.input_channels,
        sample_rate: render_info.sample_rate,
        output_samples: render_info.output_samples,
        size_bytes: fs::metadata(path).ok().map(|metadata| metadata.len()),
        waveform: render_info.waveform,
    }
}
