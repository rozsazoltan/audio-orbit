use rustfft::{num_complex::Complex, FftPlanner};

const DEFAULT_FFT_SIZE: usize = 4096;
const LIVE_FFT_SIZE: usize = 2048;
const MIN_FREQUENCY_HZ: f32 = 24.0;
const MAX_FREQUENCY_HZ: f32 = 18_000.0;
const LOW_SPLIT_HZ: f32 = 250.0;
const MID_SPLIT_HZ: f32 = 4_000.0;
const ANALYSIS_BANDS: usize = 36;

#[derive(Clone, Copy, Debug, Default)]
pub struct SpectrumBucket {
    pub level: f32,
    pub low: f32,
    pub mid: f32,
    pub high: f32,
}


/// Build a DJ-player-style overview waveform.
///
/// The visible amplitude uses min/max + RMS like classic waveform generators, while
/// the companion vector is packed as low/mid/high triples so the renderer can draw
/// a stacked RGB-style frequency profile instead of a single loudness-only wall.
pub fn spectrum_waveform(samples: &[f32], sample_rate: u32, points: usize) -> (Vec<f32>, Vec<f32>) {
    if samples.is_empty() || sample_rate == 0 || points == 0 {
        return (Vec::new(), Vec::new());
    }

    let chunk_size = (samples.len() / points).max(1);
    let mut analyzer = SpectrumAnalyzer::new(sample_rate, DEFAULT_FFT_SIZE);
    let mut levels = Vec::with_capacity(points);
    let mut bands = Vec::with_capacity(points * 3);

    for chunk in samples.chunks(chunk_size).take(points) {
        let bucket = analyzer.analyze_chunk(chunk);
        levels.push(bucket.level);
        bands.extend_from_slice(&[bucket.low, bucket.mid, bucket.high]);
    }

    while levels.len() < points {
        levels.push(0.0);
        bands.extend_from_slice(&[0.0, 0.0, 0.0]);
    }

    normalize_waveform_levels(&mut levels);
    normalize_packed_bands(&mut bands);
    smooth_packed_bands(&mut bands);

    (levels, bands)
}

pub struct LiveSpectrumAnalyzer {
    fft_size: usize,
    hop_size: usize,
    ring: Vec<f32>,
    ring_pos: usize,
    filled: usize,
    samples_since_bucket: usize,
    analyzer: SpectrumAnalyzer,
    previous_bucket: SpectrumBucket,
    adaptive_floor: f32,
    adaptive_peak: f32,
}

impl LiveSpectrumAnalyzer {
    pub fn new(sample_rate: u32, buckets_per_second: usize) -> Self {
        let sample_rate = sample_rate.max(1);
        let fft_size = LIVE_FFT_SIZE.min(sample_rate as usize).max(512).next_power_of_two();
        let hop_size = (sample_rate as usize / buckets_per_second.max(1)).max(1);
        Self {
            fft_size,
            hop_size,
            ring: vec![0.0; fft_size],
            ring_pos: 0,
            filled: 0,
            samples_since_bucket: 0,
            analyzer: SpectrumAnalyzer::new(sample_rate, fft_size),
            previous_bucket: SpectrumBucket {
                level: 0.0,
                low: 0.0,
                mid: 0.0,
                high: 0.0,
            },
            adaptive_floor: 0.030,
            adaptive_peak: 0.26,
        }
    }

    pub fn push_sample(&mut self, sample: f32) -> Option<SpectrumBucket> {
        self.ring[self.ring_pos] = sample.clamp(-1.0, 1.0);
        self.ring_pos = (self.ring_pos + 1) % self.fft_size;
        self.filled = (self.filled + 1).min(self.fft_size);
        self.samples_since_bucket += 1;

        if self.filled < self.fft_size || self.samples_since_bucket < self.hop_size {
            return None;
        }

        self.samples_since_bucket = 0;
        let mut ordered = Vec::with_capacity(self.fft_size);
        ordered.extend_from_slice(&self.ring[self.ring_pos..]);
        ordered.extend_from_slice(&self.ring[..self.ring_pos]);

        let target = self.analyzer.analyze_window(&ordered);
        let adaptive_level = self.normalize_live_level(target.level);
        let level = smooth_attack_release(adaptive_level, self.previous_bucket.level, 0.62, 0.18);

        let bucket = SpectrumBucket {
            level: level.clamp(0.0, 1.0),
            low: smooth_attack_release(target.low, self.previous_bucket.low, 0.54, 0.22),
            mid: smooth_attack_release(target.mid, self.previous_bucket.mid, 0.54, 0.22),
            high: smooth_attack_release(target.high, self.previous_bucket.high, 0.58, 0.24),
        };
        self.previous_bucket = bucket;
        Some(bucket)
    }

    fn normalize_live_level(&mut self, level: f32) -> f32 {
        let level = level.clamp(0.0, 1.0);
        self.adaptive_floor = if level < self.adaptive_floor {
            self.adaptive_floor * 0.90 + level * 0.10
        } else {
            self.adaptive_floor * 0.9985 + level * 0.0015
        };
        self.adaptive_peak = if level > self.adaptive_peak {
            self.adaptive_peak * 0.70 + level * 0.30
        } else {
            self.adaptive_peak * 0.996 + level * 0.004
        };

        let floor = self.adaptive_floor.min(0.24);
        let peak = self.adaptive_peak.max(floor + 0.12);
        let normalized = ((level - floor) / (peak - floor)).clamp(0.0, 1.0);
        normalized.powf(0.86) * 0.94
    }
}

struct SpectrumAnalyzer {
    sample_rate: u32,
    fft_size: usize,
    window: Vec<f32>,
    buffer: Vec<Complex<f32>>,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
    previous_bands: Vec<f32>,
}

impl SpectrumAnalyzer {
    fn new(sample_rate: u32, fft_size: usize) -> Self {
        let fft_size = fft_size.max(512).next_power_of_two();
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(fft_size);
        let window = hann_window(fft_size);
        Self {
            sample_rate: sample_rate.max(1),
            fft_size,
            window,
            buffer: vec![Complex::new(0.0, 0.0); fft_size],
            fft,
            previous_bands: vec![0.0; ANALYSIS_BANDS],
        }
    }

    fn analyze_chunk(&mut self, chunk: &[f32]) -> SpectrumBucket {
        if chunk.is_empty() {
            return SpectrumBucket::default();
        }

        if chunk.len() <= self.fft_size {
            return self.analyze_window(chunk);
        }

        let step = (self.fft_size / 2).max(1);
        let mut sum = SpectrumBucket::default();
        let mut strongest = SpectrumBucket::default();
        let mut count = 0usize;
        let mut offset = 0usize;

        while offset < chunk.len() {
            let end = (offset + self.fft_size).min(chunk.len());
            let bucket = self.analyze_window(&chunk[offset..end]);
            sum.level += bucket.level;
            sum.low += bucket.low;
            sum.mid += bucket.mid;
            sum.high += bucket.high;
            if bucket.level > strongest.level {
                strongest = bucket;
            }
            count += 1;
            if end == chunk.len() {
                break;
            }
            offset += step;
        }

        if count == 0 {
            SpectrumBucket::default()
        } else {
            let inv = 1.0 / count as f32;
            SpectrumBucket {
                level: (sum.level * inv * 0.62 + strongest.level * 0.38).clamp(0.0, 1.0),
                low: (sum.low * inv * 0.70 + strongest.low * 0.30).clamp(0.0, 1.0),
                mid: (sum.mid * inv * 0.70 + strongest.mid * 0.30).clamp(0.0, 1.0),
                high: (sum.high * inv * 0.70 + strongest.high * 0.30).clamp(0.0, 1.0),
            }
        }
    }

    fn analyze_window(&mut self, samples: &[f32]) -> SpectrumBucket {
        for value in &mut self.buffer {
            *value = Complex::new(0.0, 0.0);
        }

        let sample_count = samples.len().min(self.fft_size);
        if sample_count == 0 {
            return SpectrumBucket::default();
        }

        let mut time_energy = 0.0_f64;
        let mut min_sample = 1.0_f32;
        let mut max_sample = -1.0_f32;
        for index in 0..sample_count {
            let sample = samples[index].clamp(-1.0, 1.0);
            time_energy += (sample as f64) * (sample as f64);
            min_sample = min_sample.min(sample);
            max_sample = max_sample.max(sample);
            self.buffer[index].re = sample * self.window[index];
        }

        self.fft.process(&mut self.buffer);

        let rms = (time_energy / sample_count as f64).sqrt() as f32;
        let peak = max_sample.abs().max(min_sample.abs()).clamp(0.0, 1.0);
        let min_max_span = ((max_sample - min_sample).abs() * 0.5).clamp(0.0, 1.0);
        let loudness = db_to_unit(20.0 * rms.max(0.000_001).log10(), -58.0, -4.0, 1.16);
        let profile = self.perceptual_spectral_profile();
        let strongest_band = profile.low.max(profile.mid).max(profile.high);
        let contrast = (profile.strongest - profile.average).clamp(0.0, 1.0);
        let flux = profile.flux;

        let level = min_max_span * 0.36
            + peak.powf(0.72) * 0.20
            + loudness * 0.20
            + strongest_band * 0.15
            + contrast * 0.05
            + flux * 0.04;

        SpectrumBucket {
            level: level.clamp(0.0, 1.0),
            low: profile.low,
            mid: profile.mid,
            high: profile.high,
        }
    }

    fn perceptual_spectral_profile(&mut self) -> SpectralProfile {
        let nyquist = self.sample_rate as f32 / 2.0;
        let max_frequency = MAX_FREQUENCY_HZ.min(nyquist.max(MIN_FREQUENCY_HZ));
        let mut bands = Vec::with_capacity(ANALYSIS_BANDS);

        for index in 0..ANALYSIS_BANDS {
            let start_ratio = index as f32 / ANALYSIS_BANDS as f32;
            let end_ratio = (index + 1) as f32 / ANALYSIS_BANDS as f32;
            let low = log_lerp(MIN_FREQUENCY_HZ, max_frequency, start_ratio);
            let high = log_lerp(MIN_FREQUENCY_HZ, max_frequency, end_ratio);
            bands.push((low, high, self.band_level(low, high)));
        }

        let mut weighted_sum = 0.0_f32;
        let mut weight_sum = 0.0_f32;
        let mut strongest = 0.0_f32;
        let mut low_sum = 0.0_f32;
        let mut low_weight = 0.0_f32;
        let mut mid_sum = 0.0_f32;
        let mut mid_weight = 0.0_f32;
        let mut high_sum = 0.0_f32;
        let mut high_weight = 0.0_f32;

        for (low_hz, high_hz, band) in bands.iter().copied() {
            let center_hz = (low_hz * high_hz).sqrt();
            let music_presence_weight = if center_hz < 80.0 {
                0.78
            } else if center_hz < 12_000.0 {
                1.0
            } else {
                0.84
            };
            weighted_sum += band * music_presence_weight;
            weight_sum += music_presence_weight;
            strongest = strongest.max(band);

            if center_hz < LOW_SPLIT_HZ {
                low_sum += band * music_presence_weight;
                low_weight += music_presence_weight;
            } else if center_hz < MID_SPLIT_HZ {
                mid_sum += band * music_presence_weight;
                mid_weight += music_presence_weight;
            } else {
                high_sum += band * music_presence_weight;
                high_weight += music_presence_weight;
            }
        }

        let average = if weight_sum > 0.0 { weighted_sum / weight_sum } else { 0.0 };
        let low = if low_weight > 0.0 { low_sum / low_weight } else { 0.0 };
        let mid = if mid_weight > 0.0 { mid_sum / mid_weight } else { 0.0 };
        let high = if high_weight > 0.0 { high_sum / high_weight } else { 0.0 };

        let mut positive_flux = 0.0_f32;
        let mut flux_count = 0usize;
        for ((_, _, band), previous) in bands.iter().zip(self.previous_bands.iter()) {
            positive_flux += (*band - *previous).max(0.0);
            flux_count += 1;
        }
        self.previous_bands = bands.iter().map(|(_, _, band)| *band).collect();
        let flux = if flux_count == 0 {
            0.0
        } else {
            (positive_flux / flux_count as f32 * 2.6).clamp(0.0, 1.0)
        };

        SpectralProfile {
            average,
            strongest,
            flux,
            low: low.clamp(0.0, 1.0),
            mid: mid.clamp(0.0, 1.0),
            high: high.clamp(0.0, 1.0),
        }
    }

    fn band_level(&self, low_hz: f32, high_hz: f32) -> f32 {
        let bin_hz = self.sample_rate as f32 / self.fft_size as f32;
        if bin_hz <= 0.0 || high_hz <= low_hz {
            return 0.0;
        }

        let start = (low_hz / bin_hz).floor().max(1.0) as usize;
        let end = (high_hz / bin_hz).ceil().min((self.fft_size / 2) as f32) as usize;
        if end <= start {
            return 0.0;
        }

        let mut sum = 0.0_f64;
        let mut count = 0usize;
        for bin in start..end {
            let complex = self.buffer[bin];
            let frequency = bin as f32 * bin_hz;
            let magnitude = complex.norm() / self.fft_size as f32;
            let perceptual_weight = equal_loudness_visual_weight(frequency);
            let weighted = magnitude * perceptual_weight;
            sum += (weighted as f64) * (weighted as f64);
            count += 1;
        }

        if count == 0 {
            return 0.0;
        }

        let energy = (sum / count as f64).sqrt() as f32;
        let db = 20.0 * energy.max(0.000_001).log10();
        db_to_unit(db, -82.0, -12.0, 1.02)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SpectralProfile {
    average: f32,
    strongest: f32,
    flux: f32,
    low: f32,
    mid: f32,
    high: f32,
}

fn hann_window(size: usize) -> Vec<f32> {
    if size <= 1 {
        return vec![1.0; size];
    }

    (0..size)
        .map(|index| {
            let phase = std::f32::consts::TAU * index as f32 / (size - 1) as f32;
            0.5 - 0.5 * phase.cos()
        })
        .collect()
}

fn log_lerp(min: f32, max: f32, t: f32) -> f32 {
    let min = min.max(1.0);
    let max = max.max(min + 1.0);
    (min.ln() + (max.ln() - min.ln()) * t.clamp(0.0, 1.0)).exp()
}

fn normalize_waveform_levels(levels: &mut [f32]) {
    if levels.len() < 4 {
        return;
    }

    let mut sorted = levels.to_vec();
    sorted.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
    let floor = sorted[((sorted.len() - 1) as f32 * 0.04) as usize].min(0.18);
    let ceiling = sorted[((sorted.len() - 1) as f32 * 0.985) as usize].max(floor + 0.12);
    let range = (ceiling - floor).max(0.08);

    for value in levels.iter_mut() {
        let original = (*value).clamp(0.0, 1.0);
        let normalized = ((original - floor) / range).clamp(0.006, 0.98).powf(0.82);
        *value = (normalized * 0.78 + original.powf(0.78) * 0.22).clamp(0.0, 1.0);
    }
}

fn normalize_packed_bands(bands: &mut [f32]) {
    if bands.len() < 12 {
        return;
    }

    for channel in 0..3 {
        let mut values = bands
            .chunks_exact(3)
            .map(|chunk| chunk[channel])
            .collect::<Vec<_>>();
        values.sort_by(|left, right| left.partial_cmp(right).unwrap_or(std::cmp::Ordering::Equal));
        let floor = values[((values.len() - 1) as f32 * 0.05) as usize].min(0.18);
        let ceiling = values[((values.len() - 1) as f32 * 0.985) as usize].max(floor + 0.10);
        let range = (ceiling - floor).max(0.08);
        for chunk in bands.chunks_exact_mut(3) {
            let original = chunk[channel].clamp(0.0, 1.0);
            let normalized = ((original - floor) / range).clamp(0.0, 1.0).powf(0.88);
            chunk[channel] = (normalized * 0.72 + original * 0.28).clamp(0.0, 1.0);
        }
    }
}

fn smooth_packed_bands(bands: &mut [f32]) {
    if bands.len() < 6 {
        return;
    }

    let mut previous = [bands[0], bands[1], bands[2]];
    for chunk in bands.chunks_exact_mut(3) {
        for channel in 0..3 {
            previous[channel] = previous[channel] * 0.42 + chunk[channel].clamp(0.0, 1.0) * 0.58;
            chunk[channel] = previous[channel].clamp(0.0, 1.0);
        }
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

fn equal_loudness_visual_weight(frequency_hz: f32) -> f32 {
    let frequency_hz = frequency_hz.max(1.0);
    if frequency_hz < 60.0 {
        0.72
    } else if frequency_hz < 120.0 {
        0.88
    } else if frequency_hz < 2_000.0 {
        1.0
    } else if frequency_hz < 7_500.0 {
        1.08
    } else if frequency_hz < 12_000.0 {
        0.96
    } else {
        0.74
    }
}

fn db_to_unit(db: f32, floor_db: f32, ceiling_db: f32, curve: f32) -> f32 {
    ((db - floor_db) / (ceiling_db - floor_db))
        .clamp(0.0, 1.0)
        .powf(curve)
}
