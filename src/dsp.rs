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
            Self::VirtualEightDirectionOrbit => "Virtual 8-direction orbit",
        }
    }

    pub fn description(self) -> &'static str {
        match self {
            Self::SmoothStereoOrbit => "Continuous left/right headphone panning with a stable volume curve.",
            Self::VirtualEightDirectionOrbit => "Experimental stereo cues for left/front/back/right positions. This is not real Dolby/HRTF surround.",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DspSettings {
    pub output_level_percent: u8,
    pub stereo_width_percent: u8,
    pub orbit_speed_percent: u8,
    pub transition_smoothness_percent: u8,
    pub depth_cue_percent: u8,
    pub mode: OrbitMode,
}

impl Default for DspSettings {
    fn default() -> Self {
        Self {
            output_level_percent: 92,
            stereo_width_percent: 100,
            orbit_speed_percent: 75,
            transition_smoothness_percent: 85,
            depth_cue_percent: 60,
            mode: OrbitMode::SmoothStereoOrbit,
        }
    }
}

pub struct RenderInfo {
    pub duration_seconds: f32,
    pub input_channels: u16,
    pub sample_rate: u32,
    pub output_samples: usize,
}

const BASE_ORBIT_RATE_HZ: f32 = 0.22;
const MAX_INTERAURAL_DELAY_SECONDS: f32 = 0.00085;

pub fn render_orbit_to_stereo(
    input_samples: &[f32],
    input_channels: u16,
    sample_rate: u32,
    settings: DspSettings,
) -> (Vec<f32>, RenderInfo) {
    let channels = input_channels.max(1) as usize;
    let frame_count = input_samples.len() / channels;
    let mono = downmix_to_mono(input_samples, channels, frame_count);

    let output_level = settings.output_level_percent.clamp(1, 100) as f32 / 100.0;
    let width = settings.stereo_width_percent.min(100) as f32 / 100.0;
    let speed = settings.orbit_speed_percent.clamp(10, 200) as f32 / 100.0;
    let depth_amount = settings.depth_cue_percent.min(100) as f32 / 100.0;
    let orbit_rate = BASE_ORBIT_RATE_HZ * speed;
    let max_delay_samples = MAX_INTERAURAL_DELAY_SECONDS * sample_rate as f32;

    let smoothness = settings.transition_smoothness_percent.min(100) as f32 / 100.0;
    let smoothing_time_seconds = 0.008 + smoothness * 0.28;
    let smoothing_coeff = (-1.0 / (sample_rate as f32 * smoothing_time_seconds)).exp();

    let mut smoothed_pan = 0.0_f32;
    let mut smoothed_frontness = 1.0_f32;
    let mut smoothed_backness = 0.0_f32;
    let mut rear_filter_state = 0.0_f32;
    let mut output = Vec::with_capacity(frame_count * 2);

    for frame_index in 0..frame_count {
        let time = frame_index as f32 / sample_rate as f32;
        let angle = 2.0 * PI * orbit_rate * time;

        let target = match settings.mode {
            OrbitMode::SmoothStereoOrbit => OrbitPosition {
                pan: angle.sin() * width,
                frontness: 1.0,
                backness: 0.0,
            },
            OrbitMode::VirtualEightDirectionOrbit => virtual_eight_direction_position(angle, width),
        };

        smoothed_pan = smooth_value(smoothed_pan, target.pan, smoothing_coeff);
        smoothed_frontness = smooth_value(smoothed_frontness, target.frontness, smoothing_coeff);
        smoothed_backness = smooth_value(smoothed_backness, target.backness, smoothing_coeff);

        let source_sample = mono[frame_index];
        rear_filter_state = low_pass(rear_filter_state, source_sample, smoothed_backness, depth_amount);

        let depth_mix = smoothed_backness * depth_amount;
        let spatial_sample = source_sample * (1.0 - depth_mix) + rear_filter_state * depth_mix;

        let delay_samples = smoothed_pan.abs() * max_delay_samples;
        let delayed_sample = delayed_sample(&mono, frame_index, delay_samples);

        let (left_source, right_source) = if smoothed_pan >= 0.0 {
            (delayed_sample, spatial_sample)
        } else {
            (spatial_sample, delayed_sample)
        };

        let (mut left_gain, mut right_gain) = equal_power_pan_gains(smoothed_pan);

        if matches!(settings.mode, OrbitMode::VirtualEightDirectionOrbit) {
            let rear_attenuation = 1.0 - (smoothed_backness * depth_amount * 0.22);
            let front_presence = 1.0 + (smoothed_frontness * depth_amount * 0.035);
            left_gain *= rear_attenuation * front_presence;
            right_gain *= rear_attenuation * front_presence;
        }

        let left = soft_limit(left_source * left_gain * output_level);
        let right = soft_limit(right_source * right_gain * output_level);

        output.push(left);
        output.push(right);
    }

    let duration_seconds = if sample_rate == 0 {
        0.0
    } else {
        frame_count as f32 / sample_rate as f32
    };

    (
        output,
        RenderInfo {
            duration_seconds,
            input_channels,
            sample_rate,
            output_samples: frame_count * 2,
        },
    )
}

#[derive(Clone, Copy)]
struct OrbitPosition {
    pan: f32,
    frontness: f32,
    backness: f32,
}

fn virtual_eight_direction_position(angle: f32, width: f32) -> OrbitPosition {
    // Continuous path through eight perceived zones:
    // front-center, right-front, right-mid, right-back, rear-center,
    // left-back, left-mid, left-front.
    let x = angle.sin();
    let y = angle.cos();

    OrbitPosition {
        pan: x * width,
        frontness: y.max(0.0),
        backness: (-y).max(0.0),
    }
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

fn low_pass(previous: f32, input: f32, backness: f32, depth_amount: f32) -> f32 {
    let coefficient = 0.08 + (1.0 - backness * depth_amount) * 0.18;
    previous + (input - previous) * coefficient.clamp(0.04, 0.35)
}

fn soft_limit(value: f32) -> f32 {
    let driven = value * 0.96;
    (driven / (1.0 + driven.abs() * 0.12)).clamp(-1.0, 1.0)
}
