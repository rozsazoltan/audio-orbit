use crate::{app_data_dir, DjMixEvent, DjMixOptions, DjMixTrack};
use anyhow::{anyhow, Context, Result};
use rodio::{source::UniformSourceIterator, Decoder, Source};
use serde::{Deserialize, Serialize};
use shine_rs::{Mp3Encoder, Mp3EncoderConfig, StereoMode};
use std::{
    cmp::Ordering,
    collections::{BTreeMap, VecDeque},
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc::Sender,
        Arc,
    },
    time::UNIX_EPOCH,
};

const OUTPUT_SAMPLE_RATE: u32 = 44_100;
const OUTPUT_CHANNELS: u16 = 2;
const ANALYSIS_RATE_HZ: usize = 100;
const ANALYSIS_SECONDS: usize = 120;
const MIN_BPM: f32 = 70.0;
const MAX_BPM: f32 = 180.0;
const DEFAULT_BPM: f32 = 120.0;
const MAX_TEMPO_CHANGE: f32 = 0.04;
const MAX_TRANSITION_SECONDS: f32 = 28.0;
const MAX_INTRO_SKIP_SECONDS: f32 = 10.0;
const TARGET_RMS: f32 = 0.16;

#[derive(Clone, Debug)]
pub(crate) struct ExportRequest {
    pub tracks: Vec<DjMixTrack>,
    pub options: DjMixOptions,
    pub output_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TrackAnalysis {
    bpm: f32,
    beat_offset_seconds: f32,
    leading_silence_seconds: f32,
    rms: f32,
    duration_seconds: Option<f32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedTrackAnalysis {
    file_len: u64,
    modified_nanos: u128,
    analysis: TrackAnalysis,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct AnalysisCache {
    tracks: BTreeMap<String, CachedTrackAnalysis>,
}

#[derive(Clone, Debug)]
struct PlannedTrack {
    track: DjMixTrack,
    analysis: TrackAnalysis,
    speed_ratio: f32,
    gain: f32,
}

struct StereoTrackStream {
    source: UniformSourceIterator<Decoder<BufReader<File>>, f32>,
    previous: Option<[f32; 2]>,
    next: Option<[f32; 2]>,
    source_position: f64,
    speed_ratio: f64,
}

impl StereoTrackStream {
    fn open(path: &Path, speed_ratio: f32) -> Result<Self> {
        let file = File::open(path)
            .with_context(|| format!("Failed to open audio file: {}", path.display()))?;
        let decoder = Decoder::new(BufReader::new(file))
            .with_context(|| format!("Failed to decode audio file: {}", path.display()))?;
        let mut source =
            UniformSourceIterator::<_, f32>::new(decoder, OUTPUT_CHANNELS, OUTPUT_SAMPLE_RATE);
        let previous = read_stereo_frame(&mut source);
        let next = read_stereo_frame(&mut source);

        Ok(Self {
            source,
            previous,
            next,
            source_position: 0.0,
            speed_ratio: speed_ratio.clamp(0.5, 2.0) as f64,
        })
    }

    fn next_frame(&mut self) -> Option<[f32; 2]> {
        let previous = self.previous?;
        let next = self.next.unwrap_or(previous);
        let fraction = self.source_position.fract() as f32;
        let frame = [
            previous[0] + (next[0] - previous[0]) * fraction,
            previous[1] + (next[1] - previous[1]) * fraction,
        ];

        self.source_position += self.speed_ratio;
        while self.source_position >= 1.0 {
            self.source_position -= 1.0;
            self.previous = self.next;
            self.next = read_stereo_frame(&mut self.source);
            if self.previous.is_none() {
                break;
            }
        }

        Some(frame)
    }

    fn skip_seconds(&mut self, seconds: f32) {
        let frames = (seconds.max(0.0) * OUTPUT_SAMPLE_RATE as f32) as usize;
        for _ in 0..frames {
            if self.next_frame().is_none() {
                break;
            }
        }
    }
}

struct Mp3StreamWriter {
    file: BufWriter<File>,
    encoder: Mp3Encoder,
    pcm: Vec<i16>,
    flush_samples: usize,
}

impl Mp3StreamWriter {
    fn create(path: &Path, bitrate_kbps: u32) -> Result<Self> {
        let file = File::create(path)
            .with_context(|| format!("Failed to create MP3 file: {}", path.display()))?;
        let encoder = Mp3Encoder::new(
            Mp3EncoderConfig::new()
                .sample_rate(OUTPUT_SAMPLE_RATE)
                .bitrate(bitrate_kbps)
                .channels(OUTPUT_CHANNELS as u8)
                .stereo_mode(StereoMode::JointStereo),
        )
        .map_err(|error| anyhow!("Failed to initialize MP3 encoder: {error}"))?;
        let flush_samples = encoder.samples_per_frame() * 16;

        Ok(Self {
            file: BufWriter::new(file),
            encoder,
            pcm: Vec::with_capacity(flush_samples),
            flush_samples,
        })
    }

    fn write_frame(&mut self, frame: [f32; 2]) -> Result<()> {
        self.pcm.push(float_to_i16(frame[0]));
        self.pcm.push(float_to_i16(frame[1]));
        if self.pcm.len() >= self.flush_samples {
            self.flush_pcm()?;
        }
        Ok(())
    }

    fn flush_pcm(&mut self) -> Result<()> {
        if self.pcm.is_empty() {
            return Ok(());
        }
        let frames = self
            .encoder
            .encode_interleaved(&self.pcm)
            .map_err(|error| anyhow!("MP3 encoding failed: {error}"))?;
        self.pcm.clear();
        for frame in frames {
            self.file.write_all(&frame)?;
        }
        Ok(())
    }

    fn finish(mut self) -> Result<()> {
        self.flush_pcm()?;
        let tail = self
            .encoder
            .finish()
            .map_err(|error| anyhow!("Failed to finish MP3 encoding: {error}"))?;
        self.file.write_all(&tail)?;
        self.file.flush()?;
        Ok(())
    }
}

pub(crate) fn export_mix(
    request: ExportRequest,
    sender: Sender<DjMixEvent>,
    cancel: Arc<AtomicBool>,
) {
    lower_worker_priority();
    let output_path = ensure_mp3_extension(&request.output_path);
    let temporary_path = temporary_output_path(&output_path);
    let result = export_mix_inner(&request, &temporary_path, &sender, &cancel);

    match result {
        Ok(()) => {
            if cancel.load(AtomicOrdering::Relaxed) {
                let _ = fs::remove_file(&temporary_path);
                let _ = sender.send(DjMixEvent::Cancelled);
                return;
            }
            if output_path.exists() {
                let _ = fs::remove_file(&output_path);
            }
            match fs::rename(&temporary_path, &output_path) {
                Ok(()) => {
                    let _ = sender.send(DjMixEvent::Completed {
                        output_path,
                        track_count: request.tracks.len(),
                    });
                }
                Err(error) => {
                    let _ = fs::remove_file(&temporary_path);
                    let _ = sender.send(DjMixEvent::Failed(format!(
                        "Failed to finalize DJ mix: {error}"
                    )));
                }
            }
        }
        Err(error) => {
            let _ = fs::remove_file(&temporary_path);
            if cancel.load(AtomicOrdering::Relaxed) {
                let _ = sender.send(DjMixEvent::Cancelled);
            } else {
                let _ = sender.send(DjMixEvent::Failed(error.to_string()));
            }
        }
    }
}

fn export_mix_inner(
    request: &ExportRequest,
    temporary_path: &Path,
    sender: &Sender<DjMixEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    if request.tracks.len() < 2 {
        return Err(anyhow!("DJ mix requires at least two tracks."));
    }
    if let Some(parent) = temporary_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut cache = load_analysis_cache();
    let mut analyzed = Vec::with_capacity(request.tracks.len());
    for (index, track) in request.tracks.iter().enumerate() {
        ensure_not_cancelled(cancel)?;
        send_progress(
            sender,
            format!("Analyzing {}", track.title),
            index as f32 / (request.tracks.len() as f32 * 2.0),
        );
        let analysis = cached_or_analyze(track, &mut cache, cancel)?;
        analyzed.push((track.clone(), analysis));
    }
    save_analysis_cache(&cache);

    let planned = plan_tracks(analyzed, request.options);
    let mut writer = Mp3StreamWriter::create(temporary_path, request.options.bitrate_kbps)?;
    render_planned_mix(&planned, request.options, &mut writer, sender, cancel)?;
    ensure_not_cancelled(cancel)?;
    writer.finish()?;
    Ok(())
}

fn render_planned_mix(
    tracks: &[PlannedTrack],
    options: DjMixOptions,
    writer: &mut Mp3StreamWriter,
    sender: &Sender<DjMixEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    let mut current_stream = StereoTrackStream::open(&tracks[0].track.path, tracks[0].speed_ratio)?;
    let first_skip = aligned_intro_skip(&tracks[0]);
    current_stream.skip_seconds(first_skip);

    for pair_index in 0..tracks.len() - 1 {
        ensure_not_cancelled(cancel)?;
        let current = &tracks[pair_index];
        let next = &tracks[pair_index + 1];
        send_progress(
            sender,
            format!("Mixing {} → {}", current.track.title, next.track.title),
            0.5 + pair_index as f32 / ((tracks.len() - 1) as f32 * 2.0),
        );

        let transition_frames = transition_frame_count(current, next, options.transition_beats);
        let current_tail = write_body_keep_tail(
            &mut current_stream,
            transition_frames,
            current.gain,
            writer,
            cancel,
        )?;

        let mut next_stream = StereoTrackStream::open(&next.track.path, next.speed_ratio)?;
        next_stream.skip_seconds(aligned_intro_skip(next));
        let mut next_intro = Vec::with_capacity(current_tail.len());
        for _ in 0..current_tail.len() {
            if let Some(frame) = next_stream.next_frame() {
                next_intro.push(frame);
            } else {
                break;
            }
        }

        let overlap = current_tail.len().min(next_intro.len());
        if current_tail.len() > overlap {
            for frame in &current_tail[..current_tail.len() - overlap] {
                writer.write_frame(scale_frame(*frame, current.gain))?;
            }
        }
        let outgoing = &current_tail[current_tail.len().saturating_sub(overlap)..];
        let incoming = &next_intro[next_intro.len().saturating_sub(overlap)..];
        write_transition(
            outgoing,
            incoming,
            current.gain,
            next.gain,
            options.bass_swap,
            writer,
            cancel,
        )?;

        current_stream = next_stream;
    }

    let last = tracks.last().expect("validated non-empty DJ plan");
    while let Some(frame) = current_stream.next_frame() {
        ensure_not_cancelled(cancel)?;
        writer.write_frame(scale_frame(frame, last.gain))?;
    }
    send_progress(sender, "Finalizing MP3".to_owned(), 0.99);
    Ok(())
}

fn write_body_keep_tail(
    stream: &mut StereoTrackStream,
    keep_frames: usize,
    gain: f32,
    writer: &mut Mp3StreamWriter,
    cancel: &AtomicBool,
) -> Result<Vec<[f32; 2]>> {
    let mut tail = VecDeque::with_capacity(keep_frames.saturating_add(1));
    let mut frames_since_cancel_check = 0usize;
    while let Some(frame) = stream.next_frame() {
        tail.push_back(frame);
        if tail.len() > keep_frames {
            if let Some(output) = tail.pop_front() {
                writer.write_frame(scale_frame(output, gain))?;
            }
        }
        frames_since_cancel_check += 1;
        if frames_since_cancel_check >= OUTPUT_SAMPLE_RATE as usize / 2 {
            ensure_not_cancelled(cancel)?;
            frames_since_cancel_check = 0;
        }
    }
    Ok(tail.into_iter().collect())
}

fn write_transition(
    outgoing: &[[f32; 2]],
    incoming: &[[f32; 2]],
    outgoing_gain: f32,
    incoming_gain: f32,
    bass_swap: bool,
    writer: &mut Mp3StreamWriter,
    cancel: &AtomicBool,
) -> Result<()> {
    let frames = outgoing.len().min(incoming.len());
    if frames == 0 {
        return Ok(());
    }

    let mut outgoing_filter = LowPassStereo::new(180.0);
    let mut incoming_filter = LowPassStereo::new(180.0);
    for index in 0..frames {
        if index % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        let progress = if frames <= 1 {
            1.0
        } else {
            index as f32 / (frames - 1) as f32
        };
        let out_fade = ((1.0 - progress) * std::f32::consts::FRAC_PI_2).sin();
        let in_fade = (progress * std::f32::consts::FRAC_PI_2).sin();
        let mut out = scale_frame(outgoing[index], outgoing_gain);
        let mut input = scale_frame(incoming[index], incoming_gain);

        if bass_swap {
            let out_low = outgoing_filter.process(out);
            let in_low = incoming_filter.process(input);
            let out_low_gain = (1.0 - progress * 1.25).clamp(0.0, 1.0);
            let in_low_gain = ((progress - 0.20) * 1.25).clamp(0.0, 1.0);
            out = [
                out[0] - out_low[0] + out_low[0] * out_low_gain,
                out[1] - out_low[1] + out_low[1] * out_low_gain,
            ];
            input = [
                input[0] - in_low[0] + in_low[0] * in_low_gain,
                input[1] - in_low[1] + in_low[1] * in_low_gain,
            ];
        }

        writer.write_frame([
            soft_clip(out[0] * out_fade + input[0] * in_fade),
            soft_clip(out[1] * out_fade + input[1] * in_fade),
        ])?;
    }
    Ok(())
}

struct LowPassStereo {
    alpha: f32,
    state: [f32; 2],
}

impl LowPassStereo {
    fn new(cutoff_hz: f32) -> Self {
        let dt = 1.0 / OUTPUT_SAMPLE_RATE as f32;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        Self {
            alpha: dt / (rc + dt),
            state: [0.0, 0.0],
        }
    }

    fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        self.state[0] += self.alpha * (frame[0] - self.state[0]);
        self.state[1] += self.alpha * (frame[1] - self.state[1]);
        self.state
    }
}

fn cached_or_analyze(
    track: &DjMixTrack,
    cache: &mut AnalysisCache,
    cancel: &AtomicBool,
) -> Result<TrackAnalysis> {
    let metadata = fs::metadata(&track.path)
        .with_context(|| format!("Track is missing: {}", track.path.display()))?;
    let modified_nanos = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let key = track.path.to_string_lossy().to_string();
    if let Some(entry) = cache.tracks.get(&key) {
        if entry.file_len == metadata.len() && entry.modified_nanos == modified_nanos {
            return Ok(entry.analysis.clone());
        }
    }

    let analysis = analyze_track(&track.path, cancel)?;
    cache.tracks.insert(
        key,
        CachedTrackAnalysis {
            file_len: metadata.len(),
            modified_nanos,
            analysis: analysis.clone(),
        },
    );
    Ok(analysis)
}

fn analyze_track(path: &Path, cancel: &AtomicBool) -> Result<TrackAnalysis> {
    let file = File::open(path)?;
    let decoder = Decoder::new(BufReader::new(file))
        .with_context(|| format!("Failed to decode audio file: {}", path.display()))?;
    let duration_seconds = decoder
        .total_duration()
        .map(|duration| duration.as_secs_f32());
    let mut source = UniformSourceIterator::<_, f32>::new(decoder, 1, OUTPUT_SAMPLE_RATE);
    let samples_per_bucket = OUTPUT_SAMPLE_RATE as usize / ANALYSIS_RATE_HZ;
    let max_samples = ANALYSIS_SECONDS * OUTPUT_SAMPLE_RATE as usize;
    let mut envelope = Vec::with_capacity(ANALYSIS_SECONDS * ANALYSIS_RATE_HZ);
    let mut bucket_sum = 0.0f64;
    let mut bucket_count = 0usize;
    let mut total_sum = 0.0f64;
    let mut total_count = 0usize;

    for sample in source.by_ref().take(max_samples) {
        if total_count % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        let value = sample as f64;
        let squared = value * value;
        bucket_sum += squared;
        total_sum += squared;
        bucket_count += 1;
        total_count += 1;
        if bucket_count >= samples_per_bucket {
            envelope.push((bucket_sum / bucket_count as f64).sqrt() as f32);
            bucket_sum = 0.0;
            bucket_count = 0;
        }
    }
    if bucket_count > 0 {
        envelope.push((bucket_sum / bucket_count as f64).sqrt() as f32);
    }
    if envelope.len() < ANALYSIS_RATE_HZ * 8 {
        return Err(anyhow!(
            "Track is too short for DJ analysis: {}",
            path.display()
        ));
    }

    let rms = if total_count > 0 {
        (total_sum / total_count as f64).sqrt() as f32
    } else {
        0.0
    };
    let max_level = envelope.iter().copied().fold(0.0f32, f32::max);
    let silence_threshold = (max_level * 0.08).max(0.0035);
    let leading_bucket = first_sustained_level(&envelope, silence_threshold, 4).unwrap_or(0);
    let leading_silence_seconds = leading_bucket as f32 / ANALYSIS_RATE_HZ as f32;

    let mut onset = vec![0.0f32; envelope.len()];
    for index in 1..envelope.len() {
        onset[index] = (envelope[index] - envelope[index - 1]).max(0.0).powf(0.75);
    }
    remove_local_mean(&mut onset, ANALYSIS_RATE_HZ / 2);
    let (bpm, lag) = estimate_bpm(&onset);
    let beat_phase = estimate_beat_phase(&onset, lag);

    Ok(TrackAnalysis {
        bpm,
        beat_offset_seconds: beat_phase as f32 / ANALYSIS_RATE_HZ as f32,
        leading_silence_seconds,
        rms,
        duration_seconds,
    })
}

fn estimate_bpm(onset: &[f32]) -> (f32, usize) {
    let min_lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / MAX_BPM).floor() as usize;
    let max_lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / MIN_BPM).ceil() as usize;
    let mut best_lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / DEFAULT_BPM) as usize;
    let mut best_score = 0.0f64;

    for lag in min_lag..=max_lag {
        let mut score = 0.0f64;
        let mut energy = 0.0f64;
        for index in lag..onset.len() {
            score += onset[index] as f64 * onset[index - lag] as f64;
            energy += onset[index] as f64 * onset[index] as f64;
        }
        let normalized = if energy > f64::EPSILON {
            score / energy.sqrt()
        } else {
            0.0
        };
        if normalized > best_score {
            best_score = normalized;
            best_lag = lag;
        }
    }

    let mut bpm = ANALYSIS_RATE_HZ as f32 * 60.0 / best_lag as f32;
    while bpm < 88.0 {
        bpm *= 2.0;
    }
    while bpm > 176.0 {
        bpm *= 0.5;
    }
    if !bpm.is_finite() || best_score <= 0.0 {
        bpm = DEFAULT_BPM;
    }
    let normalized_lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / bpm).round().max(1.0) as usize;
    (bpm, normalized_lag)
}

fn estimate_beat_phase(onset: &[f32], lag: usize) -> usize {
    if lag == 0 {
        return 0;
    }
    (0..lag)
        .max_by(|left, right| {
            phase_score(onset, *left, lag)
                .partial_cmp(&phase_score(onset, *right, lag))
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or(0)
}

fn phase_score(onset: &[f32], phase: usize, lag: usize) -> f32 {
    (phase..onset.len())
        .step_by(lag)
        .map(|index| onset[index])
        .sum()
}

fn remove_local_mean(values: &mut [f32], radius: usize) {
    if values.is_empty() || radius == 0 {
        return;
    }
    let original = values.to_vec();
    let mut prefix = vec![0.0f32; original.len() + 1];
    for (index, value) in original.iter().enumerate() {
        prefix[index + 1] = prefix[index] + value;
    }
    for index in 0..values.len() {
        let start = index.saturating_sub(radius);
        let end = (index + radius + 1).min(values.len());
        let mean = (prefix[end] - prefix[start]) / (end - start) as f32;
        values[index] = (original[index] - mean).max(0.0);
    }
}

fn first_sustained_level(values: &[f32], threshold: f32, count: usize) -> Option<usize> {
    if count == 0 || values.len() < count {
        return None;
    }
    values
        .windows(count)
        .position(|window| window.iter().all(|value| *value >= threshold))
}

fn plan_tracks(
    mut analyzed: Vec<(DjMixTrack, TrackAnalysis)>,
    options: DjMixOptions,
) -> Vec<PlannedTrack> {
    if options.smart_order && analyzed.len() > 2 {
        analyzed = smart_order(analyzed);
    }
    let mut bpms = analyzed
        .iter()
        .map(|(_, analysis)| analysis.bpm)
        .filter(|bpm| bpm.is_finite() && *bpm > 0.0)
        .collect::<Vec<_>>();
    bpms.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let target_bpm = bpms.get(bpms.len() / 2).copied().unwrap_or(DEFAULT_BPM);

    analyzed
        .into_iter()
        .map(|(track, analysis)| {
            let speed_ratio = if analysis.bpm > 0.0 {
                (target_bpm / analysis.bpm).clamp(1.0 - MAX_TEMPO_CHANGE, 1.0 + MAX_TEMPO_CHANGE)
            } else {
                1.0
            };
            let gain = if options.normalize_loudness && analysis.rms > 0.001 {
                (TARGET_RMS / analysis.rms).clamp(0.55, 1.8)
            } else {
                1.0
            };
            PlannedTrack {
                track,
                analysis,
                speed_ratio,
                gain,
            }
        })
        .collect()
}

fn smart_order(mut tracks: Vec<(DjMixTrack, TrackAnalysis)>) -> Vec<(DjMixTrack, TrackAnalysis)> {
    if tracks.len() <= 2 {
        return tracks;
    }
    let mut ordered = Vec::with_capacity(tracks.len());
    ordered.push(tracks.remove(0));
    while !tracks.is_empty() {
        let previous_bpm = ordered
            .last()
            .map(|(_, analysis)| analysis.bpm)
            .unwrap_or(DEFAULT_BPM);
        let next_index = tracks
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                (left.1.bpm - previous_bpm)
                    .abs()
                    .partial_cmp(&(right.1.bpm - previous_bpm).abs())
                    .unwrap_or(Ordering::Equal)
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        ordered.push(tracks.remove(next_index));
    }
    ordered
}

fn transition_frame_count(current: &PlannedTrack, next: &PlannedTrack, beats: u32) -> usize {
    let current_bpm = (current.analysis.bpm * current.speed_ratio).clamp(MIN_BPM, MAX_BPM);
    let next_bpm = (next.analysis.bpm * next.speed_ratio).clamp(MIN_BPM, MAX_BPM);
    let mix_bpm = (current_bpm + next_bpm) * 0.5;
    let desired_seconds = (beats.max(4) as f32 * 60.0 / mix_bpm).clamp(4.0, MAX_TRANSITION_SECONDS);
    let actual_seconds = current
        .analysis
        .duration_seconds
        .map(|duration| duration / current.speed_ratio)
        .filter(|duration| *duration > desired_seconds + 1.0)
        .map(|duration| {
            let period = 60.0 / current_bpm;
            let phase = current.analysis.beat_offset_seconds / current.speed_ratio;
            let candidate = duration - desired_seconds;
            let aligned_start = if candidate <= phase {
                phase.min(candidate)
            } else {
                phase + ((candidate - phase) / period).floor() * period
            };
            (duration - aligned_start).clamp(desired_seconds, MAX_TRANSITION_SECONDS)
        })
        .unwrap_or(desired_seconds);

    (actual_seconds * OUTPUT_SAMPLE_RATE as f32)
        .round()
        .max(1.0) as usize
}

fn aligned_intro_skip(track: &PlannedTrack) -> f32 {
    let period = 60.0 / (track.analysis.bpm * track.speed_ratio).max(MIN_BPM);
    let leading = (track.analysis.leading_silence_seconds / track.speed_ratio)
        .clamp(0.0, MAX_INTRO_SKIP_SECONDS);
    let phase = (track.analysis.beat_offset_seconds / track.speed_ratio).max(0.0);
    if leading <= phase {
        phase.min(MAX_INTRO_SKIP_SECONDS)
    } else {
        (phase + ((leading - phase) / period).ceil() * period).min(MAX_INTRO_SKIP_SECONDS)
    }
}

fn load_analysis_cache() -> AnalysisCache {
    let Some(path) = analysis_cache_path() else {
        return AnalysisCache::default();
    };
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

fn save_analysis_cache(cache: &AnalysisCache) {
    let Some(path) = analysis_cache_path() else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    if let Ok(bytes) = serde_json::to_vec(cache) {
        let _ = fs::write(path, bytes);
    }
}

fn analysis_cache_path() -> Option<PathBuf> {
    app_data_dir().map(|directory| directory.join("dj-analysis-cache.json"))
}

fn read_stereo_frame<I>(source: &mut I) -> Option<[f32; 2]>
where
    I: Iterator<Item = f32>,
{
    let left = source.next()?;
    let right = source.next().unwrap_or(left);
    Some([left, right])
}

fn scale_frame(frame: [f32; 2], gain: f32) -> [f32; 2] {
    [soft_clip(frame[0] * gain), soft_clip(frame[1] * gain)]
}

fn soft_clip(value: f32) -> f32 {
    value.clamp(-1.0, 1.0)
}

fn float_to_i16(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn ensure_mp3_extension(path: &Path) -> PathBuf {
    if path
        .extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| extension.eq_ignore_ascii_case("mp3"))
    {
        path.to_path_buf()
    } else {
        path.with_extension("mp3")
    }
}

fn temporary_output_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("dj-mix.mp3");
    path.with_file_name(format!("{file_name}.part"))
}

#[cfg(windows)]
fn lower_worker_priority() {
    use windows_sys::Win32::System::Threading::{
        GetCurrentThread, SetThreadPriority, THREAD_PRIORITY_BELOW_NORMAL,
    };
    unsafe {
        let _ = SetThreadPriority(GetCurrentThread(), THREAD_PRIORITY_BELOW_NORMAL);
    }
}

#[cfg(not(windows))]
fn lower_worker_priority() {}

fn ensure_not_cancelled(cancel: &AtomicBool) -> Result<()> {
    if cancel.load(AtomicOrdering::Relaxed) {
        Err(anyhow!("DJ mix export cancelled."))
    } else {
        Ok(())
    }
}

fn send_progress(sender: &Sender<DjMixEvent>, stage: String, progress: f32) {
    let _ = sender.send(DjMixEvent::Progress {
        stage,
        progress: progress.clamp(0.0, 1.0),
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn estimates_known_bpm_from_impulses() {
        let bpm = 120.0;
        let lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / bpm) as usize;
        let mut onset = vec![0.0; ANALYSIS_RATE_HZ * 30];
        for index in (7..onset.len()).step_by(lag) {
            onset[index] = 1.0;
        }
        let (estimated, _) = estimate_bpm(&onset);
        assert!((estimated - bpm).abs() < 1.0, "estimated {estimated}");
    }

    #[test]
    fn smart_order_keeps_first_track_and_clusters_bpm() {
        let track = |title: &str, bpm: f32| {
            (
                DjMixTrack {
                    path: PathBuf::from(title),
                    title: title.to_owned(),
                },
                TrackAnalysis {
                    bpm,
                    beat_offset_seconds: 0.0,
                    leading_silence_seconds: 0.0,
                    rms: 0.1,
                    duration_seconds: Some(180.0),
                },
            )
        };
        let ordered = smart_order(vec![
            track("first", 120.0),
            track("far", 150.0),
            track("near", 124.0),
        ]);
        assert_eq!(ordered[0].0.title, "first");
        assert_eq!(ordered[1].0.title, "near");
    }

    #[test]
    fn mp3_extension_is_added_once() {
        assert_eq!(
            ensure_mp3_extension(Path::new("mix")),
            PathBuf::from("mix.mp3")
        );
        assert_eq!(
            ensure_mp3_extension(Path::new("mix.MP3")),
            PathBuf::from("mix.MP3")
        );
    }

    #[test]
    fn mp3_stream_writer_creates_output() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "audio-orbit-dj-writer-{}-{unique}.mp3",
            std::process::id()
        ));
        let mut writer = Mp3StreamWriter::create(&path, 192).expect("create MP3 writer");
        for index in 0..(OUTPUT_SAMPLE_RATE / 4) {
            let phase = index as f32 / OUTPUT_SAMPLE_RATE as f32 * 440.0 * std::f32::consts::TAU;
            let sample = phase.sin() * 0.2;
            writer.write_frame([sample, sample]).expect("encode frame");
        }
        writer.finish().expect("finish MP3");
        let bytes = fs::read(&path).expect("read MP3");
        let _ = fs::remove_file(path);
        assert!(bytes.len() > 1_000);
        assert_eq!(bytes[0], 0xff);
    }
}
