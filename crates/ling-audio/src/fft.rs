//! Real-time FFT analysis for frequency-driven visuals.
//!
//! Usage:
//! ```
//! use ling_audio::FftAnalyzer;
//! let mut fft = FftAnalyzer::new(2048, 44100);
//! let audio_frame = vec![0.0f32; 2048];
//! fft.push_samples(&audio_frame);
//! let bands = fft.freq_bands(32); // 32 log-spaced frequency bands
//! let beat  = fft.is_beat();
//! let _ = (bands, beat);
//! ```

use rustfft::{FftPlanner, num_complex::Complex};

/// Frequency analysis window shapes.
#[derive(Clone, Copy, Debug)]
pub enum Window { Hann, Hamming, Blackman, Rectangular }

impl Window {
    fn apply(self, buf: &mut [f32]) {
        let n = buf.len();
        for (i, s) in buf.iter_mut().enumerate() {
            let w = match self {
                Self::Hann =>
                    0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos()),
                Self::Hamming =>
                    0.54 - 0.46 * (2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32).cos(),
                Self::Blackman => {
                    let t = 2.0 * std::f32::consts::PI * i as f32 / (n - 1) as f32;
                    0.42 - 0.5 * t.cos() + 0.08 * (2.0 * t).cos()
                }
                Self::Rectangular => 1.0,
            };
            *s *= w;
        }
    }
}

/// Holds the rolling sample buffer and performs FFT analysis.
pub struct FftAnalyzer {
    fft_size:    usize,
    sample_rate: u32,
    buffer:      Vec<f32>,
    window:      Window,
    /// Latest magnitude spectrum (fft_size/2 bins).
    pub magnitudes: Vec<f32>,
    /// Smoothed magnitudes for visuals.
    pub smoothed:   Vec<f32>,
    /// Beat detection state.
    beat_energy:    f32,
    beat_avg:       f32,
    planner:        FftPlanner<f32>,
}

impl FftAnalyzer {
    /// Create an analyser with the given FFT window size and sample rate.
    /// Typical sizes: 512, 1024, 2048, 4096.
    pub fn new(fft_size: usize, sample_rate: u32) -> Self {
        let bins = fft_size / 2;
        Self {
            fft_size,
            sample_rate,
            buffer:      vec![0.0; fft_size],
            window:      Window::Hann,
            magnitudes:  vec![0.0; bins],
            smoothed:    vec![0.0; bins],
            beat_energy: 0.0,
            beat_avg:    0.001,
            planner:     FftPlanner::new(),
        }
    }

    pub fn set_window(&mut self, w: Window) { self.window = w; }

    /// Push new audio samples (mono f32).  Keeps a rolling window.
    pub fn push_samples(&mut self, samples: &[f32]) {
        let n = samples.len().min(self.fft_size);
        let shift = self.fft_size - n;
        self.buffer.copy_within(n.., 0);
        self.buffer[shift..].copy_from_slice(&samples[..n]);
        self.compute();
    }

    fn compute(&mut self) {
        let fft = self.planner.plan_fft_forward(self.fft_size);
        let mut buf: Vec<Complex<f32>> = self.buffer.iter()
            .cloned()
            .map(|s| Complex { re: s, im: 0.0 })
            .collect();

        // Apply window function.
        {
            let mut real: Vec<f32> = buf.iter().map(|c| c.re).collect();
            self.window.apply(&mut real);
            for (c, &r) in buf.iter_mut().zip(real.iter()) { c.re = r; }
        }

        fft.process(&mut buf);

        // Compute magnitude spectrum, normalised by FFT size.
        let scale = 2.0 / self.fft_size as f32;
        let mut energy = 0.0f32;
        for (i, mag) in self.magnitudes.iter_mut().enumerate() {
            let m = buf[i].norm() * scale;
            *mag = m;
            energy += m * m;
        }

        // Smooth: exponential moving average.
        let smooth = 0.82;
        for (s, &m) in self.smoothed.iter_mut().zip(self.magnitudes.iter()) {
            *s = *s * smooth + m * (1.0 - smooth);
        }

        // Beat detection: compare current energy to rolling average.
        self.beat_energy = energy / self.magnitudes.len() as f32;
        self.beat_avg = self.beat_avg * 0.992 + self.beat_energy * 0.008;
    }

    // ── Frequency analysis ────────────────────────────────────────────────────

    /// Return `bands` logarithmically-spaced frequency magnitudes.
    /// Index 0 = bass, index bands-1 = treble.
    pub fn freq_bands(&self, bands: usize) -> Vec<f32> {
        if bands == 0 { return vec![]; }
        let nyq   = self.sample_rate as f32 / 2.0;
        let bins  = self.magnitudes.len();
        let f_min = 20.0f32;
        let f_max = nyq;
        let log_lo = f_min.log2();
        let log_hi = f_max.log2();

        (0..bands).map(|b| {
            let lo = 2.0f32.powf(log_lo + (b as f32 / bands as f32) * (log_hi - log_lo));
            let hi = 2.0f32.powf(log_lo + ((b + 1) as f32 / bands as f32) * (log_hi - log_lo));
            let bin_lo = ((lo / nyq) * bins as f32) as usize;
            let bin_hi = ((hi / nyq) * bins as f32) as usize + 1;
            let bin_lo = bin_lo.min(bins - 1);
            let bin_hi = bin_hi.min(bins);
            if bin_hi <= bin_lo {
                self.smoothed[bin_lo]
            } else {
                self.smoothed[bin_lo..bin_hi].iter().cloned().fold(0.0f32, f32::max)
            }
        }).collect()
    }

    /// Dominant frequency in Hz (peak of magnitude spectrum).
    pub fn dominant_freq(&self) -> f32 {
        let nyq = self.sample_rate as f32 / 2.0;
        let bins = self.magnitudes.len();
        let peak_bin = self.magnitudes.iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .map(|(i, _)| i)
            .unwrap_or(0);
        peak_bin as f32 / bins as f32 * nyq
    }

    /// RMS level of the current window.
    pub fn rms(&self) -> f32 {
        (self.buffer.iter().map(|s| s * s).sum::<f32>() / self.fft_size as f32).sqrt()
    }

    /// Beat detection: true if current frame energy significantly exceeds average.
    pub fn is_beat(&self) -> bool {
        self.beat_energy > self.beat_avg * 1.5
    }

    /// Beat energy ratio (1.0 = at threshold, >1 = strong beat).
    pub fn beat_ratio(&self) -> f32 {
        self.beat_energy / self.beat_avg.max(1e-6)
    }
}

// ── Color palettes ─────────────────────────────────────────────────────────────

/// IQ cosine palette: color(t) = a + b * cos(2π * (c*t + d))
#[derive(Clone, Debug)]
pub struct CosPalette {
    pub a: [f32; 3],  // offset
    pub b: [f32; 3],  // amplitude
    pub c: [f32; 3],  // frequency
    pub d: [f32; 3],  // phase
}

impl CosPalette {
    /// Classic rainbow palette.
    pub fn rainbow() -> Self {
        Self {
            a: [0.5, 0.5, 0.5],
            b: [0.5, 0.5, 0.5],
            c: [1.0, 1.0, 1.0],
            d: [0.0, 0.333, 0.667],
        }
    }

    /// Warm fire: red → orange → yellow.
    pub fn fire() -> Self {
        Self {
            a: [0.8, 0.4, 0.1],
            b: [0.7, 0.3, 0.1],
            c: [1.0, 0.5, 0.3],
            d: [0.0, 0.5, 0.8],
        }
    }

    /// Deep ocean: blue → cyan → white.
    pub fn ocean() -> Self {
        Self {
            a: [0.1, 0.4, 0.7],
            b: [0.3, 0.3, 0.4],
            c: [0.8, 1.0, 0.5],
            d: [0.3, 0.0, 0.6],
        }
    }

    /// Psychedelic high-saturation cycling.
    pub fn psychedelic() -> Self {
        Self {
            a: [0.5, 0.5, 0.5],
            b: [0.8, 0.8, 0.8],
            c: [1.0, 1.3, 0.7],
            d: [0.0, 0.15, 0.3],
        }
    }

    /// Bass (red) → mid (green) → high freq (blue/violet).
    pub fn frequency() -> Self {
        Self {
            a: [0.5, 0.4, 0.5],
            b: [0.5, 0.3, 0.5],
            c: [0.5, 0.5, 1.0],
            d: [0.0, 0.33, 0.67],
        }
    }

    /// Evaluate the palette at parameter `t` (0..1), returns `[r, g, b]` in 0..1.
    pub fn eval(&self, t: f32) -> [f32; 3] {
        [0, 1, 2].map(|i| {
            let v = self.a[i] + self.b[i] * (2.0 * std::f32::consts::PI * (self.c[i] * t + self.d[i])).cos();
            v.clamp(0.0, 1.0)
        })
    }

    /// Evaluate with time-shifted cycle (for animation).
    pub fn eval_animated(&self, t: f32, time: f32, speed: f32) -> [f32; 3] {
        self.eval((t + time * speed) % 1.0)
    }

    /// Map an array of frequency band magnitudes to colors.
    /// `magnitudes`: 0..1 per band.  Returns `bands` RGBA pixels.
    pub fn map_bands(&self, magnitudes: &[f32], time: f32, speed: f32) -> Vec<[u8; 4]> {
        magnitudes.iter().enumerate().map(|(i, &m)| {
            let t = i as f32 / magnitudes.len() as f32;
            let [r, g, b] = self.eval_animated(t, time, speed);
            let brightness = m.clamp(0.0, 1.0);
            [(r * brightness * 255.0) as u8, (g * brightness * 255.0) as u8, (b * brightness * 255.0) as u8, (brightness * 255.0) as u8]
        }).collect()
    }
}

/// Generate an RGBA texture (width × height) where each column is a frequency band,
/// brightness is magnitude, and colour is palette-based.
pub fn freq_texture(bands: &[f32], palette: &CosPalette, width: u32, height: u32, time: f32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut pixels = vec![0u8; w * h * 4];
    for x in 0..w {
        let band_idx = (x * bands.len() / w).min(bands.len() - 1);
        let mag      = bands[band_idx].clamp(0.0, 1.0);
        let fill_h   = (mag * h as f32) as usize;
        let t        = x as f32 / w as f32;
        let [r, g, b] = palette.eval_animated(t, time, 0.3);
        for y in (h - fill_h)..h {
            let alpha = (mag * (1.0 - (y as f32 / h as f32)) * 1.5).clamp(0.0, 1.0);
            let idx = (y * w + x) * 4;
            pixels[idx    ] = (r * 255.0) as u8;
            pixels[idx + 1] = (g * 255.0) as u8;
            pixels[idx + 2] = (b * 255.0) as u8;
            pixels[idx + 3] = (alpha * 255.0) as u8;
        }
    }
    pixels
}

/// Build a circular waveform / scope texture (Lissajous-style).
pub fn waveform_texture(samples: &[f32], palette: &CosPalette, width: u32, height: u32, time: f32) -> Vec<u8> {
    let w = width as usize;
    let h = height as usize;
    let mut pixels = vec![0u8; w * h * 4];
    for (i, &s) in samples.iter().enumerate() {
        let x = i * w / samples.len();
        let y = ((s * 0.5 + 0.5) * h as f32).clamp(0.0, h as f32 - 1.0) as usize;
        let t = i as f32 / samples.len() as f32;
        let [r, g, b] = palette.eval_animated(t, time, 0.2);
        let idx = (y * w + x) * 4;
        pixels[idx    ] = (r * 255.0) as u8;
        pixels[idx + 1] = (g * 255.0) as u8;
        pixels[idx + 2] = (b * 255.0) as u8;
        pixels[idx + 3] = 255;
    }
    pixels
}
