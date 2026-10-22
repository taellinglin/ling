//! Real-signal spectra on top of `rustfft`.

use rustfft::num_complex::Complex32;
use rustfft::FftPlanner;

pub fn hann(n: usize) -> Vec<f32> {
    (0..n).map(|i| 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()).collect()
}

/// Reusable transform of a fixed power-of-two size.
pub struct Spectrum {
    pub n: usize,
    window: Vec<f32>,
    fft: std::sync::Arc<dyn rustfft::Fft<f32>>,
}

impl Spectrum {
    pub fn new(n: usize) -> Spectrum {
        let n = n.max(8).next_power_of_two();
        Spectrum { n, window: hann(n), fft: FftPlanner::new().plan_fft_forward(n) }
    }

    /// Windowed transform of `frame` (zero padded or truncated to `n`); returns bins `0..=n/2`.
    pub fn bins(&self, frame: &[f32]) -> Vec<Complex32> {
        let mut buf: Vec<Complex32> =
            (0..self.n).map(|i| Complex32::new(frame.get(i).copied().unwrap_or(0.0) * self.window[i], 0.0)).collect();
        self.fft.process(&mut buf);
        buf.truncate(self.n / 2 + 1);
        buf
    }

    pub fn bin_freq(&self, bin: f32, rate: f32) -> f32 {
        bin * rate / self.n as f32
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locates_a_sine() {
        let rate = 8000.0;
        let f = 1000.0;
        let s: Vec<f32> = (0..1024).map(|i| (2.0 * std::f32::consts::PI * f * i as f32 / rate).sin()).collect();
        let sp = Spectrum::new(1024);
        let bins = sp.bins(&s);
        let (peak, _) = bins.iter().enumerate().max_by(|a, b| a.1.norm().total_cmp(&b.1.norm())).unwrap();
        assert!((sp.bin_freq(peak as f32, rate) - f).abs() < 10.0);
    }
}
