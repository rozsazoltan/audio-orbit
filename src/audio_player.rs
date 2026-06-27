use crate::dsp::{render_orbit_to_stereo, DspSettings, RenderInfo};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{buffer::SamplesBuffer, Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::{
    collections::VecDeque,
    f32::consts::PI,
    fs,
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant},
};

const RADIO_VISUALIZER_HISTORY_SECONDS: usize = 180;
const RADIO_VISUALIZER_BUCKETS_PER_SECOND: usize = 6;
const RADIO_VISUALIZER_MAX_BUCKETS: usize = RADIO_VISUALIZER_HISTORY_SECONDS * RADIO_VISUALIZER_BUCKETS_PER_SECOND;

#[derive(Clone, Debug)]
pub struct PlaybackInfo {
    pub path: PathBuf,
    pub original_duration_seconds: f32,
    pub rendered_duration_seconds: f32,
    pub input_channels: u16,
    pub sample_rate: u32,
    pub size_bytes: Option<u64>,
    pub waveform: Vec<f32>,
    pub silence_ranges: Vec<(f32, f32)>,
}

pub struct PreparedPlayback {
    path: PathBuf,
    settings: DspSettings,
    start_seconds: f32,
    processed_samples: Vec<f32>,
    render_info: RenderInfo,
    sample_rate: u32,
}

struct RadioStream<R> {
    inner: Mutex<R>,
    position: u64,
}

impl<R> RadioStream<R> {
    fn new(inner: R) -> Self {
        Self {
            inner: Mutex::new(inner),
            position: 0,
        }
    }
}

impl<R: Read + Send> Read for RadioStream<R> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let mut inner = self
            .inner
            .lock()
            .map_err(|_| io::Error::new(io::ErrorKind::Other, "radio stream lock poisoned"))?;
        let read = inner.read(buffer)?;
        self.position += read as u64;
        Ok(read)
    }
}

impl<R> Seek for RadioStream<R> {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        match position {
            SeekFrom::Current(0) => Ok(self.position),
            SeekFrom::Start(current) if current == self.position => Ok(self.position),
            _ => Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "internet radio streams are not seekable",
            )),
        }
    }
}

#[derive(Clone, Copy)]
struct RadioVisualizerBucket {
    at: Instant,
    peak: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct RadioVisualizerPoint {
    pub age_seconds: f32,
    pub peak: f32,
}

#[derive(Clone, Debug, Default)]
pub struct RadioVisualizerFrame {
    pub points: Vec<RadioVisualizerPoint>,
    pub bucket_seconds: f32,
}

#[derive(Default)]
struct RadioVisualizerState {
    peaks: VecDeque<RadioVisualizerBucket>,
    smoothed_peak: f32,
}

type RadioVisualizerHandle = Arc<Mutex<RadioVisualizerState>>;

struct LiveRadioSource<S> {
    inner: S,
    settings: DspSettings,
    input_channels: u16,
    sample_rate: u32,
    frame_index: u64,
    output_frame: [f32; 2],
    output_channel: usize,
    visualizer: RadioVisualizerHandle,
    visualizer_current_peak: f32,
    visualizer_bucket_energy: f64,
    visualizer_bucket_sample_count: usize,
    visualizer_sample_counter: usize,
}

impl<S: Source<Item = f32>> LiveRadioSource<S> {
    fn new(inner: S, settings: DspSettings, visualizer: RadioVisualizerHandle) -> Self {
        let input_channels = inner.channels().max(1);
        let sample_rate = inner.sample_rate().max(1);
        Self {
            inner,
            settings,
            input_channels,
            sample_rate,
            frame_index: 0,
            output_frame: [0.0, 0.0],
            output_channel: 2,
            visualizer,
            visualizer_current_peak: 0.0,
            visualizer_bucket_energy: 0.0,
            visualizer_bucket_sample_count: 0,
            visualizer_sample_counter: 0,
        }
    }

    fn read_input_frame(&mut self) -> Option<([f32; 2], f32)> {
        let channels = self.input_channels.max(1) as usize;
        let mut sum = 0.0_f32;
        let mut count = 0usize;
        let mut left = 0.0_f32;
        let mut right = 0.0_f32;

        for channel in 0..channels {
            match self.inner.next() {
                Some(sample) => {
                    if channel == 0 {
                        left = sample;
                    } else if channel == 1 {
                        right = sample;
                    }
                    sum += sample;
                    count += 1;
                }
                None if count == 0 => return None,
                None => break,
            }
        }

        if count == 0 {
            None
        } else {
            if count == 1 {
                right = left;
            }
            Some(([left, right], sum / count as f32))
        }
    }

    fn record_visualizer_peak(&mut self, peak: f32) {
        let level = peak.abs().min(1.0);
        self.visualizer_current_peak = self.visualizer_current_peak.max(level);
        self.visualizer_bucket_energy += (level as f64) * (level as f64);
        self.visualizer_bucket_sample_count += 1;
        self.visualizer_sample_counter += 1;

        let samples_per_bucket = (self.sample_rate.max(1) as usize / RADIO_VISUALIZER_BUCKETS_PER_SECOND).max(1);
        if self.visualizer_sample_counter < samples_per_bucket {
            return;
        }

        let rms = if self.visualizer_bucket_sample_count == 0 {
            0.0
        } else {
            (self.visualizer_bucket_energy / self.visualizer_bucket_sample_count as f64).sqrt() as f32
        };
        let envelope = (rms * 1.65 + self.visualizer_current_peak * 0.28).clamp(0.0, 1.0);
        let now = Instant::now();

        if let Ok(mut state) = self.visualizer.lock() {
            let previous = state.peaks.back().map(|bucket| bucket.peak).unwrap_or(state.smoothed_peak);
            state.smoothed_peak = if envelope > previous {
                previous * 0.30 + envelope * 0.70
            } else {
                previous * 0.82 + envelope * 0.18
            };
            let display_peak = state.smoothed_peak.clamp(0.0, 1.0);
            state.peaks.push_back(RadioVisualizerBucket {
                at: now,
                peak: display_peak,
            });

            let history = Duration::from_secs(RADIO_VISUALIZER_HISTORY_SECONDS as u64);
            while state.peaks.len() > RADIO_VISUALIZER_MAX_BUCKETS
                || state
                    .peaks
                    .front()
                    .map(|bucket| now.duration_since(bucket.at) > history)
                    .unwrap_or(false)
            {
                state.peaks.pop_front();
            }
        }

        self.visualizer_current_peak = 0.0;
        self.visualizer_bucket_energy = 0.0;
        self.visualizer_bucket_sample_count = 0;
        self.visualizer_sample_counter = 0;
    }

    fn process_frame(&mut self, stereo: [f32; 2], mono: f32) -> [f32; 2] {
        self.record_visualizer_peak(stereo[0].abs().max(stereo[1].abs()).max(mono.abs()));
        let output_level = self.settings.output_level_percent.clamp(1, 100) as f32 / 100.0;
        if !self.settings.orbit_enabled {
            return [
                soft_limit_radio(stereo[0] * output_level),
                soft_limit_radio(stereo[1] * output_level),
            ];
        }

        let width = self.settings.stereo_width_percent.min(100) as f32 / 100.0;
        let speed = self.settings.orbit_speed_percent.clamp(10, 200) as f32 / 100.0;
        let time = self.frame_index as f32 / self.sample_rate as f32;
        let pan = (2.0 * PI * 0.20 * speed * time).sin() * width;
        let angle = (pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
        let mut left_gain = angle.cos();
        let mut right_gain = angle.sin();

        if matches!(self.settings.mode, crate::dsp::OrbitMode::VirtualEightDirectionOrbit) {
            let depth = (2.0 * PI * 0.20 * speed * time).cos();
            let rear = (-depth).max(0.0) * (self.settings.depth_cue_percent.min(100) as f32 / 100.0);
            let shade = 1.0 - rear * 0.22;
            left_gain *= shade;
            right_gain *= shade;
        }

        [
            soft_limit_radio(mono * left_gain * output_level),
            soft_limit_radio(mono * right_gain * output_level),
        ]
    }
}

impl<S: Source<Item = f32>> Iterator for LiveRadioSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.output_channel < 2 {
            let sample = self.output_frame[self.output_channel];
            self.output_channel += 1;
            return Some(sample);
        }

        let (stereo, mono) = self.read_input_frame()?;
        self.output_frame = self.process_frame(stereo, mono);
        self.output_channel = 1;
        self.frame_index = self.frame_index.saturating_add(1);
        Some(self.output_frame[0])
    }
}

impl<S: Source<Item = f32>> Source for LiveRadioSource<S> {
    fn current_frame_len(&self) -> Option<usize> {
        None
    }

    fn channels(&self) -> u16 {
        2
    }

    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn total_duration(&self) -> Option<Duration> {
        None
    }
}

fn soft_limit_radio(value: f32) -> f32 {
    (value / (1.0 + value.abs() * 0.12)).clamp(-1.0, 1.0)
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
    current_radio_url: Option<String>,
    volume_percent: u8,
    radio_visualizer: RadioVisualizerHandle,
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
            current_radio_url: None,
            volume_percent: 100,
            radio_visualizer: Arc::new(Mutex::new(RadioVisualizerState::default())),
        })
    }

    pub fn output_device_name(&self) -> &str {
        &self.output_device_name
    }

    pub fn set_volume_percent(&mut self, volume_percent: u8) {
        self.volume_percent = volume_percent.clamp(0, 100);
        if let Some(sink) = &self.sink {
            sink.set_volume(self.volume_gain());
        }
    }

    fn volume_gain(&self) -> f32 {
        self.volume_percent as f32 / 100.0
    }

    pub fn play_radio_stream(&mut self, url: &str, settings: DspSettings) -> Result<()> {
        let response = reqwest::blocking::Client::builder()
            .user_agent("Audio-Orbit-Radio")
            .build()?
            .get(url)
            .send()
            .with_context(|| format!("failed to open internet radio stream: {url}"))?
            .error_for_status()
            .with_context(|| format!("internet radio stream returned an error: {url}"))?;

        let stream = RadioStream::new(response);
        let decoder = Decoder::new(BufReader::new(stream))
            .with_context(|| format!("failed to decode internet radio stream: {url}"))?;

        let keep_visualizer_history = self.current_radio_url.as_deref() == Some(url);
        self.stop();
        if !keep_visualizer_history {
            self.radio_visualizer = Arc::new(Mutex::new(RadioVisualizerState::default()));
        }
        let visualizer = Arc::clone(&self.radio_visualizer);
        let radio_source = LiveRadioSource::new(decoder.convert_samples::<f32>(), settings, visualizer);
        let sink = Sink::try_new(&self.stream_handle)
            .context("failed to create audio playback sink")?;
        sink.set_volume(self.volume_gain());
        sink.append(radio_source);
        sink.play();

        self.sink = Some(sink);
        self.started_at = Some(Instant::now());
        self.paused_at = None;
        self.accumulated_pause = Duration::ZERO;
        self.current_duration = None;
        self.current_start_offset_seconds = 0.0;
        self.current_path = None;
        self.current_settings = None;
        self.current_radio_url = Some(url.to_owned());

        Ok(())
    }

    pub fn radio_visualizer_frame(&self, requested_points: usize) -> RadioVisualizerFrame {
        let Ok(mut state) = self.radio_visualizer.lock() else {
            return RadioVisualizerFrame::default();
        };
        if requested_points == 0 || state.peaks.is_empty() {
            return RadioVisualizerFrame::default();
        }

        let now = Instant::now();
        let bucket_seconds = 1.0 / RADIO_VISUALIZER_BUCKETS_PER_SECOND as f32;
        let visible_seconds = requested_points as f32 * bucket_seconds;
        let overscan_seconds = bucket_seconds * 2.0;

        while state
            .peaks
            .front()
            .map(|bucket| now.duration_since(bucket.at).as_secs_f32() > visible_seconds + overscan_seconds)
            .unwrap_or(false)
        {
            state.peaks.pop_front();
        }
        while state.peaks.len() > requested_points + 4 {
            state.peaks.pop_front();
        }

        let points = state
            .peaks
            .iter()
            .filter_map(|bucket| {
                let age_seconds = now.duration_since(bucket.at).as_secs_f32();
                if age_seconds > visible_seconds + overscan_seconds {
                    None
                } else {
                    Some(RadioVisualizerPoint {
                        age_seconds,
                        peak: bucket.peak,
                    })
                }
            })
            .collect();

        RadioVisualizerFrame {
            points,
            bucket_seconds,
        }
    }

    pub fn prepare_file(path: PathBuf, settings: DspSettings, start_seconds: f32) -> Result<PreparedPlayback> {
        let (processed_samples, render_info, sample_rate) = render_file_data(&path, settings, start_seconds)?;
        Ok(PreparedPlayback {
            path,
            settings,
            start_seconds,
            processed_samples,
            render_info,
            sample_rate,
        })
    }

    pub fn play_prepared(&mut self, prepared: PreparedPlayback) -> Result<PlaybackInfo> {
        let PreparedPlayback {
            path,
            settings,
            start_seconds,
            processed_samples,
            render_info,
            sample_rate,
        } = prepared;

        self.stop();
        let rendered_duration = Duration::from_secs_f32(render_info.rendered_duration_seconds.max(0.0));
        self.play_processed_samples(processed_samples, sample_rate, rendered_duration, &path, settings, start_seconds)?;

        Ok(playback_info(&path, render_info))
    }

    pub fn play_prepared_from_live_position(
        &mut self,
        mut prepared: PreparedPlayback,
        render_elapsed_seconds: f32,
    ) -> Result<PlaybackInfo> {
        let compensated_start_seconds = prepared.start_seconds + render_elapsed_seconds.max(0.0);

        if render_elapsed_seconds > 0.025 {
            let trim_frames = (render_elapsed_seconds * prepared.sample_rate as f32).round().max(0.0) as usize;
            let trim_samples = (trim_frames * 2).min(prepared.processed_samples.len());
            if trim_samples > 0 && trim_samples < prepared.processed_samples.len() {
                prepared.processed_samples.drain(0..trim_samples);
                prepared.render_info.rendered_duration_seconds = (prepared.render_info.rendered_duration_seconds - render_elapsed_seconds).max(0.0);
            }
        }

        self.stop();
        let rendered_duration = Duration::from_secs_f32(prepared.render_info.rendered_duration_seconds.max(0.0));
        self.play_processed_samples(
            prepared.processed_samples,
            prepared.sample_rate,
            rendered_duration,
            &prepared.path,
            prepared.settings,
            compensated_start_seconds,
        )?;

        Ok(playback_info(&prepared.path, prepared.render_info))
    }

    pub fn play_file_with_orbit_from(
        &mut self,
        path: &Path,
        settings: DspSettings,
        start_seconds: f32,
    ) -> Result<PlaybackInfo> {
        let prepared = Self::prepare_file(path.to_path_buf(), settings, start_seconds)?;
        self.play_prepared(prepared)
    }

    pub fn crossfade_to_prepared(
        &mut self,
        mut prepared: PreparedPlayback,
        crossfade_seconds: f32,
    ) -> Result<PlaybackInfo> {
        let fade_seconds = crossfade_seconds
            .max(0.0)
            .min(prepared.render_info.rendered_duration_seconds.max(0.0));

        apply_fade_in(&mut prepared.processed_samples, prepared.sample_rate, fade_seconds);

        if fade_seconds > 0.05 {
            if let Some(old_sink) = self.sink.take() {
                fade_out_and_stop(old_sink, fade_seconds, self.volume_gain());
            }
        } else {
            self.stop();
        }

        let rendered_duration = Duration::from_secs_f32(prepared.render_info.rendered_duration_seconds.max(0.0));
        self.play_processed_samples(
            prepared.processed_samples,
            prepared.sample_rate,
            rendered_duration,
            &prepared.path,
            prepared.settings,
            prepared.start_seconds,
        )?;

        Ok(playback_info(&prepared.path, prepared.render_info))
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
        self.current_radio_url = None;
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

    pub fn current_start_offset_seconds(&self) -> f32 {
        self.current_start_offset_seconds
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

        sink.set_volume(self.volume_gain());
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
        self.current_radio_url = None;

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

fn fade_out_and_stop(sink: Sink, fade_seconds: f32, base_volume: f32) {
    let steps = ((fade_seconds * 30.0) as usize).clamp(8, 180);
    let sleep_duration = Duration::from_secs_f32((fade_seconds / steps as f32).max(0.005));

    thread::spawn(move || {
        for step in 0..steps {
            let remaining = 1.0 - (step as f32 / steps as f32);
            sink.set_volume((base_volume * remaining).max(0.0));
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

fn render_file_data(
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

    let input_samples: Vec<f32> = decoder.convert_samples::<f32>().collect();
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

fn playback_info(path: &Path, render_info: RenderInfo) -> PlaybackInfo {
    PlaybackInfo {
        path: path.to_path_buf(),
        original_duration_seconds: render_info.original_duration_seconds,
        rendered_duration_seconds: render_info.rendered_duration_seconds,
        input_channels: render_info.input_channels,
        sample_rate: render_info.sample_rate,
        size_bytes: fs::metadata(path).ok().map(|metadata| metadata.len()),
        waveform: render_info.waveform,
        silence_ranges: render_info.silence_ranges,
    }
}
