use crate::{
    dsp::{render_orbit_to_stereo, DspSettings, RenderInfo},
    spectrum_waveform::LiveSpectrumAnalyzer,
};
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

const RADIO_VISUALIZER_HISTORY_SECONDS: usize = 20;
const RADIO_VISUALIZER_VISIBLE_SECONDS: f32 = 15.0;
const RADIO_VISUALIZER_BUCKETS_PER_SECOND: usize = 24;
const RADIO_VISUALIZER_MAX_BUCKETS: usize = RADIO_VISUALIZER_HISTORY_SECONDS * RADIO_VISUALIZER_BUCKETS_PER_SECOND;
const RADIO_RECOGNITION_BUFFER_SECONDS: usize = 24;

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

        let mut file = File::create(path)
            .with_context(|| format!("failed to create recognition sample: {}", path.display()))?;
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

pub struct PreparedPlayback {
    path: PathBuf,
    settings: DspSettings,
    start_seconds: f32,
    processed_samples: Vec<f32>,
    render_info: RenderInfo,
    sample_rate: u32,
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
                        recording.bytes_written = recording.bytes_written.saturating_add(read as u64);
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
    peak: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct RadioVisualizerBar {
    pub age_seconds: f32,
    pub peak: f32,
}

#[derive(Clone, Debug, Default)]
pub struct RadioVisualizerFrame {
    pub bars: Vec<RadioVisualizerBar>,
    pub bucket_seconds: f32,
}

struct RadioVisualizerState {
    peaks: VecDeque<RadioVisualizerBucket>,
}

impl Default for RadioVisualizerState {
    fn default() -> Self {
        Self {
            peaks: VecDeque::new(),
        }
    }
}

type RadioVisualizerHandle = Arc<Mutex<RadioVisualizerState>>;

#[derive(Default)]
struct RecognitionSampleBuffer {
    sample_rate: u32,
    samples: VecDeque<f32>,
}

type RecognitionSampleHandle = Arc<Mutex<RecognitionSampleBuffer>>;

impl RecognitionSampleBuffer {
    fn push_chunk(&mut self, sample_rate: u32, samples: &[f32]) {
        if sample_rate == 0 || samples.is_empty() {
            return;
        }
        if self.sample_rate != sample_rate {
            self.sample_rate = sample_rate;
            self.samples.clear();
        }
        self.samples.extend(samples.iter().copied().map(|sample| sample.clamp(-1.0, 1.0)));
        let max_samples = sample_rate as usize * RADIO_RECOGNITION_BUFFER_SECONDS;
        while self.samples.len() > max_samples {
            self.samples.pop_front();
        }
    }

    fn snapshot(&self, seconds: f32) -> Option<RecognitionAudioSample> {
        if self.sample_rate == 0 || self.samples.is_empty() {
            return None;
        }
        let take = (seconds.max(1.0) * self.sample_rate as f32).round() as usize;
        let start = self.samples.len().saturating_sub(take);
        let samples = self.samples.iter().skip(start).copied().collect::<Vec<_>>();
        if samples.is_empty() {
            None
        } else {
            Some(RecognitionAudioSample {
                sample_rate: self.sample_rate,
                channels: 1,
                samples,
            })
        }
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
    visualizer: RadioVisualizerHandle,
    visualizer_analyzer: LiveSpectrumAnalyzer,
    recognition: RecognitionSampleHandle,
    recognition_chunk: Vec<f32>,
}

impl<S: Source<Item = f32>> LiveRadioSource<S> {
    fn new(
        inner: S,
        settings: DspSettings,
        visualizer: RadioVisualizerHandle,
        recognition: RecognitionSampleHandle,
    ) -> Self {
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
            visualizer_analyzer: LiveSpectrumAnalyzer::new(sample_rate, RADIO_VISUALIZER_BUCKETS_PER_SECOND),
            recognition,
            recognition_chunk: Vec::with_capacity((sample_rate as usize / 4).max(256)),
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

    fn record_visualizer_sample(&mut self, mono: f32) {
        let Some(level) = self.visualizer_analyzer.push_sample(mono) else {
            return;
        };
        let now = Instant::now();

        if let Ok(mut state) = self.visualizer.lock() {
            state.peaks.push_back(RadioVisualizerBucket {
                at: now,
                peak: level.clamp(0.0, 1.0),
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

    fn record_recognition_sample(&mut self, mono: f32) {
        self.recognition_chunk.push(mono.clamp(-1.0, 1.0));
        let flush_samples = (self.sample_rate.max(1) as usize / 5).max(1024);
        if self.recognition_chunk.len() < flush_samples {
            return;
        }

        if let Ok(mut buffer) = self.recognition.lock() {
            buffer.push_chunk(self.sample_rate, &self.recognition_chunk);
        }
        self.recognition_chunk.clear();
    }

    fn process_frame(&mut self, stereo: [f32; 2], mono: f32) -> [f32; 2] {
        self.record_visualizer_sample(mono);
        self.record_recognition_sample(mono);
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
    radio_recorder: RadioRecordingHandle,
    radio_recognition: RecognitionSampleHandle,
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
            radio_recognition: Arc::new(Mutex::new(RecognitionSampleBuffer::default())),
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
        let stream = RadioStream::new(response, Arc::clone(&self.radio_recorder));
        let decoder = Decoder::new(BufReader::new(stream))
            .with_context(|| format!("failed to decode internet radio stream: {url}"))?;

        let keep_visualizer_history = self.current_radio_url.as_deref() == Some(url);
        self.stop();
        if !keep_visualizer_history {
            self.radio_visualizer = Arc::new(Mutex::new(RadioVisualizerState::default()));
        }
        self.radio_recognition = Arc::new(Mutex::new(RecognitionSampleBuffer::default()));
        let visualizer = Arc::clone(&self.radio_visualizer);
        let recognition = Arc::clone(&self.radio_recognition);
        let radio_source = LiveRadioSource::new(decoder.convert_samples::<f32>(), settings, visualizer, recognition);
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

        fs::create_dir_all(output_folder)
            .with_context(|| format!("failed to create recording folder: {}", output_folder.display()))?;
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

    pub fn radio_visualizer_frame(&self, requested_points: usize) -> RadioVisualizerFrame {
        let Ok(mut state) = self.radio_visualizer.lock() else {
            return RadioVisualizerFrame::default();
        };
        if requested_points == 0 || state.peaks.is_empty() {
            return RadioVisualizerFrame::default();
        }

        let now = Instant::now();
        let visible_seconds = RADIO_VISUALIZER_VISIBLE_SECONDS;
        let bucket_seconds = visible_seconds / requested_points.max(1) as f32;
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
            slot_peaks[slot] = slot_peaks[slot].max(bucket.peak);
        }

        let mut previous = 0.0_f32;
        let bars = slot_peaks
            .into_iter()
            .enumerate()
            .filter_map(|(slot, peak)| {
                let shaped = if peak > previous {
                    previous * 0.25 + peak * 0.75
                } else {
                    previous * 0.68 + peak * 0.32
                };
                previous = shaped;
                if shaped <= 0.003 {
                    return None;
                }
                Some(RadioVisualizerBar {
                    age_seconds: (requested_points - 1 - slot) as f32 * bucket_seconds,
                    peak: shaped.clamp(0.0, 1.0),
                })
            })
            .collect();

        RadioVisualizerFrame {
            bars,
            bucket_seconds,
        }
    }

    pub fn radio_recognition_sample(&self, seconds: f32) -> Result<Option<RecognitionAudioSample>> {
        if self.current_radio_url.is_none() {
            return Ok(None);
        }

        let sample = self
            .radio_recognition
            .lock()
            .map_err(|_| anyhow::anyhow!("radio recognition buffer lock poisoned"))?
            .snapshot(seconds);

        if let Some(sample) = &sample {
            if sample.duration_seconds() < 3.0 {
                anyhow::bail!("wait a few seconds before identifying this internet radio stream");
            }
        }

        Ok(sample)
    }

    pub fn capture_file_recognition_sample(path: &Path, start_seconds: f32, seconds: f32) -> Result<RecognitionAudioSample> {
        capture_file_recognition_sample(path, start_seconds, seconds)
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
        self.radio_recognition = Arc::new(Mutex::new(RecognitionSampleBuffer::default()));
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

fn capture_file_recognition_sample(path: &Path, start_seconds: f32, seconds: f32) -> Result<RecognitionAudioSample> {
    let file = File::open(path)
        .with_context(|| format!("failed to open audio file for recognition: {}", path.display()))?;
    let decoder = Decoder::new(BufReader::new(file))
        .with_context(|| format!("failed to decode audio file for recognition: {}", path.display()))?;

    let channels = decoder.channels().max(1) as usize;
    let sample_rate = decoder.sample_rate().max(1);
    let input_samples: Vec<f32> = decoder.convert_samples::<f32>().collect();
    if input_samples.is_empty() {
        anyhow::bail!("the selected audio file did not contain any decoded samples");
    }

    let total_frames = input_samples.len() / channels;
    if total_frames == 0 {
        anyhow::bail!("the selected audio file did not contain any complete audio frames");
    }

    let wanted_frames = (seconds.max(3.0) * sample_rate as f32).round() as usize;
    let requested_start = (start_seconds.max(0.0) * sample_rate as f32).round() as usize;
    let start_frame = requested_start.min(total_frames.saturating_sub(1));
    let end_frame = (start_frame + wanted_frames).min(total_frames);
    let mut samples = Vec::with_capacity(end_frame.saturating_sub(start_frame));

    for frame in start_frame..end_frame {
        let frame_offset = frame * channels;
        let mut sum = 0.0_f32;
        let mut count = 0usize;
        for channel in 0..channels {
            if let Some(sample) = input_samples.get(frame_offset + channel) {
                sum += *sample;
                count += 1;
            }
        }
        if count > 0 {
            samples.push((sum / count as f32).clamp(-1.0, 1.0));
        }
    }

    if samples.len() < sample_rate as usize {
        anyhow::bail!("recognition sample is too short");
    }

    Ok(RecognitionAudioSample {
        sample_rate,
        channels: 1,
        samples,
    })
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
