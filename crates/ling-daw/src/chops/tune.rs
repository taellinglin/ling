//! Tuning the chops to the song.
//!
//! * Pitch of a chop ([`chop_f0`]): the DAW's YIN tracker (`vocal::track_pitch`, 5 ms frames) over the chop; only voiced frames
//!   count, the first and last fifth of them (the scoop into the vowel, the fall at its end) are dropped, frames an octave off the
//!   median are folded back, and the median of the rest is the pitch. The confidence is the share of voiced frames times how steady
//!   they are (the median distance from the median, in cents). A chop with too few voiced frames or a wobbly pitch has none: an "s",
//!   a "t", a breath plays as it was recorded.
//! * Where it goes ([`Tune`]): a static transposition per chop note (the sampler plays the slice `cents` off its key; no vibrato, no
//!   sweep). The target is a chord tone of what sounds at the beat the chop plays ([`chord_at`]: held notes and the bass of the
//!   other tracks; a lone bass becomes the diatonic triad over it), else the nearest tone of the key. The nearest target to the sung
//!   pitch wins, so the shift is the smallest that fits. A note the arrangement means (call and response on the chord tones, a layer
//!   a third or an octave up, the written note of a dry hit) is the target itself ([`Tune::fit`]), unless reaching it would move the
//!   chop more than `max_shift` semitones (default 5: further than that a voice turns into a chipmunk or a monster): then the same
//!   note an octave nearer, else the nearest chord tone. The fine part - the singer's drift off the semitone - is corrected by `fine`
//!   (1 = to the cent, 0 = the drift stays and only whole semitones move).
use crate::model::Note;
use crate::theory::Key;

/// how far the chops are tuned
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TuneMode {
    /// as sung: every chop at its own pitch (the layer still a third / an octave above it)
    Off,
    /// to the nearest tone of the key
    Key,
    /// to the chord tones sounding where the chop plays
    Chords,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Tune {
    pub mode: TuneMode,
    /// 0 .. 1: how much of the singer's drift off the semitone is corrected
    pub fine: f32,
    /// the largest transposition in semitones (the riser's bend is not counted)
    pub max_shift: f32,
}

impl Default for Tune {
    fn default() -> Tune {
        Tune { mode: TuneMode::Chords, fine: 1.0, max_shift: 5.0 }
    }
}

impl Tune {
    pub fn off() -> Tune {
        Tune { mode: TuneMode::Off, fine: 0.0, max_shift: 5.0 }
    }
    /// "off" | "key" | "chords" | "fine" (chords + fine, the default); `fine` / `max` override the amount and the cap
    pub fn parse(amount: &str, fine: Option<f64>, max: Option<f64>) -> Tune {
        let a = amount.trim().to_lowercase();
        let (mode, f) = match a.as_str() {
            "off" | "0" | "no" | "none" | "false" => (TuneMode::Off, 0.0),
            "key" | "scale" | "key only" => (TuneMode::Key, 0.0),
            "chords" | "chord" => (TuneMode::Chords, 0.0),
            _ => (TuneMode::Chords, 1.0),
        };
        let fine = if mode == TuneMode::Off { 0.0 } else { fine.map(|x| x.clamp(0.0, 1.0) as f32).unwrap_or(f) };
        Tune { mode, fine, max_shift: max.map(|x| x.clamp(1.0, 24.0) as f32).unwrap_or(5.0) }
    }
    pub fn on(&self) -> bool {
        self.mode != TuneMode::Off
    }
    pub fn name(&self) -> &'static str {
        match (self.mode, self.fine >= 0.5) {
            (TuneMode::Off, _) => "off",
            (TuneMode::Key, false) => "key",
            (TuneMode::Key, true) => "key + fine",
            (TuneMode::Chords, false) => "chords",
            (TuneMode::Chords, true) => "chords + fine",
        }
    }

    /// the pitch classes a chop may land on at beat `t` (the scale when nothing sounds, or in "key" mode)
    pub fn tones(&self, key: Key, harmony: &[Note], t: f64) -> Vec<u8> {
        match self.mode {
            TuneMode::Chords => chord_at(key, harmony, t).unwrap_or_else(|| key.scale().to_vec()),
            _ => key.scale().to_vec(),
        }
    }

    /// the target for a chop sung at `f0` (MIDI, fractional): the nearest of `tones` (the sung semitone when tuning is off)
    pub fn nearest(&self, f0: f32, tones: &[u8]) -> i32 {
        if !self.on() || tones.is_empty() {
            return f0.round() as i32;
        }
        let r = f0.round() as i32;
        (r - 7..=r + 7).filter(|p| tones.contains(&(p.rem_euclid(12) as u8))).min_by(|a, b| (*a as f32 - f0).abs().total_cmp(&(*b as f32 - f0).abs())).unwrap_or(r)
    }

    /// an intended note `want` made playable by a chop sung at `f0`: itself while the shift stays under the cap, else the same note
    /// an octave nearer, else the nearest of `tones`; and whether it had to give way
    pub fn fit(&self, f0: f32, want: i32, tones: &[u8]) -> (i32, bool) {
        if (want as f32 - f0).abs() <= self.max_shift + 1e-3 {
            return (want, false);
        }
        let folded = want - 12 * ((want as f32 - f0) / 12.0).round() as i32;
        if (folded as f32 - f0).abs() <= self.max_shift + 1e-3 {
            return (folded, true);
        }
        let n = if self.on() { self.nearest(f0, tones) } else { Tune::default().nearest(f0, tones) };
        (n, true)
    }

    /// a note above `main` for the layer: `want` (the third or the octave the arrangement asks for) when the shift stays under the
    /// cap, else the first of `tones` 3 .. 9 semitones above `main` that does; None when nothing fits
    pub fn layer(&self, f0: f32, main: i32, want: i32, tones: &[u8]) -> Option<i32> {
        let ok = |p: i32| (p as f32 - f0).abs() <= self.max_shift + 1e-3;
        if ok(want) {
            return Some(want);
        }
        (main + 3..=main + 9).find(|p| tones.contains(&(p.rem_euclid(12) as u8)) && ok(*p))
    }

    /// the note key and the slice's cents that make a chop sung at `f0` sound at `target` (plus the drift `1 - fine` leaves);
    /// the sampler's zone root is 60
    pub fn place(&self, f0: f32, target: i32) -> (u8, f32) {
        let fine = if self.on() { self.fine } else { 0.0 };
        let r = f0.round();
        let heard = target as f32 + (1.0 - fine) * (f0 - r);
        let key = target.clamp(0, 127);
        (key as u8, (heard - f0 - (key - 60) as f32) * 100.0)
    }
}

/// The chord sounding at beat `t` in `harmony` (pitch classes): the held notes (a beat or longer, or under C4) of what sounds; a
/// lone bass (or a bass and one more) is filled out to the diatonic triad over it (a major triad over a bass outside the key); a
/// cluster of more than five pitch classes is read as the triad over its bass. None when nothing sounds.
pub fn chord_at(key: Key, harmony: &[Note], t: f64) -> Option<Vec<u8>> {
    let sounding: Vec<&Note> = harmony.iter().filter(|n| n.start <= t + 1e-6 && n.start + n.len > t + 1e-6).collect();
    if sounding.is_empty() {
        return None;
    }
    let held: Vec<&Note> = sounding.iter().copied().filter(|n| n.len >= 0.75 || n.key < 60).collect();
    let src = if held.is_empty() { &sounding } else { &held };
    let bass = src.iter().map(|n| n.key).min().unwrap_or(0) % 12;
    let mut pcs: Vec<u8> = vec![];
    for n in src {
        let pc = n.key % 12;
        if !pcs.contains(&pc) {
            pcs.push(pc);
        }
    }
    let over_bass = || -> Vec<u8> {
        let sc = key.scale();
        match sc.iter().position(|p| *p == bass) {
            Some(d) => vec![sc[d], sc[(d + 2) % 7], sc[(d + 4) % 7]],
            None => vec![bass, (bass + 4) % 12, (bass + 7) % 12],
        }
    };
    if pcs.len() <= 2 {
        let mut t3 = over_bass();
        for p in pcs {
            if !t3.contains(&p) {
                t3.push(p);
            }
        }
        Some(t3)
    } else if pcs.len() <= 5 {
        Some(pcs)
    } else {
        Some(over_bass())
    }
}

/// the pitch of a chop and how sure it is
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct F0 {
    /// MIDI, fractional
    pub midi: f32,
    /// 0 .. 1
    pub conf: f32,
    /// share of the chop's frames that are voiced
    pub voiced: f32,
    /// median distance of the steady frames from the pitch, cents
    pub spread: f32,
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(f32::total_cmp);
    let n = v.len();
    if n == 0 {
        0.0
    } else if n % 2 == 1 {
        v[n / 2]
    } else {
        0.5 * (v[n / 2 - 1] + v[n / 2])
    }
}

/// the least confidence a chop needs to be tuned
pub const MIN_CONF: f32 = 0.3;

/// The pitch of `mono[s..e]` (see the module notes), None when it has no steady voiced part.
pub fn chop_f0(mono: &[f32], s: usize, e: usize, rate: u32) -> Option<F0> {
    let e = e.min(mono.len());
    if e <= s + rate as usize / 50 {
        return None;
    }
    let p = crate::vocal::track_pitch(&mono[s..e], rate as f32);
    let frames = p.f0.len().max(1);
    let voiced: Vec<f32> = p.f0.iter().copied().filter(|x| *x > 60.0 && *x < 1400.0).map(|f| 69.0 + 12.0 * (f / 440.0).log2()).collect();
    let share = voiced.len() as f32 / frames as f32;
    if voiced.len() < 4 || share < 0.4 {
        return None;
    }
    // the steady middle: without the first and the last fifth of the voiced frames
    let n = voiced.len();
    let mid = if n >= 10 { &voiced[n / 5..n - n / 5] } else { &voiced[..] };
    let m0 = median(&mut mid.to_vec());
    let mut folded: Vec<f32> = mid.iter().map(|m| if (m - m0 - 12.0).abs() < 0.8 { m - 12.0 } else if (m - m0 + 12.0).abs() < 0.8 { m + 12.0 } else { *m }).collect();
    let med = median(&mut folded);
    let mut dev: Vec<f32> = folded.iter().map(|m| (m - med).abs() * 100.0).collect();
    let spread = median(&mut dev);
    let conf = (share * 1.4).min(1.0) * (1.0 - (spread - 20.0) / 80.0).clamp(0.0, 1.0);
    (conf >= MIN_CONF).then_some(F0 { midi: med, conf, voiced: share, spread })
}

/// what the tuning did to the chop notes of a part
#[derive(Clone, Debug, Default)]
pub struct TuneStats {
    /// pitched chop notes moved by 3 cents or more
    pub tuned: usize,
    /// pitched chop notes left where they were
    pub kept: usize,
    /// unpitched chop notes (played as recorded)
    pub noise: usize,
    /// notes whose intended pitch was too far: played an octave nearer or on another chord tone
    pub capped: usize,
    /// the shifts of the tuned notes, cents (signed)
    pub shifts: Vec<f32>,
}

impl TuneStats {
    pub fn add(&mut self, o: &TuneStats) {
        self.tuned += o.tuned;
        self.kept += o.kept;
        self.noise += o.noise;
        self.capped += o.capped;
        self.shifts.extend_from_slice(&o.shifts);
    }
    pub fn note(&mut self, shift_cents: Option<f32>, capped: bool) {
        match shift_cents {
            None => self.noise += 1,
            Some(c) if c.abs() >= 3.0 => {
                self.tuned += 1;
                self.shifts.push(c);
            }
            Some(_) => self.kept += 1,
        }
        self.capped += capped as usize;
    }
    /// the median size of the shifts, cents
    pub fn median_shift(&self) -> f32 {
        let mut a: Vec<f32> = self.shifts.iter().map(|x| x.abs()).collect();
        median(&mut a)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: u32 = 44100;

    /// a voice-like tone: a few harmonics at `midi`, `secs` long, with a short attack and release
    fn voiced(midi: f32, secs: f32) -> Vec<f32> {
        let hz = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
        let n = (secs * RATE as f32) as usize;
        (0..n)
            .map(|i| {
                let t = i as f32 / RATE as f32;
                let env = (t / 0.01).min(1.0).min((n - i) as f32 / RATE as f32 / 0.02);
                let ph = t * hz * std::f32::consts::TAU;
                (ph.sin() * 0.5 + (2.0 * ph).sin() * 0.25 + (3.0 * ph).sin() * 0.12) * env * 0.6
            })
            .collect()
    }

    fn noise(secs: f32) -> Vec<f32> {
        let mut r = crate::compose::Rng::new(9);
        (0..(secs * RATE as f32) as usize).map(|_| (r.f() as f32 * 2.0 - 1.0) * 0.4).collect()
    }

    #[test]
    fn a_voiced_chop_has_its_pitch_and_an_unvoiced_one_none() {
        for m in [52.3f32, 60.0, 64.6, 69.34, 76.8] {
            let v = voiced(m, 0.3);
            let f = chop_f0(&v, 0, v.len(), RATE).expect("voiced");
            assert!((f.midi - m).abs() < 0.05, "{m}: {f:?}");
            assert!(f.conf > 0.8, "{f:?}");
        }
        let nz = noise(0.25);
        assert_eq!(chop_f0(&nz, 0, nz.len(), RATE), None, "an 's' has no pitch");
        // a scoop into the note and a fall off it do not move the pitch: the steady middle counts
        let mut sc = voiced(67.0, 0.06);
        sc.extend(voiced(69.0, 0.3));
        sc.extend(voiced(65.0, 0.05));
        let f = chop_f0(&sc, 0, sc.len(), RATE).unwrap();
        assert!((f.midi - 69.0).abs() < 0.1, "{f:?}");
    }

    #[test]
    fn a_chop_moves_to_the_nearest_chord_tone_by_the_smallest_shift() {
        let key = Key::new(0, false);
        let f0 = 69.34; // A4, 34 cents sharp
        // F major sounds (bass F + pad): A is a chord tone - only the drift is corrected
        let f = vec![Note::new(0.0, 4.0, 41, 0.8), Note::new(0.0, 4.0, 65, 0.5), Note::new(0.0, 4.0, 69, 0.5), Note::new(0.0, 4.0, 72, 0.5)];
        let t = Tune::default();
        let tones = t.tones(key, &f, 1.0);
        assert_eq!(t.nearest(f0, &tones), 69);
        let (k, c) = t.place(f0, 69);
        assert_eq!(k, 69);
        assert!((f0 + (k as f32 - 60.0) + c / 100.0 - 69.0).abs() < 0.005, "lands on A to the cent");
        assert!(((k as f32 - 60.0) * 100.0 + c - -34.0).abs() < 0.5, "a shift of -34 cents");
        // C major: G (2.34 down) beats C (2.66 up)
        let cmaj = vec![Note::new(0.0, 4.0, 48, 0.8)];
        let tones = t.tones(key, &cmaj, 1.0);
        assert_eq!(tones, vec![0, 4, 7]);
        assert_eq!(t.nearest(f0, &tones), 67);
        // "chords" without fine: whole semitones, the singer's drift stays
        let coarse = Tune::parse("chords", None, None);
        let (k, c) = coarse.place(f0, 67);
        assert!((f0 + (k as f32 - 60.0) + c / 100.0 - 67.34).abs() < 0.005);
        // key only: A is in C major, it stays (only the drift with "key + fine")
        let ko = Tune::parse("key", None, None);
        assert_eq!(ko.nearest(f0, &ko.tones(key, &cmaj, 1.0)), 69);
        assert_eq!(ko.nearest(70.0, &ko.tones(key, &cmaj, 1.0)), 69, "Bb -> A (nearest scale tone, the lower when it is as near)");
        assert_eq!(ko.nearest(70.6, &ko.tones(key, &cmaj, 1.0)), 71);
        // nothing sounds: the key's scale
        assert_eq!(t.tones(key, &cmaj, 9.0), key.scale().to_vec());
        // off: as sung
        let off = Tune::parse("off", None, None);
        let (k, c) = off.place(f0, off.nearest(f0, &tones));
        assert!((f0 + (k as f32 - 60.0) + c / 100.0 - f0).abs() < 0.005);
    }

    #[test]
    fn an_intended_note_too_far_away_is_folded_or_replaced() {
        let t = Tune::default();
        let tones = vec![9, 0, 4]; // A minor
        // within the cap: the intended note itself
        assert_eq!(t.fit(69.2, 72, &tones), (72, false));
        assert_eq!(t.fit(69.2, 64, &tones), (64, false));
        // an octave up is too far: the same note an octave nearer
        assert_eq!(t.fit(69.2, 81, &tones), (69, true));
        assert_eq!(t.fit(57.0, 76, &tones), (52, true));
        // a tritone either way: the nearest chord tone instead
        let (p, capped) = t.fit(62.0, 56, &tones);
        assert!(capped && tones.contains(&((p % 12) as u8)) && (p - 62).abs() <= 3, "{p}");
        // a layer an octave up becomes the next chord tone above that fits; a larger cap allows the octave
        assert_eq!(t.layer(69.0, 69, 81, &tones), Some(72));
        assert_eq!(Tune { max_shift: 12.0, ..t }.layer(69.0, 69, 81, &tones), Some(81));
        assert_eq!(t.layer(69.0, 74, 86, &[9, 0, 4]), None, "nothing above 74 within 5 semitones of 69 but 74 itself");
        // every shift stays under the cap
        for f0 in [48.0f32, 55.5, 63.2, 70.7, 79.9] {
            for want in 40..100 {
                let (p, _) = t.fit(f0, want, &tones);
                assert!((p as f32 - f0).abs() <= 5.0 + 1e-3, "{f0} -> {want}: {p}");
            }
        }
    }

    #[test]
    fn options_parse() {
        assert_eq!(Tune::parse("", None, None), Tune::default());
        assert_eq!(Tune::parse("off", Some(1.0), None).fine, 0.0);
        assert_eq!(Tune::parse("key", None, None).mode, TuneMode::Key);
        assert_eq!(Tune::parse("chords", Some(0.5), Some(7.0)), Tune { mode: TuneMode::Chords, fine: 0.5, max_shift: 7.0 });
    }
}
