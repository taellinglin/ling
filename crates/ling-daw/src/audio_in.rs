//! Audio input capture for recording audio clips, built on `ling-mic`.
//!
//! While recording, a coarse min/max envelope is kept next to the raw samples so the UI can draw
//! the waveform as it grows without touching millions of samples every frame.
use ling_mic::{MicConfig, MicInput};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

/// Frames per envelope bucket (~11 ms at 44.1 kHz).
const BUCKET: usize = 512;

#[derive(Default)]
struct Wave {
    /// (min, max) of the mono mix per finished bucket
    buckets: Vec<(f32, f32)>,
    lo: f32,
    hi: f32,
    n: usize,
}

pub struct Capture {
    mic: MicInput,
    pub buf: Arc<Mutex<Vec<f32>>>, // interleaved stereo at `rate`
    wave: Arc<Mutex<Wave>>,
    pub rate: u32,
    pub level: Arc<AtomicU32>, // f32 bits: recent peak
    pub device: String,
}

pub fn list_inputs() -> Vec<String> {
    ling_mic::input_devices()
}

impl Capture {
    /// Seconds recorded so far.
    pub fn seconds(&self) -> f64 {
        self.buf.lock().map(|b| b.len() as f64 / 2.0).unwrap_or(0.0) / self.rate.max(1) as f64
    }

    /// The waveform so far squeezed into at most `cols` columns: flat `[min, max, ...]`.
    pub fn wave(&self, cols: usize) -> Vec<f32> {
        let Ok(w) = self.wave.lock() else { return vec![] };
        let total = w.buckets.len() + usize::from(w.n > 0);
        if total == 0 {
            return vec![];
        }
        let cols = cols.clamp(1, total);
        let mut out = Vec::with_capacity(cols * 2);
        for c in 0..cols {
            let a = c * total / cols;
            let b = ((c + 1) * total / cols).max(a + 1).min(total);
            let (mut lo, mut hi) = (0.0f32, 0.0f32);
            for i in a..b {
                let (l, h) = w.buckets.get(i).copied().unwrap_or((w.lo, w.hi));
                lo = lo.min(l);
                hi = hi.max(h);
            }
            out.push(lo);
            out.push(hi);
        }
        out
    }

    pub fn stop(&self) {
        self.mic.stop();
    }
}

pub fn start(preferred: Option<&str>, gain_db: f32) -> Result<Capture, String> {
    let gain = 10f32.powf(gain_db.clamp(-24.0, 60.0) / 20.0);
    let mic = MicInput::open(MicConfig { channels: 2, device: preferred.map(str::to_string), ..Default::default() }).map_err(|e| e.to_string())?;
    let buf: Arc<Mutex<Vec<f32>>> = Arc::new(Mutex::new(Vec::with_capacity(44_100 * 2 * 60)));
    let wave = Arc::new(Mutex::new(Wave::default()));
    let level = Arc::new(AtomicU32::new(0));
    let (b2, w2, l2) = (buf.clone(), wave.clone(), level.clone());
    mic.start_framed(move |d, ch| {
        let ch = ch.max(1);
        let mut pk = 0.0f32;
        if let (Ok(mut b), Ok(mut w)) = (b2.lock(), w2.lock()) {
            for f in d.chunks_exact(ch) {
                let (l, r) = (f[0] * gain, if ch > 1 { f[1] * gain } else { f[0] * gain });
                pk = pk.max(l.abs()).max(r.abs());
                b.push(l);
                b.push(r);
                let m = (l + r) * 0.5;
                if w.n == 0 {
                    w.lo = m;
                    w.hi = m;
                } else {
                    w.lo = w.lo.min(m);
                    w.hi = w.hi.max(m);
                }
                w.n += 1;
                if w.n >= BUCKET {
                    let e = (w.lo, w.hi);
                    w.buckets.push(e);
                    w.n = 0;
                }
            }
        }
        l2.store(pk.to_bits(), Ordering::Relaxed);
    })
    .map_err(|e| e.to_string())?;
    let (rate, device) = (mic.sample_rate(), mic.device_name());
    Ok(Capture { mic, buf, wave, rate, level, device })
}
