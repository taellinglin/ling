//! LingStation for LingOS: the sequencer and synthesizer behind the LingOS window app.
//!
//! The Windows / Linux / macOS LingStationX runs on the `ling-daw` engine, which wants a heap, threads, audio devices and files. LingOS has
//! none of those yet, so this is the same idea at kernel scale: a tracker-style song (8 channels x 64 rows), fourteen built-in instruments
//! (saw lead, bass, pluck, pad, bell, kick, snare, hat, supersaw, growl, voice, organ, clap, tom), a master effect rack (drive, crusher, echo,
//! reverb), note scripts (arpeggiator, humanize, euclid, transpose ...), Mix AI, MIDI in / out, a sample-accurate sequencer and a WAV renderer,
//! in `no_std` + `alloc` with only `libm` for maths. The kernel feeds `Engine::render` into the AC'97 mixer and draws the grid; everything musical lives here,
//! so it is unit-tested on the host.
//!
//! Not here, because they need things LingOS does not have yet: the Ling language runtime and its UI (the whole `.ling` app), audio *input*
//! (the AC'97 driver only plays), MIDI / USB input devices, a file picker, threads, TTF text for sheet music and PDF, the Whisper / Qwen
//! tools (Vocal -> MIDI, lyric timing, AI instrumentation), sample import (SoundFonts, WAV / FLAC / OGG decode) and the 385-preset
//! Prism / Growl / Pluck / Vocoder / Ringstorm instrument set (those lean on `std` and large tables).
#![no_std]
extern crate alloc;

use alloc::vec;
use alloc::vec::Vec;

pub mod fx;
pub mod midi;
pub mod mix;
pub mod script;

pub const CHANNELS: usize = 8;
pub const MAX_ROWS: usize = 64;
pub const SAMPLE_RATE: u32 = 48_000;
pub const VOICES: usize = 24;
const KS_LEN: usize = 2048;

pub const INSTRUMENTS: [&str; 14] = ["Saw Lead", "Square Bass", "Pluck", "Pad", "FM Bell", "Kick", "Snare", "Hat", "Supersaw", "Growl", "Voice", "Organ", "Clap", "Tom"];
/// the names of the master effect knobs, in `Song::fx` order
pub const FX_NAMES: [&str; 4] = ["Drive", "Echo", "Verb", "Crush"];
pub const NOTE_OFF: u8 = 255;

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cell {
    /// 0 = empty, 1..=127 = MIDI key, 255 = note off
    pub note: u8,
    /// 1..=15 (0 = the channel default)
    pub vel: u8,
}
impl Cell {
    pub const EMPTY: Cell = Cell { note: 0, vel: 0 };
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Channel {
    pub inst: u8,
    /// 0..=100
    pub vol: u8,
    /// -100..=100
    pub pan: i8,
    pub mute: bool,
}

#[derive(Clone)]
pub struct Song {
    pub cells: [[Cell; CHANNELS]; MAX_ROWS],
    pub chans: [Channel; CHANNELS],
    pub bpm: u16,
    /// rows per beat (4 = sixteenths)
    pub rpb: u8,
    /// rows played before the pattern loops (8..=64)
    pub length: u8,
    /// master effect levels 0..=10: drive, echo, reverb, crusher
    pub fx: [u8; 4],
}

impl Default for Song {
    fn default() -> Song {
        Song::empty()
    }
}

fn note_name(k: u8, out: &mut [u8; 3]) {
    const N: [&[u8; 2]; 12] = [b"C-", b"C#", b"D-", b"D#", b"E-", b"F-", b"F#", b"G-", b"G#", b"A-", b"A#", b"B-"];
    let n = N[(k % 12) as usize];
    out[0] = n[0];
    out[1] = n[1];
    out[2] = b'0' + (k / 12).saturating_sub(1).min(9);
}

impl Song {
    pub fn empty() -> Song {
        let insts = [0u8, 1, 2, 3, 4, 5, 6, 7];
        let mut chans = [Channel { inst: 0, vol: 80, pan: 0, mute: false }; CHANNELS];
        for (i, c) in chans.iter_mut().enumerate() {
            c.inst = insts[i];
            c.pan = match i {
                0 => 0,
                1 => 0,
                2 => -35,
                3 => 25,
                4 => 45,
                _ => 0,
            };
        }
        Song { cells: [[Cell::EMPTY; CHANNELS]; MAX_ROWS], chans, bpm: 124, rpb: 4, length: 32, fx: [0; 4] }
    }

    /// a groove that plays as soon as the app opens: four on the floor, a bass line, a plucked arpeggio, a pad and a bell
    pub fn demo() -> Song {
        let mut s = Song::empty();
        s.bpm = 122;
        s.length = 32;
        // kick (ch5), hat (ch7), snare (ch6)
        for r in 0..32usize {
            if r % 4 == 0 {
                s.cells[r][5] = Cell { note: 36, vel: 15 };
            }
            if r % 4 == 2 {
                s.cells[r][7] = Cell { note: 42, vel: 9 };
            }
            if r % 8 == 4 {
                s.cells[r][6] = Cell { note: 38, vel: 13 };
            }
        }
        s.cells[30][7] = Cell { note: 44, vel: 12 }; // an open hat to turn the bar
        // bass (ch1): A minor movement
        let bass = [(0usize, 33u8), (3, 33), (6, 36), (8, 38), (11, 38), (14, 40), (16, 29), (19, 29), (22, 33), (24, 31), (27, 31), (30, 35)];
        for (r, k) in bass {
            s.cells[r][1] = Cell { note: k, vel: 13 };
        }
        // pluck arpeggio (ch2)
        let arp = [57u8, 60, 64, 69, 64, 60, 57, 60];
        for r in 0..32usize {
            if r % 2 == 0 {
                let chord_shift = [0i32, 0, -4, -2][(r / 8) % 4];
                let k = (arp[(r / 2) % 8] as i32 + chord_shift).clamp(1, 127) as u8;
                s.cells[r][2] = Cell { note: k, vel: 10 };
            }
        }
        // pad chords (ch3), held to the next chord
        let pad = [(0usize, 57u8), (8, 53), (16, 55), (24, 52)];
        for (r, k) in pad {
            s.cells[r][3] = Cell { note: k, vel: 8 };
            s.cells[r + 7][3] = Cell { note: NOTE_OFF, vel: 0 };
        }
        // a bell (ch4) on the high notes
        for (r, k) in [(0usize, 81u8), (12, 79), (16, 77), (28, 76)] {
            s.cells[r][4] = Cell { note: k, vel: 9 };
        }
        s.chans[0].mute = true; // the lead is there to play
        s.fx = [0, 2, 3, 0];
        s
    }

    /// "C-4 v9" style text of a cell for the grid: (note text, velocity digit or b' ')
    pub fn cell_text(c: Cell) -> ([u8; 3], u8) {
        let mut n = *b"...";
        if c.note == NOTE_OFF {
            n = *b"===";
        } else if c.note > 0 {
            note_name(c.note, &mut n);
        }
        let v = if c.note > 0 && c.note != NOTE_OFF { b"0123456789abcdef"[(c.vel.min(15)) as usize] } else { b' ' };
        (n, v)
    }

    // ── saving and loading: "LSX1", bpm, rpb, length, channels, cells ──────────
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = Vec::with_capacity(16 + CHANNELS * 4 + MAX_ROWS * CHANNELS * 2);
        v.extend_from_slice(b"LSX2");
        v.extend_from_slice(&self.bpm.to_le_bytes());
        v.push(self.rpb);
        v.push(self.length);
        for c in &self.chans {
            v.push(c.inst);
            v.push(c.vol);
            v.push(c.pan as u8);
            v.push(c.mute as u8);
        }
        for r in 0..MAX_ROWS {
            for c in 0..CHANNELS {
                v.push(self.cells[r][c].note);
                v.push(self.cells[r][c].vel);
            }
        }
        v.extend_from_slice(&self.fx);
        v
    }

    pub fn from_bytes(b: &[u8]) -> Option<Song> {
        if b.len() < 8 + CHANNELS * 4 + MAX_ROWS * CHANNELS * 2 || (&b[0..4] != b"LSX1" && &b[0..4] != b"LSX2") {
            return None;
        }
        let mut s = Song::empty();
        s.bpm = u16::from_le_bytes([b[4], b[5]]).clamp(40, 240);
        s.rpb = b[6].clamp(1, 8);
        s.length = b[7].clamp(8, MAX_ROWS as u8);
        let mut p = 8;
        for c in s.chans.iter_mut() {
            c.inst = b[p].min(INSTRUMENTS.len() as u8 - 1);
            c.vol = b[p + 1].min(100);
            c.pan = (b[p + 2] as i8).clamp(-100, 100);
            c.mute = b[p + 3] != 0;
            p += 4;
        }
        for r in 0..MAX_ROWS {
            for c in 0..CHANNELS {
                let (n, v) = (b[p], b[p + 1]);
                s.cells[r][c] = Cell { note: if n == NOTE_OFF || n <= 127 { n } else { 0 }, vel: v.min(15) };
                p += 2;
            }
        }
        if &b[0..4] == b"LSX2" && b.len() >= p + 4 {
            for i in 0..4 {
                s.fx[i] = b[p + i].min(10);
            }
        }
        Some(s)
    }

    pub fn seconds(&self) -> f32 {
        self.length as f32 * 60.0 / (self.bpm.max(1) as f32 * self.rpb.max(1) as f32)
    }

    /// how long the sound rings after the last row: echoes and the reverb need a few seconds to die away
    pub fn tail_seconds(&self) -> f32 {
        1.0 + if self.fx[1] > 0 { 2.0 } else { 0.0 } + if self.fx[2] > 0 { 2.5 } else { 0.0 }
    }
}

// ── the synthesizer ──────────────────────────────────────────────────────────

#[inline]
fn hz(key: u8) -> f32 {
    440.0 * libm::powf(2.0, (key as f32 - 69.0) / 12.0)
}

struct Rng(u32);
impl Rng {
    #[inline]
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

#[derive(Clone)]
struct Voice {
    active: bool,
    chan: u8,
    inst: u8,
    key: u8,
    vel: f32,
    gate: bool,
    /// seconds since the note began / since the gate closed
    t: f32,
    rel_t: f32,
    ph: [f32; 5],
    sv: [f32; 4],
    lp: f32,
    lp2: f32,
    env: f32,
    freq: f32,
    ks_pos: usize,
    ks_len: usize,
    ks_prev: f32,
    age: u64,
}

impl Voice {
    fn idle() -> Voice {
        Voice { active: false, chan: 0, inst: 0, key: 0, vel: 0.0, gate: false, t: 0.0, rel_t: 0.0, ph: [0.0; 5], sv: [0.0; 4], lp: 0.0, lp2: 0.0, env: 0.0, freq: 440.0, ks_pos: 0, ks_len: 100, ks_prev: 0.0, age: 0 }
    }
}

pub struct Engine {
    pub song: Song,
    pub playing: bool,
    /// the row being played (or last played)
    pub row: usize,
    pub master: f32,
    /// 0..1 peak of the last block, for a meter
    pub peak: f32,
    voices: Vec<Voice>,
    ks: Vec<f32>,
    rng: Rng,
    sample_in_row: f32,
    clock: u64,
    /// set when the sequencer starts a row (the UI follows it)
    pub rows_played: u64,
    /// loop the pattern (true) or stop at its end
    pub looping: bool,
    sr: f32,
    chain: fx::Chain,
}

impl Engine {
    pub fn new(song: Song) -> Engine {
        Engine { song, playing: false, row: 0, master: 0.8, peak: 0.0, voices: vec![Voice::idle(); VOICES], ks: vec![0.0; VOICES * KS_LEN], rng: Rng(0x9E37_79B9), sample_in_row: 0.0, clock: 0, rows_played: 0, looping: true, sr: SAMPLE_RATE as f32, chain: fx::Chain::new() }
    }

    pub fn play(&mut self) {
        if !self.playing {
            self.playing = true;
            self.sample_in_row = f32::MAX; // trigger the current row at once
            self.row = self.row % self.song.length.max(1) as usize;
        }
    }
    pub fn stop(&mut self) {
        self.playing = false;
        self.row = 0;
        for v in self.voices.iter_mut() {
            if v.active {
                v.gate = false;
            }
        }
    }
    pub fn all_off(&mut self) {
        for v in self.voices.iter_mut() {
            v.active = false;
        }
    }
    pub fn seek(&mut self, row: usize) {
        self.row = row.min(self.song.length as usize - 1);
        self.sample_in_row = f32::MAX;
    }

    fn samples_per_row(&self) -> f32 {
        self.sr * 60.0 / (self.song.bpm.max(20) as f32 * self.song.rpb.max(1) as f32)
    }

    /// start a note on a channel (the sequencer's trigger, and the keyboard's audition)
    pub fn trigger(&mut self, chan: usize, key: u8, vel: u8) {
        if chan >= CHANNELS || key == 0 || key == NOTE_OFF {
            return;
        }
        let inst = self.song.chans[chan].inst;
        let vel = if vel == 0 { 11 } else { vel.min(15) } as f32 / 15.0;
        self.clock += 1;
        // a mono channel (bass, lead, drums of one kind) cuts its previous note
        for v in self.voices.iter_mut() {
            if v.active && v.chan as usize == chan && v.gate {
                v.gate = false;
            }
        }
        let vi = self.voices.iter().position(|v| !v.active).unwrap_or_else(|| self.voices.iter().enumerate().min_by_key(|(_, v)| v.age).map(|(i, _)| i).unwrap_or(0));
        let f = hz(key);
        let mut v = Voice::idle();
        v.active = true;
        v.chan = chan as u8;
        v.inst = inst;
        v.key = key;
        v.vel = vel;
        v.gate = true;
        v.freq = f;
        v.age = self.clock;
        v.env = 0.0;
        if inst == 2 {
            // Karplus-Strong: a noise burst in a delay line one period long
            let len = ((self.sr / f) as usize).clamp(8, KS_LEN - 1);
            v.ks_len = len;
            let base = vi * KS_LEN;
            for i in 0..len {
                self.ks[base + i] = self.rng.next() * 0.9;
            }
        }
        self.voices[vi] = v;
    }

    pub fn release(&mut self, chan: usize) {
        for v in self.voices.iter_mut() {
            if v.active && v.chan as usize == chan && v.gate {
                v.gate = false;
            }
        }
    }

    fn step_row(&mut self) {
        let r = self.row;
        for c in 0..CHANNELS {
            let cell = self.song.cells[r][c];
            if self.song.chans[c].mute {
                continue;
            }
            if cell.note == NOTE_OFF {
                self.release(c);
            } else if cell.note > 0 {
                self.trigger(c, cell.note, cell.vel);
            }
        }
        self.rows_played += 1;
    }

    /// Render interleaved stereo i16 (`out.len()` / 2 frames), advancing the sequencer.
    pub fn render(&mut self, out: &mut [i16]) {
        let frames = out.len() / 2;
        let spr = self.samples_per_row();
        let dt = 1.0 / self.sr;
        let mut peak = 0.0f32;
        for f in 0..frames {
            if self.playing {
                if self.sample_in_row >= spr {
                    if self.sample_in_row != f32::MAX {
                        self.sample_in_row -= spr;
                        self.row += 1;
                        if self.row >= self.song.length as usize {
                            if self.looping {
                                self.row = 0;
                            } else {
                                self.playing = false;
                                self.row = 0;
                            }
                        }
                    } else {
                        self.sample_in_row = 0.0;
                    }
                    if self.playing {
                        self.step_row();
                    }
                }
                self.sample_in_row += 1.0;
            }
            let (mut l, mut r) = (0.0f32, 0.0f32);
            for vi in 0..VOICES {
                if !self.voices[vi].active {
                    continue;
                }
                let s = self.voice_sample(vi, dt);
                let ch = self.song.chans[self.voices[vi].chan as usize];
                let g = ch.vol as f32 / 100.0;
                let pan = ch.pan as f32 / 100.0;
                let (gl, gr) = if pan <= 0.0 { (1.0, 1.0 + pan) } else { (1.0 - pan, 1.0) };
                l += s * g * gl;
                r += s * g * gr;
            }
            // a soft ceiling: tanh-like without a call
            let (l, r) = self.chain.process(l * self.master * 0.55, r * self.master * 0.55, self.song.fx);
            let (l, r) = (soft(l), soft(r));
            peak = peak.max(l.abs()).max(r.abs());
            out[f * 2] = (l * 32000.0) as i16;
            out[f * 2 + 1] = (r * 32000.0) as i16;
        }
        self.peak = peak.max(self.peak * 0.9);
    }

    fn voice_sample(&mut self, vi: usize, dt: f32) -> f32 {
        let sr = self.sr;
        let noise = self.rng.next();
        let v = &mut self.voices[vi];
        v.t += dt;
        if !v.gate {
            v.rel_t += dt;
        }
        let f = v.freq;
        let out: f32;
        match v.inst {
            0 => {
                // saw lead: two detuned saws, an envelope-driven low-pass
                let d = [1.0f32, 1.006];
                let mut s = 0.0;
                for k in 0..2 {
                    v.ph[k] = (v.ph[k] + f * d[k] * dt).fract_pos();
                    s += 2.0 * v.ph[k] - 1.0;
                }
                let env = adsr(v.t, v.gate, v.rel_t, 0.004, 0.14, 0.55, 0.18);
                let cut = 0.08 + 0.55 * env;
                v.lp += (s * 0.5 - v.lp) * cut;
                out = v.lp * env;
                if !v.gate && v.rel_t > 0.18 {
                    v.active = false;
                }
            }
            1 => {
                // square bass with a sine sub
                v.ph[0] = (v.ph[0] + f * dt).fract_pos();
                v.ph[1] = (v.ph[1] + f * 0.5 * dt).fract_pos();
                let sq = if v.ph[0] < 0.5 { 1.0 } else { -1.0 };
                let sub = libm::sinf(v.ph[1] * core::f32::consts::TAU);
                let env = adsr(v.t, v.gate, v.rel_t, 0.003, 0.12, 0.75, 0.07);
                let cut = 0.05 + 0.1 * env;
                v.lp += (sq * 0.6 - v.lp) * cut;
                out = (v.lp + sub * 0.7) * env * 0.9;
                if !v.gate && v.rel_t > 0.07 {
                    v.active = false;
                }
            }
            2 => {
                // Karplus-Strong string
                let base = vi * KS_LEN;
                let i = v.ks_pos;
                let a = self.ks[base + i];
                let b = self.ks[base + (i + 1) % v.ks_len];
                let new = (a + b) * 0.5 * 0.9965;
                self.ks[base + i] = new;
                v.ks_pos = (i + 1) % v.ks_len;
                let mut s = a;
                if !v.gate {
                    // a released string is damped
                    s *= libm::expf(-v.rel_t * 14.0);
                }
                out = s * 0.9;
                if v.t > 4.0 || (!v.gate && v.rel_t > 0.4) {
                    v.active = false;
                }
            }
            3 => {
                // pad: three slow saws through a low-pass
                let d = [1.0f32, 1.004, 0.996];
                let mut s = 0.0;
                for k in 0..3 {
                    v.ph[k] = (v.ph[k] + f * d[k] * dt).fract_pos();
                    s += 2.0 * v.ph[k] - 1.0;
                }
                let env = adsr(v.t, v.gate, v.rel_t, 0.25, 0.4, 0.8, 0.5);
                v.lp += (s / 3.0 - v.lp) * 0.035;
                v.lp2 += (v.lp - v.lp2) * 0.05;
                out = v.lp2 * env * 1.4;
                if !v.gate && v.rel_t > 0.5 {
                    v.active = false;
                }
            }
            4 => {
                // FM bell: a 3.5x modulator whose depth rings out
                let dec = libm::expf(-v.t * 2.4);
                v.ph[1] = (v.ph[1] + f * 3.5 * dt).fract_pos();
                let m = libm::sinf(v.ph[1] * core::f32::consts::TAU) * 2.2 * dec;
                v.ph[0] = (v.ph[0] + f * dt).fract_pos();
                out = libm::sinf(v.ph[0] * core::f32::consts::TAU + m) * dec * 0.8;
                if v.t > 3.0 || (!v.gate && v.rel_t > 0.6) {
                    v.active = false;
                }
            }
            5 => {
                // kick: a sine falling from 160 Hz to 45 Hz, with a click
                let fr = 45.0 + 115.0 * libm::expf(-v.t * 45.0);
                v.ph[0] = (v.ph[0] + fr * dt).fract_pos();
                let env = libm::expf(-v.t * 9.0);
                out = libm::sinf(v.ph[0] * core::f32::consts::TAU) * env * 1.1 + noise * libm::expf(-v.t * 400.0) * 0.4;
                if v.t > 0.45 {
                    v.active = false;
                }
            }
            6 => {
                // snare: band-passed noise over a 190 Hz body
                v.ph[0] = (v.ph[0] + 190.0 * dt).fract_pos();
                let body = libm::sinf(v.ph[0] * core::f32::consts::TAU) * libm::expf(-v.t * 28.0);
                v.lp += (noise - v.lp) * 0.6;
                let hp = noise - v.lp; // high-passed noise
                out = (hp * libm::expf(-v.t * 20.0) * 0.9 + body * 0.5) * 0.9;
                if v.t > 0.3 {
                    v.active = false;
                }
            }
            8 => {
                // supersaw: five detuned saws, softened
                let d = [-0.012f32, -0.006, 0.0, 0.006, 0.012];
                let mut s = 0.0;
                for k in 0..5 {
                    v.ph[k] = (v.ph[k] + f * (1.0 + d[k]) * dt).fract_pos();
                    s += 2.0 * v.ph[k] - 1.0;
                }
                let env = adsr(v.t, v.gate, v.rel_t, 0.01, 0.2, 0.7, 0.25);
                v.lp += (s * 0.2 - v.lp) * 0.3;
                out = v.lp * env * 0.9;
                if !v.gate && v.rel_t > 0.25 {
                    v.active = false;
                }
            }
            9 => {
                // growl: a saw and an octave-down square through a wobbling low-pass, then driven
                v.ph[0] = (v.ph[0] + f * dt).fract_pos();
                v.ph[1] = (v.ph[1] + f * 0.5 * dt).fract_pos();
                v.ph[4] = (v.ph[4] + 3.0 * dt).fract_pos();
                let lfo = 0.5 + 0.5 * libm::sinf(v.ph[4] * core::f32::consts::TAU);
                let x = (2.0 * v.ph[0] - 1.0) * 0.6 + if v.ph[1] < 0.5 { 0.5 } else { -0.5 };
                let cut = 0.025 + 0.3 * lfo;
                v.lp += (x - v.lp) * cut;
                v.lp2 += (v.lp - v.lp2) * cut;
                let env = adsr(v.t, v.gate, v.rel_t, 0.004, 0.1, 0.85, 0.08);
                out = libm::tanhf(v.lp2 * 4.0) * env * 0.7;
                if !v.gate && v.rel_t > 0.08 {
                    v.active = false;
                }
            }
            10 => {
                // voice: a vibrato saw through two vowel formants (an "ah")
                let vib = libm::sinf(v.t * 5.2 * core::f32::consts::TAU) * 0.004 * (v.t * 4.0).min(1.0);
                v.ph[0] = (v.ph[0] + f * (1.0 + vib) * dt).fract_pos();
                let x = 2.0 * v.ph[0] - 1.0;
                let mut y = 0.0;
                for (i, fc) in [700.0f32, 1150.0].iter().enumerate() {
                    let g = 2.0 * libm::sinf(core::f32::consts::PI * fc / sr);
                    let (lo, bd) = (v.sv[i * 2], v.sv[i * 2 + 1]);
                    let hi = x - lo - 0.18 * bd;
                    let bd = bd + g * hi;
                    v.sv[i * 2] = lo + g * bd;
                    v.sv[i * 2 + 1] = bd;
                    y += bd * if i == 0 { 1.0 } else { 0.6 };
                }
                let env = adsr(v.t, v.gate, v.rel_t, 0.06, 0.1, 0.85, 0.15);
                out = y * env * 0.55;
                if !v.gate && v.rel_t > 0.15 {
                    v.active = false;
                }
            }
            11 => {
                // organ: three drawbars
                let mut s = 0.0;
                for (k, (m, a)) in [(1.0f32, 1.0f32), (2.0, 0.5), (3.0, 0.25)].iter().enumerate() {
                    v.ph[k] = (v.ph[k] + f * m * dt).fract_pos();
                    s += libm::sinf(v.ph[k] * core::f32::consts::TAU) * a;
                }
                let env = adsr(v.t, v.gate, v.rel_t, 0.008, 0.05, 0.9, 0.05);
                out = s * env * 0.45;
                if !v.gate && v.rel_t > 0.05 {
                    v.active = false;
                }
            }
            12 => {
                // clap: three quick noise bursts and a tail through a band-pass
                let burst = if v.t < 0.033 { libm::expf(-(v.t % 0.011) * 300.0) } else { libm::expf(-(v.t - 0.033) * 25.0) };
                v.lp += (noise - v.lp) * 0.35;
                let hp = noise - v.lp;
                v.lp2 += (hp - v.lp2) * 0.6;
                out = v.lp2 * burst * 1.3;
                if v.t > 0.3 {
                    v.active = false;
                }
            }
            13 => {
                // tom: a sine dropping onto the pitch asked for
                let fr = f * (1.0 + 1.2 * libm::expf(-v.t * 35.0));
                v.ph[0] = (v.ph[0] + fr * dt).fract_pos();
                out = libm::sinf(v.ph[0] * core::f32::consts::TAU) * libm::expf(-v.t * 7.0) * 1.0 + noise * libm::expf(-v.t * 300.0) * 0.2;
                if v.t > 0.6 {
                    v.active = false;
                }
            }
            _ => {
                // hat: high-passed noise; the key chooses closed or open
                let open = v.key % 12 == 2 || v.key % 12 == 10;
                v.lp += (noise - v.lp) * 0.8;
                let hp = noise - v.lp;
                let decay = if open { 9.0 } else { 70.0 };
                out = hp * libm::expf(-v.t * decay) * 0.55;
                if v.t > if open { 0.6 } else { 0.12 } {
                    v.active = false;
                }
            }
        }
        out * v.vel
    }

    /// Render the song once (plus a tail) to a 16-bit stereo 48 kHz WAV file image.
    pub fn render_wav(song: &Song) -> Vec<u8> {
        let mut e = Engine::new(song.clone());
        e.looping = false;
        e.play();
        let frames = ((song.seconds() + song.tail_seconds()) * SAMPLE_RATE as f32) as usize;
        let mut pcm: Vec<i16> = vec![0; frames * 2];
        let mut done = 0;
        while done < frames {
            let n = (frames - done).min(2048);
            e.render(&mut pcm[done * 2..(done + n) * 2]);
            done += n;
        }
        let data_len = (frames * 4) as u32;
        let mut w = Vec::with_capacity(44 + data_len as usize);
        w.extend_from_slice(b"RIFF");
        w.extend_from_slice(&(36 + data_len).to_le_bytes());
        w.extend_from_slice(b"WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&1u16.to_le_bytes());
        w.extend_from_slice(&2u16.to_le_bytes());
        w.extend_from_slice(&SAMPLE_RATE.to_le_bytes());
        w.extend_from_slice(&(SAMPLE_RATE * 4).to_le_bytes());
        w.extend_from_slice(&4u16.to_le_bytes());
        w.extend_from_slice(&16u16.to_le_bytes());
        w.extend_from_slice(b"data");
        w.extend_from_slice(&data_len.to_le_bytes());
        for s in pcm {
            w.extend_from_slice(&s.to_le_bytes());
        }
        w
    }
}

trait FractPos {
    fn fract_pos(self) -> f32;
}
impl FractPos for f32 {
    #[inline]
    fn fract_pos(self) -> f32 {
        self - libm::floorf(self)
    }
}

#[inline]
fn soft(x: f32) -> f32 {
    // x / (1 + |x|)-style limiter, gentle below 0.5
    let a = x.abs();
    if a < 0.5 {
        x
    } else {
        let s = if x < 0.0 { -1.0 } else { 1.0 };
        s * (0.5 + (a - 0.5) / (1.0 + (a - 0.5) * 2.0))
    }
}

#[inline]
fn adsr(t: f32, gate: bool, rel_t: f32, a: f32, d: f32, s: f32, r: f32) -> f32 {
    let on = if t < a {
        t / a
    } else if t < a + d {
        1.0 - (1.0 - s) * ((t - a) / d)
    } else {
        s
    };
    if gate {
        on
    } else {
        on * (1.0 - (rel_t / r).min(1.0))
    }
}

#[cfg(test)]
extern crate std;

#[cfg(test)]
mod tests {
    use super::*;

    fn render_secs(e: &mut Engine, secs: f32) -> Vec<i16> {
        let n = (secs * SAMPLE_RATE as f32) as usize;
        let mut out = vec![0i16; n * 2];
        let mut p = 0;
        while p < n {
            let b = (n - p).min(1024);
            e.render(&mut out[p * 2..(p + b) * 2]);
            p += b;
        }
        out
    }

    fn mono(v: &[i16]) -> Vec<f32> {
        v.chunks(2).map(|c| c[0] as f32 / 32768.0).collect()
    }

    fn f0(x: &[f32]) -> f32 {
        // the strongest repeat of the waveform between 60 Hz and 1 kHz (autocorrelation)
        let (lo, hi) = (SAMPLE_RATE as usize / 1000, SAMPLE_RATE as usize / 60);
        let mut best = (0.0f32, lo);
        for lag in lo..hi.min(x.len() / 2) {
            let mut c = 0.0f32;
            let mut e = 1e-9f32;
            for i in 0..x.len() - lag {
                c += x[i] * x[i + lag];
                e += x[i] * x[i];
            }
            let r = c / e;
            if r > best.0 {
                best = (r, lag);
            }
        }
        SAMPLE_RATE as f32 / best.1 as f32
    }

    #[test]
    fn every_instrument_plays_the_pitch_asked_for() {
        // instruments with a steady pitch: saw lead, bass, pluck, pad, bell (the drums have none)
        for inst in 0..5usize {
            let mut s = Song::empty();
            s.chans[0].inst = inst as u8;
            s.chans[0].pan = 0;
            s.chans[0].vol = 100;
            let mut e = Engine::new(s);
            e.trigger(0, 57, 15); // A3 = 220 Hz
            let out = render_secs(&mut e, 0.6);
            let m = mono(&out);
            let seg = &m[(0.1 * SAMPLE_RATE as f32) as usize..(0.5 * SAMPLE_RATE as f32) as usize];
            assert!(seg.iter().any(|x| x.abs() > 0.02), "instrument {inst} makes sound");
            let fr = f0(seg);
            // the bass has a sub at half the pitch: accept 110 or 220; a bell's partials move it: accept a wide band
            let ok = if inst == 4 { fr > 100.0 && fr < 900.0 } else { (fr - 220.0).abs() < 12.0 || (fr - 110.0).abs() < 8.0 };
            assert!(ok, "instrument {inst} f0 {fr}");
        }
    }

    #[test]
    fn the_sequencer_keeps_time() {
        let mut s = Song::empty();
        s.bpm = 120;
        s.rpb = 4; // a row is 125 ms
        s.length = 8;
        s.cells[0][5] = Cell { note: 36, vel: 15 };
        s.cells[4][5] = Cell { note: 36, vel: 15 };
        let mut e = Engine::new(s);
        e.looping = false;
        e.play();
        let out = render_secs(&mut e, 1.2);
        let m = mono(&out);
        // the two kicks begin at 0 s and 0.5 s: energy right after each, near silence just before the second
        let win = |a: f32, b: f32| m[(a * 48000.0) as usize..(b * 48000.0) as usize].iter().map(|x| x * x).sum::<f32>();
        assert!(win(0.0, 0.05) > 0.5);
        assert!(win(0.5, 0.55) > 0.5, "the second kick lands on row 4");
        assert!(win(0.495, 0.4999) < win(0.5, 0.505) * 0.2, "silence-ish just before the downbeat");
        assert!(e.rows_played >= 8, "rows {}", e.rows_played);
        assert!(!e.playing, "a non-looping song stops at its end: {}", e.row);
    }

    #[test]
    fn the_demo_is_musical_and_clean() {
        let mut e = Engine::new(Song::demo());
        e.play();
        let out = render_secs(&mut e, 4.0);
        let peak = out.iter().map(|x| x.unsigned_abs()).max().unwrap();
        assert!(peak > 6000, "audible: {peak}");
        assert!(peak <= 32000, "never clips: {peak}");
        assert!(out.iter().all(|x| (*x as i32).abs() <= 32000));
        // loud and quiet moments: the groove has dynamics
        let rms = |a: usize, b: usize| (out[a * 2..b * 2].iter().map(|x| (*x as f32).powi(2)).sum::<f32>() / ((b - a) * 2) as f32).sqrt();
        assert!(rms(0, 24000) > 800.0);
    }

    #[test]
    fn songs_round_trip_and_reject_junk() {
        let s = Song::demo();
        let b = s.to_bytes();
        let t = Song::from_bytes(&b).expect("parses");
        assert_eq!(t.bpm, s.bpm);
        assert_eq!(t.length, s.length);
        assert_eq!(t.cells[0][5], s.cells[0][5]);
        assert_eq!(t.chans[0].mute, true);
        assert!(Song::from_bytes(&b[..20]).is_none());
        let mut bad = b.clone();
        bad[0] = b'X';
        assert!(Song::from_bytes(&bad).is_none());
        // hostile bytes become safe values
        let mut wild = b.clone();
        for x in wild[8..].iter_mut() {
            *x = 200;
        }
        let w = Song::from_bytes(&wild).unwrap();
        assert!(w.chans[0].inst < 14 && w.chans[0].vol <= 100 && w.fx.iter().all(|x| *x <= 10));
        let mut e = Engine::new(w);
        e.play();
        let _ = render_secs(&mut e, 0.3);
    }

    #[test]
    fn wav_export_is_a_valid_wav_of_the_right_length() {
        let s = Song::demo();
        let w = Engine::render_wav(&s);
        assert_eq!(&w[0..4], b"RIFF");
        assert_eq!(&w[8..16], b"WAVEfmt ");
        let data_len = u32::from_le_bytes([w[40], w[41], w[42], w[43]]) as usize;
        assert_eq!(w.len(), 44 + data_len);
        let secs = data_len as f32 / (SAMPLE_RATE as f32 * 4.0);
        assert!((secs - (s.seconds() + s.tail_seconds())).abs() < 0.05, "{secs} vs {}", s.seconds());
    }

    #[test]
    fn polyphony_and_stealing_do_not_blow_up() {
        let mut e = Engine::new(Song::empty());
        for k in 30..90u8 {
            e.trigger((k % 8) as usize, k, 15);
        }
        let out = render_secs(&mut e, 0.5);
        assert!(out.iter().all(|x| (*x as i32).abs() <= 32000));
    }

    #[test]
    fn cell_text_reads_like_a_tracker() {
        let (n, v) = Song::cell_text(Cell { note: 60, vel: 9 });
        assert_eq!(&n, b"C-4");
        assert_eq!(v, b'9');
        let (n, _) = Song::cell_text(Cell { note: NOTE_OFF, vel: 0 });
        assert_eq!(&n, b"===");
        let (n, v) = Song::cell_text(Cell::EMPTY);
        assert_eq!((&n, v), (b"...", b' '));
    }
}
