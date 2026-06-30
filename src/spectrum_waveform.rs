const DEFAULT_BUCKETS_PER_SECOND: usize = 18;
const MIN_ANALYSIS_WINDOW: usize = 256;
const MAX_ANALYSIS_WINDOW: usize = 4096;

#[derive(Clone, Copy, Debug, Default)]
pub struct SpectrumBucket {
    pub level: f32,
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}

/// Build an AIMP-style overview waveform from decoded PCM samples.
///
/// This intentionally renders a single perceptual amplitude lane instead of a
/// multicolor spectrum. The UI paints unplayed audio gray, played audio blue,
/// and silence-skip regions yellow. The analyzer combines peak, RMS, crest, and
/// short-term transient energy so the bars follow musical rhythm without turning
/// every loud passage into a full-height wall.
pub fn spectrum_waveform(samples: &[f32], sample_rate: u32, points: usize) -> (Vec<f32>, Vec<f32>) {
    if samples.is_empty() || sample_rate == 0 || points == 0 {
        return (Vec::new(), Vec::new());
    }

    let chunk_size = (samples.len() / points).max(1);
    let mut levels = Vec::with_capacity(points.min(samples.len()));
    let mut previous_level = 0.0_f32;

    for chunk in samples.chunks(chunk_size).take(points) {
        let bucket = analyze_amplitude_bucket(chunk, previous_level);
        previous_level = bucket.level;
        levels.push(bucket.level);
    }

    while levels.len() < points {
        levels.push(0.0);
    }

    normalize_waveform_levels(&mut levels);
    smooth_waveform_levels(&mut levels);

    // Kept for backwards compatibility with the cached track metadata schema.
    // The current renderer ignores these packed values because Audio Orbit now
    // uses a clean one-color AIMP-style lane.
    let mut packed = Vec::with_capacity(levels.len() * 3);
    for level in &levels {
        packed.extend_from_slice(&[*level, *level, *level]);
    }

    (levels, packed)
}

pub struct LiveSpectrumAnalyzer {
    window: Vec<f32>,
    window_pos: usize,
    filled: usize,
    hop_size: usize,
    samples_since_bucket: usize,
    previous_bucket: SpectrumBucket,
    adaptive_floor: f32,
    adaptive_peak: f32,
}

impl LiveSpectrumAnalyzer {
    pub fn new(sample_rate: u32, buckets_per_second: usize) -> Self {
        let sample_rate = sample_rate.max(1) as usize;
        let buckets_per_second = buckets_per_second.max(1).max(DEFAULT_BUCKETS_PER_SECOND / 2);
        let hop_size = (sample_rate / buckets_per_second).max(1);
        let window_size = (hop_size * 4)
            .clamp(MIN_ANALYSIS_WINDOW, MAX_ANALYSIS_WINDOW)
            .next_power_of_two();

        Self {
            window: vec![0.0; window_size],
            window_pos: 0,
            filled: 0,
            hop_size,
            samples_since_bucket: 0,
            previous_bucket: SpectrumBucket::default(),
            adaptive_floor: 0.018,
            adaptive_peak: 0.32,
        }
    }

    pub fn push_sample(&mut self, sample: f32) -> Option<SpectrumBucket> {
        self.window[self.window_pos] = sample.clamp(-1.0, 1.0);
        self.window_pos = (self.window_pos + 1) % self.window.len();
        self.filled = (self.filled + 1).min(self.window.len());
        self.samples_since_bucket += 1;

        if self.filled < self.window.len() || self.samples_since_bucket < self.hop_size {
            return None;
        }

        self.samples_since_bucket = 0;
        let mut ordered = Vec::with_capacity(self.window.len());
        ordered.extend_from_slice(&self.window[self.window_pos..]);
        ordered.extend_from_slice(&self.window[..self.window_pos]);

        let raw = analyze_amplitude_bucket(&ordered, self.previous_bucket.level);
        let normalized = self.normalize_live_level(raw.level);
        let level = smooth_attack_release(normalized, self.previous_bucket.level, 0.38, 0.12);
        let bucket = SpectrumBucket {
            level,
            low: level,
            mid: level,
            high: level,
        };
        self.previous_bucket = bucket;
        Some(bucket)
    }

    fn normalize_live_level(&mut self, level: f32) -> f32 {
        let level = level.clamp(0.0, 1.0);
        self.adaptive_floor = if level < self.adaptive_floor {
            self.adaptive_floor * 0.94 + level * 0.06
        } else {
            self.adaptive_floor * 0.999 + level * 0.001
        };
        self.adaptive_peak = if level > self.adaptive_peak {
            self.adaptive_peak * 0.78 + level * 0.22
        } else {
            self.adaptive_peak * 0.998 + level * 0.002
        };

        let floor = self.adaptive_floor.min(0.22);
        let peak = self.adaptive_peak.max(floor + 0.18);
        let normalized = ((level - floor) / (peak - floor)).clamp(0.0, 1.0);
        (normalized.powf(1.08) * 0.88).clamp(0.0, 0.92)
    }
}

fn analyze_amplitude_bucket(samples: &[f32], previous_level: f32) -> SpectrumBucket {
    if samples.is_empty() {
        return SpectrumBucket::default();
    }

    let mut min_sample = 1.0_f32;
    let mut max_sample = -1.0_f32;
    let mut sum_sq = 0.0_f64;
    let mut diff_sum = 0.0_f64;
    let mut previous = samples[0].clamp(-1.0, 1.0);

    for sample in samples.iter().copied() {
        let sample = sample.clamp(-1.0, 1.0);
        min_sample = min_sample.min(sample);
        max_sample = max_sample.max(sample);
        sum_sq += (sample as f64) * (sample as f64);
        diff_sum += ((sample - previous).abs() as f64).min(1.0);
        previous = sample;
    }

    let count = samples.len().max(1) as f64;
    let rms = (sum_sq / count).sqrt() as f32;
    let peak = max_sample.abs().max(min_sample.abs()).clamp(0.0, 1.0);
    let span = ((max_sample - min_sample).abs() * 0.5).clamp(0.0, 1.0);
    let crest = (peak - rms).max(0.0);
    let transient = (diff_sum / count).sqrt() as f32;
    let loudness = db_to_unit(20.0 * rms.max(0.000_001).log10(), -56.0, -5.0, 1.08);

    let raw_level = loudness * 0.44
        + span.powf(0.88) * 0.28
        + peak.powf(0.92) * 0.16
        + transient.clamp(0.0, 1.0).powf(0.75) * 0.08
        + crest.clamp(0.0, 1.0) * 0.04;
    let level = smooth_attack_release(raw_level.clamp(0.0, 1.0), previous_level, 0.58, 0.24);

    SpectrumBucket {
        level,
        low: level,
        mid: level,
        high: level,
    }
}

fn normalize_waveform_levels(levels: &mut [f32]) {
    if levels.len() < 4 {
        return;
    }

    let mut sorted = levels.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let floor = sorted[((sorted.len() - 1) as f32 * 0.06) as usize].min(0.18);
    let ceiling = sorted[((sorted.len() - 1) as f32 * 0.975) as usize].max(floor + 0.18);
    let range = (ceiling - floor).max(0.08);

    for value in levels.iter_mut() {
        let original = (*value).clamp(0.0, 1.0);
        let normalized = ((original - floor) / range).clamp(0.0, 0.96).powf(1.06);
        *value = (normalized * 0.62 + original.powf(0.96) * 0.38).clamp(0.0, 0.96);
    }
}

fn smooth_waveform_levels(levels: &mut [f32]) {
    if levels.len() < 3 {
        return;
    }

    let mut forward = levels.to_vec();
    for index in 1..forward.len() {
        let previous = forward[index - 1];
        let current = forward[index];
        forward[index] = if current > previous {
            previous * 0.20 + current * 0.80
        } else {
            previous * 0.58 + current * 0.42
        };
    }

    for index in (0..levels.len() - 1).rev() {
        levels[index] = (forward[index] * 0.74 + forward[index + 1] * 0.26).clamp(0.0, 0.96);
    }
    if let Some(last) = levels.last_mut() {
        *last = forward.last().copied().unwrap_or(*last).clamp(0.0, 0.96);
    }
}

fn smooth_attack_release(target: f32, previous: f32, attack: f32, release: f32) -> f32 {
    let target = target.clamp(0.0, 1.0);
    let previous = previous.clamp(0.0, 1.0);
    if target > previous {
        previous * (1.0 - attack) + target * attack
    } else {
        previous * (1.0 - release) + target * release
    }
}

fn db_to_unit(db: f32, floor_db: f32, ceiling_db: f32, curve: f32) -> f32 {
    ((db - floor_db) / (ceiling_db - floor_db))
        .clamp(0.0, 1.0)
        .powf(curve)
}
