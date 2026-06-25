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
    let start_frame = ((start_seconds.max(0.0) * sample_rate as f32) as usize).min(frame_count);
    let waveform = waveform_peaks(&mono, WAVEFORM_POINTS);

    let output_level = settings.output_level_percent.clamp(1, 100) as f32 / 100.0;
    let silence_floor = silence_floor_from_percent(settings.silence_level_threshold_percent);

    if !settings.orbit_enabled {
        let (output, rendered_duration_seconds) = render_plain_stereo(
            input_samples,
            channels,
            &mono,
            start_frame,
            sample_rate,
            output_level,
            settings.skip_silence_enabled,
            settings.silence_threshold_seconds,
            silence_floor,
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

    let silence_limit = settings.silence_threshold_seconds.max(1) as usize * sample_rate.max(1) as usize;
    let mut consecutive_silent_frames = 0usize;

    let mut smoothed_pan = 0.0_f32;
    let mut smoothed_frontness = 1.0_f32;
    let mut smoothed_backness = 0.0_f32;
    let mut rear_low_pass_state = 0.0_f32;
    let mut front_presence_state = 0.0_f32;
    let mut output = Vec::with_capacity((frame_count.saturating_sub(start_frame)) * 2);

    for frame_index in start_frame..frame_count {
        let source_sample = mono[frame_index];
        let is_silent = source_sample.abs() <= silence_floor;
        if is_silent {
            consecutive_silent_frames += 1;
        } else {
            consecutive_silent_frames = 0;
        }

        if settings.skip_silence_enabled && consecutive_silent_frames > silence_limit {
            continue;
        }

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
    }

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
        },
    )
}

fn silence_floor_from_percent(percent: u8) -> f32 {
    percent.clamp(1, 20) as f32 / 100.0
}

fn render_plain_stereo(
    input_samples: &[f32],
    channels: usize,
    mono: &[f32],
    start_frame: usize,
    sample_rate: u32,
    output_level: f32,
    skip_silence_enabled: bool,
    silence_threshold_seconds: u8,
    silence_floor: f32,
) -> (Vec<f32>, f32) {
    let frame_count = mono.len();
    let silence_limit = silence_threshold_seconds.max(1) as usize * sample_rate.max(1) as usize;
    let mut consecutive_silent_frames = 0usize;
    let mut output = Vec::with_capacity((frame_count.saturating_sub(start_frame)) * 2);

    for frame_index in start_frame..frame_count {
        let source_sample = mono[frame_index];
        let is_silent = source_sample.abs() <= silence_floor;
        if is_silent {
            consecutive_silent_frames += 1;
        } else {
            consecutive_silent_frames = 0;
        }

        if skip_silence_enabled && consecutive_silent_frames > silence_limit {
            continue;
        }

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
    }

    let rendered_duration_seconds = if sample_rate == 0 {
        0.0
    } else {
        (output.len() / 2) as f32 / sample_rate as f32
    };

    (output, rendered_duration_seconds)
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

fn waveform_peaks(samples: &[f32], points: usize) -> Vec<f32> {
    if samples.is_empty() || points == 0 {
        return Vec::new();
    }

    let chunk_size = (samples.len() / points).max(1);
    let mut peaks = Vec::with_capacity(points);

    for chunk in samples.chunks(chunk_size).take(points) {
        let peak = chunk
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0_f32, f32::max)
            .min(1.0);
        peaks.push(peak);
    }

    while peaks.len() < points {
        peaks.push(0.0);
    }

    peaks
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
