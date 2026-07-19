use crate::{
    app_data_dir, time_stretch::pitch_preserving_stretch, DjBridgeMode, DjMixEvent, DjMixOptions,
    DjMixStyle, DjMixTrack, DjTrackSectionMode,
};
use anyhow::{anyhow, Context, Result};
use ebur128::{EbuR128, Mode};
use rodio::{source::UniformSourceIterator, Decoder, Source};
use serde::{Deserialize, Serialize};
use shine_rs::{Mp3Encoder, Mp3EncoderConfig, StereoMode};
use std::{
    cmp::Ordering,
    collections::BTreeMap,
    env,
    ffi::{OsStr, OsString},
    fs::{self, File},
    hash::{Hash, Hasher},
    io::{BufReader, BufWriter, Write},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering as AtomicOrdering},
        mpsc::Sender,
        Arc,
    },
    thread,
    time::{Duration, Instant, UNIX_EPOCH},
};

const ENGINE_SCHEMA_VERSION: u32 = 3;
const ANALYZER_VERSION: &str = "audio-orbit+optional-essentia-v3";
const ENGINE_VERSION: &str = "human-dj-v7-essentia-rubberband-demucs";
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
struct ExternalCommand {
    program: PathBuf,
    prefix_args: Vec<OsString>,
    display_name: &'static str,
}

impl ExternalCommand {
    fn command(&self) -> Command {
        let mut command = Command::new(&self.program);
        command.args(&self.prefix_args);
        command
    }

    fn label(&self) -> String {
        format!("{} ({})", self.display_name, self.program.display())
    }
}

#[derive(Clone, Debug, Default)]
struct ProfessionalToolchain {
    essentia: Option<ExternalCommand>,
    rubber_band: Option<ExternalCommand>,
    demucs: Option<ExternalCommand>,
}

impl ProfessionalToolchain {
    fn detect(enabled: bool) -> Self {
        if !enabled {
            return Self::default();
        }
        Self {
            essentia: resolve_external_command(
                "AUDIO_ORBIT_ESSENTIA_PATH",
                &[
                    "essentia_streaming_extractor_music.exe",
                    "essentia_streaming_extractor_music",
                ],
                &[],
                "Essentia",
            ),
            rubber_band: resolve_external_command(
                "AUDIO_ORBIT_RUBBERBAND_PATH",
                &[
                    "rubberband-r3.exe",
                    "rubberband.exe",
                    "rubberband-r3",
                    "rubberband",
                ],
                &[],
                "Rubber Band R3",
            ),
            demucs: resolve_demucs_command(),
        }
    }

    fn summary(&self) -> String {
        format!(
            "Professional tools: Essentia {} · Rubber Band {} · Demucs {}",
            availability_mark(self.essentia.is_some()),
            availability_mark(self.rubber_band.is_some()),
            availability_mark(self.demucs.is_some()),
        )
    }

    fn report(&self) -> ProfessionalToolReport {
        ProfessionalToolReport {
            essentia: self
                .essentia
                .as_ref()
                .map(ExternalCommand::label)
                .unwrap_or_else(|| "not available; built-in rhythm analyzer used".to_owned()),
            rubber_band: self
                .rubber_band
                .as_ref()
                .map(ExternalCommand::label)
                .unwrap_or_else(|| "not available; built-in WSOLA used".to_owned()),
            demucs: self
                .demucs
                .as_ref()
                .map(ExternalCommand::label)
                .unwrap_or_else(|| "not available; full-mix fallback used".to_owned()),
        }
    }
}

pub(crate) fn professional_tool_status_summary() -> String {
    ProfessionalToolchain::detect(true).summary()
}

fn availability_mark(available: bool) -> &'static str {
    if available {
        "detected"
    } else {
        "missing"
    }
}

fn resolve_demucs_command() -> Option<ExternalCommand> {
    if let Some(command) = resolve_external_command(
        "AUDIO_ORBIT_DEMUCS_PATH",
        &["demucs.exe", "demucs"],
        &[],
        "Demucs",
    ) {
        return Some(command);
    }
    let python = env::var_os("AUDIO_ORBIT_DEMUCS_PYTHON")
        .and_then(|value| resolve_executable(OsStr::new(&value)))?;
    Some(ExternalCommand {
        program: python,
        prefix_args: vec![OsString::from("-m"), OsString::from("demucs")],
        display_name: "Demucs",
    })
}

fn resolve_external_command(
    env_name: &str,
    candidates: &[&str],
    prefix_args: &[&str],
    display_name: &'static str,
) -> Option<ExternalCommand> {
    let program = env::var_os(env_name)
        .and_then(|value| resolve_executable(OsStr::new(&value)))
        .or_else(|| {
            candidates
                .iter()
                .find_map(|candidate| resolve_executable(OsStr::new(*candidate)))
        })?;
    Some(ExternalCommand {
        program,
        prefix_args: prefix_args
            .iter()
            .map(|value| OsString::from(*value))
            .collect(),
        display_name,
    })
}

fn resolve_executable(candidate: &OsStr) -> Option<PathBuf> {
    let candidate_path = PathBuf::from(candidate);
    if candidate_path.components().count() > 1 || candidate_path.is_absolute() {
        return candidate_path.is_file().then_some(candidate_path);
    }

    let path = env::var_os("PATH")?;
    let extensions = executable_extensions(&candidate_path);
    for directory in env::split_paths(&path) {
        for extension in &extensions {
            let mut file_name = candidate_path.clone();
            if !extension.is_empty() && file_name.extension().is_none() {
                file_name.set_extension(extension.trim_start_matches('.'));
            }
            let full = directory.join(file_name);
            if full.is_file() {
                return Some(full);
            }
        }
    }
    None
}

fn executable_extensions(candidate: &Path) -> Vec<String> {
    if candidate.extension().is_some() {
        return vec![String::new()];
    }
    #[cfg(windows)]
    {
        let mut values = env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".to_owned())
            .split(';')
            .filter(|value| !value.is_empty())
            .map(|value| value.to_ascii_lowercase())
            .collect::<Vec<_>>();
        values.push(String::new());
        values
    }
    #[cfg(not(windows))]
    {
        vec![String::new()]
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ExportRequest {
    pub tracks: Vec<DjMixTrack>,
    pub options: DjMixOptions,
    pub custom_bridge_path: Option<PathBuf>,
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
    beat_positions_seconds: Vec<f32>,
    sections: Vec<DetectedSection>,
    musical_key: Option<String>,
    key_confidence: Option<f32>,
    vocal_profile: AnalysisAvailability,
    analysis_backend: String,
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
    professional_tools: ProfessionalToolReport,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
struct ProfessionalToolReport {
    essentia: String,
    rubber_band: String,
    demucs: String,
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
    analysis_backend: String,
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

    let tools = ProfessionalToolchain::detect(request.options.professional_tools);
    send_progress(sender, tools.summary(), 0.01);

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
        let analysis = cached_or_analyze(track, &mut cache, &tools, cancel)?;
        analyzed.push((track.clone(), analysis));
    }
    save_analysis_cache(&cache);
    timings.analysis = analysis_started.elapsed().as_millis();

    let planning_started = Instant::now();
    let planned = plan_tracks(analyzed, request.options)?;
    let transitions = plan_transition_diagnostics(&planned, request.options, &tools);
    timings.planning = planning_started.elapsed().as_millis();

    let rendering_started = Instant::now();
    let mut writer = Mp3StreamWriter::create(temporary_path, request.options.bitrate_kbps)?;
    let custom_bridge = load_custom_bridge(request, sender, cancel)?;
    render_planned_mix(
        &planned,
        request.options,
        custom_bridge.as_ref(),
        &tools,
        &mut writer,
        sender,
        cancel,
    )?;
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
        tools.report(),
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
    if !(0.25..=60.0).contains(&options.bridge_loop_seconds) {
        return Err(anyhow!(
            "Custom bridge loop length must be between 0.25 and 60 seconds."
        ));
    }
    if !(0.15..=1.0).contains(&options.bridge_level) {
        return Err(anyhow!("Bridge level must be between 0.15 and 1.0."));
    }
    Ok(())
}

struct RenderedSection {
    mix: Vec<[f32; 2]>,
    stems: Option<StemSet>,
}

struct StemSet {
    drums: Vec<[f32; 2]>,
    bass: Vec<[f32; 2]>,
    other: Vec<[f32; 2]>,
    vocals: Vec<[f32; 2]>,
}

impl StemSet {
    fn frame(&self, stem: StemKind, index: usize) -> [f32; 2] {
        let frames = match stem {
            StemKind::Drums => &self.drums,
            StemKind::Bass => &self.bass,
            StemKind::Other => &self.other,
            StemKind::Vocals => &self.vocals,
        };
        frames.get(index).copied().unwrap_or([0.0, 0.0])
    }

    fn target_len(&self) -> usize {
        self.drums
            .len()
            .max(self.bass.len())
            .max(self.other.len())
            .max(self.vocals.len())
    }

    fn normalize_lengths(&mut self) {
        let target = self.target_len();
        fit_frames(&mut self.drums, target);
        fit_frames(&mut self.bass, target);
        fit_frames(&mut self.other, target);
        fit_frames(&mut self.vocals, target);
    }

    fn recombine(&self) -> Vec<[f32; 2]> {
        (0..self.target_len())
            .map(|index| {
                let drums = self.frame(StemKind::Drums, index);
                let bass = self.frame(StemKind::Bass, index);
                let other = self.frame(StemKind::Other, index);
                let vocals = self.frame(StemKind::Vocals, index);
                [
                    drums[0] + bass[0] + other[0] + vocals[0],
                    drums[1] + bass[1] + other[1] + vocals[1],
                ]
            })
            .collect()
    }
}

#[derive(Clone, Copy)]
enum StemKind {
    Drums,
    Bass,
    Other,
    Vocals,
}

fn render_planned_mix(
    tracks: &[PlannedTrack],
    options: DjMixOptions,
    custom_bridge: Option<&CustomBridgeAudio>,
    tools: &ProfessionalToolchain,
    writer: &mut Mp3StreamWriter,
    sender: &Sender<DjMixEvent>,
    cancel: &AtomicBool,
) -> Result<()> {
    let first = tracks
        .first()
        .ok_or_else(|| anyhow!("DJ plan contains no tracks."))?;
    send_progress(sender, format!("Preparing {}", first.track.title), 0.40);
    let mut current_audio = render_planned_section(first, options, tools, sender, cancel)?;
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
                    DjMixStyle::SmartDj => "Planning human transition",
                },
                current.track.title,
                next.track.title
            ),
            0.45 + pair_index as f32 / ((tracks.len() - 1) as f32 * 2.0),
        );

        let next_audio = render_planned_section(next, options, tools, sender, cancel)?;
        let requested_transition = planned_transitions[pair_index];
        let following_transition = planned_transitions
            .get(pair_index + 1)
            .copied()
            .unwrap_or(0);
        let next_budget = incoming_transition_budget(
            next_audio.mix.len(),
            requested_transition,
            following_transition,
        );
        let overlap = requested_transition
            .min(current_audio.mix.len())
            .min(next_budget);
        if overlap == 0 {
            write_frames(&current_audio.mix, current.gain, writer, cancel)?;
            current_audio = next_audio;
            continue;
        }

        let body_len = current_audio.mix.len().saturating_sub(overlap);
        write_frames(&current_audio.mix[..body_len], current.gain, writer, cancel)?;
        match options.style {
            DjMixStyle::Crossfade => write_crossfade_transition(
                &current_audio.mix[body_len..],
                &next_audio.mix[..overlap],
                current.gain,
                next.gain,
                options.bass_swap,
                writer,
                cancel,
            )?,
            DjMixStyle::SmartDj => write_performance_transition(
                &current_audio.mix[body_len..],
                &next_audio.mix[..overlap],
                current_audio.stems.as_ref(),
                body_len,
                next_audio.stems.as_ref(),
                current,
                next,
                options.bass_swap,
                options.bridge_mode,
                options.bridge_level,
                custom_bridge,
                writer,
                cancel,
            )?,
        }
        current_audio.mix = next_audio.mix[overlap..].to_vec();
        current_audio.stems = next_audio.stems.map(|mut stems| {
            stems.drums = stems.drums.into_iter().skip(overlap).collect();
            stems.bass = stems.bass.into_iter().skip(overlap).collect();
            stems.other = stems.other.into_iter().skip(overlap).collect();
            stems.vocals = stems.vocals.into_iter().skip(overlap).collect();
            stems
        });
    }

    let last = tracks
        .last()
        .ok_or_else(|| anyhow!("DJ plan contains no tracks."))?;
    write_frames(&current_audio.mix, last.gain, writer, cancel)?;
    Ok(())
}

fn render_planned_section(
    track: &PlannedTrack,
    options: DjMixOptions,
    tools: &ProfessionalToolchain,
    sender: &Sender<DjMixEvent>,
    cancel: &AtomicBool,
) -> Result<RenderedSection> {
    send_progress(sender, format!("Decoding {}", track.track.title), 0.42);
    let decoded = decode_section(
        &track.track.path,
        track.section_start_seconds,
        track.section_end_seconds,
        cancel,
    )?;

    let mut stems = if options.style == DjMixStyle::SmartDj
        && options.professional_tools
        && options.stem_separation
        && tools.demucs.is_some()
    {
        send_progress(
            sender,
            format!(
                "Separating drums, bass, vocals and music: {}",
                track.track.title
            ),
            0.43,
        );
        match separate_with_demucs(track, &decoded, tools, cancel) {
            Ok(stems) => stems,
            Err(error) if cancel.load(AtomicOrdering::Relaxed) => return Err(error),
            Err(_) => None,
        }
    } else {
        None
    };

    if options.style == DjMixStyle::Crossfade || (track.speed_ratio - 1.0).abs() < 0.0005 {
        if let Some(stems) = stems.as_mut() {
            stems.normalize_lengths();
        }
        return Ok(RenderedSection {
            mix: stems.as_ref().map(StemSet::recombine).unwrap_or(decoded),
            stems,
        });
    }

    ensure_not_cancelled(cancel)?;
    let cache_tag = section_cache_tag(track)?;
    if let Some(stems) = stems.as_mut() {
        send_progress(
            sender,
            format!("Studio tempo match per stem: {}", track.track.title),
            0.44,
        );
        stems.drums = stretch_audio(
            &stems.drums,
            track.speed_ratio,
            tools,
            &format!("{cache_tag}-drums"),
            cancel,
        )?;
        stems.bass = stretch_audio(
            &stems.bass,
            track.speed_ratio,
            tools,
            &format!("{cache_tag}-bass"),
            cancel,
        )?;
        stems.other = stretch_audio(
            &stems.other,
            track.speed_ratio,
            tools,
            &format!("{cache_tag}-other"),
            cancel,
        )?;
        stems.vocals = stretch_audio(
            &stems.vocals,
            track.speed_ratio,
            tools,
            &format!("{cache_tag}-vocals"),
            cancel,
        )?;
        stems.normalize_lengths();
        let mix = stems.recombine();
        if mix.is_empty() {
            return Err(anyhow!("Tempo matching failed for {}", track.track.title));
        }
        return Ok(RenderedSection { mix, stems });
    }

    let output = stretch_audio(&decoded, track.speed_ratio, tools, &cache_tag, cancel)?;
    ensure_not_cancelled(cancel)?;
    if output.is_empty() {
        return Err(anyhow!(
            "Pitch-preserving time stretch failed for {}",
            track.track.title
        ));
    }
    Ok(RenderedSection {
        mix: output,
        stems: None,
    })
}

fn section_cache_tag(track: &PlannedTrack) -> Result<String> {
    let start = track.section_start_seconds.to_le_bytes();
    let end = track.section_end_seconds.to_le_bytes();
    let key = file_cache_key(
        &track.track.path,
        &[ENGINE_VERSION.as_bytes(), &start, &end],
    )?;
    Ok(format!("{key:016x}-{:.6}", track.speed_ratio))
}

fn separate_with_demucs(
    track: &PlannedTrack,
    decoded: &[[f32; 2]],
    tools: &ProfessionalToolchain,
    cancel: &AtomicBool,
) -> Result<Option<StemSet>> {
    let Some(demucs) = tools.demucs.as_ref() else {
        return Ok(None);
    };
    let cache_tag = section_cache_tag(track)?;
    let cache_dir = professional_cache_dir().join("demucs").join(cache_tag);
    fs::create_dir_all(&cache_dir)?;
    let input_path = cache_dir.join("section.wav");
    if !input_path.is_file() {
        write_pcm16_wav(&input_path, decoded)?;
    }

    let mut stem_paths = locate_demucs_stems(&cache_dir);
    if stem_paths.is_none() {
        let mut command = demucs.command();
        command
            .arg("-n")
            .arg("htdemucs")
            .arg("--out")
            .arg(&cache_dir)
            .arg("--shifts")
            .arg("0")
            .arg("--overlap")
            .arg("0.25")
            .arg(&input_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = run_command_cancellable(command, cancel)?;
        if !status.success() {
            return Ok(None);
        }
        stem_paths = locate_demucs_stems(&cache_dir);
    }
    let Some([drums_path, bass_path, other_path, vocals_path]) = stem_paths else {
        return Ok(None);
    };
    Ok(Some(StemSet {
        drums: decode_entire_audio(&drums_path, cancel)?,
        bass: decode_entire_audio(&bass_path, cancel)?,
        other: decode_entire_audio(&other_path, cancel)?,
        vocals: decode_entire_audio(&vocals_path, cancel)?,
    }))
}

fn locate_demucs_stems(root: &Path) -> Option<[PathBuf; 4]> {
    Some([
        find_file_named(root, "drums.wav")?,
        find_file_named(root, "bass.wav")?,
        find_file_named(root, "other.wav")?,
        find_file_named(root, "vocals.wav")?,
    ])
}

fn find_file_named(root: &Path, file_name: &str) -> Option<PathBuf> {
    let entries = fs::read_dir(root).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_named(&path, file_name) {
                return Some(found);
            }
        } else if path
            .file_name()
            .and_then(|value| value.to_str())
            .is_some_and(|value| value.eq_ignore_ascii_case(file_name))
        {
            return Some(path);
        }
    }
    None
}

fn stretch_audio(
    frames: &[[f32; 2]],
    speed_ratio: f32,
    tools: &ProfessionalToolchain,
    cache_tag: &str,
    cancel: &AtomicBool,
) -> Result<Vec<[f32; 2]>> {
    ensure_not_cancelled(cancel)?;
    if frames.is_empty() || (speed_ratio - 1.0).abs() < 0.0005 {
        return Ok(frames.to_vec());
    }
    if let Some(rubber_band) = tools.rubber_band.as_ref() {
        match stretch_with_rubber_band(frames, speed_ratio, rubber_band, cache_tag, cancel) {
            Ok(output) if !output.is_empty() => return Ok(output),
            Err(error) if cancel.load(AtomicOrdering::Relaxed) => return Err(error),
            _ => {}
        }
    }
    ensure_not_cancelled(cancel)?;
    Ok(pitch_preserving_stretch(frames, speed_ratio))
}

fn stretch_with_rubber_band(
    frames: &[[f32; 2]],
    speed_ratio: f32,
    rubber_band: &ExternalCommand,
    cache_tag: &str,
    cancel: &AtomicBool,
) -> Result<Vec<[f32; 2]>> {
    let cache_dir = professional_cache_dir().join("rubber-band");
    fs::create_dir_all(&cache_dir)?;
    let safe_tag = cache_tag
        .chars()
        .map(|value| {
            if value.is_ascii_alphanumeric() || matches!(value, '-' | '_') {
                value
            } else {
                '_'
            }
        })
        .collect::<String>();
    let input_path = cache_dir.join(format!("{safe_tag}-input.wav"));
    let output_path = cache_dir.join(format!("{safe_tag}-output.wav"));
    if !output_path.is_file() {
        write_pcm16_wav(&input_path, frames)?;
        let mut command = rubber_band.command();
        command
            .arg("-3")
            .arg("--centre-focus")
            .arg("--quiet")
            .arg("--tempo")
            .arg(format!("{speed_ratio:.8}"))
            .arg(&input_path)
            .arg(&output_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = run_command_cancellable(command, cancel)?;
        if !status.success() {
            return Err(anyhow!("Rubber Band exited with {status}"));
        }
    }
    decode_entire_audio(&output_path, cancel)
}

fn write_pcm16_wav(path: &Path, frames: &[[f32; 2]]) -> Result<()> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let data_bytes = frames.len() as u32 * OUTPUT_CHANNELS as u32 * 2;
    let byte_rate = OUTPUT_SAMPLE_RATE * OUTPUT_CHANNELS as u32 * 2;
    let block_align = OUTPUT_CHANNELS * 2;
    let mut file = BufWriter::new(File::create(path)?);
    file.write_all(b"RIFF")?;
    file.write_all(&(36u32.saturating_add(data_bytes)).to_le_bytes())?;
    file.write_all(b"WAVEfmt ")?;
    file.write_all(&16u32.to_le_bytes())?;
    file.write_all(&1u16.to_le_bytes())?;
    file.write_all(&OUTPUT_CHANNELS.to_le_bytes())?;
    file.write_all(&OUTPUT_SAMPLE_RATE.to_le_bytes())?;
    file.write_all(&byte_rate.to_le_bytes())?;
    file.write_all(&block_align.to_le_bytes())?;
    file.write_all(&16u16.to_le_bytes())?;
    file.write_all(b"data")?;
    file.write_all(&data_bytes.to_le_bytes())?;
    for frame in frames {
        file.write_all(&float_to_i16(frame[0]).to_le_bytes())?;
        file.write_all(&float_to_i16(frame[1]).to_le_bytes())?;
    }
    file.flush()?;
    Ok(())
}

fn decode_entire_audio(path: &Path, cancel: &AtomicBool) -> Result<Vec<[f32; 2]>> {
    let file = File::open(path)
        .with_context(|| format!("Failed to open audio file: {}", path.display()))?;
    let decoder = Decoder::new(BufReader::new(file))
        .with_context(|| format!("Failed to decode audio file: {}", path.display()))?;
    let mut source =
        UniformSourceIterator::<_, f32>::new(decoder, OUTPUT_CHANNELS, OUTPUT_SAMPLE_RATE);
    let mut output = Vec::new();
    let mut index = 0usize;
    while let Some(frame) = read_stereo_frame(&mut source) {
        if index % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        output.push(frame);
        index += 1;
    }
    Ok(output)
}

fn fit_frames(frames: &mut Vec<[f32; 2]>, target: usize) {
    if frames.len() > target {
        frames.truncate(target);
    } else if frames.len() < target {
        frames.resize(target, [0.0, 0.0]);
    }
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
    outgoing_stems: Option<&StemSet>,
    outgoing_stem_offset: usize,
    incoming_stems: Option<&StemSet>,
    current: &PlannedTrack,
    next: &PlannedTrack,
    bass_swap: bool,
    bridge_mode: DjBridgeMode,
    bridge_level: f32,
    custom_bridge: Option<&CustomBridgeAudio>,
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
    let loop_frames = beat_frames.saturating_mul(4).max(256).min(frames);
    let strong_change = transition_should_hit_hard(current, next);
    let stems_available = outgoing_stems.is_some() && incoming_stems.is_some();
    let recipe = TransitionRecipe::resolve(
        bridge_mode,
        transition_seed(current, next),
        strong_change,
        stems_available,
        keys_are_compatible(current, next),
    );

    let mut outgoing_sweep = LowPassStereo::new(18_000.0);
    let mut incoming_sweep = LowPassStereo::new(320.0);
    let mut harmonic_sweep = LowPassStereo::new(7_500.0);
    let mut delay = StereoDelay::new((beat_frames / 2).max(1), 0.42);
    let mut percussion = TransitionPercussion::new(bpm, transition_seed(current, next));
    let mut outgoing_vocal_guard = CenterVocalGuard::new();
    let mut incoming_vocal_guard = CenterVocalGuard::new();
    let vocal_plan = plan_vocal_handoff(&outgoing[..frames], &incoming[..frames]);

    for index in 0..frames {
        if index % (OUTPUT_SAMPLE_RATE as usize / 2) == 0 {
            ensure_not_cancelled(cancel)?;
        }
        let progress = normalized_progress(index, frames);
        if index % 64 == 0 {
            outgoing_sweep.set_cutoff(18_000.0 - smoothstep(0.28, 0.78, progress) * 16_900.0);
            incoming_sweep.set_cutoff(300.0 + smoothstep(0.58, 0.90, progress) * 17_600.0);
            harmonic_sweep.set_cutoff(2_800.0 + smoothstep(0.25, 0.70, progress) * 9_000.0);
        }

        let mut mixed = if let (Some(out_stems), Some(in_stems)) = (outgoing_stems, incoming_stems)
        {
            let gates = recipe.stem_gates(progress, strong_change, bass_swap);
            mix_stem_transition_frame(
                out_stems,
                outgoing_stem_offset + index,
                in_stems,
                index,
                gates,
                current.gain * peak_scale,
                next.gain * peak_scale,
            )
        } else {
            let mut out = multiply_frame(outgoing[index], current.gain * peak_scale);
            let mut input = multiply_frame(incoming[index], next.gain * peak_scale);
            let (outgoing_vocal_duck, incoming_vocal_duck) =
                vocal_plan.duck_amounts(index, progress);
            out = outgoing_vocal_guard.process(out, outgoing_vocal_duck);
            input = incoming_vocal_guard.process(input, incoming_vocal_duck);
            out = mix_frame(
                out,
                outgoing_sweep.process(out),
                smoothstep(0.28, 0.72, progress) * 0.88,
            );
            input = mix_frame(
                incoming_sweep.process(input),
                input,
                smoothstep(0.66, 0.90, progress),
            );
            let out_gate =
                1.0 - smoothstep(0.42, if strong_change { 0.66 } else { 0.74 }, progress);
            let in_gate = smoothstep(if strong_change { 0.72 } else { 0.66 }, 0.88, progress);
            [
                out[0] * out_gate + input[0] * in_gate,
                out[1] * out_gate + input[1] * in_gate,
            ]
        };

        let deck_bridge = transition_deck_bridge(
            recipe,
            outgoing,
            incoming,
            outgoing_stems,
            outgoing_stem_offset,
            incoming_stems,
            index,
            frames,
            loop_frames,
            &mut harmonic_sweep,
        );
        let custom = custom_bridge
            .filter(|_| bridge_mode == DjBridgeMode::Custom)
            .map(|audio| audio.render(index, progress));
        let bridge_frame = custom.unwrap_or(deck_bridge);
        let bridge_gate = recipe.bridge_gate(progress, strong_change) * bridge_level;
        mixed[0] += bridge_frame[0] * bridge_gate;
        mixed[1] += bridge_frame[1] * bridge_gate;

        let percussion_frame = percussion.render(index, progress, strong_change);
        let percussion_gate = recipe.percussion_gate(progress);
        mixed[0] += percussion_frame[0] * percussion_gate;
        mixed[1] += percussion_frame[1] * percussion_gate;

        if recipe.uses_echo_out() {
            let echo_source = multiply_frame(outgoing[index], current.gain * peak_scale);
            let delayed = delay.process(echo_source);
            let echo_gate =
                smoothstep(0.36, 0.55, progress) * (1.0 - smoothstep(0.76, 0.90, progress));
            mixed[0] += delayed[0] * echo_gate * 0.46;
            mixed[1] += delayed[1] * echo_gate * 0.46;
        }

        if recipe.has_drop_cut() && progress > 0.69 && progress < 0.735 {
            let down = 1.0 - smoothstep(0.69, 0.715, progress);
            let up = smoothstep(0.715, 0.735, progress);
            let cut = down.max(up).clamp(0.0, 1.0);
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

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum TransitionRecipe {
    DrumSwap,
    HarmonicBridge,
    EchoDrop,
    StemMashup,
    CustomBridge,
}

impl TransitionRecipe {
    fn resolve(
        mode: DjBridgeMode,
        seed: u32,
        strong: bool,
        stems_available: bool,
        harmonic_match: bool,
    ) -> Self {
        match mode {
            DjBridgeMode::DrumSwap => Self::DrumSwap,
            DjBridgeMode::HarmonicBridge => Self::HarmonicBridge,
            DjBridgeMode::EchoDrop => Self::EchoDrop,
            DjBridgeMode::StemMashup => Self::StemMashup,
            DjBridgeMode::Custom => Self::CustomBridge,
            DjBridgeMode::Auto if strong => Self::EchoDrop,
            DjBridgeMode::Auto if stems_available && harmonic_match => {
                if seed % 2 == 0 {
                    Self::HarmonicBridge
                } else {
                    Self::StemMashup
                }
            }
            DjBridgeMode::Auto if stems_available => Self::DrumSwap,
            DjBridgeMode::Auto => match seed % 3 {
                0 => Self::DrumSwap,
                1 => Self::EchoDrop,
                _ => Self::HarmonicBridge,
            },
        }
    }

    fn stem_gates(self, progress: f32, strong: bool, bass_swap: bool) -> StemGates {
        let bass_point = if bass_swap { 0.54 } else { 0.66 };
        match self {
            Self::DrumSwap => StemGates {
                out_drums: 1.0 - smoothstep(0.34, 0.58, progress),
                out_bass: 1.0 - smoothstep(bass_point - 0.06, bass_point, progress),
                out_other: 1.0 - smoothstep(0.58, 0.80, progress),
                out_vocals: 1.0 - smoothstep(0.32, 0.52, progress),
                in_drums: smoothstep(0.22, 0.46, progress),
                in_bass: smoothstep(bass_point, bass_point + 0.10, progress),
                in_other: smoothstep(0.62, 0.84, progress),
                in_vocals: smoothstep(0.82, 0.96, progress),
            },
            Self::HarmonicBridge => StemGates {
                out_drums: 1.0 - smoothstep(0.42, 0.68, progress),
                out_bass: 1.0 - smoothstep(0.46, 0.58, progress),
                out_other: 1.0 - smoothstep(0.66, 0.88, progress),
                out_vocals: 1.0 - smoothstep(0.26, 0.46, progress),
                in_drums: smoothstep(0.42, 0.68, progress),
                in_bass: smoothstep(0.58, 0.70, progress),
                in_other: smoothstep(0.58, 0.86, progress),
                in_vocals: smoothstep(0.84, 0.98, progress),
            },
            Self::EchoDrop => StemGates {
                out_drums: 1.0 - smoothstep(0.42, 0.68, progress),
                out_bass: 1.0 - smoothstep(0.46, 0.62, progress),
                out_other: 1.0 - smoothstep(0.50, 0.70, progress),
                out_vocals: 1.0 - smoothstep(0.38, 0.58, progress),
                in_drums: smoothstep(0.72, 0.79, progress),
                in_bass: smoothstep(0.74, 0.82, progress),
                in_other: smoothstep(0.74, 0.84, progress),
                in_vocals: smoothstep(0.84, 0.96, progress),
            },
            Self::StemMashup | Self::CustomBridge => StemGates {
                out_drums: 1.0 - smoothstep(0.48, 0.72, progress),
                out_bass: 1.0 - smoothstep(0.48, 0.58, progress),
                out_other: 1.0 - smoothstep(0.68, 0.88, progress),
                out_vocals: 1.0 - smoothstep(0.30, 0.50, progress),
                in_drums: smoothstep(0.26, 0.48, progress),
                in_bass: smoothstep(0.58, 0.70, progress),
                in_other: smoothstep(0.60, 0.84, progress),
                in_vocals: smoothstep(if strong { 0.86 } else { 0.82 }, 0.96, progress),
            },
        }
    }

    fn bridge_gate(self, progress: f32, strong: bool) -> f32 {
        let start = match self {
            Self::EchoDrop => 0.28,
            _ => 0.18,
        };
        let end = if strong { 0.88 } else { 0.92 };
        smoothstep(start, start + 0.12, progress) * (1.0 - smoothstep(end, 0.98, progress))
    }

    fn percussion_gate(self, progress: f32) -> f32 {
        match self {
            Self::HarmonicBridge => 0.34 * smoothstep(0.24, 0.44, progress),
            Self::EchoDrop => 0.72 * smoothstep(0.32, 0.68, progress),
            _ => 0.48 * smoothstep(0.20, 0.44, progress),
        }
    }

    fn uses_echo_out(self) -> bool {
        matches!(self, Self::EchoDrop | Self::CustomBridge)
    }

    fn has_drop_cut(self) -> bool {
        matches!(self, Self::EchoDrop)
    }

    fn diagnostic_name(self) -> &'static str {
        match self {
            Self::DrumSwap => "drum_swap",
            Self::HarmonicBridge => "harmonic_bridge",
            Self::EchoDrop => "echo_drop",
            Self::StemMashup => "stem_mashup",
            Self::CustomBridge => "custom_bridge",
        }
    }
}

#[derive(Clone, Copy)]
struct StemGates {
    out_drums: f32,
    out_bass: f32,
    out_other: f32,
    out_vocals: f32,
    in_drums: f32,
    in_bass: f32,
    in_other: f32,
    in_vocals: f32,
}

fn mix_stem_transition_frame(
    outgoing: &StemSet,
    outgoing_index: usize,
    incoming: &StemSet,
    incoming_index: usize,
    gates: StemGates,
    outgoing_gain: f32,
    incoming_gain: f32,
) -> [f32; 2] {
    let mut mixed = [0.0, 0.0];
    add_scaled(
        &mut mixed,
        outgoing.frame(StemKind::Drums, outgoing_index),
        outgoing_gain * gates.out_drums,
    );
    add_scaled(
        &mut mixed,
        outgoing.frame(StemKind::Bass, outgoing_index),
        outgoing_gain * gates.out_bass,
    );
    add_scaled(
        &mut mixed,
        outgoing.frame(StemKind::Other, outgoing_index),
        outgoing_gain * gates.out_other,
    );
    add_scaled(
        &mut mixed,
        outgoing.frame(StemKind::Vocals, outgoing_index),
        outgoing_gain * gates.out_vocals,
    );
    add_scaled(
        &mut mixed,
        incoming.frame(StemKind::Drums, incoming_index),
        incoming_gain * gates.in_drums,
    );
    add_scaled(
        &mut mixed,
        incoming.frame(StemKind::Bass, incoming_index),
        incoming_gain * gates.in_bass,
    );
    add_scaled(
        &mut mixed,
        incoming.frame(StemKind::Other, incoming_index),
        incoming_gain * gates.in_other,
    );
    add_scaled(
        &mut mixed,
        incoming.frame(StemKind::Vocals, incoming_index),
        incoming_gain * gates.in_vocals,
    );
    mixed
}

fn add_scaled(target: &mut [f32; 2], frame: [f32; 2], gain: f32) {
    target[0] += frame[0] * gain;
    target[1] += frame[1] * gain;
}

#[allow(clippy::too_many_arguments)]
fn transition_deck_bridge(
    recipe: TransitionRecipe,
    outgoing: &[[f32; 2]],
    incoming: &[[f32; 2]],
    outgoing_stems: Option<&StemSet>,
    outgoing_stem_offset: usize,
    incoming_stems: Option<&StemSet>,
    index: usize,
    frames: usize,
    loop_frames: usize,
    harmonic_filter: &mut LowPassStereo,
) -> [f32; 2] {
    let progress = normalized_progress(index, frames);
    let loop_position = index % loop_frames.max(1);
    if let (Some(out_stems), Some(in_stems)) = (outgoing_stems, incoming_stems) {
        let out_loop_start = outgoing_stem_offset + frames.saturating_sub(loop_frames);
        let out_index = out_loop_start + loop_position;
        let in_index = loop_position;
        let out_drums = out_stems.frame(StemKind::Drums, out_index);
        let in_drums = in_stems.frame(StemKind::Drums, in_index);
        let out_other = out_stems.frame(StemKind::Other, out_index);
        let in_other = in_stems.frame(StemKind::Other, in_index);
        return match recipe {
            TransitionRecipe::DrumSwap => [
                out_drums[0] * (1.0 - progress) + in_drums[0] * progress,
                out_drums[1] * (1.0 - progress) + in_drums[1] * progress,
            ],
            TransitionRecipe::HarmonicBridge => {
                let harmonic = [
                    out_other[0] * (1.0 - progress) + in_other[0] * progress,
                    out_other[1] * (1.0 - progress) + in_other[1] * progress,
                ];
                harmonic_filter.process(harmonic)
            }
            TransitionRecipe::EchoDrop => [out_drums[0] * 0.42, out_drums[1] * 0.42],
            TransitionRecipe::StemMashup | TransitionRecipe::CustomBridge => [
                out_other[0] * (1.0 - progress) + in_drums[0] * progress,
                out_other[1] * (1.0 - progress) + in_drums[1] * progress,
            ],
        };
    }

    let out_loop_start = frames.saturating_sub(loop_frames);
    let out_index = (out_loop_start + loop_position).min(outgoing.len().saturating_sub(1));
    let in_index = loop_position.min(incoming.len().saturating_sub(1));
    let out = outgoing.get(out_index).copied().unwrap_or([0.0, 0.0]);
    let input = incoming.get(in_index).copied().unwrap_or([0.0, 0.0]);
    let blend = [
        out[0] * (1.0 - progress) + input[0] * progress,
        out[1] * (1.0 - progress) + input[1] * progress,
    ];
    harmonic_filter.process(blend)
}

fn keys_are_compatible(current: &PlannedTrack, next: &PlannedTrack) -> bool {
    analysis_keys_are_compatible(&current.analysis, &next.analysis)
}

fn analysis_keys_are_compatible(left: &TrackAnalysis, right: &TrackAnalysis) -> bool {
    let (Some(left_key), Some(right_key)) =
        (left.musical_key.as_deref(), right.musical_key.as_deref())
    else {
        return false;
    };
    harmonic_keys_are_compatible(left_key, right_key)
}

fn harmonic_keys_are_compatible(left: &str, right: &str) -> bool {
    let (Some((left_pitch, left_mode)), Some((right_pitch, right_mode))) =
        (parse_musical_key(left), parse_musical_key(right))
    else {
        return left.eq_ignore_ascii_case(right);
    };
    if left_pitch == right_pitch && left_mode == right_mode {
        return true;
    }

    match (left_mode, right_mode) {
        (KeyMode::Major, KeyMode::Minor) => (right_pitch + 3).rem_euclid(12) == left_pitch,
        (KeyMode::Minor, KeyMode::Major) => (left_pitch + 3).rem_euclid(12) == right_pitch,
        _ => {
            let interval = (right_pitch - left_pitch).rem_euclid(12);
            matches!(interval, 0 | 5 | 7)
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum KeyMode {
    Major,
    Minor,
    Unknown,
}

fn parse_musical_key(key: &str) -> Option<(i32, KeyMode)> {
    let mut tokens = key.split_whitespace();
    let pitch = key_pitch_class(tokens.next()?)?;
    let mode = match tokens.next().map(str::to_ascii_lowercase).as_deref() {
        Some("major" | "maj") => KeyMode::Major,
        Some("minor" | "min") => KeyMode::Minor,
        _ => KeyMode::Unknown,
    };
    Some((pitch, mode))
}

fn key_pitch_class(token: &str) -> Option<i32> {
    match token {
        "C" => Some(0),
        "C#" | "Db" => Some(1),
        "D" => Some(2),
        "D#" | "Eb" => Some(3),
        "E" => Some(4),
        "F" => Some(5),
        "F#" | "Gb" => Some(6),
        "G" => Some(7),
        "G#" | "Ab" => Some(8),
        "A" => Some(9),
        "A#" | "Bb" => Some(10),
        "B" => Some(11),
        _ => None,
    }
}

struct CustomBridgeAudio {
    frames: Vec<[f32; 2]>,
}

impl CustomBridgeAudio {
    fn render(&self, frame_index: usize, progress: f32) -> [f32; 2] {
        if self.frames.is_empty() {
            return [0.0, 0.0];
        }
        let position = frame_index % self.frames.len();
        let frame = self.frames[position];
        let loop_edge = position as f32 / self.frames.len() as f32;
        let edge_fade =
            smoothstep(0.0, 0.035, loop_edge) * (1.0 - smoothstep(0.965, 1.0, loop_edge));
        let movement = 0.94 + 0.06 * (std::f32::consts::TAU * progress).sin();
        [
            frame[0] * edge_fade * movement,
            frame[1] * edge_fade * movement,
        ]
    }
}

fn load_custom_bridge(
    request: &ExportRequest,
    sender: &Sender<DjMixEvent>,
    cancel: &AtomicBool,
) -> Result<Option<CustomBridgeAudio>> {
    if request.options.bridge_mode != DjBridgeMode::Custom {
        return Ok(None);
    }
    let path = request
        .custom_bridge_path
        .as_ref()
        .ok_or_else(|| anyhow!("Custom bridge mode requires an audio file."))?;
    send_progress(sender, "Preparing custom bridge audio".to_owned(), 0.39);
    let start = request.options.bridge_start_seconds.max(0.0);
    let end = start + request.options.bridge_loop_seconds.clamp(0.25, 60.0);
    let frames = decode_section(path, start, end, cancel)
        .with_context(|| format!("Failed to prepare custom bridge: {}", path.display()))?;
    Ok(Some(CustomBridgeAudio { frames }))
}

struct TransitionPercussion {
    bpm: f32,
    seed: u32,
    kick_phase: f32,
    noise_state: u32,
}

impl TransitionPercussion {
    fn new(bpm: f32, seed: u32) -> Self {
        Self {
            bpm,
            seed,
            kick_phase: 0.0,
            noise_state: seed ^ 0xA5A5_17C3,
        }
    }

    fn render(&mut self, frame_index: usize, progress: f32, strong: bool) -> [f32; 2] {
        let seconds = frame_index as f32 / OUTPUT_SAMPLE_RATE as f32;
        let beats = seconds * self.bpm / 60.0;
        let beat_phase = beats.fract();
        let half_beat_phase = (beats * 2.0).fract();
        let kick_env = (-beat_phase * 19.0).exp();
        let kick_hz = 46.0 + 76.0 * (-beat_phase * 25.0).exp();
        self.kick_phase = (self.kick_phase + kick_hz / OUTPUT_SAMPLE_RATE as f32).fract();
        let kick = (std::f32::consts::TAU * self.kick_phase).sin() * kick_env * 0.20;
        let noise = transition_noise(&mut self.noise_state);
        let hat = if half_beat_phase < 0.08 {
            noise * (1.0 - half_beat_phase / 0.08) * 0.042
        } else {
            0.0
        };
        let snare_phase = (beats + 0.5).fract();
        let snare = if snare_phase < 0.10 {
            noise * (1.0 - snare_phase / 0.10).powi(2) * if strong { 0.09 } else { 0.055 }
        } else {
            0.0
        };
        let roll = smoothstep(0.56, 0.78, progress);
        let pan = (((self.seed & 0xff) as f32 / 255.0) - 0.5) * 0.18;
        [
            kick + hat * (1.0 - pan) + snare * roll,
            kick + hat * (1.0 + pan) + snare * roll,
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
    tools: &ProfessionalToolchain,
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
    let analyzer_version = if tools.essentia.is_some() {
        format!("{ANALYZER_VERSION}+essentia")
    } else {
        format!("{ANALYZER_VERSION}+builtin")
    };
    let key = track.path.to_string_lossy().to_string();
    if cache.schema_version == ENGINE_SCHEMA_VERSION {
        if let Some(entry) = cache.tracks.get(&key) {
            if entry.file_len == metadata.len()
                && entry.modified_nanos == modified_nanos
                && entry.analyzer_version == analyzer_version
            {
                return Ok(entry.analysis.clone());
            }
        }
    }

    let analysis = analyze_track(&track.path, tools, cancel)?;
    cache.schema_version = ENGINE_SCHEMA_VERSION;
    cache.tracks.insert(
        key,
        CachedTrackAnalysis {
            file_len: metadata.len(),
            modified_nanos,
            analyzer_version,
            analysis: analysis.clone(),
        },
    );
    Ok(analysis)
}

fn analyze_track(
    path: &Path,
    tools: &ProfessionalToolchain,
    cancel: &AtomicBool,
) -> Result<TrackAnalysis> {
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
    if frames_read == 0 {
        return Err(anyhow!(
            "Track contains no decodable audio: {}",
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

    let mut beat_positions_seconds = Vec::new();
    let beat_period = 60.0 / bpm.max(1.0);
    let mut beat = beat_offset_seconds.max(0.0);
    while beat <= duration_seconds {
        beat_positions_seconds.push(beat);
        beat += beat_period;
    }

    let mut analysis = TrackAnalysis {
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
        beat_positions_seconds,
        sections,
        musical_key: None,
        key_confidence: None,
        vocal_profile: AnalysisAvailability::Unavailable {
            reason: "Stem separation is evaluated during export when Demucs is available."
                .to_owned(),
        },
        analysis_backend: "built-in rhythm envelope + ebur128".to_owned(),
    };

    if let Some(essentia) = tools.essentia.as_ref() {
        match analyze_with_essentia(path, essentia, cancel) {
            Ok(external) => apply_essentia_analysis(&mut analysis, external),
            Err(error) if cancel.load(AtomicOrdering::Relaxed) => return Err(error),
            Err(_) => {}
        }
    }
    Ok(analysis)
}

#[derive(Clone, Debug)]
struct EssentiaTrackAnalysis {
    bpm: Option<f32>,
    confidence: Option<f32>,
    beats: Vec<f32>,
    key: Option<String>,
    key_strength: Option<f32>,
}

fn analyze_with_essentia(
    path: &Path,
    essentia: &ExternalCommand,
    cancel: &AtomicBool,
) -> Result<EssentiaTrackAnalysis> {
    let cache_dir = professional_cache_dir().join("essentia");
    fs::create_dir_all(&cache_dir)?;
    let cache_key = file_cache_key(path, &[ANALYZER_VERSION.as_bytes()])?;
    let output_path = cache_dir.join(format!("{cache_key:016x}.json"));
    let profile_path = cache_dir.join("audio-orbit-music-extractor.yaml");
    if !profile_path.is_file() {
        fs::write(
            &profile_path,
            concat!(
                "outputFormat: json\n",
                "outputFrames: 0\n",
                "requireMbid: false\n",
                "indent: 2\n",
                "analysisSampleRate: 44100.0\n",
                "rhythm:\n",
                "  method: multifeature\n",
                "  minTempo: 70\n",
                "  maxTempo: 180\n",
                "tonal:\n",
                "  frameSize: 4096\n",
                "  hopSize: 2048\n",
                "  zeroPadding: 0\n",
                "  windowType: blackmanharris62\n",
                "  silentFrames: noise\n",
                "highlevel:\n",
                "  compute: 0\n",
            ),
        )?;
    }

    if !output_path.is_file() {
        let mut command = essentia.command();
        command
            .arg(path)
            .arg(&output_path)
            .arg(&profile_path)
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        let status = run_command_cancellable(command, cancel)
            .context("Essentia music analysis failed to start")?;
        if !status.success() {
            return Err(anyhow!("Essentia exited with {status}"));
        }
    }

    let value: serde_json::Value =
        serde_json::from_slice(&fs::read(&output_path).with_context(|| {
            format!("Failed to read Essentia output: {}", output_path.display())
        })?)
        .context("Failed to parse Essentia JSON output")?;
    let bpm = json_number(&value, &["rhythm.bpm"]);
    let confidence = json_number(&value, &["rhythm.confidence", "rhythm.beats_confidence"]);
    let beats = json_number_array(&value, &["rhythm.beats_position", "rhythm.beats_positions"]);
    let key_name = json_string(
        &value,
        &[
            "tonal.key_edma.key",
            "tonal.key_temperley.key",
            "tonal.key_krumhansl.key",
        ],
    );
    let scale = json_string(
        &value,
        &[
            "tonal.key_edma.scale",
            "tonal.key_temperley.scale",
            "tonal.key_krumhansl.scale",
        ],
    );
    let key_strength = json_number(
        &value,
        &[
            "tonal.key_edma.strength",
            "tonal.key_temperley.strength",
            "tonal.key_krumhansl.strength",
        ],
    );
    let key = match (key_name, scale) {
        (Some(key), Some(scale)) => Some(format!("{key} {scale}")),
        (Some(key), None) => Some(key),
        _ => None,
    };
    Ok(EssentiaTrackAnalysis {
        bpm,
        confidence,
        beats,
        key,
        key_strength,
    })
}

fn apply_essentia_analysis(analysis: &mut TrackAnalysis, external: EssentiaTrackAnalysis) {
    if let Some(bpm) = external
        .bpm
        .filter(|bpm| bpm.is_finite() && (MIN_BPM..=MAX_BPM).contains(bpm))
    {
        analysis.bpm = bpm;
    }
    if !external.beats.is_empty() {
        analysis.beat_positions_seconds = external
            .beats
            .into_iter()
            .filter(|beat| beat.is_finite() && *beat >= 0.0 && *beat <= analysis.duration_seconds)
            .collect();
        if let Some(first) = analysis.beat_positions_seconds.first().copied() {
            analysis.beat_offset_seconds = first;
            analysis.downbeat_offset_seconds = estimate_downbeat_from_beats(
                &analysis.beat_positions_seconds,
                &analysis.energy_curve,
            );
        }
    }
    analysis.bpm_confidence = external
        .confidence
        .filter(|value| value.is_finite())
        .unwrap_or_else(|| beat_grid_confidence(&analysis.beat_positions_seconds))
        .clamp(0.0, 1.0);
    analysis.musical_key = external.key;
    analysis.key_confidence = external.key_strength.map(|value| value.clamp(0.0, 1.0));
    analysis.analysis_backend = "Essentia music extractor + ebur128".to_owned();
}

fn estimate_downbeat_from_beats(beats: &[f32], energy_curve: &[f32]) -> f32 {
    if beats.is_empty() {
        return 0.0;
    }
    let mut best_phase = 0usize;
    let mut best_score = f32::NEG_INFINITY;
    for phase in 0..4 {
        let mut score = 0.0f32;
        let mut count = 0usize;
        for beat in beats.iter().skip(phase).step_by(4) {
            let index = seconds_to_energy_index(*beat).min(energy_curve.len().saturating_sub(1));
            if let Some(value) = energy_curve.get(index) {
                score += *value;
                count += 1;
            }
        }
        if count > 0 {
            score /= count as f32;
        }
        if score > best_score {
            best_score = score;
            best_phase = phase;
        }
    }
    beats.get(best_phase).copied().unwrap_or(beats[0])
}

fn beat_grid_confidence(beats: &[f32]) -> f32 {
    if beats.len() < 4 {
        return 0.0;
    }
    let intervals = beats
        .windows(2)
        .map(|pair| pair[1] - pair[0])
        .collect::<Vec<_>>();
    let mean = intervals.iter().sum::<f32>() / intervals.len() as f32;
    if mean <= 0.0 {
        return 0.0;
    }
    let variance = intervals
        .iter()
        .map(|value| (value - mean).powi(2))
        .sum::<f32>()
        / intervals.len() as f32;
    (1.0 - variance.sqrt() / mean).clamp(0.0, 1.0)
}

fn json_value<'a>(root: &'a serde_json::Value, dotted_path: &str) -> Option<&'a serde_json::Value> {
    if let Some(value) = root.get(dotted_path) {
        return Some(value);
    }
    let mut current = root;
    for part in dotted_path.split('.') {
        current = current.get(part)?;
    }
    Some(current)
}

fn json_number(root: &serde_json::Value, paths: &[&str]) -> Option<f32> {
    paths.iter().find_map(|path| {
        let value = json_value(root, path)?;
        value.as_f64().map(|number| number as f32).or_else(|| {
            value
                .as_array()?
                .first()?
                .as_f64()
                .map(|number| number as f32)
        })
    })
}

fn json_string(root: &serde_json::Value, paths: &[&str]) -> Option<String> {
    paths
        .iter()
        .find_map(|path| json_value(root, path)?.as_str().map(ToOwned::to_owned))
}

fn json_number_array(root: &serde_json::Value, paths: &[&str]) -> Vec<f32> {
    paths
        .iter()
        .find_map(|path| {
            json_value(root, path)?.as_array().map(|values| {
                values
                    .iter()
                    .filter_map(|value| value.as_f64().map(|number| number as f32))
                    .collect::<Vec<_>>()
            })
        })
        .unwrap_or_default()
}

fn run_command_cancellable(mut command: Command, cancel: &AtomicBool) -> Result<ExitStatus> {
    let mut child = command.spawn()?;
    wait_for_child(&mut child, cancel)
}

fn wait_for_child(child: &mut Child, cancel: &AtomicBool) -> Result<ExitStatus> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status);
        }
        if cancel.load(AtomicOrdering::Relaxed) {
            let _ = child.kill();
            let _ = child.wait();
            return Err(anyhow!("DJ mix export cancelled."));
        }
        thread::sleep(Duration::from_millis(50));
    }
}

fn professional_cache_dir() -> PathBuf {
    app_data_dir()
        .unwrap_or_else(env::temp_dir)
        .join("dj-professional-cache")
}

fn file_cache_key(path: &Path, extra: &[&[u8]]) -> Result<u64> {
    let metadata = fs::metadata(path)?;
    let modified = metadata
        .modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or(0);
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    path.to_string_lossy().hash(&mut hasher);
    metadata.len().hash(&mut hasher);
    modified.hash(&mut hasher);
    for value in extra {
        value.hash(&mut hasher);
    }
    Ok(hasher.finish())
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
            if end <= start || end - start < 0.25 {
                return Err(anyhow!(
                    "Favorite range for '{}' must contain at least 0.25 seconds inside track duration.",
                    track.title
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
        let previous = &ordered
            .last()
            .expect("ordered DJ plan always contains the first track")
            .1;
        let next_index = tracks
            .iter()
            .enumerate()
            .min_by(|(_, left), (_, right)| {
                compatibility_cost(previous, &left.1)
                    .partial_cmp(&compatibility_cost(previous, &right.1))
                    .unwrap_or(Ordering::Equal)
            })
            .map(|(index, _)| index)
            .unwrap_or(0);
        ordered.push(tracks.remove(next_index));
    }
    ordered
}

fn compatibility_cost(previous: &TrackAnalysis, candidate: &TrackAnalysis) -> f32 {
    let bpm_cost = (candidate.bpm - previous.bpm).abs();
    let confidence_penalty = (1.0 - candidate.bpm_confidence) * 8.0;
    let harmonic_penalty = match (
        previous.musical_key.as_deref(),
        candidate.musical_key.as_deref(),
    ) {
        (Some(_), Some(_)) if analysis_keys_are_compatible(previous, candidate) => 0.0,
        (Some(_), Some(_)) => 9.0,
        _ => 3.0,
    };
    let previous_energy = representative_energy(previous);
    let candidate_energy = representative_energy(candidate);
    let energy_penalty = (candidate_energy - previous_energy).abs() * 12.0;
    bpm_cost + confidence_penalty + harmonic_penalty + energy_penalty
}

fn representative_energy(analysis: &TrackAnalysis) -> f32 {
    if analysis.energy_curve.is_empty() {
        return 0.0;
    }
    analysis.energy_curve.iter().copied().sum::<f32>() / analysis.energy_curve.len() as f32
}

fn plan_transition_diagnostics(
    tracks: &[PlannedTrack],
    options: DjMixOptions,
    tools: &ProfessionalToolchain,
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
            let recipe = TransitionRecipe::resolve(
                options.bridge_mode,
                transition_seed(current, next),
                transition_should_hit_hard(current, next),
                options.professional_tools && options.stem_separation && tools.demucs.is_some(),
                keys_are_compatible(current, next),
            );
            let mut warnings = Vec::new();
            let mut decisions = vec![
                format!(
                    "Aligned to {}-bar phrase boundary.",
                    options.transition_bars
                ),
                format!(
                    "Tempo target {:.2} BPM; Rubber Band R3 is preferred with built-in WSOLA fallback.",
                    effective_bpm
                ),
                format!("Transition recipe: {}.", recipe.diagnostic_name()),
                "Outgoing and incoming lead vocals are never intentionally layered; the handoff is instrumental between vocal phrases.".to_owned(),
            ];
            if actual_frames < requested_frames {
                decisions.push(format!(
                    "Transition shortened from {:.2} to {:.2} seconds to fit selected sections.",
                    requested_frames as f32 / OUTPUT_SAMPLE_RATE as f32,
                    duration_seconds
                ));
            }
            if options.bass_swap {
                decisions.push("Only one bass stem owns the low end around the handoff.".to_owned());
            }
            if options.stem_separation && tools.demucs.is_some() {
                decisions.push(
                    "Demucs detected: renderer attempts drums, bass, accompaniment and vocal separation with independent phrase gates."
                        .to_owned(),
                );
            } else if options.stem_separation {
                warnings.push(
                    "Stem-aware mixing requested, but Demucs was not detected; full-mix vocal guard used."
                        .to_owned(),
                );
            }
            if current.analysis.musical_key.is_none() || next.analysis.musical_key.is_none() {
                warnings.push(
                    "Harmonic compatibility unavailable; automatic mode avoids assuming a key match."
                        .to_owned(),
                );
            }
            if current.analysis.bpm_confidence < 0.15 || next.analysis.bpm_confidence < 0.15 {
                warnings.push("Low BPM confidence; transition may need manual review.".to_owned());
            }
            if !options.professional_tools {
                warnings.push(
                    "Professional tools disabled; built-in analysis, WSOLA and full-mix fallback used."
                        .to_owned(),
                );
            }
            TransitionDiagnostic {
                from_title: current.track.title.clone(),
                to_title: next.track.title.clone(),
                style: recipe.diagnostic_name().to_owned(),
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
    professional_tools: ProfessionalToolReport,
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
            analysis_backend: track.analysis.analysis_backend.clone(),
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
            "Section labels remain deterministic energy heuristics; Essentia supplies beat and key descriptors, not semantic song structure."
                .to_owned(),
            "Essentia, Rubber Band and Demucs are optional user-installed executables and are not bundled with Audio Orbit."
                .to_owned(),
            "When an external tool fails or is missing, export continues with deterministic built-in fallbacks."
                .to_owned(),
        ],
        professional_tools,
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
        let analysis = analyze_track(&path, &ProfessionalToolchain::default(), &cancel)
            .expect("analyze fixture");
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
    fn favorite_range_accepts_short_usable_selection() {
        let mut track = DjMixTrack::new(PathBuf::from("favorite"), "favorite".to_owned());
        track.section_mode = DjTrackSectionMode::FavoriteRange;
        track.favorite_start_seconds = 50.0;
        track.favorite_end_seconds = 55.0;
        let analysis = test_analysis(120.0);
        let result = select_track_section(&track, &analysis, 60.0, 16);
        assert!(result.is_ok());
    }

    #[test]
    fn favorite_range_rejects_empty_selection() {
        let mut track = DjMixTrack::new(PathBuf::from("favorite"), "favorite".to_owned());
        track.section_mode = DjTrackSectionMode::FavoriteRange;
        track.favorite_start_seconds = 55.0;
        track.favorite_end_seconds = 55.0;
        let analysis = test_analysis(120.0);
        let result = select_track_section(&track, &analysis, 60.0, 16);
        assert!(result.is_err());
    }

    #[test]
    fn harmonic_key_rules_accept_same_relative_and_fifth_keys() {
        assert!(harmonic_keys_are_compatible("C major", "C major"));
        assert!(harmonic_keys_are_compatible("C major", "A minor"));
        assert!(harmonic_keys_are_compatible("C major", "G major"));
        assert!(!harmonic_keys_are_compatible("C major", "F# major"));
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
            beat_positions_seconds: (0..360).map(|beat| beat as f32 * 0.5).collect(),
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
            analysis_backend: "test".to_owned(),
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
