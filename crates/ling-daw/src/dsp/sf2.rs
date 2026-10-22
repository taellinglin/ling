//! SoundFont 2 (.sf2) parser and sample player ("SoundFont" instrument).
//!
//! Supported: presets/instruments/zones with key + velocity ranges and layering, sample loops (mode 1 and 3),
//! the volume envelope (delay, attack, hold, decay, sustain, release incl. key scaling), initial attenuation,
//! pan, coarse/fine tuning, scale tuning, root-key override, the low-pass filter (cutoff + Q) with LFO
//! modulation, vibrato and modulation LFOs (pitch / filter / volume), exclusive classes, velocity and
//! pitch-wheel. Modulators, the modulation envelope and effects sends (chorus/reverb) are not evaluated.
use crate::dsp::{midi_hz, Instrument, TAU};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

#[derive(Clone, Debug)]
pub struct SfSample {
    pub name: String,
    pub start: u32,
    pub end: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub rate: u32,
    pub key: u8,
    pub correction: i8,
}

/// Fully resolved generators of one (preset zone x instrument zone) layer.
#[derive(Clone, Copy, Debug)]
pub struct Layer {
    pub key_lo: u8,
    pub key_hi: u8,
    pub vel_lo: u8,
    pub vel_hi: u8,
    pub sample: usize,
    pub pan: f32,        // -1..1
    pub atten_db: f32,   // >= 0
    pub tune_cents: f32, // coarse + fine
    pub scale: f32,      // cents per key
    pub root: i32,       // -1 = use the sample's
    pub mode: u8,        // 0 none, 1 loop, 3 loop until release
    pub excl: u16,
    pub start_off: i32,
    pub end_off: i32,
    pub loop_start_off: i32,
    pub loop_end_off: i32,
    pub delay: f32,
    pub attack: f32,
    pub hold: f32,
    pub decay: f32,
    pub sustain_db: f32,
    pub release: f32,
    pub key_hold: f32,  // timecents per key from 60
    pub key_decay: f32,
    pub fc: f32, // absolute cents
    pub q_db: f32,
    pub mod_delay: f32,
    pub mod_freq: f32,
    pub vib_delay: f32,
    pub vib_freq: f32,
    pub mod_to_pitch: f32,
    pub vib_to_pitch: f32,
    pub mod_to_fc: f32,
    pub mod_to_vol_db: f32,
    pub fixed_key: i32,
    pub fixed_vel: i32,
}

#[derive(Clone, Debug)]
pub struct Preset {
    pub name: String,
    pub bank: u16,
    pub program: u16,
    pub layers: Vec<Layer>,
}

pub struct SoundFont {
    pub name: String,
    pub data: Arc<Vec<i16>>,
    pub samples: Vec<SfSample>,
    /// sorted by (bank, program)
    pub presets: Vec<Preset>,
}

impl SoundFont {
    /// Index of the preset with exactly this bank / program.
    pub fn find(&self, bank: u16, program: u16) -> Option<usize> {
        self.presets.iter().position(|p| p.bank == bank && p.program == program)
    }
    /// Best preset for a MIDI (bank, program); drums fall back to the percussion bank 128.
    pub fn find_best(&self, bank: u16, program: u16, drums: bool) -> Option<usize> {
        if let Some(i) = self.find(bank, program) {
            return Some(i);
        }
        if drums {
            return self.find(128, program).or_else(|| self.find(128, 0));
        }
        // a bank-less (General MIDI) request only matches a bank-0 preset; anything else falls back to the built-in sounds
        self.find(0, program)
    }
}

// ── parsing ─────────────────────────────────────────────────────────────────
fn u16le(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}
fn i16le(b: &[u8], i: usize) -> i16 {
    i16::from_le_bytes([b[i], b[i + 1]])
}
fn u32le(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn cstr(b: &[u8]) -> String {
    String::from_utf8_lossy(b.split(|c| *c == 0).next().unwrap_or(&[])).trim().to_string()
}

/// Raw generator list: (operator, amount as i16 and as raw bytes)
type Gens = Vec<(u16, i16, [u8; 2])>;

struct Zone {
    gens: Gens,
}

/// Split the bag/gen tables into per-parent zone lists.
fn zones(bags: &[u8], gens: &[u8], first: usize, last: usize) -> Vec<Zone> {
    let nb = bags.len() / 4;
    let ng = gens.len() / 4;
    let mut out = vec![];
    for z in first..last {
        if z + 1 >= nb {
            break;
        }
        let g0 = u16le(bags, z * 4) as usize;
        let g1 = u16le(bags, (z + 1) * 4) as usize;
        let mut gl = Vec::new();
        for g in g0..g1.min(ng) {
            gl.push((u16le(gens, g * 4), i16le(gens, g * 4 + 2), [gens[g * 4 + 2], gens[g * 4 + 3]]));
        }
        out.push(Zone { gens: gl });
    }
    out
}

struct Acc {
    v: [i32; 61],
    key: (u8, u8),
    vel: (u8, u8),
}

fn defaults() -> Acc {
    let mut v = [0i32; 61];
    v[8] = 13500;
    for o in [21, 23, 25, 26, 27, 28, 30, 33, 34, 35, 36, 38] {
        v[o] = -12000;
    }
    v[56] = 100;
    v[58] = -1;
    v[46] = -1;
    v[47] = -1;
    Acc { v, key: (0, 127), vel: (0, 127) }
}

/// Operators that are not summed at preset level.
fn non_additive(op: u16) -> bool {
    matches!(op, 0 | 1 | 2 | 3 | 4 | 12 | 45 | 50 | 41 | 53 | 54 | 57 | 58 | 46 | 47 | 43 | 44)
}

fn apply(acc: &mut Acc, gens: &Gens, preset_level: bool) {
    for (op, amount, raw) in gens {
        let o = *op as usize;
        match *op {
            43 => acc.key = (raw[0].min(127), raw[1].min(127)),
            44 => acc.vel = (raw[0].min(127), raw[1].min(127)),
            _ if o < 61 => {
                if preset_level {
                    if !non_additive(*op) {
                        acc.v[o] += *amount as i32;
                    }
                } else {
                    acc.v[o] = *amount as i32;
                }
            }
            _ => {}
        }
    }
}

fn tc(x: i32) -> f32 {
    2f32.powf(x as f32 / 1200.0)
}

fn layer_from(acc: &Acc, sample: usize) -> Layer {
    let v = &acc.v;
    let hz = |cents: i32| 8.176 * 2f32.powf(cents as f32 / 1200.0);
    Layer {
        key_lo: acc.key.0,
        key_hi: acc.key.1,
        vel_lo: acc.vel.0,
        vel_hi: acc.vel.1,
        sample,
        pan: (v[17] as f32 / 500.0).clamp(-1.0, 1.0),
        atten_db: (v[48] as f32 / 10.0).max(0.0),
        tune_cents: v[51] as f32 * 100.0 + v[52] as f32,
        scale: v[56] as f32,
        root: v[58],
        mode: v[54] as u8,
        excl: v[57].max(0) as u16,
        start_off: v[0] + v[4] * 32768,
        end_off: v[1] + v[12] * 32768,
        loop_start_off: v[2] + v[45] * 32768,
        loop_end_off: v[3] + v[50] * 32768,
        delay: if v[33] <= -12000 { 0.0 } else { tc(v[33]) },
        attack: if v[34] <= -12000 { 0.0 } else { tc(v[34]) },
        hold: if v[35] <= -12000 { 0.0 } else { tc(v[35]) },
        decay: tc(v[36]).min(100.0),
        sustain_db: (v[37] as f32 / 10.0).clamp(0.0, 100.0),
        release: tc(v[38]).min(100.0).max(0.0),
        key_hold: v[39] as f32,
        key_decay: v[40] as f32,
        fc: v[8] as f32,
        q_db: (v[9] as f32 / 10.0).clamp(0.0, 96.0),
        mod_delay: if v[21] <= -12000 { 0.0 } else { tc(v[21]) },
        mod_freq: hz(v[22]),
        vib_delay: if v[23] <= -12000 { 0.0 } else { tc(v[23]) },
        vib_freq: hz(v[24]),
        mod_to_pitch: v[5] as f32,
        vib_to_pitch: v[6] as f32,
        mod_to_fc: v[10] as f32,
        mod_to_vol_db: v[13] as f32 / 10.0,
        fixed_key: v[46],
        fixed_vel: v[47],
    }
}

pub fn parse(data: &[u8]) -> Result<SoundFont, String> {
    if data.len() < 12 || &data[0..4] != b"RIFF" || &data[8..12] != b"sfbk" {
        return Err("not a SoundFont 2 file".into());
    }
    let mut name = String::new();
    let mut smpl: Option<Vec<i16>> = None;
    let mut pdta: HashMap<[u8; 4], &[u8]> = HashMap::new();
    let mut i = 12;
    while i + 8 <= data.len() {
        let id = &data[i..i + 4];
        let sz = u32le(data, i + 4) as usize;
        let body = &data[i + 8..(i + 8 + sz).min(data.len())];
        if id == b"LIST" && body.len() >= 4 {
            let kind = &body[0..4];
            let mut j = 4;
            while j + 8 <= body.len() {
                let cid: [u8; 4] = [body[j], body[j + 1], body[j + 2], body[j + 3]];
                let csz = u32le(body, j + 4) as usize;
                let cb = &body[j + 8..(j + 8 + csz).min(body.len())];
                match (kind, &cid) {
                    (b"INFO", b"INAM") => name = cstr(cb),
                    (b"sdta", b"smpl") => {
                        smpl = Some(cb.chunks_exact(2).map(|c| i16::from_le_bytes([c[0], c[1]])).collect());
                    }
                    (b"pdta", _) => {
                        pdta.insert(cid, cb);
                    }
                    _ => {}
                }
                j += 8 + csz + (csz & 1);
            }
        }
        i += 8 + sz + (sz & 1);
    }
    let smpl = smpl.ok_or("no sample data")?;
    let get = |k: &[u8; 4]| -> Result<&[u8], String> { pdta.get(k).copied().ok_or_else(|| format!("missing {}", String::from_utf8_lossy(k))) };
    let (phdr, pbag, pgen, inst, ibag, igen, shdr) = (get(b"phdr")?, get(b"pbag")?, get(b"pgen")?, get(b"inst")?, get(b"ibag")?, get(b"igen")?, get(b"shdr")?);
    // samples (the last record is the terminator)
    let mut samples = vec![];
    for k in 0..shdr.len() / 46 {
        let r = &shdr[k * 46..];
        samples.push(SfSample { name: cstr(&r[0..20]), start: u32le(r, 20), end: u32le(r, 24), loop_start: u32le(r, 28), loop_end: u32le(r, 32), rate: u32le(r, 36).max(1), key: r[40], correction: r[41] as i8 });
    }
    samples.pop();
    // instruments -> zones
    let ni = inst.len() / 22;
    let mut inst_zones: Vec<(Option<Zone>, Vec<Zone>)> = vec![];
    for k in 0..ni.saturating_sub(1) {
        let (b0, b1) = (u16le(inst, k * 22 + 20) as usize, u16le(inst, (k + 1) * 22 + 20) as usize);
        let mut zs = zones(ibag, igen, b0, b1);
        let global = if zs.first().map(|z| !z.gens.iter().any(|g| g.0 == 53)).unwrap_or(false) { Some(zs.remove(0)) } else { None };
        inst_zones.push((global, zs));
    }
    // presets
    let np = phdr.len() / 38;
    let mut presets = vec![];
    for k in 0..np.saturating_sub(1) {
        let r = &phdr[k * 38..];
        let (program, bank) = (u16le(r, 20), u16le(r, 22));
        let (b0, b1) = (u16le(r, 24) as usize, u16le(phdr, (k + 1) * 38 + 24) as usize);
        let mut zs = zones(pbag, pgen, b0, b1);
        let pglobal = if zs.first().map(|z| !z.gens.iter().any(|g| g.0 == 41)).unwrap_or(false) { Some(zs.remove(0)) } else { None };
        let mut layers = vec![];
        for pz in &zs {
            let Some(&(_, inst_id, _)) = pz.gens.iter().find(|g| g.0 == 41) else { continue };
            let Some((iglobal, izs)) = inst_zones.get(inst_id as usize) else { continue };
            for iz in izs {
                let Some(&(_, sid, _)) = iz.gens.iter().find(|g| g.0 == 53) else { continue };
                if sid as usize >= samples.len() {
                    continue;
                }
                let mut acc = defaults();
                if let Some(g) = iglobal {
                    apply(&mut acc, &g.gens, false);
                }
                apply(&mut acc, &iz.gens, false);
                // ranges of the layer = intersection of preset zone and instrument zone
                let (ik, iv) = (acc.key, acc.vel);
                let mut pacc = Acc { v: [0; 61], key: (0, 127), vel: (0, 127) };
                if let Some(g) = &pglobal {
                    apply(&mut pacc, &g.gens, true);
                }
                apply(&mut pacc, &pz.gens, true);
                for o in 0..61 {
                    if !non_additive(o as u16) {
                        acc.v[o] += pacc.v[o];
                    }
                }
                acc.key = (ik.0.max(pacc.key.0), ik.1.min(pacc.key.1));
                acc.vel = (iv.0.max(pacc.vel.0), iv.1.min(pacc.vel.1));
                if acc.key.0 > acc.key.1 || acc.vel.0 > acc.vel.1 {
                    continue;
                }
                layers.push(layer_from(&acc, sid as usize));
            }
        }
        if !layers.is_empty() {
            presets.push(Preset { name: cstr(&r[0..20]), bank, program, layers });
        }
    }
    presets.sort_by_key(|p| (p.bank, p.program));
    Ok(SoundFont { name, data: Arc::new(smpl), samples, presets })
}

static CACHE: OnceLock<Mutex<HashMap<String, Arc<SoundFont>>>> = OnceLock::new();

/// Load (or fetch from the cache) a soundfont file.
pub fn load(path: &str) -> Result<Arc<SoundFont>, String> {
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    if let Some(sf) = cache.lock().unwrap_or_else(|p| p.into_inner()).get(path) {
        return Ok(sf.clone());
    }
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let sf = Arc::new(parse(&bytes)?);
    cache.lock().unwrap_or_else(|p| p.into_inner()).insert(path.to_string(), sf.clone());
    Ok(sf)
}

// ── the player ──────────────────────────────────────────────────────────────
const VOICES: usize = 64;

#[derive(Clone, Copy, PartialEq)]
enum Stage {
    Delay,
    Attack,
    Hold,
    Decay,
    Sustain,
    Release,
    Off,
}

/// Trapezoidal state-variable low-pass.
#[derive(Clone, Copy, Default)]
struct Lp {
    s1: f32,
    s2: f32,
}
impl Lp {
    #[inline]
    fn tick(&mut self, x: f32, g: f32, k: f32) -> f32 {
        let a1 = 1.0 / (1.0 + g * (g + k));
        let a2 = g * a1;
        let a3 = g * a2;
        let v3 = x - self.s2;
        let v1 = a1 * self.s1 + a2 * v3;
        let v2 = self.s2 + a2 * self.s1 + a3 * v3;
        self.s1 = 2.0 * v1 - self.s1;
        self.s2 = 2.0 * v2 - self.s2;
        v2
    }
}

#[derive(Clone)]
struct Voice {
    stage: Stage,
    key: u8,
    layer: Layer,
    pos: f64,
    end: f64,
    loop_s: f64,
    loop_e: f64,
    looping: bool,
    released: bool,
    base_cents: f32,
    rate_ratio: f64,
    step: f64,
    amp: f32,
    env: f32,
    sustain: f32,
    t: f32,
    atk_left: f32,
    hold_left: f32,
    delay_left: f32,
    dec_k: f32,
    rel_k: f32,
    gl: f32,
    gr: f32,
    lfo_m: f32,
    lfo_v: f32,
    lp: Lp,
    age: u64,
    tick: u32,
    g: f32,
    kq: f32,
    use_filter: bool,
}

pub struct Sf2Player {
    sf: Arc<SoundFont>,
    path: String,
    sr: f32,
    preset: usize,
    bank: u16,
    program: u16,
    gain: f32,
    tune: f32,
    bend: f32,
    bend_range: f32,
    env_a: f32,
    env_r: f32,
    voices: Vec<Voice>,
    clock: u64,
}

impl Sf2Player {
    pub fn new(sr: f32, path: &str) -> Result<Sf2Player, String> {
        let sf = load(path)?;
        let preset = sf.find_best(0, 0, false).or_else(|| if sf.presets.is_empty() { None } else { Some(0) }).ok_or("the soundfont has no presets")?;
        let (bank, program) = (sf.presets[preset].bank, sf.presets[preset].program);
        let blank = Voice {
            stage: Stage::Off,
            key: 0,
            layer: layer_from(&defaults(), 0),
            pos: 0.0,
            end: 0.0,
            loop_s: 0.0,
            loop_e: 0.0,
            looping: false,
            released: false,
            base_cents: 0.0,
            rate_ratio: 1.0,
            step: 1.0,
            amp: 0.0,
            env: 0.0,
            sustain: 1.0,
            t: 0.0,
            atk_left: 0.0,
            hold_left: 0.0,
            delay_left: 0.0,
            dec_k: 1.0,
            rel_k: 1.0,
            gl: 0.7,
            gr: 0.7,
            lfo_m: 0.0,
            lfo_v: 0.0,
            lp: Lp::default(),
            age: 0,
            tick: 0,
            g: 0.0,
            kq: 1.0,
            use_filter: false,
        };
        Ok(Sf2Player { sf, path: path.to_string(), sr, preset, bank, program, gain: 0.7, tune: 0.5, bend: 0.5, bend_range: 1.0 / 23.0, env_a: 0.5, env_r: 0.5, voices: vec![blank; VOICES], clock: 0 })
    }

    pub fn soundfont(&self) -> &Arc<SoundFont> {
        &self.sf
    }

    /// Pick the best preset for a MIDI bank / program; returns whether something was found.
    pub fn select(&mut self, bank: u16, program: u16, drums: bool) -> bool {
        match self.sf.find_best(bank, program, drums) {
            Some(i) => {
                self.preset = i;
                self.bank = self.sf.presets[i].bank;
                self.program = self.sf.presets[i].program;
                true
            }
            None => false,
        }
    }

    fn start_voice(&mut self, layer: Layer, key: u8, vel: f32, pan: f32) {
        self.clock += 1;
        let idx = self
            .voices
            .iter()
            .position(|v| v.stage == Stage::Off)
            .or_else(|| self.voices.iter().enumerate().filter(|(_, v)| v.stage == Stage::Release).min_by_key(|(_, v)| v.age).map(|(i, _)| i))
            .or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i))
            .unwrap();
        let s = &self.sf.samples[layer.sample];
        let len = self.sf.data.len() as i64;
        let start = (s.start as i64 + layer.start_off as i64).clamp(0, len) as f64;
        let end = (s.end as i64 + layer.end_off as i64).clamp(0, len) as f64;
        let ls = (s.loop_start as i64 + layer.loop_start_off as i64).clamp(0, len) as f64;
        let le = (s.loop_end as i64 + layer.loop_end_off as i64).clamp(0, len) as f64;
        let looping = (layer.mode == 1 || layer.mode == 3) && le > ls + 8.0 && le <= end + 1.0;
        let fixed = if layer.fixed_key >= 0 { layer.fixed_key } else { key as i32 };
        let root = if layer.root >= 0 { layer.root } else if s.key <= 127 { s.key as i32 } else { 60 };
        let cents = (fixed - root) as f32 * layer.scale + layer.tune_cents + s.correction as f32;
        let vel_i = if layer.fixed_vel >= 0 { layer.fixed_vel as f32 / 127.0 } else { vel };
        let kk = (fixed - 60) as f32;
        let hold = layer.hold * 2f32.powf(layer.key_hold * -kk / 1200.0).max(0.0) * 2f32.powf((self.env_a - 0.5) * 4.0);
        let decay = (layer.decay * 2f32.powf(layer.key_decay * -kk / 1200.0)).max(0.001) * 1.0;
        let atk = layer.attack * 2f32.powf((self.env_a - 0.5) * 4.0);
        let rel = (layer.release * 2f32.powf((self.env_r - 0.5) * 4.0)).clamp(0.002, 60.0);
        let sr = self.sr;
        let v = &mut self.voices[idx];
        let pa = ((layer.pan + pan).clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
        let a_lin = 10f32.powf(-layer.atten_db / 20.0) * vel_i * vel_i;
        *v = Voice {
            stage: if layer.delay > 0.0 { Stage::Delay } else { Stage::Attack },
            key,
            layer,
            pos: start,
            end,
            loop_s: ls,
            loop_e: le,
            looping,
            released: false,
            base_cents: cents,
            rate_ratio: s.rate as f64 / sr as f64,
            step: 1.0,
            amp: a_lin * std::f32::consts::SQRT_2,
            env: 0.0,
            sustain: 10f32.powf(-layer.sustain_db / 20.0),
            t: 0.0,
            atk_left: atk.max(0.0),
            hold_left: hold,
            delay_left: layer.delay,
            dec_k: (-11.5129 / (decay * sr)).exp(),
            rel_k: (-11.5129 / (rel * sr)).exp(),
            gl: pa.cos(),
            gr: pa.sin(),
            lfo_m: 0.0,
            lfo_v: 0.0,
            lp: Lp::default(),
            age: self.clock,
            tick: 0,
            g: 0.0,
            kq: 1.0,
            use_filter: layer.fc < 13300.0 || layer.mod_to_fc != 0.0,
        };
        if v.stage == Stage::Attack && atk <= 0.0 {
            v.env = 1.0;
            v.atk_left = 0.0;
        }
        if v.layer.excl != 0 {
            let (ex, me) = (v.layer.excl, idx);
            for (i, o) in self.voices.iter_mut().enumerate() {
                if i != me && o.stage != Stage::Off && o.layer.excl == ex {
                    o.stage = Stage::Release;
                    o.released = true;
                }
            }
        }
    }
}

impl Instrument for Sf2Player {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.note_on_ex(key, vel, 0.0, 0.5, 0.5);
    }
    fn note_on_ex(&mut self, key: u8, vel: f32, pan: f32, _cutoff: f32, _reso: f32) {
        let vi = (vel * 127.0).round().clamp(1.0, 127.0) as u8;
        let layers: Vec<Layer> = self.sf.presets[self.preset].layers.iter().filter(|l| key >= l.key_lo && key <= l.key_hi && vi >= l.vel_lo && vi <= l.vel_hi).copied().collect();
        for l in layers {
            self.start_voice(l, key, vel, pan);
        }
    }
    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.key == key && !v.released && v.stage != Stage::Off) {
            v.released = true;
            v.stage = Stage::Release;
            if v.layer.mode == 3 {
                v.looping = false;
            }
        }
    }
    fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            if v.stage != Stage::Off {
                v.released = true;
                v.stage = Stage::Release;
                v.looping = false;
            }
        }
    }

    fn render(&mut self, out: &mut [f32]) {
        let sr = self.sr;
        let dt = 1.0 / sr;
        let data = self.sf.data.clone();
        let gain = self.gain * 1.3;
        let glob_cents = (self.tune - 0.5) * 200.0 + (self.bend - 0.5) * 2.0 * (1.0 + self.bend_range * 23.0) * 100.0;
        for v in self.voices.iter_mut() {
            if v.stage == Stage::Off {
                continue;
            }
            let l = v.layer;
            for fr in out.chunks_exact_mut(2) {
                // every 16 samples: LFOs, pitch step and filter coefficients
                if v.tick % 16 == 0 {
                    v.t += 16.0 * dt;
                    v.lfo_m = if v.t > l.mod_delay { (((v.t - l.mod_delay) * l.mod_freq).fract() * 4.0 - 2.0).abs() - 1.0 } else { 0.0 };
                    v.lfo_v = if v.t > l.vib_delay { (((v.t - l.vib_delay) * l.vib_freq).fract() * 4.0 - 2.0).abs() - 1.0 } else { 0.0 };
                    let cents = v.base_cents + glob_cents + v.lfo_m * l.mod_to_pitch + v.lfo_v * l.vib_to_pitch;
                    v.step = v.rate_ratio * (cents as f64 / 1200.0).exp2();
                    if v.use_filter {
                        let fc = l.fc + v.lfo_m * l.mod_to_fc;
                        let hz = (8.176 * (fc / 1200.0).exp2()).clamp(20.0, sr * 0.45);
                        v.g = (std::f32::consts::PI * hz / sr).tan();
                        v.kq = 1.0 / 10f32.powf(l.q_db / 40.0); // resonance peak of q_db above DC gain
                    }
                }
                v.tick = v.tick.wrapping_add(1);
                // envelope
                match v.stage {
                    Stage::Delay => {
                        v.delay_left -= dt;
                        if v.delay_left <= 0.0 {
                            v.stage = Stage::Attack;
                        }
                    }
                    Stage::Attack => {
                        if v.atk_left > 0.0 {
                            v.env += dt / v.atk_left.max(1e-4) * if v.env < 1.0 { 1.0 } else { 0.0 };
                            if v.env >= 1.0 {
                                v.env = 1.0;
                                v.stage = Stage::Hold;
                            }
                        } else {
                            v.env = 1.0;
                            v.stage = Stage::Hold;
                        }
                    }
                    Stage::Hold => {
                        v.hold_left -= dt;
                        if v.hold_left <= 0.0 {
                            v.stage = Stage::Decay;
                        }
                    }
                    Stage::Decay => {
                        v.env *= v.dec_k;
                        if v.env <= v.sustain {
                            v.env = v.sustain;
                            v.stage = Stage::Sustain;
                        }
                    }
                    Stage::Sustain => {}
                    Stage::Release => {
                        v.env *= v.rel_k;
                        if v.env < 1e-5 {
                            v.stage = Stage::Off;
                            break;
                        }
                    }
                    Stage::Off => break,
                }
                // sample read (linear interpolation)
                let i = v.pos as usize;
                if v.pos >= v.end || i + 1 >= data.len() {
                    v.stage = Stage::Off;
                    break;
                }
                let f = (v.pos - i as f64) as f32;
                let a = data[i] as f32;
                let b = data[i + 1] as f32;
                let mut s = (a + (b - a) * f) * (1.0 / 32768.0);
                if v.use_filter {
                    s = v.lp.tick(s, v.g, v.kq);
                }
                let trem = if l.mod_to_vol_db != 0.0 { 10f32.powf(v.lfo_m * l.mod_to_vol_db / 20.0) } else { 1.0 };
                let y = s * v.env * v.amp * gain * trem;
                fr[0] += y * v.gl;
                fr[1] += y * v.gr;
                v.pos += v.step;
                if v.looping && v.pos >= v.loop_e {
                    v.pos -= v.loop_e - v.loop_s;
                }
            }
        }
    }

    fn params(&self) -> Vec<(&'static str, f32)> {
        vec![
            ("bank", self.bank as f32 / 128.0),
            ("program", self.program as f32 / 127.0),
            ("gain", self.gain),
            ("tune", self.tune),
            ("attack", self.env_a),
            ("release", self.env_r),
            ("bend", self.bend),
            ("bend range", self.bend_range),
        ]
    }
    fn set_param(&mut self, n: &str, v: f32) {
        if !v.is_finite() {
            return;
        }
        let v = v.clamp(0.0, 1.0);
        match n {
            "bank" => {
                let b = (v * 128.0).round() as u16;
                if self.sf.find(b, self.program).is_some() {
                    self.select(b, self.program, false);
                } else if let Some(i) = self.sf.presets.iter().position(|p| p.bank == b) {
                    self.preset = i;
                    self.bank = b;
                    self.program = self.sf.presets[i].program;
                }
            }
            "program" => {
                let p = (v * 127.0).round() as u16;
                if let Some(i) = self.sf.find(self.bank, p) {
                    self.preset = i;
                    self.program = p;
                }
            }
            "gain" => self.gain = v,
            "tune" => self.tune = v,
            "attack" => self.env_a = v,
            "release" => self.env_r = v,
            "bend" => self.bend = v,
            "bend range" => self.bend_range = v,
            _ => {}
        }
    }
    fn name(&self) -> &'static str {
        "SoundFont"
    }
    fn source_path(&self) -> String {
        self.path.clone()
    }
    fn preset_names(&self) -> Vec<String> {
        self.sf.presets.iter().map(|p| format!("{:03}:{:03}  {}", p.bank, p.program, p.name)).collect()
    }
    fn preset_index(&self) -> usize {
        self.preset
    }
    fn select_preset_index(&mut self, i: usize) {
        if let Some(p) = self.sf.presets.get(i) {
            self.preset = i;
            self.bank = p.bank;
            self.program = p.program;
        }
    }
    fn midi_select(&mut self, bank: u16, program: u16, drums: bool) -> bool {
        self.select(bank, program, drums)
    }
}

/// Helper used by tests and tools: frequency of a key (kept here so the module is self-contained).
pub fn key_hz(k: u8) -> f32 {
    midi_hz(k as f32)
}

#[allow(dead_code)]
const _TAU: f32 = TAU;
