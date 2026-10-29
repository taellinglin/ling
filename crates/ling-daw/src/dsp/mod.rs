//! lsx-dsp — instruments and effects for LingStationX.
pub mod drums;
pub mod guitar;
pub mod insert;
pub mod pluck;
pub mod scriptdsp;
pub mod vocoder;
pub mod articulator;
pub mod bassfx;
pub mod edmfx;
pub mod chip;
pub mod cinefx;
pub mod cinema;
pub mod fx;
pub mod vox;
pub mod hypergrowl;
pub mod growl;
pub mod prism;
pub mod sampler;
pub mod sf2;
pub mod synth;
pub mod voice;
pub mod wavetable;

pub const TAU: f32 = std::f32::consts::TAU;

/// Anything that renders stereo audio and takes MIDI-ish note events.
pub trait Instrument: Send {
    fn note_on(&mut self, key: u8, vel: f32);
    /// Note with per-note pan (-1..1) and cutoff/reso modifiers (0..1, 0.5 neutral).
    fn note_on_ex(&mut self, key: u8, vel: f32, _pan: f32, _cutoff: f32, _reso: f32) {
        self.note_on(key, vel);
    }
    /// Note with its lyric (`~` tie marker + IPA syllables joined by `-`, see lyrics.rs) and its length in samples.
    /// Only sung instruments use the lyric; everything else plays the note.
    fn note_on_lyric(&mut self, key: u8, vel: f32, pan: f32, cutoff: f32, reso: f32, _lyric: &str, _len_samples: f32) {
        self.note_on_ex(key, vel, pan, cutoff, reso);
    }
    fn note_off(&mut self, key: u8);
    /// Per-note pitch gesture: the voice(s) sounding `key` play `semis` semitones away from it (0 = in tune). The engine calls
    /// it once per render block while a note with a pitch gesture (scoop, slide, dive, 808 glide) sounds; the instrument
    /// smooths it inside the block. Instruments without it keep the note's pitch.
    fn note_pitch(&mut self, _key: u8, _semis: f32) {}
    /// Per-note pressure (polyphonic aftertouch) 0..1 for the voice(s) sounding `key`, 0 = none; sent like `note_pitch` while a
    /// note with a pressure curve sounds. Instruments map it to loudness / brightness.
    fn note_pressure(&mut self, _key: u8, _v: f32) {}
    /// Per-note timbre (MPE slide / Y) 0..1 for the voice(s) sounding `key`, 0.5 = neutral; sent like `note_pitch`. Instruments
    /// map it to brightness (their filter cutoff) or a tone control.
    fn note_timbre(&mut self, _key: u8, _v: f32) {}
    fn all_off(&mut self);
    /// The project tempo, handed over before every render: instruments with a tempo-synced part (the Ling Voice arpeggiator) follow it.
    fn set_tempo(&mut self, _bpm: f64) {}
    /// The track slept through `frames` samples (it was silent, so it was not rendered): free-running clocks (tempo-synced
    /// modulation lanes) move on by that much, so the instrument wakes where it would have been.
    fn idle(&mut self, _frames: usize) {}
    /// May the engine skip this instrument while its track is silent? (false: it has free-running state that idle() does not
    /// carry, so it always renders)
    fn can_sleep(&self) -> bool {
        true
    }
    /// Render and ADD into `out` (interleaved stereo).
    fn render(&mut self, out: &mut [f32]);
    /// Named parameters, 0..1 normalised.
    fn params(&self) -> Vec<(&'static str, f32)>;
    fn set_param(&mut self, name: &str, v: f32);
    fn name(&self) -> &'static str;
    /// Sub-instruments the params apply to (e.g. drum pieces). Empty = none.
    fn parts(&self) -> Vec<&'static str> {
        vec![]
    }
    fn part(&self) -> usize {
        0
    }
    fn select_part(&mut self, _i: usize) {}
    fn presets(&self) -> &'static [&'static str] {
        &[]
    }
    /// File the instrument was built from (soundfont path), empty for built-in instruments.
    fn source_path(&self) -> String {
        String::new()
    }
    /// Dynamic preset list (soundfont presets) and the selected index.
    fn preset_names(&self) -> Vec<String> {
        vec![]
    }
    fn preset_index(&self) -> usize {
        0
    }
    fn select_preset_index(&mut self, _i: usize) {}
    /// Choose a sound by MIDI bank / program; false when the instrument has no such sound.
    fn midi_select(&mut self, _bank: u16, _program: u16, _drums: bool) -> bool {
        false
    }
    fn load_preset(&mut self, _name: &str) {}
    /// What a built-in preset sounds like and is for, in a sentence (shown in the preset lists, read by the generators); "" = none.
    fn preset_info(&self, _name: &str) -> String {
        String::new()
    }
    /// The category of each built-in preset: (preset name, category), empty when the instrument keeps its presets in one list.
    fn preset_categories(&self) -> Vec<(String, String)> {
        vec![]
    }
    /// Instrument-specific edit commands (the Sampler's zones): `cmd` with numeric `args` and some `text`; the reply is a list of numbers.
    fn command(&mut self, _cmd: &str, _args: &[f64], _text: &str) -> Vec<f64> {
        vec![]
    }
    /// Instrument-specific text: "names", "files", "config", "error" for the Sampler.
    fn info(&self, _what: &str) -> String {
        String::new()
    }
    /// One parameter by name (0..1), `None` when the instrument has no such parameter.
    fn get_param(&self, name: &str) -> Option<f32> {
        self.params().into_iter().find(|(n, _)| *n == name).map(|(_, v)| v)
    }
    /// Curves for the panel: "wave" (one cycle of the oscillator), "filter" (response, 0..1 = -24..+24 dB). `n` points.
    fn plot(&self, _what: &str, _n: usize) -> Vec<f32> {
        vec![]
    }
    /// Full serialisable state (name, value); drums include every part.
    fn state(&self) -> Vec<(String, f32)> {
        self.params().into_iter().map(|(n, v)| (n.to_string(), v)).collect()
    }
    fn load_state(&mut self, st: &[(String, f32)]) {
        for (n, v) in st {
            self.set_param(n, *v);
        }
    }
    /// A new, random but musical sound (same seed, same sound); false when the instrument cannot do that.
    fn randomize(&mut self, _seed: u32) -> bool {
        false
    }
}

/// A per-voice note-expression value (pitch gesture, pressure, timbre) that moves to its target linearly over one render block,
/// so a value the engine sends once a block has no zipper noise. A voice that has just started jumps to the first value.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Smooth {
    pub cur: f32,
    pub tgt: f32,
    step: f32,
}
impl Smooth {
    pub const fn at(v: f32) -> Smooth {
        Smooth { cur: v, tgt: v, step: 0.0 }
    }
    /// New target; `jump` (a fresh voice) sets it at once.
    pub fn set(&mut self, v: f32, jump: bool) {
        self.tgt = v;
        if jump {
            self.cur = v;
            self.step = 0.0;
        }
    }
    /// Back to rest at once (a new note on the voice).
    pub fn reset(&mut self, v: f32) {
        *self = Smooth::at(v);
    }
    /// Before a block of `frames` samples: the per-sample step towards the target.
    #[inline]
    pub fn begin(&mut self, frames: usize) {
        self.step = if self.cur == self.tgt { 0.0 } else { (self.tgt - self.cur) / frames.max(1) as f32 };
    }
    /// One sample on.
    #[inline]
    pub fn tick(&mut self) -> f32 {
        if self.step != 0.0 {
            self.cur += self.step;
            if (self.step > 0.0 && self.cur >= self.tgt) || (self.step < 0.0 && self.cur <= self.tgt) {
                self.cur = self.tgt;
                self.step = 0.0;
            }
        }
        self.cur
    }
    /// At rest (and staying there)?
    #[inline]
    pub fn idle(&self, rest: f32) -> bool {
        self.cur == rest && self.tgt == rest
    }
}

#[inline]
pub fn midi_hz(k: f32) -> f32 {
    440.0 * 2f32.powf((k - 69.0) / 12.0)
}

/// Small xorshift RNG (no allocation, realtime safe).
#[derive(Clone)]
pub struct Rng(u32);
impl Rng {
    pub fn new(seed: u32) -> Self {
        Rng(seed.max(1))
    }
    #[inline]
    pub fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// Instrument factory: "synth" (Ling Synth X) or "drums" (Ling Membrane).
/// Like `make_instrument`, for kinds that need a file ("sf2" = SoundFont).
pub fn make_instrument_file(kind: &str, sr: f32, path: &str) -> Option<Box<dyn Instrument>> {
    match kind {
        "sf2" => match sf2::Sf2Player::new(sr, path) {
            Ok(p) => Some(Box::new(p)),
            Err(e) => {
                eprintln!("soundfont: {e}");
                None
            }
        },
        other => make_instrument(other, sr),
    }
}

pub fn make_instrument(kind: &str, sr: f32) -> Option<Box<dyn Instrument>> {
    match kind {
        "synth" => Some(Box::new(synth::LingSynth::new(sr))),
        "drums" => Some(Box::new(drums::DrumSynth::new(sr))),
        "growl" => Some(Box::new(growl::Growl::new(sr))),
        "voice" => Some(Box::new(voice::Voice::new(sr))),
        "prism" => Some(Box::new(prism::Prism::new(sr))),
        "sampler" => Some(Box::new(sampler::Sampler::new(sr))),
        "pluck" => Some(Box::new(pluck::Pluck::new(sr))),
        "chip" => Some(Box::new(chip::Chip::new(sr))),
        "cinema" => Some(Box::new(cinema::Cinema::new(sr))),
        "script" => Some(Box::new(scriptdsp::ScriptSynth::new(sr))),
        _ => None,
    }
}
