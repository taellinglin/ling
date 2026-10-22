//! Tracker module import: ProTracker / FastTracker / ScreamTracker / Impulse Tracker / OpenMPT files (`.mod .s3m .xm .it .mptm`).
//!
//! A module is a set of samples and a grid of patterns. The importer keeps both:
//!
//! * every sample (8 / 16-bit, mono, stereo mixed down, IT214 compressed too) goes into a SoundFont 2 file written next to the project, one
//!   preset per tracker instrument (note-to-sample maps become key zones, loops loop, the fade-out becomes the release), so the DAW's own
//!   SoundFont player plays the module's real sounds;
//! * the patterns are played through in song order (position jumps and pattern breaks followed, repeats cut off) and every note becomes a
//!   MIDI-style note on the track of its instrument, with its volume, its panning, its length (until the next note on that channel, a
//!   note-off, or a note cut) and the song's tempo changes.
//!
//! Pitch bends, vibrato, arpeggios, sample offsets and volume slides are not modelled (they are counted in the report). OpenMPT's
//! `.mptm` is Impulse Tracker's format with extensions and reads as `.it`; MO3 (compressed) is not supported.
use crate::midi_import::{ImportedMidi, ImportedTrack};
use crate::dsp::sampler::{Config, Zone};
use crate::model::Note;
use std::collections::HashSet;

// ── bytes ─────────────────────────────────────────────────────────────────────

struct Rd<'a> {
    d: &'a [u8],
}
impl Rd<'_> {
    fn u8(&self, o: usize) -> u8 {
        self.d.get(o).copied().unwrap_or(0)
    }
    fn u16(&self, o: usize) -> u16 {
        self.u8(o) as u16 | (self.u8(o + 1) as u16) << 8
    }
    fn u16be(&self, o: usize) -> u16 {
        (self.u8(o) as u16) << 8 | self.u8(o + 1) as u16
    }
    fn u32(&self, o: usize) -> u32 {
        self.u16(o) as u32 | (self.u16(o + 2) as u32) << 16
    }
    fn i8(&self, o: usize) -> i8 {
        self.u8(o) as i8
    }
    fn text(&self, o: usize, n: usize) -> String {
        let end = (o + n).min(self.d.len());
        if o >= end {
            return String::new();
        }
        let s: String = self.d[o..end].iter().take_while(|b| **b != 0).map(|b| if *b >= 32 && *b < 127 { *b as char } else { ' ' }).collect();
        s.trim().to_string()
    }
    fn slice(&self, o: usize, n: usize) -> &[u8] {
        let a = o.min(self.d.len());
        &self.d[a..(a + n).min(self.d.len())]
    }
}

// ── the module ────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct Smp {
    pub name: String,
    /// mono, 16-bit
    pub data: Vec<i16>,
    /// sample rate at the sample's own pitch (the root note)
    pub rate: u32,
    pub loop_start: u32,
    pub loop_end: u32,
    pub looped: bool,
    /// default volume 0..64
    pub volume: u8,
    pub pan: Option<f32>,
    /// fine tuning in cents
    pub cents: f32,
    /// semitones the sample sounds higher than its root (XM relative note)
    pub rel_note: i32,
    /// the loop runs forward and back
    pub pingpong: bool,
    /// the sample's own global volume in dB (IT)
    pub vol_db: f32,
}

#[derive(Clone, Debug, Default)]
pub struct Ins {
    pub name: String,
    pub smps: Vec<Smp>,
    /// MIDI key 0..127 -> index into `smps` (255 = silent)
    pub map: Vec<u8>,
    /// per key: semitones the key plays above itself (Impulse Tracker's note table)
    pub shift: Vec<i8>,
    pub fadeout_s: f32,
    /// the volume envelope: (tick, volume 0..64) points, the sustain point and whether it loops / sustains
    pub env: Vec<(u16, u8)>,
    pub env_sustain: Option<usize>,
    pub env_loop: Option<(usize, usize)>,
    /// the panning envelope (tick, -1..1) and the pitch envelope (tick, semitones) with their sustain / loop points
    pub pan_env: Vec<(u16, f32)>,
    pub pan_sustain: Option<usize>,
    pub pan_loop: Option<(usize, usize)>,
    pub pitch_env: Vec<(u16, f32)>,
    pub pitch_sustain: Option<usize>,
    pub pitch_loop: Option<(usize, usize)>,
    /// the instrument's default filter (Impulse Tracker, 0..127 each) and its envelope (factor 0..1 on the cutoff)
    pub cutoff: Option<u8>,
    pub reso: Option<u8>,
    pub filter_env: Vec<(u16, f32)>,
    pub filter_sustain: Option<usize>,
    pub filter_loop: Option<(usize, usize)>,
    /// random variation per note, 0..1: volume, pan, cutoff, resonance
    pub swing: [f32; 4],
    /// seconds for the fade-out after a key off to reach silence (0 = none)
    pub fade_s: f32,
    /// the instrument's global volume in dB (IT)
    pub gain_db: f32,
    /// the instrument's default pan (IT), which overrides the sample's
    pub pan: Option<f32>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Fx {
    None,
    Speed(u8),
    Tempo(u8),
    Jump(u8),
    Break(u8),
    Vol(u8),
    Pan(f32),
    Cut(u8),
    Delay(u8),
    Off(u8),
    /// arpeggio: the two offsets above the note (0, 0 = use the last)
    Arp(u8, u8),
    /// pitch slide: signed units per tick (positive up); 0 = the last one
    Porta(i16),
    /// a slide done once at the start of the row
    FinePorta(i16),
    XFinePorta(i16),
    /// tone portamento towards the next note, units per tick (0 = the last)
    Tone(u8),
    Vib(u8, u8),
    FineVib(u8, u8),
    /// volume slide per tick (+ up) and once per row
    VolSlide(i8),
    FineVol(i8),
    ToneVol(i8),
    VibVol(i8),
    Trem(u8, u8),
    /// retrigger every `interval` ticks, with a volume change mode
    Retrig(u8, u8),
    Offset(u8),
    Filter(u8),
    /// pattern loop (0 = set the start row, n = repeat n times) and pattern delay (repeat the row n times)
    PatLoop(u8),
    PatDelay(u8),
}

pub const NOTE_NONE: i16 = -1;
pub const NOTE_OFF: i16 = 200;
pub const NOTE_CUT: i16 = 201;

#[derive(Clone, Copy, Debug)]
pub struct Cell {
    /// MIDI key, or NOTE_NONE / NOTE_OFF / NOTE_CUT
    pub note: i16,
    pub inst: u8,
    /// volume column 0..64, -1 = none
    pub vol: i16,
    pub pan: Option<f32>,
    pub fx: Fx,
    /// the volume column's effect (slides, vibrato, tone portamento)
    pub vfx: Fx,
}
impl Default for Cell {
    fn default() -> Cell {
        Cell { note: NOTE_NONE, inst: 0, vol: -1, pan: None, fx: Fx::None, vfx: Fx::None }
    }
}

pub struct Module {
    pub title: String,
    pub format: &'static str,
    pub nch: usize,
    pub speed: u8,
    pub tempo: u8,
    pub orders: Vec<usize>,
    /// pattern -> row -> channel
    pub patterns: Vec<Vec<Vec<Cell>>>,
    /// 1-based in the patterns: instrument i is `ins[i - 1]`
    pub ins: Vec<Ins>,
    pub chan_pan: Vec<f32>,
    /// effects that were not modelled
    pub ignored: usize,
    /// pitch slides are in even frequency steps (XM / IT linear mode) rather than in Amiga periods
    pub linear: bool,
    /// how loud the module mixes (global / mixing volume), 1 = a full-scale mix of the importer
    pub gain: f32,
}

fn one_sample_ins(s: Smp) -> Ins {
    Ins { name: s.name.clone(), smps: vec![s], map: vec![0; 128], shift: vec![0; 128], fadeout_s: 0.02, ..Ins::default() }
}

/// signed 8-bit / 16-bit PCM bytes -> i16
fn pcm(d: &[u8], bits16: bool, signed: bool) -> Vec<i16> {
    if bits16 {
        d.chunks_exact(2).map(|c| { let v = u16::from_le_bytes([c[0], c[1]]); if signed { v as i16 } else { (v ^ 0x8000) as i16 } }).collect()
    } else {
        d.iter().map(|b| { let v = if signed { *b as i8 as i16 } else { *b as i16 - 128 }; v << 8 }).collect()
    }
}

// ── detection ─────────────────────────────────────────────────────────────────

pub fn looks_like_module(d: &[u8]) -> bool {
    d.starts_with(b"Extended Module: ") || d.starts_with(b"IMPM") || (d.len() > 0x30 && &d[0x2c..0x30] == b"SCRM") || mod_channels(d).is_some()
}

fn mod_channels(d: &[u8]) -> Option<usize> {
    if d.len() < 1084 {
        return None;
    }
    let s = &d[1080..1084];
    match s {
        b"M.K." | b"M!K!" | b"FLT4" | b"4CHN" | b"N.T." => Some(4),
        b"6CHN" => Some(6),
        b"8CHN" | b"FLT8" | b"CD81" | b"OCTA" => Some(8),
        _ if s[2..4] == *b"CH" || s[2..4] == *b"CN" => {
            let n = (s[0] as char).to_digit(10)? * 10 + (s[1] as char).to_digit(10)?;
            if (2..=32).contains(&n) { Some(n as usize) } else { None }
        }
        _ => None,
    }
}

pub fn parse_module(d: &[u8]) -> Result<Module, String> {
    if d.starts_with(b"Extended Module: ") {
        parse_xm(d)
    } else if d.starts_with(b"IMPM") {
        parse_it(d)
    } else if d.len() > 0x30 && &d[0x2c..0x30] == b"SCRM" {
        parse_s3m(d)
    } else if mod_channels(d).is_some() {
        parse_mod(d)
    } else if d.starts_with(b"MO3") {
        Err("MO3 modules are compressed and cannot be read: save the module as .it or .xm in OpenMPT".into())
    } else {
        Err("not a tracker module".into())
    }
}

thread_local! {
    /// effects that were not modelled, by name (for tests and the report)
    pub static IGNORED: std::cell::RefCell<std::collections::BTreeMap<String, usize>> = std::cell::RefCell::new(std::collections::BTreeMap::new());
}
fn ign(ig: &mut usize, what: String) {
    *ig += 1;
    IGNORED.with(|m| *m.borrow_mut().entry(what).or_insert(0) += 1);
}

// ── effect decoding ───────────────────────────────────────────────────────────

fn slide_val(x: u8, y: u8) -> i8 {
    if x > 0 {
        x as i8
    } else {
        -(y as i8)
    }
}

/// ProTracker command (MOD, and the first sixteen XM effects)
fn dec_pt(cmd: u8, par: u8, ig: &mut usize) -> Fx {
    let (x, y) = (par >> 4, par & 15);
    match cmd {
        0 => {
            if par == 0 {
                Fx::None
            } else {
                Fx::Arp(x, y)
            }
        }
        1 => Fx::Porta(par as i16),
        2 => Fx::Porta(-(par as i16)),
        3 => Fx::Tone(par),
        4 => Fx::Vib(x, y),
        5 => Fx::ToneVol(slide_val(x, y)),
        6 => Fx::VibVol(slide_val(x, y)),
        7 => Fx::Trem(x, y),
        8 => Fx::Pan((par as f32 - 128.0) / 128.0),
        9 => Fx::Offset(par),
        0xA => Fx::VolSlide(slide_val(x, y)),
        0xB => Fx::Jump(par),
        0xC => Fx::Vol(par.min(64)),
        0xD => Fx::Break(x * 10 + y),
        0xE => match x {
            1 => Fx::FinePorta(y as i16),
            2 => Fx::FinePorta(-(y as i16)),
            6 => Fx::PatLoop(y),
            0xE => Fx::PatDelay(y),
            8 => Fx::Pan((y as f32 - 7.5) / 7.5),
            9 => {
                if y > 0 {
                    Fx::Retrig(y, 0)
                } else {
                    Fx::None
                }
            }
            0xA => Fx::FineVol(y as i8),
            0xB => Fx::FineVol(-(y as i8)),
            0xC => Fx::Cut(y),
            0xD => Fx::Delay(y),
            _ => {
                if par != 0 {
                    ign(ig, format!("E{:X}", x));
                }
                Fx::None
            }
        },
        0xF => {
            if par == 0 {
                Fx::None
            } else if par < 32 {
                Fx::Speed(par)
            } else {
                Fx::Tempo(par)
            }
        }
        _ => {
            ign(ig, format!("{:X}", cmd));
            Fx::None
        }
    }
}

/// FastTracker II effect (0..35 = 0-9 A-Z)
fn dec_xm(eff: u8, par: u8, ig: &mut usize) -> Fx {
    let (x, y) = (par >> 4, par & 15);
    match eff {
        0..=15 => dec_pt(eff, par, ig),
        0x14 => Fx::Off(par),
        0x1B => {
            if y > 0 {
                Fx::Retrig(y, x)
            } else {
                Fx::None
            }
        }
        0x21 => match x {
            1 => Fx::XFinePorta(y as i16),
            2 => Fx::XFinePorta(-(y as i16)),
            _ => Fx::None,
        },
        0x10 | 0x11 | 0x15 | 0x19 | 0x1D => {
            ign(ig, format!("xm{:X}", eff));
            Fx::None
        }
        _ => {
            ign(ig, format!("xm{:X}", eff));
            Fx::None
        }
    }
}

/// XM volume column: (set volume, pan, effect)
fn xm_vcol(v: u8) -> (i16, Option<f32>, Fx) {
    match v {
        0x10..=0x50 => ((v - 0x10) as i16, None, Fx::None),
        0x60..=0x6F => (-1, None, Fx::VolSlide(-((v & 15) as i8))),
        0x70..=0x7F => (-1, None, Fx::VolSlide((v & 15) as i8)),
        0x80..=0x8F => (-1, None, Fx::FineVol(-((v & 15) as i8))),
        0x90..=0x9F => (-1, None, Fx::FineVol((v & 15) as i8)),
        0xA0..=0xAF => (-1, None, Fx::Vib(v & 15, 0)),
        0xB0..=0xBF => (-1, None, Fx::Vib(0, v & 15)),
        0xC0..=0xCF => (-1, Some(((v & 15) as f32 - 7.5) / 7.5), Fx::None),
        0xF0..=0xFF => (-1, None, Fx::Tone((v & 15) * 16)),
        _ => (-1, None, Fx::None),
    }
}

/// ScreamTracker 3 / Impulse Tracker effect (A = 1 ...)
fn dec_it(cmd: u8, par: u8, it: bool, ig: &mut usize) -> Fx {
    let (x, y) = (par >> 4, par & 15);
    let dslide = |ig: &mut usize| -> Fx {
        let _ = ig;
        if par == 0 {
            Fx::VolSlide(0)
        } else if y == 0 {
            Fx::VolSlide(x as i8)
        } else if x == 0 {
            Fx::VolSlide(-(y as i8))
        } else if y == 0xF {
            Fx::FineVol(x as i8)
        } else if x == 0xF {
            Fx::FineVol(-(y as i8))
        } else {
            Fx::VolSlide(0)
        }
    };
    match cmd {
        0 => Fx::None,
        1 => {
            if par > 0 {
                Fx::Speed(par)
            } else {
                Fx::None
            }
        }
        2 => Fx::Jump(par),
        3 => Fx::Break(if it { par } else { x * 10 + y }),
        4 => dslide(ig),
        5 => {
            if par >= 0xF0 {
                Fx::FinePorta(-((par & 15) as i16))
            } else if par >= 0xE0 {
                Fx::XFinePorta(-((par & 15) as i16))
            } else {
                Fx::Porta(-(par as i16))
            }
        }
        6 => {
            if par >= 0xF0 {
                Fx::FinePorta((par & 15) as i16)
            } else if par >= 0xE0 {
                Fx::XFinePorta((par & 15) as i16)
            } else {
                Fx::Porta(par as i16)
            }
        }
        7 => Fx::Tone(par),
        8 => Fx::Vib(x, y),
        21 => Fx::FineVib(x, y),
        10 => Fx::Arp(x, y),
        11 => match dslide(ig) {
            Fx::VolSlide(v) | Fx::FineVol(v) => Fx::VibVol(v),
            f => f,
        },
        12 => match dslide(ig) {
            Fx::VolSlide(v) | Fx::FineVol(v) => Fx::ToneVol(v),
            f => f,
        },
        15 => Fx::Offset(par),
        17 => {
            if y > 0 {
                Fx::Retrig(y, x)
            } else {
                Fx::None
            }
        }
        18 => Fx::Trem(x, y),
        19 => match x {
            8 => Fx::Pan((y as f32 - 7.5) / 7.5),
            0xB => Fx::PatLoop(y),
            0xE => Fx::PatDelay(y),
            0xC => Fx::Cut(y),
            0xD => Fx::Delay(y),
            _ => {
                if par != 0 {
                    ign(ig, format!("S{:X}", x));
                }
                Fx::None
            }
        },
        20 => {
            if par >= 32 {
                Fx::Tempo(par)
            } else {
                Fx::None
            }
        }
        24 if it => Fx::Pan((par as f32 - 128.0) / 128.0),
        26 if it => Fx::Filter(par),
        _ => {
            ign(ig, format!("it{}{:02X}", (b'A' + cmd.max(1) - 1) as char, par));
            Fx::None
        }
    }
}

/// Impulse Tracker volume column: (set volume, pan, effect)
fn it_vcol(v: u8) -> (i16, Option<f32>, Fx) {
    match v {
        0..=64 => (v as i16, None, Fx::None),
        65..=74 => (-1, None, Fx::FineVol((v - 65) as i8)),
        75..=84 => (-1, None, Fx::FineVol(-((v - 75) as i8))),
        85..=94 => (-1, None, Fx::VolSlide((v - 85) as i8)),
        95..=104 => (-1, None, Fx::VolSlide(-((v - 95) as i8))),
        105..=114 => (-1, None, Fx::Porta(-(((v - 105) as i16) * 4))),
        115..=124 => (-1, None, Fx::Porta(((v - 115) as i16) * 4)),
        128..=192 => (-1, Some((v as f32 - 128.0 - 32.0) / 32.0), Fx::None),
        193..=202 => (-1, None, Fx::Tone([0, 1, 4, 8, 16, 32, 64, 96, 128, 255][(v - 193) as usize])),
        203..=212 => (-1, None, Fx::Vib(0, v - 203)),
        _ => (-1, None, Fx::None),
    }
}

// ── MOD ───────────────────────────────────────────────────────────────────────

const PT_PERIODS: [u16; 36] = [856, 808, 762, 720, 678, 640, 604, 570, 538, 508, 480, 453, 428, 404, 381, 360, 339, 320, 302, 285, 269, 254, 240, 226, 214, 202, 190, 180, 170, 160, 151, 143, 135, 127, 120, 113];

fn parse_mod(d: &[u8]) -> Result<Module, String> {
    let r = Rd { d };
    let nch = mod_channels(d).ok_or("not a MOD")?;
    let title = r.text(0, 20);
    let mut smps: Vec<Smp> = vec![];
    let mut lens = vec![];
    for i in 0..31 {
        let o = 20 + i * 30;
        let len = r.u16be(o + 22) as usize * 2;
        let ft = (r.u8(o + 24) & 15) as i32;
        let ft = if ft > 7 { ft - 16 } else { ft };
        let (ls, ll) = (r.u16be(o + 26) as usize * 2, r.u16be(o + 28) as usize * 2);
        smps.push(Smp { name: r.text(o, 22), rate: 8363, volume: r.u8(o + 25).min(64), cents: ft as f32 * 12.5, looped: ll > 2, loop_start: ls as u32, loop_end: (ls + ll) as u32, ..Smp::default() });
        lens.push(len);
    }
    let song_len = (r.u8(950) as usize).clamp(1, 128);
    let orders: Vec<usize> = (0..song_len).map(|i| r.u8(952 + i) as usize).collect();
    let npat = orders.iter().max().copied().unwrap_or(0) + 1;
    let mut patterns = vec![];
    let mut ignored = 0;
    let mut pos = 1084;
    let (mut speed, mut tempo) = (6u8, 125u8);
    for _ in 0..npat {
        let mut rows = vec![];
        for _ in 0..64 {
            let mut row = vec![];
            for _ in 0..nch {
                let (b0, b1, b2, b3) = (r.u8(pos), r.u8(pos + 1), r.u8(pos + 2), r.u8(pos + 3));
                pos += 4;
                let period = ((b0 & 15) as u16) << 8 | b1 as u16;
                let inst = (b0 & 0xf0) | (b2 >> 4);
                let (cmd, par) = (b2 & 15, b3);
                let note = if period == 0 {
                    NOTE_NONE
                } else {
                    // C-2 (period 428) plays the sample as recorded: that is MIDI 60
                    let idx = (0..36).min_by(|a, b| ((period as f32).ln() - (PT_PERIODS[*a] as f32).ln()).abs().total_cmp(&((period as f32).ln() - (PT_PERIODS[*b] as f32).ln()).abs())).unwrap_or(12);
                    48 + idx as i16
                };
                let mut c = Cell { note, inst, ..Cell::default() };
                c.fx = dec_pt(cmd, par, &mut ignored);
                if let Fx::Vol(v) = c.fx {
                    c.vol = v as i16;
                    c.fx = Fx::None;
                }
                row.push(c);
            }
            rows.push(row);
        }
        patterns.push(rows);
    }
    // sample data follows the patterns
    for (i, s) in smps.iter_mut().enumerate() {
        let n = lens[i];
        let raw = r.slice(pos, n);
        pos += n;
        s.data = pcm(raw, false, true);
        if s.data.len() <= 1 {
            s.data.clear();
        }
        s.loop_end = s.loop_end.min(s.data.len() as u32);
        if s.loop_start >= s.loop_end {
            s.looped = false;
        }
    }
    let _ = (&mut speed, &mut tempo);
    // the Amiga's panning: channels 0 and 3 left, 1 and 2 right
    let chan_pan = (0..nch).map(|c| if (c & 3) == 0 || (c & 3) == 3 { -0.5 } else { 0.5 }).collect();
    Ok(Module { title, format: "ProTracker MOD", nch, speed: 6, tempo: 125, orders, patterns, ins: smps.into_iter().map(one_sample_ins).collect(), chan_pan, ignored, linear: false, gain: 0.45 })
}

// ── S3M ───────────────────────────────────────────────────────────────────────

fn parse_s3m(d: &[u8]) -> Result<Module, String> {
    let r = Rd { d };
    let title = r.text(0, 28);
    let (ordnum, insnum, patnum) = (r.u16(0x20) as usize, r.u16(0x22) as usize, r.u16(0x24) as usize);
    let ffi = r.u16(0x2a);
    let (speed, tempo) = (r.u8(0x31).max(1), r.u8(0x32).max(32));
    let default_pan = r.u8(0x35) == 252;
    // channel settings: 0..7 left, 8..15 right, 255 unused
    let mut chan_map = vec![usize::MAX; 32];
    let mut nch = 0;
    let mut chan_pan = vec![];
    for c in 0..32 {
        let s = r.u8(0x40 + c);
        if s < 16 {
            chan_map[c] = nch;
            nch += 1;
            chan_pan.push(if s < 8 { -0.5 } else { 0.5 });
        }
    }
    let nch = nch.max(1);
    let ord_at = 0x60;
    let mut orders: Vec<usize> = vec![];
    for i in 0..ordnum {
        let o = r.u8(ord_at + i);
        if o == 255 {
            break;
        }
        if o < 254 {
            orders.push(o as usize);
        }
    }
    let ins_ptr = ord_at + ordnum;
    let pat_ptr = ins_ptr + insnum * 2;
    let mut ins = vec![];
    for i in 0..insnum {
        let o = r.u16(ins_ptr + i * 2) as usize * 16;
        if o == 0 || o + 0x50 > d.len() {
            ins.push(Ins::default());
            continue;
        }
        let kind = r.u8(o);
        let name = r.text(o + 0x30, 28);
        if kind != 1 {
            ins.push(Ins { name, ..Ins::default() });
            continue;
        }
        let memseg = (r.u8(o + 0x0d) as usize) << 16 | r.u16(o + 0x0e) as usize;
        let (len, ls, le) = (r.u32(o + 0x10) as usize, r.u32(o + 0x14), r.u32(o + 0x18));
        let (vol, flags, c2) = (r.u8(o + 0x1c), r.u8(o + 0x1f), r.u32(o + 0x20));
        let (stereo, bits16) = (flags & 2 != 0, flags & 4 != 0);
        let bytes = len * if bits16 { 2 } else { 1 } * if stereo { 2 } else { 1 };
        let raw = r.slice(memseg * 16, bytes);
        let mut data = pcm(&raw[..if stereo { raw.len() / 2 } else { raw.len() }], bits16, ffi == 1);
        data.truncate(len);
        if stereo {
            // left block then right block: keep the left
        }
        let looped = flags & 1 != 0 && le > ls && (le as usize) <= data.len() + 1;
        ins.push(one_sample_ins(Smp { name, data, rate: c2.clamp(1000, 192000), volume: vol.min(64), looped, loop_start: ls, loop_end: le.min(len as u32), pan: None, ..Smp::default() }));
    }
    let mut patterns = vec![];
    let mut ignored = 0;
    for p in 0..patnum {
        let o = r.u16(pat_ptr + p * 2) as usize * 16;
        let mut rows = vec![vec![Cell::default(); nch]; 64];
        if o != 0 && o + 2 <= d.len() {
            let end = (o + 2 + r.u16(o) as usize).min(d.len());
            let mut q = o + 2;
            let mut row = 0;
            while row < 64 && q < end {
                let what = r.u8(q);
                q += 1;
                if what == 0 {
                    row += 1;
                    continue;
                }
                let ch = (what & 31) as usize;
                let mut c = Cell::default();
                if what & 32 != 0 {
                    let (n, i) = (r.u8(q), r.u8(q + 1));
                    q += 2;
                    c.note = match n {
                        255 => NOTE_NONE,
                        254 => NOTE_CUT,
                        _ => ((n >> 4) as i16) * 12 + (n & 15) as i16,
                    };
                    c.inst = i;
                }
                if what & 64 != 0 {
                    let v = r.u8(q);
                    q += 1;
                    if v <= 64 {
                        c.vol = v as i16;
                    }
                }
                if what & 128 != 0 {
                    let (cmd, par) = (r.u8(q), r.u8(q + 1));
                    q += 2;
                    c.fx = dec_it(cmd, par, false, &mut ignored);
                }
                if ch < 32 && chan_map[ch] != usize::MAX && row < 64 {
                    rows[row][chan_map[ch]] = c;
                }
            }
        }
        patterns.push(rows);
    }
    let _ = default_pan;
    Ok(Module { title, format: "ScreamTracker S3M", nch, speed, tempo, orders, patterns, ins, chan_pan, ignored, linear: false, gain: (r.u8(0x30).min(64) as f32 / 64.0) * ((r.u8(0x33) & 127) as f32 / 128.0).max(0.1) * 1.2 })
}

// ── XM ────────────────────────────────────────────────────────────────────────

fn parse_xm(d: &[u8]) -> Result<Module, String> {
    let r = Rd { d };
    let title = r.text(17, 20);
    let hsize = r.u32(60) as usize;
    let (songlen, nch, npat, nins) = (r.u16(64) as usize, r.u16(68) as usize, r.u16(70) as usize, r.u16(72) as usize);
    let (speed, tempo) = (r.u16(76).clamp(1, 31) as u8, r.u16(78).clamp(32, 255) as u8);
    if nch == 0 || nch > 128 {
        return Err("bad XM channel count".into());
    }
    let orders: Vec<usize> = (0..songlen.min(256)).map(|i| r.u8(80 + i) as usize).filter(|p| *p < npat.max(1)).collect();
    let mut pos = 60 + hsize;
    let mut patterns = vec![];
    let mut ignored = 0;
    for _ in 0..npat {
        let hl = r.u32(pos) as usize;
        let rows_n = r.u16(pos + 5) as usize;
        let psize = r.u16(pos + 7) as usize;
        let mut q = pos + hl;
        let end = q + psize;
        let mut rows = vec![vec![Cell::default(); nch]; rows_n.min(256)];
        if psize > 0 {
            for row in rows.iter_mut() {
                for cell in row.iter_mut() {
                    if q >= end {
                        break;
                    }
                    let first = r.u8(q);
                    let (mut note, mut inst, mut vol, mut eff, mut par) = (0u8, 0u8, 0u8, 0u8, 0u8);
                    if first & 0x80 != 0 {
                        q += 1;
                        for (bit, slot) in [(1u8, &mut note), (2, &mut inst), (4, &mut vol), (8, &mut eff), (16, &mut par)] {
                            if first & bit != 0 {
                                *slot = r.u8(q);
                                q += 1;
                            }
                        }
                    } else {
                        note = first;
                        inst = r.u8(q + 1);
                        vol = r.u8(q + 2);
                        eff = r.u8(q + 3);
                        par = r.u8(q + 4);
                        q += 5;
                    }
                    let mut c = Cell { inst, ..Cell::default() };
                    c.note = match note {
                        0 => NOTE_NONE,
                        97 => NOTE_OFF,
                        n if n <= 96 => n as i16 + 11,
                        _ => NOTE_NONE,
                    };
                    let (cv, cp, cfx) = xm_vcol(vol);
                    c.vol = cv;
                    c.pan = cp;
                    c.vfx = cfx;
                    c.fx = dec_xm(eff, par, &mut ignored);
                    if let Fx::Vol(v) = c.fx {
                        c.vol = v as i16;
                        c.fx = Fx::None;
                    }
                    *cell = c;
                }
            }
        }
        pos = end;
        patterns.push(rows);
    }
    // instruments
    let mut ins = vec![];
    for _ in 0..nins {
        let isz = r.u32(pos) as usize;
        if isz < 29 || pos + isz > d.len() + 1 {
            ins.push(Ins::default());
            pos += isz.max(29);
            continue;
        }
        let name = r.text(pos + 4, 22);
        let ns = r.u16(pos + 27) as usize;
        let mut inst = Ins { name, map: vec![255; 128], shift: vec![0; 128], fadeout_s: 0.02, ..Ins::default() };
        let mut hdrs = vec![];
        if ns > 0 {
            let shs = r.u32(pos + 29) as usize;
            for k in 0..96 {
                inst.map[k + 12] = r.u8(pos + 33 + k);
            }
            let fo = r.u16(pos + 239);
            inst.fadeout_s = if fo == 0 { 0.02 } else { (32768.0 / fo as f32 * 0.02).clamp(0.02, 6.0) };
            inst.fade_s = if fo == 0 { 0.0 } else { (32768.0 / fo as f32 * 2.5 / tempo as f32).clamp(0.0, 8.0) };
            // the volume envelope: up to 12 (tick, value) points
            let (nvol, vsus, vls, vle, vtype) = (r.u8(pos + 225) as usize, r.u8(pos + 227) as usize, r.u8(pos + 228) as usize, r.u8(pos + 229) as usize, r.u8(pos + 233));
            let (npan, psus, pls, ple, ptype) = (r.u8(pos + 226) as usize, r.u8(pos + 230) as usize, r.u8(pos + 231) as usize, r.u8(pos + 232) as usize, r.u8(pos + 234));
            if ptype & 1 != 0 && npan >= 2 {
                for k in 0..npan.min(12) {
                    inst.pan_env.push((r.u16(pos + 177 + k * 4), (r.u16(pos + 179 + k * 4).min(64) as f32 - 32.0) / 32.0));
                }
                if ptype & 2 != 0 && psus < inst.pan_env.len() {
                    inst.pan_sustain = Some(psus);
                }
                if ptype & 4 != 0 && pls <= ple && ple < inst.pan_env.len() {
                    inst.pan_loop = Some((pls, ple));
                }
            }
            if vtype & 1 != 0 && nvol >= 2 {
                for k in 0..nvol.min(12) {
                    inst.env.push((r.u16(pos + 129 + k * 4), r.u16(pos + 131 + k * 4).min(64) as u8));
                }
                if vtype & 2 != 0 && vsus < inst.env.len() {
                    inst.env_sustain = Some(vsus);
                }
                if vtype & 4 != 0 && vls <= vle && vle < inst.env.len() {
                    inst.env_loop = Some((vls, vle));
                }
            }
            let mut sp = pos + isz;
            for _ in 0..ns {
                hdrs.push(sp);
                sp += shs.max(40);
            }
            pos = sp;
            for &h in &hdrs {
                let len = r.u32(h) as usize;
                let (ls, ll) = (r.u32(h + 4) as usize, r.u32(h + 8) as usize);
                let ty = r.u8(h + 14);
                let bits16 = ty & 0x10 != 0;
                let bytes = r.slice(pos, len);
                pos += len;
                // delta-coded
                let mut data: Vec<i16> = vec![];
                if bits16 {
                    let mut acc = 0i16;
                    for c in bytes.chunks_exact(2) {
                        acc = acc.wrapping_add(i16::from_le_bytes([c[0], c[1]]));
                        data.push(acc);
                    }
                } else {
                    let mut acc = 0i8;
                    for b in bytes {
                        acc = acc.wrapping_add(*b as i8);
                        data.push((acc as i16) << 8);
                    }
                }
                let div = if bits16 { 2 } else { 1 };
                let (ls, ll) = ((ls / div) as u32, (ll / div) as u32);
                let looped = ty & 3 != 0 && ll > 1;
                inst.smps.push(Smp {
                    name: r.text(h + 18, 22),
                    data,
                    rate: 8363,
                    loop_start: ls,
                    loop_end: (ls + ll).min(len as u32 / div as u32),
                    looped,
                    volume: r.u8(h + 12).min(64),
                    pan: Some((r.u8(h + 15) as f32 - 128.0) / 128.0),
                    cents: r.i8(h + 13) as f32 / 128.0 * 100.0,
                    rel_note: r.i8(h + 16) as i32,
                    pingpong: ty & 3 == 2,
                    vol_db: 0.0,
                });
            }
        } else {
            pos += isz;
        }
        ins.push(inst);
    }
    let chan_pan = vec![0.0; nch];
    let linear = r.u16(74) & 1 != 0;
    Ok(Module { title, format: "FastTracker XM", nch, speed, tempo, orders, patterns, ins, chan_pan, ignored, linear, gain: 0.38 })
}

// ── IT / MPTM ─────────────────────────────────────────────────────────────────

/// IT214 / IT215 sample decompression
fn it_decompress(src: &[u8], len: usize, bits16: bool, it215: bool) -> Vec<i16> {
    struct Bits<'a> {
        d: &'a [u8],
        pos: usize,
        buf: u32,
        n: u32,
    }
    impl Bits<'_> {
        fn read(&mut self, mut n: u32) -> u32 {
            let mut v = 0u32;
            let mut shift = 0;
            while n > 0 {
                if self.n == 0 {
                    self.buf = self.d.get(self.pos).copied().unwrap_or(0) as u32;
                    self.pos += 1;
                    self.n = 8;
                }
                let take = n.min(self.n);
                v |= (self.buf & ((1 << take) - 1)) << shift;
                self.buf >>= take;
                self.n -= take;
                shift += take;
                n -= take;
            }
            v
        }
    }
    let mut out: Vec<i16> = Vec::with_capacity(len);
    let mut p = 0usize;
    let block_max = if bits16 { 0x4000 } else { 0x8000 };
    while out.len() < len && p + 2 <= src.len() {
        let csize = src[p] as usize | (src[p + 1] as usize) << 8;
        p += 2;
        let data = &src[p.min(src.len())..(p + csize).min(src.len())];
        p += csize;
        let want = block_max.min(len - out.len());
        let mut b = Bits { d: data, pos: 0, buf: 0, n: 0 };
        let mut width: u32 = if bits16 { 17 } else { 9 };
        let (mut d1, mut d2) = (0i32, 0i32);
        let mut got = 0;
        while got < want {
            if b.pos > data.len() + 4 {
                break;
            }
            let mut v = b.read(width) as i64;
            if width < 7 {
                if v == 1i64 << (width - 1) {
                    let nw = b.read(if bits16 { 4 } else { 3 }) + 1;
                    width = if nw < width { nw } else { nw + 1 };
                    continue;
                }
            } else if width < if bits16 { 17 } else { 9 } {
                let (i, span) = if bits16 { (((0xFFFFu32 >> (17 - width)) + 8) as i64, 16) } else { (((0xFFu32 >> (9 - width)) + 4) as i64, 8) };
                let j = i - span;
                if v > j && v <= i {
                    let nw = (v - j) as u32;
                    width = if nw < width { nw } else { nw + 1 };
                    continue;
                }
            } else if bits16 && width >= 18 || !bits16 && width >= 10 {
                got += 1;
                continue;
            } else if v >= if bits16 { 0x10000 } else { 0x100 } {
                width = (v + 1) as u32 & 0xFF;
                continue;
            }
            // a value
            let w = width;
            let full = if bits16 { 16 } else { 8 };
            if w < full {
                let shift = full - w;
                v = if bits16 { ((v as i16) << shift >> shift) as i64 } else { (((v as i8) << shift) >> shift) as i64 };
            }
            d1 = d1.wrapping_add(v as i32);
            let cur = if bits16 { d1 as i16 as i32 } else { d1 as i8 as i32 };
            d1 = cur;
            d2 = d2.wrapping_add(cur);
            let o = if it215 { d2 } else { cur };
            out.push(if bits16 { o as i16 } else { ((o as i8) as i16) << 8 });
            got += 1;
        }
        // a damaged block: keep the length right
        while got < want {
            out.push(0);
            got += 1;
        }
    }
    out.resize(len, 0);
    out
}

fn parse_it(d: &[u8]) -> Result<Module, String> {
    let r = Rd { d };
    let title = r.text(4, 26);
    let (ordnum, insnum, smpnum, patnum) = (r.u16(0x20) as usize, r.u16(0x22) as usize, r.u16(0x24) as usize, r.u16(0x26) as usize);
    let (cmwt, flags) = (r.u16(0x2a), r.u16(0x2c));
    let use_ins = flags & 4 != 0;
    let (speed, tempo) = (r.u8(0x32).max(1), r.u8(0x33).max(32));
    let chan_pan_raw: Vec<u8> = (0..64).map(|c| r.u8(0x40 + c)).collect();
    let mut orders = vec![];
    for i in 0..ordnum.min(1024) {
        let o = r.u8(0xc0 + i);
        if o == 255 {
            break;
        }
        if o < 254 {
            orders.push(o as usize);
        }
    }
    let ins_ptr = 0xc0 + ordnum;
    let smp_ptr = ins_ptr + insnum * 4;
    let pat_ptr = smp_ptr + smpnum * 4;
    // samples first: instruments point at them
    let mut smps: Vec<Option<Smp>> = vec![];
    for i in 0..smpnum {
        let o = r.u32(smp_ptr + i * 4) as usize;
        if o == 0 || o + 0x50 > d.len() || &d[o..o + 4] != b"IMPS" {
            smps.push(None);
            continue;
        }
        let (gvl, fl, vol) = (r.u8(o + 0x11), r.u8(o + 0x12), r.u8(o + 0x13));
        let name = r.text(o + 0x14, 26);
        let (cvt, dfp) = (r.u8(o + 0x2e), r.u8(o + 0x2f));
        let (len, ls, le, c5, sls, sle, ptr) = (r.u32(o + 0x30) as usize, r.u32(o + 0x34), r.u32(o + 0x38), r.u32(o + 0x3c), r.u32(o + 0x40), r.u32(o + 0x44), r.u32(o + 0x48) as usize);
        let (present, bits16, stereo, comp) = (fl & 1 != 0, fl & 2 != 0, fl & 4 != 0, fl & 8 != 0);
        let mut data = vec![];
        if present && len > 0 && len < 1 << 28 && ptr < d.len() {
            let signed = cvt & 1 != 0;
            let chans = if stereo { 2 } else { 1 };
            let mut chan_data: Vec<Vec<i16>> = vec![];
            let mut at = ptr;
            for _ in 0..chans {
                if comp {
                    // each channel is its own run of compressed blocks; walk it to find where it ends
                    let block_max = if bits16 { 0x4000 } else { 0x8000 };
                    let mut left = len;
                    let mut q = at;
                    while left > 0 && q + 2 <= d.len() {
                        q += 2 + r.u16(q) as usize;
                        left = left.saturating_sub(block_max);
                    }
                    let v = it_decompress(r.slice(at, q.saturating_sub(at)), len, bits16, cvt & 4 != 0);
                    chan_data.push(v);
                    at = q;
                } else {
                    let bytes = len * if bits16 { 2 } else { 1 };
                    chan_data.push(pcm(r.slice(at, bytes), bits16, signed));
                    at += bytes;
                }
            }
            data = chan_data.remove(0);
            if let Some(right) = chan_data.first() {
                for (l, rr) in data.iter_mut().zip(right) {
                    *l = ((*l as i32 + *rr as i32) / 2) as i16;
                }
            }
        }
        let (looped, lstart, lend) = if fl & 16 != 0 && le > ls { (true, ls, le) } else if fl & 32 != 0 && sle > sls { (true, sls, sle) } else { (false, 0, 0) };
        smps.push(Some(Smp {
            name,
            rate: c5.clamp(1000, 384000),
            volume: vol.min(64),
            pan: if dfp & 128 != 0 { Some(((dfp & 127).min(64) as f32 - 32.0) / 32.0) } else { None },
            loop_start: lstart,
            loop_end: lend.min(data.len() as u32),
            looped: looped && (lend as usize) <= data.len() + 1,
            pingpong: fl & 64 != 0,
            vol_db: if gvl >= 64 { 0.0 } else { 20.0 * (gvl.max(1) as f32 / 64.0).log10() },
            data,
            ..Smp::default()
        }));
    }
    let mut ins = vec![];
    if use_ins {
        for i in 0..insnum {
            let o = r.u32(ins_ptr + i * 4) as usize;
            if o == 0 || o + 0x80 > d.len() || &d[o..o + 4] != b"IMPI" {
                ins.push(Ins::default());
                continue;
            }
            let _ = cmwt;
            let fadeout = r.u16(o + 0x14);
            let gbv = r.u8(o + 0x18).min(128);
            let dfp = r.u8(o + 0x19);
            let name = r.text(o + 0x20, 26);
            let mut inst = Ins { name, map: vec![255; 128], shift: vec![0; 128], fadeout_s: if fadeout == 0 { 0.02 } else { (4096.0 / fadeout as f32 * 0.02).clamp(0.02, 5.0) }, ..Ins::default() };
            inst.pan = if dfp & 0x80 == 0 { Some((dfp.min(64) as f32 - 32.0) / 32.0 * 0.8) } else { None };
            inst.gain_db = if gbv >= 128 { 0.0 } else { 20.0 * (gbv.max(1) as f32 / 128.0).log10() };
            let mut used: Vec<usize> = vec![];
            // volume envelope (flags, count, loop begin / end, sustain begin / end, then (value, tick) nodes) and the fade-out
            if r.u8(o + 0x130) & 1 != 0 {
                let cnt = (r.u8(o + 0x131) as usize).min(25);
                for k in 0..cnt {
                    inst.env.push((r.u16(o + 0x136 + k * 3 + 1), r.u8(o + 0x136 + k * 3).min(64)));
                }
                if r.u8(o + 0x130) & 4 != 0 && (r.u8(o + 0x134) as usize) < inst.env.len() {
                    inst.env_sustain = Some(r.u8(o + 0x134) as usize);
                }
                if r.u8(o + 0x130) & 2 != 0 && (r.u8(o + 0x132) as usize) <= (r.u8(o + 0x133) as usize) && (r.u8(o + 0x133) as usize) < inst.env.len() {
                    inst.env_loop = Some((r.u8(o + 0x132) as usize, r.u8(o + 0x133) as usize));
                }
            }
            // the panning (0x182) and pitch / filter (0x1D4) envelopes: signed -32..32 nodes
            let (ifc, ifr) = (r.u8(o + 0x3A), r.u8(o + 0x3B));
            inst.swing[0] = (r.u8(o + 0x1A).min(100) as f32) / 100.0;
            inst.swing[1] = (r.u8(o + 0x1B).min(64) as f32) / 64.0;
            inst.cutoff = if ifc & 0x80 != 0 { Some(ifc & 0x7F) } else { None };
            inst.reso = if ifr & 0x80 != 0 { Some(ifr & 0x7F) } else { None };
            for (base_off, is_pan) in [(0x182usize, true), (0x1D4usize, false)] {
                let flags = r.u8(o + base_off);
                if flags & 1 == 0 {
                    continue;
                }
                let filter_env = !is_pan && flags & 0x80 != 0;
                let cnt = (r.u8(o + base_off + 1) as usize).min(25);
                let mut pts: Vec<(u16, f32)> = vec![];
                for k in 0..cnt {
                    let raw = r.i8(o + base_off + 6 + k * 3) as f32;
                    // pan: -1..1; filter: the factor (node + 32) / 64 of the cutoff; pitch: semitones
                    pts.push((r.u16(o + base_off + 6 + k * 3 + 1), if is_pan { raw / 32.0 } else if filter_env { (raw + 32.0) / 64.0 } else { raw }));
                }
                if pts.len() < 2 {
                    continue;
                }
                let sus = if flags & 4 != 0 && (r.u8(o + base_off + 4) as usize) < pts.len() { Some(r.u8(o + base_off + 4) as usize) } else { None };
                let lp = if flags & 2 != 0 && r.u8(o + base_off + 2) <= r.u8(o + base_off + 3) && (r.u8(o + base_off + 3) as usize) < pts.len() { Some((r.u8(o + base_off + 2) as usize, r.u8(o + base_off + 3) as usize)) } else { None };
                if is_pan {
                    inst.pan_env = pts;
                    inst.pan_sustain = sus;
                    inst.pan_loop = lp;
                } else if filter_env {
                    inst.filter_env = pts;
                    inst.filter_sustain = sus;
                    inst.filter_loop = lp;
                } else {
                    inst.pitch_env = pts;
                    inst.pitch_sustain = sus;
                    inst.pitch_loop = lp;
                }
            }
            inst.fade_s = if fadeout == 0 { 0.0 } else { (2048.0 / fadeout as f32 * 2.5 / tempo as f32).clamp(0.0, 8.0) };
            for n in 0..120usize {
                let (play, s) = (r.u8(o + 0x40 + n * 2) as i32, r.u8(o + 0x41 + n * 2) as usize);
                if s == 0 || s > smps.len() || smps[s - 1].is_none() {
                    continue;
                }
                let idx = match used.iter().position(|u| *u == s - 1) {
                    Some(k) => k,
                    None => {
                        used.push(s - 1);
                        inst.smps.push(smps[s - 1].clone().unwrap());
                        used.len() - 1
                    }
                };
                inst.map[n] = idx as u8;
                inst.shift[n] = (play - n as i32).clamp(-127, 127) as i8;
            }
            ins.push(inst);
        }
    } else {
        for s in &smps {
            ins.push(match s {
                Some(s) => one_sample_ins(s.clone()),
                None => Ins::default(),
            });
        }
    }
    // patterns
    let nch = 64;
    let mut patterns = vec![];
    let mut ignored = 0;
    let mut max_ch = 0usize;
    for p in 0..patnum {
        let o = r.u32(pat_ptr + p * 4) as usize;
        if o == 0 || o + 8 > d.len() {
            patterns.push(vec![vec![Cell::default(); nch]; 64]);
            continue;
        }
        let (plen, rows_n) = (r.u16(o) as usize, r.u16(o + 2) as usize);
        let rows_n = rows_n.clamp(1, 1024);
        let mut rows = vec![vec![Cell::default(); nch]; rows_n];
        let end = (o + 8 + plen).min(d.len());
        let mut q = o + 8;
        let mut row = 0usize;
        let mut last_mask = [0u8; 64];
        let mut last: Vec<(u8, u8, u8, u8, u8)> = vec![(0, 0, 0, 0, 0); 64];
        while row < rows_n && q < end {
            let cv = r.u8(q);
            q += 1;
            if cv == 0 {
                row += 1;
                continue;
            }
            let ch = ((cv - 1) & 63) as usize;
            let mask = if cv & 128 != 0 {
                let m = r.u8(q);
                q += 1;
                last_mask[ch] = m;
                m
            } else {
                last_mask[ch]
            };
            let mut cell = Cell::default();
            let l = &mut last[ch];
            if mask & 1 != 0 {
                l.0 = r.u8(q);
                q += 1;
            }
            if mask & 2 != 0 {
                l.1 = r.u8(q);
                q += 1;
            }
            if mask & 4 != 0 {
                l.2 = r.u8(q);
                q += 1;
            }
            if mask & 8 != 0 {
                l.3 = r.u8(q);
                l.4 = r.u8(q + 1);
                q += 2;
            }
            let has_note = mask & (1 | 16) != 0;
            let has_ins = mask & (2 | 32) != 0;
            let has_vol = mask & (4 | 64) != 0;
            let has_cmd = mask & (8 | 128) != 0;
            if has_note {
                cell.note = match l.0 {
                    254 => NOTE_CUT,
                    255 => NOTE_OFF,
                    n if n < 120 => n as i16,
                    _ => NOTE_NONE,
                };
            }
            if has_ins {
                cell.inst = l.1;
            }
            if has_vol {
                let (cv, cp, cfx) = it_vcol(l.2);
                cell.vol = cv;
                cell.pan = cp;
                cell.vfx = cfx;
            }
            if has_cmd {
                cell.fx = dec_it(l.3, l.4, true, &mut ignored);
            }
            if row < rows_n {
                rows[row][ch] = cell;
                max_ch = max_ch.max(ch + 1);
            }
        }
        patterns.push(rows);
    }
    let nch = max_ch.max(1);
    for p in patterns.iter_mut() {
        for row in p.iter_mut() {
            row.truncate(nch);
        }
    }
    let chan_pan: Vec<f32> = (0..nch).map(|c| match chan_pan_raw[c] { v if v <= 64 => (v as f32 - 32.0) / 32.0 * 0.8, _ => 0.0 }).collect();
    Ok(Module { title, format: if d.len() > 0x2d && r.u16(0x28) >= 0x889 { "OpenMPT module (MPTM / IT)" } else { "Impulse Tracker IT" }, nch, speed, tempo, orders, patterns, ins, chan_pan, ignored, linear: flags & 8 != 0, gain: (r.u8(0x30).min(128) as f32 / 128.0) * (r.u8(0x31).min(128) as f32 / 128.0) * 1.0 })
}

// ── playing the patterns into notes, pitch bends and volume lanes ─────────────────

/// One track of the result: an instrument's notes, or (when a channel uses slides, vibrato or volume effects) one channel of it with its own
/// pitch-bend and volume lanes.
pub struct PTrack {
    /// 0-based instrument index
    pub ins: usize,
    /// the channel when the track carries its own lanes
    pub chan: Option<usize>,
    pub notes: Vec<Note>,
    /// (beat, 0..1 with 0.5 = centre)
    pub bend: Vec<(f64, f32)>,
    pub bend_range: f32,
    /// (beat, 0..1.5 track volume)
    pub vol: Vec<(f64, f32)>,
    /// filter lanes from Zxx: (beat, 0..1 cutoff) and (beat, 0..1 resonance)
    pub cut: Vec<(f64, f32)>,
    pub res: Vec<(f64, f32)>,
}

pub struct Played {
    pub tracks: Vec<PTrack>,
    pub tempos: Vec<(f64, f64)>,
    pub beats: f64,
    pub dropped: usize,
    /// effects turned into notes / lanes, for the report
    pub arps: usize,
    pub slides: usize,
    pub retrigs: usize,
}

struct Open {
    start: f64,
    key: u8,
    vel: f32,
    pan: f32,
    ins: usize,
}

#[derive(Default)]
struct Ch {
    ins: usize,
    key: i32,
    open: Option<Open>,
    semis: f32,
    tone: Option<f32>,
    vol: f32,
    pan: f32,
    pending: Option<(u32, i32)>,
    cut_at: Option<u32>,
    arp: Option<(u8, u8)>,
    arp_off: i32,
    retrig: Option<(u8, u8)>,
    m_porta: i16,
    m_tone: u8,
    m_vib: (u8, u8),
    m_vol: i8,
    m_arp: (u8, u8),
    m_trem: (u8, u8),
    vib_pos: f32,
    trem_pos: f32,
    last_bend: f32,
    last_vol: f32,
    /// the channel's filter (Impulse Tracker units 0..127)
    cutv: f32,
    resv: f32,
}

fn vol_change(mode: u8, v: f32) -> f32 {
    match mode {
        1 => v - 1.0,
        2 => v - 2.0,
        3 => v - 4.0,
        4 => v - 8.0,
        5 => v - 16.0,
        6 => v * 2.0 / 3.0,
        7 => v * 0.5,
        9 => v + 1.0,
        10 => v + 2.0,
        11 => v + 4.0,
        12 => v + 8.0,
        13 => v + 16.0,
        14 => v * 1.5,
        15 => v * 2.0,
        _ => v,
    }
    .clamp(0.0, 64.0)
}

fn is_pitch_fx(f: Fx) -> bool {
    matches!(f, Fx::Porta(_) | Fx::FinePorta(_) | Fx::XFinePorta(_) | Fx::Tone(_) | Fx::ToneVol(_) | Fx::Vib(..) | Fx::FineVib(..) | Fx::VibVol(_))
}
fn is_filter_fx(f: Fx) -> bool {
    matches!(f, Fx::Filter(_))
}
fn is_vol_fx(f: Fx) -> bool {
    matches!(f, Fx::VolSlide(_) | Fx::FineVol(_) | Fx::ToneVol(_) | Fx::VibVol(_) | Fx::Trem(..))
}

pub fn play(m: &Module) -> Played {
    let nch = m.nch;
    // channels that use pitch / volume effects get their own tracks with lanes
    let mut variant = vec![false; nch];
    for &o in &m.orders {
        if let Some(pat) = m.patterns.get(o) {
            for row in pat {
                for (ch, c) in row.iter().enumerate().take(nch) {
                    if is_pitch_fx(c.fx) || is_pitch_fx(c.vfx) || is_vol_fx(c.fx) || is_vol_fx(c.vfx) || is_filter_fx(c.fx) {
                        variant[ch] = true;
                    }
                }
            }
        }
    }
    let mut tracks: Vec<PTrack> = vec![];
    let mut tix: std::collections::HashMap<(usize, usize), usize> = std::collections::HashMap::new();
    // lane points are kept in semitones / volume units until the range is known
    let mut raw_bend: Vec<Vec<(f64, f32)>> = vec![];
    let mut raw_vol: Vec<Vec<(f64, f32)>> = vec![];
    let (mut speed, mut tempo) = (m.speed.max(1) as f64, m.tempo.max(32) as f64);
    let bpm = |sp: f64, tp: f64| (6.0 * tp / sp).clamp(20.0, 400.0);
    let mut tempos: Vec<(f64, f64)> = vec![(0.0, bpm(speed, tempo))];
    let mut chs: Vec<Ch> = (0..nch).map(|_| Ch { vol: 64.0, last_vol: -1.0, cutv: 127.0, ..Ch::default() }).collect();
    let (mut dropped, mut arps, mut slides, mut retrigs) = (0usize, 0usize, 0usize, 0usize);
    let mut beat = 0.0f64;
    let mut visited: HashSet<(usize, usize, usize)> = HashSet::new();
    let (mut oi, mut row, mut guard) = (0usize, 0usize, 0usize);
    // pattern loops: the start row and the repeats left, per channel; `pass` makes a repeated row count as a new visit
    let mut loop_start = vec![0usize; nch];
    let mut loop_left = vec![0u32; nch];
    let mut pass = 0usize;

    // the track index of an (instrument, channel) pair
    macro_rules! track_of {
        ($ins:expr, $ch:expr) => {{
            let key = ($ins, if variant[$ch] { $ch + 1 } else { 0 });
            *tix.entry(key).or_insert_with(|| {
                tracks.push(PTrack { ins: $ins, chan: if key.1 > 0 { Some($ch) } else { None }, notes: vec![], bend: vec![], bend_range: 2.0, vol: vec![], cut: vec![], res: vec![] });
                raw_bend.push(vec![]);
                raw_vol.push(vec![]);
                tracks.len() - 1
            })
        }};
    }

    while oi < m.orders.len() && guard < 400_000 {
        guard += 1;
        let Some(pat) = m.patterns.get(m.orders[oi]) else {
            oi += 1;
            row = 0;
            continue;
        };
        if row >= pat.len() {
            oi += 1;
            row = 0;
            continue;
        }
        if !visited.insert((oi, row, pass)) {
            break;
        }
        let cells = &pat[row];
        let (mut jump, mut brk): (Option<usize>, Option<usize>) = (None, None);
        for c in cells.iter() {
            match c.fx {
                Fx::Speed(sp) if sp > 0 => speed = sp as f64,
                Fx::Tempo(t) if t >= 32 => tempo = t as f64,
                Fx::Jump(o) => jump = Some(o as usize),
                Fx::Break(r) => brk = Some(r as usize),
                _ => {}
            }
        }
        // pattern loop / delay of this row
        let mut loop_to: Option<usize> = None;
        let mut delay_rows = 0u32;
        for (ch, c) in cells.iter().enumerate().take(nch) {
            match c.fx {
                Fx::PatLoop(0) => loop_start[ch] = row,
                Fx::PatLoop(n) => {
                    if loop_left[ch] == 0 {
                        loop_left[ch] = n as u32;
                        loop_to = Some(loop_start[ch]);
                    } else {
                        loop_left[ch] -= 1;
                        if loop_left[ch] > 0 {
                            loop_to = Some(loop_start[ch]);
                        }
                    }
                }
                Fx::PatDelay(n) => delay_rows = delay_rows.max(n as u32),
                _ => {}
            }
        }
        let b_now = bpm(speed, tempo);
        if tempos.last().map(|t| (t.1 - b_now).abs() > 0.05).unwrap_or(true) {
            tempos.push((beat, b_now));
        }
        let ticks = speed.round().max(1.0) as u32;
        let tick_len = 0.25 / ticks as f64;

        // ── the row: notes, volumes and the effects that start now
        for ch in 0..nch.min(cells.len()) {
            let c = cells[ch];
            let st = &mut chs[ch];
            if c.inst > 0 {
                st.ins = c.inst as usize;
            }
            if let Some(p) = c.pan {
                st.pan = p;
            } else if let Fx::Pan(p) = c.fx {
                st.pan = p;
            }
            let tone_row = matches!(c.fx, Fx::Tone(_) | Fx::ToneVol(_)) || matches!(c.vfx, Fx::Tone(_));
            let delay = if let Fx::Delay(d) = c.fx { d as u32 } else { 0 };
            st.pending = None;
            st.cut_at = None;
            st.retrig = None;
            // arpeggio state of the row
            st.arp = match c.fx {
                Fx::Arp(x, y) => {
                    let (x, y) = if x == 0 && y == 0 { st.m_arp } else { (x, y) };
                    st.m_arp = (x, y);
                    Some((x, y))
                }
                _ => None,
            };
            match c.note {
                NOTE_OFF | NOTE_CUT => {
                    close_note(&mut tracks, &mut tix, &variant, &mut raw_bend, &mut raw_vol, st, ch, beat, c.note == NOTE_CUT);
                }
                n if n >= 0 => {
                    if tone_row && st.open.is_some() {
                        st.tone = Some(n as f32 - st.key as f32);
                    } else if delay < ticks {
                        st.pending = Some((delay, n as i32));
                    }
                }
                _ => {}
            }
            if c.vol >= 0 {
                st.vol = c.vol as f32;
                if c.note < 0 && st.open.is_some() {
                    // a volume change without a note acts on the playing one (lane tracks follow it below)
                }
            }
            if let Fx::Filter(par) = c.fx {
                if par <= 0x7F {
                    st.cutv = par as f32;
                } else if par <= 0x8F {
                    st.resv = (par - 0x80) as f32 * 8.0;
                }
                if st.open.is_some() && variant[ch] {
                    let ins0 = st.open.as_ref().map(|o| o.ins).unwrap_or(0);
                    let ti = track_of!(ins0, ch);
                    tracks[ti].cut.push((beat, st.cutv));
                    tracks[ti].res.push((beat, st.resv));
                }
            }
            match c.fx {
                Fx::Cut(d) => st.cut_at = Some(d as u32),
                Fx::Off(d) => st.cut_at = Some(d as u32),
                Fx::Retrig(i, v) => st.retrig = Some((i, v)),
                Fx::Offset(_) | Fx::Filter(_) => {}
                _ => {}
            }
            // a row without arpeggio returns a held arpeggio to its note
            if st.arp.is_none() && st.arp_off != 0 && st.open.is_some() {
                let k = st.key;
                reopen(&mut tracks, &mut tix, &variant, &mut raw_bend, &mut raw_vol, st, ch, beat, k);
                st.arp_off = 0;
            }
        }

        // ── the ticks of the row
        for t in 0..ticks {
            let bt = beat + t as f64 * tick_len;
            for ch in 0..nch.min(cells.len()) {
                let c = cells[ch];
                let st = &mut chs[ch];
                // trigger
                if let Some((d, n)) = st.pending {
                    if d == t {
                        st.pending = None;
                        close_note(&mut tracks, &mut tix, &variant, &mut raw_bend, &mut raw_vol, st, ch, bt, true);
                        let ins_i = st.ins;
                        let ins_pan = ins_i.checked_sub(1).and_then(|k| m.ins.get(k)).and_then(|ins| ins.pan);
                        let smp = ins_i.checked_sub(1).and_then(|k| m.ins.get(k)).and_then(|ins| ins.map.get((n.clamp(0, 127)) as usize).copied().and_then(|si| ins.smps.get(si as usize)));
                        match smp {
                            Some(sm) if !sm.data.is_empty() => {
                                if c.vol < 0 {
                                    st.vol = sm.volume as f32;
                                }
                                st.key = n;
                                st.semis = 0.0;
                                st.tone = None;
                                st.vib_pos = 0.0;
                                st.trem_pos = 0.0;
                                st.arp_off = 0;
                                let pan = c.pan.or(if let Fx::Pan(p) = c.fx { Some(p) } else { None }).or(ins_pan).or(sm.pan.map(|p| p * 0.7)).unwrap_or(m.chan_pan.get(ch).copied().unwrap_or(0.0)).clamp(-1.0, 1.0);
                                st.pan = pan;
                                if let Some(ic) = m.ins.get(ins_i.saturating_sub(1)).and_then(|i| i.cutoff) {
                                    st.cutv = ic as f32;
                                    st.resv = m.ins.get(ins_i.saturating_sub(1)).and_then(|i| i.reso).unwrap_or(0) as f32;
                                }
                                open_note(&mut tracks, &mut tix, &variant, &mut raw_bend, &mut raw_vol, st, ch, bt, n.clamp(0, 127) as u8);
                            }
                            _ => dropped += 1,
                        }
                    }
                }
                if st.open.is_none() {
                    continue;
                }
                let first = t == 0;
                // effects: the row's and the volume column's
                for f in [c.fx, c.vfx] {
                    match f {
                        Fx::Porta(u) => {
                            let u = if u == 0 { st.m_porta } else { u };
                            if u != 0 {
                                st.m_porta = u;
                            }
                            if !first {
                                st.semis += slide_semis(m, st, u);
                                slides += (t == 1) as usize;
                            }
                        }
                        Fx::FinePorta(u) if first => {
                            st.semis += slide_semis(m, st, u);
                            slides += 1;
                        }
                        Fx::XFinePorta(u) if first => {
                            st.semis += slide_semis(m, st, u) / 4.0;
                            slides += 1;
                        }
                        Fx::Tone(_) | Fx::ToneVol(_) => {
                            let u = if let Fx::Tone(x) = f { x } else { 0 };
                            let u = if u == 0 { st.m_tone } else { u };
                            if u != 0 {
                                st.m_tone = u;
                            }
                            if !first {
                                if let Some(target) = st.tone {
                                    let step = slide_semis(m, st, u as i16).abs();
                                    if st.semis < target {
                                        st.semis = (st.semis + step).min(target);
                                    } else {
                                        st.semis = (st.semis - step).max(target);
                                    }
                                    slides += (t == 1) as usize;
                                }
                            }
                        }
                        _ => {}
                    }
                    match f {
                        Fx::Vib(x, y) | Fx::FineVib(x, y) => {
                            let (x, y) = (if x == 0 { st.m_vib.0 } else { x }, if y == 0 { st.m_vib.1 } else { y });
                            st.m_vib = (x, y);
                            if !first {
                                st.vib_pos += x as f32 / 64.0;
                            }
                        }
                        Fx::VibVol(_) => {
                            let (x, y) = st.m_vib;
                            if !first {
                                st.vib_pos += x as f32 / 64.0;
                            }
                            let _ = y;
                        }
                        _ => {}
                    }
                    match f {
                        Fx::VolSlide(v) | Fx::ToneVol(v) | Fx::VibVol(v) => {
                            let v = if v == 0 { st.m_vol } else { v };
                            if v != 0 {
                                st.m_vol = v;
                            }
                            if !first {
                                st.vol = (st.vol + v as f32).clamp(0.0, 64.0);
                            }
                        }
                        Fx::FineVol(v) if first => st.vol = (st.vol + v as f32).clamp(0.0, 64.0),
                        Fx::Trem(x, y) => {
                            let (x, y) = (if x == 0 { st.m_trem.0 } else { x }, if y == 0 { st.m_trem.1 } else { y });
                            st.m_trem = (x, y);
                            if !first {
                                st.trem_pos += x as f32 / 64.0;
                            }
                        }
                        _ => {}
                    }
                }
                // retrigger
                if let Some((iv, vm)) = st.retrig {
                    if t > 0 && iv > 0 && t % iv as u32 == 0 {
                        st.vol = vol_change(vm, st.vol);
                        let k = st.key;
                        reopen(&mut tracks, &mut tix, &variant, &mut raw_bend, &mut raw_vol, st, ch, bt, k);
                        retrigs += 1;
                    }
                }
                // arpeggio: a new note for every tick that changes the offset
                if let Some((x, y)) = st.arp {
                    let off = [0, x as i32, y as i32][(t % 3) as usize];
                    if off != st.arp_off {
                        st.arp_off = off;
                        let k = (st.key + off).clamp(0, 127);
                        reopen(&mut tracks, &mut tix, &variant, &mut raw_bend, &mut raw_vol, st, ch, bt, k);
                        arps += 1;
                    }
                }
                // cut / key off
                if st.cut_at == Some(t) {
                    st.cut_at = None;
                    close_note(&mut tracks, &mut tix, &variant, &mut raw_bend, &mut raw_vol, st, ch, bt, matches!(c.fx, Fx::Cut(_)));
                    continue;
                }
                // lanes of channels that carry their own
                if variant[ch] {
                    let vib = {
                        let (_, y) = st.m_vib;
                        let on = matches!(c.fx, Fx::Vib(..) | Fx::FineVib(..) | Fx::VibVol(_)) || matches!(c.vfx, Fx::Vib(..));
                        if on {
                            let depth = if matches!(c.fx, Fx::FineVib(..)) { y as f32 / 64.0 } else { y as f32 / 16.0 };
                            (st.vib_pos * std::f32::consts::TAU).sin() * depth
                        } else {
                            0.0
                        }
                    };
                    let trem = if matches!(c.fx, Fx::Trem(..)) || matches!(c.vfx, Fx::Trem(..)) { (st.trem_pos * std::f32::consts::TAU).sin() * st.m_trem.1 as f32 * 3.0 } else { 0.0 };
                    let ti = track_of!(st.open.as_ref().map(|o| o.ins).unwrap_or(0), ch);
                    let bend = st.semis + vib;
                    if (bend - st.last_bend).abs() > 0.004 || first {
                        raw_bend[ti].push((bt, bend));
                        st.last_bend = bend;
                    }
                    let v = (st.vol + trem).clamp(0.0, 64.0);
                    if (v - st.last_vol).abs() > 0.4 || first {
                        raw_vol[ti].push((bt, v));
                        st.last_vol = v;
                    }
                }
            }
        }
        beat += 0.25 * (1.0 + delay_rows as f64);
        if let Some(r0) = loop_to {
            pass += 1;
            row = r0;
            continue;
        }
        match (jump, brk) {
            (Some(o), b) => {
                oi = o;
                row = b.unwrap_or(0);
            }
            (None, Some(b)) => {
                oi += 1;
                row = b;
            }
            (None, None) => row += 1,
        }
    }
    for (ch, st) in chs.iter_mut().enumerate() {
        close_note(&mut tracks, &mut tix, &variant, &mut raw_bend, &mut raw_vol, st, ch, beat, false);
    }
    // finish the lanes: bend range from the biggest slide, points thinned
    for (ti, t) in tracks.iter_mut().enumerate() {
        t.notes.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
        let maxb = raw_bend[ti].iter().fold(0.0f32, |a, p| a.max(p.1.abs()));
        let range = maxb.ceil().clamp(1.0, 24.0);
        t.bend_range = range;
        let mut pts: Vec<(f64, f32)> = vec![];
        for (b, v) in &raw_bend[ti] {
            let val = (0.5 + v / (2.0 * range)).clamp(0.0, 1.0);
            if let Some(l) = pts.last() {
                if (l.1 - val).abs() < 0.0006 && *b - l.0 < 2.0 {
                    continue;
                }
            }
            pts.push((*b, val));
        }
        if pts.iter().all(|p| (p.1 - 0.5).abs() < 0.002) {
            pts.clear();
        }
        t.bend = pts;
        let mut vp: Vec<(f64, f32)> = vec![];
        for (b, v) in &raw_vol[ti] {
            let val = (v / 64.0 * 0.9).clamp(0.0, 1.5);
            if let Some(l) = vp.last() {
                if (l.1 - val).abs() < 0.004 && *b - l.0 < 2.0 {
                    continue;
                }
            }
            vp.push((*b, val));
        }
        t.vol = vp;
        let mut cp: Vec<(f64, f32)> = vec![];
        for (b, c) in &t.cut {
            let val = it_cut_norm(*c);
            if cp.last().map(|l| (l.1 - val).abs() < 0.002 && *b - l.0 < 4.0).unwrap_or(false) {
                continue;
            }
            cp.push((*b, val));
        }
        t.cut = if cp.iter().all(|p| p.1 > 0.995) { vec![] } else { cp };
        let mut rp: Vec<(f64, f32)> = vec![];
        for (b, r) in &t.res {
            let val = (r / 127.0).clamp(0.0, 1.0);
            if rp.last().map(|l| (l.1 - val).abs() < 0.002 && *b - l.0 < 4.0).unwrap_or(false) {
                continue;
            }
            rp.push((*b, val));
        }
        t.res = if rp.iter().all(|p| p.1 < 0.002) { vec![] } else { rp };
        // keep the lanes a sensible size
        if t.bend.len() > 30000 {
            let k = t.bend.len() / 30000 + 1;
            t.bend = t.bend.iter().enumerate().filter(|(i, _)| i % k == 0).map(|(_, p)| *p).collect();
        }
        if t.vol.len() > 30000 {
            let k = t.vol.len() / 30000 + 1;
            t.vol = t.vol.iter().enumerate().filter(|(i, _)| i % k == 0).map(|(_, p)| *p).collect();
        }
    }
    Played { tracks, tempos, beats: beat, dropped, arps, slides, retrigs }
}

/// semitones a slide of `u` units moves the pitch on one tick
fn slide_semis(m: &Module, st: &Ch, u: i16) -> f32 {
    if u == 0 {
        return 0.0;
    }
    if m.linear {
        return u as f32 / 16.0;
    }
    // Amiga periods: C-5 is 428; going up shortens the period
    let p = 428.0 * 2f32.powf(-((st.key as f32 + st.semis) - 60.0) / 12.0);
    let np = (p - u as f32).max(20.0);
    12.0 * (p / np).log2()
}

#[allow(clippy::too_many_arguments)]
fn open_note(tracks: &mut Vec<PTrack>, tix: &mut std::collections::HashMap<(usize, usize), usize>, variant: &[bool], raw_bend: &mut Vec<Vec<(f64, f32)>>, raw_vol: &mut Vec<Vec<(f64, f32)>>, st: &mut Ch, ch: usize, bt: f64, key: u8) {
    let ins = st.ins.saturating_sub(1);
    let lane = variant[ch];
    let vel = if lane { 1.0 } else { (st.vol / 64.0).clamp(0.03, 1.0) };
    st.open = Some(Open { start: bt, key, vel, pan: st.pan, ins });
    if lane {
        let k = (ins, ch + 1);
        let ti = *tix.entry(k).or_insert_with(|| {
            tracks.push(PTrack { ins, chan: Some(ch), notes: vec![], bend: vec![], bend_range: 2.0, vol: vec![], cut: vec![], res: vec![] });
            raw_bend.push(vec![]);
            raw_vol.push(vec![]);
            tracks.len() - 1
        });
        // hold the old value up to the new note, then jump
        if let Some(l) = raw_bend[ti].last().copied() {
            raw_bend[ti].push((bt - 0.0004, l.1));
        }
        raw_bend[ti].push((bt, st.semis));
        if let Some(l) = raw_vol[ti].last().copied() {
            raw_vol[ti].push((bt - 0.0004, l.1));
        }
        raw_vol[ti].push((bt, st.vol));
        tracks[ti].cut.push((bt, st.cutv));
        tracks[ti].res.push((bt, st.resv));
        st.last_bend = st.semis;
        st.last_vol = st.vol;
    }
}

#[allow(clippy::too_many_arguments)]
fn close_note(tracks: &mut Vec<PTrack>, tix: &mut std::collections::HashMap<(usize, usize), usize>, variant: &[bool], raw_bend: &mut Vec<Vec<(f64, f32)>>, raw_vol: &mut Vec<Vec<(f64, f32)>>, st: &mut Ch, ch: usize, end: f64, hard: bool) {
    let Some(o) = st.open.take() else { return };
    let len = (end - o.start).max(0.03);
    let k = (o.ins, if variant[ch] { ch + 1 } else { 0 });
    let ti = *tix.entry(k).or_insert_with(|| {
        tracks.push(PTrack { ins: o.ins, chan: if k.1 > 0 { Some(ch) } else { None }, notes: vec![], bend: vec![], bend_range: 2.0, vol: vec![], cut: vec![], res: vec![] });
        raw_bend.push(vec![]);
        raw_vol.push(vec![]);
        tracks.len() - 1
    });
    let mut n = Note::new(o.start, len, o.key, o.vel);
    n.pan = o.pan;
    // reso 0 tells the Sampler this note is cut, not released (a new note on the channel, a note cut): no fade-out tail
    n.reso = if hard { 0.0 } else { 0.5 };
    tracks[ti].notes.push(n);
}

/// end the playing note and start the same instrument again on `key` (arpeggio steps, retriggers)
#[allow(clippy::too_many_arguments)]
fn reopen(tracks: &mut Vec<PTrack>, tix: &mut std::collections::HashMap<(usize, usize), usize>, variant: &[bool], raw_bend: &mut Vec<Vec<(f64, f32)>>, raw_vol: &mut Vec<Vec<(f64, f32)>>, st: &mut Ch, ch: usize, bt: f64, key: i32) {
    close_note(tracks, tix, variant, raw_bend, raw_vol, st, ch, bt, true);
    if st.ins > 0 {
        open_note(tracks, tix, variant, raw_bend, raw_vol, st, ch, bt, key.clamp(0, 127) as u8);
    }
}

// ── the Sampler of a module ───────────────────────────────────────────────────

/// ADSR (attack, decay, sustain 0..1, release) in seconds from a volume envelope and a fade-out
fn adsr_of(ins: &Ins, sec_per_tick: f32) -> (f32, f32, f32, f32) {
    let release_floor = 0.012;
    if ins.env.len() < 2 {
        return (0.0, 0.2, 1.0, if ins.fade_s > 0.0 { ins.fade_s.min(6.0) } else { release_floor });
    }
    let peak = ins.env.iter().map(|p| p.1).max().unwrap_or(64).max(1) as f32;
    let t_of = |i: usize| ins.env[i].0 as f32 * sec_per_tick;
    let ipeak = ins.env.iter().position(|p| p.1 as f32 >= peak - 0.5).unwrap_or(0);
    let attack = t_of(ipeak);
    let (sustain, decay, rel_env) = match ins.env_sustain {
        Some(si) if si >= ipeak => {
            let level = ins.env[si].1 as f32 / peak;
            let tail = t_of(ins.env.len() - 1) - t_of(si);
            (level, (t_of(si) - t_of(ipeak)).max(0.0), tail.max(0.0))
        }
        _ => {
            let last = ins.env.len() - 1;
            // no sustain point: it runs to the end of the envelope on its own; keep the last level
            (ins.env[last].1 as f32 / peak, (t_of(last) - t_of(ipeak)).max(0.0), 0.0)
        }
    };
    (attack, decay, sustain.clamp(0.0, 1.0), (rel_env.max(ins.fade_s)).clamp(release_floor, 6.0))
}

/// an Impulse Tracker cutoff (0..127, frequency 110 * 2^(0.25 + c / 24)) as the Sampler's 0..1 cutoff; 127 is open
pub fn it_cut_norm(c: f32) -> f32 {
    if c >= 127.0 {
        return 1.0;
    }
    let f = 110.0 * 2f32.powf(0.25 + c / 24.0);
    ((f / 40.0).ln() / (20000.0f32 / 40.0).ln()).clamp(0.0, 1.0)
}

fn norm_a(s: f32) -> f32 {
    ((s - 0.0005).max(0.0) / 4.0).sqrt().clamp(0.0, 1.0)
}
fn norm_d(s: f32) -> f32 {
    ((s - 0.005).max(0.0) / 4.0).sqrt().clamp(0.0, 1.0)
}
fn norm_r(s: f32) -> f32 {
    ((s - 0.004).max(0.0) / 6.0).sqrt().clamp(0.0, 1.0)
}

fn write_wav(path: &std::path::Path, data: &[i16], rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec { channels: 1, sample_rate: rate.clamp(1000, 384000), bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| format!("{}: {e}", path.display()))?;
    for v in data {
        w.write_sample(*v).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// The Sampler config (JSON, absolute file paths) of one tracker instrument, with its WAV files written to `dir`, and the instrument parameters.
pub fn sampler_for(ins: &Ins, idx: usize, dir: &std::path::Path, sec_per_tick: f32) -> Result<Option<(String, Vec<(String, f32)>)>, String> {
    if ins.smps.is_empty() {
        return Ok(None);
    }
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let mut zones: Vec<Zone> = vec![];
    // zones: runs of keys with the same sample and note shift
    let mut files: Vec<Option<String>> = vec![None; ins.smps.len()];
    let mut k = 0usize;
    while k < 128 {
        let si = ins.map.get(k).copied().unwrap_or(255);
        if si == 255 || si as usize >= ins.smps.len() || ins.smps[si as usize].data.is_empty() {
            k += 1;
            continue;
        }
        let sh = ins.shift.get(k).copied().unwrap_or(0);
        let mut e = k;
        while e + 1 < 128 && ins.map.get(e + 1).copied() == Some(si) && ins.shift.get(e + 1).copied().unwrap_or(0) == sh {
            e += 1;
        }
        let smp = &ins.smps[si as usize];
        let file = match &files[si as usize] {
            Some(f) => f.clone(),
            None => {
                let name: String = smp.name.chars().map(|c| if c.is_alphanumeric() { c } else { '_' }).collect();
                let f = dir.join(format!("{:02}_{:02}_{name}.wav", idx + 1, si)).to_string_lossy().replace(std::path::MAIN_SEPARATOR, "/");
                write_wav(std::path::Path::new(&f), &smp.data, smp.rate)?;
                files[si as usize] = Some(f.clone());
                f
            }
        };
        let mut z = Zone::new(if smp.name.trim().is_empty() { &ins.name } else { &smp.name }, &file);
        z.lo = k as u8;
        z.hi = e as u8;
        z.root = (60 - sh as i32 - smp.rel_note).clamp(0, 127) as u8;
        z.tune = smp.cents;
        // the part is panned by its notes (sample, instrument and channel pan are folded in there)
        z.pan = 0.0;
        z.vol = (smp.vol_db + ins.gain_db).clamp(-48.0, 18.0);
        let frames = smp.data.len() as u32;
        z.loop_end = frames;
        if smp.looped && smp.loop_end > smp.loop_start + 2 {
            z.loop_mode = if smp.pingpong { 2 } else { 1 };
            z.loop_start = smp.loop_start.min(frames);
            z.loop_end = smp.loop_end.min(frames);
        }
        zones.push(z);
        k = e + 1;
    }
    if zones.is_empty() {
        return Ok(None);
    }
    let (a, d, s, r) = adsr_of(ins, sec_per_tick);
    let mut cfg = Config { name: ins.name.clone(), zones, attack: norm_a(a), decay: norm_d(d), sustain: s, release: norm_r(r), ..Config::default() };
    if ins.pan_env.len() >= 2 {
        cfg.pan_env = crate::dsp::sampler::Env { pts: ins.pan_env.iter().map(|p| (p.0 as f32 * sec_per_tick, p.1)).collect(), sustain: ins.pan_sustain, looped: ins.pan_loop };
    }
    cfg.rnd_vol = ins.swing[0];
    cfg.rnd_pan = ins.swing[1];
    cfg.rnd_cut = ins.swing[2];
    cfg.rnd_res = ins.swing[3];
    if let Some(c) = ins.cutoff {
        cfg.it_cut = c as f32;
        cfg.cutoff = it_cut_norm(c as f32);
    }
    if let Some(r) = ins.reso {
        cfg.reso = (r as f32 / 127.0).clamp(0.0, 1.0);
    }
    if ins.filter_env.len() >= 2 {
        cfg.filter_env = crate::dsp::sampler::Env { pts: ins.filter_env.iter().map(|p| (p.0 as f32 * sec_per_tick, p.1)).collect(), sustain: ins.filter_sustain, looped: ins.filter_loop };
    }
    if ins.pitch_env.len() >= 2 {
        cfg.pitch_env = crate::dsp::sampler::Env { pts: ins.pitch_env.iter().map(|p| (p.0 as f32 * sec_per_tick, p.1)).collect(), sustain: ins.pitch_sustain, looped: ins.pitch_loop };
    }
    if ins.env.len() >= 2 {
        // the real multi-point envelope, in seconds
        cfg.env = ins.env.iter().map(|p| (p.0 as f32 * sec_per_tick, p.1 as f32 / 64.0)).collect();
        cfg.env_sustain = ins.env_sustain;
        cfg.env_loop = ins.env_loop;
        cfg.fade = ins.fade_s;
    } else if ins.fade_s > 0.0 {
        // no envelope: the sound fades out after the key-off
        cfg.env = vec![(0.0, 1.0), (0.001, 1.0)];
        cfg.env_sustain = Some(1);
        cfg.fade = ins.fade_s;
    }
    let json = serde_json::to_string(&cfg).map_err(|e| e.to_string())?;
    Ok(Some((json, vec![])))
}

// ── the import ───────────────────────────────────────────────────────────────────

/// What was imported, for the status line.
pub struct Report {
    pub format: &'static str,
    pub title: String,
    pub channels: usize,
    pub instruments: usize,
    pub notes: usize,
    pub ignored: usize,
    pub dropped: usize,
    pub arps: usize,
    pub slides: usize,
    pub retrigs: usize,
}

/// Parse a module, write its samples (beside `dir`) and return it as parts, each with the Sampler that plays the module's own samples.
pub fn parse(data: &[u8], path: &str, dir: &std::path::Path) -> Result<(ImportedMidi, Report), String> {
    let m = parse_module(data)?;
    let played = play(&m);
    let stem: String = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "module".into());
    let safe: String = stem.chars().map(|c| if c.is_alphanumeric() || c == '-' { c } else { '_' }).collect();
    // a digest keeps two modules of the same name from overwriting each other's samples
    let mut h: u32 = 2166136261;
    for b in data.iter().step_by((data.len() / 4096).max(1)) {
        h = (h ^ *b as u32).wrapping_mul(16777619);
    }
    let sdir = dir.join(format!("{safe}_{h:08x}"));
    let spt = 2.5 / m.tempo.max(32) as f32;
    let mut samplers: std::collections::HashMap<usize, Option<(String, Vec<(String, f32)>)>> = std::collections::HashMap::new();
    let mut tracks = vec![];
    let mut total = 0usize;
    for pt in &played.tracks {
        if pt.notes.is_empty() {
            continue;
        }
        let ins = &m.ins[pt.ins.min(m.ins.len().saturating_sub(1))];
        let cfg = match samplers.get(&pt.ins) {
            Some(c) => c.clone(),
            None => {
                let c = sampler_for(ins, pt.ins, &sdir, spt)?;
                samplers.insert(pt.ins, c.clone());
                c
            }
        };
        let Some((json, _)) = cfg else { continue };
        let mut name = if ins.name.trim().is_empty() { format!("Instrument {}", pt.ins + 1) } else { ins.name.trim().to_string() };
        if let Some(c) = pt.chan {
            name = format!("{name} (ch {})", c + 1);
        }
        total += pt.notes.len();
        tracks.push(ImportedTrack {
            name,
            plugin: String::new(),
            named: true,
            program: Some(0),
            bank: 0,
            channel: 0,
            notes: pt.notes.clone(),
            volume: 0.8 * m.gain,
            pan: 0.0,
            vol_auto: pt.vol.iter().map(|(b, v)| (*b, v * m.gain)).collect(),
            pan_auto: vec![],
            bend_auto: pt.bend.clone(),
            bend_range: pt.bend_range,
            samples: vec![],
            sampler: Some((json, vec![])),
            percussive: looks_percussive(&ins.name),
            extra_auto: { let mut v = vec![]; if !pt.cut.is_empty() { v.push(("cutoff".to_string(), pt.cut.clone())); } if !pt.res.is_empty() { v.push(("reso".to_string(), pt.res.clone())); } v },
        });
    }
    let bpm = played.tempos.first().map(|t| t.1);
    let rep = Report { format: m.format, title: m.title.clone(), channels: m.nch, instruments: samplers.len(), notes: total, ignored: m.ignored, dropped: played.dropped, arps: played.arps, slides: played.slides, retrigs: played.retrigs };
    Ok((ImportedMidi { tracks, bpm, tempos: played.tempos, length_beats: played.beats }, rep))
}

/// an instrument name that sounds like a drum or percussion piece
pub fn looks_percussive(name: &str) -> bool {
    let n = name.to_lowercase();
    ["kick", "bass drum", "bassdrum", "snare", "hat", "cymbal", "crash", "ride", "splash", "tom", "clap", "rim", "cowbell", "conga", "bongo", "wood block", "woodblock", "tambourine", "shaker", "perc", "drum", "cabasa", "claves", "triangle", "agogo", "timbale", "stick"].iter().any(|w| n.contains(w))
}
