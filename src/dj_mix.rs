use crate::{
    app_data_dir, time_stretch::pitch_preserving_stretch, DjMixEvent, DjMixOptions, DjMixStyle,
    DjMixTrack, DjTrackSectionMode,
};
use anyhow::{anyhow, Context, Result};
use ebur128::{EbuR128, Mode};
use rodio::{source::UniformSourceIterator, Decoder, Source};
use serde::{Deserialize, Serialize};
use shine_rs::{Mp3Encoder, Mp3EncoderConfig, StereoMode};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    fs::{self, File},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc::Sender,
        Arc,
    },
    time::{Instant, UNIX_EPOCH},
};

const ENGINE_SCHEMA_VERSION: u32 = 2;
const ANALYZER_VERSION: &str = "audio-orbit-envelope-ebur128-v2";
const ENGINE_VERSION: &str = "performance-dj-v5-composed-bridge-wsola";
const OUTPUT_SAMPLE_RATE: u32 = 44_100;
const OUTPUT_CHANNELS: u16 = 2;
const ANALYSIS_RATE_HZ: usize = 100;
const ENERGY_RATE_HZ: usize = 2;
const MAX_ANALYSIS_SECONDS: usize = 60 * 15;
const MIN_BPM: f32 = 70.0;
const MAX_BPM: f32 = 180.0;
const DEFAULT_BPM: f32 = 120.0;
const MAX_SMART_TEMPO_CHANGE: f32 = 0.06;
const TARGET_LUFS: f64 = -14.0;
const OUTPUT_PEAK_LIMIT: f32 = 0.96;
const MIN_SECTION_SECONDS: f32 = 24.0;
const MAX_AUTO_SECTION_SECONDS: f32 = 150.0;

#[derive(Clone, Debug)]
pub(crate) struct ExportRequest {
    pub tracks: Vec<DjMixTrack>,
    pub options: DjMixOptions,
    pub output_path: PathBuf,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TrackAnalysis {
    bpm: f32,
    bpm_confidence: f32,
    beat_offset_seconds: f32,
    downbeat_offset_seconds: f32,
    leading_silence_seconds: f32,
    trailing_silence_seconds: f32,
    integrated_lufs: Option<f64>,
    sample_peak: f64,
    true_peak: f64,
    duration_seconds: f32,
    energy_curve: Vec<f32>,
    sections: Vec<DetectedSection>,
    musical_key: Option<String>,
    key_confidence: Option<f32>,
    vocal_profile: AnalysisAvailability,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct DetectedSection {
    kind: SectionKind,
    start_seconds: f32,
    end_seconds: f32,
    energy: f32,
    confidence: f32,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum SectionKind {
    Intro,
    BuildUp,
    Breakdown,
    Drop,
    Chorus,
    Outro,
    Main,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "snake_case")]
enum AnalysisAvailability {
    Unavailable { reason: String },
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct CachedTrackAnalysis {
    file_len: u64,
    modified_nanos: u128,
    analyzer_version: String,
    analysis: TrackAnalysis,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct AnalysisCache {
    schema_version: u32,
    tracks: BTreeMap<String, CachedTrackAnalysis>,
}

impl Default for AnalysisCache {
    fn default() -> Self {
        Self {
            schema_version: ENGINE_SCHEMA_VERSION,
            tracks: BTreeMap::new(),
        }
    }
}

#[derive(Clone, Debug)]
struct PlannedTrack {
    track: DjMixTrack,
    analysis: TrackAnalysis,
    speed_ratio: f32,
    gain: f32,
    section_start_seconds: f32,
    section_end_seconds: f32,
    section_reason: String,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct MixReport {
    schema_version: u32,
    engine_version: String,
    analyzer_version: String,
    output_path: String,
    requested_target_minutes: f32,
    planned_duration_seconds: f32,
    rendered_duration_seconds: f32,
    integrated_lufs: Option<f64>,
    sample_peak_dbfs: Option<f64>,
    true_peak_dbfs: Option<f64>,
    timings_ms: RenderTimings,
    tracks: Vec<TrackDiagnostic>,
    transitions: Vec<TransitionDiagnostic>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct RenderTimings {
    analysis: u128,
    planning: u128,
    rendering: u128,
    finalization: u128,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TrackDiagnostic {
    title: String,
    path: String,
    bpm: f32,
    bpm_confidence: f32,
    beat_offset_seconds: f32,
    downbeat_offset_seconds: f32,
    integrated_lufs: Option<f64>,
    true_peak_dbfs: Option<f64>,
    musical_key: Option<String>,
    key_confidence: Option<f32>,
    vocal_analysis: String,
    selected_section: String,
    section_start_seconds: f32,
    section_end_seconds: f32,
    section_reason: String,
    speed_ratio: f32,
    gain_db: f32,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
struct TransitionDiagnostic {
    from_title: String,
    to_title: String,
    style: String,
    phrase_bars: u32,
    duration_seconds: f32,
    effective_bpm: f32,
    beat_alignment_error_ms: f32,
    peak_reduction_db: f32,
    decisions: Vec<String>,
    warnings: Vec<String>,
}

#[derive(Clone, Debug, Default)]
struct OutputMeasurement {
    duration_seconds: f32,
    integrated_lufs: Option<f64>,
    sample_peak_dbfs: Option<f64>,
    true_peak_dbfs: Option<f64>,
}

struct Mp3StreamWriter {
    file: BufWriter<File>,
    encoder: Mp3Encoder,
    pcm: Vec<i16>,
    measurement_pcm: Vec<f32>,
    loudness: EbuR128,
    flush_samples: usize,
    frames_written: u64,
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
        let loudness = EbuR128::new(
            OUTPUT_CHANNELS as u32,
            OUTPUT_SAMPLE_RATE,
            Mode::I | Mode::SAMPLE_PEAK | Mode::TRUE_PEAK,
        )
        .map_err(|error| anyhow!("Failed to initialize EBU R128 output meter: {error}"))?;

        Ok(Self {
            file: BufWriter::new(file),
            encoder,
            pcm: Vec::with_capacity(flush_samples),
            measurement_pcm: Vec::with_capacity(flush_samples),
            loudness,
            flush_samples,
            frames_written: 0,
        })
    }

    fn write_frame(&mut self, frame: [f32; 2]) -> Result<()> {
        let frame = limit_frame(frame);
        self.pcm.push(float_to_i16(frame[0]));
        self.pcm.push(float_to_i16(frame[1]));
        self.measurement_pcm.push(frame[0]);
        self.measurement_pcm.push(frame[1]);
        self.frames_written += 1;
        if self.pcm.len() >= self.flush_samples {
            self.flush_pcm()?;
        }
        Ok(())
    }

    fn flush_pcm(&mut self) -> Result<()> {
        if self.pcm.is_empty() {
            return Ok(());
        }
        self.loudness
            .add_frames_f32(&self.measurement_pcm)
            .map_err(|error| anyhow!("EBU R128 output measurement failed: {error}"))?;
        self.measurement_pcm.clear();
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

    fn finish(mut self) -> Result<OutputMeasurement> {
        self.flush_pcm()?;
        let tail = self
            .encoder
            .finish()
            .map_err(|error| anyhow!("Failed to finish MP3 encoding: {error}"))?;
        self.file.write_all(&tail)?;
        self.file.flush()?;

        Ok(OutputMeasurement {
            duration_seconds: self.frames_written as f32 / OUTPUT_SAMPLE_RATE as f32,
            integrated_lufs: self
                .loudness
                .loudness_global()
                .ok()
                .filter(|value| value.is_finite()),
            sample_peak_dbfs: maximum_peak_dbfs(&self.loudness, false),
            true_peak_dbfs: maximum_peak_dbfs(&self.loudness, true),
        })
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
    let report_path = report_output_path(&output_path);
    let temporary_report_path = temporary_output_path(&report_path);
    let result = export_mix_inner(
        &request,
        &temporary_path,
        &temporary_report_path,
        &output_path,
        &sender,
        &cancel,
    );

    match result {
        Ok((measurement, summary)) => {
            if cancel.load(AtomicOrdering::Relaxed) {
                cleanup_temporary_files(&temporary_path, &temporary_report_path);
                let _ = sender.send(DjMixEvent::Cancelled);
                return;
            }
            let _ = fs::remove_file(&output_path);
            let _ = fs::remove_file(&report_path);
            let audio_result = fs::rename(&temporary_path, &output_path);
            let report_result = fs::rename(&temporary_report_path, &report_path);
            match (audio_result, report_result) {
                (Ok(()), Ok(())) => {
                    let _ = sender.send(DjMixEvent::Completed {
                        output_path,
                        report_path,
                        track_count: request.tracks.len(),
                        duration_seconds: measurement.duration_seconds,
                        integrated_lufs: measurement.integrated_lufs,
                        true_peak_dbfs: measurement.true_peak_dbfs,
                        diagnostics_summary: summary,
                    });
                }
                (audio, report) => {
                    cleanup_temporary_files(&temporary_path, &temporary_report_path);
                    let _ = fs::remove_file(&output_path);
                    let _ = fs::remove_file(&report_path);
                    let _ = sender.send(DjMixEvent::Failed(format!(
                        "Failed to finalize DJ mix: audio={audio:?}, diagnostics={report:?}"
                    )));
                }
            }
        }
        Err(error) => {
            cleanup_temporary_files(&temporary_path, &temporary_report_path);
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
    temporary_report_path: &Path,
    final_output_path: &Path,
    sender: &Sender<DjMixEvent>,
    cancel: &AtomicBool,
) -> Result<(OutputMeasurement, String)> {
    if request.tracks.len() < 2 {
        return Err(anyhow!("DJ mix requires at least two tracks."));
    }
    validate_options(request.options)?;
    if let Some(parent) = temporary_path.parent() {
        fs::create_dir_all(parent)?;
    }

    let mut timings = RenderTimings::default();
    let analysis_started = Instant::now();
    let mut cache = load_analysis_cache();
    let mut analyzed = Vec::with_capacity(request.tracks.len());
    for (index, track) in request.tracks.iter().enumerate() {
        ensure_not_cancelled(cancel)?;
        send_progress(
            sender,
            format!("Analyzing {}", track.title),
            index as f32 / (request.tracks.len() as f32 * 2.5),
        );
        let analysis = cached_or_analyze(track, &mut cache, cancel)?;
        analyzed.push((track.clone(), analysis));
    }
    save_analysis_cache(&cache);
    timings.analysis = analysis_started.elapsed().as_millis();

    let planning_started = Instant::now();
    let planned = plan_tracks(analyzed, request.options)?;
    let transitions = plan_transition_diagnostics(&planned, request.options);
    timings.planning = planning_started.elapsed().as_millis();

    let rendering_started = Instant::now();
    let mut writer = Mp3StreamWriter::create(temporary_path, request.options.bitrate_kbps)?;
    render_planned_mix(&planned, request.options, &mut writer, sender, cancel)?;
    ensure_not_cancelled(cancel)?;
    timings.rendering = rendering_started.elapsed().as_millis();

    send_progress(sender, "Finalizing MP3 and diagnostics".to_owned(), 0.99);
    let finalization_started = Instant::now();
    let measurement = writer.finish()?;
    timings.finalization = finalization_started.elapsed().as_millis();
    let report = build_report(
        final_output_path,
        &planned,
        transitions,
        request.options,
        measurement.clone(),
        timings.clone(),
    );
    let report_bytes = serde_json::to_vec_pretty(&report)
        .context("Failed to serialize DJ transition diagnostics")?;
    fs::write(temporary_report_path, report_bytes).with_context(|| {
        format!(
            "Failed to write diagnostics: {}",
            temporary_report_path.display()
        )
    })?;

    let summary = format!(
        "{:.1} min, {} transitions, {:.1} LUFS, {:.1} dBTP",
        measurement.duration_seconds / 60.0,
        report.transitions.len(),
        measurement.integrated_lufs.unwrap_or(f64::NAN),
        measurement.true_peak_dbfs.unwrap_or(f64::NAN)
    );
    Ok((measurement, summary))
}

fn validate_options(options: DjMixOptions) -> Result<()> {
    if !matches!(options.transition_bars, 8 | 16 | 32) {
        return Err(anyhow!("DJ transition must be 8, 16, or 32 bars."));
    }
    if !(1.0..=180.0).contains(&options.target_minutes) {
        return Err(anyhow!(
            "DJ mix target length must be between 1 and 180 minutes."
        ));
    }
    if !matches!(options.bitrate_kbps, 192 | 256 | 320) {
        return Err(anyhow!("DJ MP3 bitrate must be 192, 256, or 320 kbps."));
    }
    Ok(())
}

fn render_planned_mix(
    tracks: &[PlannedTrack],
    options: DjMixOptions,
    writer: &mut Mp3StreamWriter,
    sender: &Sender<DjMixEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    let first = tracks
        .first()
        .ok_or_else(|| anyhow!("DJ plan contains no tracks."))?;
    send_progress(
        sender,
        format!("Decoding and preparing {}", first.track.title),
        0.40,
    );
    let mut current_audio = render_planned_section(first, options.style, cancel)?;
    let planned_transitions = planned_transition_frame_counts(tracks, options);

    for pair_index in 0..tracks.len() - 1 {
        ensure_not_cancelled(cancel)?;
        let current = &tracks[pair_index];
        let next = &tracks[pair_index + 1];
        send_progress(
            sender,
            format!(
                "{} {} → {}",
                match options.style {
                    DjMixStyle::Crossfade => "Crossfading",
                    DjMixStyle::SmartDj => "Human DJ mixing",
                },
                current.track.title,
                next.track.title
            ),
            0.45 + pair_index as f32 / ((tracks.len() - 1) as f32 * 2.0),
        );

        send_progress(
            sender,
            format!("Decoding and preparing {}", next.track.title),
            0.45 + pair_index as f32 / ((tracks.len() - 1) as f32 * 2.0),
        );
        let next_audio = render_planned_section(next, options.style, cancel)?;
        let requested_transition = planned_transitions[pair_index];
        let following_transition = planned_transitions
            .get(pair_index + 1)
            .copied()
            .unwrap_or(0);
        let next_budget = incoming_transition_budget(
            next_audio.len(),
            requested_transition,
            following_transition,
        );
        let overlap = requested_transition
            .min(current_audio.len())
            .min(next_budget);
        if overlap == 0 {
            write_frames(&current_audio, current.gain, writer, cancel)?;
            current_audio = next_audio;
            continue;
        }

        let body_len = current_audio.len().saturating_sub(overlap);
        write_frames(&current_audio[..body_len], current.gain, writer, cancel)?;
        match options.style {
            DjMixStyle::Crossfade => write_crossfade_transition(
                &current_audio[body_len..],
                &next_audio[..overlap],
                current.gain,
                next.gain,
                options.bass_swap,
                writer,
                cancel,
            )?,
            DjMixStyle::SmartDj => write_performance_transition(
                &current_audio[body_len..],
                &next_audio[..overlap],
                current,
                next,
                options.bass_swap,
                writer,
                cancel,
            )?,
        }
        current_audio = next_audio[overlap..].to_vec();
    }

    let last = tracks
        .last()
        .ok_or_else(|| anyhow!("DJ plan contains no tracks."))?;
    write_frames(&current_audio, last.gain, writer, cancel)?;
    Ok(())
}

fn render_planned_section(
    track: &PlannedTrack,
    style: DjMixStyle,
    cancel: &AtomicBool,
) -> Result<Vec<[f32; 2]>> {
    let decoded = decode_section(
        &track.track.path,
        track.section_start_seconds,
        track.section_end_seconds,
        cancel,
    )?;
    if style == DjMixStyle::Crossfade || (track.speed_ratio - 1.0).abs() < 0.0005 {
        return Ok(decoded);
    }

    ensure_not_cancelled(cancel)?;
    let output = pitch_preserving_stretch(&decoded, track.speed_ratio);
    ensure_not_cancelled(cancel)?;
    if output.is_empty() {
        return Err(anyhow!(
            "Pitch-preserving time stretch failed for {}",
            track.track.title
        ));
    }
    Ok(output)
}

fn decode_section(
    path: &Path,
    start_seconds: f32,
    end_seconds: f32,
    cancel: &AtomicBool,
) -> Result<Vec<[f32; 2]>> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open audio file: {}", path.display()))?;
    let decoder = Decoder::new(BufReader::new(file))
        .with_context(|| format!("Failed to decode audio file: {}", path.display()))?;
    let mut source =
        UniformSourceIterator::<_, f32>::new(decoder, OUTPUT_CHANNELS, OUTPUT_SAMPLE_RATE);
    let start_frame = (start_seconds.max(0.0) * OUTPUT_SAMPLE_RATE as f32).round() as usize;
    let end_frame = (end_seconds.max(start_seconds) * OUTPUT_SAMPLE_RATE as f32).round() as usize;
    let frame_count = end_frame.saturating_sub(start_frame);
    for index in 0..start_frame {
        if index % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        if read_stereo_frame(&mut source).is_none() {
            break;
        }
    }
    let mut output = Vec::with_capacity(frame_count);
    for index in 0..frame_count {
        if index % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        let Some(frame) = read_stereo_frame(&mut source) else {
            break;
        };
        output.push(frame);
    }
    if output.is_empty() {
        return Err(anyhow!(
            "Selected section is unavailable: {}",
            path.display()
        ));
    }
    Ok(output)
}

fn write_frames(
    frames: &[[f32; 2]],
    gain: f32,
    writer: &mut Mp3StreamWriter,
    cancel: &AtomicBool,
) -> Result<()> {
    for (index, frame) in frames.iter().enumerate() {
        if index % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        writer.write_frame(multiply_frame(*frame, gain))?;
    }
    Ok(())
}

fn write_crossfade_transition(
    outgoing: &[[f32; 2]],
    incoming: &[[f32; 2]],
    outgoing_gain: f32,
    incoming_gain: f32,
    bass_swap: bool,
    writer: &mut Mp3StreamWriter,
    cancel: &AtomicBool,
) -> Result<()> {
    let frames = outgoing.len().min(incoming.len());
    let peak_scale = transition_peak_scale(
        &outgoing[..frames],
        &incoming[..frames],
        outgoing_gain,
        incoming_gain,
    );
    let mut outgoing_filter = LowPassStereo::new(180.0);
    let mut incoming_filter = LowPassStereo::new(180.0);
    for index in 0..frames {
        if index % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        let progress = normalized_progress(index, frames);
        let out_fade = ((1.0 - progress) * std::f32::consts::FRAC_PI_2).sin();
        let in_fade = (progress * std::f32::consts::FRAC_PI_2).sin();
        let mut out = multiply_frame(outgoing[index], outgoing_gain * peak_scale);
        let mut input = multiply_frame(incoming[index], incoming_gain * peak_scale);
        if bass_swap {
            apply_bass_swap(
                &mut out,
                &mut input,
                progress,
                &mut outgoing_filter,
                &mut incoming_filter,
            );
        }
        writer.write_frame([
            out[0] * out_fade + input[0] * in_fade,
            out[1] * out_fade + input[1] * in_fade,
        ])?;
    }
    Ok(())
}

fn write_performance_transition(
    outgoing: &[[f32; 2]],
    incoming: &[[f32; 2]],
    current: &PlannedTrack,
    next: &PlannedTrack,
    bass_swap: bool,
    writer: &mut Mp3StreamWriter,
    cancel: &AtomicBool,
) -> Result<()> {
    let frames = outgoing.len().min(incoming.len());
    if frames == 0 {
        return Ok(());
    }

    let peak_scale = transition_peak_scale(
        &outgoing[..frames],
        &incoming[..frames],
        current.gain,
        next.gain,
    );
    let bpm = ((current.analysis.bpm + next.analysis.bpm) * 0.5).clamp(MIN_BPM, MAX_BPM);
    let beat_frames = ((60.0 / bpm) * OUTPUT_SAMPLE_RATE as f32).round().max(1.0) as usize;
    let loop_frames = beat_frames
        .min(frames)
        .max((OUTPUT_SAMPLE_RATE / 8) as usize);
    let strong_change = transition_should_hit_hard(current, next);
    let mut outgoing_low = LowPassStereo::new(180.0);
    let mut incoming_low = LowPassStereo::new(180.0);
    let mut outgoing_sweep = LowPassStereo::new(18_000.0);
    let mut incoming_sweep = LowPassStereo::new(350.0);
    let mut delay = StereoDelay::new((beat_frames / 2).max(1), 0.38);
    let mut outgoing_vocal_guard = CenterVocalGuard::new();
    let mut incoming_vocal_guard = CenterVocalGuard::new();
    let vocal_plan = plan_vocal_handoff(&outgoing[..frames], &incoming[..frames]);
    let seed = transition_seed(current, next);
    let mut bridge = MusicalBridge::new(bpm, seed, strong_change);

    for index in 0..frames {
        if index % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        let progress = normalized_progress(index, frames);
        let repeat_divisor = if progress < 0.42 {
            1
        } else if progress < 0.62 {
            2
        } else {
            4
        };
        let repeat_len = (loop_frames / repeat_divisor).max(64);
        let loop_start = frames.saturating_sub(loop_frames);
        let loop_index = loop_start + ((index.saturating_sub(loop_start)) % repeat_len);
        let source_index = if index >= loop_start {
            loop_index.min(frames - 1)
        } else {
            index
        };

        if index % 64 == 0 {
            outgoing_sweep.set_cutoff(18_000.0 - progress.powf(1.6) * 17_300.0);
            incoming_sweep.set_cutoff(450.0 + progress.powf(1.5) * 17_500.0);
        }

        let mut out = multiply_frame(outgoing[source_index], current.gain * peak_scale);
        let mut input = multiply_frame(incoming[index], next.gain * peak_scale);
        let filtered_out = outgoing_sweep.process(out);
        let filtered_in = incoming_sweep.process(input);
        out = mix_frame(out, filtered_out, (progress * 1.3).clamp(0.0, 0.94));
        input = mix_frame(
            input,
            filtered_in,
            ((1.0 - progress) * 1.35).clamp(0.0, 0.96),
        );

        if bass_swap {
            apply_bass_swap(
                &mut out,
                &mut input,
                progress,
                &mut outgoing_low,
                &mut incoming_low,
            );
        }

        let (outgoing_vocal_duck, incoming_vocal_duck) = vocal_plan.duck_amounts(index, progress);
        out = outgoing_vocal_guard.process(out, outgoing_vocal_duck);
        input = incoming_vocal_guard.process(input, incoming_vocal_duck);

        let delayed = delay.process(out);
        let echo_mix = ((progress - 0.30) / 0.42).clamp(0.0, 0.38);
        out = [
            out[0] + delayed[0] * echo_mix,
            out[1] + delayed[1] * echo_mix,
        ];

        // Senior-style handoff: the next complete song does not sit underneath and slowly grow.
        // The outgoing phrase exits, an original rhythmic/melodic bridge owns the middle,
        // then the incoming track is revealed on a phrase boundary or drop.
        let out_gate = 1.0 - smoothstep(0.40, if strong_change { 0.64 } else { 0.72 }, progress);
        let in_gate = smoothstep(
            if strong_change { 0.76 } else { 0.68 },
            if strong_change { 0.84 } else { 0.88 },
            progress,
        );
        let mut mixed = [
            out[0] * out_gate + input[0] * in_gate,
            out[1] * out_gate + input[1] * in_gate,
        ];

        let bridge_frame = bridge.render(index, progress);
        let bridge_gate =
            smoothstep(0.22, 0.38, progress) * (1.0 - smoothstep(0.80, 0.94, progress));
        mixed[0] += bridge_frame[0] * bridge_gate;
        mixed[1] += bridge_frame[1] * bridge_gate;

        if strong_change && progress > 0.72 && progress < 0.76 {
            let cut = 1.0 - smoothstep(0.72, 0.755, progress);
            mixed[0] *= cut;
            mixed[1] *= cut;
        }

        writer.write_frame(limit_frame(mixed))?;
    }
    Ok(())
}

fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    let x = ((value - edge0) / (edge1 - edge0).max(1.0e-6)).clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

struct MusicalBridge {
    bpm: f32,
    seed: u32,
    strong: bool,
    kick_phase: f32,
    bass_phase: f32,
    pluck_phase: f32,
    string_phase: f32,
    previous_step: usize,
    pluck_envelope: f32,
    string_filter: f32,
    noise_state: u32,
}

impl MusicalBridge {
    fn new(bpm: f32, seed: u32, strong: bool) -> Self {
        Self {
            bpm,
            seed,
            strong,
            kick_phase: 0.0,
            bass_phase: 0.0,
            pluck_phase: 0.0,
            string_phase: 0.0,
            previous_step: usize::MAX,
            pluck_envelope: 0.0,
            string_filter: 0.0,
            noise_state: seed ^ 0xA5A5_17C3,
        }
    }

    fn render(&mut self, frame_index: usize, progress: f32) -> [f32; 2] {
        let seconds = frame_index as f32 / OUTPUT_SAMPLE_RATE as f32;
        let beats = seconds * self.bpm / 60.0;
        let step = (beats * 2.0).floor() as usize;
        let beat_phase = beats.fract();
        let half_beat_phase = (beats * 2.0).fract();
        if step != self.previous_step {
            self.previous_step = step;
            self.pluck_envelope = if step % 2 == 0 { 1.0 } else { 0.68 };
        }

        let root_midi = 43 + (self.seed % 8) as i32;
        let scale = [0, 3, 5, 7, 10, 12, 15, 17];
        let note = root_midi + scale[(step + ((self.seed >> 8) as usize)) % scale.len()];
        let note_hz = 440.0 * 2.0f32.powf((note as f32 - 69.0) / 12.0);
        let bass_hz = 440.0 * 2.0f32.powf((root_midi as f32 - 69.0) / 12.0);

        let kick_env = (-beat_phase * 18.0).exp();
        let kick_hz = 48.0 + 75.0 * (-beat_phase * 24.0).exp();
        self.kick_phase = (self.kick_phase + kick_hz / OUTPUT_SAMPLE_RATE as f32).fract();
        let kick = (std::f32::consts::TAU * self.kick_phase).sin() * kick_env * 0.30;

        self.bass_phase = (self.bass_phase + bass_hz / OUTPUT_SAMPLE_RATE as f32).fract();
        let bass = (std::f32::consts::TAU * self.bass_phase).sin()
            * (0.08 + 0.05 * (1.0 - half_beat_phase))
            * (1.0 - kick_env * 0.65);

        self.pluck_phase = (self.pluck_phase + note_hz / OUTPUT_SAMPLE_RATE as f32).fract();
        self.pluck_envelope *= 0.99972;
        let fundamental = (std::f32::consts::TAU * self.pluck_phase).sin();
        let harmonic = (std::f32::consts::TAU * self.pluck_phase * 2.0).sin() * 0.34;
        let pluck = (fundamental + harmonic) * self.pluck_envelope * 0.13;

        let string_hz = note_hz * 0.5;
        self.string_phase = (self.string_phase + string_hz / OUTPUT_SAMPLE_RATE as f32).fract();
        let bow = (std::f32::consts::TAU * self.string_phase).sin()
            + (std::f32::consts::TAU * self.string_phase * 2.01).sin() * 0.28;
        self.string_filter += 0.018 * (bow - self.string_filter);
        let string_rise =
            smoothstep(0.34, 0.66, progress) * (1.0 - smoothstep(0.68, 0.88, progress));
        let string = self.string_filter * string_rise * 0.10;

        let noise = transition_noise(&mut self.noise_state);
        let hat_env = if half_beat_phase < 0.10 {
            (1.0 - half_beat_phase / 0.10) * 0.055
        } else {
            0.0
        };
        let hat = noise * hat_env;
        let snare_position = (beats + 0.5).fract();
        let snare_env = if snare_position < 0.12 {
            (1.0 - snare_position / 0.12).powi(2)
        } else {
            0.0
        };
        let snare = noise * snare_env * if self.strong { 0.12 } else { 0.075 };

        let stereo_motion = (std::f32::consts::TAU * seconds * 0.23).sin() * 0.18;
        let tonal = kick + bass + pluck + string;
        [
            tonal * (1.0 - stereo_motion) + hat + snare,
            tonal * (1.0 + stereo_motion) - hat * 0.72 + snare,
        ]
    }
}

#[derive(Clone, Copy, Debug)]
struct VocalHandoffPlan {
    outgoing_activity: f32,
    incoming_activity: f32,
    handoff_progress: f32,
}

impl VocalHandoffPlan {
    fn duck_amounts(self, _index: usize, progress: f32) -> (f32, f32) {
        let both_vocal = self.outgoing_activity.min(self.incoming_activity);
        if both_vocal < 0.18 {
            return (0.0, 0.0);
        }

        let handoff_width = 0.08;
        let handoff = ((progress - (self.handoff_progress - handoff_width))
            / (handoff_width * 2.0))
            .clamp(0.0, 1.0);
        let strength = ((both_vocal - 0.18) / 0.42).clamp(0.0, 1.0);
        let max_duck = 0.88 * strength;

        // One lead vocal at a time: incoming stays back before the phrase handoff,
        // outgoing gets removed after it. The short interpolation avoids a hard hole.
        (max_duck * handoff, max_duck * (1.0 - handoff))
    }
}

fn plan_vocal_handoff(outgoing: &[[f32; 2]], incoming: &[[f32; 2]]) -> VocalHandoffPlan {
    let outgoing_activity = estimate_center_vocal_activity(outgoing);
    let incoming_activity = estimate_center_vocal_activity(incoming);
    let bias = (incoming_activity - outgoing_activity) * 0.08;
    VocalHandoffPlan {
        outgoing_activity,
        incoming_activity,
        handoff_progress: (0.52 - bias).clamp(0.42, 0.62),
    }
}

fn estimate_center_vocal_activity(frames: &[[f32; 2]]) -> f32 {
    if frames.is_empty() {
        return 0.0;
    }
    let stride = (OUTPUT_SAMPLE_RATE as usize / 50).max(1);
    let mut active_blocks = 0usize;
    let mut measured_blocks = 0usize;
    for block in frames.chunks(stride) {
        let mut mid_energy = 0.0f32;
        let mut side_energy = 0.0f32;
        let mut total_energy = 0.0f32;
        for frame in block {
            let mid = (frame[0] + frame[1]) * 0.5;
            let side = (frame[0] - frame[1]) * 0.5;
            mid_energy += mid * mid;
            side_energy += side * side;
            total_energy += (frame[0] * frame[0] + frame[1] * frame[1]) * 0.5;
        }
        let count = block.len().max(1) as f32;
        let rms = (total_energy / count).sqrt();
        if rms < 0.012 {
            continue;
        }
        measured_blocks += 1;
        let center_ratio = mid_energy / (mid_energy + side_energy + 1.0e-9);
        if center_ratio > 0.68 {
            active_blocks += 1;
        }
    }
    if measured_blocks == 0 {
        0.0
    } else {
        active_blocks as f32 / measured_blocks as f32
    }
}

struct CenterVocalGuard {
    bass_low_pass: f32,
}

impl CenterVocalGuard {
    fn new() -> Self {
        Self { bass_low_pass: 0.0 }
    }

    fn process(&mut self, frame: [f32; 2], amount: f32) -> [f32; 2] {
        let amount = amount.clamp(0.0, 0.92);
        if amount <= 0.0001 {
            return frame;
        }
        let mid = (frame[0] + frame[1]) * 0.5;
        let side = (frame[0] - frame[1]) * 0.5;
        // Preserve centered kick/bass while suppressing the vocal-heavy center band.
        let alpha = 1.0 - (-std::f32::consts::TAU * 180.0 / OUTPUT_SAMPLE_RATE as f32).exp();
        self.bass_low_pass += alpha * (mid - self.bass_low_pass);
        let upper_center = mid - self.bass_low_pass;
        let guarded_mid = self.bass_low_pass + upper_center * (1.0 - amount);
        [guarded_mid + side, guarded_mid - side]
    }
}

fn transition_should_hit_hard(current: &PlannedTrack, next: &PlannedTrack) -> bool {
    let current_energy = current
        .analysis
        .energy_curve
        .iter()
        .copied()
        .fold(0.0, f32::max);
    let next_energy = next
        .analysis
        .energy_curve
        .iter()
        .copied()
        .fold(0.0, f32::max);
    let energy_jump = (next_energy - current_energy).abs();
    let bpm_jump = (next.analysis.bpm - current.analysis.bpm).abs();
    energy_jump > 0.22 || bpm_jump > 12.0
}

fn transition_seed(current: &PlannedTrack, next: &PlannedTrack) -> u32 {
    current
        .track
        .title
        .bytes()
        .chain(next.track.title.bytes())
        .fold(0x9E37_79B9, |state, byte| {
            state.rotate_left(5) ^ byte as u32
        })
}

fn transition_noise(state: &mut u32) -> f32 {
    *state ^= *state << 13;
    *state ^= *state >> 17;
    *state ^= *state << 5;
    (*state as f32 / u32::MAX as f32) * 2.0 - 1.0
}

fn mix_frame(dry: [f32; 2], wet: [f32; 2], amount: f32) -> [f32; 2] {
    let amount = amount.clamp(0.0, 1.0);
    [
        dry[0] * (1.0 - amount) + wet[0] * amount,
        dry[1] * (1.0 - amount) + wet[1] * amount,
    ]
}

struct StereoDelay {
    buffer: Vec<[f32; 2]>,
    cursor: usize,
    feedback: f32,
}

impl StereoDelay {
    fn new(frames: usize, feedback: f32) -> Self {
        Self {
            buffer: vec![[0.0, 0.0]; frames.max(1)],
            cursor: 0,
            feedback: feedback.clamp(0.0, 0.92),
        }
    }

    fn process(&mut self, input: [f32; 2]) -> [f32; 2] {
        let delayed = self.buffer[self.cursor];
        self.buffer[self.cursor] = [
            input[0] + delayed[0] * self.feedback,
            input[1] + delayed[1] * self.feedback,
        ];
        self.cursor = (self.cursor + 1) % self.buffer.len();
        delayed
    }
}

fn apply_bass_swap(
    outgoing: &mut [f32; 2],
    incoming: &mut [f32; 2],
    progress: f32,
    outgoing_filter: &mut LowPassStereo,
    incoming_filter: &mut LowPassStereo,
) {
    let outgoing_low = outgoing_filter.process(*outgoing);
    let incoming_low = incoming_filter.process(*incoming);
    let outgoing_low_gain = if progress < 0.50 { 1.0 } else { 0.0 };
    let incoming_low_gain = if progress < 0.50 { 0.0 } else { 1.0 };
    *outgoing = [
        outgoing[0] - outgoing_low[0] + outgoing_low[0] * outgoing_low_gain,
        outgoing[1] - outgoing_low[1] + outgoing_low[1] * outgoing_low_gain,
    ];
    *incoming = [
        incoming[0] - incoming_low[0] + incoming_low[0] * incoming_low_gain,
        incoming[1] - incoming_low[1] + incoming_low[1] * incoming_low_gain,
    ];
}

struct LowPassStereo {
    alpha: f32,
    state: [f32; 2],
}

impl LowPassStereo {
    fn new(cutoff_hz: f32) -> Self {
        let mut filter = Self {
            alpha: 0.0,
            state: [0.0, 0.0],
        };
        filter.set_cutoff(cutoff_hz);
        filter
    }

    fn process(&mut self, frame: [f32; 2]) -> [f32; 2] {
        self.state[0] += self.alpha * (frame[0] - self.state[0]);
        self.state[1] += self.alpha * (frame[1] - self.state[1]);
        self.state
    }

    fn set_cutoff(&mut self, cutoff_hz: f32) {
        let cutoff_hz = cutoff_hz.clamp(20.0, OUTPUT_SAMPLE_RATE as f32 * 0.45);
        let dt = 1.0 / OUTPUT_SAMPLE_RATE as f32;
        let rc = 1.0 / (2.0 * std::f32::consts::PI * cutoff_hz);
        self.alpha = dt / (rc + dt);
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
    if cache.schema_version == ENGINE_SCHEMA_VERSION {
        if let Some(entry) = cache.tracks.get(&key) {
            if entry.file_len == metadata.len()
                && entry.modified_nanos == modified_nanos
                && entry.analyzer_version == ANALYZER_VERSION
            {
                return Ok(entry.analysis.clone());
            }
        }
    }

    let analysis = analyze_track(&track.path, cancel)?;
    cache.schema_version = ENGINE_SCHEMA_VERSION;
    cache.tracks.insert(
        key,
        CachedTrackAnalysis {
            file_len: metadata.len(),
            modified_nanos,
            analyzer_version: ANALYZER_VERSION.to_owned(),
            analysis: analysis.clone(),
        },
    );
    Ok(analysis)
}

fn analyze_track(path: &Path, cancel: &AtomicBool) -> Result<TrackAnalysis> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open audio file: {}", path.display()))?;
    let decoder = Decoder::new(BufReader::new(file))
        .with_context(|| format!("Failed to decode audio file: {}", path.display()))?;
    let declared_duration = decoder.total_duration().map(|value| value.as_secs_f32());
    let mut source =
        UniformSourceIterator::<_, f32>::new(decoder, OUTPUT_CHANNELS, OUTPUT_SAMPLE_RATE);
    let mut loudness = EbuR128::new(
        OUTPUT_CHANNELS as u32,
        OUTPUT_SAMPLE_RATE,
        Mode::I | Mode::SAMPLE_PEAK | Mode::TRUE_PEAK,
    )
    .map_err(|error| anyhow!("Failed to initialize EBU R128 analyzer: {error}"))?;

    let analysis_frames = OUTPUT_SAMPLE_RATE as usize * MAX_ANALYSIS_SECONDS;
    let envelope_stride = OUTPUT_SAMPLE_RATE as usize / ANALYSIS_RATE_HZ;
    let energy_stride = OUTPUT_SAMPLE_RATE as usize / ENERGY_RATE_HZ;
    let meter_chunk_frames = 4096usize;
    let mut meter_chunk = Vec::with_capacity(meter_chunk_frames * 2);
    let mut envelope = Vec::new();
    let mut energy_curve = Vec::new();
    let mut envelope_sum = 0.0f32;
    let mut envelope_count = 0usize;
    let mut energy_sum = 0.0f32;
    let mut energy_count = 0usize;
    let mut frames_read = 0usize;

    while frames_read < analysis_frames {
        if frames_read % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        let Some(frame) = read_stereo_frame(&mut source) else {
            break;
        };
        frames_read += 1;
        meter_chunk.extend_from_slice(&frame);
        let mono = (frame[0] + frame[1]) * 0.5;
        envelope_sum += mono.abs();
        envelope_count += 1;
        energy_sum += mono * mono;
        energy_count += 1;

        if envelope_count >= envelope_stride {
            envelope.push(envelope_sum / envelope_count as f32);
            envelope_sum = 0.0;
            envelope_count = 0;
        }
        if energy_count >= energy_stride {
            energy_curve.push((energy_sum / energy_count as f32).sqrt());
            energy_sum = 0.0;
            energy_count = 0;
        }
        if meter_chunk.len() >= meter_chunk_frames * 2 {
            loudness
                .add_frames_f32(&meter_chunk)
                .map_err(|error| anyhow!("EBU R128 analysis failed: {error}"))?;
            meter_chunk.clear();
        }
    }
    if !meter_chunk.is_empty() {
        loudness
            .add_frames_f32(&meter_chunk)
            .map_err(|error| anyhow!("EBU R128 analysis failed: {error}"))?;
    }
    if envelope_count > 0 {
        envelope.push(envelope_sum / envelope_count as f32);
    }
    if energy_count > 0 {
        energy_curve.push((energy_sum / energy_count as f32).sqrt());
    }
    if frames_read < OUTPUT_SAMPLE_RATE as usize * 8 {
        return Err(anyhow!(
            "Track is too short for DJ analysis: {}",
            path.display()
        ));
    }

    let decoded_duration = frames_read as f32 / OUTPUT_SAMPLE_RATE as f32;
    let duration_seconds = declared_duration
        .unwrap_or(decoded_duration)
        .max(decoded_duration);
    let silence_threshold = silence_threshold(&envelope);
    let leading_index =
        first_sustained_level(&envelope, silence_threshold, ANALYSIS_RATE_HZ / 5).unwrap_or(0);
    let trailing_index = last_sustained_level(&envelope, silence_threshold, ANALYSIS_RATE_HZ / 5)
        .unwrap_or_else(|| envelope.len().saturating_sub(1));
    let leading_silence_seconds = leading_index as f32 / ANALYSIS_RATE_HZ as f32;
    let trailing_silence_seconds = if decoded_duration >= duration_seconds - 0.25 {
        ((envelope.len().saturating_sub(1 + trailing_index)) as f32 / ANALYSIS_RATE_HZ as f32)
            .min(duration_seconds)
    } else {
        0.0
    };

    let mut onset = Vec::with_capacity(envelope.len());
    let mut previous = envelope.first().copied().unwrap_or(0.0);
    for value in &envelope {
        onset.push((value - previous).max(0.0));
        previous = *value;
    }
    remove_local_mean(&mut onset, ANALYSIS_RATE_HZ / 2);
    let (bpm, beat_lag, bpm_confidence) = estimate_bpm(&onset);
    let beat_phase = estimate_beat_phase(&onset, beat_lag);
    let downbeat_phase = estimate_downbeat_phase(&onset, beat_phase, beat_lag);
    let beat_offset_seconds = beat_phase as f32 / ANALYSIS_RATE_HZ as f32;
    let downbeat_offset_seconds = downbeat_phase as f32 / ANALYSIS_RATE_HZ as f32;
    let sections = detect_sections(
        &energy_curve,
        duration_seconds,
        leading_silence_seconds,
        trailing_silence_seconds,
    );

    Ok(TrackAnalysis {
        bpm,
        bpm_confidence,
        beat_offset_seconds,
        downbeat_offset_seconds,
        leading_silence_seconds,
        trailing_silence_seconds,
        integrated_lufs: loudness
            .loudness_global()
            .ok()
            .filter(|value| value.is_finite()),
        sample_peak: maximum_peak(&loudness, false).unwrap_or(0.0),
        true_peak: maximum_peak(&loudness, true).unwrap_or(0.0),
        duration_seconds,
        energy_curve,
        sections,
        musical_key: None,
        key_confidence: None,
        vocal_profile: AnalysisAvailability::Unavailable {
            reason: "No approved vocal/stem model is bundled.".to_owned(),
        },
    })
}

fn silence_threshold(envelope: &[f32]) -> f32 {
    if envelope.is_empty() {
        return 0.0005;
    }
    let mut sorted = envelope.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let floor = percentile(&sorted, 0.10);
    let body = percentile(&sorted, 0.75);
    (floor * 2.5).max(body * 0.04).max(0.0005)
}

fn estimate_bpm(onset: &[f32]) -> (f32, usize, f32) {
    if onset.len() < ANALYSIS_RATE_HZ * 8 {
        let lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / DEFAULT_BPM).round() as usize;
        return (DEFAULT_BPM, lag, 0.0);
    }
    let min_lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / MAX_BPM).floor() as usize;
    let max_lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / MIN_BPM).ceil() as usize;
    let mut best_lag = min_lag.max(1);
    let mut best_score = 0.0f32;
    let mut second_score = 0.0f32;
    for lag in min_lag.max(1)..=max_lag.min(onset.len().saturating_sub(1)) {
        let mut score = 0.0f32;
        for index in lag..onset.len() {
            score += onset[index] * onset[index - lag];
        }
        score /= (onset.len() - lag) as f32;
        if score > best_score {
            second_score = best_score;
            best_score = score;
            best_lag = lag;
        } else if score > second_score {
            second_score = score;
        }
    }
    let bpm = (ANALYSIS_RATE_HZ as f32 * 60.0 / best_lag as f32).clamp(MIN_BPM, MAX_BPM);
    let confidence = if best_score > 0.0 {
        ((best_score - second_score) / best_score).clamp(0.0, 1.0)
    } else {
        0.0
    };
    (bpm, best_lag, confidence)
}

fn estimate_beat_phase(onset: &[f32], lag: usize) -> usize {
    if onset.is_empty() || lag == 0 {
        return 0;
    }
    (0..lag.min(onset.len()))
        .max_by(|left, right| {
            let score = |phase: usize| onset.iter().skip(phase).step_by(lag).copied().sum::<f32>();
            score(*left)
                .partial_cmp(&score(*right))
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or(0)
}

fn estimate_downbeat_phase(onset: &[f32], beat_phase: usize, beat_lag: usize) -> usize {
    if onset.is_empty() || beat_lag == 0 {
        return beat_phase;
    }
    let bar_lag = beat_lag.saturating_mul(4);
    (0..4)
        .map(|beat_in_bar| beat_phase + beat_in_bar * beat_lag)
        .filter(|phase| *phase < onset.len())
        .max_by(|left, right| {
            let score = |phase: usize| {
                onset
                    .iter()
                    .skip(phase)
                    .step_by(bar_lag.max(1))
                    .copied()
                    .sum::<f32>()
            };
            score(*left)
                .partial_cmp(&score(*right))
                .unwrap_or(Ordering::Equal)
        })
        .unwrap_or(beat_phase)
}

fn detect_sections(
    energy_curve: &[f32],
    duration_seconds: f32,
    leading_silence_seconds: f32,
    trailing_silence_seconds: f32,
) -> Vec<DetectedSection> {
    let playable_start = leading_silence_seconds.min(duration_seconds);
    let playable_end = (duration_seconds - trailing_silence_seconds)
        .max(playable_start)
        .min(duration_seconds);
    if playable_end - playable_start < 8.0 || energy_curve.is_empty() {
        return vec![DetectedSection {
            kind: SectionKind::Main,
            start_seconds: playable_start,
            end_seconds: playable_end,
            energy: 0.0,
            confidence: 0.25,
        }];
    }

    let mut sorted = energy_curve.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal));
    let low = percentile(&sorted, 0.30);
    let high = percentile(&sorted, 0.75);
    let intro_end = (playable_start + 24.0).min(playable_end);
    let outro_start = (playable_end - 24.0).max(playable_start);
    let body_start = intro_end;
    let body_end = outro_start.max(body_start);
    let body_start_index = seconds_to_energy_index(body_start).min(energy_curve.len());
    let body_end_index = seconds_to_energy_index(body_end).min(energy_curve.len());
    let body = &energy_curve[body_start_index..body_end_index.max(body_start_index)];
    let peak_index = body
        .iter()
        .enumerate()
        .max_by(|(_, left), (_, right)| left.partial_cmp(right).unwrap_or(Ordering::Equal))
        .map(|(index, _)| body_start_index + index)
        .unwrap_or(body_start_index);
    let peak_seconds = peak_index as f32 / ENERGY_RATE_HZ as f32;
    let drop_start = (peak_seconds - 8.0).clamp(body_start, body_end);
    let drop_end = (drop_start + 32.0).min(body_end);
    let breakdown_start = (drop_start - 32.0).max(body_start);
    let build_start = (drop_start - 16.0).max(breakdown_start);
    let chorus_start = (drop_end + 8.0).min(body_end);
    let chorus_end = (chorus_start + 32.0).min(body_end);

    vec![
        DetectedSection {
            kind: SectionKind::Intro,
            start_seconds: playable_start,
            end_seconds: intro_end,
            energy: average_energy_range(
                energy_curve,
                seconds_to_energy_index(playable_start),
                seconds_to_energy_index(intro_end),
            ),
            confidence: 0.55,
        },
        DetectedSection {
            kind: SectionKind::Breakdown,
            start_seconds: breakdown_start,
            end_seconds: build_start,
            energy: low,
            confidence: 0.40,
        },
        DetectedSection {
            kind: SectionKind::BuildUp,
            start_seconds: build_start,
            end_seconds: drop_start,
            energy: average_energy_range(
                energy_curve,
                seconds_to_energy_index(build_start),
                seconds_to_energy_index(drop_start),
            ),
            confidence: 0.45,
        },
        DetectedSection {
            kind: SectionKind::Drop,
            start_seconds: drop_start,
            end_seconds: drop_end,
            energy: high,
            confidence: 0.65,
        },
        DetectedSection {
            kind: SectionKind::Chorus,
            start_seconds: chorus_start,
            end_seconds: chorus_end,
            energy: average_energy_range(
                energy_curve,
                seconds_to_energy_index(chorus_start),
                seconds_to_energy_index(chorus_end),
            ),
            confidence: 0.45,
        },
        DetectedSection {
            kind: SectionKind::Main,
            start_seconds: body_start,
            end_seconds: body_end,
            energy: average_energy_range(energy_curve, body_start_index, body_end_index),
            confidence: 0.60,
        },
        DetectedSection {
            kind: SectionKind::Outro,
            start_seconds: outro_start,
            end_seconds: playable_end,
            energy: average_energy_range(
                energy_curve,
                seconds_to_energy_index(outro_start),
                energy_curve.len(),
            ),
            confidence: 0.55,
        },
    ]
}

fn plan_tracks(
    mut analyzed: Vec<(DjMixTrack, TrackAnalysis)>,
    options: DjMixOptions,
) -> Result<Vec<PlannedTrack>> {
    if options.smart_order && analyzed.len() > 2 {
        analyzed = smart_order(analyzed);
    }
    let requested_total = options.target_minutes * 60.0;
    let transition_seconds = estimated_transition_seconds(&analyzed, options.transition_bars);
    let per_track_target = ((requested_total + transition_seconds * (analyzed.len() - 1) as f32)
        / analyzed.len() as f32)
        .clamp(MIN_SECTION_SECONDS, MAX_AUTO_SECTION_SECONDS);
    let mut previous_effective_bpm = analyzed
        .first()
        .map(|(_, analysis)| analysis.bpm)
        .filter(|bpm| bpm.is_finite() && *bpm > 0.0)
        .unwrap_or(DEFAULT_BPM);
    let mut planned = Vec::with_capacity(analyzed.len());

    for (index, (track, analysis)) in analyzed.into_iter().enumerate() {
        let speed_ratio = match options.style {
            DjMixStyle::Crossfade => 1.0,
            DjMixStyle::SmartDj if index == 0 || analysis.bpm <= 0.0 => 1.0,
            DjMixStyle::SmartDj => (previous_effective_bpm / analysis.bpm)
                .clamp(1.0 - MAX_SMART_TEMPO_CHANGE, 1.0 + MAX_SMART_TEMPO_CHANGE),
        };
        previous_effective_bpm = (analysis.bpm * speed_ratio).clamp(MIN_BPM, MAX_BPM);
        let (section_start_seconds, section_end_seconds, section_reason) =
            select_track_section(&track, &analysis, per_track_target, options.transition_bars)?;
        let gain = planned_track_gain(&analysis, options.normalize_loudness);
        planned.push(PlannedTrack {
            track,
            analysis,
            speed_ratio,
            gain,
            section_start_seconds,
            section_end_seconds,
            section_reason,
        });
    }
    Ok(planned)
}

fn select_track_section(
    track: &DjMixTrack,
    analysis: &TrackAnalysis,
    target_seconds: f32,
    phrase_bars: u32,
) -> Result<(f32, f32, String)> {
    let playable_start = analysis
        .leading_silence_seconds
        .min(analysis.duration_seconds);
    let playable_end = (analysis.duration_seconds - analysis.trailing_silence_seconds)
        .max(playable_start + 1.0)
        .min(analysis.duration_seconds);
    let phrase = phrase_seconds(analysis.bpm, phrase_bars);
    match track.section_mode {
        DjTrackSectionMode::FullTrack => Ok((
            playable_start,
            playable_end,
            "Full track selected; leading and trailing silence trimmed.".to_owned(),
        )),
        DjTrackSectionMode::FavoriteRange => {
            let start = track.favorite_start_seconds.max(playable_start);
            let end = track.favorite_end_seconds.min(playable_end);
            if end - start < MIN_SECTION_SECONDS * 0.5 {
                return Err(anyhow!(
                    "Favorite range for '{}' must be at least {:.0} seconds and inside track duration.",
                    track.title,
                    MIN_SECTION_SECONDS * 0.5
                ));
            }
            let aligned_start =
                align_to_phrase(start, analysis.downbeat_offset_seconds, phrase, true);
            let aligned_end = align_to_phrase(end, analysis.downbeat_offset_seconds, phrase, false)
                .max(aligned_start + phrase.min(end - aligned_start));
            let safe_start = aligned_start
                .min((playable_end - 1.0).max(playable_start))
                .max(playable_start);
            let safe_end = aligned_end.max(safe_start + 1.0).min(playable_end);
            Ok((
                safe_start,
                safe_end,
                "User favorite range, snapped to phrase boundaries.".to_owned(),
            ))
        }
        DjTrackSectionMode::AutoHighlight => {
            let playable_length = playable_end - playable_start;
            let desired = target_seconds
                .min(playable_length)
                .max(MIN_SECTION_SECONDS.min(playable_length));
            let anchor = analysis
                .sections
                .iter()
                .filter(|section| {
                    matches!(
                        section.kind,
                        SectionKind::Drop | SectionKind::Chorus | SectionKind::Main
                    )
                })
                .max_by(|left, right| {
                    (left.energy * left.confidence)
                        .partial_cmp(&(right.energy * right.confidence))
                        .unwrap_or(Ordering::Equal)
                });
            let center = anchor
                .map(|section| (section.start_seconds + section.end_seconds) * 0.5)
                .unwrap_or((playable_start + playable_end) * 0.5);
            let raw_start = (center - desired * 0.42)
                .clamp(playable_start, (playable_end - desired).max(playable_start));
            let aligned_start =
                align_to_phrase(raw_start, analysis.downbeat_offset_seconds, phrase, true);
            let safe_start = aligned_start
                .min((playable_end - 1.0).max(playable_start))
                .max(playable_start);
            let raw_end = (safe_start + desired).min(playable_end);
            let aligned_end =
                align_to_phrase(raw_end, analysis.downbeat_offset_seconds, phrase, false);
            let safe_end = aligned_end.max(safe_start + 1.0).min(playable_end);
            let label = anchor
                .map(|section| {
                    format!(
                        "Auto highlight around {:?}; deterministic energy heuristic and phrase alignment.",
                        section.kind
                    )
                })
                .unwrap_or_else(|| {
                    "Auto highlight around track midpoint; phrase-aligned.".to_owned()
                });
            Ok((safe_start, safe_end, label))
        }
    }
}

fn planned_track_gain(analysis: &TrackAnalysis, normalize: bool) -> f32 {
    let loudness_gain = if normalize {
        analysis
            .integrated_lufs
            .filter(|value| value.is_finite())
            .map(|value| db_to_gain((TARGET_LUFS - value).clamp(-8.0, 8.0) as f32))
            .unwrap_or(1.0)
    } else {
        1.0
    };
    let peak = analysis.true_peak.max(analysis.sample_peak) as f32;
    let peak_gain = if peak > 0.0 {
        (OUTPUT_PEAK_LIMIT / peak).min(1.0)
    } else {
        1.0
    };
    loudness_gain.min(peak_gain).clamp(0.25, 2.0)
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
                compatibility_cost(previous_bpm, &left.1)
                    .partial_cmp(&compatibility_cost(previous_bpm, &right.1))
                    .unwrap_or(Ordering::Equal)
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        ordered.push(tracks.remove(next_index));
    }
    ordered
}

fn compatibility_cost(previous_bpm: f32, analysis: &TrackAnalysis) -> f32 {
    let bpm_cost = (analysis.bpm - previous_bpm).abs();
    let confidence_penalty = (1.0 - analysis.bpm_confidence) * 8.0;
    bpm_cost + confidence_penalty
}

fn plan_transition_diagnostics(
    tracks: &[PlannedTrack],
    options: DjMixOptions,
) -> Vec<TransitionDiagnostic> {
    let transition_frames = planned_transition_frame_counts(tracks, options);
    tracks
        .windows(2)
        .enumerate()
        .map(|(index, pair)| {
            let current = &pair[0];
            let next = &pair[1];
            let effective_bpm = ((current.analysis.bpm * current.speed_ratio
                + next.analysis.bpm * next.speed_ratio)
                * 0.5)
                .clamp(MIN_BPM, MAX_BPM);
            let requested_frames =
                transition_frame_count(current, next, options.transition_bars);
            let actual_frames = transition_frames[index];
            let duration_seconds = actual_frames as f32 / OUTPUT_SAMPLE_RATE as f32;
            let mut decisions = vec![
                format!(
                    "Aligned to {}-bar phrase boundary.",
                    options.transition_bars
                ),
                format!(
                    "Tempo target {:.2} BPM; pitch preserved by built-in WSOLA.",
                    effective_bpm
                ),
                "Composed instrumental bridge selected instead of a long incoming-track fade.".to_owned(),
            ];
            if actual_frames < requested_frames {
                decisions.push(format!(
                    "Transition shortened from {:.2} to {:.2} seconds to fit selected sections.",
                    requested_frames as f32 / OUTPUT_SAMPLE_RATE as f32,
                    duration_seconds
                ));
            }
            if options.bass_swap {
                decisions.push("Bass swap at transition midpoint.".to_owned());
            }
            decisions.push("Original beat, percussion, pluck, bass, and string bridge generated for the handoff.".to_owned());
            let mut warnings = Vec::new();
            if current.analysis.musical_key.is_none() || next.analysis.musical_key.is_none() {
                warnings.push(
                    "Harmonic compatibility unavailable: no approved key analyzer bundled."
                        .to_owned(),
                );
            }
            decisions.push(
                "Center-channel vocal guard selected: one lead vocal is prioritized at a time."
                    .to_owned(),
            );
            warnings.push(
                "Vocal guard uses deterministic center-channel activity detection; isolated stems are not bundled."
                    .to_owned(),
            );
            if current.analysis.bpm_confidence < 0.15 || next.analysis.bpm_confidence < 0.15 {
                warnings.push("Low BPM confidence; transition may need manual review.".to_owned());
            }
            TransitionDiagnostic {
                from_title: current.track.title.clone(),
                to_title: next.track.title.clone(),
                style: if transition_should_hit_hard(current, next) {
                    "composed_bridge_drop".to_owned()
                } else {
                    "composed_bridge_handoff".to_owned()
                },
                phrase_bars: options.transition_bars,
                duration_seconds,
                effective_bpm,
                beat_alignment_error_ms: 1_000.0 / OUTPUT_SAMPLE_RATE as f32,
                peak_reduction_db: transition_peak_reduction_db(current, next),
                decisions,
                warnings,
            }
        })
        .collect()
}

fn analysis_availability_label(availability: &AnalysisAvailability) -> String {
    match availability {
        AnalysisAvailability::Unavailable { reason } => format!("unavailable: {reason}"),
    }
}

fn build_report(
    output_path: &Path,
    tracks: &[PlannedTrack],
    transitions: Vec<TransitionDiagnostic>,
    options: DjMixOptions,
    measurement: OutputMeasurement,
    timings: RenderTimings,
) -> MixReport {
    let planned_duration_seconds = tracks
        .iter()
        .map(|track| {
            (track.section_end_seconds - track.section_start_seconds) / track.speed_ratio.max(0.001)
        })
        .sum::<f32>()
        - transitions
            .iter()
            .map(|transition| transition.duration_seconds)
            .sum::<f32>();
    let track_diagnostics = tracks
        .iter()
        .map(|track| TrackDiagnostic {
            title: track.track.title.clone(),
            path: track.track.path.display().to_string(),
            bpm: track.analysis.bpm,
            bpm_confidence: track.analysis.bpm_confidence,
            beat_offset_seconds: track.analysis.beat_offset_seconds,
            downbeat_offset_seconds: track.analysis.downbeat_offset_seconds,
            integrated_lufs: track.analysis.integrated_lufs,
            true_peak_dbfs: amplitude_to_db(track.analysis.true_peak),
            musical_key: track.analysis.musical_key.clone(),
            key_confidence: track.analysis.key_confidence,
            vocal_analysis: analysis_availability_label(&track.analysis.vocal_profile),
            selected_section: format!("{:?}", track.track.section_mode),
            section_start_seconds: track.section_start_seconds,
            section_end_seconds: track.section_end_seconds,
            section_reason: track.section_reason.clone(),
            speed_ratio: track.speed_ratio,
            gain_db: gain_to_db(track.gain),
        })
        .collect();
    MixReport {
        schema_version: ENGINE_SCHEMA_VERSION,
        engine_version: ENGINE_VERSION.to_owned(),
        analyzer_version: ANALYZER_VERSION.to_owned(),
        output_path: output_path.display().to_string(),
        requested_target_minutes: options.target_minutes,
        planned_duration_seconds: planned_duration_seconds.max(0.0),
        rendered_duration_seconds: measurement.duration_seconds,
        integrated_lufs: measurement.integrated_lufs,
        sample_peak_dbfs: measurement.sample_peak_dbfs,
        true_peak_dbfs: measurement.true_peak_dbfs,
        timings_ms: timings,
        tracks: track_diagnostics,
        transitions,
        warnings: vec![
            "Section labels are deterministic energy heuristics, not ML semantic classification."
                .to_owned(),
            "Key, vocal, genre, and stem analysis remain disabled until dependency/model licensing and packaging are approved."
                .to_owned(),
            "Built-in WSOLA time stretching and transition planning are deterministic."
                .to_owned(),
        ],
    }
}

fn transition_frame_count(current: &PlannedTrack, next: &PlannedTrack, bars: u32) -> usize {
    let current_bpm = (current.analysis.bpm * current.speed_ratio).clamp(MIN_BPM, MAX_BPM);
    let next_bpm = (next.analysis.bpm * next.speed_ratio).clamp(MIN_BPM, MAX_BPM);
    let mix_bpm = ((current_bpm + next_bpm) * 0.5).clamp(MIN_BPM, MAX_BPM);
    let seconds = phrase_seconds(mix_bpm, bars);
    (seconds * OUTPUT_SAMPLE_RATE as f32).round().max(1.0) as usize
}

fn planned_output_section_frame_count(track: &PlannedTrack, style: DjMixStyle) -> usize {
    let source_frames = section_frame_count(track);
    match style {
        DjMixStyle::Crossfade => source_frames,
        DjMixStyle::SmartDj => {
            ((source_frames as f32 / track.speed_ratio.max(0.001)).round() as usize).max(1)
        }
    }
}

fn planned_transition_frame_counts(tracks: &[PlannedTrack], options: DjMixOptions) -> Vec<usize> {
    if tracks.len() < 2 {
        return Vec::new();
    }

    let section_frames = tracks
        .iter()
        .map(|track| planned_output_section_frame_count(track, options.style))
        .collect::<Vec<_>>();
    let requested = tracks
        .windows(2)
        .map(|pair| transition_frame_count(&pair[0], &pair[1], options.transition_bars))
        .collect::<Vec<_>>();
    allocate_transition_frame_counts(&section_frames, &requested)
}

fn allocate_transition_frame_counts(section_frames: &[usize], requested: &[usize]) -> Vec<usize> {
    if section_frames.len() < 2 || requested.is_empty() {
        return Vec::new();
    }

    debug_assert_eq!(requested.len(), section_frames.len() - 1);
    let pair_count = requested.len().min(section_frames.len().saturating_sub(1));
    let mut remaining = section_frames.to_vec();
    let mut overlaps = Vec::with_capacity(pair_count);

    for index in 0..pair_count {
        let following_requested = requested.get(index + 1).copied().unwrap_or(0);
        let next_budget = incoming_transition_budget(
            section_frames[index + 1],
            requested[index],
            following_requested,
        );
        let overlap = requested[index].min(remaining[index]).min(next_budget);
        overlaps.push(overlap);
        remaining[index] = remaining[index].saturating_sub(overlap);
        remaining[index + 1] = section_frames[index + 1].saturating_sub(overlap);
    }

    overlaps
}

fn incoming_transition_budget(
    track_frames: usize,
    incoming_requested: usize,
    outgoing_requested: usize,
) -> usize {
    if track_frames == 0 || incoming_requested == 0 {
        return 0;
    }
    if outgoing_requested == 0 || track_frames == 1 {
        return track_frames.min(incoming_requested);
    }

    let total_requested = incoming_requested as u128 + outgoing_requested as u128;
    let proportional =
        (track_frames as u128 * incoming_requested as u128 / total_requested) as usize;
    proportional
        .clamp(1, track_frames - 1)
        .min(incoming_requested)
}

fn estimated_transition_seconds(tracks: &[(DjMixTrack, TrackAnalysis)], bars: u32) -> f32 {
    if tracks.len() < 2 {
        return 0.0;
    }
    let average_bpm =
        tracks.iter().map(|(_, analysis)| analysis.bpm).sum::<f32>() / tracks.len() as f32;
    phrase_seconds(average_bpm.clamp(MIN_BPM, MAX_BPM), bars)
}

fn phrase_seconds(bpm: f32, bars: u32) -> f32 {
    bars.max(1) as f32 * 4.0 * 60.0 / bpm.clamp(MIN_BPM, MAX_BPM)
}

fn align_to_phrase(value: f32, origin: f32, phrase_seconds: f32, forward: bool) -> f32 {
    if phrase_seconds <= 0.0 || !phrase_seconds.is_finite() {
        return value.max(0.0);
    }
    let relative = (value - origin) / phrase_seconds;
    let phrase = if forward {
        relative.ceil()
    } else {
        relative.floor()
    };
    (origin + phrase * phrase_seconds).max(0.0)
}

fn section_frame_count(track: &PlannedTrack) -> usize {
    ((track.section_end_seconds - track.section_start_seconds).max(0.0) * OUTPUT_SAMPLE_RATE as f32)
        .round() as usize
}

fn transition_peak_scale(
    outgoing: &[[f32; 2]],
    incoming: &[[f32; 2]],
    outgoing_gain: f32,
    incoming_gain: f32,
) -> f32 {
    let frames = outgoing.len().min(incoming.len());
    let mut peak = 0.0f32;
    for index in 0..frames {
        let progress = normalized_progress(index, frames);
        let out_fade = ((1.0 - progress) * std::f32::consts::FRAC_PI_2).sin();
        let in_fade = (progress * std::f32::consts::FRAC_PI_2).sin();
        for channel in 0..2 {
            let sample = outgoing[index][channel] * outgoing_gain * out_fade
                + incoming[index][channel] * incoming_gain * in_fade;
            peak = peak.max(sample.abs());
        }
    }
    if peak > OUTPUT_PEAK_LIMIT {
        OUTPUT_PEAK_LIMIT / peak
    } else {
        1.0
    }
}

fn transition_peak_reduction_db(current: &PlannedTrack, next: &PlannedTrack) -> f32 {
    let estimated = current.analysis.true_peak as f32 * current.gain
        + next.analysis.true_peak as f32 * next.gain;
    if estimated > OUTPUT_PEAK_LIMIT {
        gain_to_db(OUTPUT_PEAK_LIMIT / estimated)
    } else {
        0.0
    }
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

fn last_sustained_level(values: &[f32], threshold: f32, count: usize) -> Option<usize> {
    if count == 0 || values.len() < count {
        return None;
    }
    values
        .windows(count)
        .rposition(|window| window.iter().all(|value| *value >= threshold))
        .map(|index| index + count - 1)
}

fn percentile(sorted: &[f32], percentile: f32) -> f32 {
    if sorted.is_empty() {
        return 0.0;
    }
    let index = ((sorted.len() - 1) as f32 * percentile.clamp(0.0, 1.0)).round() as usize;
    sorted[index]
}

fn average_energy_range(values: &[f32], start: usize, end: usize) -> f32 {
    let start = start.min(values.len());
    let end = end.min(values.len()).max(start);
    if end <= start {
        return 0.0;
    }
    values[start..end].iter().sum::<f32>() / (end - start) as f32
}

fn seconds_to_energy_index(seconds: f32) -> usize {
    (seconds.max(0.0) * ENERGY_RATE_HZ as f32).round() as usize
}

fn load_analysis_cache() -> AnalysisCache {
    let Some(path) = analysis_cache_path() else {
        return AnalysisCache::default();
    };
    fs::read(path)
        .ok()
        .and_then(|bytes| serde_json::from_slice::<AnalysisCache>(&bytes).ok())
        .filter(|cache| cache.schema_version == ENGINE_SCHEMA_VERSION)
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

fn maximum_peak(analyzer: &EbuR128, true_peak: bool) -> Option<f64> {
    (0..OUTPUT_CHANNELS as u32)
        .filter_map(|channel| {
            if true_peak {
                analyzer.true_peak(channel).ok()
            } else {
                analyzer.sample_peak(channel).ok()
            }
        })
        .filter(|value| value.is_finite())
        .max_by(|left, right| left.partial_cmp(right).unwrap_or(Ordering::Equal))
}

fn maximum_peak_dbfs(analyzer: &EbuR128, true_peak: bool) -> Option<f64> {
    maximum_peak(analyzer, true_peak).and_then(amplitude_to_db)
}

fn amplitude_to_db(value: f64) -> Option<f64> {
    if value > 0.0 && value.is_finite() {
        Some(20.0 * value.log10())
    } else {
        None
    }
}

fn read_stereo_frame<I>(source: &mut I) -> Option<[f32; 2]>
where
    I: Iterator<Item = f32>,
{
    let left = source.next()?;
    let right = source.next().unwrap_or(left);
    Some([left, right])
}

fn normalized_progress(index: usize, frames: usize) -> f32 {
    if frames <= 1 {
        1.0
    } else {
        index as f32 / (frames - 1) as f32
    }
}

fn multiply_frame(frame: [f32; 2], gain: f32) -> [f32; 2] {
    [frame[0] * gain, frame[1] * gain]
}

fn limit_frame(frame: [f32; 2]) -> [f32; 2] {
    let peak = frame[0].abs().max(frame[1].abs());
    if peak > OUTPUT_PEAK_LIMIT {
        let gain = OUTPUT_PEAK_LIMIT / peak;
        [frame[0] * gain, frame[1] * gain]
    } else {
        frame
    }
}

fn db_to_gain(db: f32) -> f32 {
    10.0f32.powf(db / 20.0)
}

fn gain_to_db(gain: f32) -> f32 {
    if gain > 0.0 {
        20.0 * gain.log10()
    } else {
        f32::NEG_INFINITY
    }
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

fn report_output_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("dj-mix.mp3");
    path.with_file_name(format!("{file_name}.dj-plan.json"))
}

fn temporary_output_path(path: &Path) -> PathBuf {
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("dj-mix.mp3");
    path.with_file_name(format!("{file_name}.part"))
}

fn cleanup_temporary_files(audio: &Path, report: &Path) {
    let _ = fs::remove_file(audio);
    let _ = fs::remove_file(report);
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
    fn estimates_known_bpm_and_downbeat_from_impulses() {
        let bpm = 120.0;
        let lag = (ANALYSIS_RATE_HZ as f32 * 60.0 / bpm) as usize;
        let mut onset = vec![0.0; ANALYSIS_RATE_HZ * 30];
        for (beat, index) in (7..onset.len()).step_by(lag).enumerate() {
            onset[index] = if beat % 4 == 0 { 2.0 } else { 1.0 };
        }
        let (estimated, estimated_lag, _) = estimate_bpm(&onset);
        let beat_phase = estimate_beat_phase(&onset, estimated_lag);
        let downbeat = estimate_downbeat_phase(&onset, beat_phase, estimated_lag);
        assert!((estimated - bpm).abs() < 1.0, "estimated {estimated}");
        assert_eq!(downbeat.abs_diff(7) % (estimated_lag * 4), 0);
    }

    #[test]
    fn analyzes_generated_wav_fixture() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "audio-orbit-generated-dj-fixture-{}-{unique}.wav",
            std::process::id()
        ));
        write_generated_click_fixture(&path, 120.0, 12.0).expect("write fixture");
        let cancel = AtomicBool::new(false);
        let analysis = analyze_track(&path, &cancel).expect("analyze fixture");
        let _ = fs::remove_file(path);
        assert!(
            (analysis.bpm - 120.0).abs() < 2.0,
            "estimated {}",
            analysis.bpm
        );
        assert!(analysis.integrated_lufs.is_some());
        assert!(analysis.true_peak > 0.0);
        assert!(!analysis.sections.is_empty());
    }

    #[test]
    fn planner_is_deterministic_and_phrase_aligned() {
        let options = DjMixOptions::default();
        let input = vec![
            test_analysis_track("first", 120.0),
            test_analysis_track("second", 124.0),
        ];
        let first = plan_tracks(input.clone(), options).expect("first plan");
        let second = plan_tracks(input, options).expect("second plan");
        assert_eq!(first.len(), second.len());
        for (left, right) in first.iter().zip(second.iter()) {
            assert_eq!(left.track.title, right.track.title);
            assert!(
                (left.section_start_seconds - right.section_start_seconds).abs() < f32::EPSILON
            );
            let phrase = phrase_seconds(left.analysis.bpm, options.transition_bars);
            let relative =
                (left.section_start_seconds - left.analysis.downbeat_offset_seconds) / phrase;
            assert!(
                (relative - relative.round()).abs() < 0.001
                    || left.section_start_seconds <= left.analysis.leading_silence_seconds + 0.001
            );
        }
    }

    #[test]
    fn favorite_range_rejects_invalid_selection() {
        let mut track = DjMixTrack::new(PathBuf::from("favorite"), "favorite".to_owned());
        track.section_mode = DjTrackSectionMode::FavoriteRange;
        track.favorite_start_seconds = 50.0;
        track.favorite_end_seconds = 55.0;
        let analysis = test_analysis(120.0);
        let result = select_track_section(&track, &analysis, 60.0, 16);
        assert!(result.is_err());
    }

    #[test]
    fn smart_order_keeps_first_track_and_clusters_bpm() {
        let ordered = smart_order(vec![
            test_analysis_track("first", 120.0),
            test_analysis_track("far", 150.0),
            test_analysis_track("near", 124.0),
        ]);
        assert_eq!(ordered[0].0.title, "first");
        assert_eq!(ordered[1].0.title, "near");
    }

    #[test]
    fn crossfade_mode_keeps_original_playback_speed() {
        let analyzed = vec![
            test_analysis_track("first", 100.0),
            test_analysis_track("second", 140.0),
        ];
        let mut options = DjMixOptions::default();
        options.style = DjMixStyle::Crossfade;
        options.smart_order = false;
        let planned = plan_tracks(analyzed, options).expect("plan");
        assert!(planned
            .iter()
            .all(|track| (track.speed_ratio - 1.0).abs() < f32::EPSILON));
    }

    #[test]
    fn smart_dj_mode_caps_pairwise_tempo_sync() {
        let analyzed = vec![
            test_analysis_track("first", 100.0),
            test_analysis_track("second", 140.0),
        ];
        let mut options = DjMixOptions::default();
        options.style = DjMixStyle::SmartDj;
        options.smart_order = false;
        let planned = plan_tracks(analyzed, options).expect("plan");
        assert!((planned[0].speed_ratio - 1.0).abs() < f32::EPSILON);
        assert!((planned[1].speed_ratio - (1.0 - MAX_SMART_TEMPO_CHANGE)).abs() < 0.0001);
    }

    #[test]
    fn adaptive_transitions_preserve_middle_track_for_both_sides() {
        let overlaps = allocate_transition_frame_counts(&[100, 100, 100], &[100, 100]);
        assert_eq!(overlaps, vec![50, 50]);
        assert_eq!(overlaps[0] + overlaps[1], 100);
    }

    #[test]
    fn adaptive_transitions_shorten_to_available_sections() {
        let overlaps = allocate_transition_frame_counts(&[20, 30, 40], &[100, 100]);
        assert_eq!(overlaps, vec![15, 15]);
        assert!(overlaps[0] <= 20);
        assert!(overlaps[0] + overlaps[1] <= 30);
        assert!(overlaps[1] <= 40);
    }

    #[test]
    fn final_transition_can_use_remaining_last_track() {
        let overlaps = allocate_transition_frame_counts(&[20, 50], &[100]);
        assert_eq!(overlaps, vec![20]);
    }

    #[test]
    fn transition_peak_scale_prevents_overflow() {
        let outgoing = vec![[0.9, 0.9]; 100];
        let incoming = vec![[0.9, 0.9]; 100];
        let scale = transition_peak_scale(&outgoing, &incoming, 1.0, 1.0);
        assert!(scale < 1.0);
        for index in 0..100 {
            let progress = normalized_progress(index, 100);
            let out_fade = ((1.0 - progress) * std::f32::consts::FRAC_PI_2).sin();
            let in_fade = (progress * std::f32::consts::FRAC_PI_2).sin();
            let mixed = 0.9 * scale * out_fade + 0.9 * scale * in_fade;
            assert!(mixed <= OUTPUT_PEAK_LIMIT + 0.001);
        }
    }

    #[test]
    fn mp3_and_report_paths_are_deterministic() {
        let output = ensure_mp3_extension(Path::new("mix"));
        assert_eq!(output, PathBuf::from("mix.mp3"));
        assert_eq!(
            report_output_path(&output),
            PathBuf::from("mix.mp3.dj-plan.json")
        );
    }

    #[test]
    fn mp3_stream_writer_creates_output_and_measurement() {
        let unique = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let path = std::env::temp_dir().join(format!(
            "audio-orbit-dj-writer-{}-{unique}.mp3",
            std::process::id()
        ));
        let mut writer = Mp3StreamWriter::create(&path, 192).expect("create MP3 writer");
        for index in 0..OUTPUT_SAMPLE_RATE {
            let phase = index as f32 / OUTPUT_SAMPLE_RATE as f32 * 440.0 * std::f32::consts::TAU;
            let sample = phase.sin() * 0.2;
            writer.write_frame([sample, sample]).expect("encode frame");
        }
        let measurement = writer.finish().expect("finish MP3");
        let bytes = fs::read(&path).expect("read MP3");
        let _ = fs::remove_file(path);
        assert!(bytes.len() > 1_000);
        assert!(measurement.integrated_lufs.is_some());
        assert!(measurement.true_peak_dbfs.is_some());
    }

    fn test_analysis_track(title: &str, bpm: f32) -> (DjMixTrack, TrackAnalysis) {
        (
            DjMixTrack::new(PathBuf::from(title), title.to_owned()),
            test_analysis(bpm),
        )
    }

    fn test_analysis(bpm: f32) -> TrackAnalysis {
        TrackAnalysis {
            bpm,
            bpm_confidence: 0.8,
            beat_offset_seconds: 0.0,
            downbeat_offset_seconds: 0.0,
            leading_silence_seconds: 0.0,
            trailing_silence_seconds: 0.0,
            integrated_lufs: Some(-14.0),
            sample_peak: 0.5,
            true_peak: 0.55,
            duration_seconds: 180.0,
            energy_curve: vec![0.1; 360],
            sections: vec![DetectedSection {
                kind: SectionKind::Main,
                start_seconds: 16.0,
                end_seconds: 164.0,
                energy: 0.1,
                confidence: 0.7,
            }],
            musical_key: None,
            key_confidence: None,
            vocal_profile: AnalysisAvailability::Unavailable {
                reason: "test".to_owned(),
            },
        }
    }

    fn write_generated_click_fixture(path: &Path, bpm: f32, seconds: f32) -> Result<()> {
        let sample_rate = OUTPUT_SAMPLE_RATE;
        let channels = OUTPUT_CHANNELS;
        let frames = (sample_rate as f32 * seconds) as u32;
        let data_bytes = frames * channels as u32 * 2;
        let byte_rate = sample_rate * channels as u32 * 2;
        let block_align = channels * 2;
        let mut file = BufWriter::new(File::create(path)?);
        file.write_all(b"RIFF")?;
        file.write_all(&(36 + data_bytes).to_le_bytes())?;
        file.write_all(b"WAVEfmt ")?;
        file.write_all(&16u32.to_le_bytes())?;
        file.write_all(&1u16.to_le_bytes())?;
        file.write_all(&channels.to_le_bytes())?;
        file.write_all(&sample_rate.to_le_bytes())?;
        file.write_all(&byte_rate.to_le_bytes())?;
        file.write_all(&block_align.to_le_bytes())?;
        file.write_all(&16u16.to_le_bytes())?;
        file.write_all(b"data")?;
        file.write_all(&data_bytes.to_le_bytes())?;
        let beat_frames = (sample_rate as f32 * 60.0 / bpm).round() as u32;
        for frame in 0..frames {
            let beat_position = frame % beat_frames.max(1);
            let envelope = if beat_position < 500 {
                1.0 - beat_position as f32 / 500.0
            } else {
                0.0
            };
            let tone = (frame as f32 * 0.13).sin() * envelope * 0.65;
            let sample = float_to_i16(tone);
            file.write_all(&sample.to_le_bytes())?;
            file.write_all(&sample.to_le_bytes())?;
        }
        file.flush()?;
        Ok(())
    }
}
