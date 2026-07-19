use crate::dsp::{render_orbit_to_stereo_with_cached_analysis, DspSettings, RenderInfo};
use anyhow::{Context, Result};
use cpal::traits::{DeviceTrait, HostTrait};
use rodio::{buffer::SamplesBuffer, Decoder, OutputStream, OutputStreamHandle, Sink, Source};
use std::{
    collections::VecDeque,
    f32::consts::PI,
    fs,
    fs::File,
    io::{self, BufReader, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RADIO_VISUALIZER_HISTORY_SECONDS: usize = 180;
// Keep the raw radio envelope at a higher resolution than the UI needs.
// The UI then bins these live samples into one value per horizontal pixel,
// which avoids synthetic/repeating patterns and keeps the strip tied to the
// decoded audio itself.
const RADIO_VISUALIZER_BUCKETS_PER_SECOND: usize = 64;
const RADIO_VISUALIZER_MAX_BUCKETS: usize =
    RADIO_VISUALIZER_HISTORY_SECONDS * RADIO_VISUALIZER_BUCKETS_PER_SECOND;
// The orbit position changes very slowly compared to the audio sample rate.
// Updating gain coefficients once per small block avoids expensive sin/cos work
// for every single decoded frame while keeping the movement perceptually smooth.
const LIVE_ORBIT_GAIN_UPDATE_FRAMES: u64 = 128;

#[derive(Clone, Copy, Debug)]
struct LiveOrbitGains {
    left: f32,
    right: f32,
    output_level: f32,
}

impl Default for LiveOrbitGains {
    fn default() -> Self {
        Self {
            left: 1.0,
            right: 1.0,
            output_level: 1.0,
        }
    }
}

fn live_orbit_gains(settings: DspSettings, frame_index: u64, sample_rate: u32) -> LiveOrbitGains {
    let output_level = settings.output_level_percent.clamp(1, 100) as f32 / 100.0;
    if !settings.orbit_enabled {
        return LiveOrbitGains {
            output_level,
            ..Default::default()
        };
    }

    let sample_rate = sample_rate.max(1) as f32;
    let width = settings.stereo_width_percent.min(100) as f32 / 100.0;
    let speed = settings.orbit_speed_percent.clamp(10, 200) as f32 / 100.0;
    let phase = 2.0 * PI * 0.20 * speed * (frame_index as f32 / sample_rate);
    let pan = phase.sin() * width;
    let angle = (pan.clamp(-1.0, 1.0) + 1.0) * PI / 4.0;
    let mut left = angle.cos();
    let mut right = angle.sin();

    if matches!(
        settings.mode,
        crate::dsp::OrbitMode::VirtualEightDirectionOrbit
    ) {
        let depth = phase.cos();
        let rear = (-depth).max(0.0) * (settings.depth_cue_percent.min(100) as f32 / 100.0);
        let shade = 1.0 - rear * 0.22;
        left *= shade;
        right *= shade;
    }

    LiveOrbitGains {
        left,
        right,
        output_level,
    }
}

#[derive(Clone, Debug)]
pub struct PlaybackInfo {
    pub path: PathBuf,
    pub original_duration_seconds: f32,
    pub input_channels: u16,
    pub sample_rate: u32,
    pub size_bytes: Option<u64>,
    pub waveform: Vec<f32>,
    pub waveform_brightness: Vec<f32>,
    pub silence_ranges: Vec<(f32, f32)>,
}

#[derive(Clone, Debug)]
pub struct PreparedPlayback {
    path: PathBuf,
    settings: DspSettings,
    start_seconds: f32,
    processed_samples: Vec<f32>,
    render_info: RenderInfo,
    sample_rate: u32,
}

impl PreparedPlayback {
    pub fn silence_analysis_cache_data(&self) -> (&Path, DspSettings, Vec<(f32, f32)>) {
        (
            self.path.as_path(),
            self.settings,
            self.render_info.silence_ranges.clone(),
        )
    }
}

#[derive(Clone, Debug)]
pub struct RadioRecordingInfo {
    pub path: PathBuf,
    pub started_at: Instant,
    pub bytes_written: u64,
}

struct ActiveRadioRecording {
    file: File,
    path: PathBuf,
    started_at: Instant,
    bytes_written: u64,
}

type RadioRecordingHandle = Arc<Mutex<Option<ActiveRadioRecording>>>;

struct RadioStream<R> {
    inner: Mutex<R>,
    position: u64,
    recorder: RadioRecordingHandle,
}

impl<R> RadioStream<R> {
    fn new(inner: R, recorder: RadioRecordingHandle) -> Self {
        Self {
            inner: Mutex::new(inner),
            position: 0,
            recorder,
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
        if read > 0 {
            if let Ok(mut recording) = self.recorder.lock() {
                if let Some(recording) = recording.as_mut() {
                    if recording.file.write_all(&buffer[..read]).is_ok() {
                        recording.bytes_written =
                            recording.bytes_written.saturating_add(read as u64);
                    }
                }
            }
        }
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
    level: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct RadioVisualizerBar {
    pub peak: f32,
}

#[derive(Clone, Debug, Default)]
pub struct RadioVisualizerFrame {
    pub bars: Vec<RadioVisualizerBar>,
}

struct RadioVisualizerState {
    peaks: VecDeque<RadioVisualizerBucket>,
    display_floor: f32,
    display_peak: f32,
}

impl Default for RadioVisualizerState {
    fn default() -> Self {
        Self {
            peaks: VecDeque::new(),
            // A live stream cannot be normalized against a full track. Keep a slow
            // visual range so radio does not pump frame-by-frame, but do not smooth
            // the actual audio buckets: the strip must be built from the live music.
            display_floor: 0.018,
            display_peak: 0.42,
        }
    }
}

type RadioVisualizerHandle = Arc<Mutex<RadioVisualizerState>>;

struct LiveRadioWaveformAnalyzer {
    samples_per_bucket: usize,
    samples_in_bucket: usize,
    sum_squares: f64,
    peak: f32,
}

impl LiveRadioWaveformAnalyzer {
    fn new(sample_rate: u32, buckets_per_second: usize) -> Self {
        Self {
            samples_per_bucket: (sample_rate as usize / buckets_per_second.max(1)).max(1),
            samples_in_bucket: 0,
            sum_squares: 0.0,
            peak: 0.0,
        }
    }

    fn push_sample(&mut self, sample: f32) -> Option<f32> {
        let sample = sample.clamp(-1.0, 1.0);
        let abs = sample.abs();
        self.sum_squares += (sample as f64) * (sample as f64);
        self.peak = self.peak.max(abs);
        self.samples_in_bucket += 1;

        if self.samples_in_bucket < self.samples_per_bucket {
            return None;
        }

        let rms = (self.sum_squares / self.samples_in_bucket.max(1) as f64).sqrt() as f32;
        self.samples_in_bucket = 0;
        self.sum_squares = 0.0;
        let peak = std::mem::take(&mut self.peak);

        // Real live waveform envelope. Use the decoded audio's short-window RMS
        // as the main shape, with only a small peak component so compressed radio
        // streams do not turn into a full-height rectangle. No attack/release
        // smoothing here: smoothing created the fake/repeating pattern.
        let loudness = db_to_unit_for_radio(20.0 * rms.max(0.000_001).log10(), -52.0, -7.0, 1.18);
        let transient = peak.clamp(0.0, 1.0).powf(0.95);
        Some((loudness * 0.94 + transient * 0.06).clamp(0.0, 1.0))
    }
}

fn db_to_unit_for_radio(db: f32, min_db: f32, max_db: f32, power: f32) -> f32 {
    ((db - min_db) / (max_db - min_db).max(0.001))
        .clamp(0.0, 1.0)
        .powf(power)
}

struct FadeInSource<S> {
    inner: S,
    total_samples: u64,
    emitted_samples: u64,
}

impl<S: Source<Item = f32>> FadeInSource<S> {
    fn new(inner: S, fade_seconds: f32) -> Self {
        let total_samples = if fade_seconds > 0.0 {
            (fade_seconds * inner.sample_rate() as f32 * inner.channels().max(1) as f32)
                .round()
                .max(0.0) as u64
        } else {
            0
        };

        Self {
            inner,
            total_samples,
            emitted_samples: 0,
        }
    }
}

impl<S: Source<Item = f32>> Iterator for FadeInSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        let sample = self.inner.next()?;
        if self.total_samples == 0 || self.emitted_samples >= self.total_samples {
            self.emitted_samples = self.emitted_samples.saturating_add(1);
            return Some(sample);
        }

        let gain = (self.emitted_samples as f32 / self.total_samples as f32).clamp(0.0, 1.0);
        self.emitted_samples = self.emitted_samples.saturating_add(1);
        Some(sample * gain)
    }
}

impl<S: Source<Item = f32>> Source for FadeInSource<S> {
    fn current_frame_len(&self) -> Option<usize> {
        self.inner.current_frame_len()
    }

    fn channels(&self) -> u16 {
        self.inner.channels()
    }

    fn sample_rate(&self) -> u32 {
        self.inner.sample_rate()
    }

    fn total_duration(&self) -> Option<Duration> {
        self.inner.total_duration()
    }
}

fn fill_radio_waveform_gaps(values: &mut [f32]) {
    let mut previous: Option<(usize, f32)> = None;

    for index in 0..values.len() {
        if values[index] <= 0.0005 {
            continue;
        }

        if let Some((previous_index, previous_value)) = previous {
            let gap = index.saturating_sub(previous_index + 1);
            if gap > 0 && gap <= 3 {
                let current_value = values[index];
                for offset in 1..=gap {
                    let mix = offset as f32 / (gap + 1) as f32;
                    values[previous_index + offset] =
                        previous_value * (1.0 - mix) + current_value * mix;
                }
            }
        }

        previous = Some((index, values[index]));
    }
}

fn silence_adjusted_position(seconds: f32, silence_ranges: Option<&[(f32, f32)]>) -> f32 {
    let mut position = if seconds.is_finite() {
        seconds.max(0.0)
    } else {
        0.0
    };
    let Some(ranges) = silence_ranges else {
        return position;
    };

    for _ in 0..8 {
        let previous = position;
        for (start, end) in ranges {
            if *end > *start && position >= *start && position < *end {
                position = *end;
            }
        }
        if (position - previous).abs() < 0.001 {
            break;
        }
    }

    position
}

fn silence_ranges_to_frame_ranges(
    ranges: Option<&[(f32, f32)]>,
    sample_rate: u32,
) -> Vec<(u64, u64)> {
    let Some(ranges) = ranges else {
        return Vec::new();
    };
    let sample_rate = sample_rate.max(1) as f32;
    let mut frame_ranges = ranges
        .iter()
        .filter_map(|(start, end)| {
            if !start.is_finite() || !end.is_finite() || *end <= *start {
                return None;
            }
            let start_frame = (*start * sample_rate).round().max(0.0) as u64;
            let end_frame = (*end * sample_rate).round().max(0.0) as u64;
            (end_frame > start_frame).then_some((start_frame, end_frame))
        })
        .collect::<Vec<_>>();
    frame_ranges.sort_by_key(|(start, _)| *start);
    frame_ranges
}

struct LiveFileSource<S> {
    inner: S,
    settings: DspSettings,
    input_channels: u16,
    sample_rate: u32,
    frame_index: u64,
    output_frame: [f32; 2],
    output_channel: usize,
    cached_gains: LiveOrbitGains,
    cached_gains_until_frame: u64,
    skip_ranges: Vec<(u64, u64)>,
    skip_index: usize,
}

impl<S: Source<Item = f32>> LiveFileSource<S> {
    fn new(
        inner: S,
        settings: DspSettings,
        start_seconds: f32,
        silence_ranges: Option<Vec<(f32, f32)>>,
    ) -> Self {
        let input_channels = inner.channels().max(1);
        let sample_rate = inner.sample_rate().max(1);
        let start_seconds = silence_adjusted_position(start_seconds, silence_ranges.as_deref());
        let frame_index = (start_seconds * sample_rate as f32).round().max(0.0) as u64;
        let skip_ranges = silence_ranges_to_frame_ranges(silence_ranges.as_deref(), sample_rate);
        let skip_index = skip_ranges
            .iter()
            .position(|(_, end)| *end > frame_index)
            .unwrap_or(skip_ranges.len());
        Self {
            inner,
            settings,
            input_channels,
            sample_rate,
            frame_index,
            output_frame: [0.0, 0.0],
            output_channel: 2,
            cached_gains: LiveOrbitGains::default(),
            cached_gains_until_frame: 0,
            skip_ranges,
            skip_index,
        }
    }

    fn current_gains(&mut self) -> LiveOrbitGains {
        if self.frame_index >= self.cached_gains_until_frame {
            self.cached_gains = live_orbit_gains(self.settings, self.frame_index, self.sample_rate);
            self.cached_gains_until_frame = self
                .frame_index
                .saturating_add(LIVE_ORBIT_GAIN_UPDATE_FRAMES);
        }
        self.cached_gains
    }

    fn discard_input_frame(&mut self) -> bool {
        let channels = self.input_channels.max(1) as usize;
        let mut read_any = false;
        for _ in 0..channels {
            if self.inner.next().is_some() {
                read_any = true;
            } else {
                break;
            }
        }
        read_any
    }

    fn skip_silent_frames_if_needed(&mut self) -> bool {
        loop {
            let Some((start, end)) = self.skip_ranges.get(self.skip_index).copied() else {
                return true;
            };
            if self.frame_index >= end {
                self.skip_index += 1;
                continue;
            }
            if self.frame_index < start {
                return true;
            }

            while self.frame_index < end {
                if !self.discard_input_frame() {
                    return false;
                }
                self.frame_index = self.frame_index.saturating_add(1);
            }
            self.skip_index += 1;
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

    fn process_frame(&mut self, stereo: [f32; 2], mono: f32) -> [f32; 2] {
        let gains = self.current_gains();
        if !self.settings.orbit_enabled {
            return [
                (stereo[0] * gains.output_level).clamp(-1.0, 1.0),
                (stereo[1] * gains.output_level).clamp(-1.0, 1.0),
            ];
        }

        [
            soft_limit_radio(mono * gains.left * gains.output_level),
            soft_limit_radio(mono * gains.right * gains.output_level),
        ]
    }
}

impl<S: Source<Item = f32>> Iterator for LiveFileSource<S> {
    type Item = f32;

    fn next(&mut self) -> Option<Self::Item> {
        if self.output_channel < 2 {
            let sample = self.output_frame[self.output_channel];
            self.output_channel += 1;
            return Some(sample);
        }

        if !self.skip_silent_frames_if_needed() {
            return None;
        }

        let (stereo, mono) = self.read_input_frame()?;
        self.output_frame = self.process_frame(stereo, mono);
        self.output_channel = 1;
        self.frame_index = self.frame_index.saturating_add(1);
        Some(self.output_frame[0])
    }
}

impl<S: Source<Item = f32>> Source for LiveFileSource<S> {
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

struct LiveRadioSource<S> {
    inner: S,
    settings: DspSettings,
    input_channels: u16,
    sample_rate: u32,
    frame_index: u64,
    output_frame: [f32; 2],
    output_channel: usize,
    cached_gains: LiveOrbitGains,
    cached_gains_until_frame: u64,
    visualizer: RadioVisualizerHandle,
    visualizer_analyzer: LiveRadioWaveformAnalyzer,
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
            cached_gains: LiveOrbitGains::default(),
            cached_gains_until_frame: 0,
            visualizer,
            visualizer_analyzer: LiveRadioWaveformAnalyzer::new(
                sample_rate,
                RADIO_VISUALIZER_BUCKETS_PER_SECOND,
            ),
        }
    }

    fn current_gains(&mut self) -> LiveOrbitGains {
        if self.frame_index >= self.cached_gains_until_frame {
            self.cached_gains = live_orbit_gains(self.settings, self.frame_index, self.sample_rate);
            self.cached_gains_until_frame = self
                .frame_index
                .saturating_add(LIVE_ORBIT_GAIN_UPDATE_FRAMES);
        }
        self.cached_gains
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

    fn record_visualizer_sample(&mut self, mono: f32) {
        let Some(level) = self.visualizer_analyzer.push_sample(mono) else {
            return;
        };
        let now = Instant::now();

        if let Ok(mut state) = self.visualizer.lock() {
            state.peaks.push_back(RadioVisualizerBucket {
                at: now,
                level: level.clamp(0.0, 1.0),
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
    }

    fn process_frame(&mut self, stereo: [f32; 2], mono: f32) -> [f32; 2] {
        self.record_visualizer_sample(mono);
        let gains = self.current_gains();
        if !self.settings.orbit_enabled {
            return [
                soft_limit_radio(stereo[0] * gains.output_level),
                soft_limit_radio(stereo[1] * gains.output_level),
            ];
        }

        [
            soft_limit_radio(mono * gains.left * gains.output_level),
            soft_limit_radio(mono * gains.right * gains.output_level),
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
    radio_recorder: RadioRecordingHandle,
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
            radio_recorder: Arc::new(Mutex::new(None)),
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

    pub fn play_radio_stream_with_crossfade(
        &mut self,
        url: &str,
        settings: DspSettings,
        crossfade_seconds: f32,
    ) -> Result<()> {
        let response = reqwest::blocking::Client::builder()
            .user_agent("Audio-Orbit-Radio")
            .build()?
            .get(url)
            .send()
            .with_context(|| format!("failed to open internet radio stream: {url}"))?
            .error_for_status()
            .with_context(|| format!("internet radio stream returned an error: {url}"))?;
        let stream = RadioStream::new(response, Arc::clone(&self.radio_recorder));
        let decoder = Decoder::new(BufReader::new(stream))
            .with_context(|| format!("failed to decode internet radio stream: {url}"))?;

        let fade_seconds = crossfade_seconds.max(0.0);
        let keep_visualizer_history = self.current_radio_url.as_deref() == Some(url);
        if fade_seconds > 0.05 {
            let _ = self.stop_radio_recording();
            if let Some(old_sink) = self.sink.take() {
                fade_out_and_stop(old_sink, fade_seconds, self.volume_gain());
            }
        } else {
            self.stop();
        }

        if !keep_visualizer_history {
            self.radio_visualizer = Arc::new(Mutex::new(RadioVisualizerState::default()));
        }
        let visualizer = Arc::clone(&self.radio_visualizer);
        let radio_source =
            LiveRadioSource::new(decoder.convert_samples::<f32>(), settings, visualizer);
        let sink =
            Sink::try_new(&self.stream_handle).context("failed to create audio playback sink")?;
        sink.set_volume(self.volume_gain());
        if fade_seconds > 0.05 {
            sink.append(FadeInSource::new(radio_source, fade_seconds));
        } else {
            sink.append(radio_source);
        }
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

    pub fn is_radio_recording(&self) -> bool {
        self.radio_recorder
            .lock()
            .ok()
            .and_then(|recording| recording.as_ref().map(|_| ()))
            .is_some()
    }

    pub fn radio_recording_info(&self) -> Option<RadioRecordingInfo> {
        self.radio_recorder.lock().ok().and_then(|recording| {
            recording.as_ref().map(|recording| RadioRecordingInfo {
                path: recording.path.clone(),
                started_at: recording.started_at,
                bytes_written: recording.bytes_written,
            })
        })
    }

    pub fn start_radio_recording(
        &mut self,
        output_folder: &Path,
        _station_name: &str,
        _stream_title: Option<&str>,
    ) -> Result<PathBuf> {
        if self.current_radio_url.is_none() {
            anyhow::bail!("start an internet radio station before recording");
        }
        if self.is_radio_recording() {
            if let Some(info) = self.radio_recording_info() {
                return Ok(info.path);
            }
        }

        fs::create_dir_all(output_folder).with_context(|| {
            format!(
                "failed to create recording folder: {}",
                output_folder.display()
            )
        })?;
        let path = unique_recording_path(output_folder, "audio-orbit-records-recording", "part");
        let file = File::create(&path)
            .with_context(|| format!("failed to create recording file: {}", path.display()))?;

        let mut recorder = self
            .radio_recorder
            .lock()
            .map_err(|_| anyhow::anyhow!("radio recorder lock poisoned"))?;
        *recorder = Some(ActiveRadioRecording {
            file,
            path: path.clone(),
            started_at: Instant::now(),
            bytes_written: 0,
        });
        Ok(path)
    }

    pub fn stop_radio_recording(&mut self) -> Result<Option<RadioRecordingInfo>> {
        let mut recorder = self
            .radio_recorder
            .lock()
            .map_err(|_| anyhow::anyhow!("radio recorder lock poisoned"))?;
        let Some(mut recording) = recorder.take() else {
            return Ok(None);
        };
        recording.file.flush()?;
        drop(recording.file);

        let output_folder = recording
            .path
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("."));
        let final_path = unique_recording_path(&output_folder, &recording_stop_stem(), "mp3");
        fs::rename(&recording.path, &final_path).with_context(|| {
            format!(
                "failed to finalize recording from {} to {}",
                recording.path.display(),
                final_path.display()
            )
        })?;

        Ok(Some(RadioRecordingInfo {
            path: final_path,
            started_at: recording.started_at,
            bytes_written: recording.bytes_written,
        }))
    }

    pub fn radio_visualizer_frame(
        &self,
        requested_points: usize,
        visible_seconds: f32,
    ) -> RadioVisualizerFrame {
        let Ok(mut state) = self.radio_visualizer.lock() else {
            return RadioVisualizerFrame::default();
        };
        if requested_points == 0 || state.peaks.is_empty() {
            return RadioVisualizerFrame::default();
        }

        let now = Instant::now();
        let requested_points = requested_points.clamp(1, RADIO_VISUALIZER_MAX_BUCKETS);
        let visible_seconds = visible_seconds.clamp(1.0, RADIO_VISUALIZER_HISTORY_SECONDS as f32);
        let bucket_seconds = (visible_seconds / requested_points as f32).max(1.0 / 240.0);
        let max_age = visible_seconds + bucket_seconds * 2.0;

        while state
            .peaks
            .front()
            .map(|bucket| now.duration_since(bucket.at).as_secs_f32() > max_age)
            .unwrap_or(false)
            || state.peaks.len() > RADIO_VISUALIZER_MAX_BUCKETS
        {
            state.peaks.pop_front();
        }

        let mut slot_peaks = vec![0.0_f32; requested_points];
        let mut slot_sums = vec![0.0_f32; requested_points];
        let mut slot_counts = vec![0_u16; requested_points];
        let mut has_audio = false;
        for bucket in &state.peaks {
            let age_seconds = now.duration_since(bucket.at).as_secs_f32();
            if age_seconds > max_age {
                continue;
            }
            let slot_from_right = (age_seconds / bucket_seconds).floor() as usize;
            if slot_from_right >= requested_points {
                continue;
            }
            let slot = requested_points - 1 - slot_from_right;
            let level = bucket.level.clamp(0.0, 1.0);
            if level > 0.0005 {
                has_audio = true;
            }
            slot_peaks[slot] = slot_peaks[slot].max(level);
            slot_sums[slot] += level;
            slot_counts[slot] = slot_counts[slot].saturating_add(1);
        }

        let mut slot_levels = slot_peaks
            .into_iter()
            .zip(slot_sums)
            .zip(slot_counts)
            .map(|((peak, sum), count)| {
                if count == 0 {
                    0.0
                } else {
                    let mean = sum / count as f32;
                    // AIMP-like overview: keep transients, but do not let a single
                    // hot live-radio bucket make the whole small strip look solid.
                    (mean * 0.68 + peak * 0.32).clamp(0.0, 1.0)
                }
            })
            .collect::<Vec<_>>();

        if !has_audio {
            return RadioVisualizerFrame {
                bars: slot_levels
                    .into_iter()
                    .map(|_| RadioVisualizerBar { peak: 0.0 })
                    .collect(),
            };
        }

        fill_radio_waveform_gaps(&mut slot_levels);

        let mut nonzero = slot_levels
            .iter()
            .copied()
            .filter(|value| *value > 0.0005)
            .collect::<Vec<_>>();
        nonzero.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let last = nonzero.len().saturating_sub(1);
        let target_floor = nonzero[((last as f32 * 0.06) as usize).min(last)].min(0.22);
        let target_peak =
            nonzero[((last as f32 * 0.94) as usize).min(last)].max(target_floor + 0.18);

        // Slow range tracking gives radio a full-track-like overview feel without
        // making every UI frame rescale the entire strip.
        let floor_blend = if target_floor > state.display_floor {
            0.010
        } else {
            0.040
        };
        state.display_floor = (state.display_floor * (1.0 - floor_blend)
            + target_floor * floor_blend)
            .clamp(0.0, 0.24);

        let peak_blend = if target_peak > state.display_peak {
            0.040
        } else {
            0.010
        };
        state.display_peak = (state.display_peak * (1.0 - peak_blend) + target_peak * peak_blend)
            .max(state.display_floor + 0.16)
            .clamp(0.22, 1.0);

        let display_floor = state.display_floor;
        let display_range = (state.display_peak - display_floor).max(0.12);
        let bars = slot_levels
            .into_iter()
            .map(|level| {
                let normalized = ((level.clamp(0.0, 1.0) - display_floor) / display_range)
                    .clamp(0.0, 1.0)
                    .powf(1.08);
                RadioVisualizerBar {
                    peak: normalized.clamp(0.0, 1.0),
                }
            })
            .collect();

        RadioVisualizerFrame { bars }
    }

    pub fn play_file_streaming_with_cached_waveform_and_crossfade(
        &mut self,
        path: &Path,
        settings: DspSettings,
        start_seconds: f32,
        cached_waveform: Option<(Vec<f32>, Vec<f32>)>,
        cached_silence_ranges: Option<Vec<(f32, f32)>>,
        known_duration_seconds: Option<f32>,
        crossfade_seconds: f32,
    ) -> Result<PlaybackInfo> {
        // This streaming path intentionally starts immediately even when the selected
        // settings include silence skipping. Silence skipping needs a full render pass,
        // so callers can use this as the sub-second audible path and prepare the
        // silence-skipped render in the background.
        let start_seconds = if start_seconds.is_finite() {
            start_seconds.max(0.0)
        } else {
            0.0
        };
        let start_seconds =
            silence_adjusted_position(start_seconds, cached_silence_ranges.as_deref());
        let file = File::open(path)
            .with_context(|| format!("failed to open audio file: {}", path.display()))?;
        let mut decoder = Decoder::new(BufReader::new(file))
            .with_context(|| format!("failed to decode audio file: {}", path.display()))?;

        let input_channels = decoder.channels();
        let sample_rate = decoder.sample_rate();
        if sample_rate == 0 {
            anyhow::bail!("the selected audio file reported an invalid sample rate");
        }

        // Do not call `decoder.total_duration()` on the fast streaming path. Some
        // formats compute it by scanning metadata/frames, which makes long tracks
        // feel slow exactly when the user is trying to start or seek immediately.
        // Prefer the library metadata cache; if it is missing, start playback first
        // and let background/library metadata fill the duration later.
        let total_duration = known_duration_seconds
            .filter(|seconds| seconds.is_finite() && *seconds > 0.0)
            .map(Duration::from_secs_f32);
        let (waveform, waveform_brightness) = cached_waveform.unwrap_or_default();
        let seek_to = Duration::from_secs_f32(start_seconds);
        let crossfade_seconds = crossfade_seconds.max(0.0);

        // Use the decoder's real random-access seek when available. The previous
        // implementation used `skip_duration`, which had to decode and discard audio
        // until the target timestamp and caused a visible pause after dragging the
        // waveform playhead.
        if decoder.try_seek(seek_to).is_ok() {
            return self.start_file_streaming_source(
                path,
                settings,
                start_seconds,
                decoder.convert_samples::<f32>(),
                total_duration,
                input_channels,
                sample_rate,
                waveform,
                waveform_brightness,
                cached_silence_ranges.clone(),
                crossfade_seconds,
            );
        }

        self.start_file_streaming_source(
            path,
            settings,
            start_seconds,
            decoder.convert_samples::<f32>().skip_duration(seek_to),
            total_duration,
            input_channels,
            sample_rate,
            waveform,
            waveform_brightness,
            cached_silence_ranges,
            crossfade_seconds,
        )
    }

    fn start_file_streaming_source<S>(
        &mut self,
        path: &Path,
        settings: DspSettings,
        start_seconds: f32,
        source: S,
        total_duration: Option<Duration>,
        input_channels: u16,
        sample_rate: u32,
        waveform: Vec<f32>,
        waveform_brightness: Vec<f32>,
        cached_silence_ranges: Option<Vec<(f32, f32)>>,
        crossfade_seconds: f32,
    ) -> Result<PlaybackInfo>
    where
        S: Source<Item = f32> + Send + 'static,
    {
        let remaining_duration = total_duration
            .map(|duration| duration.saturating_sub(Duration::from_secs_f32(start_seconds)))
            .unwrap_or(Duration::ZERO);
        let source = LiveFileSource::new(
            source,
            settings,
            start_seconds,
            cached_silence_ranges.clone(),
        );
        let fade_seconds = crossfade_seconds.max(0.0);

        if fade_seconds > 0.05 {
            let _ = self.stop_radio_recording();
            if let Some(old_sink) = self.sink.take() {
                fade_out_and_stop(old_sink, fade_seconds, self.volume_gain());
            }
        } else {
            self.stop();
        }

        let sink =
            Sink::try_new(&self.stream_handle).context("failed to create audio playback sink")?;
        sink.set_volume(self.volume_gain());
        if fade_seconds > 0.05 {
            sink.append(FadeInSource::new(source, fade_seconds));
        } else {
            sink.append(source);
        }
        sink.play();

        self.sink = Some(sink);
        self.started_at = Some(Instant::now());
        self.paused_at = None;
        self.accumulated_pause = Duration::ZERO;
        self.current_duration = Some(remaining_duration);
        self.current_start_offset_seconds = start_seconds;
        self.current_path = Some(path.to_path_buf());
        self.current_settings = Some(settings);
        self.current_radio_url = None;

        let original_duration_seconds = total_duration
            .map(|duration| duration.as_secs_f32())
            .unwrap_or(0.0);
        Ok(PlaybackInfo {
            path: path.to_path_buf(),
            original_duration_seconds,
            input_channels,
            sample_rate,
            size_bytes: fs::metadata(path).ok().map(|metadata| metadata.len()),
            waveform,
            waveform_brightness,
            silence_ranges: Vec::new(),
        })
    }

    pub fn prepare_file_with_cached_analysis(
        path: PathBuf,
        settings: DspSettings,
        start_seconds: f32,
        cached_waveform: Option<(Vec<f32>, Vec<f32>)>,
        cached_silence_ranges: Option<Vec<(f32, f32)>>,
    ) -> Result<PreparedPlayback> {
        let (processed_samples, render_info, sample_rate) = render_file_data(
            &path,
            settings,
            start_seconds,
            cached_waveform,
            cached_silence_ranges,
        )?;
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
        let rendered_duration =
            Duration::from_secs_f32(render_info.rendered_duration_seconds.max(0.0));
        self.play_processed_samples(
            processed_samples,
            sample_rate,
            rendered_duration,
            &path,
            settings,
            start_seconds,
        )?;

        Ok(playback_info(&path, render_info))
    }

    pub fn play_prepared_from_live_position(
        &mut self,
        mut prepared: PreparedPlayback,
        render_elapsed_seconds: f32,
    ) -> Result<PlaybackInfo> {
        let compensated_start_seconds = prepared.start_seconds + render_elapsed_seconds.max(0.0);

        if render_elapsed_seconds > 0.025 {
            let trim_frames = (render_elapsed_seconds * prepared.sample_rate as f32)
                .round()
                .max(0.0) as usize;
            let trim_samples = (trim_frames * 2).min(prepared.processed_samples.len());
            if trim_samples > 0 && trim_samples < prepared.processed_samples.len() {
                prepared.processed_samples.drain(0..trim_samples);
                prepared.render_info.rendered_duration_seconds =
                    (prepared.render_info.rendered_duration_seconds - render_elapsed_seconds)
                        .max(0.0);
            }
        }

        self.stop();
        let rendered_duration =
            Duration::from_secs_f32(prepared.render_info.rendered_duration_seconds.max(0.0));
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

    pub fn crossfade_to_prepared(
        &mut self,
        mut prepared: PreparedPlayback,
        crossfade_seconds: f32,
    ) -> Result<PlaybackInfo> {
        let fade_seconds = crossfade_seconds
            .max(0.0)
            .min(prepared.render_info.rendered_duration_seconds.max(0.0));

        apply_fade_in(
            &mut prepared.processed_samples,
            prepared.sample_rate,
            fade_seconds,
        );

        if fade_seconds > 0.05 {
            let _ = self.stop_radio_recording();
            if let Some(old_sink) = self.sink.take() {
                fade_out_and_stop(old_sink, fade_seconds, self.volume_gain());
            }
        } else {
            self.stop();
        }

        let rendered_duration =
            Duration::from_secs_f32(prepared.render_info.rendered_duration_seconds.max(0.0));
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

    pub fn stop(&mut self) {
        let _ = self.stop_radio_recording();
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
            Some(duration) => {
                position.min(self.current_start_offset_seconds + duration.as_secs_f32())
            }
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

    pub fn current_settings(&self) -> Option<DspSettings> {
        self.current_settings
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
        let sink =
            Sink::try_new(&self.stream_handle).context("failed to create audio playback sink")?;
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

fn recording_stop_stem() -> String {
    let seconds = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0);
    let (year, month, day, hour, minute, second) = utc_timestamp_parts(seconds);
    format!("audio-orbit-records-{year:04}-{month:02}-{day:02}-{hour:02}-{minute:02}-{second:02}")
}

fn unique_recording_path(folder: &Path, stem: &str, extension: &str) -> PathBuf {
    let mut path = folder.join(format!("{stem}.{extension}"));
    let mut suffix = 2usize;
    while path.exists() {
        path = folder.join(format!("{stem}-{suffix}.{extension}"));
        suffix = suffix.saturating_add(1);
    }
    path
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
    cached_waveform: Option<(Vec<f32>, Vec<f32>)>,
    cached_silence_ranges: Option<Vec<(f32, f32)>>,
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

    let (processed_samples, render_info) = render_orbit_to_stereo_with_cached_analysis(
        &input_samples,
        input_channels,
        sample_rate,
        settings,
        start_seconds,
        cached_waveform,
        cached_silence_ranges,
    );

    if processed_samples.is_empty() {
        anyhow::bail!("the rendered audio was empty after processing; disable silence skip or seek earlier in the track");
    }

    Ok((processed_samples, render_info, sample_rate))
}

fn playback_info(path: &Path, render_info: RenderInfo) -> PlaybackInfo {
    PlaybackInfo {
        path: path.to_path_buf(),
        original_duration_seconds: render_info.original_duration_seconds,
        input_channels: render_info.input_channels,
        sample_rate: render_info.sample_rate,
        size_bytes: fs::metadata(path).ok().map(|metadata| metadata.len()),
        waveform: render_info.waveform,
        waveform_brightness: render_info.waveform_brightness,
        silence_ranges: render_info.silence_ranges,
    }
}
