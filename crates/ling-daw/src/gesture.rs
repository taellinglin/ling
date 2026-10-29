//! Per-note expression: pitch gestures (scoop, fall, slide, dive, riser), pressure (polyphonic aftertouch) and timbre (MPE slide)
//! as curves on the note itself (`Note::bend` / `press` / `timbre`, model.rs).
//!
//! A curve is a list of (position 0..1 of the note's length, value) breakpoints, linear between them and held before the first
//! and after the last; empty = none. Positions are fractions of the note, so a gesture follows the tempo (and the tempo map) and
//! stretches with the note; cutting a note (`Note::cut`) keeps the part of the curve that sounded there. Pitch values are
//! semitones from the note's key, pressure 0..1 (0 = none), timbre 0..1 (0.5 = neutral).
//!
//! The engine (engine.rs) hands the value at each render block to the instrument (`Instrument::note_pitch`, `note_pressure`,
//! `note_timbre`); the instrument smooths it inside the block. Gestures are written on purpose only (the pitch guard,
//! redesign.rs): by the user in the piano roll, by recipes whose composition.pitch asks for scoops / slides / dives / risers, by
//! the song generator's expression rules and by the Project Agent when the request names one.
use crate::model::{Curve, Note};

/// The most breakpoints a curve keeps (a freehand stroke is thinned to this).
pub const MAX_POINTS: usize = 64;
/// Pitch gestures stay within four octaves either way.
pub const MAX_SEMIS: f32 = 48.0;

/// The named pitch shapes the piano roll, the agent and the generators write.
pub const SHAPES: [&str; 5] = ["scoop", "fall", "slide", "dive", "riser"];

/// The value of a curve at `x` (0..1 of the note); 0 for an empty curve.
pub fn at(c: &[(f32, f32)], x: f32) -> f32 {
    match c {
        [] => 0.0,
        [p] => p.1,
        _ => {
            if x <= c[0].0 {
                return c[0].1;
            }
            for w in c.windows(2) {
                if x <= w[1].0 {
                    let span = w[1].0 - w[0].0;
                    if span <= 1e-9 {
                        return w[1].1;
                    }
                    return w[0].1 + (w[1].1 - w[0].1) * (x - w[0].0) / span;
                }
            }
            c[c.len() - 1].1
        }
    }
}

/// A curve as stored: finite points, positions clamped to 0..1 and in order, values within range, at most MAX_POINTS
/// (a longer one is thinned evenly, keeping both ends).
pub fn tidy(c: &[(f32, f32)]) -> Curve {
    let mut v: Curve = c.iter().filter(|p| p.0.is_finite() && p.1.is_finite()).map(|p| (p.0.clamp(0.0, 1.0), p.1.clamp(-MAX_SEMIS, MAX_SEMIS))).collect();
    v.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    if v.len() > MAX_POINTS {
        let n = v.len();
        v = (0..MAX_POINTS).map(|i| v[i * (n - 1) / (MAX_POINTS - 1)]).collect();
    }
    v
}

/// The part of a curve between positions `a` and `b` (0..1), stretched back over 0..1 (what a cut note keeps).
pub fn trim(c: &[(f32, f32)], a: f32, b: f32) -> Curve {
    if c.is_empty() {
        return vec![];
    }
    let (a, b) = (a.clamp(0.0, 1.0), b.clamp(0.0, 1.0));
    if b - a < 1e-6 {
        return vec![(0.0, at(c, a))];
    }
    let mut v = vec![(0.0, at(c, a))];
    v.extend(c.iter().filter(|p| p.0 > a + 1e-6 && p.0 < b - 1e-6).map(|p| ((p.0 - a) / (b - a), p.1)));
    v.push((1.0, at(c, b)));
    // a flat piece is one point
    if v.iter().all(|p| (p.1 - v[0].1).abs() < 1e-6) {
        v.truncate(1);
    }
    v
}

/// The largest |value| of a pitch curve (its depth in semitones).
pub fn depth(c: &[(f32, f32)]) -> f32 {
    c.iter().fold(0.0f32, |m, p| m.max(p.1.abs()))
}

/// Does the curve ever leave `rest` (a pitch curve that stays at 0 is no gesture)?
pub fn moves(c: &[(f32, f32)], rest: f32) -> bool {
    c.iter().any(|p| (p.1 - rest).abs() > 1e-4)
}

/// How deep a shape goes by default (semitones; a slide goes to the next note).
pub fn default_depth(shape: &str) -> f32 {
    match shape {
        "dive" => 12.0,
        "riser" => 12.0,
        "fall" => 3.0,
        "slide" => 2.0,
        _ => 3.0, // scoop
    }
}

/// A named pitch shape for a note `len_beats` long: `depth` semitones (the sign of `depth` is taken from the shape, except for a
/// slide, which goes `depth` semitones up or down: the distance to the next note). Times are musical: a scoop rises within a
/// sixteenth to an eighth, a fall drops over the last eighth to quarter, a dive over the last 60 % of the note, a riser over all
/// of it. None for an unknown name.
pub fn shape(name: &str, depth: f32, len_beats: f64) -> Option<Curve> {
    let len = len_beats.max(0.01) as f32;
    let d = depth.abs().min(MAX_SEMIS);
    let frac = |beats: f32, most: f32| (beats / len).min(most).max(0.02);
    let c: Curve = match name {
        // up into the note from below, fast then settling (ease-out)
        "scoop" => {
            let t = frac(0.125 + 0.03 * d.min(5.0), 0.45);
            vec![(0.0, -d), (t * 0.35, -d * 0.45), (t * 0.7, -d * 0.12), (t, 0.0)]
        }
        // away from the note at its end, slow then faster (ease-in)
        "fall" => {
            let t = frac(0.375, 0.5);
            vec![(1.0 - t, 0.0), (1.0 - t * 0.6, -d * 0.12), (1.0 - t * 0.3, -d * 0.42), (1.0, -d)]
        }
        // a screech / bass dive: a long accelerating drop
        "dive" => {
            let t = frac(len * 0.6, 0.85);
            vec![(1.0 - t, 0.0), (1.0 - t * 0.66, -d * 0.1), (1.0 - t * 0.33, -d * 0.38), (1.0, -d)]
        }
        // the whole note climbs (x^2), the riser into a drop
        "riser" => (0..=6).map(|i| {
            let x = i as f32 / 6.0;
            (x, d * x * x)
        }).collect(),
        // a portamento into the next note over the end of this one (ease-out, the 303 / 808 slide)
        "slide" => {
            let s = depth.clamp(-MAX_SEMIS, MAX_SEMIS);
            let t = frac(0.2, 0.4);
            vec![(1.0 - t, 0.0), (1.0 - t * 0.6, s * 0.55), (1.0 - t * 0.25, s * 0.9), (1.0, s)]
        }
        _ => return None,
    };
    Some(c)
}

/// The next note a slide from note `i` goes to: the earliest note starting after `i` starts (and no later than a beat after it
/// ends), nearest in pitch among those starting together. Returns the semitone distance.
pub fn next_interval(notes: &[Note], i: usize) -> Option<f32> {
    let n = notes.get(i)?;
    let end = n.start + n.len;
    let mut best: Option<&Note> = None;
    for (j, m) in notes.iter().enumerate() {
        if j == i || m.start <= n.start + 1e-6 || m.start > end + 1.0 {
            continue;
        }
        best = match best {
            None => Some(m),
            Some(b) if m.start < b.start - 1e-6 => Some(m),
            Some(b) if (m.start - b.start).abs() <= 1e-6 && (m.key as i32 - n.key as i32).abs() < (b.key as i32 - n.key as i32).abs() => Some(m),
            keep => keep,
        };
    }
    best.map(|m| m.key as f32 - n.key as f32)
}

/// Give note `i` a named pitch shape (`depth` <= 0 = the shape's default; a slide goes to the next note, or `depth` up when there
/// is none); "clear" / "none" removes the gesture. False for an unknown shape or index.
pub fn apply_shape(notes: &mut [Note], i: usize, name: &str, depth: f32) -> bool {
    if i >= notes.len() {
        return false;
    }
    if name == "clear" || name == "none" || name.is_empty() {
        notes[i].bend.clear();
        return true;
    }
    let d = if name == "slide" {
        match next_interval(notes, i) {
            Some(iv) if iv.abs() > 0.01 => iv,
            _ => if depth > 0.0 { depth } else { default_depth("slide") },
        }
    } else if depth > 0.0 {
        depth
    } else {
        default_depth(name)
    };
    match shape(name, d, notes[i].len) {
        Some(c) => {
            notes[i].bend = c;
            true
        }
        None => false,
    }
}

/// Scale a pitch curve to a new depth (semitones, the shape kept).
pub fn with_depth(c: &[(f32, f32)], new_depth: f32) -> Curve {
    let d = depth(c);
    if d < 1e-6 {
        return c.to_vec();
    }
    let k = new_depth.abs().min(MAX_SEMIS) / d;
    c.iter().map(|p| (p.0, p.1 * k)).collect()
}

/// A drawn stroke on a curve: the points between positions `p0` and `p1` are replaced by the line from `v0` to `v1`. An empty
/// curve starts at `rest` over the whole note, so a stroke in the middle of a note does not spread to its ends.
pub fn draw(c: &[(f32, f32)], rest: f32, p0: f32, v0: f32, p1: f32, v1: f32) -> Curve {
    let (mut p0, mut v0, mut p1, mut v1) = (p0.clamp(0.0, 1.0), v0, p1.clamp(0.0, 1.0), v1);
    if p1 < p0 {
        std::mem::swap(&mut p0, &mut p1);
        std::mem::swap(&mut v0, &mut v1);
    }
    let base: Curve = if c.is_empty() { vec![(0.0, rest), (1.0, rest)] } else { c.to_vec() };
    let mut v: Curve = base.iter().copied().filter(|p| p.0 < p0 - 1e-5 || p.0 > p1 + 1e-5).collect();
    v.push((p0, v0));
    if p1 > p0 + 1e-5 {
        v.push((p1, v1));
    }
    tidy(&v)
}

/// Drop a curve that never leaves its rest value (it would only cost the engine work).
pub fn prune(c: &mut Curve, rest: f32) {
    if !moves(c, rest) {
        c.clear();
    }
}

/// The velocity line tool: `old` velocities under a drawn line whose value at each note is `line`, blended by `influence` 0..1.
/// Relative (the default): new = old + influence x (line - mean(old)): the average follows the line (a crescendo, a fade) while
/// every note keeps its accent against its neighbours. Absolute: new = old + influence x (line - old): at 100 % every note sits
/// on the line. Results stay in 0.02..1.
pub fn velocity_line(old: &[f32], line: &[f32], influence: f32, absolute: bool) -> Vec<f32> {
    let k = influence.clamp(0.0, 1.0);
    let mean = if old.is_empty() { 0.0 } else { old.iter().sum::<f32>() / old.len() as f32 };
    old.iter()
        .zip(line)
        .map(|(o, l)| {
            let target = if absolute { *l - *o } else { *l - mean };
            (o + k * target).clamp(0.02, 1.0)
        })
        .collect()
}

/// Write gestures of one kind on purpose into the notes that start between `from` and `to` (beats, in the notes' own time) -
/// for the generators, recipes and the Project Agent. Which notes get one is musical:
/// * `scoop`: every chord entry (notes starting together, a new chord); on a single line, the notes of a beat or longer;
/// * `slide`: a mono line's notes that run into the next one (it starts within an eighth of this one's end), up to an octave;
/// * `fall`: the last note of each phrase (a rest of half a beat or more after it, or the end of the span);
/// * `dive`: the last note before a rest of a beat or more, or the end of the span (a fill / screech dive);
/// * `riser`: the note(s) sounding at the end of the span (into a drop).
/// `depth` <= 0 = the shape's default. Notes that already have a gesture keep it. Returns how many notes got one.
pub fn write_kind(notes: &mut [Note], kind: &str, from: f64, to: f64, depth: f32) -> usize {
    let mut idx: Vec<usize> = (0..notes.len()).filter(|i| notes[*i].start >= from - 1e-6 && notes[*i].start < to - 1e-6).collect();
    idx.sort_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start).then(notes[*a].key.cmp(&notes[*b].key)));
    if idx.is_empty() {
        return 0;
    }
    let together = |a: &Note, b: &Note| (a.start - b.start).abs() < 0.03;
    let chordal = idx.windows(2).any(|w| together(&notes[w[0]], &notes[w[1]]));
    let mut pick: Vec<usize> = vec![];
    match kind {
        "scoop" => {
            if chordal {
                // a chord entry: a group of 2+ notes starting together whose pitches differ from the group before
                let mut groups: Vec<Vec<usize>> = vec![];
                for &i in &idx {
                    match groups.last_mut() {
                        Some(g) if together(&notes[g[0]], &notes[i]) => g.push(i),
                        _ => groups.push(vec![i]),
                    }
                }
                let mut prev: Vec<u8> = vec![];
                for g in groups {
                    let mut keys: Vec<u8> = g.iter().map(|i| notes[*i].key).collect();
                    keys.sort_unstable();
                    if g.len() >= 2 && keys != prev {
                        pick.extend(&g);
                    }
                    prev = keys;
                }
            } else {
                pick = idx.iter().copied().filter(|i| notes[*i].len >= 0.99).collect();
            }
        }
        "slide" => {
            if !chordal {
                for w in idx.windows(2) {
                    let (a, b) = (&notes[w[0]], &notes[w[1]]);
                    let gap = b.start - (a.start + a.len);
                    let iv = (b.key as i32 - a.key as i32).abs();
                    if gap < 0.125 && (1..=12).contains(&iv) {
                        pick.push(w[0]);
                    }
                }
            }
        }
        "fall" | "dive" => {
            let rest = if kind == "dive" { 1.0 } else { 0.5 };
            for (k, &i) in idx.iter().enumerate() {
                let end = notes[i].start + notes[i].len;
                let next = idx[k + 1..].iter().map(|j| notes[*j].start).find(|s| *s > notes[i].start + 0.03);
                let last = next.is_none();
                if (last || next.unwrap_or(f64::MAX) - end >= rest) && notes[i].len >= 0.24 && !(chordal && !last) {
                    pick.push(i);
                }
            }
        }
        "riser" => {
            let end = idx.iter().map(|i| notes[*i].start + notes[*i].len).fold(f64::MIN, f64::max).min(to);
            pick = idx.iter().copied().filter(|i| notes[*i].start + notes[*i].len >= end - 0.05).collect();
        }
        _ => return 0,
    }
    let mut n = 0;
    for i in pick {
        if notes[i].bend.is_empty() && apply_shape(notes, i, kind, depth) {
            n += 1;
        }
    }
    n
}

/// A curve as text for the piano roll's clipboard ("p:v p:v ..."), and back.
pub fn to_text(c: &[(f32, f32)]) -> String {
    c.iter().map(|p| format!("{}:{}", p.0, p.1)).collect::<Vec<_>>().join(" ")
}
pub fn from_text(s: &str) -> Curve {
    tidy(&s.split_whitespace().filter_map(|t| {
        let (a, b) = t.split_once(':')?;
        Some((a.parse().ok()?, b.parse().ok()?))
    }).collect::<Vec<_>>())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn curves_interpolate_hold_and_trim() {
        let c = vec![(0.0, -3.0), (0.5, 0.0), (1.0, 2.0)];
        assert_eq!(at(&c, 0.0), -3.0);
        assert!((at(&c, 0.25) + 1.5).abs() < 1e-6);
        assert_eq!(at(&c, 1.0), 2.0);
        assert_eq!(at(&[], 0.3), 0.0);
        let t = trim(&c, 0.5, 1.0);
        assert!((at(&t, 0.0)).abs() < 1e-6 && (at(&t, 1.0) - 2.0).abs() < 1e-6 && (at(&t, 0.5) - 1.0).abs() < 1e-6);
        // a note cut in two keeps the same pitch path in time
        let mut a = Note::new(1.0, 2.0, 60, 0.8);
        a.bend = c.clone();
        let mut b = a.clone();
        a.cut(0.0, 0.8);
        b.cut(0.8, 2.0);
        assert!((a.start - 1.0).abs() < 1e-9 && (a.len - 0.8).abs() < 1e-9 && (b.start - 1.8).abs() < 1e-9 && (b.len - 1.2).abs() < 1e-9);
        for k in 0..=20 {
            let beat = k as f32 * 0.1;
            let want = at(&c, beat / 2.0);
            let got = if beat <= 0.8 { at(&a.bend, beat / 0.8) } else { at(&b.bend, (beat - 0.8) / 1.2) };
            assert!((want - got).abs() < 1e-4, "{beat}: {want} vs {got}");
        }
        // a note without curves is cut as before
        let mut p = Note::new(0.0, 1.0, 60, 0.8);
        p.cut(0.25, 0.75);
        assert!(p.bend.is_empty() && (p.start - 0.25).abs() < 1e-9 && (p.len - 0.5).abs() < 1e-9);
    }

    #[test]
    fn shapes_have_the_right_direction_and_timing() {
        let s = shape("scoop", 3.0, 1.0).unwrap();
        assert_eq!(at(&s, 0.0), -3.0);
        assert_eq!(at(&s, 1.0), 0.0);
        assert!(s.last().unwrap().0 <= 0.45);
        let f = shape("fall", 4.0, 2.0).unwrap();
        assert_eq!(at(&f, 0.0), 0.0);
        assert_eq!(at(&f, 1.0), -4.0);
        assert!(at(&f, 0.7) > -0.01, "a fall starts near the end");
        let d = shape("dive", 12.0, 2.0).unwrap();
        assert_eq!(at(&d, 1.0), -12.0);
        let r = shape("riser", 12.0, 8.0).unwrap();
        assert!(at(&r, 0.5) > 0.0 && at(&r, 0.5) < 6.0 && at(&r, 1.0) == 12.0);
        let mut notes = vec![Note::new(0.0, 1.0, 60, 0.8), Note::new(1.0, 1.0, 55, 0.8)];
        assert!(apply_shape(&mut notes, 0, "slide", 0.0));
        assert_eq!(at(&notes[0].bend, 1.0), -5.0);
        assert!(apply_shape(&mut notes, 0, "clear", 0.0) && notes[0].bend.is_empty());
        assert!(!apply_shape(&mut notes, 0, "wobble", 0.0));
        assert!((depth(&with_depth(&s, 5.0)) - 5.0).abs() < 1e-5);
    }

    #[test]
    fn written_gestures_go_where_they_belong() {
        // chords: a scoop on every new chord, not on a repeated one
        let mut ch = vec![];
        for (st, keys) in [(0.0, [57u8, 60, 64]), (2.0, [57, 60, 64]), (4.0, [53, 57, 60])] {
            for k in keys {
                ch.push(Note::new(st, 1.5, k, 0.8));
            }
        }
        assert_eq!(write_kind(&mut ch, "scoop", 0.0, 8.0, 0.0), 6);
        assert!(ch[3].bend.is_empty() && !ch[0].bend.is_empty() && !ch[6].bend.is_empty());
        // a legato line slides, a detached one does not
        let mut line = vec![Note::new(0.0, 1.0, 36, 0.8), Note::new(1.0, 1.0, 43, 0.8), Note::new(3.0, 1.0, 41, 0.8), Note::new(4.0, 0.5, 60, 0.8)];
        assert_eq!(write_kind(&mut line, "slide", 0.0, 8.0, 0.0), 1);
        assert_eq!(at(&line[0].bend, 1.0), 7.0);
        let mut l2 = line.clone();
        assert_eq!(write_kind(&mut l2, "dive", 0.0, 8.0, 0.0), 2, "before the rest and at the end");
        let mut l3 = vec![Note::new(0.0, 4.0, 48, 0.8)];
        assert_eq!(write_kind(&mut l3, "riser", 0.0, 4.0, 0.0), 1);
        assert_eq!(write_kind(&mut l3, "riser", 0.0, 4.0, 0.0), 0, "a gesture already there is kept");
        assert_eq!(write_kind(&mut l3, "wobble", 0.0, 4.0, 0.0), 0);
    }

    #[test]
    fn drawing_and_the_velocity_line() {
        let c = draw(&[], 0.0, 0.4, 0.8, 0.6, 0.8);
        assert_eq!(at(&c, 0.0), 0.0);
        assert_eq!(at(&c, 0.5), 0.8);
        assert_eq!(at(&c, 1.0), 0.0);
        let c2 = draw(&c, 0.0, 0.5, 0.2, 0.5, 0.2);
        assert!((at(&c2, 0.5) - 0.2).abs() < 1e-6);
        // relative: the accents survive a crescendo
        let old = [0.4, 0.6, 0.4, 0.6];
        let line = [0.3, 0.4, 0.5, 0.6];
        let v = velocity_line(&old, &line, 1.0, false);
        assert!(v[1] > v[0] && v[1] > v[2] && v[3] > v[2] && v[3] > v[1], "accents kept, rising: {v:?}");
        assert!((v.iter().sum::<f32>() / 4.0 - 0.45).abs() < 1e-4, "the mean follows the line");
        let a = velocity_line(&old, &line, 1.0, true);
        assert!(a.iter().zip(&line).all(|(x, y)| (x - y).abs() < 1e-6));
        let h = velocity_line(&old, &line, 0.0, false);
        assert_eq!(h, old.to_vec());
        assert_eq!(from_text(&to_text(&c)), c);
    }
}
