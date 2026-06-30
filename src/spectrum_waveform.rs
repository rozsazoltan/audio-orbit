const DEFAULT_BUCKETS_PER_SECOND: usize = 10;

#[derive(Clone, Copy, Debug, Default)]
pub struct SpectrumBucket {
    pub level: f32,
}

/// Build an AIMP-style overview waveform from decoded PCM samples.
///
/// This is an amplitude/peak envelope for a seek bar, not a frequency spectrum.
/// It deliberately avoids per-frame full-height normalization so loud mastered
/// music and live radio do not turn into a solid wall of identical bars.
pub fn spectrum_waveform(samples: &[f32], sample_rate: u32, points: usize) -> (Vec<f32>, Vec<f32>) {
    if samples.is_empty() || sample_rate == 0 || points == 0 {
        return (Vec::new(), Vec::new());
    }

    let chunk_size = (samples.len() / points).max(1);
    let mut levels = Vec::with_capacity(points.min(samples.len()));
    let mut previous_level = 0.0_f32;

    for chunk in samples.chunks(chunk_size).take(points) {
        let bucket = analyze_amplitude_bucket(chunk, previous_level, AnalyzerProfile::Overview);
        previous_level = bucket.level;
        levels.push(bucket.level);
    }

    while levels.len() < points {
        levels.push(0.0);
    }

    gently_normalize_overview(&mut levels);
    smooth_overview_levels(&mut levels);

    // No secondary RGB/spectrum lane. The renderer paints this one lane as
    // gray/blue/yellow depending on playback and silence-skip state.
    (levels, Vec::new())
}

pub struct LiveSpectrumAnalyzer {
    hop_size: usize,
    samples_since_bucket: usize,
    bucket_peak: f32,
    bucket_sum_sq: f64,
    bucket_sum_abs: f64,
    bucket_diff_sum: f64,
    bucket_count: usize,
    previous_sample: f32,
    previous_level: f32,
}

impl LiveSpectrumAnalyzer {
    pub fn new(sample_rate: u32, buckets_per_second: usize) -> Self {
        let sample_rate = sample_rate.max(1) as usize;
        let buckets_per_second = buckets_per_second.max(1).max(DEFAULT_BUCKETS_PER_SECOND / 2);
        let hop_size = (sample_rate / buckets_per_second).max(1);

        Self {
            hop_size,
            samples_since_bucket: 0,
            bucket_peak: 0.0,
            bucket_sum_sq: 0.0,
            bucket_sum_abs: 0.0,
            bucket_diff_sum: 0.0,
            bucket_count: 0,
            previous_sample: 0.0,
            previous_level: 0.0,
        }
    }

    pub fn push_sample(&mut self, sample: f32) -> Option<SpectrumBucket> {
        let sample = sample.clamp(-1.0, 1.0);
        let abs = sample.abs();
        self.bucket_peak = self.bucket_peak.max(abs);
        self.bucket_sum_sq += (sample as f64) * (sample as f64);
        self.bucket_sum_abs += abs as f64;
        self.bucket_diff_sum += (sample - self.previous_sample).abs().min(1.0) as f64;
        self.previous_sample = sample;
        self.bucket_count += 1;
        self.samples_since_bucket += 1;

        if self.samples_since_bucket < self.hop_size {
            return None;
        }

        let level = analyze_bucket_stats(
            self.bucket_peak,
            self.bucket_sum_sq,
            self.bucket_sum_abs,
            self.bucket_diff_sum,
            self.bucket_count,
            self.previous_level,
            AnalyzerProfile::LiveRadio,
        );

        self.samples_since_bucket = 0;
        self.bucket_peak = 0.0;
        self.bucket_sum_sq = 0.0;
        self.bucket_sum_abs = 0.0;
        self.bucket_diff_sum = 0.0;
        self.bucket_count = 0;
        self.previous_level = level;

        Some(SpectrumBucket { level })
    }
}

#[derive(Clone, Copy)]
enum AnalyzerProfile {
    Overview,
    LiveRadio,
}

fn analyze_amplitude_bucket(samples: &[f32], previous_level: f32, profile: AnalyzerProfile) -> SpectrumBucket {
    if samples.is_empty() {
        return SpectrumBucket::default();
    }

    let mut peak = 0.0_f32;
    let mut sum_sq = 0.0_f64;
    let mut sum_abs = 0.0_f64;
    let mut diff_sum = 0.0_f64;
    let mut previous = samples[0].clamp(-1.0, 1.0);

    for sample in samples.iter().copied() {
        let sample = sample.clamp(-1.0, 1.0);
        let abs = sample.abs();
        peak = peak.max(abs);
        sum_sq += (sample as f64) * (sample as f64);
        sum_abs += abs as f64;
        diff_sum += (sample - previous).abs().min(1.0) as f64;
        previous = sample;
    }

    SpectrumBucket {
        level: analyze_bucket_stats(peak, sum_sq, sum_abs, diff_sum, samples.len(), previous_level, profile),
    }
}

fn analyze_bucket_stats(
    peak: f32,
    sum_sq: f64,
    sum_abs: f64,
    diff_sum: f64,
    count: usize,
    previous_level: f32,
    profile: AnalyzerProfile,
) -> f32 {
    let count = count.max(1) as f64;
    let rms = (sum_sq / count).sqrt() as f32;
    let average_abs = (sum_abs / count) as f32;
    let transient = (diff_sum / count).sqrt() as f32;
    let crest = (peak - rms).max(0.0);

    let rms_db = amplitude_to_db(rms);
    let peak_db = amplitude_to_db(peak);
    let avg_db = amplitude_to_db(average_abs);

    let (floor_db, ceiling_db, max_level, curve, attack, release) = match profile {
        AnalyzerProfile::Overview => (-52.0, -5.0, 0.82, 1.22, 0.58, 0.34),
        AnalyzerProfile::LiveRadio => (-48.0, -4.5, 0.70, 1.42, 0.42, 0.12),
    };

    let loudness = db_to_unit(rms_db, floor_db, ceiling_db, curve);
    let body = db_to_unit(avg_db, floor_db - 2.0, ceiling_db - 2.0, curve * 0.92);
    let peak_body = db_to_unit(peak_db, floor_db + 4.0, ceiling_db + 1.5, curve * 1.05);
    let transient_body = transient.clamp(0.0, 1.0).powf(0.78);
    let crest_body = crest.clamp(0.0, 1.0).powf(0.95);

    let mut raw = loudness * 0.42
        + body * 0.24
        + peak_body * 0.18
        + transient_body * 0.10
        + crest_body * 0.06;

    // Keep true silence close to the center line, but do not hide low musical
    // detail completely. This is the visual difference between a waveform and a
    // saturated VU meter.
    if rms < 0.000_9 && peak < 0.004 {
        raw = 0.0;
    } else {
        raw = raw.clamp(0.018, max_level);
    }

    let smoothed = smooth_attack_release(raw, previous_level, attack, release);
    smoothed.clamp(0.0, max_level)
}

fn gently_normalize_overview(levels: &mut [f32]) {
    if levels.len() < 4 {
        return;
    }

    let mut sorted = levels
        .iter()
        .copied()
        .filter(|value| value.is_finite())
        .collect::<Vec<_>>();
    if sorted.is_empty() {
        return;
    }
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));

    let percentile = |pct: f32| -> f32 {
        let index = ((sorted.len().saturating_sub(1)) as f32 * pct.clamp(0.0, 1.0)).round() as usize;
        sorted.get(index).copied().unwrap_or(0.0)
    };

    let floor = percentile(0.10).min(0.16);
    let ceiling = percentile(0.96).max(floor + 0.18);
    let range = (ceiling - floor).max(0.10);

    for value in levels.iter_mut() {
        let original = (*value).clamp(0.0, 0.82);
        if original <= 0.003 {
            *value = 0.0;
            continue;
        }
        let normalized = ((original - floor) / range).clamp(0.0, 1.0).powf(1.14) * 0.78;
        *value = (original * 0.64 + normalized * 0.36).clamp(0.0, 0.82);
    }
}

fn smooth_overview_levels(levels: &mut [f32]) {
    if levels.len() < 3 {
        return;
    }

    let mut forward = levels.to_vec();
    for index in 1..forward.len() {
        let previous = forward[index - 1];
        let current = forward[index];
        forward[index] = if current > previous {
            previous * 0.28 + current * 0.72
        } else {
            previous * 0.46 + current * 0.54
        };
    }

    for index in (0..levels.len() - 1).rev() {
        levels[index] = (forward[index] * 0.82 + forward[index + 1] * 0.18).clamp(0.0, 0.82);
    }
    if let Some(last) = levels.last_mut() {
        *last = forward.last().copied().unwrap_or(*last).clamp(0.0, 0.82);
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

fn amplitude_to_db(value: f32) -> f32 {
    20.0 * value.max(0.000_001).log10()
}

fn db_to_unit(db: f32, floor_db: f32, ceiling_db: f32, curve: f32) -> f32 {
    ((db - floor_db) / (ceiling_db - floor_db))
        .clamp(0.0, 1.0)
        .powf(curve)
}
