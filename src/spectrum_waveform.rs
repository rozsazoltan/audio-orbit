use rustfft::{num_complex::Complex, FftPlanner};

const DEFAULT_FFT_SIZE: usize = 4096;
const MIN_FREQUENCY_HZ: f32 = 28.0;
const MAX_FREQUENCY_HZ: f32 = 18_000.0;
const ANALYSIS_BANDS: usize = 28;

#[derive(Clone, Copy, Debug, Default)]
pub struct SpectrumBucket {
    pub level: f32,
    pub brightness: f32,
}

/// Build an AIMP-style perceptual waveform.
///
/// The first vector is the visible bar level over time. The second vector is a matching
/// low/high-frequency balance value used by the renderer to make bass-heavy and bright
/// passages visually different instead of only showing loud/quiet changes.
pub fn spectrum_waveform(samples: &[f32], sample_rate: u32, points: usize) -> (Vec<f32>, Vec<f32>) {
    if samples.is_empty() || sample_rate == 0 || points == 0 {
        return (Vec::new(), Vec::new());
    }

    let chunk_size = (samples.len() / points).max(1);
    let mut analyzer = SpectrumAnalyzer::new(sample_rate, DEFAULT_FFT_SIZE);
    let mut levels = Vec::with_capacity(points);
    let mut brightness = Vec::with_capacity(points);

    for chunk in samples.chunks(chunk_size).take(points) {
        let bucket = analyzer.analyze_chunk(chunk);
        levels.push(bucket.level);
        brightness.push(bucket.brightness);
    }

    while levels.len() < points {
        levels.push(0.0);
        brightness.push(0.5);
    }

    normalize_waveform_levels(&mut levels);
    smooth_brightness(&mut brightness);

    (levels, brightness)
}

pub struct LiveSpectrumAnalyzer {
    fft_size: usize,
    hop_size: usize,
    ring: Vec<f32>,
    ring_pos: usize,
    filled: usize,
    samples_since_bucket: usize,
    analyzer: SpectrumAnalyzer,
    previous_level: f32,
    previous_brightness: f32,
    adaptive_floor: f32,
    adaptive_peak: f32,
}

impl LiveSpectrumAnalyzer {
    pub fn new(sample_rate: u32, buckets_per_second: usize) -> Self {
        let sample_rate = sample_rate.max(1);
        let fft_size = DEFAULT_FFT_SIZE.min(sample_rate as usize).max(512).next_power_of_two();
        let hop_size = (sample_rate as usize / buckets_per_second.max(1)).max(1);
        Self {
            fft_size,
            hop_size,
            ring: vec![0.0; fft_size],
            ring_pos: 0,
            filled: 0,
            samples_since_bucket: 0,
            analyzer: SpectrumAnalyzer::new(sample_rate, fft_size),
            previous_level: 0.0,
            previous_brightness: 0.5,
            adaptive_floor: 0.035,
            adaptive_peak: 0.24,
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

        // Smooth like a desktop player visualizer: fast attack, slower release, and no constant 100% wall.
        let smoothed_level = if adaptive_level > self.previous_level {
            self.previous_level * 0.42 + adaptive_level * 0.58
        } else {
            self.previous_level * 0.84 + adaptive_level * 0.16
        };
        self.previous_level = smoothed_level.clamp(0.0, 1.0);
        self.previous_brightness = (self.previous_brightness * 0.62 + target.brightness * 0.38).clamp(0.0, 1.0);

        Some(SpectrumBucket {
            level: self.previous_level,
            brightness: self.previous_brightness,
        })
    }

    fn normalize_live_level(&mut self, level: f32) -> f32 {
        let level = level.clamp(0.0, 1.0);
        self.adaptive_floor = if level < self.adaptive_floor {
            self.adaptive_floor * 0.94 + level * 0.06
        } else {
            self.adaptive_floor * 0.998 + level * 0.002
        };
        self.adaptive_peak = if level > self.adaptive_peak {
            self.adaptive_peak * 0.74 + level * 0.26
        } else {
            self.adaptive_peak * 0.996 + level * 0.004
        };

        let floor = self.adaptive_floor.min(0.22);
        let peak = self.adaptive_peak.max(floor + 0.10);
        let normalized = ((level - floor) / (peak - floor)).clamp(0.0, 1.0);
        normalized.powf(0.92) * 0.96
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
        let mut sum_level = 0.0_f32;
        let mut sum_brightness = 0.0_f32;
        let mut strongest = SpectrumBucket::default();
        let mut count = 0usize;
        let mut offset = 0usize;
        while offset < chunk.len() {
            let end = (offset + self.fft_size).min(chunk.len());
            let bucket = self.analyze_window(&chunk[offset..end]);
            sum_level += bucket.level;
            sum_brightness += bucket.brightness;
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
            let average_level = sum_level / count as f32;
            let average_brightness = sum_brightness / count as f32;
            SpectrumBucket {
                level: (average_level * 0.70 + strongest.level * 0.30).clamp(0.0, 1.0),
                brightness: (average_brightness * 0.72 + strongest.brightness * 0.28).clamp(0.0, 1.0),
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
        for index in 0..sample_count {
            let sample = samples[index].clamp(-1.0, 1.0);
            time_energy += (sample as f64) * (sample as f64);
            self.buffer[index].re = sample * self.window[index];
        }

        self.fft.process(&mut self.buffer);

        let rms = (time_energy / sample_count as f64).sqrt() as f32;
        let loudness = db_to_unit(20.0 * rms.max(0.000_001).log10(), -64.0, -5.0, 1.35);
        let profile = self.perceptual_spectral_profile();

        // The visible value is deliberately spectral-first. Loudness still matters, but low/mid/high
        // balance and motion must change the bars even when two moments have similar RMS.
        let contrast = (profile.strongest - profile.average).clamp(0.0, 1.0);
        let band_presence = (profile.low * 0.30 + profile.mid * 0.28 + profile.high * 0.30 + profile.strongest * 0.12)
            .clamp(0.0, 1.0);
        let tilt_emphasis = ((profile.brightness - 0.5).abs() * 2.0).clamp(0.0, 1.0);
        let level = profile.average * 0.25
            + band_presence * 0.25
            + loudness * 0.18
            + profile.flux * 0.14
            + contrast * 0.10
            + tilt_emphasis * profile.strongest * 0.08;

        SpectrumBucket {
            level: level.clamp(0.0, 1.0),
            brightness: profile.brightness.clamp(0.0, 1.0),
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
            bands.push(self.band_level(low, high));
        }

        let mut weighted_sum = 0.0_f32;
        let mut weight_sum = 0.0_f32;
        let mut strongest = 0.0_f32;
        let mut centroid_sum = 0.0_f32;
        let mut low = 0.0_f32;
        let mut mid = 0.0_f32;
        let mut high = 0.0_f32;

        for (index, band) in bands.iter().copied().enumerate() {
            let position = index as f32 / (ANALYSIS_BANDS - 1).max(1) as f32;
            let weight = 0.80 + (1.0 - (position - 0.46).abs()).max(0.0) * 0.34;
            weighted_sum += band * weight;
            weight_sum += weight;
            strongest = strongest.max(band);
            centroid_sum += band * position;
            if position < 0.30 {
                low += band;
            } else if position < 0.70 {
                mid += band;
            } else {
                high += band;
            }
        }

        let average = if weight_sum > 0.0 { weighted_sum / weight_sum } else { 0.0 };
        let total = bands.iter().copied().sum::<f32>().max(0.000_001);
        let centroid_brightness = (centroid_sum / total).clamp(0.0, 1.0);
        let low = low / (ANALYSIS_BANDS as f32 * 0.30).max(1.0);
        let mid = mid / (ANALYSIS_BANDS as f32 * 0.40).max(1.0);
        let high = high / (ANALYSIS_BANDS as f32 * 0.30).max(1.0);
        let low_mid_high_total = (low + mid + high).max(0.000_001);
        let high_balance = ((high + mid * 0.42) / low_mid_high_total).clamp(0.0, 1.0);
        let brightness = ((centroid_brightness * 0.58 + high_balance * 0.42 - 0.5) * 1.42 + 0.5)
            .clamp(0.0, 1.0);

        let mut positive_flux = 0.0_f32;
        let mut flux_count = 0usize;
        for (band, previous) in bands.iter().zip(self.previous_bands.iter()) {
            positive_flux += (*band - *previous).max(0.0);
            flux_count += 1;
        }
        self.previous_bands = bands;
        let flux = if flux_count == 0 {
            0.0
        } else {
            (positive_flux / flux_count as f32 * 2.8).clamp(0.0, 1.0)
        };

        SpectralProfile {
            average,
            strongest,
            flux,
            brightness,
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
            let magnitude = complex.norm() / self.fft_size as f32;
            sum += (magnitude as f64) * (magnitude as f64);
            count += 1;
        }

        if count == 0 {
            return 0.0;
        }

        let energy = (sum / count as f64).sqrt() as f32;
        let db = 20.0 * energy.max(0.000_001).log10();
        db_to_unit(db, -86.0, -13.0, 1.14)
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct SpectralProfile {
    average: f32,
    strongest: f32,
    flux: f32,
    brightness: f32,
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
    let floor = sorted[((sorted.len() - 1) as f32 * 0.06) as usize].min(0.24);
    let ceiling = sorted[((sorted.len() - 1) as f32 * 0.992) as usize].max(floor + 0.10);
    let range = (ceiling - floor).max(0.08);

    for value in levels.iter_mut() {
        let normalized = ((*value - floor) / range).clamp(0.012, 0.985);
        *value = normalized.powf(0.92);
    }
}

fn smooth_brightness(values: &mut [f32]) {
    if values.is_empty() {
        return;
    }

    let mut previous = values[0].clamp(0.0, 1.0);
    for value in values.iter_mut() {
        let expanded = ((*value).clamp(0.0, 1.0) - 0.5) * 1.28 + 0.5;
        previous = previous * 0.48 + expanded.clamp(0.0, 1.0) * 0.52;
        *value = previous;
    }
}

fn db_to_unit(db: f32, floor_db: f32, ceiling_db: f32, curve: f32) -> f32 {
    ((db - floor_db) / (ceiling_db - floor_db))
        .clamp(0.0, 1.0)
        .powf(curve)
}
