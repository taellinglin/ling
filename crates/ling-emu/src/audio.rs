//! Shared audio sink: cores push stereo samples at their native rate; a cpal
//! output thread resamples to the device rate.
//!
//! Smoothness design:
//! * **Pre-roll** — playback starts only after ~70 ms is buffered, so the first
//!   callbacks never underrun.
//! * **Dynamic rate control** — the resample ratio is nudged (±1.5 %, slewed)
//!   by how far the buffer is from its target fill. Emulation never runs at
//!   exactly the device clock (PAL 50 Hz on a 60 Hz tick, 60.0988 Hz NTSC,
//!   frame-time jitter); without this the buffer slowly drains or floods and
//!   you hear periodic clicks. Pitch error is inaudible at ≤1.5 %.
//! * **Catmull-Rom cubic interpolation** — far less imaging than linear.
//! * **Graceful underrun** — fades the last sample out instead of a hard cut,
//!   then re-primes.
//! * Blocking `lock()` in the callback (the producer's critical section is a
//!   few hundred pushes) — a failed `try_lock` zeroing the buffer was itself a
//!   source of clicks.

use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

/// Resampler + rate controller, independent of any audio API (unit-testable).
pub struct Resampler {
    hist: [(f32, f32); 4],
    frac: f64,
    ratio_slew: f64,
    primed: bool,
    fade: f32,
    pub underruns: u64,
}

impl Resampler {
    pub fn new() -> Resampler {
        Resampler {
            hist: [(0.0, 0.0); 4],
            frac: 0.0,
            ratio_slew: 1.0,
            primed: false,
            fade: 1.0,
            underruns: 0,
        }
    }

    #[inline]
    fn cubic(p: [f32; 4], t: f32) -> f32 {
        let a = -0.5 * p[0] + 1.5 * p[1] - 1.5 * p[2] + 0.5 * p[3];
        let b = p[0] - 2.5 * p[1] + 2.0 * p[2] - 0.5 * p[3];
        let c = -0.5 * p[0] + 0.5 * p[2];
        ((a * t + b) * t + c) * t + p[1]
    }

    /// Fill `out` (interleaved, `channels` wide) from `q`, converting
    /// `src_rate` → `out_rate`.
    pub fn render(
        &mut self,
        q: &mut VecDeque<(f32, f32)>,
        src_rate: f64,
        out_rate: f64,
        channels: usize,
        vol: f32,
        out: &mut [f32],
    ) {
        let target = src_rate * 0.07; // ~70 ms
        if !self.primed {
            if q.len() as f64 >= target {
                self.primed = true;
                self.fade = 1.0;
            } else {
                for s in out.iter_mut() {
                    *s = 0.0;
                }
                return;
            }
        }
        let base = src_rate / out_rate;
        for frame in out.chunks_mut(channels) {
            // dynamic rate control: > target → consume faster, < target → slower
            let err = (q.len() as f64 - target) / target;
            let want = 1.0 + (err * 0.015).clamp(-0.015, 0.015);
            self.ratio_slew += (want - self.ratio_slew) * 0.0005;
            self.frac += base * self.ratio_slew;
            while self.frac >= 1.0 {
                self.frac -= 1.0;
                self.hist.rotate_left(1);
                match q.pop_front() {
                    Some(s) => {
                        self.hist[3] = s;
                        self.fade = (self.fade + 0.01).min(1.0);
                    },
                    None => {
                        // underrun: fade the held sample toward silence
                        self.underruns += 1;
                        self.fade = (self.fade - 0.02).max(0.0);
                        let h = self.hist[2];
                        self.hist[3] = (h.0 * self.fade, h.1 * self.fade);
                        if self.fade <= 0.0 {
                            self.primed = false;
                        }
                    },
                }
            }
            let t = self.frac as f32;
            let l = Self::cubic([self.hist[0].0, self.hist[1].0, self.hist[2].0, self.hist[3].0], t);
            let r = Self::cubic([self.hist[0].1, self.hist[1].1, self.hist[2].1, self.hist[3].1], t);
            for (i, s) in frame.iter_mut().enumerate() {
                *s = match i {
                    0 => l * vol,
                    1 => r * vol,
                    _ => 0.0,
                };
            }
        }
    }
}

static RING: OnceLock<Arc<Mutex<VecDeque<(f32, f32)>>>> = OnceLock::new();
static SRC_RATE: AtomicU32 = AtomicU32::new(32_000);
static MUTED: AtomicBool = AtomicBool::new(false);
static VOLUME: AtomicU32 = AtomicU32::new(0x3F80_0000); // 1.0f32
pub static OVERRUNS: AtomicU64 = AtomicU64::new(0);
pub static UNDERRUNS: AtomicU64 = AtomicU64::new(0);
/// Milliseconds (since [`clock`] start) of the last device callback.
static LAST_CB_MS: AtomicU64 = AtomicU64::new(0);
/// Milliseconds of the last producer push (a core that stopped emitting
/// sound must not be paced by an empty queue).
static LAST_PUSH_MS: AtomicU64 = AtomicU64::new(0);

fn clock() -> u64 {
    static T0: OnceLock<std::time::Instant> = OnceLock::new();
    T0.get_or_init(std::time::Instant::now).elapsed().as_millis() as u64 + 1
}

/// True while an output device is actually pulling samples.
pub fn is_live() -> bool {
    let last = LAST_CB_MS.load(Ordering::Relaxed);
    let push = LAST_PUSH_MS.load(Ordering::Relaxed);
    let now = clock();
    last != 0
        && now.saturating_sub(last) < 250
        && push != 0
        && now.saturating_sub(push) < 250
        && !MUTED.load(Ordering::Relaxed)
}

/// Seconds of audio queued ahead of the device.
pub fn fill_secs() -> f64 {
    let n = ring().lock().map(|r| r.len()).unwrap_or(0);
    n as f64 / SRC_RATE.load(Ordering::Relaxed).max(1) as f64
}

/// Audio-master pacing: how many emulated frames to run this host tick so
/// the queue stays near its target. The host loop's timer is never exactly
/// the emulated rate (and a busy machine runs it slow); rate control alone
/// only bends ±1.5 %, so beyond that the queue would underrun or overflow,
/// which is what produces clicks. Repeating or dropping one video frame now
/// and then is far less noticeable than a gap in the sound.
pub fn frames_wanted() -> u32 {
    static OFF: OnceLock<bool> = OnceLock::new();
    if *OFF.get_or_init(|| std::env::var_os("LING_EMU_NOPACE").is_some()) || !is_live() {
        return 1;
    }
    let f = fill_secs();
    if f < 0.045 {
        2
    } else if f > 0.16 {
        0
    } else {
        1
    }
}
/// Headless capture: when set, pushed samples are recorded here instead of
/// being played (probes and tests write them out as WAV).
static CAPTURE: Mutex<Option<(u32, Vec<(i16, i16)>)>> = Mutex::new(None);

pub fn start_capture() {
    *CAPTURE.lock().unwrap() = Some((0, Vec::new()));
}

/// Stop capturing; returns (rate, samples).
pub fn take_capture() -> Option<(u32, Vec<(i16, i16)>)> {
    CAPTURE.lock().unwrap().take()
}

fn ring() -> &'static Arc<Mutex<VecDeque<(f32, f32)>>> {
    RING.get_or_init(|| Arc::new(Mutex::new(VecDeque::with_capacity(16384))))
}

pub fn set_volume(v: f32) {
    VOLUME.store(v.clamp(0.0, 1.0).to_bits(), Ordering::Relaxed);
}

pub fn set_muted(m: bool) {
    MUTED.store(m, Ordering::Relaxed);
    if m {
        clear();
    }
}

pub fn clear() {
    if let Ok(mut r) = ring().lock() {
        r.clear();
    }
}

/// Push signed-16 stereo samples produced at `rate` Hz.
pub fn push_stereo(rate: u32, samples: &[(i16, i16)]) {
    if MUTED.load(Ordering::Relaxed) {
        return;
    }
    if let Some((r, buf)) = CAPTURE.lock().unwrap().as_mut() {
        *r = rate;
        buf.extend_from_slice(samples);
        return;
    }
    ensure_thread();
    LAST_PUSH_MS.store(clock(), Ordering::Relaxed);
    SRC_RATE.store(rate, Ordering::Relaxed);
    if let Ok(mut r) = ring().lock() {
        for &(l, rr) in samples {
            r.push_back((l as f32 / 32768.0, rr as f32 / 32768.0));
        }
        // hard cap ~300 ms: only reached if the consumer stalls
        let cap = (rate as usize) * 3 / 10;
        if r.len() > cap {
            let drop = r.len() - cap;
            r.drain(..drop);
            OVERRUNS.fetch_add(1, Ordering::Relaxed);
        }
    }
}

pub fn push_stereo_32k(samples: &[(i16, i16)]) {
    push_stereo(32_000, samples);
}

fn ensure_thread() {
    static STARTED: OnceLock<()> = OnceLock::new();
    STARTED.get_or_init(|| {
        let ring = ring().clone();
        std::thread::spawn(move || {
            use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
            let host = cpal::default_host();
            let Some(device) = host.default_output_device() else { return };
            let Ok(config) = device.default_output_config() else { return };
            let channels = config.channels() as usize;
            let out_rate = config.sample_rate().0 as f64;
            let mut rs = Resampler::new();
            let stream = device.build_output_stream(
                &config.into(),
                move |out: &mut [f32], _| {
                    LAST_CB_MS.store(clock(), Ordering::Relaxed);
                    let src = SRC_RATE.load(Ordering::Relaxed) as f64;
                    let vol = f32::from_bits(VOLUME.load(Ordering::Relaxed));
                    match ring.lock() {
                        Ok(mut q) => {
                            let before = rs.underruns;
                            rs.render(&mut q, src, out_rate, channels, vol, out);
                            if rs.underruns != before {
                                UNDERRUNS.fetch_add(1, Ordering::Relaxed);
                            }
                        },
                        Err(_) => out.iter_mut().for_each(|s| *s = 0.0),
                    }
                },
                |e| eprintln!("ling-emu audio error: {e}"),
                None,
            );
            if let Ok(stream) = stream {
                let _ = stream.play();
                loop {
                    std::thread::park();
                }
            }
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Simulate a bursty producer whose clock is 1 % off the consumer's,
    /// with frame-time jitter, for 60 s of audio. Smooth playback means: no
    /// underruns after pre-roll, the buffer stays bounded, and the output of a
    /// 440 Hz sine never jumps more than a clean resample would.
    #[test]
    fn drifting_bursty_producer_stays_smooth() {
        let src_rate = 32_000.0;
        let out_rate = 48_000.0;
        let mut rs = Resampler::new();
        let mut q: VecDeque<(f32, f32)> = VecDeque::new();
        let mut phase = 0.0f64;
        let producer_rate = src_rate * 1.010; // emulator running 1 % fast
        let mut produced_time = 0.0f64; // seconds of audio produced so far
        let mut now = 0.0f64;
        let mut next_frame = 0.0f64;
        let mut last = 0.0f32;
        let mut max_jump = 0.0f32;
        let mut max_fill = 0usize;
        let mut started = false;
        let mut rng = 12345u32;
        let mut jitter = || {
            rng = rng.wrapping_mul(1664525).wrapping_add(1013904223);
            ((rng >> 16) as f64 / 65536.0 - 0.5) * 0.012 // ±6 ms
        };
        let cb_frames = 480usize;
        let cb_dt = cb_frames as f64 / out_rate;
        let mut buf = vec![0.0f32; cb_frames * 2];
        while now < 60.0 {
            // producer: one burst per ~16.67 ms (with jitter)
            while next_frame <= now {
                let n = (producer_rate / 60.0) as usize;
                let mut burst = Vec::with_capacity(n);
                for _ in 0..n {
                    let v = (phase * 2.0 * std::f64::consts::PI).sin() as f32 * 0.5;
                    burst.push((v, v));
                    phase = (phase + 440.0 / src_rate) % 1.0;
                }
                for s in burst {
                    q.push_back(s);
                }
                produced_time += n as f64 / src_rate;
                next_frame += 1.0 / 60.0 + jitter() * 0.3;
            }
            rs.render(&mut q, src_rate, out_rate, 2, 1.0, &mut buf);
            max_fill = max_fill.max(q.len());
            if rs.primed {
                if started {
                    for f in buf.chunks(2) {
                        max_jump = max_jump.max((f[0] - last).abs());
                        last = f[0];
                    }
                } else {
                    started = true;
                    last = buf[buf.len() - 2];
                }
            }
            now += cb_dt;
        }
        let _ = produced_time;
        assert_eq!(rs.underruns, 0, "underran {} times", rs.underruns);
        assert!(max_fill < (src_rate * 0.2) as usize, "buffer ran away: {max_fill}");
        // ideal max sample step for a 440 Hz sine at 0.5 amplitude @48 kHz = 0.0288;
        // rate control adds ≤1.5 % → stay well under double that
        assert!(max_jump < 0.06, "output not smooth: max jump {max_jump}");
    }
}
