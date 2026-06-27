use crate::spectrum_waveform::spectrum_waveform;
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum OrbitMode {
    SmoothStereoOrbit,
    VirtualEightDirectionOrbit,
}

impl OrbitMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::SmoothStereoOrbit => "Smooth stereo orbit",
            Self::VirtualEightDirectionOrbit => "Virtual 8-direction headphone orbit",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::SmoothStereoOrbit => "Continuous left/right motion with equal-power panning and very soft transitions.",
            Self::VirtualEightDirectionOrbit => "A headphone-only 8-zone illusion: front-left, left, rear-left, rear-center, rear-right, right, front-right, front-center. This is not real Dolby Atmos or true multichannel surround.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DspSettings {
    #[serde(default = "default_orbit_enabled")]
    pub orbit_enabled: bool,
    pub output_level_percent: u8,
    pub stereo_width_percent: u8,
    pub orbit_speed_percent: u8,
    pub transition_smoothness_percent: u8,
    pub depth_cue_percent: u8,
    pub mode: OrbitMode,
    #[serde(default)]
    pub skip_silence_enabled: bool,
    #[serde(default = "default_silence_threshold_seconds")]
    pub silence_threshold_seconds: u8,
    #[serde(default = "default_silence_level_threshold_percent")]
    pub silence_level_threshold_percent: u8,
}

impl Default for DspSettings {
    fn default() -> Self {
        Self {
            orbit_enabled: true,
            output_level_percent: 90,
            stereo_width_percent: 100,
            orbit_speed_percent: 70,
            transition_smoothness_percent: 96,
            depth_cue_percent: 75,
            mode: OrbitMode::SmoothStereoOrbit,
            skip_silence_enabled: false,
            silence_threshold_seconds: default_silence_threshold_seconds(),
            silence_level_threshold_percent: default_silence_level_threshold_percent(),
        }
    }
}

fn default_orbit_enabled() -> bool {
    true
}

fn default_silence_threshold_seconds() -> u8 {
    3
}

fn default_silence_level_threshold_percent() -> u8 {
    1
}

#[derive(Clone, Debug)]
pub struct RenderInfo {
    pub original_duration_seconds: f32,
    pub rendered_duration_seconds: f32,
    pub input_channels: u16,
    pub sample_rate: u32,
    pub waveform: Vec<f32>,
    pub silence_ranges: Vec<(f32, f32)>,
}

const BASE_ORBIT_RATE_HZ: f32 = 0.20;
const MAX_STEREO_DELAY_SECONDS: f32 = 0.00085;
const MAX_SURROUND_DELAY_SECONDS: f32 = 0.00165;
const WAVEFORM_POINTS: usize = 2048;

pub fn render_orbit_to_stereo(
    input_samples: &[f32],
    input_channels: u16,
    sample_rate: u32,
    settings: DspSettings,
    start_seconds: f32,
) -> (Vec<f32>, RenderInfo) {
    let channels = input_channels.max(1) as usize;
    let frame_count = input_samples.len() / channels;
    let mono = downmix_to_mono(input_samples, channels, frame_count);
    let mut start_frame = ((start_seconds.max(0.0) * sample_rate as f32) as usize).min(frame_count);
    let waveform = spectrum_waveform(&mono, sample_rate, WAVEFORM_POINTS);

    let output_level = settings.output_level_percent.clamp(1, 100) as f32 / 100.0;
    let silence_floor = automatic_silence_floor(&mono);
    let skip_ranges = if settings.skip_silence_enabled {
        detect_silence_ranges(
            &mono,
            sample_rate,
            settings.silence_threshold_seconds,
            silence_floor,
        )
    } else {
        Vec::new()
    };

    if let Some((_, end)) = skip_ranges
        .iter()
        .find(|(start, end)| start_frame >= *start && start_frame < *end)
        .copied()
    {
        start_frame = end.min(frame_count);
    }

    let silence_ranges = skip_ranges
        .iter()
        .filter(|(_, end)| *end > start_frame)
        .map(|(start, end)| {
            (
                (*start).max(start_frame) as f32 / sample_rate.max(1) as f32,
                *end as f32 / sample_rate.max(1) as f32,
            )
        })
        .collect::<Vec<_>>();

    if !settings.orbit_enabled {
        let (output, rendered_duration_seconds) = render_plain_stereo(
            input_samples,
            channels,
            &mono,
            start_frame,
            sample_rate,
            output_level,
            &skip_ranges,
        );

        let original_duration_seconds = if sample_rate == 0 {
            0.0
        } else {
            frame_count as f32 / sample_rate as f32
        };

        return (
            output,
            RenderInfo {
                original_duration_seconds,
                rendered_duration_seconds,
                input_channels,
                sample_rate,
                waveform,
                silence_ranges,
            },
        );
    }

    let width = settings.stereo_width_percent.min(100) as f32 / 100.0;
    let speed = settings.orbit_speed_percent.clamp(10, 200) as f32 / 100.0;
    let depth_amount = settings.depth_cue_percent.min(100) as f32 / 100.0;
    let orbit_rate = BASE_ORBIT_RATE_HZ * speed;

    let smoothness = settings.transition_smoothness_percent.min(100) as f32 / 100.0;
    let smoothing_time_seconds = 0.08 + smoothness * 0.70;
    let smoothing_coeff = (-1.0 / (sample_rate.max(1) as f32 * smoothing_time_seconds)).exp();

    let mut smoothed_pan = 0.0_f32;
    let mut smoothed_frontness = 1.0_f32;
    let mut smoothed_backness = 0.0_f32;
    let mut rear_low_pass_state = 0.0_f32;
    let mut front_presence_state = 0.0_f32;
    let mut output = Vec::with_capacity((frame_count.saturating_sub(start_frame)) * 2);
    let mut skip_index = skip_ranges
        .iter()
        .position(|(_, end)| *end > start_frame)
        .unwrap_or(skip_ranges.len());
    let mut frame_index = start_frame;

    while frame_index < frame_count {
        while let Some((_, end)) = skip_ranges.get(skip_index) {
            if frame_index < *end {
                break;
            }
            skip_index += 1;
        }

        if let Some((skip_start, skip_end)) = skip_ranges.get(skip_index).copied() {
            if frame_index >= skip_start && frame_index < skip_end {
                // The waveform marks the exact region we remove from the rendered audio.
                // Jumping the decoded cursor here makes the playhead cross skipped ranges instead of crawling through them.
                frame_index = skip_end.min(frame_count);
                continue;
            }
        }

        let source_sample = mono[frame_index];
        let time = frame_index as f32 / sample_rate.max(1) as f32;
        let angle = 2.0 * PI * orbit_rate * time;

        let target = match settings.mode {
            OrbitMode::SmoothStereoOrbit => OrbitPosition {
                pan: angle.sin() * width,
                frontness: 1.0,
                backness: 0.0,
            },
            OrbitMode::VirtualEightDirectionOrbit => virtual_surround_position(angle, width),
        };

        smoothed_pan = smooth_value(smoothed_pan, target.pan, smoothing_coeff);
        smoothed_frontness = smooth_value(smoothed_frontness, target.frontness, smoothing_coeff);
        smoothed_backness = smooth_value(smoothed_backness, target.backness, smoothing_coeff);

        front_presence_state = high_passish(front_presence_state, source_sample);
        rear_low_pass_state = rear_low_pass(rear_low_pass_state, source_sample, smoothed_backness, depth_amount);

        let (left, right) = match settings.mode {
            OrbitMode::SmoothStereoOrbit => render_smooth_stereo_frame(
                &mono,
                frame_index,
                sample_rate,
                source_sample,
                smoothed_pan,
                output_level,
            ),
            OrbitMode::VirtualEightDirectionOrbit => render_surround_frame(
                &mono,
                frame_index,
                sample_rate,
                source_sample,
                front_presence_state,
                rear_low_pass_state,
                smoothed_pan,
                smoothed_frontness,
                smoothed_backness,
                depth_amount,
                output_level,
            ),
        };

        output.push(left);
        output.push(right);
        frame_index += 1;
    }

    apply_skip_boundary_smoothing(&mut output, sample_rate, start_frame, &skip_ranges);

    let original_duration_seconds = if sample_rate == 0 {
        0.0
    } else {
        frame_count as f32 / sample_rate as f32
    };

    let rendered_duration_seconds = if sample_rate == 0 {
        0.0
    } else {
        (output.len() / 2) as f32 / sample_rate as f32
    };

    (
        output,
        RenderInfo {
            original_duration_seconds,
            rendered_duration_seconds,
            input_channels,
            sample_rate,
            waveform,
            silence_ranges,
        },
    )
}

fn automatic_silence_floor(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.012;
    }

    let mut levels = samples
        .iter()
        .map(|sample| sample.abs())
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if levels.is_empty() {
        return 0.012;
    }

    levels.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let peak = levels.last().copied().unwrap_or(0.0).max(0.001);
    let percentile = |fraction: f32| -> f32 {
        let index = ((levels.len().saturating_sub(1)) as f32 * fraction.clamp(0.0, 1.0)) as usize;
        levels.get(index).copied().unwrap_or(0.0)
    };

    let p08 = percentile(0.08);
    let p18 = percentile(0.18);
    let p35 = percentile(0.35);

    // AIMP-like behavior: the user controls only how long a gap must be. The level is inferred
    // from the track's own noise floor so MP3/AAC dither and tiny decoder noise still count as silence,
    // while genuinely quiet musical passages are not aggressively removed.
    (0.010_f32
        .max(p08 * 5.0)
        .max(p18 * 3.0)
        .max(p35 * 1.35)
        .max(peak * 0.005))
        .clamp(0.006, 0.034)
}

fn detect_silence_ranges(
    mono: &[f32],
    sample_rate: u32,
    threshold_seconds: u8,
    silence_floor: f32,
) -> Vec<(usize, usize)> {
    if mono.is_empty() || sample_rate == 0 {
        return Vec::new();
    }

    let sample_rate_usize = sample_rate.max(1) as usize;
    let window_frames = (sample_rate_usize / 25).max(256); // about 40 ms at common sample rates
    let min_silent_windows = ((threshold_seconds.max(1) as f32 * sample_rate as f32) / window_frames as f32)
        .ceil()
        .max(1.0) as usize;
    let bridge_tolerance_windows = ((sample_rate as f32 * 0.24) / window_frames as f32)
        .ceil()
        .max(1.0) as usize;
    let edge_padding_frames = ((sample_rate as f32 * 0.025) as usize).max(1);

    let mut ranges = Vec::new();
    let mut candidate_start: Option<usize> = None;
    let mut candidate_last_silent_end = 0usize;
    let mut bridge_windows = 0usize;

    for (window_index, chunk) in mono.chunks(window_frames).enumerate() {
        let start = window_index * window_frames;
        let end = (start + chunk.len()).min(mono.len());
        let rms = (chunk.iter().map(|sample| sample * sample).sum::<f32>() / chunk.len().max(1) as f32).sqrt();
        let peak = chunk.iter().map(|sample| sample.abs()).fold(0.0_f32, f32::max);
        let silent = rms <= silence_floor && peak <= silence_floor * 12.0;

        if silent {
            if candidate_start.is_none() {
                candidate_start = Some(start);
            }
            candidate_last_silent_end = end;
            bridge_windows = 0;
            continue;
        }

        if candidate_start.is_some() && bridge_windows < bridge_tolerance_windows && rms <= silence_floor * 2.25 {
            bridge_windows += 1;
            continue;
        }

        if let Some(start) = candidate_start.take() {
            let silent_windows = candidate_last_silent_end.saturating_sub(start) / window_frames;
            if silent_windows >= min_silent_windows {
                push_silence_range(&mut ranges, start, candidate_last_silent_end, edge_padding_frames);
            }
        }
        candidate_last_silent_end = 0;
        bridge_windows = 0;
    }

    if let Some(start) = candidate_start.take() {
        let silent_windows = mono.len().saturating_sub(start) / window_frames;
        if silent_windows >= min_silent_windows {
            push_silence_range(&mut ranges, start, mono.len(), edge_padding_frames);
        }
    }

    ranges
}

fn push_silence_range(ranges: &mut Vec<(usize, usize)>, start: usize, end: usize, padding: usize) {
    let start = start.saturating_add(padding);
    let end = end.saturating_sub(padding);
    if end <= start {
        return;
    }

    if let Some((_, previous_end)) = ranges.last_mut() {
        if start <= previous_end.saturating_add(padding * 2) {
            *previous_end = (*previous_end).max(end);
            return;
        }
    }

    ranges.push((start, end));
}

fn render_plain_stereo(
    input_samples: &[f32],
    channels: usize,
    mono: &[f32],
    start_frame: usize,
    sample_rate: u32,
    output_level: f32,
    skip_ranges: &[(usize, usize)],
) -> (Vec<f32>, f32) {
    let frame_count = mono.len();
    let mut output = Vec::with_capacity((frame_count.saturating_sub(start_frame)) * 2);
    let mut skip_index = skip_ranges
        .iter()
        .position(|(_, end)| *end > start_frame)
        .unwrap_or(skip_ranges.len());
    let mut frame_index = start_frame;

    while frame_index < frame_count {
        while let Some((_, end)) = skip_ranges.get(skip_index) {
            if frame_index < *end {
                break;
            }
            skip_index += 1;
        }

        if let Some((skip_start, skip_end)) = skip_ranges.get(skip_index).copied() {
            if frame_index >= skip_start && frame_index < skip_end {
                frame_index = skip_end.min(frame_count);
                continue;
            }
        }

        let source_sample = mono[frame_index];
        let offset = frame_index * channels;
        let (left, right) = if channels == 1 {
            let sample = input_samples.get(offset).copied().unwrap_or(source_sample);
            (sample, sample)
        } else {
            (
                input_samples.get(offset).copied().unwrap_or(source_sample),
                input_samples.get(offset + 1).copied().unwrap_or(source_sample),
            )
        };

        output.push(left * output_level);
        output.push(right * output_level);
        frame_index += 1;
    }

    apply_skip_boundary_smoothing(&mut output, sample_rate, start_frame, skip_ranges);

    let rendered_duration_seconds = if sample_rate == 0 {
        0.0
    } else {
        (output.len() / 2) as f32 / sample_rate as f32
    };

    (output, rendered_duration_seconds)
}

fn apply_skip_boundary_smoothing(
    samples: &mut [f32],
    sample_rate: u32,
    start_frame: usize,
    skip_ranges: &[(usize, usize)],
) {
    if samples.is_empty() || sample_rate == 0 || skip_ranges.is_empty() {
        return;
    }

    // The cut points are already near silence. This short fade-in only hides decoder residue/clicks;
    // it should not feel like a transition or crossfade.
    let fade_frames = ((sample_rate as f32 * 0.006) as usize).clamp(8, 256);
    let frame_count = samples.len() / 2;
    let mut skipped_before = 0usize;

    for (start, end) in skip_ranges.iter().copied() {
        if end <= start_frame {
            continue;
        }

        let effective_start = start.max(start_frame);
        if end <= effective_start {
            continue;
        }

        let rendered_cut_frame = effective_start
            .saturating_sub(start_frame)
            .saturating_sub(skipped_before)
            .min(frame_count);

        for frame in rendered_cut_frame..(rendered_cut_frame + fade_frames).min(frame_count) {
            let gain = (frame - rendered_cut_frame) as f32 / fade_frames as f32;
            let left = frame * 2;
            samples[left] *= gain;
            if let Some(right) = samples.get_mut(left + 1) {
                *right *= gain;
            }
        }

        skipped_before = skipped_before.saturating_add(end - effective_start);
    }
}


#[derive(Clone, Copy)]
struct OrbitPosition {
    pan: f32,
    frontness: f32,
    backness: f32,
}

fn virtual_surround_position(angle: f32, width: f32) -> OrbitPosition {
    // Continuous version of the 8 named perceived zones. The sound does not jump between
    // eight hard steps; it glides through them with different side/depth cues.
    let side = angle.sin();
    let depth = angle.cos();
    let rear_strength = (-depth).max(0.0);
    let front_strength = depth.max(0.0);

    OrbitPosition {
        pan: side * width,
        frontness: front_strength,
        backness: rear_strength,
    }
}

fn render_smooth_stereo_frame(
    mono: &[f32],
    frame_index: usize,
    sample_rate: u32,
    source_sample: f32,
    pan: f32,
    output_level: f32,
) -> (f32, f32) {
    let delay_samples = pan.abs() * MAX_STEREO_DELAY_SECONDS * sample_rate as f32;
    let delayed_sample = delayed_sample(mono, frame_index, delay_samples);
    let (left_source, right_source) = if pan >= 0.0 {
        (delayed_sample, source_sample)
    } else {
        (source_sample, delayed_sample)
    };
    let (left_gain, right_gain) = equal_power_pan_gains(pan);

    (
        soft_limit(left_source * left_gain * output_level),
        soft_limit(right_source * right_gain * output_level),
    )
}

fn render_surround_frame(
    mono: &[f32],
    frame_index: usize,
    sample_rate: u32,
    source_sample: f32,
    front_presence_state: f32,
    rear_low_pass_state: f32,
    pan: f32,
    frontness: f32,
    backness: f32,
    depth_amount: f32,
    output_level: f32,
) -> (f32, f32) {
    let rear_mix = backness * depth_amount;
    let front_mix = frontness * depth_amount;

    let front_sample = source_sample + front_presence_state * front_mix * 0.30;
    let rear_sample = (source_sample * (1.0 - rear_mix * 0.70)) + (rear_low_pass_state * rear_mix * 1.05);
    let spatial_sample = front_sample * (1.0 - rear_mix) + rear_sample * rear_mix;

    let delay_base = MAX_STEREO_DELAY_SECONDS + MAX_SURROUND_DELAY_SECONDS * rear_mix;
    let delay_samples = pan.abs() * delay_base * sample_rate as f32;
    let delayed_sample = delayed_sample(mono, frame_index, delay_samples);

    let (left_source, right_source) = if pan >= 0.0 {
        (delayed_sample, spatial_sample)
    } else {
        (spatial_sample, delayed_sample)
    };

    let (mut left_gain, mut right_gain) = equal_power_pan_gains(pan);
    let rear_attenuation = 1.0 - rear_mix * 0.30;
    let front_presence = 1.0 + front_mix * 0.12;
    let side_focus = 1.0 + pan.abs() * depth_amount * 0.08;

    left_gain *= rear_attenuation * front_presence * side_focus;
    right_gain *= rear_attenuation * front_presence * side_focus;

    let crossfeed = (0.08 + rear_mix * 0.24).min(0.34);
    let left_mixed = left_source * left_gain + right_source * right_gain * crossfeed;
    let right_mixed = right_source * right_gain + left_source * left_gain * crossfeed;

    (
        soft_limit(left_mixed * output_level),
        soft_limit(right_mixed * output_level),
    )
}

fn downmix_to_mono(input_samples: &[f32], channels: usize, frame_count: usize) -> Vec<f32> {
    let mut mono = Vec::with_capacity(frame_count);

    for frame in input_samples.chunks_exact(channels) {
        let sum: f32 = frame.iter().copied().sum();
        mono.push(sum / channels as f32);
    }

    mono
}

fn smooth_value(previous: f32, target: f32, smoothing_coeff: f32) -> f32 {
    previous * smoothing_coeff + target * (1.0 - smoothing_coeff)
}

fn equal_power_pan_gains(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * PI / 4.0;
    (angle.cos(), angle.sin())
}

fn delayed_sample(samples: &[f32], frame_index: usize, delay_samples: f32) -> f32 {
    if delay_samples <= 0.0 || frame_index == 0 {
        return samples[frame_index];
    }

    let delay_floor = delay_samples.floor() as usize;
    let delay_fraction = delay_samples - delay_floor as f32;
    let index_a = frame_index.saturating_sub(delay_floor);
    let index_b = frame_index.saturating_sub(delay_floor + 1);
    let sample_a = samples[index_a];
    let sample_b = samples[index_b];

    sample_a * (1.0 - delay_fraction) + sample_b * delay_fraction
}

fn rear_low_pass(previous: f32, input: f32, backness: f32, depth_amount: f32) -> f32 {
    let coefficient = 0.025 + (1.0 - backness * depth_amount) * 0.10;
    previous + (input - previous) * coefficient.clamp(0.018, 0.14)
}

fn high_passish(previous: f32, input: f32) -> f32 {
    let low = previous + (input - previous) * 0.06;
    input - low
}

fn soft_limit(value: f32) -> f32 {
    let driven = value * 0.94;
    (driven / (1.0 + driven.abs() * 0.16)).clamp(-1.0, 1.0)
}
