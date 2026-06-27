use rustfft::{num_complex::Complex, FftPlanner};

const DEFAULT_FFT_SIZE: usize = 4096;
const MIN_FREQUENCY_HZ: f32 = 28.0;
const MAX_FREQUENCY_HZ: f32 = 16_000.0;

pub fn spectrum_waveform(samples: &[f32], sample_rate: u32, points: usize) -> Vec<f32> {
    if samples.is_empty() || sample_rate == 0 || points == 0 {
        return Vec::new();
    }

    let chunk_size = (samples.len() / points).max(1);
    let mut analyzer = SpectrumAnalyzer::new(sample_rate, DEFAULT_FFT_SIZE);
    let mut bars = Vec::with_capacity(points);

    for chunk in samples.chunks(chunk_size).take(points) {
        let level = analyzer.analyze_chunk(chunk);
        bars.push(level);
    }

    while bars.len() < points {
        bars.push(0.0);
    }

    bars
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
        }
    }

    pub fn push_sample(&mut self, sample: f32) -> Option<f32> {
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

        // AIMP-style smooth envelope: transients rise clearly, then decay slowly instead of flickering.
        let smoothed = if target > self.previous_level {
            self.previous_level * 0.58 + target * 0.42
        } else {
            self.previous_level * 0.93 + target * 0.07
        };
        self.previous_level = smoothed.clamp(0.0, 1.0);
        Some(self.previous_level)
    }
}

struct SpectrumAnalyzer {
    sample_rate: u32,
    fft_size: usize,
    window: Vec<f32>,
    buffer: Vec<Complex<f32>>,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
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
        }
    }

    fn analyze_chunk(&mut self, chunk: &[f32]) -> f32 {
        if chunk.is_empty() {
            return 0.0;
        }

        if chunk.len() <= self.fft_size {
            return self.analyze_window(chunk);
        }

        let step = (self.fft_size / 2).max(1);
        let mut best = 0.0_f32;
        let mut offset = 0usize;
        while offset < chunk.len() {
            let end = (offset + self.fft_size).min(chunk.len());
            best = best.max(self.analyze_window(&chunk[offset..end]));
            if end == chunk.len() {
                break;
            }
            offset += step;
        }
        best
    }

    fn analyze_window(&mut self, samples: &[f32]) -> f32 {
        for value in &mut self.buffer {
            *value = Complex::new(0.0, 0.0);
        }

        let sample_count = samples.len().min(self.fft_size);
        if sample_count == 0 {
            return 0.0;
        }

        let mut time_energy = 0.0_f64;
        for index in 0..sample_count {
            let sample = samples[index].clamp(-1.0, 1.0);
            time_energy += (sample as f64) * (sample as f64);
            self.buffer[index].re = sample * self.window[index];
        }

        self.fft.process(&mut self.buffer);

        let rms = (time_energy / sample_count as f64).sqrt() as f32;
        let spectral = self.perceptual_spectral_level();
        let loudness = db_to_unit(20.0 * rms.max(0.000_001).log10(), -58.0, -7.0, 1.38);

        // Spectrum carries the musical shape; RMS keeps dense passages readable without saturating everything.
        (spectral * 0.84 + loudness * 0.16).clamp(0.0, 1.0)
    }

    fn perceptual_spectral_level(&self) -> f32 {
        let nyquist = self.sample_rate as f32 / 2.0;
        let max_frequency = MAX_FREQUENCY_HZ.min(nyquist.max(MIN_FREQUENCY_HZ));
        let bands = [
            (MIN_FREQUENCY_HZ, 80.0, 0.54),
            (80.0, 180.0, 0.70),
            (180.0, 420.0, 0.94),
            (420.0, 1_000.0, 1.08),
            (1_000.0, 2_600.0, 1.14),
            (2_600.0, 6_000.0, 1.05),
            (6_000.0, max_frequency, 0.76),
        ];

        let mut weighted_sum = 0.0_f32;
        let mut weight_sum = 0.0_f32;
        let mut strongest = 0.0_f32;

        for (low, high, weight) in bands {
            if high <= low || low >= max_frequency {
                continue;
            }
            let band = self.band_level(low, high.min(max_frequency));
            strongest = strongest.max(band);
            weighted_sum += band * weight;
            weight_sum += weight;
        }

        if weight_sum <= 0.0 {
            0.0
        } else {
            let average = weighted_sum / weight_sum;
            (average * 0.72 + strongest * 0.28).clamp(0.0, 1.0)
        }
    }

    fn band_level(&self, low_hz: f32, high_hz: f32) -> f32 {
        let bin_hz = self.sample_rate as f32 / self.fft_size as f32;
        if bin_hz <= 0.0 {
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
        db_to_unit(db, -82.0, -16.0, 1.28)
    }
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

fn db_to_unit(db: f32, floor_db: f32, ceiling_db: f32, curve: f32) -> f32 {
    ((db - floor_db) / (ceiling_db - floor_db))
        .clamp(0.0, 1.0)
        .powf(curve)
}
