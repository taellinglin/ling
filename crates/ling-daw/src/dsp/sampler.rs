//! A basic sampler: key zones of audio files with a root note, fine tuning, volume, pan, loops (forward / ping-pong), one-shot, reverse,
//! start / end trim and pitch tracking, an amplitude envelope, a low-pass filter and global tuning.
//!
//! Zones are edited through `Instrument::command` (add / remove / set / detect / autoloop / peaks) and saved as a JSON `config` inside the
//! project. Tracker modules import into one of these so their samples stay editable.
use crate::dsp::insert::{Insert, InsertParams};
use crate::dsp::{Instrument, TAU};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq)]
pub struct Zone {
    pub name: String,
    pub file: String,
    /// the key at which the sample plays at its recorded pitch
    pub root: u8,
    pub lo: u8,
    pub hi: u8,
    pub vlo: u8,
    pub vhi: u8,
    /// fine tuning in cents
    pub tune: f32,
    /// level in dB
    pub vol: f32,
    pub pan: f32,
    /// 0 off, 1 forward, 2 ping-pong
    pub loop_mode: u8,
    pub loop_start: u32,
    pub loop_end: u32,
    /// play to the end whatever the note length is (drums)
    pub one_shot: bool,
    /// trim: first and last frame (end 0 = to the end)
    pub start: u32,
    pub end: u32,
    pub reverse: bool,
    /// how much the pitch follows the key (1 = normally, 0 = every key plays it at the same pitch)
    pub track: f32,
    /// this sample's own insert effects (filter, drive, crusher, punch, squash, echo, room, width); the defaults are a bypass
    #[serde(default, skip_serializing_if = "fx_is_default")]
    pub fx: InsertParams,
}

fn fx_is_default(p: &InsertParams) -> bool {
    *p == InsertParams::default()
}

impl Zone {
    pub fn new(name: &str, file: &str) -> Zone {
        Zone { name: name.into(), file: file.into(), root: 60, lo: 0, hi: 127, vlo: 1, vhi: 127, tune: 0.0, vol: 0.0, pan: 0.0, loop_mode: 0, loop_start: 0, loop_end: 0, one_shot: false, start: 0, end: 0, reverse: false, track: 1.0, fx: InsertParams::default() }
    }
}

/// A multi-point envelope: (seconds, value) points, a sustain point the held note stops at, and a loop it cycles in while held.
#[derive(Clone, Serialize, Deserialize, Debug, Default, PartialEq)]
pub struct Env {
    pub pts: Vec<(f32, f32)>,
    #[serde(default)]
    pub sustain: Option<usize>,
    #[serde(default)]
    pub looped: Option<(usize, usize)>,
}
impl Env {
    pub fn on(&self) -> bool {
        self.pts.len() >= 2
    }
    /// the time a held note has reached: it stops at the sustain point and wraps inside the loop
    pub fn eff(&self, t: f32, held: bool) -> f32 {
        env_eff(&self.pts, self.sustain, self.looped, t, held)
    }
    pub fn at(&self, t: f32) -> f32 {
        env_at(&self.pts, t)
    }
    pub fn end(&self) -> f32 {
        self.pts.last().map(|p| p.0).unwrap_or(0.0)
    }
}

fn env_eff(pts: &[(f32, f32)], sustain: Option<usize>, looped: Option<(usize, usize)>, t: f32, held: bool) -> f32 {
    if held {
        if let Some(p) = sustain.and_then(|si| pts.get(si)) {
            if t >= p.0 {
                return p.0;
            }
        }
        if let Some((a, b)) = looped {
            if let (Some(pa), Some(pb)) = (pts.get(a), pts.get(b)) {
                if pb.0 > pa.0 && t >= pb.0 {
                    return pa.0 + (t - pa.0) % (pb.0 - pa.0);
                }
            }
        }
    }
    t
}

fn env_at(e: &[(f32, f32)], t: f32) -> f32 {
    if e.is_empty() {
        return 1.0;
    }
    if t <= e[0].0 {
        return e[0].1;
    }
    for w in e.windows(2) {
        if t <= w[1].0 {
            let span = (w[1].0 - w[0].0).max(1e-6);
            return w[0].1 + (w[1].1 - w[0].1) * (t - w[0].0) / span;
        }
    }
    e[e.len() - 1].1
}

#[derive(Clone, Serialize, Deserialize, Debug)]
pub struct Config {
    #[serde(default)]
    pub name: String,
    pub zones: Vec<Zone>,
    #[serde(default = "d_a")]
    pub attack: f32,
    #[serde(default = "d_d")]
    pub decay: f32,
    #[serde(default = "d_s")]
    pub sustain: f32,
    #[serde(default = "d_r")]
    pub release: f32,
    #[serde(default)]
    pub tune: f32,
    #[serde(default)]
    pub gain: f32,
    #[serde(default = "d_c")]
    pub cutoff: f32,
    #[serde(default)]
    pub reso: f32,
    /// a multi-point volume envelope (seconds, level 0..1) instead of the ADSR: it holds at `env_sustain` until the key is released,
    /// loops between `env_loop` while held, and the fade-out (seconds to silence after the release) runs on top of it
    #[serde(default)]
    pub env: Vec<(f32, f32)>,
    #[serde(default)]
    pub env_sustain: Option<usize>,
    #[serde(default)]
    pub env_loop: Option<(usize, usize)>,
    #[serde(default)]
    pub fade: f32,
    /// the panning envelope (value -1..1 added to the zone's pan) and the pitch envelope (semitones)
    #[serde(default)]
    pub pan_env: Env,
    #[serde(default)]
    pub pitch_env: Env,
    /// the filter envelope: a factor 0..1 on the base cutoff `it_cut` (0..127, Impulse Tracker units) while held
    #[serde(default)]
    pub filter_env: Env,
    #[serde(default)]
    pub it_cut: f32,
    /// random variation per note, 0..1 (OpenMPT's volume / pan / cutoff / resonance "swing")
    #[serde(default)]
    pub rnd_vol: f32,
    #[serde(default)]
    pub rnd_pan: f32,
    #[serde(default)]
    pub rnd_cut: f32,
    #[serde(default)]
    pub rnd_res: f32,
}
fn d_a() -> f32 {
    0.0
}
fn d_d() -> f32 {
    0.3
}
fn d_s() -> f32 {
    1.0
}
fn d_r() -> f32 {
    0.25
}
fn d_c() -> f32 {
    1.0
}

struct Loaded {
    /// interleaved stereo
    data: Vec<f32>,
    rate: u32,
}
impl Loaded {
    fn frames(&self) -> usize {
        self.data.len() / 2
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Attack,
    Decay,
    Sustain,
    Release,
}

struct Voice {
    zone: usize,
    key: u8,
    pos: f64,
    dir: f64,
    step: f64,
    level: f32,
    stage: Stage,
    gain: f32,
    pan_l: f32,
    pan_r: f32,
    age: u64,
    held: bool,
    /// seconds into the envelope, and the fade-out left (1 = none yet)
    env_t: f32,
    pan_t: f32,
    pitch_t: f32,
    filt_t: f32,
    pan_base: f32,
    fade: f32,
    /// this note's own cutoff (0..1) and resonance, drawn at its start when the random variation is on
    cut_v: f32,
    res_v: f32,
    /// released as a cut: a very short fade, whatever the envelope and fade-out say
    cut: bool,
    lp1: [f32; 2],
    /// a note's own slice of the sample ("@start:end:cents" in its lyric): (first frame, end frame, edge fade in frames)
    slice: Option<(f64, f64, f64)>,
    lp2: [f32; 2],
    /// per-note expression: pitch gesture (semitones: re-pitches the playing sample or slice), pressure 0..1 (level),
    /// timbre 0..1 (the filter, when it is on); `fresh` = the first value is taken at once
    pb: crate::dsp::Smooth,
    pr: crate::dsp::Smooth,
    tm: crate::dsp::Smooth,
    fresh: bool,
}

pub struct Sampler {
    sr: f32,
    pub cfg: Config,
    loaded: Vec<Option<Arc<Loaded>>>,
    voices: Vec<Voice>,
    clock: u64,
    /// pitch wheel 0..1 (0.5 = centre) and its range (1 + 23 * v semitones)
    bend: f32,
    bend_range: f32,
    /// last error of an add / load
    pub last_error: String,
    rng: crate::dsp::Rng,
    /// one insert rack per zone, and the scratch buffer the zones with effects play into
    zins: Vec<Insert>,
    zbuf: Vec<f32>,
}

const MAX_VOICES: usize = 40;

/// constant-power pan with a little extra level (centre 0.85 each side, hard pan 1.2 on one)
fn pan_gains(p: f32) -> (f32, f32) {
    let a = (p.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
    (a.cos() * 1.2, a.sin() * 1.2)
}

fn atk_s(v: f32) -> f32 {
    0.0005 + v * v * 4.0
}
fn dec_s(v: f32) -> f32 {
    0.005 + v * v * 4.0
}
fn rel_s(v: f32) -> f32 {
    0.004 + v * v * 6.0
}
fn cut_hz(v: f32) -> f32 {
    40.0 * (20000.0f32 / 40.0).powf(v.clamp(0.0, 1.0))
}

/// ffmpeg on PATH or in a few usual places (empty when there is none)
/// "@start:end:cents" -> (start frame, end frame, cents)
pub fn parse_slice(lyric: &str) -> Option<(u32, u32, f32)> {
    let b = lyric.strip_prefix('@')?;
    let mut it = b.split(':');
    let a = it.next()?.parse::<u32>().ok()?;
    let e = it.next()?.parse::<u32>().ok()?;
    let c = it.next().and_then(|x| x.parse::<f32>().ok()).unwrap_or(0.0);
    (e > a).then_some((a, e, c))
}

pub fn find_ffmpeg() -> String {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(p) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&p) {
            if d.join(exe).is_file() {
                return d.join(exe).to_string_lossy().into_owned();
            }
        }
    }
    for p in ["C:/ffmpeg/bin/ffmpeg.exe", "C:/Program Files/ffmpeg/bin/ffmpeg.exe", "/usr/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/opt/homebrew/bin/ffmpeg"] {
        if std::path::Path::new(p).is_file() {
            return p.to_string();
        }
    }
    String::new()
}

/// Decode a file the built-in decoder does not know (WavPack .wv, ...) through ffmpeg, into a cached 16-bit WAV.
fn via_ffmpeg(path: &str) -> Result<String, String> {
    let ff = find_ffmpeg();
    if ff.is_empty() {
        return Err(format!("{path}: format not supported (install ffmpeg to read it)"));
    }
    let meta = std::fs::metadata(path).map_err(|e| format!("{path}: {e}"))?;
    let mut h: u64 = 1469598103934665603;
    for b in path.bytes().chain(meta.len().to_le_bytes()) {
        h = (h ^ b as u64).wrapping_mul(1099511628211);
    }
    let dir = std::env::temp_dir().join("lingstation-samples");
    let _ = std::fs::create_dir_all(&dir);
    let out = dir.join(format!("{h:016x}.wav"));
    if !out.is_file() {
        let r = std::process::Command::new(&ff).args(["-y", "-hide_banner", "-loglevel", "error", "-i", path, "-c:a", "pcm_s16le"]).arg(&out).output().map_err(|e| format!("ffmpeg: {e}"))?;
        if !r.status.success() || !out.is_file() {
            return Err(format!("{path}: ffmpeg could not read it"));
        }
    }
    Ok(out.to_string_lossy().into_owned())
}

impl Default for Config {
    fn default() -> Config {
        Config { name: String::new(), zones: vec![], attack: d_a(), decay: d_d(), sustain: d_s(), release: d_r(), tune: 0.0, gain: 0.0, cutoff: 1.0, reso: 0.0, env: vec![], env_sustain: None, env_loop: None, fade: 0.0, pan_env: Env::default(), pitch_env: Env::default(), filter_env: Env::default(), it_cut: 127.0, rnd_vol: 0.0, rnd_pan: 0.0, rnd_cut: 0.0, rnd_res: 0.0 }
    }
}

impl Config {
    fn env_eff(&self, t: f32, held: bool) -> f32 {
        env_eff(&self.env, self.env_sustain, self.env_loop, t, held)
    }
    fn env_at(&self, t: f32) -> f32 {
        env_at(&self.env, t)
    }
    fn env_end(&self) -> f32 {
        self.env.last().map(|p| p.0).unwrap_or(0.0)
    }
}

pub fn load_audio(path: &str) -> Result<(Vec<f32>, u32), String> {
    match ling_music::decode::load(path) {
        Ok(d) => Ok((d.stereo, d.rate)),
        Err(first) => {
            // an Ogg Vorbis stream inside a RIFF / WAVE container (format tag 0x674F ...): decode the Ogg part
            if let Ok(bytes) = std::fs::read(path) {
                if bytes.starts_with(b"RIFF") {
                    if let Some(at) = bytes.windows(4).take(4096).position(|w| w == b"OggS") {
                        if let Ok(d) = ling_music::decode::from_bytes(&bytes[at..]) {
                            return Ok((d.stereo, d.rate));
                        }
                    }
                }
            }
            let ext = std::path::Path::new(path).extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
            if ["wv", "ape", "tta", "wma", "opus", "m4a", "mka"].contains(&ext.as_str()) || first.contains("unsupported") {
                let wav = via_ffmpeg(path)?;
                let d = ling_music::decode::load(&wav)?;
                Ok((d.stereo, d.rate))
            } else {
                Err(first)
            }
        }
    }
}

impl Sampler {
    pub fn new(sr: f32) -> Sampler {
        Sampler { sr, cfg: Config::default(), loaded: vec![], voices: vec![], clock: 0, bend: 0.5, bend_range: 1.0 / 23.0, last_error: String::new(), rng: crate::dsp::Rng::new(0x5eed1234), zins: vec![], zbuf: vec![] }
    }

    /// Build from a saved config; relative file names are resolved against `base`.
    pub fn from_config(sr: f32, json: &str, base: Option<&std::path::Path>) -> Result<Sampler, String> {
        let mut cfg: Config = serde_json::from_str(json).map_err(|e| format!("sampler config: {e}"))?;
        if let Some(b) = base {
            for z in cfg.zones.iter_mut() {
                if !z.file.is_empty() && !std::path::Path::new(&z.file).is_absolute() {
                    z.file = b.join(&z.file).to_string_lossy().replace('\\', "/");
                }
            }
        }
        let mut s = Sampler::new(sr);
        s.cfg = cfg;
        s.loaded = vec![None; s.cfg.zones.len()];
        for i in 0..s.cfg.zones.len() {
            s.load_zone(i);
        }
        Ok(s)
    }

    pub fn config_json(&self) -> String {
        serde_json::to_string(&self.cfg).unwrap_or_default()
    }

    fn load_zone(&mut self, i: usize) {
        let f = self.cfg.zones[i].file.clone();
        if f.is_empty() {
            return;
        }
        match load_audio(&f) {
            Ok((data, rate)) => {
                self.loaded[i] = Some(Arc::new(Loaded { data, rate }));
                let fr = self.loaded[i].as_ref().map(|l| l.frames()).unwrap_or(0) as u32;
                let z = &mut self.cfg.zones[i];
                if z.loop_end == 0 || z.loop_end > fr {
                    z.loop_end = fr;
                }
            }
            Err(e) => {
                self.last_error = format!("{f}: {e}");
                self.loaded[i] = None;
            }
        }
    }

    /// Add a zone from an audio file; returns its index.
    pub fn add_file(&mut self, path: &str) -> Result<usize, String> {
        let name = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "sample".into());
        let (data, rate) = load_audio(path)?;
        let frames = data.len() / 2;
        let mut z = Zone::new(&name, path);
        z.loop_end = frames as u32;
        self.cfg.zones.push(z);
        self.loaded.push(Some(Arc::new(Loaded { data, rate })));
        Ok(self.cfg.zones.len() - 1)
    }

    /// Add a zone from decoded samples (mono or interleaved stereo given as stereo) with the file they were saved to.
    pub fn add_loaded(&mut self, zone: Zone, stereo: Vec<f32>, rate: u32) -> usize {
        self.cfg.zones.push(zone);
        self.loaded.push(Some(Arc::new(Loaded { data: stereo, rate })));
        self.cfg.zones.len() - 1
    }

    fn zone_range(&self, i: usize) -> (usize, usize) {
        let Some(Some(l)) = self.loaded.get(i) else { return (0, 0) };
        let z = &self.cfg.zones[i];
        let end = if z.end == 0 { l.frames() } else { (z.end as usize).min(l.frames()) };
        ((z.start as usize).min(end), end)
    }

    // ── analysis helpers ──────────────────────────────────────────────────────

    /// pitch in Hz of the sample's body (YIN on a mono mix), 0 when not pitched
    pub fn detect_pitch(&self, i: usize) -> f32 {
        let Some(Some(l)) = self.loaded.get(i) else { return 0.0 };
        let (s, e) = self.zone_range(i);
        let n = e.saturating_sub(s);
        if n < 2048 {
            return 0.0;
        }
        // analyse a window after the attack
        let w0 = s + (n / 5).min(n.saturating_sub(4096));
        let len = 4096.min(e - w0);
        let mono: Vec<f32> = (0..len).map(|k| (l.data[(w0 + k) * 2] + l.data[(w0 + k) * 2 + 1]) * 0.5).collect();
        let rate = l.rate as f32;
        let (tmin, tmax) = ((rate / 2000.0) as usize, (rate / 40.0) as usize);
        let tmax = tmax.min(len / 2 - 1);
        if tmax <= tmin + 2 {
            return 0.0;
        }
        let mut d = vec![0f32; tmax + 1];
        for t in 1..=tmax {
            let mut acc = 0.0;
            for k in 0..(len - tmax) {
                let df = mono[k] - mono[k + t];
                acc += df * df;
            }
            d[t] = acc;
        }
        let mut run = 0.0;
        let mut cmnd = vec![1f32; tmax + 1];
        for t in 1..=tmax {
            run += d[t];
            cmnd[t] = if run > 0.0 { d[t] * t as f32 / run } else { 1.0 };
        }
        let mut best = 0usize;
        for t in tmin..tmax {
            if cmnd[t] < 0.15 && cmnd[t] <= cmnd[t + 1] {
                best = t;
                break;
            }
        }
        if best == 0 {
            return 0.0;
        }
        // parabolic refinement
        let (a, b, c) = (cmnd[best - 1], cmnd[best], cmnd[best + 1]);
        let denom = a - 2.0 * b + c;
        let off = if denom.abs() > 1e-9 { 0.5 * (a - c) / denom } else { 0.0 };
        rate / (best as f32 + off)
    }

    /// root key (and the fine tuning in cents) of a sample, from its pitch
    pub fn detect_root(&self, i: usize) -> Option<(u8, f32)> {
        let hz = self.detect_pitch(i);
        if hz < 30.0 {
            return None;
        }
        let m = 69.0 + 12.0 * (hz / 440.0).log2();
        let k = m.round();
        Some((k.clamp(0.0, 127.0) as u8, (m - k) * 100.0))
    }

    /// A loop over whole periods of the sample's body, ending on zero crossings; false when the sample is not pitched.
    pub fn auto_loop(&mut self, i: usize) -> bool {
        let hz = self.detect_pitch(i);
        let Some(Some(l)) = self.loaded.get(i).cloned() else { return false };
        let (s, e) = self.zone_range(i);
        let n = e.saturating_sub(s);
        let period = if hz > 30.0 { l.rate as f32 / hz } else { 0.0 };
        let zero = |from: usize| -> usize {
            let mut k = from;
            while k + 1 < e && !(l.data[k * 2] <= 0.0 && l.data[(k + 1) * 2] > 0.0) {
                k += 1;
            }
            k
        };
        if period < 4.0 || n < (period * 6.0) as usize {
            // not pitched: loop the last half
            let z = &mut self.cfg.zones[i];
            z.loop_start = (s + n / 2) as u32;
            z.loop_end = e as u32;
            z.loop_mode = 1;
            return false;
        }
        let ls = zero(s + n * 2 / 5);
        let periods = (((n as f32) * 0.35) / period).floor().max(2.0);
        let le = zero(ls + (periods * period) as usize).min(e.saturating_sub(1));
        let z = &mut self.cfg.zones[i];
        z.loop_start = ls as u32;
        z.loop_end = le as u32;
        z.loop_mode = 1;
        true
    }

    /// `n` (min, max) pairs of a zone's waveform, flattened
    pub fn peaks(&self, i: usize, n: usize) -> Vec<f64> {
        let Some(Some(l)) = self.loaded.get(i) else { return vec![] };
        let fr = l.frames();
        if fr == 0 || n == 0 {
            return vec![];
        }
        let mut out = Vec::with_capacity(n * 2);
        for b in 0..n {
            let (a, z) = (b * fr / n, ((b + 1) * fr / n).max(b * fr / n + 1).min(fr));
            let (mut lo, mut hi) = (0f32, 0f32);
            for k in a..z {
                let v = (l.data[k * 2] + l.data[k * 2 + 1]) * 0.5;
                lo = lo.min(v);
                hi = hi.max(v);
            }
            out.push(lo as f64);
            out.push(hi as f64);
        }
        out
    }

    fn start_voice(&mut self, key: u8, vel: f32, pan: f32, hard: bool) {
        self.start_voice_slice(key, vel, pan, hard, None);
    }

    /// `slice`: (start frame, end frame, cents) - the note plays that span of every zone it hits, its pitch moved by the cents,
    /// faded in over 3 ms and out over the last 25 ms (at most a fifth of it) so a chop never ends in a click
    fn start_voice_slice(&mut self, key: u8, vel: f32, pan: f32, hard: bool, slice: Option<(u32, u32, f32)>) {
        let hit: Vec<usize> = (0..self.cfg.zones.len()).filter(|i| self.loaded.get(*i).map(|l| l.is_some()).unwrap_or(false)).filter(|i| {
            let z = &self.cfg.zones[*i];
            key >= z.lo && key <= z.hi && ((vel * 127.0).round() as u8) >= z.vlo && ((vel * 127.0).round() as u8) <= z.vhi
        }).collect();
        for i in hit {
            let z = self.cfg.zones[i].clone();
            let Some(Some(l)) = self.loaded.get(i).cloned() else { continue };
            let (s, e) = self.zone_range(i);
            if e <= s {
                continue;
            }
            if self.voices.len() >= MAX_VOICES {
                if let Some(k) = (0..self.voices.len()).min_by_key(|k| self.voices[*k].age) {
                    self.voices.remove(k);
                }
            }
            let semis = (key as f32 - z.root as f32) * z.track + z.tune / 100.0 + self.cfg.tune + slice.map(|x| x.2 / 100.0).unwrap_or(0.0);
            let sl = slice.and_then(|(a, b, _)| {
                let (a, b) = ((a as usize).max(s), (b as usize).min(e));
                (b > a + 16).then(|| (a as f64, b as f64, ((b - a) as f64 * 0.2).min(l.rate as f64 * 0.025).max(8.0)))
            });
            let step = l.rate as f64 / self.sr as f64 * 2f64.powf(semis as f64 / 12.0);
            // random variation: every note draws its own volume, pan and filter
            let (rv, rp, rc, rr) = (self.rng.next(), self.rng.next(), self.rng.next(), self.rng.next());
            let p = (z.pan + pan + rp * self.cfg.rnd_pan).clamp(-1.0, 1.0);
            let cut_v = (self.cfg.cutoff + rc * self.cfg.rnd_cut * 0.5).clamp(0.0, 1.0);
            let res_v = (self.cfg.reso + rr * self.cfg.rnd_res * 0.5).clamp(0.0, 1.0);
            let (pl, pr) = pan_gains(p);
            let gain = 10f32.powf((z.vol + self.cfg.gain) / 20.0) * vel.clamp(0.0, 1.0).powf(0.8) * (1.0 - self.cfg.rnd_vol * (rv * 0.5 + 0.5));
            self.clock += 1;
            self.voices.push(Voice {
                zone: i,
                key,
                pos: match sl {
                    Some((a, b, _)) => if z.reverse { b - 1.0 } else { a },
                    None => if z.reverse { (e - 1) as f64 } else { s as f64 },
                },
                dir: if z.reverse { -1.0 } else { 1.0 },
                step,
                level: if atk_s(self.cfg.attack) < 0.002 { 1.0 } else { 0.0 },
                stage: if atk_s(self.cfg.attack) < 0.002 { Stage::Decay } else { Stage::Attack },
                gain,
                pan_l: pl,
                pan_r: pr,
                age: self.clock,
                held: true,
                env_t: 0.0,
                pan_t: 0.0,
                pitch_t: 0.0,
                filt_t: 0.0,
                pan_base: p,
                cut_v: cut_v,
                res_v: res_v,
                fade: 1.0,
                cut: hard,
                lp1: [0.0; 2],
                lp2: [0.0; 2],
                slice: sl,
                pb: crate::dsp::Smooth::at(0.0),
                pr: crate::dsp::Smooth::at(0.0),
                tm: crate::dsp::Smooth::at(0.5),
                fresh: true,
            });
        }
    }
}

impl Instrument for Sampler {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.start_voice(key, vel, 0.0, false);
    }
    fn note_on_ex(&mut self, key: u8, vel: f32, pan: f32, _cutoff: f32, reso: f32) {
        // a note with its "reso" at 0 is a cut (tracker imports): no release tail
        self.start_voice(key, vel, pan, reso < 0.01);
    }
    fn note_on_lyric(&mut self, key: u8, vel: f32, pan: f32, _cutoff: f32, reso: f32, lyric: &str, _len_samples: f32) {
        // "@start:end:cents" plays a slice of the sample (vocal chops); anything else is an ordinary note
        match parse_slice(lyric) {
            Some(sl) => self.start_voice_slice(key, vel, pan, reso < 0.01, Some(sl)),
            None => self.start_voice(key, vel, pan, reso < 0.01),
        }
    }
    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.held) {
            v.held = false;
            let one = self.cfg.zones.get(v.zone).map(|z| z.one_shot).unwrap_or(false);
            if !one {
                v.stage = Stage::Release;
                if self.cfg.env.len() >= 2 {
                    v.env_t = self.cfg.env_eff(v.env_t, true);
                }
                if self.cfg.pan_env.on() {
                    v.pan_t = self.cfg.pan_env.eff(v.pan_t, true);
                }
                if self.cfg.pitch_env.on() {
                    v.pitch_t = self.cfg.pitch_env.eff(v.pitch_t, true);
                }
                if self.cfg.filter_env.on() {
                    v.filt_t = self.cfg.filter_env.eff(v.filt_t, true);
                }
            }
        }
    }
    fn all_off(&mut self) {
        self.voices.clear();
    }
    fn note_pitch(&mut self, key: u8, semis: f32) {
        for v in self.voices.iter_mut().filter(|v| v.held && v.key == key) {
            v.pb.set(semis, v.fresh);
        }
    }
    fn note_pressure(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.held && v.key == key) {
            v.pr.set(x, v.fresh);
        }
    }
    fn note_timbre(&mut self, key: u8, x: f32) {
        for v in self.voices.iter_mut().filter(|v| v.held && v.key == key) {
            v.tm.set(x, v.fresh);
        }
    }
    fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        let (a, d, s, r) = (atk_s(self.cfg.attack), dec_s(self.cfg.decay), self.cfg.sustain.clamp(0.0, 1.0), rel_s(self.cfg.release));
        let (da, dd, dr) = (1.0 / (a * self.sr).max(1.0), 1.0 / (d * self.sr).max(1.0), 1.0 / (r * self.sr).max(1.0));
        let filt_on = self.cfg.filter_env.on();
        let swing = self.cfg.rnd_cut > 0.0 || self.cfg.rnd_res > 0.0;
        let filt = self.cfg.cutoff < 0.995 || filt_on || swing;
        let mut fc = cut_hz(self.cfg.cutoff);
        let mut kq = 1.0 / (0.6 + self.cfg.reso * 7.0);
        let use_env = self.cfg.env.len() >= 2;
        let (pan_on, pitch_on, filt_on) = (self.cfg.pan_env.on(), self.cfg.pitch_env.on(), self.cfg.filter_env.on());
        let dt = 1.0 / self.sr;
        let fade_step = if self.cfg.fade > 0.0 { dt / self.cfg.fade } else { 0.0 };
        let mut dead: Vec<usize> = vec![];
        // per-sample effects: the zones that have a rack play into their own buffer, which goes through it afterwards
        let sr_ = self.sr;
        self.zins.truncate(self.cfg.zones.len());
        while self.zins.len() < self.cfg.zones.len() {
            self.zins.push(Insert::new(sr_));
        }
        let mut fxz: Vec<bool> = Vec::with_capacity(self.cfg.zones.len());
        for (i, z) in self.cfg.zones.iter().enumerate() {
            self.zins[i].set_params(z.fx);
            fxz.push(self.zins[i].active());
        }
        let any_fx = fxz.iter().any(|b| *b);
        let mut zb = std::mem::take(&mut self.zbuf);
        if any_fx {
            zb.clear();
            zb.resize(self.cfg.zones.len() * frames * 2, 0.0);
        }
        let bend_mul = 2f64.powf(((self.bend as f64 - 0.5) * 2.0 * (1.0 + self.bend_range as f64 * 23.0)) / 12.0);
        for (vi, v) in self.voices.iter_mut().enumerate() {
            let Some(Some(l)) = self.loaded.get(v.zone) else {
                dead.push(vi);
                continue;
            };
            let z = &self.cfg.zones[v.zone];
            let (st, en) = match v.slice {
                Some((a, b, _)) => (a as usize, (b as usize).min(l.frames())),
                None => {
                    let end = if z.end == 0 { l.frames() } else { (z.end as usize).min(l.frames()) };
                    ((z.start as usize).min(end), end)
                }
            };
            let looped = z.loop_mode > 0 && z.loop_end > z.loop_start + 8 && (z.loop_end as usize) <= l.frames();
            let (ls, le) = (z.loop_start as f64, z.loop_end as f64);
            v.pb.begin(frames);
            v.pr.begin(frames);
            v.tm.begin(frames);
            v.fresh = false;
            let expr = !(v.pb.idle(0.0) && v.pr.idle(0.0) && v.tm.idle(0.5));
            for f in 0..frames {
                // the envelope
                // the pan and pitch envelopes
                let mut env_pitch = 1.0f64;
                if swing {
                    fc = cut_hz(v.cut_v);
                    kq = 1.0 / (0.6 + v.res_v * 7.0);
                }
                let (mut g_pitch, mut g_amp, mut g_cut) = (1.0f64, 1.0f32, 1.0f32);
                if expr {
                    g_pitch = 2f64.powf(v.pb.tick() as f64 / 12.0);
                    g_amp = 1.0 + v.pr.tick() * 0.5;
                    // timbre moves the cutoff up to two octaves either way
                    g_cut = 2f32.powf((v.tm.tick() - 0.5) * 4.0);
                }
                if filt_on {
                    // Impulse Tracker: the envelope scales the base cutoff; its frequency is 110 * 2^(0.25 + cutoff / 24)
                    v.filt_t += dt;
                    let tt = self.cfg.filter_env.eff(v.filt_t, v.stage != Stage::Release);
                    let c = (self.cfg.it_cut * self.cfg.filter_env.at(tt)).clamp(0.0, 127.0);
                    let hz = 110.0 * 2f32.powf(0.25 + c / 24.0);
                    fc = hz;
                }
                if pan_on || pitch_on {
                    let held_now = v.stage != Stage::Release;
                    if pan_on {
                        v.pan_t += dt;
                        let tt = self.cfg.pan_env.eff(v.pan_t, held_now);
                        let p = (v.pan_base + self.cfg.pan_env.at(tt)).clamp(-1.0, 1.0);
                        let (pl, pr) = pan_gains(p);
                        v.pan_l = pl;
                        v.pan_r = pr;
                    }
                    if pitch_on {
                        v.pitch_t += dt;
                        let tt = self.cfg.pitch_env.eff(v.pitch_t, held_now);
                        env_pitch = 2f64.powf(self.cfg.pitch_env.at(tt) as f64 / 12.0);
                    }
                }
                if use_env {
                    v.env_t += dt;
                    let held = v.stage != Stage::Release;
                    let t = self.cfg.env_eff(v.env_t, held);
                    v.level = self.cfg.env_at(t);
                    if !held {
                        // after the key-off the envelope runs on and the fade-out takes the sound down; with no fade-out the sound ends when it does
                        if v.cut {
                            v.fade -= dt / 0.006;
                        } else if fade_step > 0.0 {
                            v.fade -= fade_step;
                        } else if v.env_t >= self.cfg.env_end() {
                            v.fade = 0.0;
                        }
                        if self.cfg.fade <= 0.0 && v.env_t < self.cfg.env_end() {
                            // no fade-out: FastTracker cuts the sound at the key-off unless the envelope carries on; it does here
                        }
                        v.level *= v.fade.max(0.0);
                        if v.level <= 0.0004 && (v.fade <= 0.0 || v.env_t >= self.cfg.env_end() || v.cut) {
                            dead.push(vi);
                            break;
                        }
                    }
                } else {
                match v.stage {
                    Stage::Attack => {
                        v.level += da;
                        if v.level >= 1.0 {
                            v.level = 1.0;
                            v.stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => {
                        if v.level > s {
                            v.level = (v.level - dd * (1.0 - s).max(0.02)).max(s);
                        }
                        if v.level <= s + 1e-4 {
                            v.stage = Stage::Sustain;
                        }
                    }
                    Stage::Sustain => {}
                    Stage::Release => {
                        v.level -= dr * v.level.max(0.05) * 1.5 * if v.cut { 20.0 } else { 1.0 };
                        if v.level <= 0.0005 {
                            dead.push(vi);
                            break;
                        }
                    }
                }
                }
                // position, loop and end
                let mut p = v.pos;
                if looped && (v.held || z.loop_mode > 0) {
                    if v.dir > 0.0 && p >= le {
                        if z.loop_mode == 2 {
                            v.dir = -1.0;
                            p = le - (p - le);
                        } else {
                            p = ls + (p - le);
                        }
                    } else if v.dir < 0.0 && p <= ls {
                        v.dir = 1.0;
                        p = ls + (ls - p);
                    }
                }
                v.pos = p;
                if p < st as f64 || (!looped && p >= (en - 1) as f64) || p >= en as f64 {
                    dead.push(vi);
                    break;
                }
                let i0 = p.floor() as usize;
                let fr = (p - i0 as f64) as f32;
                // the frame after: at the very end of a loop it is the loop's start
                let i1 = if i0 + 1 >= en { if looped { z.loop_start as usize } else { i0 } } else { i0 + 1 };
                let (a0l, a0r) = (l.data[i0 * 2], l.data[i0 * 2 + 1]);
                let (a1l, a1r) = (l.data[i1 * 2], l.data[i1 * 2 + 1]);
                let mut sl = a0l + (a1l - a0l) * fr;
                let mut sr_ = a0r + (a1r - a0r) * fr;
                if filt {
                    // a two-pole state-variable low-pass (trapezoidal, stable at every cutoff)
                    let fcx = if g_cut != 1.0 { (fc * g_cut).max(20.0) } else { fc };
                    let gg = (std::f32::consts::PI * fcx.min(self.sr * 0.45) / self.sr).tan();
                    let a1 = 1.0 / (1.0 + gg * (gg + kq));
                    let (a2, a3) = (gg * a1, gg * gg * a1);
                    for (ch, x) in [(0usize, &mut sl), (1usize, &mut sr_)] {
                        let v3 = *x - v.lp2[ch];
                        let v1 = a1 * v.lp1[ch] + a2 * v3;
                        let v2 = v.lp2[ch] + a2 * v.lp1[ch] + a3 * v3;
                        v.lp1[ch] = 2.0 * v1 - v.lp1[ch];
                        v.lp2[ch] = 2.0 * v2 - v.lp2[ch];
                        *x = v2;
                    }
                }
                let mut amp = v.level * v.gain;
                if g_amp != 1.0 {
                    amp *= g_amp;
                }
                if let Some((a, b, edge)) = v.slice {
                    // a slice fades in and out at its edges
                    let fin = ((p - a) / (l.rate as f64 * 0.003)).min(1.0);
                    let fout = ((b - p) / edge).min(1.0);
                    amp *= (fin.min(fout)).max(0.0) as f32;
                }
                let (tgt, off): (&mut [f32], usize) = if fxz.get(v.zone).copied().unwrap_or(false) { (&mut zb[..], v.zone * frames * 2) } else { (&mut out[..], 0) };
                tgt[off + f * 2] += sl * amp * v.pan_l;
                tgt[off + f * 2 + 1] += sr_ * amp * v.pan_r;
                if g_pitch != 1.0 {
                    v.pos += v.step * bend_mul * env_pitch * g_pitch * v.dir;
                } else {
                    v.pos += v.step * bend_mul * env_pitch * v.dir;
                }
            }
        }
        dead.sort_unstable();
        dead.dedup();
        for vi in dead.into_iter().rev() {
            if vi < self.voices.len() {
                self.voices.remove(vi);
            }
        }
        if any_fx {
            for (zi, on) in fxz.iter().enumerate() {
                if !*on {
                    continue;
                }
                let off = zi * frames * 2;
                for f in 0..frames {
                    let (l, r) = self.zins[zi].process(zb[off + f * 2], zb[off + f * 2 + 1]);
                    out[f * 2] += l;
                    out[f * 2 + 1] += r;
                }
            }
        }
        self.zbuf = zb;
    }
    fn params(&self) -> Vec<(&'static str, f32)> {
        let c = &self.cfg;
        vec![("attack", c.attack), ("decay", c.decay), ("sustain", c.sustain), ("release", c.release), ("tune", (c.tune / 24.0 + 0.5).clamp(0.0, 1.0)), ("gain", ((c.gain + 24.0) / 36.0).clamp(0.0, 1.0)), ("cutoff", c.cutoff), ("reso", c.reso), ("bend", self.bend), ("bend range", self.bend_range), ("rnd cut", c.rnd_cut), ("rnd res", c.rnd_res), ("rnd vol", c.rnd_vol), ("rnd pan", c.rnd_pan)]
    }
    fn set_param(&mut self, name: &str, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match name {
            "attack" => self.cfg.attack = v,
            "decay" => self.cfg.decay = v,
            "sustain" => self.cfg.sustain = v,
            "release" => self.cfg.release = v,
            "tune" => self.cfg.tune = (v - 0.5) * 24.0,
            "gain" => self.cfg.gain = v * 36.0 - 24.0,
            "cutoff" => self.cfg.cutoff = v,
            "reso" => self.cfg.reso = v,
            "bend" => self.bend = v,
            "bend range" => self.bend_range = v,
            "rnd cut" => self.cfg.rnd_cut = v,
            "rnd res" => self.cfg.rnd_res = v,
            "rnd vol" => self.cfg.rnd_vol = v,
            "rnd pan" => self.cfg.rnd_pan = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "Sampler"
    }
    fn command(&mut self, cmd: &str, args: &[f64], text: &str) -> Vec<f64> {
        let zi = args.first().copied().unwrap_or(0.0).max(0.0) as usize;
        match cmd {
            "count" => vec![self.cfg.zones.len() as f64],
            "add" => match self.add_file(text) {
                Ok(i) => {
                    // a pitched sample gets its root key from its pitch
                    if let Some((k, cents)) = self.detect_root(i) {
                        let z = &mut self.cfg.zones[i];
                        z.root = k;
                        z.tune = -cents;
                    }
                    vec![i as f64]
                }
                Err(e) => {
                    self.last_error = e;
                    vec![-1.0]
                }
            },
            // a library sample as a one-shot zone on one key: args [key, track pitch 0/1]
            "add_lib" => match self.add_file(text) {
                Ok(i) => {
                    let key = args.first().copied().unwrap_or(60.0).clamp(0.0, 127.0) as u8;
                    let z = &mut self.cfg.zones[i];
                    z.lo = key;
                    z.hi = key;
                    z.root = key;
                    z.one_shot = true;
                    z.track = args.get(1).copied().unwrap_or(0.0).clamp(0.0, 1.0) as f32;
                    vec![i as f64]
                }
                Err(e) => {
                    self.last_error = e;
                    vec![-1.0]
                }
            },
            // the zone of another sampler's config that covers middle C, put on one key: args [key]
            "merge" => {
                let Ok(other) = serde_json::from_str::<Config>(text) else { return vec![-1.0] };
                let key = args.first().copied().unwrap_or(60.0).clamp(0.0, 127.0) as u8;
                let Some(src) = other.zones.iter().find(|z| z.lo <= 60 && 60 <= z.hi).or(other.zones.first()) else { return vec![-1.0] };
                match self.add_file(&src.file) {
                    Ok(i) => {
                        let frames = self.cfg.zones[i].loop_end;
                        let z = &mut self.cfg.zones[i];
                        z.name = src.name.clone();
                        z.lo = key;
                        z.hi = key;
                        // the sample plays at the pitch it has when middle C is played: that is the pitch it gets on its new key
                        z.root = (key as i32 - (60 - src.root as i32)).clamp(0, 127) as u8;
                        z.tune = src.tune;
                        z.pan = src.pan;
                        z.one_shot = src.loop_mode == 0;
                        z.loop_mode = src.loop_mode;
                        z.loop_start = src.loop_start;
                        z.loop_end = if src.loop_mode > 0 { src.loop_end.min(frames) } else { frames };
                        vec![i as f64]
                    }
                    Err(e) => {
                        self.last_error = e;
                        vec![-1.0]
                    }
                }
            }
            "remove" => {
                if zi < self.cfg.zones.len() {
                    self.cfg.zones.remove(zi);
                    self.loaded.remove(zi);
                    self.voices.clear();
                    vec![1.0]
                } else {
                    vec![0.0]
                }
            }
            // 20 numbers per zone: root lo hi vlo vhi tune vol pan loop loop_start loop_end one_shot start end reverse track frames rate detected-root-or--1 detected-hz
            // the zone's insert rack, in `insert::NAMES` order (14 numbers; empty for an unknown zone)
            "zfx" => match self.cfg.zones.get(zi) {
                Some(z) => z.fx.list().iter().map(|(_, v)| *v as f64).collect(),
                None => vec![],
            },
            "zones" => {
                let mut v = vec![];
                for (i, z) in self.cfg.zones.iter().enumerate() {
                    let (fr, rate) = self.loaded.get(i).and_then(|l| l.as_ref()).map(|l| (l.frames(), l.rate)).unwrap_or((0, 0));
                    v.extend([z.root as f64, z.lo as f64, z.hi as f64, z.vlo as f64, z.vhi as f64, z.tune as f64, z.vol as f64, z.pan as f64, z.loop_mode as f64, z.loop_start as f64, z.loop_end as f64, z.one_shot as u8 as f64, z.start as f64, z.end as f64, z.reverse as u8 as f64, z.track as f64, fr as f64, rate as f64, 0.0, 0.0]);
                }
                v
            }
            "set" => {
                let (field, val) = (args.get(1).copied().unwrap_or(0.0) as usize, args.get(2).copied().unwrap_or(0.0));
                let fr = self.loaded.get(zi).and_then(|l| l.as_ref()).map(|l| l.frames() as u32).unwrap_or(0);
                let Some(z) = self.cfg.zones.get_mut(zi) else { return vec![0.0] };
                match field {
                    0 => z.root = val.clamp(0.0, 127.0) as u8,
                    1 => z.lo = val.clamp(0.0, 127.0) as u8,
                    2 => z.hi = val.clamp(0.0, 127.0) as u8,
                    3 => z.vlo = val.clamp(0.0, 127.0) as u8,
                    4 => z.vhi = val.clamp(0.0, 127.0) as u8,
                    5 => z.tune = val.clamp(-1200.0, 1200.0) as f32,
                    6 => z.vol = val.clamp(-48.0, 18.0) as f32,
                    7 => z.pan = val.clamp(-1.0, 1.0) as f32,
                    8 => z.loop_mode = val.clamp(0.0, 2.0) as u8,
                    9 => z.loop_start = (val.max(0.0) as u32).min(fr),
                    10 => z.loop_end = (val.max(0.0) as u32).min(fr),
                    11 => z.one_shot = val > 0.5,
                    12 => z.start = (val.max(0.0) as u32).min(fr),
                    13 => z.end = (val.max(0.0) as u32).min(fr),
                    14 => z.reverse = val > 0.5,
                    15 => z.track = val.clamp(0.0, 1.0) as f32,
                    // 100.. : the insert rack knobs
                    100..=113 => {
                        z.fx.set_index(field - 100, val as f32);
                    },
                    _ => {}
                }
                if z.lo > z.hi {
                    std::mem::swap(&mut z.lo, &mut z.hi);
                }
                vec![1.0]
            }
            "detect" => match self.detect_root(zi) {
                Some((k, cents)) => {
                    let z = &mut self.cfg.zones[zi];
                    z.root = k;
                    z.tune = -cents;
                    vec![k as f64, cents as f64]
                }
                None => vec![-1.0],
            },
            "autoloop" => {
                if zi < self.cfg.zones.len() {
                    let ok = self.auto_loop(zi);
                    vec![ok as u8 as f64]
                } else {
                    vec![0.0]
                }
            }
            "spread" => {
                // spread the zones over the keyboard: the boundary between two zones is halfway between their roots
                let mut idx: Vec<usize> = (0..self.cfg.zones.len()).collect();
                idx.sort_by_key(|i| self.cfg.zones[*i].root);
                let roots: Vec<u8> = idx.iter().map(|i| self.cfg.zones[*i].root).collect();
                for (n, &i) in idx.iter().enumerate() {
                    let lo = if n == 0 { 0 } else { ((roots[n - 1] as u16 + roots[n] as u16) / 2 + 1).min(roots[n] as u16) as u8 };
                    let hi = if n + 1 == idx.len() { 127 } else { ((roots[n] as u16 + roots[n + 1] as u16) / 2).max(roots[n] as u16) as u8 };
                    let z = &mut self.cfg.zones[i];
                    z.lo = lo;
                    z.hi = hi;
                }
                vec![1.0]
            }
            // the envelopes: 0 volume, 1 pan, 2 pitch -> [sustain or -1, loop start or -1, loop end or -1, fade-out s, count, (time, value)...]
            "env" => {
                let which = args.first().copied().unwrap_or(0.0) as usize;
                let (pts, sus, lp): (Vec<(f32, f32)>, Option<usize>, Option<(usize, usize)>) = match which {
                    0 => (self.cfg.env.clone(), self.cfg.env_sustain, self.cfg.env_loop),
                    1 => (self.cfg.pan_env.pts.clone(), self.cfg.pan_env.sustain, self.cfg.pan_env.looped),
                    2 => (self.cfg.pitch_env.pts.clone(), self.cfg.pitch_env.sustain, self.cfg.pitch_env.looped),
                    _ => (self.cfg.filter_env.pts.clone(), self.cfg.filter_env.sustain, self.cfg.filter_env.looped),
                };
                let mut v = vec![sus.map(|x| x as f64).unwrap_or(-1.0), lp.map(|x| x.0 as f64).unwrap_or(-1.0), lp.map(|x| x.1 as f64).unwrap_or(-1.0), self.cfg.fade as f64, pts.len() as f64];
                for p in pts {
                    v.extend([p.0 as f64, p.1 as f64]);
                }
                v
            }
            "drop_env" => {
                self.cfg.env.clear();
                self.cfg.env_sustain = None;
                self.cfg.env_loop = None;
                self.cfg.fade = 0.0;
                vec![1.0]
            }
            "peaks" => self.peaks(zi, args.get(1).copied().unwrap_or(256.0).clamp(8.0, 2048.0) as usize),
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            "names" => self.cfg.zones.iter().map(|z| z.name.clone()).collect::<Vec<_>>().join("\n"),
            "files" => self.cfg.zones.iter().map(|z| z.file.clone()).collect::<Vec<_>>().join("\n"),
            "config" => self.config_json(),
            "error" => self.last_error.clone(),
            _ => String::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(hz: f32, secs: f32, rate: u32) -> Vec<f32> {
        (0..(secs * rate as f32) as usize).flat_map(|i| { let v = (i as f32 / rate as f32 * hz * TAU).sin() * 0.7; [v, v] }).collect()
    }

    fn with_sine(hz: f32) -> Sampler {
        let mut s = Sampler::new(48000.0);
        let mut z = Zone::new("sine", "");
        z.loop_end = 48000;
        s.add_loaded(z, sine(hz, 1.0, 48000), 48000);
        s
    }

    fn zero_crossings(buf: &[f32]) -> f32 {
        let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
        l.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32 / (l.len() as f32 / 48000.0)
    }

    fn rms_of(buf: &[f32]) -> f32 {
        (buf.iter().step_by(2).map(|x| x * x).sum::<f32>() / (buf.len() / 2).max(1) as f32).sqrt()
    }

    #[test]
    fn every_sample_has_its_own_effects() {
        // two zones, one sample each: a bright 9 kHz tone on C3 and the same tone on C5; only the first gets a low-pass
        let mut s = Sampler::new(48000.0);
        for (name, lo, hi) in [("a", 0u8, 59u8), ("b", 60, 127)] {
            let mut z = Zone::new(name, "");
            z.lo = lo;
            z.hi = hi;
            z.root = if lo == 0 { 48 } else { 72 };
            z.loop_mode = 1;
            z.loop_start = 4800;
            z.loop_end = 24000;
            s.add_loaded(z, sine(9000.0, 1.0, 48000), 48000);
        }
        let play = |s: &mut Sampler, key: u8| {
            s.all_off();
            s.note_on(key, 1.0);
            let mut buf = vec![0f32; 2 * 9600];
            s.render(&mut buf);
            rms_of(&buf[2 * 4800..])
        };
        let (dry_a, dry_b) = (play(&mut s, 48), play(&mut s, 72));
        assert!(dry_a > 0.2 && dry_b > 0.2, "both sound: {dry_a} {dry_b}");
        // the zone's rack: filter mode low-pass, cutoff low
        assert_eq!(s.command("set", &[0.0, 100.0, 0.4], ""), vec![1.0]);
        assert_eq!(s.command("set", &[0.0, 101.0, 0.3], ""), vec![1.0]);
        let fx = s.command("zfx", &[0.0], "");
        assert_eq!(fx.len(), 14);
        assert!((fx[0] - 0.4).abs() < 1e-6 && (fx[1] - 0.3).abs() < 1e-6, "{fx:?}");
        assert_eq!(s.command("zfx", &[1.0], "")[0], 0.0, "the other zone keeps its defaults");
        let (wet_a, wet_b) = (play(&mut s, 48), play(&mut s, 72));
        assert!(wet_a < dry_a * 0.3, "the filtered sample loses its highs: {wet_a} vs {dry_a}");
        assert!((wet_b - dry_b).abs() < dry_b * 0.02, "the other sample is untouched: {wet_b} vs {dry_b}");
        // an echo keeps sounding after the note is gone
        s.command("set", &[1.0, 108.0, 0.6], "");
        s.command("set", &[1.0, 109.0, 0.2], "");
        s.all_off();
        s.note_on(72, 1.0);
        let mut buf = vec![0f32; 2 * 2400];
        s.render(&mut buf);
        s.all_off();
        let mut tail = vec![0f32; 2 * 14400];
        s.render(&mut tail);
        assert!(rms_of(&tail) > 0.01, "the echo rings on after the voice ended: {}", rms_of(&tail));
        // saved with the project, and a project without the field still loads
        let json = serde_json::to_string(&s.cfg).unwrap();
        assert!(json.contains("\"fx\""));
        let back: Config = serde_json::from_str(&json).unwrap();
        assert_eq!(back.zones[0].fx, s.cfg.zones[0].fx);
        let plain = serde_json::to_string(&Zone::new("x", "")).unwrap();
        assert!(!plain.contains("\"fx\""), "a bypassed rack is not written");
        assert!(serde_json::from_str::<Zone>(&plain).unwrap().fx.is_off());
    }

    #[test]
    fn detects_root_and_cents() {
        let mut s = with_sine(440.0 * 2f32.powf(0.3 / 12.0)); // A4, 30 cents sharp
        let r = s.command("detect", &[0.0], "");
        assert_eq!(r[0], 69.0, "{r:?}");
        assert!((r[1] - 30.0).abs() < 6.0, "{r:?}");
    }

    #[test]
    fn plays_at_the_pitch_of_the_key_and_loops_until_release() {
        let mut s = with_sine(220.0);
        s.command("set", &[0.0, 0.0, 57.0], ""); // root A3
        s.command("set", &[0.0, 8.0, 1.0], ""); // loop
        s.command("set", &[0.0, 9.0, 4800.0], "");
        s.command("set", &[0.0, 10.0, 24000.0], "");
        s.note_on(69, 1.0); // an octave up: 440 Hz
        let mut buf = vec![0f32; 2 * 48000 * 2];
        s.render(&mut buf);
        let hz = zero_crossings(&buf[..2 * 24000]);
        assert!((hz - 440.0).abs() < 12.0, "pitch {hz}");
        // 2 s of sound from a 1 s sample: it looped
        let tail: f32 = buf[2 * 80000..2 * 90000].iter().map(|x| x.abs()).sum();
        assert!(tail > 100.0, "looping keeps sounding: {tail}");
        s.note_off(69);
        let mut rel = vec![0f32; 2 * 48000 * 2];
        s.render(&mut rel);
        let late: f32 = rel[2 * 90000..].iter().map(|x| x.abs()).sum();
        assert!(late < 1.0, "released: {late}");
    }

    #[test]
    fn one_shot_ignores_note_off_and_key_zones_select() {
        let mut s = with_sine(220.0);
        s.command("set", &[0.0, 11.0, 1.0], "");
        s.command("set", &[0.0, 1.0, 60.0], "");
        s.command("set", &[0.0, 2.0, 72.0], "");
        s.note_on(30, 1.0);
        let mut b = vec![0f32; 2048];
        s.render(&mut b);
        assert!(b.iter().all(|x| *x == 0.0), "outside the key range: silent");
        s.note_on(60, 1.0);
        s.note_off(60);
        let mut b = vec![0f32; 2 * 24000];
        s.render(&mut b);
        assert!(b[2 * 20000..].iter().map(|x| x.abs()).sum::<f32>() > 10.0, "one-shot keeps playing after the note off");
    }

    #[test]
    fn config_round_trips() {
        let mut s = with_sine(330.0);
        s.command("set", &[0.0, 5.0, 25.0], "");
        let j = s.config_json();
        let t = Sampler::from_config(48000.0, &j, None).unwrap();
        assert_eq!(t.cfg.zones[0].tune, 25.0);
    }

    fn saw(secs: f32) -> Vec<f32> {
        (0..(secs * 48000.0) as usize).flat_map(|i| { let v = ((i as f32 * 220.0 / 48000.0) % 1.0) * 2.0 - 1.0; [v * 0.6, v * 0.6] }).collect()
    }

    fn hf_energy(buf: &[f32]) -> f32 {
        // energy of the first difference: a crude high-frequency measure
        let l: Vec<f32> = buf.iter().step_by(2).copied().collect();
        l.windows(2).map(|w| (w[1] - w[0]).powi(2)).sum::<f32>()
    }

    #[test]
    fn a_filter_envelope_closes_the_filter_over_time() {
        let mut s = Sampler::new(48000.0);
        let mut z = Zone::new("saw", "");
        z.loop_mode = 1;
        z.loop_start = 4800;
        z.loop_end = 40000;
        s.add_loaded(z, saw(1.0), 48000);
        s.cfg.it_cut = 100.0;
        s.cfg.cutoff = 1.0;
        s.cfg.filter_env = Env { pts: vec![(0.0, 1.0), (0.3, 0.2)], sustain: Some(1), looped: None };
        s.note_on(60, 1.0);
        let mut b = vec![0f32; 2 * 24000];
        s.render(&mut b);
        let early = hf_energy(&b[2 * 200..2 * 4000]);
        let late = hf_energy(&b[2 * 18000..2 * 22000]);
        assert!(late < early * 0.5, "the filter closes: {early} -> {late}");
    }

    #[test]
    fn random_cutoff_gives_every_note_its_own_brightness() {
        let mut s = Sampler::new(48000.0);
        let mut z = Zone::new("saw", "");
        z.loop_mode = 1;
        z.loop_start = 4800;
        z.loop_end = 40000;
        s.add_loaded(z, saw(1.0), 48000);
        s.cfg.cutoff = 0.5;
        s.cfg.rnd_cut = 1.0;
        let mut vals = vec![];
        for _ in 0..6 {
            s.all_off();
            s.note_on(60, 1.0);
            let mut b = vec![0f32; 2 * 4800];
            s.render(&mut b);
            vals.push(hf_energy(&b[2 * 1000..]));
        }
        let (lo, hi) = (vals.iter().cloned().fold(f32::MAX, f32::min), vals.iter().cloned().fold(0.0, f32::max));
        assert!(hi > lo * 1.2, "notes differ: {vals:?}");
        // and with the variation off they are all the same
        s.cfg.rnd_cut = 0.0;
        s.all_off();
        s.note_on(60, 1.0);
        let mut a = vec![0f32; 2 * 4800];
        s.render(&mut a);
        s.all_off();
        s.note_on(60, 1.0);
        let mut c = vec![0f32; 2 * 4800];
        s.render(&mut c);
        assert!((hf_energy(&a[2000..]) - hf_energy(&c[2000..])).abs() < 1e-3);
    }

    #[test]
    fn pan_and_pitch_envelopes_move_the_sound() {
        let mut s = with_sine(220.0);
        s.command("set", &[0.0, 0.0, 57.0], "");
        s.command("set", &[0.0, 8.0, 1.0], "");
        s.command("set", &[0.0, 9.0, 4800.0], "");
        s.command("set", &[0.0, 10.0, 24000.0], "");
        s.cfg.pan_env = Env { pts: vec![(0.0, -1.0), (0.2, 1.0)], sustain: Some(1), looped: None };
        s.note_on(57, 1.0);
        let mut b = vec![0f32; 2 * 24000];
        s.render(&mut b);
        let side = |a: usize, z: usize| -> (f32, f32) { (b[2 * a..2 * z].iter().step_by(2).map(|x| x.abs()).sum(), b[2 * a + 1..2 * z].iter().step_by(2).map(|x| x.abs()).sum()) };
        let (l0, r0) = side(0, 2000);
        let (l1, r1) = side(16000, 20000);
        assert!(l0 > r0 * 1.5, "starts left: {l0} {r0}");
        assert!(r1 > l1 * 1.5, "ends right: {l1} {r1}");
    }
}

