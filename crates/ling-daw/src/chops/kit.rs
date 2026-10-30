//! The vocal chops on Ling Slicer: the chop notes the composers write (chops.rs `compose` / `compose_edm`, a Sampler slice in each
//! note's lyric "@start:end:cents") become a Slicer kit - one slice per key - so the chops are slices you can see, play and move
//! in the piano roll, and the Slicer's auto-tune keeps them on the song's chords and keys as they change.
//!
//! * The slices ([`build`]): every chop a note plays is its syllable's whole span in the recording (the gate ends it where the
//!   note ends, with the Slicer's release: no more one cut per note length); a reversed copy of the hook word in the Sampler's bank
//!   ([`Layout`]) is the same span with the slice's reverse on; a short note (a 1/16 or 1/32 stutter) plays a variant in choke
//!   group 2, so each retrigger cuts the last instead of smearing into it; the rest ring into each other (choke off). Fades of
//!   3 ms in and 25 ms out, as the Sampler's slices had.
//! * The pitch: a chop note meant a pitch (its key and cents: the sung pitch moved to a chord tone, a layer a third or an octave
//!   above, a riser step). A pitched slice is tuned by the Slicer at the beat it plays (tune `chords` or `key`, amount 100 %), so
//!   the variant is the slice with the whole-semitone transposition whose tuned pitch, on the song's harmony at that beat, is the
//!   one the note meant: the same syllable at the chord tone, or a variant a third / an octave up for the layer. When the chords
//!   change later the slices follow them. An unpitched slice (an "s", a breath; the Slicer's own measurement) plays as the Sampler
//!   played it: its semitones and cents are the note's.
//! * Keys: root + i, the slices in recording order (variants after their syllable); more variants than keys and the rarest ones
//!   play the nearest variant of the same syllable that is kept.
//! * The production edits (chops/edits.rs) ask for more in a fourth field of the lyric, "@start:end:cents:flags" (the Sampler
//!   reads the first three): `r` plays the slice backwards (a reverse swell or ear candy without a reversed copy in the bank),
//!   `l` loops it while the note is held (a pad, a riser, a trance gate: the span is the steady middle of a vowel, cut by
//!   [`loop_span`] a whole number of its periods long), `o` makes it a one-shot that rings past its note, `x` puts it in choke
//!   group 3 (a choke-group cut: each hit cuts the last).
use crate::dsp::slicer::{self, Seg, Slice};
use crate::model::Note;
use std::collections::HashMap;

/// the first key of a chop kit (C2: 92 keys up to G8)
pub const ROOT: u8 = 36;

/// Where a Sampler bank's slices come from: the recording (its first `frames` frames) and reversed copies of some of its chops
/// after it: (copy start, copy end, original start, original end); `spans` are the whole chops (a note that starts one plays it).
#[derive(Clone, Debug, Default)]
pub struct Layout {
    pub frames: u32,
    pub rev: Vec<(u32, u32, u32, u32)>,
    pub spans: Vec<(u32, u32)>,
}

impl Layout {
    /// the bank of a cut vocal (chops.rs `Vocal::add_reverse`): `frames` of recording, then its reversed hook pieces
    pub fn of_vocal(v: &super::Vocal, frames: u32) -> Layout {
        let rev = v.rev.iter().filter_map(|(pi, c)| v.pieces.get(*pi).map(|p| (c.start, c.end, p.chop.start, p.chop.end))).collect();
        Layout { frames, rev, spans: v.pieces.iter().map(|p| (p.chop.start, p.chop.end)).collect() }
    }
    /// a file cut at its transients (chops.rs `pick_chops`)
    pub fn of_chops(ch: &[super::Chop], frames: u32) -> Layout {
        Layout { frames, rev: vec![], spans: ch.iter().map(|c| (c.start, c.end)).collect() }
    }
    /// The span of the recording a Sampler slice `a..b` plays and whether it plays it backwards; None when it is outside both.
    pub fn source(&self, a: u32, b: u32) -> Option<(u32, u32, bool)> {
        if a >= self.frames {
            let &(rs, re, oa, ob) = self.rev.iter().find(|r| a >= r.0 && a < r.1)?;
            // frame x of the copy is frame ob - 1 - (x - rs) of the recording
            let b = b.min(re);
            let s = ob.saturating_sub(b - rs).max(oa);
            let e = ob.saturating_sub(a - rs);
            return (e > s).then_some((s, e, true));
        }
        let b = b.min(self.frames);
        match self.spans.iter().find(|s| s.0 == a && b <= s.1 + 1) {
            Some(s) => Some((s.0, s.1.min(self.frames), false)),
            None => (b > a).then_some((a, b, false)),
        }
    }
}

/// a slice of the kit: the span, backwards, the transposition (semitones, cents), the choke group and the play flags
/// ([`LOOP`], [`ONE_SHOT`])
type Var = (u32, u32, bool, i32, i32, u8, u8);

pub const LOOP: u8 = 1;
pub const ONE_SHOT: u8 = 2;

/// the edit flags of a chop note's lyric: the fourth field of "@start:end:cents:flags" ("" when none)
pub fn flags_of(lyric: &str) -> &str {
    lyric.strip_prefix('@').and_then(|b| b.splitn(4, ':').nth(3)).unwrap_or("")
}

/// The loop of a held slice over `s..e` of `mono`: from the rising zero crossing nearest `s` to the one a whole number of the
/// sung periods later (the most that fit before `e`), so one cycle joins the next at the seam; `s..e` as it is when it has no
/// steady pitch or holds fewer than two periods.
pub fn loop_span(mono: &[f32], s: u32, e: u32, rate: u32) -> (u32, u32) {
    let (s0, e0) = (s as usize, (e as usize).min(mono.len()));
    if e0 <= s0 + 64 {
        return (s, e);
    }
    let Some(f) = super::chop_f0(mono, s0, e0, rate) else { return (s, e) };
    let period = rate as f64 / crate::dsp::midi_hz(f.midi) as f64;
    if !(period >= 8.0) || ((e0 - s0) as f64) < 2.5 * period {
        return (s, e);
    }
    // the rising zero crossing nearest `at` within `r` frames (inside s0..e0)
    let rising = |at: usize, r: usize| -> Option<usize> {
        (0..=r).flat_map(|d| [at.checked_sub(d), Some(at + d)]).flatten().find(|c| *c > s0 && *c < e0 && mono[c - 1] < 0.0 && mono[*c] >= 0.0)
    };
    let Some(a) = rising(s0 + period as usize, period as usize) else { return (s, e) };
    let n = ((e0 - a) as f64 / period).floor();
    if n < 2.0 {
        return (s, e);
    }
    let want = a + (n * period).round() as usize;
    let b = rising(want.min(e0 - 1), (period / 4.0) as usize).filter(|b| *b > a).unwrap_or(want.min(e0));
    (a as u32, b as u32)
}

/// what [`build`] made
#[derive(Clone, Debug, Default)]
pub struct Built {
    pub slices: Vec<Slice>,
    /// per note given: its key in the kit (None: it plays nothing - outside the recording)
    pub keys: Vec<Option<u8>>,
    /// notes that play a pitched slice (the Slicer tunes them), and variants merged into a neighbour for want of keys
    pub pitched: usize,
    pub merged: usize,
}

/// how a note on variant `v` sounds at `beat` (MIDI), for a slice sung at `f0`
fn played(f0: f32, u: i32, tl: &[Seg], tune: usize, beat: f64) -> f32 {
    let h = f0 + u as f32;
    h + slicer::tune_shift(tl, tune, 1.0, h, beat)
}

/// The kit for the chop notes `notes` ((absolute beat, note) with Sampler slice lyrics) over the recording `mono` (the bank as the
/// Slicer decodes it), tuned by the Slicer in `tune` mode (0 off, 1 key, 2 chords) on the harmony `tl`; keys from `root`.
pub fn build(notes: &[(f64, &Note)], lay: &Layout, mono: &[f32], rate: u32, tl: &[Seg], tune: usize, root: u8) -> Built {
    let mut f0s: HashMap<(u32, u32), Option<f32>> = HashMap::new();
    let mut want: Vec<Option<Var>> = vec![];
    let mut out = Built::default();
    for (beat, n) in notes {
        let Some((a, b, cents)) = crate::dsp::sampler::parse_slice(&n.lyric) else {
            want.push(None);
            continue;
        };
        let Some((s, e, rev)) = lay.source(a, b) else {
            want.push(None);
            continue;
        };
        let fl = flags_of(&n.lyric);
        let rev = rev ^ fl.contains('r');
        let play = if fl.contains('l') { LOOP } else { 0 } | if fl.contains('o') { ONE_SHOT } else { 0 };
        let (s, e) = if play & LOOP != 0 { loop_span(mono, s, e, rate) } else { (s, e) };
        // what the Sampler played: the recording moved by the key (root 60) and the cents
        let shift = n.key as f32 - 60.0 + cents / 100.0;
        let f0 = *f0s.entry((s, e)).or_insert_with(|| super::chop_f0(mono, s as usize, e as usize, rate).map(|f| f.midi));
        let choke = if fl.contains('x') { 3 } else if n.len <= 0.26 { 2 } else { 0 };
        let v = match f0 {
            Some(f) if tune > 0 => {
                out.pitched += 1;
                // the whole-semitone variant the Slicer's tuning turns into the pitch the note meant
                let meant = f + shift;
                let r = shift.round() as i32;
                let u = (r - 3..=r + 3).min_by(|x, y| {
                    let cost = |u: i32| (played(f, u, tl, tune, *beat) - meant).abs() + 0.01 * (u as f32 - shift).abs();
                    cost(*x).total_cmp(&cost(*y))
                });
                (s, e, rev, u.unwrap_or(r), 0, choke, play)
            }
            _ => {
                // played as recorded, moved exactly as the Sampler moved it
                let u = shift.round();
                (s, e, rev, u as i32, ((shift - u) * 100.0).round() as i32, choke, play)
            }
        };
        want.push(Some(v));
    }
    // the variants, the most used first when there are more than keys
    let mut uses: HashMap<Var, usize> = HashMap::new();
    for v in want.iter().flatten() {
        *uses.entry(*v).or_insert(0) += 1;
    }
    let cap = ((128 - root as usize).min(slicer::MAX_SLICES)).max(1);
    let mut vars: Vec<Var> = uses.keys().copied().collect();
    vars.sort_by(|x, y| uses[y].cmp(&uses[x]).then(x.cmp(y)));
    let dropped: Vec<Var> = if vars.len() > cap { vars.split_off(cap) } else { vec![] };
    // in recording order: the keys climb through the vocal
    vars.sort();
    let at: HashMap<Var, usize> = vars.iter().enumerate().map(|(i, v)| (*v, i)).collect();
    let mut remap: HashMap<Var, usize> = HashMap::new();
    for d in &dropped {
        // the nearest kept variant of the same syllable, else the nearest kept syllable
        let best = vars.iter().enumerate().min_by_key(|(_, k)| (if (k.0, k.1, k.2) == (d.0, d.1, d.2) { 0 } else { 1 }, (k.3 - d.3).abs() * 100 + (k.4 - d.4).abs(), (k.0 as i64 - d.0 as i64).abs()));
        if let Some((i, _)) = best {
            remap.insert(*d, i);
            out.merged += 1;
        }
    }
    out.slices = vars
        .iter()
        .map(|v| {
            let mut s = Slice::new(v.0, v.1);
            s.reverse = v.2;
            s.semis = v.3.clamp(-24, 24) as f32;
            s.cents = v.4.clamp(-100, 100) as f32;
            s.gate = v.6 & ONE_SHOT == 0;
            s.looped = v.6 & LOOP != 0;
            s.choke = v.5;
            s.fade_in = 3.0;
            s.fade_out = 25.0;
            s
        })
        .collect();
    out.keys = want.iter().map(|w| w.and_then(|v| at.get(&v).or_else(|| remap.get(&v)).map(|i| (root as usize + i).min(127) as u8))).collect();
    out
}

/// the Slicer's parameters for a chop kit: manual slices from `ROOT`, a 3 ms attack and a 60 ms release (the Sampler's chops'
/// envelope), tuned in `tune` mode at 100 %, pitch shifts keeping the length
pub fn params(tune: usize) -> Vec<(&'static str, f32)> {
    vec![("mode", 1.0), ("root", ROOT as f32 / 127.0), ("attack", (0.003f32 / 0.5).sqrt()), ("release", ((0.06f32 - 0.005) / 2.0).sqrt()), ("tune", tune.min(2) as f32 / 2.0), ("amount", 1.0), ("keeplen", 1.0), ("stretch", 0.0)]
}

/// the Slicer's tune mode for the chops' tuning
pub fn tune_mode(t: &super::Tune) -> usize {
    match t.mode {
        super::TuneMode::Off => 0,
        super::TuneMode::Key => 1,
        super::TuneMode::Chords => 2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::Instrument;
    use crate::theory::Key;

    const RATE: u32 = 44100;

    /// three sung tones (A3, C#4, a noise burst), 0.4 s each with 0.1 s gaps: interleaved stereo
    fn tones() -> (Vec<f32>, Vec<(u32, u32)>) {
        let mut out = vec![];
        let mut spans = vec![];
        let mut seed = 7u32;
        for k in 0..3 {
            let a = (out.len() / 2) as u32;
            let n = (RATE as f32 * 0.4) as usize;
            let hz = [220.0f32, 277.18, 0.0][k];
            for i in 0..n {
                let t = i as f32 / RATE as f32;
                let env = (t / 0.01).min(1.0).min((0.4 - t) / 0.02).max(0.0);
                seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                let x = if hz > 0.0 { (t * hz * std::f32::consts::TAU).sin() * 0.5 } else { ((seed >> 9) as f32 / (1u32 << 23) as f32 - 0.5) * 0.4 };
                out.push(x * env);
                out.push(x * env);
            }
            spans.push((a, (out.len() / 2) as u32));
            out.extend(vec![0.0; (RATE as f32 * 0.1) as usize * 2]);
        }
        (out, spans)
    }

    fn sampler_note(beat: f64, len: f64, a: u32, b: u32, key: u8, cents: f32) -> Note {
        let mut n = Note::new(beat, len, key, 0.8);
        n.lyric = format!("@{a}:{b}:{cents:.0}");
        n
    }

    /// the Sampler's chop note that makes a chop sung at `f0` sound at `target` (chops.rs tuned_note)
    fn meant(beat: f64, len: f64, a: u32, b: u32, f0: f32, target: i32) -> Note {
        let (k, cents) = crate::chops::Tune::default().place(f0, target);
        sampler_note(beat, len, a, b, k, cents)
    }

    /// C major for 4 beats, then F major
    fn harmony() -> Vec<Seg> {
        vec![Seg { start: 0.0, end: 4.0, key: Some(Key::new(0, false)), chord: vec![0, 4, 7] }, Seg { start: 4.0, end: f64::MAX, key: Some(Key::new(0, false)), chord: vec![0, 5, 9] }]
    }

    #[test]
    fn chop_notes_become_slices_the_slicer_tunes_to_the_meant_pitch() {
        let (st, spans) = tones();
        let mono = slicer::mono_of(&st);
        let lay = Layout { frames: (st.len() / 2) as u32, rev: vec![], spans: spans.clone() };
        let (a0, b0) = spans[0];
        let f0 = crate::chops::chop_f0(&mono, a0 as usize, b0 as usize, RATE).unwrap().midi;
        // A3 (57) meant as G3 over C, as A3 over F, and the layer a third up: C4 over F
        let n1 = meant(0.0, 1.0, a0, b0, f0, 55);
        let n2 = meant(4.0, 1.0, a0, b0, f0, 57);
        let n3 = meant(4.0, 1.0, a0, b0, f0, 60);
        // a stutter of the same syllable, trimmed: the whole syllable, gated, in choke group 2
        let n4 = meant(5.0, 0.125, a0, a0 + 3000, f0, 57);
        // the noise burst: unpitched, played exactly as the Sampler did (key 61 and 30 cents)
        let (a2, b2) = spans[2];
        let n5 = sampler_note(6.0, 1.0, a2, b2, 61, 30.0);
        let ns = [n1, n2, n3, n4, n5];
        let input: Vec<(f64, &Note)> = ns.iter().map(|n| (n.start, n)).collect();
        let k = build(&input, &lay, &mono, RATE, &harmony(), 2, ROOT);
        assert!(k.keys.iter().all(|x| x.is_some()), "{:?}", k.keys);
        // every slice of syllable 1 spans the whole syllable
        for s in k.slices.iter().filter(|s| s.start == a0) {
            assert_eq!(s.end, b0);
            assert!(s.gate);
        }
        let slice_of = |i: usize| &k.slices[(k.keys[i].unwrap() - ROOT) as usize];
        assert_eq!(slice_of(3).choke, 2, "the stutter cuts itself");
        assert_eq!(slice_of(0).choke, 0);
        assert_eq!((slice_of(4).semis, slice_of(4).cents), (1.0, 30.0), "unpitched: the Sampler's move");
        // the tuned pitch of each note on the harmony at its beat
        for (i, meant) in [(0usize, 55.0f32), (1, 57.0), (2, 60.0)] {
            let s = slice_of(i);
            let p = played(f0, s.semis as i32, &harmony(), 2, ns[i].start);
            assert!((p - meant).abs() < 0.05, "note {i}: {p} for {meant}");
        }
        // the same note at the same pitch shares a slice: n2 and the stutter differ only by the choke
        assert_eq!(k.slices.len(), 5, "{:?}", k.slices.iter().map(|s| (s.start, s.semis, s.choke)).collect::<Vec<_>>());
    }

    #[test]
    fn a_reversed_bank_copy_is_the_slice_played_backwards() {
        let lay = Layout { frames: 1000, rev: vec![(1200, 1500, 100, 400)], spans: vec![(100, 400)] };
        assert_eq!(lay.source(100, 400), Some((100, 400, false)));
        assert_eq!(lay.source(100, 250), Some((100, 400, false)), "a trimmed chop plays its whole syllable, gated");
        assert_eq!(lay.source(1200, 1500), Some((100, 400, true)));
        // the swell skips the copy's start: the original's last frames
        assert_eq!(lay.source(1250, 1500), Some((100, 350, true)));
        assert_eq!(lay.source(1600, 1700), None);
    }

    #[test]
    fn edit_flags_make_reversed_looped_one_shot_and_choked_slices() {
        let (st, spans) = tones();
        let mono = slicer::mono_of(&st);
        let lay = Layout { frames: (st.len() / 2) as u32, rev: vec![], spans: spans.clone() };
        let (a0, b0) = spans[0];
        let (a2, b2) = spans[2];
        let mid = (a0 + (b0 - a0) * 3 / 10, b0 - (b0 - a0) / 5);
        let mut ns = vec![sampler_note(0.0, 1.0, a0, b0, 57, 0.0), sampler_note(1.0, 1.0, a0, b0, 57, 0.0), sampler_note(2.0, 4.0, mid.0, mid.1, 57, 0.0), sampler_note(6.0, 0.5, a2, b2, 60, 0.0), sampler_note(7.0, 0.5, a2, b2, 60, 0.0)];
        crate::chops::edits::flag(&mut ns[1], "r");
        crate::chops::edits::flag(&mut ns[2], "l");
        crate::chops::edits::flag(&mut ns[3], "o");
        crate::chops::edits::flag(&mut ns[4], "x");
        let input: Vec<(f64, &Note)> = ns.iter().map(|n| (n.start, n)).collect();
        let k = build(&input, &lay, &mono, RATE, &harmony(), 0, ROOT);
        let slice_of = |i: usize| &k.slices[(k.keys[i].unwrap() - ROOT) as usize];
        assert!(!slice_of(0).reverse && slice_of(1).reverse, "r plays the syllable backwards");
        assert_eq!((slice_of(1).start, slice_of(1).end), (a0, b0));
        let l = slice_of(2);
        assert!(l.looped && l.gate && !slice_of(0).looped);
        assert!(l.start >= mid.0 && l.end <= mid.1, "the loop stays in the vowel's middle");
        assert!(!slice_of(3).gate, "o: a one-shot");
        assert_eq!(slice_of(4).choke, 3);
        assert_eq!(slice_of(0).choke, 0);
    }

    #[test]
    fn a_held_vowel_loops_whole_periods_and_never_dips() {
        // a 220 Hz tone: the loop is a whole number of periods from a rising zero crossing
        let (st, spans) = tones();
        let mono = slicer::mono_of(&st);
        let (a0, b0) = spans[0];
        let (s, e) = loop_span(&mono, a0 + 4000, b0 - 3000, RATE);
        let period = RATE as f64 / 220.0;
        let n = (e - s) as f64 / period;
        assert!((n - n.round()).abs() < 0.05 && n >= 2.0, "{n} periods");
        assert!(mono[s as usize - 1] < 0.0 && mono[s as usize] >= 0.0);
        assert!((mono[e as usize] - mono[s as usize]).abs() < 0.05, "the seam joins: {} {}", mono[s as usize], mono[e as usize]);
        // the Slicer holds it for two seconds at a steady level: the loop's start is not faded in again on every pass
        let dir = std::env::temp_dir().join("lsx_chop_kit");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tones_loop.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for x in &st {
            w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        let mut sl = Slice::new(s, e);
        sl.looped = true;
        sl.gate = true;
        sl.fade_in = 3.0;
        sl.fade_out = 25.0;
        sl.choke = 0;
        let mut x = slicer::Slicer::with_slices(RATE as f32, &path.to_string_lossy(), vec![sl], &params(0));
        x.note_on(ROOT, 0.9);
        let mut out = vec![];
        let mut buf = vec![0f32; 512];
        while out.len() < RATE as usize * 2 {
            buf.fill(0.0);
            x.render(&mut buf);
            out.extend(buf.chunks(2).map(|f| (f[0] + f[1]) * 0.5));
        }
        // the level of every period after the attack: a fade-in at each pass of the loop would take a fifth of one away
        let p = period.round() as usize;
        let lv: Vec<f32> = out[p * 4..].chunks(p).filter(|c| c.len() == p).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / p as f32).sqrt()).collect();
        let top = lv.iter().copied().fold(0.0f32, f32::max);
        let low = lv.iter().copied().fold(f32::MAX, f32::min);
        assert!(top > 0.05 && low > 0.95 * top, "steady: {low} .. {top}");
    }

    #[test]
    fn too_many_variants_share_the_nearest_kept_one() {
        let (st, spans) = tones();
        let mono = slicer::mono_of(&st);
        let lay = Layout { frames: (st.len() / 2) as u32, rev: vec![], spans: spans.clone() };
        let (a2, b2) = spans[2];
        // the noise burst at 12 transpositions, a kit of 120 keys from root 120: only 8 keys
        let ns: Vec<Note> = (0..12).map(|k| sampler_note(k as f64, 1.0, a2, b2, 54 + k as u8, 0.0)).collect();
        let input: Vec<(f64, &Note)> = ns.iter().map(|n| (n.start, n)).collect();
        let k = build(&input, &lay, &mono, RATE, &harmony(), 2, 120);
        assert_eq!(k.slices.len(), 8);
        assert_eq!(k.merged, 4);
        assert!(k.keys.iter().all(|x| x.map(|k| k >= 120).unwrap_or(false)));
    }

    #[test]
    fn the_slicer_sounds_the_kit_on_the_chords() {
        // the kit in a real Slicer: the note over C and the same note over F, rendered and measured
        let (st, spans) = tones();
        let dir = std::env::temp_dir().join("lsx_chop_kit");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("tones.wav");
        let spec = hound::WavSpec { channels: 2, sample_rate: RATE, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
        let mut w = hound::WavWriter::create(&path, spec).unwrap();
        for x in &st {
            w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16).unwrap();
        }
        w.finalize().unwrap();
        let (dec, rate) = crate::dsp::sampler::load_audio(&path.to_string_lossy()).unwrap();
        let mono = slicer::mono_of(&dec);
        let lay = Layout { frames: (dec.len() / 2) as u32, rev: vec![], spans: spans.clone() };
        let (a0, b0) = spans[0];
        let f0 = crate::chops::chop_f0(&mono, a0 as usize, b0 as usize, rate).unwrap().midi;
        let ns = [meant(0.0, 1.0, a0, b0, f0, 55), meant(4.0, 1.0, a0, b0, f0, 60)];
        let input: Vec<(f64, &Note)> = ns.iter().map(|n| (n.start, n)).collect();
        let k = build(&input, &lay, &mono, rate, &harmony(), 2, ROOT);
        let mut s = slicer::Slicer::with_slices(48000.0, &path.to_string_lossy(), k.slices.clone(), &params(2));
        assert_eq!(s.slices.len(), k.slices.len(), "the kit is kept as it is");
        s.timeline = harmony();
        for (i, meant) in [(0usize, 55.0f32), (1, 60.0)] {
            s.all_off();
            s.note_beat(ns[i].start);
            s.note_on(k.keys[i].unwrap(), 0.9);
            let mut out = vec![];
            let mut buf = vec![0f32; 1024];
            while out.len() < 48000 * 3 / 10 {
                buf.fill(0.0);
                s.render(&mut buf);
                out.extend(buf.chunks(2).map(|f| (f[0] + f[1]) * 0.5));
            }
            let f = crate::chops::chop_f0(&out, 0, out.len(), 48000).expect("pitched").midi;
            assert!((f - meant).abs() < 0.3, "note {i}: heard {f} for {meant}");
        }
        // a project round trip keeps the kit (overlapping slices, manual mode)
        let st_ = s.state();
        let mut r = slicer::Slicer::new(48000.0);
        r.load_state(&st_);
        r.command("load", &[], &path.to_string_lossy());
        assert_eq!(r.slices.len(), s.slices.len());
        assert_eq!(r.slices.iter().map(|x| (x.start, x.end, x.semis as i32, x.choke)).collect::<Vec<_>>(), s.slices.iter().map(|x| (x.start, x.end, x.semis as i32, x.choke)).collect::<Vec<_>>());
    }
}
