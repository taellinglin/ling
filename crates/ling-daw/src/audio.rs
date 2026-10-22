//! cpal output stream fed by a dedicated render thread through a lock-free ring buffer.
//!
//! The device callback only copies frames out of the ring, so a busy UI thread, a heavy project
//! or a lock held elsewhere can no longer glitch the hardware buffer directly: the render thread
//! works ahead by `latency_ms` and the ring absorbs the jitter. Underruns are counted.
use crate::engine::{Engine, SharedEngine};
use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;

/// User-facing audio options.
#[derive(Clone, Copy, Debug)]
pub struct AudioConfig {
    /// device buffer size in frames; 0 = driver default
    pub buffer: u32,
    /// how far ahead the render thread runs, in milliseconds (bigger = fewer dropouts, more latency)
    pub latency_ms: u32,
    /// engine rate: 0 = auto (44.1/48 kHz), 1 = device rate, otherwise Hz
    pub rate: u32,
}

impl Default for AudioConfig {
    fn default() -> Self {
        AudioConfig { buffer: 0, latency_ms: 60, rate: 0 }
    }
}

#[derive(Default)]
pub struct AudioStats {
    /// callbacks that ran out of rendered audio
    pub underruns: AtomicU32,
    /// render time / real time in 1/1000 (decaying peak)
    pub load_permille: AtomicU32,
    /// buffer size the device really uses (0 = unknown / driver default)
    pub buffer_frames: AtomicU32,
}

pub struct AudioOut {
    pub engine: SharedEngine,
    /// engine (DSP) rate
    pub sr: f32,
    /// hardware rate the stream runs at
    pub device_sr: f32,
    pub device: String,
    pub cfg: AudioConfig,
    pub stats: Arc<AudioStats>,
    run: Arc<AtomicBool>,
    worker: Option<std::thread::JoinHandle<()>>,
    _stream: Option<cpal::Stream>,
}

impl AudioOut {
    /// True when an output stream was actually opened.
    pub fn is_running(&self) -> bool {
        self._stream.is_some()
    }
    /// Approximate output latency in ms: render-ahead + device buffer.
    pub fn latency_ms(&self) -> f32 {
        let buf = self.stats.buffer_frames.load(Ordering::Relaxed) as f32;
        let dev = if buf > 0.0 { buf } else { 512.0 };
        self.cfg.latency_ms as f32 + dev / self.device_sr.max(1.0) * 1000.0
    }
}

impl Drop for AudioOut {
    fn drop(&mut self) {
        self.run.store(false, Ordering::Relaxed);
        self._stream = None;
        if let Some(h) = self.worker.take() {
            let _ = h.join();
        }
    }
}

pub fn device_names() -> Vec<String> {
    cpal::default_host().output_devices().map(|d| d.filter_map(|x| x.name().ok()).collect()).unwrap_or_default()
}

// ── ring buffer (single producer, single consumer) ──────────────────────────
const RING_FRAMES: usize = 1 << 16;

struct Ring {
    buf: Vec<AtomicU32>,
    head: AtomicUsize, // write position (floats)
    tail: AtomicUsize, // read position (floats)
}

impl Ring {
    fn new() -> Ring {
        Ring { buf: (0..RING_FRAMES * 2).map(|_| AtomicU32::new(0)).collect(), head: AtomicUsize::new(0), tail: AtomicUsize::new(0) }
    }
    fn fill_frames(&self) -> usize {
        self.head.load(Ordering::Acquire).wrapping_sub(self.tail.load(Ordering::Acquire)) / 2
    }
    fn push(&self, l: f32, r: f32) {
        let h = self.head.load(Ordering::Relaxed);
        let m = RING_FRAMES * 2 - 1;
        self.buf[h & m].store(l.to_bits(), Ordering::Relaxed);
        self.buf[(h + 1) & m].store(r.to_bits(), Ordering::Relaxed);
        self.head.store(h.wrapping_add(2), Ordering::Release);
    }
    fn pop(&self) -> Option<(f32, f32)> {
        let t = self.tail.load(Ordering::Relaxed);
        if self.head.load(Ordering::Acquire).wrapping_sub(t) < 2 {
            return None;
        }
        let m = RING_FRAMES * 2 - 1;
        let l = f32::from_bits(self.buf[t & m].load(Ordering::Relaxed));
        let r = f32::from_bits(self.buf[(t + 1) & m].load(Ordering::Relaxed));
        self.tail.store(t.wrapping_add(2), Ordering::Release);
        Some((l, r))
    }
}

/// Open the default (or named) output device.
pub fn start(preferred: Option<&str>, cfg_in: &AudioConfig) -> AudioOut {
    let host = cpal::default_host();
    let dev = preferred
        .filter(|s| !s.is_empty())
        .and_then(|n| host.output_devices().ok().and_then(|mut d| d.find(|x| x.name().map(|y| y == n).unwrap_or(false))))
        .or_else(|| host.default_output_device());
    let cfg = dev.as_ref().and_then(|d| {
        // prefer 48 kHz (then 44.1 kHz) f32/i16 stereo when the device offers it
        let want = [48000u32, 44100];
        if let Ok(ranges) = d.supported_output_configs() {
            let ranges: Vec<_> = ranges.collect();
            for w in want {
                for r in &ranges {
                    let fmt_ok = matches!(r.sample_format(), cpal::SampleFormat::F32 | cpal::SampleFormat::I16);
                    if fmt_ok && r.channels() >= 2 && r.min_sample_rate().0 <= w && r.max_sample_rate().0 >= w {
                        return Some(r.clone().with_sample_rate(cpal::SampleRate(w)));
                    }
                }
            }
        }
        d.default_output_config().ok()
    });
    let (dev_sr, ch) = cfg.as_ref().map(|c| (c.sample_rate().0, c.channels() as usize)).unwrap_or((48000, 2));
    // render at 44.1/48 kHz even on 96/192 kHz interfaces and resample: DSP cost scales with the rate
    let sr = match cfg_in.rate {
        0 => {
            if dev_sr <= 48000 {
                dev_sr
            } else if dev_sr % 44100 == 0 {
                44100
            } else {
                48000
            }
        }
        1 => dev_sr,
        hz => hz.clamp(8000, dev_sr.max(8000)),
    } as f32;
    let dev_sr_f = dev_sr as f32;
    let engine = Engine::new(sr).shared();
    let name = dev.as_ref().and_then(|d| d.name().ok()).unwrap_or_else(|| "no audio device".into());
    let stats = Arc::new(AudioStats::default());
    let run = Arc::new(AtomicBool::new(true));
    let ring = Arc::new(Ring::new());
    let mut stream = None;
    let mut worker = None;
    if let (Some(dev), Some(cfg)) = (dev, cfg) {
        let mut sc = cfg.config();
        // requested device buffer, clamped to what the driver supports
        let mut want_buf = 0u32;
        if cfg_in.buffer > 0 {
            want_buf = match cfg.buffer_size() {
                cpal::SupportedBufferSize::Range { min, max } => cfg_in.buffer.clamp(*min, *max),
                cpal::SupportedBufferSize::Unknown => cfg_in.buffer,
            };
            sc.buffer_size = cpal::BufferSize::Fixed(want_buf);
        }
        let build = |sc: &cpal::StreamConfig| {
            let (r, s) = (ring.clone(), stats.clone());
            let err = |e| eprintln!("audio error: {e}");
            match cfg.sample_format() {
                cpal::SampleFormat::F32 => dev.build_output_stream(sc, move |out: &mut [f32], _| drain(&r, &s, ch, out, |x| x), err, None),
                cpal::SampleFormat::I16 => dev.build_output_stream(sc, move |out: &mut [i16], _| drain(&r, &s, ch, out, |x| (x.clamp(-1.0, 1.0) * 32767.0) as i16), err, None),
                f => {
                    eprintln!("unsupported sample format {f:?}");
                    Err(cpal::BuildStreamError::StreamConfigNotSupported)
                }
            }
        };
        let mut built = build(&sc);
        if built.is_err() && want_buf > 0 {
            eprintln!("buffer size {want_buf} rejected, using the driver default");
            want_buf = 0;
            sc.buffer_size = cpal::BufferSize::Default;
            built = build(&sc);
        }
        match built {
            Ok(s) => {
                stats.buffer_frames.store(want_buf, Ordering::Relaxed);
                let target = ((cfg_in.latency_ms as usize * dev_sr as usize) / 1000).clamp(CHUNK * 2, RING_FRAMES / 2);
                let (e, r, st, rn) = (engine.clone(), ring.clone(), stats.clone(), run.clone());
                worker = std::thread::Builder::new().name("ling-audio-render".into()).spawn(move || render_thread(e, r, st, rn, sr / dev_sr_f, dev_sr_f, target)).ok();
                let _ = s.play();
                stream = Some(s);
            }
            Err(e) => eprintln!("could not open audio: {e}"),
        }
    }
    AudioOut { engine, sr, device_sr: dev_sr_f, device: name, cfg: *cfg_in, stats, run, worker, _stream: stream }
}

const CHUNK: usize = 256;

#[cfg(windows)]
fn boost_thread() {
    extern "system" {
        fn GetCurrentThread() -> isize;
        fn SetThreadPriority(h: isize, p: i32) -> i32;
    }
    unsafe {
        SetThreadPriority(GetCurrentThread(), 2); // THREAD_PRIORITY_HIGHEST
    }
}
#[cfg(not(windows))]
fn boost_thread() {}

/// Set up a DSP worker thread like the audio render thread: high priority, flush denormals to zero.
pub(crate) fn prepare_dsp_thread() {
    boost_thread();
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040);
    }
}

fn render_thread(e: SharedEngine, ring: Arc<Ring>, stats: Arc<AudioStats>, run: Arc<AtomicBool>, ratio: f32, dev_sr: f32, target: usize) {
    boost_thread();
    // flush-to-zero + denormals-are-zero: quiet reverb / filter tails must not cost 100x CPU
    #[cfg(target_arch = "x86_64")]
    unsafe {
        use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
        _mm_setcsr(_mm_getcsr() | 0x8040);
    }
    let mut rs = Resampler::new(ratio);
    let block_s = CHUNK as f32 / dev_sr;
    let mut load = 0.0f32;
    let mut primed = false;
    while run.load(Ordering::Relaxed) {
        let fill = ring.fill_frames();
        if fill + CHUNK <= target.max(CHUNK * 2) && fill + CHUNK < RING_FRAMES {
            let t0 = std::time::Instant::now();
            for _ in 0..CHUNK {
                let (l, r) = rs.next(&e);
                ring.push(l, r);
            }
            let l = t0.elapsed().as_secs_f32() / block_s;
            load = if l > load { l } else { load * 0.995 + l * 0.005 };
            stats.load_permille.store((load * 1000.0) as u32, Ordering::Relaxed);
        } else {
            if !primed {
                // startup callbacks that ran before the first fill do not count as dropouts
                primed = true;
                stats.underruns.store(0, Ordering::Relaxed);
            }
            std::thread::sleep(std::time::Duration::from_micros(400));
        }
    }
}

/// Linear-interpolating stereo resampler pulling engine-rate chunks on demand.
struct Resampler {
    /// engine frames consumed per output frame
    step: f64,
    pos: f64,
    buf: Vec<f32>,
    chunk: Vec<f32>,
}

const ENGINE_CHUNK: usize = 256;

impl Resampler {
    fn new(ratio: f32) -> Self {
        Resampler { step: ratio as f64, pos: 0.0, buf: vec![0.0; 4], chunk: vec![0.0; ENGINE_CHUNK * 2] }
    }

    fn pull(&mut self, e: &SharedEngine) {
        self.chunk.iter_mut().for_each(|x| *x = 0.0);
        // this thread may wait for the engine lock (the UI holds it only briefly); the ring covers the gap
        // a panic in the engine is caught here, inside the lock, so it neither poisons the lock (which would silence the audio
        // for good and break every later call from the UI) nor kills this thread: that block is silence and the log says why
        let mut eng = e.lock().unwrap_or_else(|p| p.into_inner());
        let chunk = &mut self.chunk;
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| eng.render(chunk))).is_err() {
            self.chunk.iter_mut().for_each(|x| *x = 0.0);
        }
        drop(eng);
        self.buf.extend_from_slice(&self.chunk);
    }

    fn next(&mut self, e: &SharedEngine) -> (f32, f32) {
        if self.step == 1.0 {
            if self.buf.len() < 4 + 2 {
                self.pull(e);
            }
            let (l, r) = (self.buf[2], self.buf[3]);
            self.buf.drain(0..2);
            return (l, r);
        }
        while (self.pos as usize + 2) * 2 > self.buf.len() {
            self.pull(e);
        }
        let i = self.pos as usize;
        let f = (self.pos - i as f64) as f32;
        let (a, b) = (i * 2, (i + 1) * 2);
        let l = self.buf[a] + (self.buf[b] - self.buf[a]) * f;
        let r = self.buf[a + 1] + (self.buf[b + 1] - self.buf[a + 1]) * f;
        self.pos += self.step;
        // drop consumed frames now and then
        let drop = self.pos as usize;
        if drop >= 512 {
            self.buf.drain(0..drop * 2);
            self.pos -= drop as f64;
        }
        (l, r)
    }
}

/// Device callback: copy from the ring, never block, never render.
fn drain<T: Copy + Default>(ring: &Ring, stats: &AudioStats, ch: usize, out: &mut [T], conv: impl Fn(f32) -> T) {
    let mut starved = false;
    for fr in out.chunks_exact_mut(ch.max(1)) {
        let (l, r) = match ring.pop() {
            Some(x) => x,
            None => {
                starved = true;
                (0.0, 0.0)
            }
        };
        for (c, o) in fr.iter_mut().enumerate() {
            *o = conv(match c {
                0 => l,
                1 => r,
                _ => 0.0,
            });
        }
    }
    if starved {
        stats.underruns.fetch_add(1, Ordering::Relaxed);
    }
}
