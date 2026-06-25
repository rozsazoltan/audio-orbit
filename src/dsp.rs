use std::f32::consts::PI;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OrbitMode {
    SmoothLeftRight,
    EightStepOrbit,
}

impl OrbitMode {
    pub fn label(self) -> &'static str {
        match self {
            Self::SmoothLeftRight => "Smooth left/right sweep",
            Self::EightStepOrbit => "8-step orbit cue",
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct DspSettings {
    pub output_level_percent: u8,
    pub stereo_width_percent: u8,
    pub orbit_speed_percent: u8,
    pub mode: OrbitMode,
}

impl Default for DspSettings {
    fn default() -> Self {
        Self {
            output_level_percent: 95,
            stereo_width_percent: 100,
            orbit_speed_percent: 100,
            mode: OrbitMode::SmoothLeftRight,
        }
    }
}

pub struct RenderInfo {
    pub duration_seconds: f32,
    pub input_channels: u16,
    pub sample_rate: u32,
    pub output_samples: usize,
}

const BASE_ORBIT_RATE_HZ: f32 = 0.35;
const MAX_INTERAURAL_DELAY_SECONDS: f32 = 0.00065;

pub fn render_orbit_to_stereo(
    input_samples: &[f32],
    input_channels: u16,
    sample_rate: u32,
    settings: DspSettings,
) -> (Vec<f32>, RenderInfo) {
    let channels = input_channels.max(1) as usize;
    let frame_count = input_samples.len() / channels;
    let mut mono = Vec::with_capacity(frame_count);

    for frame in input_samples.chunks_exact(channels) {
        let sum: f32 = frame.iter().copied().sum();
        mono.push(sum / channels as f32);
    }

    let output_level = settings.output_level_percent.min(100) as f32 / 100.0;
    let width = settings.stereo_width_percent.min(100) as f32 / 100.0;
    let speed = (settings.orbit_speed_percent.max(1) as f32 / 100.0).max(0.01);
    let orbit_rate = BASE_ORBIT_RATE_HZ * speed;
    let max_delay_samples = (MAX_INTERAURAL_DELAY_SECONDS * sample_rate as f32).round() as usize;

    let mut output = Vec::with_capacity(frame_count * 2);

    for frame_index in 0..frame_count {
        let time = frame_index as f32 / sample_rate as f32;
        let raw_angle = 2.0 * PI * orbit_rate * time;
        let angle = match settings.mode {
            OrbitMode::SmoothLeftRight => raw_angle,
            OrbitMode::EightStepOrbit => quantize_to_eight_directions(raw_angle),
        };

        let pan = angle.sin() * width;
        let backness = (-angle.cos()).max(0.0);
        let delay_samples = (pan.abs() * max_delay_samples as f32).round() as usize;

        let current_sample = mono[frame_index];
        let delayed_index = frame_index.saturating_sub(delay_samples);
        let delayed_sample = mono[delayed_index];

        let (left_source, right_source) = if pan >= 0.0 {
            (delayed_sample, current_sample)
        } else {
            (current_sample, delayed_sample)
        };

        let (left_gain, right_gain) = equal_power_pan_gains(pan);

        // Back-half positions are intentionally a little softer. This is only a stereo cue,
        // not real HRTF surround, but it makes the 8-step orbit mode audibly different from
        // plain left/right panning.
        let depth_gain = 1.0 - (backness * 0.18);

        let left = (left_source * left_gain * output_level * depth_gain).clamp(-1.0, 1.0);
        let right = (right_source * right_gain * output_level * depth_gain).clamp(-1.0, 1.0);

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

fn equal_power_pan_gains(pan: f32) -> (f32, f32) {
    let pan = pan.clamp(-1.0, 1.0);
    let angle = (pan + 1.0) * PI / 4.0;
    (angle.cos(), angle.sin())
}

fn quantize_to_eight_directions(angle: f32) -> f32 {
    let step = PI / 4.0;
    (angle / step).round() * step
}
