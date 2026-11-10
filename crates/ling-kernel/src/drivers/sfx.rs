//! Sound effects and music for programs (LingDev games, the desktop's UI
//! sounds): WAV files decoded once into 48 kHz samples, played by up to 16
//! voices (pitch, volume, pan, loop) plus one music voice with pause / seek,
//! and short synthesised blips (`sfx`). The mix is added to whatever the
//! output is already playing -- LingStationX's engine -- by the HDMI audio
//! ring on the Pi and the AC'97 mixer on x86 (`mix_into`).
//!
//! Spatial calls take a position and the listener's orientation; here that
//! becomes a distance roll-off and a left/right pan, which is what a small
//! game needs to feel right.

use alloc::vec::Vec;

pub const RATE: f64 = 48_000.0;
const MAX_VOICES: usize = 16;

pub struct Sample {
    rate: u32,
    stereo: bool,
    /// Interleaved L R when stereo.
    data: Vec<i16>,
}

#[derive(Clone, Copy)]
struct Voice {
    sample: usize,
    pos: f64,
    step: f64,
    vol: f32,
    pan: f32,
    looping: bool,
    active: bool,
    paused: bool,
    music: bool,
}

const SILENT: Voice = Voice { sample: 0, pos: 0.0, step: 1.0, vol: 1.0, pan: 0.0, looping: false, active: false, paused: false, music: false };

static mut SAMPLES: Vec<Sample> = Vec::new();
static mut VOICES: [Voice; MAX_VOICES] = [SILENT; MAX_VOICES];
static mut MASTER: f32 = 1.0;
static mut MUSIC_VOL: f32 = 1.0;
/// Listener yaw as (cos, sin): sounds to the listener's right pan right.
static mut LISTENER: (f32, f32) = (1.0, 0.0);
/// The music voice (one at a time), or MAX_VOICES for none.
static mut MUSIC_VOICE: usize = MAX_VOICES;

fn samples() -> &'static mut Vec<Sample> {
    unsafe { &mut *(&raw mut SAMPLES) }
}

fn voices() -> &'static mut [Voice; MAX_VOICES] {
    unsafe { &mut *(&raw mut VOICES) }
}

/// RIFF/WAVE header walk: (data offset, data length, rate, channels, bits).
fn parse_wav(buf: &[u8]) -> Option<(usize, usize, u32, u8, u8)> {
    let len = buf.len();
    if len < 12 || &buf[0..4] != b"RIFF" || &buf[8..12] != b"WAVE" {
        return None;
    }
    let u32le = |o: usize| u32::from_le_bytes([buf[o], buf[o + 1], buf[o + 2], buf[o + 3]]);
    let u16le = |o: usize| u16::from_le_bytes([buf[o], buf[o + 1]]);
    let mut off = 12;
    let (mut rate, mut ch, mut bits, mut have_fmt) = (0u32, 0u8, 0u8, false);
    while off + 8 <= len {
        let id = &buf[off..off + 4];
        let sz = u32le(off + 4) as usize;
        let body = off + 8;
        if id == b"fmt " && body + 16 <= len {
            ch = u16le(body + 2) as u8;
            rate = u32le(body + 4);
            bits = u16le(body + 14) as u8;
            have_fmt = true;
        } else if id == b"data" {
            let dlen = sz.min(len - body);
            if have_fmt {
                return Some((body, dlen, rate, ch, bits));
            }
        }
        off = body + sz + (sz & 1);
    }
    None
}

/// Decode a WAV (PCM 8 / 16 / 24 / 32-bit integer or 32-bit float, mono or
/// stereo, any rate) into a sample; None if it is not one we can read.
pub fn load_wav(bytes: &[u8]) -> Option<usize> {
    // Ogg Vorbis / FLAC: decoded whole (outside the IRQ lock -- it takes a
    // while), then stored like any WAV
    if matches!(crate::codec::sniff(bytes), Some("ogg") | Some("flac")) {
        let (rate, ch, data) = crate::codec::decode(bytes)?;
        let _irq = crate::mm::IrqLock::acquire();
        samples().push(Sample { rate, stereo: ch == 2, data });
        return Some(samples().len() - 1);
    }
    let _irq = crate::mm::IrqLock::acquire();
    let (off, len, rate, ch, bits) = parse_wav(bytes)?;
    if rate == 0 || !(1..=2).contains(&ch) {
        return None;
    }
    let body = &bytes[off..off + len];
    let bps = (bits as usize / 8).max(1);
    let n = body.len() / bps;
    let mut data: Vec<i16> = Vec::with_capacity(n);
    for i in 0..n {
        let b = &body[i * bps..i * bps + bps];
        let v: i16 = match bits {
            8 => ((b[0] as i16) - 128) << 8,
            16 => i16::from_le_bytes([b[0], b[1]]),
            24 => i16::from_le_bytes([b[1], b[2]]),
            32 => {
                // Float or integer: the fmt tag is not kept by parse_wav; a
                // float looks like |x| <= 1.0 when read as f32.
                let f = f32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                if f.is_finite() && f.abs() <= 1.0001 {
                    (f * 32767.0) as i16
                } else {
                    i16::from_le_bytes([b[2], b[3]])
                }
            },
            _ => return None,
        };
        data.push(v);
    }
    samples().push(Sample { rate, stereo: ch == 2, data });
    Some(samples().len() - 1)
}

/// A synthesised tone: `wave` 0 sine, 1 square, 2 saw, 3 triangle, 4 noise;
/// `dur` seconds, a short fade in and out. Returns a sample id.
pub fn synth(freq: f64, amp: f64, dur: f64, wave: u32) -> usize {
    let _irq = crate::mm::IrqLock::acquire();
    let n = ((dur.clamp(0.01, 4.0)) * RATE) as usize;
    let mut data: Vec<i16> = Vec::with_capacity(n);
    let mut phase = 0.0f64;
    let mut seed = 0x9E37_79B9u32;
    let fade = (RATE * 0.004) as usize;
    for i in 0..n {
        phase += freq / RATE;
        if phase >= 1.0 {
            phase -= 1.0;
        }
        let s = match wave {
            1 => {
                if phase < 0.5 {
                    1.0
                } else {
                    -1.0
                }
            },
            2 => phase * 2.0 - 1.0,
            3 => {
                if phase < 0.5 {
                    phase * 4.0 - 1.0
                } else {
                    3.0 - phase * 4.0
                }
            },
            4 => {
                seed ^= seed << 13;
                seed ^= seed >> 17;
                seed ^= seed << 5;
                (seed as f64 / u32::MAX as f64) * 2.0 - 1.0
            },
            _ => libm::sin(phase * core::f64::consts::TAU),
        };
        let env = (i.min(fade) as f64 / fade as f64).min((n - i).min(fade) as f64 / fade as f64);
        // A gentle decay so blips sound like blips.
        let decay = 1.0 - i as f64 / n as f64 * 0.5;
        data.push((s * amp.clamp(0.0, 1.0) * env * decay * 12000.0) as i16);
    }
    samples().push(Sample { rate: RATE as u32, stereo: false, data });
    samples().len() - 1
}

pub fn duration(id: usize) -> f64 {
    match samples().get(id) {
        Some(s) => s.data.len() as f64 / if s.stereo { 2.0 } else { 1.0 } / s.rate as f64,
        None => 0.0,
    }
}

/// Start a voice; `pitch` 1.0 = as recorded. Returns the voice index, or None.
pub fn play(id: usize, vol: f32, pan: f32, looping: bool, pitch: f64, music: bool) -> Option<usize> {
    let _irq = crate::mm::IrqLock::acquire();
    let s = samples().get(id)?;
    let step = s.rate as f64 / RATE * pitch.clamp(0.1, 8.0);
    let vs = voices();
    let slot = if music {
        stop_music();
        MAX_VOICES - 1
    } else {
        // A free voice, else the oldest effect (never the music slot).
        (0..MAX_VOICES - 1).find(|&i| !vs[i].active).unwrap_or_else(|| {
            let mut best = 0;
            for i in 0..MAX_VOICES - 1 {
                if vs[i].pos * vs[best].step > vs[best].pos * vs[i].step {
                    best = i;
                }
            }
            best
        })
    };
    vs[slot] = Voice { sample: id, pos: 0.0, step, vol: vol.clamp(0.0, 2.0), pan: pan.clamp(-1.0, 1.0), looping, active: true, paused: false, music };
    if music {
        unsafe { MUSIC_VOICE = slot };
    }
    Some(slot)
}

/// A sound at a point: louder when near, panned by where it is relative to
/// the listener's facing (`set_listener`). `reach` is the distance it fades
/// out over (the host API's `w`; 0 means a default of 24 units).
pub fn play_at(id: usize, x: f32, _y: f32, z: f32, reach: f32, vol: f32, looping: bool) -> Option<usize> {
    let (c, s) = unsafe { LISTENER };
    let dist = libm::sqrtf(x * x + z * z);
    let reach = if reach > 0.0 { reach } else { 24.0 };
    let gain = vol * (1.0 - (dist / reach).clamp(0.0, 1.0)).max(0.0);
    // Right of the listener: the facing rotated by +90 degrees.
    let right = (x * c - z * s) / dist.max(0.001);
    let pan = if dist < 0.5 { 0.0 } else { right.clamp(-1.0, 1.0) * 0.8 };
    play(id, gain, pan, looping, 1.0, false)
}

pub fn set_listener(cos_yaw: f32, sin_yaw: f32) {
    unsafe { LISTENER = (cos_yaw, sin_yaw) };
}

pub fn stop(voice: usize) {
    let _irq = crate::mm::IrqLock::acquire();
    if voice < MAX_VOICES {
        voices()[voice].active = false;
    }
}

pub fn set_master(v: f32) {
    unsafe { MASTER = v.clamp(0.0, 1.0) };
}

pub fn set_music_volume(v: f32) {
    unsafe { MUSIC_VOL = v.clamp(0.0, 1.0) };
}

fn music_voice() -> Option<&'static mut Voice> {
    let i = unsafe { MUSIC_VOICE };
    if i < MAX_VOICES && voices()[i].music {
        Some(&mut voices()[i])
    } else {
        None
    }
}

pub fn stop_music() {
    let _irq = crate::mm::IrqLock::acquire();
    if let Some(v) = music_voice() {
        v.active = false;
    }
    unsafe { MUSIC_VOICE = MAX_VOICES };
}

pub fn pause_music() {
    let _irq = crate::mm::IrqLock::acquire();
    if let Some(v) = music_voice() {
        v.paused = !v.paused;
    }
}

pub fn seek_music(seconds: f64) {
    let _irq = crate::mm::IrqLock::acquire();
    if let Some(v) = music_voice() {
        let s = &samples()[v.sample];
        let frames = s.data.len() / if s.stereo { 2 } else { 1 };
        v.pos = (seconds.max(0.0) * s.rate as f64).min(frames as f64 - 1.0);
    }
}

pub fn music_pos() -> f64 {
    match music_voice() {
        Some(v) => v.pos / samples()[v.sample].rate as f64,
        None => 0.0,
    }
}

pub fn music_playing() -> bool {
    music_voice().is_some_and(|v| v.active && !v.paused)
}

// ── Tone voices (`audio_tone`): continuous oscillators a program retunes every
// frame -- the host runtime's chip-style synth that Ling Arcade's NES APU
// drives (2 pulses, triangle, noise). Amplitude and frequency glide toward
// their targets (`audio_glide`) so per-frame updates ramp instead of click.

const MAX_TONES: usize = 8;

#[derive(Clone, Copy)]
struct Tone {
    freq: f32,
    amp: f32,
    target_freq: f32,
    target_amp: f32,
    /// 0 sine, 1 square, 2 saw, 3 triangle, 4 noise
    wave: u8,
    duty: f32,
    pan: f32,
    phase: f32,
    lfsr: u32,
}

const TONE_OFF: Tone = Tone { freq: 220.0, amp: 0.0, target_freq: 220.0, target_amp: 0.0, wave: 0, duty: 0.5, pan: 0.0, phase: 0.0, lfsr: 1 };
static mut TONES: [Tone; MAX_TONES] = [TONE_OFF; MAX_TONES];
/// Glide time constants in ms (amplitude, frequency); <= 0 = the default.
static mut GLIDE_MS: (f32, f32) = (5.0, 2.0);

fn tones() -> &'static mut [Tone; MAX_TONES] {
    unsafe { &mut *(&raw mut TONES) }
}

/// Retune voice `idx`: frequency (Hz), amplitude (0..1), waveform, square duty, pan (-1..1).
pub fn set_tone(idx: usize, freq: f32, amp: f32, wave: u8, duty: f32, pan: f32) {
    if let Some(t) = tones().get_mut(idx) {
        if t.target_amp <= 0.0 && t.amp <= 1e-4 {
            t.freq = freq.max(0.0); // a voice starting from silence starts on pitch
        }
        t.target_freq = freq.max(0.0);
        t.target_amp = amp.clamp(0.0, 1.0);
        t.wave = wave;
        t.duty = duty.clamp(0.01, 0.99);
        t.pan = pan.clamp(-1.0, 1.0);
    }
}

pub fn set_tone_glide(amp_ms: f32, freq_ms: f32) {
    unsafe {
        GLIDE_MS = (if amp_ms > 0.0 { amp_ms } else { 5.0 }, if freq_ms > 0.0 { freq_ms } else { 2.0 });
    }
}

fn tones_active() -> bool {
    tones().iter().any(|t| t.amp > 1e-4 || t.target_amp > 0.0)
}

fn mix_tones(out: &mut [i16]) {
    let frames = out.len() / 2;
    let master = unsafe { MASTER };
    let (ams, fms) = unsafe { GLIDE_MS };
    // one-pole smoothing coefficients per sample
    let ka = 1.0 - libm::expf(-1000.0 / (ams * RATE as f32));
    let kf = 1.0 - libm::expf(-1000.0 / (fms * RATE as f32));
    for t in tones().iter_mut() {
        if t.amp <= 1e-4 && t.target_amp <= 0.0 {
            t.amp = 0.0;
            continue;
        }
        let (lg, rg) = if t.pan <= 0.0 { (1.0, 1.0 + t.pan) } else { (1.0 - t.pan, 1.0) };
        for fr in 0..frames {
            t.amp += (t.target_amp - t.amp) * ka;
            t.freq += (t.target_freq - t.freq) * kf;
            let step = t.freq / RATE as f32;
            let p = t.phase;
            let v = match t.wave {
                1 => {
                    if p < t.duty {
                        1.0
                    } else {
                        -1.0
                    }
                },
                2 => 2.0 * p - 1.0,
                3 => 1.0 - 4.0 * libm::fabsf(p - 0.5),
                4 => {
                    if t.lfsr & 1 == 1 {
                        1.0
                    } else {
                        -1.0
                    }
                },
                _ => libm::sinf(p * core::f32::consts::TAU),
            };
            let mut np = p + step;
            if np >= 1.0 {
                np -= libm::floorf(np);
                if t.wave == 4 {
                    // 15-bit LFSR, clocked once per period (the NES noise channel's shape)
                    let bit = (t.lfsr ^ (t.lfsr >> 1)) & 1;
                    t.lfsr = (t.lfsr >> 1) | (bit << 14);
                }
            }
            t.phase = np;
            let s = v * t.amp * master * 12000.0;
            let ol = out[fr * 2] as f32 + s * lg;
            let or = out[fr * 2 + 1] as f32 + s * rg;
            out[fr * 2] = ol.clamp(-32767.0, 32767.0) as i16;
            out[fr * 2 + 1] = or.clamp(-32767.0, 32767.0) as i16;
        }
    }
}

/// Anything audible right now (so an output that is otherwise idle keeps running).
pub fn active() -> bool {
    voices().iter().any(|v| v.active && !v.paused) || tones_active()
}

/// Add the voices' next `frames` frames to `out` (interleaved stereo i16),
/// saturating. Silent voices cost nothing.
pub fn mix_into(out: &mut [i16]) {
    mix_tones(out);
    let frames = out.len() / 2;
    let master = unsafe { MASTER };
    let mvol = unsafe { MUSIC_VOL };
    for v in voices().iter_mut() {
        if !v.active || v.paused {
            continue;
        }
        let Some(s) = samples().get(v.sample) else {
            v.active = false;
            continue;
        };
        let ch = if s.stereo { 2 } else { 1 };
        let n = s.data.len() / ch;
        if n < 2 {
            v.active = false;
            continue;
        }
        let gain = v.vol * master * if v.music { mvol } else { 1.0 };
        let (lg, rg) = if v.pan <= 0.0 { (1.0, 1.0 + v.pan) } else { (1.0 - v.pan, 1.0) };
        for f in 0..frames {
            if v.pos >= (n - 1) as f64 {
                if v.looping {
                    v.pos -= (n - 1) as f64;
                } else {
                    v.active = false;
                    break;
                }
            }
            let i = v.pos as usize;
            let t = (v.pos - i as f64) as f32;
            let i1 = (i + 1).min(n - 1);
            let (l, r) = if s.stereo {
                let l = s.data[i * 2] as f32 * (1.0 - t) + s.data[i1 * 2] as f32 * t;
                let r = s.data[i * 2 + 1] as f32 * (1.0 - t) + s.data[i1 * 2 + 1] as f32 * t;
                (l, r)
            } else {
                let m = s.data[i] as f32 * (1.0 - t) + s.data[i1] as f32 * t;
                (m, m)
            };
            let ol = out[f * 2] as f32 + l * gain * lg;
            let or = out[f * 2 + 1] as f32 + r * gain * rg;
            out[f * 2] = ol.clamp(-32767.0, 32767.0) as i16;
            out[f * 2 + 1] = or.clamp(-32767.0, 32767.0) as i16;
            v.pos += v.step;
        }
    }
}
