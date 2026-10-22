//! "Add harmonized part": voices written under (or over) a chosen set of melody notes, in the key and chords of a chosen track.
//! Pure functions over absolute-time notes; `api/theory_api.rs` gathers the notes and puts the result in a new clip.
//!
//! Each voice is one of
//!   * a scale interval from the melody (a third, fifth or sixth, above or below, counted in steps of the *key*, so every note stays in key),
//!   * a chord tone below the melody (the nearest, second nearest ... which makes a closed block voicing that follows the chords),
//!   * the chord's bass note a good way under the melody,
//!   * a counter line that moves the other way from the melody.
//! After a voice is placed it is steered away from the sounds that make a harmony feel wrong: a minor second or major seventh against
//! the melody, a unison, and notes the chord under it does not have when a chord tone is a step away.
use crate::theory::{chord_at, Chord, Key, KeySection, N};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Voice {
    /// signed scale steps from the melody (-2 = a third below, +4 = a fifth above, 7 = an octave)
    Steps(i32),
    /// the n-th chord tone below the melody (1 = nearest)
    Stack(usize),
    /// the root of the chord, about an octave and a bit below
    Bass,
    /// moves opposite to the melody, below it
    Counter,
}

impl Voice {
    pub fn label(&self) -> String {
        match self {
            Voice::Steps(s) => {
                let name = match s.abs() {
                    0 => "unison",
                    1 => "second",
                    2 => "third",
                    3 => "fourth",
                    4 => "fifth",
                    5 => "sixth",
                    6 => "seventh",
                    7 => "octave",
                    _ => "interval",
                };
                format!("{name} {}", if *s < 0 { "below" } else { "above" })
            }
            Voice::Stack(n) => format!("chord tone {n} below"),
            Voice::Bass => "chord root bass".into(),
            Voice::Counter => "counter line".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub voices: Vec<Voice>,
    /// hold each harmony note until the next melody note starts
    pub sustain: bool,
    /// velocity of the harmony relative to the melody
    pub level: f32,
}

impl Spec {
    pub fn summary(&self) -> String {
        let v: Vec<String> = self.voices.iter().map(|v| v.label()).collect();
        format!("{} voice{}: {}{}{}", v.len(), if v.len() == 1 { "" } else { "s" }, v.join(", "), if self.sustain { ", held" } else { "" }, if (self.level - 0.8).abs() > 0.05 { format!(", level {}%", (self.level * 100.0).round()) } else { String::new() })
    }
}

/// Read the optional prompt: "thirds above", "choir", "jazz stack", "counter melody, soft", "two voices, sixths"...
pub fn parse_prompt(text: &str) -> Spec {
    let t = text.to_lowercase();
    let has = |w: &str| t.contains(w);
    let above = has("above") || has(" up") || has("high") || has("over");
    let dir = |step: i32| if above { step } else { -step };
    let mut voices: Vec<Voice> = vec![];
    fn push(v: Voice, voices: &mut Vec<Voice>) {
        if !voices.contains(&v) {
            voices.push(v);
        }
    }
    if has("third") || has("3rd") {
        push(Voice::Steps(dir(2)), &mut voices);
    }
    if has("fourth") || has("4th") {
        push(Voice::Steps(dir(3)), &mut voices);
    }
    if has("fifth") || has("5th") || has("power") {
        push(Voice::Steps(dir(4)), &mut voices);
    }
    if has("sixth") || has("6th") {
        push(Voice::Steps(dir(5)), &mut voices);
    }
    if has("octave") || has("double") || has("doubl") {
        push(Voice::Steps(dir(7)), &mut voices);
    }
    if has("counter") || has("contrary") || has("answer") {
        push(Voice::Counter, &mut voices);
    }
    if has("choir") || has("choral") || has("satb") || has("four-part") || has("four part") {
        for v in [Voice::Stack(1), Voice::Stack(2), Voice::Bass] {
            push(v, &mut voices);
        }
    }
    if has("jazz") || has("gospel") || has("extended") || has("lush") {
        for v in [Voice::Stack(1), Voice::Stack(2), Voice::Stack(3)] {
            push(v, &mut voices);
        }
    }
    if has("triad") || has("chord") || has("stack") || has("block") || has("close") {
        for v in [Voice::Stack(1), Voice::Stack(2)] {
            push(v, &mut voices);
        }
    }
    if has("bass") && !has("choir") {
        push(Voice::Bass, &mut voices);
    }
    // a count with nothing more specific: that many voices from the usual order
    if voices.is_empty() {
        let n = if has("four") || has("4 ") {
            4
        } else if has("three") || has("3 ") {
            3
        } else if has("two") || has("2 ") || has("duet") {
            2
        } else {
            1
        };
        let usual = [Voice::Steps(dir(2)), Voice::Steps(dir(4)), Voice::Steps(dir(-2)), Voice::Steps(7)];
        for v in usual.iter().take(n) {
            push(*v, &mut voices);
        }
    }
    voices.truncate(4);
    let level = if has("soft") || has("quiet") || has("subtle") || has("gentle") {
        0.6
    } else if has("loud") || has("strong") || has("full") {
        0.95
    } else {
        0.8
    };
    Spec { voices, sustain: has("sustain") || has("held") || has("legato") || has("pad") || has("smooth"), level }
}

fn key_at(keys: &[KeySection], main: Key, beat: f64) -> Key {
    keys.iter().find(|k| beat >= k.start - 1e-6 && beat < k.end - 1e-6).map(|k| k.key).unwrap_or(main)
}

fn intervals(key: Key) -> [i32; 7] {
    let sc = key.scale();
    let mut out = [0i32; 7];
    for i in 0..7 {
        out[i] = ((sc[i] as i32 + 12 - key.tonic as i32) % 12) as i32;
    }
    out
}

/// the pitch `steps` scale degrees from `midi` (which is first moved to the nearest scale tone)
pub fn scale_step(key: Key, midi: i32, steps: i32) -> i32 {
    let iv = intervals(key);
    let rel = (midi - key.tonic as i32).rem_euclid(12);
    let oct = (midi - key.tonic as i32).div_euclid(12);
    // nearest scale degree (ties go down)
    let mut best = 0usize;
    let mut bd = 99;
    for (d, i) in iv.iter().enumerate() {
        let dist = (rel - i).abs().min(12 - (rel - i).abs());
        if dist < bd {
            bd = dist;
            best = d;
        }
    }
    let abs = oct * 7 + best as i32 + steps;
    key.tonic as i32 + abs.div_euclid(7) * 12 + iv[abs.rem_euclid(7) as usize]
}

fn clashes(mel: i32, v: i32) -> bool {
    let d = (mel - v).abs() % 12;
    // only close intervals grate; a major seventh a ninth-and-more apart is just a wide chord
    ((d == 1 || d == 11 || d == 6) && (mel - v).abs() < 12) || mel == v
}

/// the chord tone nearest to `target` that does not clash with the melody; None when no chord tone is within 2 semitones
fn nearest_chord_tone(chord: &Chord, target: i32, mel: i32, lo: i32, hi: i32) -> Option<i32> {
    let mut best: Option<(i32, i32)> = None;
    for d in -2..=2 {
        let c = target + d;
        if c < lo || c > hi || !chord.has(c.rem_euclid(12) as u8) || clashes(mel, c) {
            continue;
        }
        let cost = d.abs();
        if best.map_or(true, |b| cost < b.1) {
            best = Some((c, cost));
        }
    }
    best.map(|b| b.0)
}

/// the n-th (1 = nearest) distinct chord tone below `mel`
fn stack_tone(chord: &Chord, mel: i32, n: usize) -> Option<i32> {
    let mut found = 0;
    let mut p = mel - 1;
    while p > mel - 24 {
        if chord.has(p.rem_euclid(12) as u8) && p.rem_euclid(12) != mel.rem_euclid(12) {
            found += 1;
            if found == n {
                return Some(p);
            }
        }
        p -= 1;
    }
    None
}

/// Write the harmony of `melody` (absolute beats, ordered by time) for every voice of `spec`. The result is in absolute beats too.
pub fn harmonize(melody: &[N], chords: &[Chord], keys: &[KeySection], main: Key, spec: &Spec) -> Vec<N> {
    let mut mel: Vec<N> = melody.to_vec();
    mel.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap().then(a.key.cmp(&b.key)));
    let mut out = vec![];
    for voice in spec.voices.iter() {
        let mut prev: Option<(i32, i32)> = None; // (melody pitch, voice pitch) of the last note, for the counter line
        for (i, n) in mel.iter().enumerate() {
            // only the top note of a chord of melody notes is harmonised
            if mel.iter().any(|o| (o.start - n.start).abs() < 0.02 && o.key > n.key) {
                continue;
            }
            let key = key_at(keys, main, n.start);
            let chord = chord_at(chords, n.start + 0.01);
            let m = n.key as i32;
            let below = matches!(voice, Voice::Stack(_) | Voice::Bass | Voice::Counter) || matches!(voice, Voice::Steps(s) if *s < 0);
            let (lo, hi) = if below { (m - 19, m - 3) } else { (m + 3, m + 19) };
            let mut p = match voice {
                Voice::Steps(s) => {
                    let mut p = scale_step(key, m, *s);
                    if let Some(c) = chord {
                        if !c.has(p.rem_euclid(12) as u8) {
                            if let Some(ct) = nearest_chord_tone(c, p, m, lo, hi) {
                                p = ct;
                            }
                        }
                    }
                    // a unison or an octave is only wanted when asked for
                    if *s != 0 && s.abs() != 7 && (p - m).abs() < 3 {
                        p = scale_step(key, p, if below { -1 } else { 1 });
                    }
                    p
                }
                Voice::Stack(k) => match chord.and_then(|c| stack_tone(c, m, *k)) {
                    Some(p) => p,
                    None => scale_step(key, m, -2 * *k as i32),
                },
                Voice::Bass => match chord {
                    Some(c) => {
                        let mut p = c.root as i32 + 12 * ((m - 12) / 12);
                        while p > m - 12 {
                            p -= 12;
                        }
                        while p < m - 24 {
                            p += 12;
                        }
                        p
                    }
                    None => scale_step(key, m, -7),
                },
                Voice::Counter => {
                    let steps = match prev {
                        Some((pm, pv)) => {
                            // the voice answers the melody's move in the other direction, at most four steps
                            let dm = m - pm;
                            let target = (pv as f64 - dm as f64 * 0.8).round() as i32;
                            let target = target.clamp(m - 17, m - 3);
                            let snapped = scale_step(key, target, 0);
                            let mut p = snapped;
                            if let Some(c) = chord {
                                if !c.has(p.rem_euclid(12) as u8) {
                                    if let Some(ct) = nearest_chord_tone(c, p, m, lo, hi) {
                                        p = ct;
                                    }
                                }
                            }
                            p
                        }
                        None => match chord.and_then(|c| stack_tone(c, m, 1)) {
                            Some(p) => p,
                            None => scale_step(key, m, -2),
                        },
                    };
                    steps
                }
            };
            // steer off a clash with the melody
            if clashes(m, p) && !matches!(voice, Voice::Steps(s) if s.abs() == 7 || *s == 0) {
                let q = scale_step(key, p, if below { -1 } else { 1 });
                if !clashes(m, q) {
                    p = q;
                }
            }
            prev = Some((m, p));
            if !(0..=127).contains(&p) {
                continue;
            }
            let mut len = n.len;
            if spec.sustain {
                if let Some(next) = mel[i + 1..].iter().find(|o| o.start > n.start + 0.05) {
                    len = len.max((next.start - n.start - 0.02).max(0.05));
                }
            }
            out.push(N { start: n.start, len, key: p as u8, vel: (n.vel * spec.level).clamp(0.05, 1.0) });
        }
    }
    out.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap().then(a.key.cmp(&b.key)));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::theory::{detect_chords, Key};

    fn n(start: f64, key: u8) -> N {
        N { start, len: 0.9, key, vel: 0.8 }
    }

    fn cmaj_melody() -> Vec<N> {
        // C D E F | G E D C over C | F | G | C
        [(0.0, 72), (1.0, 74), (2.0, 76), (3.0, 77), (4.0, 79), (5.0, 76), (6.0, 74), (7.0, 72)].iter().map(|(s, k)| n(*s, *k)).collect()
    }

    fn pad() -> Vec<N> {
        let mut v = vec![];
        for (bar, ch) in [[48, 52, 55], [53, 57, 60], [43, 47, 50], [48, 52, 55]].iter().enumerate() {
            for k in ch {
                v.push(N { start: bar as f64 * 2.0, len: 2.0, key: *k, vel: 0.7 });
            }
        }
        v
    }

    #[test]
    fn the_prompt_picks_the_voices() {
        assert_eq!(parse_prompt("").voices, vec![Voice::Steps(-2)]);
        assert_eq!(parse_prompt("sixths above").voices, vec![Voice::Steps(5)]);
        assert_eq!(parse_prompt("thirds and fifths").voices, vec![Voice::Steps(-2), Voice::Steps(-4)]);
        assert_eq!(parse_prompt("choir").voices, vec![Voice::Stack(1), Voice::Stack(2), Voice::Bass]);
        assert!(parse_prompt("counter melody, soft, held").voices.contains(&Voice::Counter));
        let s = parse_prompt("soft held thirds");
        assert!(s.sustain && s.level < 0.7);
        assert_eq!(parse_prompt("three voices").voices.len(), 3);
        assert!(parse_prompt("two voices").summary().starts_with("2 voices"));
    }

    #[test]
    fn scale_steps_stay_in_the_key() {
        let k = Key::new(0, false);
        assert_eq!(scale_step(k, 72, -2), 69); // a third below C is A
        assert_eq!(scale_step(k, 72, 2), 76);
        assert_eq!(scale_step(k, 72, 7), 84);
        assert_eq!(scale_step(k, 61, 0), 60, "C# is moved to the nearest scale tone");
        assert_eq!(scale_step(Key::new(9, true), 69, -2), 65); // a third below A in A minor is F
    }

    #[test]
    fn a_third_below_is_in_key_and_never_clashes() {
        let m = cmaj_melody();
        let mut all = m.clone();
        all.extend(pad());
        let chords = detect_chords(&pad(), 8.0, 2.0, false);
        let spec = parse_prompt("");
        let h = harmonize(&m, &chords, &[], Key::new(0, false), &spec);
        assert_eq!(h.len(), m.len());
        let k = Key::new(0, false);
        for (hn, mn) in h.iter().zip(&m) {
            assert!(k.in_scale(hn.key % 12), "{hn:?} out of key");
            assert!(hn.key < mn.key, "below the melody");
            assert!(!clashes(mn.key as i32, hn.key as i32), "{hn:?} clashes with {mn:?}");
            assert!((hn.vel - 0.64).abs() < 1e-6);
        }
        // deterministic
        assert_eq!(h, harmonize(&m, &chords, &[], Key::new(0, false), &spec));
    }

    #[test]
    fn a_stack_follows_the_chords() {
        let m = cmaj_melody();
        let chords = detect_chords(&pad(), 8.0, 2.0, false);
        let h = harmonize(&m, &chords, &[], Key::new(0, false), &parse_prompt("close chord stack"));
        assert_eq!(h.len(), m.len() * 2);
        for x in &h {
            let c = chord_at(&chords, x.start + 0.01).expect("a chord");
            assert!(c.has(x.key % 12), "{x:?} is not a tone of {c:?}");
        }
    }

    #[test]
    fn choir_counter_and_sustain() {
        let m = cmaj_melody();
        let chords = detect_chords(&pad(), 8.0, 2.0, false);
        let h = harmonize(&m, &chords, &[], Key::new(0, false), &parse_prompt("choir"));
        assert_eq!(h.len(), m.len() * 3);
        // the bass voice is the lowest of each moment, and a chord root
        for mn in &m {
            let low = h.iter().filter(|x| (x.start - mn.start).abs() < 1e-9).map(|x| x.key).min().unwrap();
            let c = chord_at(&chords, mn.start + 0.01).unwrap();
            assert_eq!(low % 12, c.root, "bass at {}", mn.start);
        }
        let ctr = harmonize(&m, &chords, &[], Key::new(0, false), &parse_prompt("counter melody"));
        // the melody rises C D E F and the counter line falls (or holds) while it does
        assert!(ctr[3].key <= ctr[0].key + 2, "{:?}", ctr.iter().map(|x| x.key).collect::<Vec<_>>());
        let held = harmonize(&[n(0.0, 72), n(2.0, 74)], &[], &[], Key::new(0, false), &parse_prompt("held thirds"));
        assert!(held[0].len > 1.9 && held[0].len < 2.0, "{held:?}");
    }

    #[test]
    fn simultaneous_melody_notes_are_harmonised_from_the_top_note() {
        let m = vec![n(0.0, 72), n(0.0, 64), n(1.0, 74)];
        let h = harmonize(&m, &[], &[], Key::new(0, false), &parse_prompt(""));
        assert_eq!(h.len(), 2);
    }
}
