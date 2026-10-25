//! The decorator ("spicer"): takes a plain sequence of notes or chords and dresses it up while staying inside music theory, and
//! turns a short theme into variations of itself.
//!
//! * [`decorate`] - a melody gains passing tones between notes a third apart, neighbour notes, turns on long notes, 4-3 suspensions
//!   into chord changes, rhythmic diminution and the odd octave leap; a chord part gains voice-led inversions, the 7th / 9th the
//!   key gives the chord, and broken-chord figures. Every added note is a scale tone, and every note that starts on a beat is a
//!   tone of the chord sounding there (the chords are read from the notes themselves when none are given). `amount` 0..1 says how
//!   much is added; the same seed gives the same result.
//! * [`variations`] - theme and variations: the theme as it is, decorated, sequenced a step up, inverted, in augmentation (twice as
//!   slow) or diminution (twice as fast, played twice), and a last variation that combines decoration and sequence - each fitted
//!   back onto the chords so it still belongs to the key.
use crate::compose::Rng;
use crate::theory::{self, Key, N};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Shape {
    Melody,
    Chords,
}

/// melody (one note at a time, mostly) or chords (three or more notes start together)
pub fn shape_of(notes: &[N]) -> Shape {
    let mut starts: Vec<i64> = notes.iter().map(|n| (n.start * 48.0).round() as i64).collect();
    starts.sort_unstable();
    let groups = {
        let mut g = starts.clone();
        g.dedup();
        g.len().max(1)
    };
    if notes.len() as f64 / groups as f64 >= 2.5 { Shape::Chords } else { Shape::Melody }
}

/// the scale tones of the key over the MIDI range
fn scale(key: Key) -> Vec<u8> {
    (0..128u8).filter(|k| key.in_scale(k % 12)).collect()
}
fn step(key: Key, k: u8, by: i32) -> u8 {
    let sc = scale(key);
    let i = sc.iter().rposition(|p| *p <= k).unwrap_or(0) as i32;
    sc[(i + by).clamp(0, sc.len() as i32 - 1) as usize]
}

/// the chords under the notes: given ones, or read from the notes a beat at a time (3+ notes sounding = a chord, else implied)
fn harmony_of(notes: &[N], given: &[(f64, f64, u8, usize)], total: f64) -> Vec<theory::Chord> {
    if !given.is_empty() {
        return given.iter().map(|(s, l, r, q)| theory::Chord { start: *s, len: *l, root: *r, q: *q, bass: *r, implied: false }).collect();
    }
    theory::detect_chords(notes, total.max(1.0), 1.0, true)
}

fn chord_at(ch: &[theory::Chord], t: f64) -> Option<&theory::Chord> {
    ch.iter().find(|c| t >= c.start - 1e-6 && t < c.start + c.len - 1e-6)
}

/// the nearest tone of `c` to `k` that the key also has (a chord read from a melody can be one the key does not own)
fn to_chord(c: &theory::Chord, k: u8, key: Key) -> u8 {
    let ok = |x: u8| c.has(x % 12) && key.accepts(x % 12);
    if ok(k) {
        return k;
    }
    (1..=6i32).flat_map(|d| [-d, d]).map(|d| k as i32 + d).find(|x| (0..128).contains(x) && ok(*x as u8)).map(|x| x as u8).unwrap_or(if key.accepts(k % 12) { k } else { step(key, k, 0) })
}

/// every note on a beat onto the chord there, everything else onto the scale
fn fit(notes: &mut [N], key: Key, ch: &[theory::Chord]) {
    for n in notes.iter_mut() {
        let on_beat = (n.start - n.start.round()).abs() < 0.02;
        match chord_at(ch, n.start + 1e-6) {
            Some(c) if on_beat => n.key = to_chord(c, n.key, key),
            _ => {
                if !key.accepts(n.key % 12) {
                    n.key = step(key, n.key, 0);
                }
            }
        }
    }
}

fn sorted(mut v: Vec<N>) -> Vec<N> {
    v.sort_by(|a, b| a.start.total_cmp(&b.start).then(a.key.cmp(&b.key)));
    v
}

fn decorate_melody(src: &[N], key: Key, ch: &[theory::Chord], amount: f64, rng: &mut Rng) -> Vec<N> {
    let mut out: Vec<N> = vec![];
    let n = src.len();
    for i in 0..n {
        let a = src[i];
        let next = src.get(i + 1).copied();
        let gap_to_next = next.map(|b| b.start - a.start).unwrap_or(a.len);
        let mut head = a;
        // a 4-3 suspension: a note landing on a new chord leans on the 4th above the chord's 3rd first
        if amount > 0.3 && a.len >= 1.0 && rng.chance(amount * 0.25) {
            if let Some(c) = chord_at(ch, a.start + 1e-6) {
                if (a.start - c.start).abs() < 0.02 && c.has(a.key % 12) {
                    let sus = step(key, a.key, 1);
                    let half = (a.len * 0.5 / 0.25).round() * 0.25;
                    out.push(N { start: a.start, len: half, key: sus, vel: a.vel });
                    head = N { start: a.start + half, len: a.len - half, ..a };
                    out.push(head);
                    continue;
                }
            }
        }
        // a turn on a long note: upper neighbour, the note, lower neighbour, the note
        if a.len >= 1.5 && rng.chance(amount * 0.3) {
            let q = 0.25;
            let figure = [step(key, a.key, 1), a.key, step(key, a.key, -1), a.key];
            for (j, k) in figure.iter().enumerate() {
                out.push(N { start: a.start + j as f64 * q, len: if j == 3 { a.len - 3.0 * q } else { q }, key: *k, vel: a.vel * if j == 0 { 1.0 } else { 0.85 } });
            }
            continue;
        }
        // rhythmic diminution: a long note becomes the note and its neighbour, or repeated in eighths
        if a.len >= 2.0 && rng.chance(amount * 0.3) {
            let parts = (a.len / 0.5) as usize;
            for j in 0..parts {
                let k = if j % 2 == 1 && rng.chance(0.5) { step(key, a.key, if rng.chance(0.5) { 1 } else { -1 }) } else { a.key };
                out.push(N { start: a.start + j as f64 * 0.5, len: 0.45, key: k, vel: a.vel * if j == 0 { 1.0 } else { 0.8 } });
            }
            continue;
        }
        // an octave leap now and then (back home by the next note)
        if amount > 0.5 && rng.chance(amount * 0.08) && a.key + 12 <= 100 {
            head.key += 12;
        }
        // the note itself, shortened when a passing or neighbour tone follows
        if let Some(b) = next {
            let dist = b.key as i32 - a.key as i32;
            let room = gap_to_next >= 1.0;
            if room && (3..=4).contains(&dist.abs()) && rng.chance(0.3 + amount * 0.5) {
                // a passing tone between notes a third apart, on the off-beat
                let mid = a.start + gap_to_next * 0.5;
                out.push(N { len: (mid - a.start).min(head.len), ..head });
                out.push(N { start: mid, len: gap_to_next * 0.5 * 0.95, key: step(key, a.key, dist.signum()), vel: a.vel * 0.85 });
                continue;
            }
            if room && dist == 0 && rng.chance(0.2 + amount * 0.4) {
                // a neighbour tone between two equal notes
                let mid = a.start + gap_to_next * 0.5;
                out.push(N { len: (mid - a.start).min(head.len), ..head });
                out.push(N { start: mid, len: gap_to_next * 0.5 * 0.95, key: step(key, a.key, if rng.chance(0.6) { 1 } else { -1 }), vel: a.vel * 0.85 });
                continue;
            }
        }
        out.push(head);
    }
    out
}

fn decorate_chords(src: &[N], key: Key, amount: f64, rng: &mut Rng) -> Vec<N> {
    // group by onset
    let mut groups: Vec<Vec<N>> = vec![];
    for n in sorted(src.to_vec()) {
        match groups.last_mut() {
            Some(g) if (g[0].start - n.start).abs() < 0.03 => g.push(n),
            _ => groups.push(vec![n]),
        }
    }
    let mut out = vec![];
    let mut prev: Vec<u8> = vec![];
    for g in groups {
        let st = g[0].start;
        let len = g.iter().map(|n| n.len).fold(0.0, f64::max);
        let vel = g.iter().map(|n| n.vel).fold(0.0, f32::max);
        let keys: Vec<u8> = g.iter().map(|n| n.key).collect();
        let (lo, hi) = (*keys.iter().min().unwrap_or(&48), *keys.iter().max().unwrap_or(&72));
        // what chord is it: the best triad of these pitch classes
        let mut w = [0f64; 12];
        for k in &keys {
            w[(*k % 12) as usize] += 1.0;
        }
        let root_pc = keys.iter().min().map(|k| k % 12).unwrap_or(key.tonic);
        let deg = key.scale().iter().position(|p| *p == root_pc);
        // the key's own seventh (and ninth) on that degree
        let mut pcs: Vec<u8> = keys.iter().map(|k| k % 12).collect();
        if let Some(d) = deg {
            let s = key.scale();
            if rng.chance(0.3 + amount * 0.5) {
                pcs.push(s[(d + 6) % 7]);
            }
            if amount > 0.6 && rng.chance(amount * 0.4) {
                pcs.push(s[(d + 1) % 7]);
            }
        }
        pcs.sort_unstable();
        pcs.dedup();
        // voice-led into the range the chord had (the nearest notes to the last chord)
        let voiced: Vec<u8> = pcs
            .iter()
            .map(|pc| {
                let cands: Vec<u8> = (lo.saturating_sub(5)..=hi.saturating_add(5).min(120)).filter(|k| k % 12 == *pc).collect();
                let target = prev.iter().copied().min_by_key(|p| cands.iter().map(|c| (*c as i32 - *p as i32).abs()).min().unwrap_or(99)).unwrap_or((lo + hi) / 2);
                cands.iter().copied().min_by_key(|c| (*c as i32 - target as i32).abs()).unwrap_or(lo)
            })
            .collect();
        let mut voiced = voiced;
        voiced.sort_unstable();
        voiced.dedup();
        // a broken-chord figure on long chords, otherwise a block
        if len >= 2.0 && rng.chance(amount * 0.5) {
            let stepl = 0.5;
            let count = (len / stepl) as usize;
            for j in 0..count {
                let k = voiced[if j % 4 == 3 { 1 % voiced.len() } else { j % voiced.len() }];
                out.push(N { start: st + j as f64 * stepl, len: stepl * 0.9, key: k, vel: vel * if j == 0 { 1.0 } else { 0.8 } });
            }
            // the root held under the figure
            out.push(N { start: st, len, key: voiced[0], vel: vel * 0.75 });
        } else {
            for k in &voiced {
                out.push(N { start: st, len, key: *k, vel });
            }
        }
        prev = voiced;
    }
    out
}

/// The notes dressed up (see the module notes). `harmony`: chords to follow (start, len, root pc, quality), or empty to read them
/// from the notes. Clip-relative beats in, the same out; the length is unchanged.
pub fn decorate(notes: &[N], key: Key, harmony: &[(f64, f64, u8, usize)], amount: f64, seed: u64) -> Vec<N> {
    if notes.is_empty() {
        return vec![];
    }
    let amount = amount.clamp(0.0, 1.0);
    let mut rng = Rng::new(seed.wrapping_mul(0x5DEC) ^ 0xD3C0);
    let total = notes.iter().map(|n| n.start + n.len).fold(0.0, f64::max);
    let ch = harmony_of(notes, harmony, total);
    let src = sorted(notes.to_vec());
    let mut out = match shape_of(&src) {
        Shape::Melody => decorate_melody(&src, key, &ch, amount, &mut rng),
        Shape::Chords => decorate_chords(&src, key, amount, &mut rng),
    };
    if shape_of(&src) == Shape::Melody {
        fit(&mut out, key, &ch);
    }
    out.retain(|n| n.start < total - 1e-6 && n.len > 0.01);
    for n in out.iter_mut() {
        n.len = n.len.min(total - n.start);
    }
    sorted(out)
}

/// The kinds of variation, in the order [`variations`] uses them.
pub const KINDS: [&str; 7] = ["theme", "decorated", "sequence", "inversion", "augmentation", "diminution", "finale"];

/// Theme and variations: `count` versions of the theme (the first is the theme itself), each as long as the theme (augmentation
/// keeps the first half of the theme at twice the length), fitted onto the theme's chords.
pub fn variations(theme: &[N], key: Key, count: usize, amount: f64, seed: u64) -> Vec<(String, Vec<N>)> {
    let theme = sorted(theme.to_vec());
    if theme.is_empty() {
        return vec![];
    }
    let total = theme.iter().map(|n| n.start + n.len).fold(0.0, f64::max);
    let len = (total / 4.0).ceil().max(1.0) * 4.0;
    let ch = harmony_of(&theme, &[], len);
    let melody = shape_of(&theme) == Shape::Melody;
    let pivot = theme[0].key;
    let mut out = vec![];
    for i in 0..count {
        let kind = KINDS[i.min(KINDS.len() - 1)];
        let s = seed.wrapping_add(i as u64 * 7919);
        let mut v: Vec<N> = match kind {
            "theme" => theme.clone(),
            "decorated" => decorate(&theme, key, &[], amount, s),
            "sequence" => theme.iter().map(|n| N { key: step(key, n.key, 1), ..*n }).collect(),
            "inversion" if melody => {
                let sc = scale(key);
                let at = |x: u8| sc.iter().rposition(|p| *p <= x).unwrap_or(0) as i32;
                theme.iter().map(|n| N { key: sc[(2 * at(pivot) - at(n.key)).clamp(0, sc.len() as i32 - 1) as usize], ..*n }).collect()
            }
            "inversion" => theme.iter().map(|n| N { key: if n.key + 12 <= 108 && n.key < pivot + 5 { n.key + 12 } else { n.key }, ..*n }).collect(),
            "augmentation" => theme.iter().filter(|n| n.start < len * 0.5).map(|n| N { start: n.start * 2.0, len: n.len * 2.0, ..*n }).collect(),
            "diminution" => {
                let half: Vec<N> = theme.iter().map(|n| N { start: n.start * 0.5, len: n.len * 0.5, ..*n }).collect();
                half.iter().copied().chain(half.iter().map(|n| N { start: n.start + len * 0.5, ..*n })).collect()
            }
            _ => {
                let d = decorate(&theme, key, &[], (amount + 0.25).min(1.0), s);
                d.iter().map(|n| N { key: step(key, n.key, if n.start >= len * 0.5 { 1 } else { 0 }), ..*n }).collect()
            }
        };
        // a sequence moves its harmony with it: it only has to stay in the key; the others keep the theme's chords
        if melody && kind != "theme" {
            if kind == "sequence" || kind == "finale" {
                for n in v.iter_mut() {
                    if !key.accepts(n.key % 12) {
                        n.key = step(key, n.key, 0);
                    }
                }
            } else {
                fit(&mut v, key, &ch);
            }
        }
        v.retain(|n| n.start < len - 1e-6);
        out.push((kind.to_string(), sorted(v)));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn melody() -> Vec<N> {
        // C major: C E G E | F A G - (quarters, then a long G)
        [(0.0, 60u8), (1.0, 64), (2.0, 67), (3.0, 64), (4.0, 65), (5.0, 69), (6.0, 67)].iter().map(|(s, k)| N { start: *s, len: if *s >= 6.0 { 2.0 } else { 1.0 }, key: *k, vel: 0.8 }).collect()
    }

    #[test]
    fn a_decorated_melody_is_richer_and_stays_in_key_on_its_chords() {
        let key = Key::new(0, false);
        let m = melody();
        let d = decorate(&m, key, &[], 0.8, 3);
        assert!(d.len() > m.len(), "{} -> {}", m.len(), d.len());
        assert!(d.iter().all(|n| key.accepts(n.key % 12)), "{d:?}");
        assert!(d.iter().all(|n| n.start < 8.0 && n.start + n.len <= 8.0 + 1e-9));
        // amount 0 adds little or nothing
        assert!(decorate(&m, key, &[], 0.0, 3).len() <= m.len() + 2);
        // the same seed, the same result
        assert_eq!(decorate(&m, key, &[], 0.8, 3).len(), d.len());
    }

    #[test]
    fn chords_get_sevenths_and_voice_leading() {
        let key = Key::new(0, false);
        let mut c = vec![];
        for (i, triad) in [[60u8, 64, 67], [57, 60, 64], [53, 57, 60], [55, 59, 62]].iter().enumerate() {
            for k in triad {
                c.push(N { start: i as f64 * 4.0, len: 4.0, key: *k, vel: 0.7 });
            }
        }
        let d = decorate(&c, key, &[], 0.9, 1);
        assert!(d.iter().all(|n| key.accepts(n.key % 12)));
        assert!(d.len() >= c.len(), "{} -> {}", c.len(), d.len());
    }

    #[test]
    fn a_theme_comes_back_in_variations() {
        let key = Key::new(0, false);
        let v = variations(&melody(), key, 7, 0.6, 9);
        assert_eq!(v.len(), 7);
        assert_eq!(v[0].1.len(), melody().len(), "the theme itself first");
        for (kind, notes) in &v {
            assert!(!notes.is_empty(), "{kind}");
            assert!(notes.iter().all(|n| key.accepts(n.key % 12) && n.start < 8.0), "{kind}: {notes:?}");
        }
        // the sequence is the theme a step higher (fitted to the chords on the beats)
        assert_ne!(v[2].1.iter().map(|n| n.key).collect::<Vec<_>>(), v[0].1.iter().map(|n| n.key).collect::<Vec<_>>());
    }
}
