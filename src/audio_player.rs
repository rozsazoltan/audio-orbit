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
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

const RADIO_VISUALIZER_HISTORY_SECONDS: usize = 20;
const RADIO_VISUALIZER_VISIBLE_SECONDS: f32 = 15.0;
const RADIO_VISUALIZER_BUCKETS_PER_SECOND: usize = 18;
const RADIO_VISUALIZER_MAX_BUCKETS: usize = RADIO_VISUALIZER_HISTORY_SECONDS * RADIO_VISUALIZER_BUCKETS_PER_SECOND;
const MAX_DECODED_SOURCE_SAMPLES: usize = 48_000_000;
const MAX_RENDERED_STEREO_SAMPLES: usize = 48_000_000;
const DECODE_RESERVE_CHUNK: usize = 262_144;

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

#[derive(Clone)]
pub struct RadioRecorderHandle(RadioRecordingHandle);

pub struct PreparedRadioPlayback {
    url: String,
    settings: DspSettings,
    decoder: Decoder<BufReader<RadioStream<reqwest::blocking::Response>>>,
}

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
}

impl<S: Source<Item = f32>> LiveRadioSource<S> {
    fn new(
        inner: S,
        settings: DspSettings,
        visualizer: RadioVisualizerHandle,
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
        let Some(bucket) = self.visualizer_analyzer.push_sample(mono) else {
            return;
        };
        let now = Instant::now();

        if let Ok(mut state) = self.visualizer.lock() {
            state.peaks.push_back(RadioVisualizerBucket {
                at: now,
                peak: bucket.level.clamp(0.0, 1.0),
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


    pub fn radio_recorder_handle(&self) -> RadioRecorderHandle {
        RadioRecorderHandle(Arc::clone(&self.radio_recorder))
    }

    pub fn prepare_radio_stream(
        url: String,
        settings: DspSettings,
        recorder: RadioRecorderHandle,
    ) -> Result<PreparedRadioPlayback> {
        let response = reqwest::blocking::Client::builder()
            .user_agent("Audio-Orbit-Radio")
            .connect_timeout(Duration::from_secs(6))
            .build()?
            .get(&url)
            .send()
            .with_context(|| format!("failed to open internet radio stream: {url}"))?
            .error_for_status()
            .with_context(|| format!("internet radio stream returned an error: {url}"))?;
        let stream = RadioStream::new(response, recorder.0);
        let decoder = Decoder::new(BufReader::new(stream))
            .with_context(|| format!("failed to decode internet radio stream: {url}"))?;

        Ok(PreparedRadioPlayback {
            url,
            settings,
            decoder,
        })
    }

    pub fn play_prepared_radio_stream(&mut self, prepared: PreparedRadioPlayback) -> Result<()> {
        let PreparedRadioPlayback { url, settings, decoder } = prepared;
        let keep_visualizer_history = self.current_radio_url.as_deref() == Some(url.as_str());
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
        self.current_radio_url = Some(url);

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

        let mut previous_peak = 0.0_f32;
        let bars = slot_peaks
            .into_iter()
            .enumerate()
            .filter_map(|(slot, peak)| {
                let shaped = if peak > previous_peak {
                    previous_peak * 0.22 + peak * 0.78
                } else {
                    previous_peak * 0.70 + peak * 0.30
                };
                previous_peak = shaped;
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

    pub fn prepare_file(path: PathBuf, settings: DspSettings, start_seconds: f32) -> Result<PreparedPlayback> {
        Self::prepare_file_with_cancel(path, settings, start_seconds, None)
    }

    pub fn prepare_file_with_cancel(
        path: PathBuf,
        settings: DspSettings,
        start_seconds: f32,
        cancel: Option<Arc<AtomicBool>>,
    ) -> Result<PreparedPlayback> {
        let cancel_ref = cancel.as_deref();
        let (processed_samples, render_info, sample_rate) = render_file_data(&path, settings, start_seconds, cancel_ref)?;
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

fn render_file_data(
    path: &Path,
    settings: DspSettings,
    start_seconds: f32,
    cancel: Option<&AtomicBool>,
) -> Result<(Vec<f32>, RenderInfo, u32)> {
    let file = File::open(path)
        .with_context(|| format!("failed to open audio file: {}", path.display()))?;
    let decoder = Decoder::new(BufReader::new(file))
        .with_context(|| format!("failed to decode audio file: {}", path.display()))?;

    let input_channels = decoder.channels().max(1);
    let sample_rate = decoder.sample_rate();
    if sample_rate == 0 {
        anyhow::bail!("the selected audio file reported an invalid sample rate");
    }

    let input_samples = decode_samples_with_memory_guard(decoder, path, cancel)?;
    if input_samples.is_empty() {
        anyhow::bail!("the selected audio file did not contain any decoded samples");
    }

    let channels = input_channels.max(1) as usize;
    let frame_count = input_samples.len() / channels;
    let start_frame = ((start_seconds.max(0.0) * sample_rate as f32) as usize).min(frame_count);
    let requested_rendered_samples = frame_count.saturating_sub(start_frame).saturating_mul(2);
    if requested_rendered_samples > MAX_RENDERED_STEREO_SAMPLES {
        anyhow::bail!(
            "the selected audio range is too large for the current in-memory renderer ({} stereo samples requested, limit is {}). Split the file or use a shorter seek range until the streaming engine lands.",
            requested_rendered_samples,
            MAX_RENDERED_STEREO_SAMPLES
        );
    }

    if is_cancelled(cancel) {
        anyhow::bail!("playback preparation was cancelled");
    }

    let (processed_samples, render_info) =
        render_orbit_to_stereo(&input_samples, input_channels, sample_rate, settings, start_seconds, cancel)?;

    if processed_samples.is_empty() {
        anyhow::bail!("the rendered audio was empty after processing; disable silence skip or seek earlier in the track");
    }

    Ok((processed_samples, render_info, sample_rate))
}

fn is_cancelled(cancel: Option<&AtomicBool>) -> bool {
    cancel
        .map(|flag| flag.load(Ordering::Relaxed))
        .unwrap_or(false)
}

fn decode_samples_with_memory_guard(
    decoder: Decoder<BufReader<File>>,
    path: &Path,
    cancel: Option<&AtomicBool>,
) -> Result<Vec<f32>> {
    let mut samples = Vec::new();

    for sample in decoder.convert_samples::<f32>() {
        if (samples.len() & 16_383) == 0 && is_cancelled(cancel) {
            anyhow::bail!("playback preparation was cancelled");
        }

        if samples.len() >= MAX_DECODED_SOURCE_SAMPLES {
            anyhow::bail!(
                "the selected audio file is too large for the current in-memory decoder ({} decoded samples limit): {}",
                MAX_DECODED_SOURCE_SAMPLES,
                path.display()
            );
        }

        if samples.len() == samples.capacity() {
            let remaining = MAX_DECODED_SOURCE_SAMPLES.saturating_sub(samples.len());
            let reserve = remaining.min(DECODE_RESERVE_CHUNK).max(1);
            samples.try_reserve(reserve).map_err(|_| {
                anyhow::anyhow!(
                    "not enough memory to decode audio safely without risking an allocator abort: {}",
                    path.display()
                )
            })?;
        }

        samples.push(sample.clamp(-1.0, 1.0));
    }

    Ok(samples)
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
        waveform_brightness: render_info.waveform_brightness,
        silence_ranges: render_info.silence_ranges,
    }
}
