//! Ling Slicer: a loop, a vocal, a shout or any recording cut into slices and laid across the keyboard, one slice per key from the
//! root up (C3 by default), so the slices can be played, rearranged in the piano roll and tuned to the song.
//!
//! * Slicing ([`marks`]): **transients** - spectral-flux onsets (log-magnitude rise over 21 ms frames above a moving median; the
//!   sensitivity lowers the threshold, the minimum gap keeps hits apart), each one moved to the real attack (the steepest rise of the
//!   level within 10 ms) or to the zero crossing just before it; **beats** - a 1/4, 1/8 or 1/16 grid at the file's tempo (from its
//!   name, "amen_170bpm", else measured: medley/acapella.rs) anchored on its first hit; **syllables** - the vocal chop detector's
//!   syllables (chops.rs); **equal** - N equal slices; **manual** - markers added, moved and deleted by hand in the panel.
//! * Each slice has its gain, pan, pitch (semitones + cents), reverse, start / end trim, fade in / out, one-shot or gate, loop and a
//!   choke group (group 1 by default: a new slice cuts the one sounding, like an open hi-hat closed by the next). Globally: attack,
//!   release, a low-pass with resonance, the velocity curve, the level, and stretch-to-tempo (the whole file time-stretched by WSOLA
//!   from its own tempo to the song's, so the slices keep their groove at any tempo).
//! * Auto-tune: every slice's pitch is measured once (chops::chop_f0: YIN over the steady voiced middle, with a confidence); a slice
//!   without a steady pitch (a drum, a breath, noise) is never tuned. With tuning on, a note on a pitched slice is moved to the
//!   nearest tone of the key (`key`) or of the chord (`chords`) sounding at the beat the note starts; the harmony is a timeline of
//!   (span, key, chord) segments the DAW reads from a reference MIDI track or from the whole song (api/slicer_api.rs: the key
//!   sections and the chord at every change, as chops/tune.rs reads them), so chord changes and key changes are followed note by note.
//!   `amount` scales the move (0 .. 100 %). The engine sends the beat of every note ([`Instrument::note_beat`]).
//! * Pitch shifting: with `keep length` on, the shifted slices are rendered ahead (stretch.rs: WSOLA then resampling, so the slice
//!   keeps its length and timing; formants move with the pitch) for every (slice, shift) the track's clips play - the DAW asks for
//!   that plan when it plays, renders or the project changes, and it is saved with the project so a render or a reload has it too.
//!   Anything not prepared (live playing, a slice shorter than two WSOLA frames, `keep length` off) is resampled on the fly
//!   (varispeed: the slice gets shorter as it goes up).
//! * The file is kept by path (project `config`, packed into `<project>/samples/` on save like the Sampler's); the slices, their
//!   settings and the prepared plan are in the instrument state. A missing file leaves the slices in place and plays silence.
use super::bassfx::{g_of, Tsvf};
use super::{Instrument, Smooth};
use crate::chops::{self, F0};
use crate::model::Note;
use crate::theory::Key;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub const MODES: [&str; 5] = ["transients", "beats", "syllables", "equal", "manual"];
pub const GRIDS: [&str; 3] = ["1/4", "1/8", "1/16"];
pub const TUNES: [&str; 3] = ["off", "key", "chords"];
pub const MAX_SLICES: usize = 128;
const MAX_VOICES: usize = 32;

/// (name, default): every parameter 0..1
pub const PARAMS: [(&str, f32); 17] = [
    ("mode", 0.0),
    ("sens", 0.5),
    ("gap", 0.25),
    ("snap", 0.0),
    ("grid", 0.5),
    ("count", 14.0 / 62.0),
    ("root", 48.0 / 127.0),
    ("attack", 0.0),
    ("release", 0.15),
    ("cutoff", 1.0),
    ("reso", 0.1),
    ("velcurve", 0.5),
    ("level", 0.8),
    ("stretch", 0.0),
    ("tune", 0.0),
    ("amount", 1.0),
    ("keeplen", 1.0),
];
const NP: usize = PARAMS.len();
const P_MODE: usize = 0;
const P_SENS: usize = 1;
const P_GAP: usize = 2;
const P_SNAP: usize = 3;
const P_GRID: usize = 4;
const P_COUNT: usize = 5;
const P_ROOT: usize = 6;
const P_ATK: usize = 7;
const P_REL: usize = 8;
const P_CUT: usize = 9;
const P_RES: usize = 10;
const P_VEL: usize = 11;
const P_LVL: usize = 12;
const P_STRETCH: usize = 13;
const P_TUNE: usize = 14;
const P_AMT: usize = 15;
const P_KEEP: usize = 16;

fn idx(name: &str) -> Option<usize> {
    PARAMS.iter().position(|p| p.0 == name)
}

fn sel_of(v: f32, n: usize) -> usize {
    ((v.clamp(0.0, 1.0) * (n - 1) as f32).round() as usize).min(n - 1)
}

pub fn gap_s(v: f32) -> f32 {
    0.01 + v * v * 0.49
}
pub fn attack_s(v: f32) -> f32 {
    v * v * 0.5
}
pub fn release_s(v: f32) -> f32 {
    0.005 + v * v * 2.0
}
pub fn cutoff_hz(v: f32) -> f32 {
    40.0 * 500f32.powf(v.clamp(0.0, 1.0))
}
pub fn count_of(v: f32) -> usize {
    2 + (v.clamp(0.0, 1.0) * 62.0).round() as usize
}

// ── the factory kits ────────────────────────────────────────────────────────────

/// A factory kit: (name, file under LingStationX/samples, slicing mode, tune mode, what it is, how to use it, parameters)
pub struct Kit {
    pub name: &'static str,
    pub file: &'static str,
    pub what: &'static str,
    pub how: &'static str,
    pub params: &'static [(&'static str, f32)],
}

pub const KITS: [Kit; 14] = [
    Kit { name: "Trap Yo Roll", file: "Shouts/MC/yo_roll_mc_1.wav", what: "A gritty hype MC rolling 'yo, yo, yo yo yo yo' - the trap ad-lib run, one 'yo' per key.", how: "Syllables mode: one 'yo' per key. Tuning off (it's a shout). Play the yo's on the 8th-note hats, roll the last two on 1/32s into the drop, or Stutter it for a fill.", params: &[("mode", 0.5)] },
    Kit { name: "Hype MC Shouts", file: "Shouts/MC/hands_up_explicit_mc_1.wav", what: "'Put your hands the f*ck up!' (explicit) from the gritty MC: the festival build-up call, word by word.", how: "Syllables mode so every word is a key. Tuning off. Fire the whole line on the last bar of a build, or just 'hands' + 'up' on the snare roll. Load hands_up_clean for the radio edit.", params: &[("mode", 0.5)] },
    Kit { name: "Hype MC Clean", file: "Shouts/MC/hands_up_clean_mc_1.wav", what: "The radio-safe 'Put your hands the heck up!' from the same MC.", how: "Syllables. Same moves as Hype MC Shouts, safe for streams and clean edits.", params: &[("mode", 0.5)] },
    Kit { name: "Festival MC", file: "Shouts/Female MC/make_some_noise_fem_1.wav", what: "A bright female festival MC: 'Make some noise!' cut into its words.", how: "Syllables, tuning off. Drop 'noise' on beat 1 of the drop, layer 'make some' over the last snare fill.", params: &[("mode", 0.5)] },
    Kit { name: "Deep Ad-Libs", file: "Shouts/Ad-Libs/yeah_adlib_1.wav", what: "A deep trap ad-lib voice ('yeah'): low and gravelly, for the gaps between bars.", how: "Transients. Tuning to chords at 60 % keeps it dark but in key. Answer the hook's last word on the 'and' of 4.", params: &[("mode", 0.0), ("tune", 1.0), ("amount", 0.6)] },
    Kit { name: "Raspy Countdown", file: "Shouts/Raspy/three_two_one_rasp_1.wav", what: "A raspy shouter counting 'Three, two, one!' into the drop.", how: "Syllables. One count per beat of the last bar before the drop; reverse the 'one' for a swell.", params: &[("mode", 0.5)] },
    Kit { name: "Demon Hype", file: "Shouts/Demon/put_your_hands_up_demon.wav", what: "The MC pitched an octave down and driven: a demonic 'put your hands up' for riddim and dubstep drops.", how: "Syllables, tuning off. Play it on the drop's first beat under the bass; choke group off lets the words overlap into a growl.", params: &[("mode", 0.5)] },
    Kit { name: "Amen-ish Break 170", file: "Breaks/break_amenish_170bpm.wav", what: "An original amen-style break (kick, snare, ghost notes, ride) at 170 bpm, rendered from Ling Membrane.", how: "Beats 1/16. Tuning off. To Piano Roll, then Shuffle for jungle edits; stretch-to-tempo keeps it locked at any tempo.", params: &[("mode", 0.25), ("grid", 1.0), ("stretch", 1.0)] },
    Kit { name: "Breakbeat 140", file: "Breaks/break_funky_140bpm.wav", what: "A funky original two-step break at 140 bpm for dubstep halftime and UK garage.", how: "Transients (each hit a key). Stutter the snares into fills; trim the ghost notes short.", params: &[("mode", 0.0), ("sens", 0.55), ("stretch", 1.0)] },
    Kit { name: "Boom Bap 90", file: "Breaks/break_boombap_90bpm.wav", what: "A dusty original boom-bap loop at 90 bpm.", how: "Beats 1/8. Reverse slice 4 for a lo-fi turnaround; release short for a chopped MPC feel.", params: &[("mode", 0.25), ("grid", 0.5), ("stretch", 1.0)] },
    Kit { name: "Vocal Phrase Chops", file: "Vocal Phrases/phrase_feel_narr_1.wav", what: "A breathy spoken phrase: 'Can you feel it? Can you feel the bass?' - pitched syllables to chop.", how: "Syllables with tuning on chords at 100 %: every chop lands on the chord under it. Play the syllables as a melody on the off-beats.", params: &[("mode", 0.5), ("tune", 1.0), ("amount", 1.0)] },
    Kit { name: "Sung Hook Chops", file: "Vocal Phrases/sung_hook_aria_128bpm.wav", what: "A sung original 'oh-ah-yeah' hook (Ling Voice, Aria) at 128 bpm: clean pitched vowels.", how: "Syllables, tuning to chords. The classic future-bass vocal chop: rearrange the vowels in the roll and they follow the chords.", params: &[("mode", 0.5), ("tune", 1.0)] },
    Kit { name: "Chord Stabs", file: "Stabs/stabs_prism_minor_chords.wav", what: "Eight Prism chord stabs in a row (a minor progression), one per key.", how: "Transients. Tune off (they're already chords). Play them on the off-beats for house, or Shuffle for a new progression.", params: &[("mode", 0.0), ("gap", 0.4), ("choke", 0.0)] },
    Kit { name: "FX Hits", file: "FX/fx_hits_cinema.wav", what: "Impacts, sub drops, risers' tails, a laser and a glitch from Ling Cinema, one per key.", how: "Equal, 6 slices (the hits sit 1.8 s apart). Tuning off. Impact on the drop, sub drop under it, laser on the last 16th.", params: &[("mode", 0.75), ("count", 4.0 / 62.0)] },
];

/// The LingStationX folder: `LSX_DIR`, the app's own folder (bin/ling-lsx.exe -> ..), the working folder, or the sibling checkout.
pub fn lsx_dir() -> std::path::PathBuf {
    if let Ok(d) = std::env::var("LSX_DIR") {
        return d.into();
    }
    let mut cands: Vec<std::path::PathBuf> = vec![];
    if let Ok(e) = std::env::current_exe() {
        if let Some(p) = e.parent() {
            cands.push(p.join(".."));
            cands.push(p.to_path_buf());
        }
    }
    if let Ok(c) = std::env::current_dir() {
        cands.push(c.clone());
        cands.push(c.join("../../../LingStationX"));
    }
    cands.into_iter().find(|d| d.join("samples").is_dir()).unwrap_or_else(|| "../../../LingStationX".into())
}

/// a kit's sample file as an absolute path
pub fn kit_path(k: &Kit) -> String {
    let p = lsx_dir().join("samples").join(k.file);
    p.canonicalize().unwrap_or(p).to_string_lossy().trim_start_matches(r"\\?\").replace('\\', "/")
}

/// a relative file name the app was given (a patch's `file = samples/...`) as a path that exists, when one does
pub fn resolve(path: &str) -> String {
    if path.is_empty() || std::path::Path::new(path).exists() {
        return path.to_string();
    }
    let j = lsx_dir().join(path);
    if j.exists() {
        return j.to_string_lossy().replace('\\', "/");
    }
    path.to_string()
}

// ── analysis ───────────────────────────────────────────────────────────────────

pub fn mono_of(stereo: &[f32]) -> Vec<f32> {
    stereo.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect()
}

/// the zero crossing nearest `at` (searching `radius` frames, the earlier one when two are as near); `at` itself when none
pub fn zero_cross(mono: &[f32], at: usize, radius: usize) -> usize {
    if mono.len() < 2 {
        return at;
    }
    let at = at.min(mono.len() - 1);
    for d in 0..=radius {
        for c in [at.checked_sub(d), Some(at + d)].into_iter().flatten() {
            if c >= 1 && c < mono.len() && (mono[c - 1] <= 0.0) != (mono[c] <= 0.0) {
                return c;
            }
        }
    }
    at
}

/// The attack of a hit near `guess`: the steepest rise of the peak level (quarter-millisecond blocks against the 2 ms before them)
/// within `+-win` frames, then the first sample of that block reaching half its peak.
pub fn attack_near(mono: &[f32], rate: u32, guess: usize, win: usize) -> usize {
    let q = (rate as usize / 4000).max(4);
    let a = guess.saturating_sub(win);
    let b = (guess + win).min(mono.len());
    if b <= a + q * 2 {
        return guess.min(mono.len().saturating_sub(1));
    }
    let lb = 8usize;
    let start = a.saturating_sub(lb * q);
    let nb = (b - start) / q;
    let mx: Vec<f32> = (0..nb).map(|j| mono[start + j * q..(start + (j + 1) * q).min(mono.len())].iter().fold(0.0f32, |m, x| m.max(x.abs()))).collect();
    let first = (a - start) / q;
    let mut best = (f32::MIN, first);
    for j in first.max(1)..nb {
        let prev = mx[j.saturating_sub(lb)..j].iter().copied().fold(0.0f32, f32::max);
        let r = mx[j] - prev;
        if r > best.0 {
            best = (r, j);
        }
    }
    let j = best.1;
    let s0 = start + j * q;
    let half = mx[j] * 0.5;
    (s0..(s0 + q).min(mono.len())).find(|i| mono[*i].abs() >= half).unwrap_or(s0)
}

/// The onsets of a recording by spectral flux: (frame) in time order. `sens` 0..1 (higher finds softer hits), `gap` the least
/// distance between two in seconds, `attack` true = at the real attack, false = at the zero crossing just before it.
pub fn onsets(stereo: &[f32], rate: u32, sens: f32, gap: f32, attack: bool) -> Vec<u32> {
    let mono = mono_of(stereo);
    let n = (rate as f32 * 0.021).max(64.0).log2().round().exp2() as usize;
    let hop = n / 4;
    if mono.len() < n {
        return if mono.iter().any(|x| x.abs() > 1e-4) { vec![0] } else { vec![] };
    }
    let win: Vec<f32> = (0..n).map(|i| 0.5 - 0.5 * (std::f32::consts::TAU * i as f32 / n as f32).cos()).collect();
    let frames = (mono.len() - n) / hop + 1;
    let mut prev = vec![0.0f32; n / 2];
    let mut flux = vec![0.0f32; frames];
    let mut level = vec![0.0f32; frames];
    let (mut re, mut im) = (vec![0.0f32; n], vec![0.0f32; n]);
    for k in 0..frames {
        let s = k * hop;
        for i in 0..n {
            re[i] = mono[s + i] * win[i];
            im[i] = 0.0;
        }
        level[k] = (re.iter().map(|x| x * x).sum::<f32>() / n as f32).sqrt();
        crate::dsp::wavetable::fft(&mut re, &mut im, false);
        let mut f = 0.0;
        for b in 1..n / 2 {
            let m = (1.0 + 100.0 * (re[b] * re[b] + im[b] * im[b]).sqrt()).ln();
            if k > 0 {
                f += (m - prev[b]).max(0.0);
            }
            prev[b] = m;
        }
        flux[k] = f;
    }
    // the first frame rises from silence: it is not measured against the rest
    let top = flux.iter().skip(1).copied().fold(0.0f32, f32::max);
    let lvl_top = level.iter().copied().fold(0.0f32, f32::max);
    if top <= 1e-6 || lvl_top <= 1e-5 {
        return vec![];
    }
    // with the rise of the frame's log level beside it: a pure tone starting is a small spectral change but a large rise in level
    let le: Vec<f32> = level.iter().map(|l| (1.0 + 300.0 * l / lvl_top).ln()).collect();
    let rise: Vec<f32> = (0..frames).map(|k| if k == 0 { 0.0 } else { (le[k] - le[k - 1]).max(0.0) }).collect();
    let rtop = rise.iter().copied().fold(1e-9f32, f32::max);
    for (k, f) in flux.iter_mut().enumerate() {
        *f = (*f / top).max(rise[k] / rtop);
    }
    let sens = sens.clamp(0.0, 1.0);
    let delta = 0.02 + (1.0 - sens) * 0.25;
    let mult = 1.0 + (1.0 - sens) * 0.8;
    let w = ((0.1 * rate as f32) / hop as f32).ceil() as usize;
    let gap_f = ((gap.max(0.005) * rate as f32) as usize).max(1);
    let mut out: Vec<u32> = vec![];
    let mut cand: Vec<(usize, f32)> = vec![];
    // a file that starts on a sound: that is a slice
    let head = mono[..n / 2].iter().fold(0.0f32, |m, x| m.max(x.abs()));
    if head > mono.iter().fold(0.0f32, |m, x| m.max(x.abs())) * 0.1 {
        cand.push((0, 1.0));
    }
    for k in 1..frames {
        let f = flux[k];
        if f < flux[k - 1] || (k + 1 < frames && f < flux[k + 1]) {
            continue;
        }
        let (a, b) = (k.saturating_sub(w), (k + w + 1).min(frames));
        let mut loc: Vec<f32> = flux[a..b].to_vec();
        loc.sort_by(f32::total_cmp);
        let med = loc[loc.len() / 2];
        if f < med * mult + delta {
            continue;
        }
        // there must be sound after it (not the tail of a hit fading into silence)
        let after = level[k..(k + 3).min(frames)].iter().copied().fold(0.0f32, f32::max);
        if after < lvl_top * 0.02 {
            continue;
        }
        cand.push((k, f));
    }
    // the first of every run closer than the gap wins, unless a much stronger one follows within it
    let mut last: Option<(usize, f32)> = None;
    for (k, f) in cand {
        let guess = k * hop + n / 2;
        let pos = attack_near(&mono, rate, guess, hop * 2);
        let pos = if attack { pos.saturating_sub(rate as usize / 2000) } else { zero_cross(&mono, pos, rate as usize / 500) };
        if let Some((lp, lf)) = last {
            if pos < lp + gap_f {
                if f > lf * 2.5 && pos > lp + gap_f / 4 {
                    out.pop();
                } else {
                    continue;
                }
            }
        }
        out.push(pos as u32);
        last = Some((pos, f));
    }
    out.dedup();
    out
}

/// a grid of `div` slices per beat at `bpm` across the file, anchored on the first hit (and back to the start)
pub fn grid_marks(stereo: &[f32], rate: u32, bpm: f64, div: usize) -> Vec<u32> {
    let mono = mono_of(stereo);
    let frames = mono.len();
    if frames == 0 || bpm <= 1.0 {
        return vec![];
    }
    let step = rate as f64 * 60.0 / bpm / div.max(1) as f64;
    let first = onsets(stereo, rate, 0.5, 0.05, true).first().copied().unwrap_or(0) as f64;
    let mut t = first;
    while t - step >= 0.0 {
        t -= step;
    }
    let mut out = vec![];
    while (t as usize) < frames {
        out.push(zero_cross(&mono, t.round() as usize, rate as usize / 1000) as u32);
        t += step;
    }
    if out.first().map(|x| *x as f64 > step * 0.5).unwrap_or(false) {
        out.insert(0, 0);
    }
    out
}

/// the syllables of a vocal (the chops detector), else its transients
pub fn syllable_marks(stereo: &[f32], rate: u32) -> Vec<u32> {
    let v = chops::Vocal::from_audio(stereo, rate, 120.0, 0.0, &[]);
    let mut m: Vec<u32> = v.pieces.iter().map(|p| p.chop.start).collect();
    if m.is_empty() {
        m = chops::transients(stereo, rate).into_iter().map(|t| t.0).collect();
    }
    m.sort_unstable();
    m.dedup();
    m
}

pub fn equal_marks(frames: usize, n: usize) -> Vec<u32> {
    let n = n.clamp(1, MAX_SLICES);
    (0..n).map(|i| (frames as f64 * i as f64 / n as f64) as u32).collect()
}

/// The tempo of a file: from its name, else measured (None when neither works)
pub fn file_bpm(path: &str, stereo: &[f32], rate: u32) -> Option<f64> {
    let stem = std::path::Path::new(path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
    crate::medley::acapella::bpm_from_name(&stem).or_else(|| crate::medley::acapella::detect_bpm(stereo, rate).filter(|b| b.1 > 0.05).map(|b| b.0))
}

// ── the harmony timeline ───────────────────────────────────────────────────────

/// A span of the song with its key (None = unknown) and the chord sounding (pitch classes, empty = none)
#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub start: f64,
    pub end: f64,
    pub key: Option<Key>,
    pub chord: Vec<u8>,
}

/// The harmony of `notes` (absolute beats) over time: the key sections (theory.rs, short ones merged) and, at every point a note
/// starts or ends, the chord sounding (chops::chord_at: held notes and the bass, a lone bass filled to its diatonic triad).
pub fn timeline(notes: &[Note], bar: f64) -> Vec<Seg> {
    if notes.is_empty() {
        return vec![];
    }
    let ns: Vec<crate::theory::N> = notes.iter().map(|n| crate::theory::N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect();
    let total = notes.iter().map(|n| n.start + n.len).fold(0.0, f64::max);
    let keys = crate::theory::merge_short_keys(crate::theory::key_sections(&ns, total, bar), bar * 2.0);
    let main = crate::theory::detect_key(&ns, 0.0, total).0;
    let mut cuts: Vec<f64> = notes.iter().flat_map(|n| [n.start, n.start + n.len]).chain(keys.iter().map(|k| k.start)).collect();
    cuts.push(0.0);
    cuts.sort_by(f64::total_cmp);
    cuts.dedup_by(|a, b| (*a - *b).abs() < 1e-6);
    let mut out: Vec<Seg> = vec![];
    for w in cuts.windows(2) {
        let (a, b) = (w[0], w[1]);
        if b - a < 1e-6 {
            continue;
        }
        let key = crate::theory::key_at(&keys, a + 1e-6).unwrap_or(main);
        let mut chord = chops::chord_at(key, notes, a + 1e-6).unwrap_or_default();
        chord.sort_unstable();
        match out.last_mut() {
            Some(l) if l.key == Some(key) && l.chord == chord && (l.end - a).abs() < 1e-6 => l.end = b,
            _ => out.push(Seg { start: a, end: b, key: Some(key), chord }),
        }
    }
    if let Some(l) = out.last_mut() {
        l.end = f64::MAX;
    }
    out
}

pub fn seg_at(tl: &[Seg], beat: f64) -> Option<&Seg> {
    tl.iter().find(|s| beat >= s.start - 1e-9 && beat < s.end)
}

/// how far (semitones) a slice heard at `heard` (MIDI, fractional) moves at `beat`: to the nearest scale tone of the key (`tune` 1)
/// or chord tone (`tune` 2, the key's scale where nothing sounds), times `amount`
pub fn tune_shift(tl: &[Seg], tune: usize, amount: f32, heard: f32, beat: f64) -> f32 {
    if tune == 0 || amount <= 0.0 {
        return 0.0;
    }
    let Some(s) = seg_at(tl, beat) else { return 0.0 };
    let tones: Vec<u8> = if tune == 2 && !s.chord.is_empty() {
        s.chord.clone()
    } else if let Some(k) = s.key {
        k.scale().to_vec()
    } else {
        return 0.0;
    };
    let t = chops::Tune { mode: chops::TuneMode::Chords, fine: 1.0, max_shift: 12.0 };
    let target = t.nearest(heard, &tones);
    (target as f32 - heard) * amount.clamp(0.0, 1.0)
}

// ── the slices ─────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Slice {
    /// frames of the source (unstretched)
    pub start: u32,
    pub end: u32,
    /// dB
    pub gain: f32,
    pub pan: f32,
    pub semis: f32,
    pub cents: f32,
    pub reverse: bool,
    /// the share of the slice cut off its start / its end
    pub trim_s: f32,
    pub trim_e: f32,
    /// ms
    pub fade_in: f32,
    pub fade_out: f32,
    /// true: stops on note-off; false: one-shot (plays to its end)
    pub gate: bool,
    pub looped: bool,
    /// 0 = none; a new slice in the same group cuts the ones sounding
    pub choke: u8,
    /// the measured pitch (not saved: measured again on load)
    pub f0: Option<F0>,
}

impl Slice {
    pub fn new(start: u32, end: u32) -> Slice {
        Slice { start, end, gain: 0.0, pan: 0.0, semis: 0.0, cents: 0.0, reverse: false, trim_s: 0.0, trim_e: 0.0, fade_in: 0.0, fade_out: 0.0, gate: false, looped: false, choke: 1, f0: None }
    }
    pub fn user_semis(&self) -> f32 {
        self.semis + self.cents / 100.0
    }
}

/// the editable fields, in the order of `daw_slc_set` / the "slices" listing
pub const FIELDS: [&str; 14] = ["start", "end", "gain", "pan", "semis", "cents", "reverse", "trim_s", "trim_e", "fade_in", "fade_out", "gate", "loop", "choke"];

fn field_get(s: &Slice, f: usize) -> f32 {
    match f {
        0 => s.start as f32,
        1 => s.end as f32,
        2 => s.gain,
        3 => s.pan,
        4 => s.semis,
        5 => s.cents,
        6 => s.reverse as u8 as f32,
        7 => s.trim_s,
        8 => s.trim_e,
        9 => s.fade_in,
        10 => s.fade_out,
        11 => s.gate as u8 as f32,
        12 => s.looped as u8 as f32,
        13 => s.choke as f32,
        _ => 0.0,
    }
}

fn field_set(s: &mut Slice, f: usize, v: f32, frames: u32) {
    if !v.is_finite() {
        return;
    }
    match f {
        0 => s.start = (v.max(0.0) as u32).min(frames.saturating_sub(1)),
        1 => s.end = (v.max(0.0) as u32).min(frames),
        2 => s.gain = v.clamp(-48.0, 12.0),
        3 => s.pan = v.clamp(-1.0, 1.0),
        4 => s.semis = v.round().clamp(-24.0, 24.0),
        5 => s.cents = v.clamp(-100.0, 100.0),
        6 => s.reverse = v > 0.5,
        7 => s.trim_s = v.clamp(0.0, 0.95),
        8 => s.trim_e = v.clamp(0.0, 0.95),
        9 => s.fade_in = v.clamp(0.0, 2000.0),
        10 => s.fade_out = v.clamp(0.0, 2000.0),
        11 => s.gate = v > 0.5,
        12 => s.looped = v > 0.5,
        13 => s.choke = v.round().clamp(0.0, 8.0) as u8,
        _ => {}
    }
    if s.end <= s.start {
        s.end = (s.start + 1).min(frames.max(s.start + 1));
    }
}

/// decoded files, the last few: an undo or a reload does not decode again
fn decode_cached(path: &str) -> Result<(Arc<Vec<f32>>, u32), String> {
    static C: OnceLock<Mutex<Vec<(String, u64, Arc<Vec<f32>>, u32)>>> = OnceLock::new();
    let c = C.get_or_init(|| Mutex::new(vec![]));
    let len = std::fs::metadata(path).map(|m| m.len()).map_err(|e| format!("{path}: {e}"))?;
    if let Some(x) = c.lock().unwrap_or_else(|p| p.into_inner()).iter().find(|x| x.0 == path && x.1 == len) {
        return Ok((x.2.clone(), x.3));
    }
    let (d, rate) = crate::dsp::sampler::load_audio(path)?;
    let d = Arc::new(d);
    let mut g = c.lock().unwrap_or_else(|p| p.into_inner());
    g.retain(|x| x.0 != path);
    g.push((path.to_string(), len, d.clone(), rate));
    if g.len() > 6 {
        g.remove(0);
    }
    Ok((d, rate))
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Stage {
    Attack,
    Hold,
    Release,
}

struct Voice {
    key: u8,
    buf: Arc<Vec<f32>>,
    /// playable span of `buf` (frames) and the position
    a: f64,
    b: f64,
    pos: f64,
    dir: f64,
    step: f64,
    bend: Smooth,
    stage: Stage,
    env: f32,
    rel_step: f32,
    gl: f32,
    gr: f32,
    fin: f64,
    fout: f64,
    gate: bool,
    looped: bool,
    held: bool,
    choke: u8,
    f: [Tsvf; 2],
    done: bool,
    /// a looped slice has come round once: no more fade-in at its start (a held vocal pad would buzz at the loop rate)
    wrapped: bool,
}

pub struct Slicer {
    sr: f32,
    v: [f32; NP],
    file: String,
    src: Option<(Arc<Vec<f32>>, u32)>,
    pub slices: Vec<Slice>,
    err: String,
    kit: String,
    /// the file's own tempo (name or measured), 0 = unknown
    src_bpm: f64,
    /// the song's tempo (set_tempo)
    bpm: f64,
    /// the source time-stretched to the song: (duration multiplier, audio)
    stretched: Option<(f64, Arc<Vec<f32>>)>,
    /// the harmony the auto-tune follows
    pub timeline: Vec<Seg>,
    /// the reference: 0 = the whole song, n = track n - 1
    pub reference: usize,
    /// (slice, cents) the clips play, rendered ahead with their length kept
    plan: Vec<(usize, i32)>,
    cache: HashMap<(usize, i32), Arc<Vec<f32>>>,
    /// the beat of the next note and where the clock was at the last one (beat, clock)
    next_beat: Option<f64>,
    last_beat: Option<(f64, u64)>,
    clock: u64,
    voices: Vec<Voice>,
    /// slices read from the state before the audio (a project being opened)
    pending: bool,
}

impl Slicer {
    pub fn new(sr: f32) -> Slicer {
        Slicer { sr, v: defaults(), file: String::new(), src: None, slices: vec![], err: String::new(), kit: String::new(), src_bpm: 0.0, bpm: 120.0, stretched: None, timeline: vec![], reference: 0, plan: vec![], cache: HashMap::new(), next_beat: None, last_beat: None, clock: 0, voices: vec![], pending: false }
    }

    /// A Slicer with a file loaded and sliced (`make_instrument_file("slicer", ...)`)
    pub fn with_file(sr: f32, path: &str) -> Slicer {
        let mut s = Slicer::new(sr);
        if !path.is_empty() {
            let _ = s.load_file(&resolve(path), false);
        }
        s
    }

    pub fn root(&self) -> u8 {
        (self.v[P_ROOT] * 127.0).round().clamp(0.0, 127.0) as u8
    }
    pub fn mode(&self) -> usize {
        sel_of(self.v[P_MODE], MODES.len())
    }
    pub fn tune_mode(&self) -> usize {
        sel_of(self.v[P_TUNE], TUNES.len())
    }
    pub fn frames(&self) -> usize {
        self.src.as_ref().map(|s| s.0.len() / 2).unwrap_or(0)
    }
    pub fn rate(&self) -> u32 {
        self.src.as_ref().map(|s| s.1).unwrap_or(48000)
    }
    pub fn file(&self) -> &str {
        &self.file
    }
    pub fn src_bpm(&self) -> f64 {
        self.src_bpm
    }

    /// A Slicer playing a kit of slices cut elsewhere (the vocal chops, chops/kit.rs): the slices may overlap, leave gaps and share a
    /// span (the same syllable at another pitch, reversed, or as a stutter), so they are kept as they are - the mode is manual and
    /// nothing re-slices them. `params` are set first (the root, the envelope, the tuning).
    pub fn with_slices(sr: f32, path: &str, slices: Vec<Slice>, params: &[(&str, f32)]) -> Slicer {
        let mut s = Slicer::new(sr);
        for (k, x) in params {
            if let Some(i) = idx(k) {
                s.v[i] = x.clamp(0.0, 1.0);
            }
        }
        s.v[P_MODE] = 1.0;
        s.slices = slices.into_iter().take(MAX_SLICES).collect();
        s.pending = true;
        if !path.is_empty() && s.load_file(&resolve(path), true).is_err() {
            s.file = path.replace('\\', "/");
        }
        s
    }

    /// the slices follow each other end to start (a file cut at markers), rather than being a kit of chops
    fn contiguous(&self) -> bool {
        self.slices.windows(2).all(|w| w[0].end == w[1].start)
    }

    /// Load an audio file; `keep` keeps the slices there are (a project being opened), else the file is sliced afresh.
    pub fn load_file(&mut self, path: &str, keep: bool) -> Result<(), String> {
        self.file = path.replace('\\', "/");
        self.voices.clear();
        self.cache.clear();
        self.stretched = None;
        match decode_cached(path) {
            Ok((d, rate)) => {
                self.src = Some((d.clone(), rate));
                self.err.clear();
                self.src_bpm = file_bpm(path, &d, rate).unwrap_or(0.0);
                let fr = (d.len() / 2) as u32;
                if keep && !self.slices.is_empty() {
                    for s in self.slices.iter_mut() {
                        s.start = s.start.min(fr.saturating_sub(1));
                        s.end = s.end.clamp(s.start + 1, fr.max(s.start + 1));
                    }
                    self.measure();
                } else {
                    self.reslice();
                }
                self.pending = false;
                self.prepare();
                Ok(())
            }
            Err(e) => {
                self.src = None;
                self.err = e.clone();
                Err(e)
            }
        }
    }

    /// Cut the file by the slicing mode (manual keeps the markers there are)
    pub fn reslice(&mut self) {
        let Some((d, rate)) = self.src.clone() else { return };
        let frames = d.len() / 2;
        let marks: Vec<u32> = match self.mode() {
            0 => onsets(&d, rate, self.v[P_SENS], gap_s(self.v[P_GAP]), self.v[P_SNAP] > 0.5),
            1 => {
                let bpm = if self.src_bpm > 1.0 { self.src_bpm } else { 120.0 };
                grid_marks(&d, rate, bpm, [1, 2, 4][sel_of(self.v[P_GRID], 3)])
            }
            2 => syllable_marks(&d, rate),
            3 => equal_marks(frames, count_of(self.v[P_COUNT])),
            _ => {
                if self.slices.is_empty() {
                    vec![0]
                } else {
                    return;
                }
            }
        };
        self.set_marks(marks);
    }

    /// slices from start markers: each runs to the next (the last to the end of the file); settings of slices that keep their start stay
    pub fn set_marks(&mut self, mut marks: Vec<u32>) {
        let frames = self.frames() as u32;
        marks.retain(|m| *m < frames);
        marks.sort_unstable();
        marks.dedup_by(|a, b| a.abs_diff(*b) < 8);
        marks.truncate(MAX_SLICES);
        if marks.is_empty() && frames > 0 {
            marks.push(0);
        }
        let old = std::mem::take(&mut self.slices);
        for (i, m) in marks.iter().enumerate() {
            let end = marks.get(i + 1).copied().unwrap_or(frames);
            let mut s = old.iter().find(|o| o.start == *m).cloned().unwrap_or_else(|| Slice::new(*m, end));
            s.start = *m;
            s.end = end;
            self.slices.push(s);
        }
        self.measure();
        self.cache.clear();
    }

    /// the pitch of every slice
    fn measure(&mut self) {
        let Some((d, rate)) = self.src.clone() else { return };
        let mono = mono_of(&d);
        for s in self.slices.iter_mut() {
            let (a, b) = (s.start as usize, (s.end as usize).min(mono.len()));
            s.f0 = if b > a { chops::chop_f0(&mono, a, b, rate) } else { None };
        }
    }

    /// the duration multiplier that puts the file at the song's tempo (1 when stretch is off or the tempo unknown)
    fn want_ratio(&self) -> f64 {
        if self.v[P_STRETCH] < 0.5 || self.src_bpm <= 1.0 || self.bpm <= 1.0 {
            return 1.0;
        }
        crate::medley::acapella::tempo_ratio(self.src_bpm, self.bpm).1
    }

    /// the audio the voices read and the ratio its frames have to the source's
    fn base(&self) -> Option<(Arc<Vec<f32>>, f64, u32)> {
        let (d, rate) = self.src.clone()?;
        match &self.stretched {
            Some((r, s)) if self.v[P_STRETCH] >= 0.5 => Some((s.clone(), *r, rate)),
            _ => Some((d, 1.0, rate)),
        }
    }

    /// The heard pitch of a slice before tuning (MIDI), None when it has none
    fn heard(&self, i: usize) -> Option<f32> {
        let s = self.slices.get(i)?;
        s.f0.map(|f| f.midi + s.user_semis())
    }

    /// semitones a note of slice `i` plays at: its own transposition and the auto-tune at `beat`
    pub fn shift_at(&self, i: usize, beat: Option<f64>) -> f32 {
        let Some(s) = self.slices.get(i) else { return 0.0 };
        let tune = match (self.heard(i), beat) {
            (Some(h), Some(b)) => tune_shift(&self.timeline, self.tune_mode(), self.v[P_AMT], h, b),
            _ => 0.0,
        };
        s.user_semis() + tune
    }

    /// Render ahead what the plan needs and the stretched source (outside the audio thread: the DAW calls it when it plays, renders,
    /// or the project changes; and on load)
    pub fn prepare(&mut self) {
        let Some((d, _rate)) = self.src.clone() else { return };
        let want = self.want_ratio();
        if (want - 1.0).abs() > 1e-4 {
            if self.stretched.as_ref().map(|s| (s.0 - want).abs() > 1e-4).unwrap_or(true) {
                self.stretched = Some((want, Arc::new(crate::stretch::time_stretch(&d, want))));
                self.cache.clear();
            }
        } else if self.stretched.is_some() {
            self.stretched = None;
            self.cache.clear();
        }
        if self.v[P_KEEP] < 0.5 {
            self.cache.clear();
            return;
        }
        let Some((buf, ratio, _)) = self.base() else { return };
        let plan = self.plan.clone();
        self.cache.retain(|k, _| plan.contains(k));
        for (i, c) in plan {
            if self.cache.contains_key(&(i, c)) || c == 0 {
                continue;
            }
            let Some(s) = self.slices.get(i) else { continue };
            let (a, b) = ((s.start as f64 * ratio) as usize, ((s.end as f64 * ratio) as usize).min(buf.len() / 2));
            if b <= a + 4096 * 2 {
                continue; // too short for WSOLA: resampled live
            }
            let shifted = crate::stretch::pitch_shift(&buf[a * 2..b * 2], c as f64 / 100.0);
            self.cache.insert((i, c), Arc::new(shifted));
        }
    }

    /// the (slice, cents) pairs that notes at (beat, key) will play
    pub fn plan_for(&self, notes: &[(f64, u8)]) -> Vec<(usize, i32)> {
        let root = self.root() as i32;
        let mut out: Vec<(usize, i32)> = vec![];
        for (b, k) in notes {
            let i = *k as i32 - root;
            if i < 0 || i as usize >= self.slices.len() {
                continue;
            }
            let c = (self.shift_at(i as usize, Some(*b)) * 100.0).round() as i32;
            if c != 0 && !out.contains(&(i as usize, c)) {
                out.push((i as usize, c));
            }
        }
        out.truncate(512);
        out
    }

    pub fn set_plan(&mut self, notes: &[(f64, u8)]) {
        self.plan = self.plan_for(notes);
    }

    /// The notes that play the file back as it is (`variant` 0), its slices shuffled (1) or with stutters (2): (start beat, length,
    /// key) at the song's tempo `bpm` (with stretch on: at the file's own tempo, which the stretch puts on the song's grid)
    pub fn roll(&self, bpm: f64, variant: usize, seed: u64) -> Vec<Note> {
        let rate = self.rate() as f64;
        let per_beat = if self.v[P_STRETCH] >= 0.5 && self.src_bpm > 1.0 { crate::medley::acapella::tempo_ratio(self.src_bpm, bpm).0 } else { bpm };
        let beat = |f: u32| f as f64 / rate * per_beat / 60.0;
        let root = self.root() as i32;
        let mut base: Vec<Note> = vec![];
        for (i, s) in self.slices.iter().enumerate() {
            let k = root + i as i32;
            if !(0..=127).contains(&k) {
                break;
            }
            let st = beat(s.start);
            let len = (beat(s.end) - st).max(1.0 / 32.0);
            base.push(Note::new(st, len, k as u8, 0.85));
        }
        let mut rng = crate::compose::Rng::new(seed ^ 0x51CE_D00D);
        match variant {
            1 if base.len() > 2 => {
                // the same rhythm, the slices dealt again (the first keeps the downbeat)
                let mut keys: Vec<u8> = base[1..].iter().map(|n| n.key).collect();
                for i in (1..keys.len()).rev() {
                    let j = (rng.next() % (i as u64 + 1)) as usize;
                    keys.swap(i, j);
                }
                for (n, k) in base[1..].iter_mut().zip(keys) {
                    n.key = k;
                }
                base
            }
            2 => {
                // some slices repeated in 1/16 or 1/32 rolls, some answered by the slice before them
                let mut out = vec![];
                for (i, n) in base.iter().enumerate() {
                    let r = rng.next() % 100;
                    if r < 30 && n.len >= 0.25 {
                        let d = if rng.next() % 2 == 0 { 0.25 } else { 0.125 };
                        let reps = ((n.len / d).floor() as usize).clamp(2, 8);
                        for k in 0..reps {
                            out.push(Note { vel: (0.6 + 0.4 * (k as f32 + 1.0) / reps as f32).min(1.0), ..Note::new(n.start + k as f64 * d, d * 0.9, n.key, 0.8) });
                        }
                    } else if r < 45 && i > 0 {
                        out.push(Note { key: base[i - 1].key, ..n.clone() });
                    } else {
                        out.push(n.clone());
                    }
                }
                out
            }
            _ => base,
        }
    }

    /// A slice as a tracker sample (tracker_export.rs): mono at the file's rate, with what the DAW does to it baked in - the pitch
    /// `cents` (the same way it plays: WSOLA keeping the length where the slice is long enough and `keep length` is on, else
    /// resampled), the trims, reverse, fades, gain, level, attack and the low-pass. Its loop (the whole sample) when it loops.
    pub fn bake(&self, i: usize, cents: i32) -> Option<(Vec<f32>, u32, Option<(u32, u32)>)> {
        let s = self.slices.get(i)?;
        let (base, ratio, rate) = self.base()?;
        let n = base.len() / 2;
        let (a0, b0) = (((s.start as f64 * ratio) as usize).min(n), ((s.end as f64 * ratio) as usize).min(n));
        if b0 <= a0 + 2 {
            return None;
        }
        let mut st = base[a0 * 2..b0 * 2].to_vec();
        if cents != 0 {
            let semis = cents as f64 / 100.0;
            st = if self.v[P_KEEP] >= 0.5 && b0 - a0 > 4096 * 2 { crate::stretch::pitch_shift(&st, semis) } else { crate::stretch::resample_speed(&st, 2f64.powf(semis / 12.0)) };
        }
        let len = st.len() / 2;
        let (a, b) = ((len as f32 * s.trim_s) as usize, len - (len as f32 * s.trim_e) as usize);
        if b <= a + 2 {
            return None;
        }
        let mut m: Vec<f32> = st[a * 2..b * 2].chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
        if s.reverse {
            m.reverse();
        }
        let ms = rate as f32 / 1000.0;
        let (fin, fout) = (s.fade_in.max(1.5) * ms, s.fade_out.max(1.5) * ms);
        let atk = attack_s(self.v[P_ATK]) * rate as f32;
        let g = 10f32.powf(s.gain / 20.0) * (self.v[P_LVL] / 0.8).powi(2);
        let k = m.len();
        let filt = self.v[P_CUT] < 0.995;
        let (gc, kq) = (g_of(cutoff_hz(self.v[P_CUT]), rate as f32), 1.0 / (0.6 + self.v[P_RES] * 7.0));
        let mut f = Tsvf::default();
        for (j, x) in m.iter_mut().enumerate() {
            let mut e = g;
            if (j as f32) < fin {
                e *= j as f32 / fin;
            }
            if ((k - j) as f32) < fout && !s.looped {
                e *= (k - j) as f32 / fout;
            }
            if atk >= 1.0 && (j as f32) < atk {
                e *= j as f32 / atk;
            }
            let y = if filt { f.run(*x, gc, kq).0 } else { *x };
            *x = y * e;
        }
        Some((m, rate, s.looped.then_some((0, k as u32))))
    }

    fn start_voice(&mut self, key: u8, vel: f32, pan: f32) {
        let i = key as i32 - self.root() as i32;
        if i < 0 || i as usize >= self.slices.len() {
            return;
        }
        let i = i as usize;
        let Some((base, ratio, rate)) = self.base() else { return };
        let beat = self.next_beat.take().or_else(|| self.last_beat.map(|(b, c)| b + (self.clock - c) as f64 / self.sr as f64 * self.bpm / 60.0));
        let semis = self.shift_at(i, beat);
        let s = self.slices[i].clone();
        // a choke group: the slices of this group sounding are cut (a fast fade)
        if s.choke > 0 {
            for v in self.voices.iter_mut().filter(|v| v.choke == s.choke && v.stage != Stage::Release) {
                v.stage = Stage::Release;
                v.rel_step = 1.0 / (0.004 * self.sr);
            }
        }
        let cents = (semis * 100.0).round() as i32;
        let cached = if self.v[P_KEEP] >= 0.5 && cents != 0 { self.cache.get(&(i, cents)).cloned() } else { None };
        let (buf, a0, b0, step) = match cached {
            Some(c) => {
                let n = (c.len() / 2) as f64;
                (c, 0.0, n, rate as f64 / self.sr as f64)
            }
            None => {
                let n = (base.len() / 2) as f64;
                let a = (s.start as f64 * ratio).min(n);
                let b = (s.end as f64 * ratio).min(n);
                (base, a, b, rate as f64 / self.sr as f64 * 2f64.powf(semis as f64 / 12.0))
            }
        };
        let len = b0 - a0;
        let (a, b) = (a0 + len * s.trim_s as f64, b0 - len * s.trim_e as f64);
        if b <= a + 2.0 {
            return;
        }
        if self.voices.len() >= MAX_VOICES {
            self.voices.remove(0);
        }
        let p = (s.pan + pan).clamp(-1.0, 1.0);
        let ang = (p + 1.0) * std::f32::consts::FRAC_PI_4;
        let vc = self.v[P_VEL] * 2.0;
        let g = 10f32.powf(s.gain / 20.0) * vel.clamp(0.0, 1.0).powf(vc) * (self.v[P_LVL] / 0.8).powi(2);
        let ms = rate as f64 / 1000.0;
        // at least 1.5 ms in and out, so a cut never clicks
        let (fin, fout) = ((s.fade_in as f64).max(1.5) * ms, (s.fade_out as f64).max(1.5) * ms);
        let atk = attack_s(self.v[P_ATK]);
        self.voices.push(Voice {
            key,
            buf,
            a,
            b,
            pos: if s.reverse { b - 1.0 } else { a },
            dir: if s.reverse { -1.0 } else { 1.0 },
            step,
            bend: Smooth::at(0.0),
            stage: Stage::Attack,
            env: if atk < 0.001 { 1.0 } else { 0.0 },
            rel_step: 1.0 / (release_s(self.v[P_REL]) * self.sr),
            gl: ang.cos() * g,
            gr: ang.sin() * g,
            fin,
            fout,
            gate: s.gate || s.looped,
            looped: s.looped,
            held: true,
            choke: s.choke,
            f: [Tsvf::default(), Tsvf::default()],
            done: false,
            wrapped: false,
        });
    }

    fn state_entries(&self) -> Vec<(String, f32)> {
        let mut st: Vec<(String, f32)> = PARAMS.iter().enumerate().map(|(i, p)| (p.0.to_string(), self.v[i])).collect();
        st.push(("tune.ref".into(), self.reference as f32));
        st.push(("slices".into(), self.slices.len() as f32));
        for (i, s) in self.slices.iter().enumerate() {
            for (f, name) in FIELDS.iter().enumerate() {
                let x = field_get(s, f);
                let dflt = field_get(&Slice::new(s.start, s.end), f);
                if f < 2 || x != dflt {
                    st.push((format!("s{i}.{name}"), x));
                }
            }
        }
        for (i, (sl, c)) in self.plan.iter().enumerate() {
            st.push((format!("plan{i}"), (*sl * 10000) as f32 + (*c + 5000) as f32));
        }
        for (i, s) in self.timeline.iter().enumerate() {
            st.push((format!("tl{i}.s"), s.start as f32));
            st.push((format!("tl{i}.e"), if s.end > 1e9 { -1.0 } else { s.end as f32 }));
            st.push((format!("tl{i}.k"), s.key.map(|k| (k.tonic + if k.minor { 12 } else { 0 }) as f32).unwrap_or(-1.0)));
            st.push((format!("tl{i}.c"), s.chord.iter().fold(0u32, |m, p| m | (1 << p)) as f32));
        }
        st
    }

    /// the slices as numbers for the panel: per slice the FIELDS, then the pitch heard (MIDI, -1 = none), its confidence and the
    /// pitch it plays at now (with the auto-tune at the last beat, -1 = none)
    fn slice_list(&self) -> Vec<f64> {
        let mut v = vec![];
        let beat = self.last_beat.map(|b| b.0);
        for (i, s) in self.slices.iter().enumerate() {
            for f in 0..FIELDS.len() {
                v.push(field_get(s, f) as f64);
            }
            let h = self.heard(i);
            v.push(s.f0.map(|f| f.midi as f64).unwrap_or(-1.0));
            v.push(s.f0.map(|f| f.conf as f64).unwrap_or(0.0));
            v.push(h.map(|h| (h - s.user_semis() + self.shift_at(i, beat.or(Some(0.0)))) as f64).unwrap_or(-1.0));
        }
        v
    }

    fn peaks(&self, n: usize) -> Vec<f64> {
        let Some((d, _)) = self.src.as_ref() else { return vec![] };
        let fr = d.len() / 2;
        let mut v = Vec::with_capacity(n * 2);
        for i in 0..n {
            let (a, b) = (fr * i / n, (fr * (i + 1) / n).max(fr * i / n + 1).min(fr));
            let (mut lo, mut hi) = (0.0f32, 0.0f32);
            let stride = ((b - a) / 256).max(1);
            let mut f = a;
            while f < b {
                let m = (d[f * 2] + d[f * 2 + 1]) * 0.5;
                lo = lo.min(m);
                hi = hi.max(m);
                f += stride;
            }
            v.push(lo as f64);
            v.push(hi as f64);
        }
        v
    }
}

fn defaults() -> [f32; NP] {
    let mut v = [0.0; NP];
    for (i, p) in PARAMS.iter().enumerate() {
        v[i] = p.1;
    }
    v
}

impl Instrument for Slicer {
    fn note_on(&mut self, key: u8, vel: f32) {
        self.start_voice(key, vel, 0.0);
    }
    fn note_on_ex(&mut self, key: u8, vel: f32, pan: f32, _cutoff: f32, _reso: f32) {
        self.start_voice(key, vel, pan);
    }
    fn note_beat(&mut self, beat: f64) {
        self.next_beat = Some(beat);
        self.last_beat = Some((beat, self.clock));
    }
    fn note_off(&mut self, key: u8) {
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.held) {
            v.held = false;
            if v.gate && v.stage != Stage::Release {
                v.stage = Stage::Release;
            }
        }
    }
    fn note_pitch(&mut self, key: u8, semis: f32) {
        for v in self.voices.iter_mut().filter(|v| v.key == key && v.held) {
            v.bend.set(semis, false);
        }
    }
    fn all_off(&mut self) {
        self.voices.clear();
    }
    fn set_tempo(&mut self, bpm: f64) {
        if bpm > 1.0 {
            self.bpm = bpm;
        }
    }
    fn render(&mut self, out: &mut [f32]) {
        let frames = out.len() / 2;
        self.clock += frames as u64;
        if self.voices.is_empty() {
            return;
        }
        let filt = self.v[P_CUT] < 0.995;
        let g = g_of(cutoff_hz(self.v[P_CUT]), self.sr);
        let k = 1.0 / (0.6 + self.v[P_RES] * 7.0);
        let atk_step = 1.0 / (attack_s(self.v[P_ATK]) * self.sr).max(1.0);
        for v in self.voices.iter_mut() {
            v.bend.begin(frames);
            let n = v.buf.len() / 2;
            for f in 0..frames {
                let bend = v.bend.tick();
                // the envelope
                match v.stage {
                    Stage::Attack => {
                        v.env += atk_step;
                        if v.env >= 1.0 {
                            v.env = 1.0;
                            v.stage = Stage::Hold;
                        }
                    }
                    Stage::Hold => {}
                    Stage::Release => {
                        v.env -= v.rel_step;
                        if v.env <= 0.0 {
                            v.done = true;
                            break;
                        }
                    }
                }
                // the end of the slice: loop or stop
                if v.pos >= v.b || v.pos < v.a {
                    if v.looped && v.held {
                        v.pos = if v.dir > 0.0 { v.a + (v.pos - v.b).max(0.0) } else { v.b - 1.0 };
                        v.wrapped = true;
                    } else {
                        v.done = true;
                        break;
                    }
                }
                let i0 = v.pos as usize;
                let fr = (v.pos - i0 as f64) as f32;
                let i1 = (i0 + 1).min(n.saturating_sub(1));
                if i0 >= n {
                    v.done = true;
                    break;
                }
                let mut l = v.buf[i0 * 2] + (v.buf[i1 * 2] - v.buf[i0 * 2]) * fr;
                let mut r = v.buf[i0 * 2 + 1] + (v.buf[i1 * 2 + 1] - v.buf[i0 * 2 + 1]) * fr;
                // the fades: from where the slice starts playing and to where it ends (a looped slice fades only at its ends once)
                let from = if v.dir > 0.0 { v.pos - v.a } else { v.b - v.pos };
                let to = if v.dir > 0.0 { v.b - v.pos } else { v.pos - v.a };
                let mut fade = 1.0f32;
                if from < v.fin && !v.wrapped {
                    fade *= (from / v.fin) as f32;
                }
                if to < v.fout && !(v.looped && v.held) {
                    fade *= (to / v.fout) as f32;
                }
                if filt {
                    l = v.f[0].run(l, g, k).0;
                    r = v.f[1].run(r, g, k).0;
                }
                let e = v.env * fade;
                out[f * 2] += l * v.gl * e;
                out[f * 2 + 1] += r * v.gr * e;
                let st = if bend != 0.0 { v.step * 2f64.powf(bend as f64 / 12.0) } else { v.step };
                v.pos += st * v.dir;
            }
        }
        self.voices.retain(|v| !v.done);
    }
    fn can_sleep(&self) -> bool {
        // the beat clock between notes is kept by note_beat; nothing free-running
        true
    }
    fn params(&self) -> Vec<(&'static str, f32)> {
        PARAMS.iter().enumerate().map(|(i, p)| (p.0, self.v[i])).collect()
    }
    fn set_param(&mut self, name: &str, v: f32) {
        if let Some(i) = idx(name) {
            if !v.is_finite() {
                return;
            }
            let old = self.v[i];
            self.v[i] = v.clamp(0.0, 1.0);
            // a new slicing setting cuts the file again (not while a project's slices are waiting for their file)
            if matches!(i, P_MODE | P_SENS | P_GAP | P_SNAP | P_GRID | P_COUNT) && old != self.v[i] && !self.pending && self.src.is_some() && self.mode() != 4 {
                self.reslice();
            }
        }
    }
    fn name(&self) -> &'static str {
        "Ling Slicer"
    }
    fn presets(&self) -> &'static [&'static str] {
        static N: OnceLock<Vec<&'static str>> = OnceLock::new();
        N.get_or_init(|| KITS.iter().map(|k| k.name).collect())
    }
    fn preset_categories(&self) -> Vec<(String, String)> {
        KITS.iter()
            .map(|k| {
                let cat = k.file.split('/').next().unwrap_or("Kits");
                (k.name.to_string(), cat.to_string())
            })
            .collect()
    }
    fn load_preset(&mut self, name: &str) {
        let Some(k) = KITS.iter().find(|k| k.name.eq_ignore_ascii_case(name)) else { return };
        self.v = defaults();
        let mut choke = None;
        for (p, x) in k.params {
            if *p == "choke" {
                choke = Some(*x);
            } else if let Some(i) = idx(p) {
                self.v[i] = *x;
            }
        }
        self.slices.clear();
        self.pending = false;
        let _ = self.load_file(&kit_path(k), false);
        if let Some(c) = choke {
            for s in self.slices.iter_mut() {
                s.choke = c as u8;
            }
        }
        self.kit = k.name.to_string();
    }
    fn preset_info(&self, name: &str) -> String {
        KITS.iter().find(|k| k.name.eq_ignore_ascii_case(name)).map(|k| format!("{} How to use it: {}", k.what, k.how)).unwrap_or_default()
    }
    fn source_path(&self) -> String {
        self.file.clone()
    }
    fn command(&mut self, cmd: &str, args: &[f64], text: &str) -> Vec<f64> {
        let a = |i: usize| args.get(i).copied().unwrap_or(0.0);
        match cmd {
            // a file of your own (keeps nothing of the last one but the settings)
            "open" => {
                self.slices.clear();
                self.pending = false;
                self.kit.clear();
                match self.load_file(text, false) {
                    Ok(()) => vec![self.slices.len() as f64],
                    Err(_) => vec![-1.0],
                }
            }
            // the project's file: the slices from the state stay
            "load" => {
                let keep = self.pending || !self.slices.is_empty();
                match self.load_file(text, keep) {
                    Ok(()) => vec![1.0],
                    Err(_) => {
                        self.file = text.replace('\\', "/");
                        vec![0.0]
                    }
                }
            }
            "reslice" => {
                self.reslice();
                vec![self.slices.len() as f64]
            }
            "count" => vec![self.slices.len() as f64],
            "slices" => self.slice_list(),
            "peaks" => self.peaks((a(0) as usize).clamp(8, 4096)),
            "set" => {
                let (i, f) = (a(0) as usize, a(1) as usize);
                let fr = self.frames() as u32;
                let markers = self.contiguous();
                if let Some(s) = self.slices.get_mut(i) {
                    field_set(s, f, a(2) as f32, fr.max(1));
                    if f <= 1 && markers {
                        let marks: Vec<u32> = self.slices.iter().map(|s| s.start).collect();
                        self.set_marks(marks);
                    } else if f <= 1 {
                        // a kit (the vocal chops): only this slice's edge moves
                        self.measure();
                        self.cache.retain(|k, _| k.0 != i);
                    }
                    if f >= 4 {
                        self.cache.retain(|k, _| k.0 != i);
                    }
                    vec![1.0]
                } else {
                    vec![0.0]
                }
            }
            // the same value for every slice
            "set_all" => {
                let (f, x) = (a(0) as usize, a(1) as f32);
                let fr = self.frames() as u32;
                for s in self.slices.iter_mut() {
                    field_set(s, f, x, fr.max(1));
                }
                self.cache.clear();
                vec![1.0]
            }
            // manual markers: add at a frame, move slice i's start, delete slice i (its span joins the one before)
            "add" => {
                let mut m: Vec<u32> = self.slices.iter().map(|s| s.start).collect();
                if m.len() >= MAX_SLICES {
                    return vec![-1.0];
                }
                let at = a(0).max(0.0) as u32;
                let mono_at = self.src.as_ref().map(|(d, _)| zero_cross(&mono_of(d), at as usize, 64) as u32).unwrap_or(at);
                m.push(mono_at);
                self.v[P_MODE] = 1.0;
                self.set_marks(m);
                vec![self.slices.iter().position(|s| s.start == mono_at).map(|x| x as f64).unwrap_or(-1.0)]
            }
            "move" => {
                let i = a(0) as usize;
                let mut m: Vec<u32> = self.slices.iter().map(|s| s.start).collect();
                if i >= m.len() {
                    return vec![-1.0];
                }
                let lo = if i == 0 { 0 } else { m[i - 1] + 16 };
                let hi = m.get(i + 1).map(|x| x.saturating_sub(16)).unwrap_or(self.frames() as u32);
                let to = (a(1).max(0.0) as u32).clamp(lo, hi.max(lo));
                if let Some(s) = self.slices.get_mut(i) {
                    s.start = to;
                }
                m[i] = to;
                self.v[P_MODE] = 1.0;
                self.set_marks(m);
                vec![i as f64]
            }
            "del" => {
                let i = a(0) as usize;
                if i < self.slices.len() && self.slices.len() > 1 {
                    let m: Vec<u32> = self.slices.iter().enumerate().filter(|(k, _)| *k != i).map(|(_, s)| s.start).collect();
                    self.v[P_MODE] = 1.0;
                    self.set_marks(m);
                    vec![1.0]
                } else {
                    vec![0.0]
                }
            }
            // the harmony: [segments, then 5 numbers each: start, end (-1 = open), key (tonic + 12 when minor, -1), chord mask]
            "timeline" => {
                let n = a(0) as usize;
                self.timeline = (0..n)
                    .map(|i| {
                        let b = 1 + i * 4;
                        let k = a(b + 2);
                        Seg { start: a(b), end: if a(b + 1) < 0.0 { f64::MAX } else { a(b + 1) }, key: (k >= 0.0).then(|| Key::new((k as u8) % 12, k >= 12.0)), chord: (0..12u8).filter(|p| (a(b + 3) as u32) & (1 << p) != 0).collect() }
                    })
                    .collect();
                vec![self.timeline.len() as f64]
            }
            "reference" => {
                self.reference = a(0).max(0.0) as usize;
                vec![1.0]
            }
            // the notes the clips play [beat, key, ...] -> the plan, rendered ahead; also the song's tempo (args[0])
            "sync" => {
                if a(0) > 1.0 {
                    self.bpm = a(0);
                }
                let notes: Vec<(f64, u8)> = args.get(1..).unwrap_or(&[]).chunks_exact(2).map(|c| (c[0], c[1].clamp(0.0, 127.0) as u8)).collect();
                self.set_plan(&notes);
                self.prepare();
                vec![self.plan.len() as f64, self.cache.len() as f64]
            }
            // [bpm, variant, seed] -> [start, len, key, vel, ...]
            "roll" => self.roll(if a(0) > 1.0 { a(0) } else { self.bpm }, a(1) as usize, a(2) as u64).into_iter().flat_map(|n| [n.start, n.len, n.key as f64, n.vel as f64]).collect(),
            "bpm" => vec![self.src_bpm, self.frames() as f64, self.rate() as f64],
            // tracker export: notes [beat, key, ...] -> per note [slice (-1 = none), cents it plays at, gate, loop, choke group]
            "tracker" => {
                let root = self.root() as i32;
                let mut v = vec![];
                for c in args.chunks_exact(2) {
                    let i = c[1] as i32 - root;
                    match (i >= 0).then(|| self.slices.get(i as usize)).flatten() {
                        Some(s) => v.extend([i as f64, (self.shift_at(i as usize, Some(c[0])) * 100.0).round() as f64, (s.gate || s.looped) as u8 as f64, s.looped as u8 as f64, s.choke as f64]),
                        None => v.extend([-1.0, 0.0, 0.0, 0.0, 0.0]),
                    }
                }
                v
            }
            // [slice, cents] -> [rate, loop start (-1 = none), loop end, mono samples ...]
            "bake" => match self.bake(a(0) as usize, a(1).round() as i32) {
                Some((m, rate, lp)) => {
                    let mut v = vec![rate as f64, lp.map(|l| l.0 as f64).unwrap_or(-1.0), lp.map(|l| l.1 as f64).unwrap_or(-1.0)];
                    v.extend(m.into_iter().map(|x| x as f64));
                    v
                }
                None => vec![],
            },
            _ => vec![],
        }
    }
    fn info(&self, what: &str) -> String {
        match what {
            "config" | "file" => self.file.clone(),
            "error" => self.err.clone(),
            "kit" => self.kit.clone(),
            "ref" => self.reference.to_string(),
            "preset" => KITS.iter().find(|k| k.name == self.kit).map(|k| k.what.to_string()).unwrap_or_default(),
            "how" => KITS.iter().find(|k| k.name == self.kit).map(|k| k.how.to_string()).unwrap_or_default(),
            "modes" => MODES.join("\n"),
            "timeline" => self
                .timeline
                .iter()
                .map(|s| format!("{:.2}\t{}\t{}", s.start, s.key.map(|k| k.name()).unwrap_or_default(), s.chord.iter().map(|p| crate::theory::Key::new(0, false).pc_name(*p)).collect::<Vec<_>>().join(" ")))
                .collect::<Vec<_>>()
                .join("\n"),
            _ => String::new(),
        }
    }
    fn state(&self) -> Vec<(String, f32)> {
        self.state_entries()
    }
    fn load_state(&mut self, st: &[(String, f32)]) {
        let mut slices: Vec<Slice> = vec![];
        let mut plan: Vec<(usize, i32)> = vec![];
        let mut tl: Vec<Seg> = vec![];
        let mut n_slices: Option<usize> = None;
        let mut params: Vec<(usize, f32)> = vec![];
        for (k, x) in st {
            if let Some(i) = idx(k) {
                params.push((i, *x));
            } else if k == "tune.ref" {
                self.reference = x.max(0.0) as usize;
            } else if k == "slices" {
                n_slices = Some((*x).clamp(0.0, MAX_SLICES as f32) as usize);
            } else if let Some(r) = k.strip_prefix("plan") {
                if r.parse::<usize>().is_ok() {
                    let v = *x as i64;
                    plan.push(((v / 10000) as usize, (v % 10000) as i32 - 5000));
                }
            } else if let Some(r) = k.strip_prefix("tl") {
                let Some((i, f)) = r.split_once('.') else { continue };
                let Ok(i) = i.parse::<usize>() else { continue };
                while tl.len() <= i {
                    tl.push(Seg { start: 0.0, end: f64::MAX, key: None, chord: vec![] });
                }
                match f {
                    "s" => tl[i].start = *x as f64,
                    "e" => tl[i].end = if *x < 0.0 { f64::MAX } else { *x as f64 },
                    "k" => tl[i].key = (*x >= 0.0).then(|| Key::new((*x as u8) % 12, *x >= 12.0)),
                    "c" => tl[i].chord = (0..12u8).filter(|p| (*x as u32) & (1 << p) != 0).collect(),
                    _ => {}
                }
            } else if let Some(r) = k.strip_prefix('s') {
                let Some((i, f)) = r.split_once('.') else { continue };
                let (Ok(i), Some(fi)) = (i.parse::<usize>(), FIELDS.iter().position(|n| *n == f)) else { continue };
                if i >= MAX_SLICES {
                    continue;
                }
                while slices.len() <= i {
                    slices.push(Slice::new(0, 1));
                }
                field_set(&mut slices[i], fi, *x, u32::MAX);
            }
        }
        // the parameters without re-slicing: the slices of the state are the ones to keep
        for (i, x) in params {
            if x.is_finite() {
                self.v[i] = x.clamp(0.0, 1.0);
            }
        }
        if let Some(n) = n_slices {
            slices.truncate(n);
        }
        if !slices.is_empty() {
            self.slices = slices;
            if self.src.is_some() {
                let fr = self.frames() as u32;
                for s in self.slices.iter_mut() {
                    s.start = s.start.min(fr.saturating_sub(1));
                    s.end = s.end.clamp(s.start + 1, fr.max(s.start + 1));
                }
                self.measure();
                self.cache.clear();
            } else {
                self.pending = true;
            }
        } else if self.src.is_some() && n_slices.is_none() {
            // a patch's parameters (no slices of its own): cut the file by them
            self.reslice();
        }
        self.plan = plan;
        self.timeline = tl;
        if self.src.is_some() {
            self.prepare();
        }
    }
}

#[cfg(test)]
#[path = "slicer_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "slicer_pack.rs"]
mod pack;
