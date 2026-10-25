//! Growl skins: a dubstep bass *performance* laid over a Growl track's MIDI. The notes keep their pitches and their place in the
//! bar; what changes is how they are played:
//!
//! * **gating** - held notes are chopped into the rhythm cell of their phrase (riddim gaps, brostep gallops, neuro 16ths), so the
//!   bass becomes a percussion part; a mono synth retriggers its envelopes on every hit;
//! * **per-hit envelopes** - every hit draws its own filter pluck, distortion spike and amp decay / sustain (short and dry for the
//!   chops, open and sustained for the notes that ring), wavetable position steps and, for the talking styles, a vowel sweep;
//! * **the form** - the phrases come from [`crate::articulate::phrase_map`]: the same material gets the same cell and positions, the
//!   depth follows the energy arc, and the last phrase before a bigger one turns into a **riser**: nested envelopes, a sawtooth bend
//!   inside each retriggered note, the notes climbing through the track's own pitch classes while the cycles halve (1 beat, 1/2,
//!   1/4, 1/8), noise and drive coming up, then a short silence before the drop;
//! * **flair** - only small pitch gestures: a legato overlap into some next notes (the Growl glides), a chirp (a quick fall of at
//!   most two semitones on the bend) on a few accented entries, a fall-off at a phrase's last note. Detune moves a little, per phrase.
//!
//! Sound engineering: the sub and the output level are never touched, the filter never closes completely, drive stays below the
//! point where it turns to mush, attack stays click-free, and the bend rests exactly at centre between gestures.
use crate::articulate::{self as art, Phrase};
use crate::compose::Rng;

/// a note of the skinned part (absolute beats); `src` is the source note it came from (which clip it goes back to)
#[derive(Clone, Debug, PartialEq)]
pub struct SNote {
    pub key: u8,
    pub start: f64,
    pub len: f64,
    pub vel: f32,
    pub src: usize,
}

#[derive(Clone, Debug)]
pub struct Style {
    pub name: &'static str,
    /// rhythm cells per material (letter A, B, C, D ...)
    cells: [&'static str; 4],
    /// share of the gap to the next hit a chopped hit sounds
    gate: f64,
    /// notes at least this long (beats) are chopped
    chop_min: f64,
    /// amp decay for chopped hits / for ringing notes (0..1 parameter values)
    decay: (f32, f32),
    /// amp sustain for chopped hits / ringing notes
    sus: (f32, f32),
    /// drive: base and the spike on a hit
    drv: (f32, f32),
    /// cutoff: floor and the top of a hit's pluck
    cut: (f32, f32),
    /// wavetable positions a phrase steps through
    pos_steps: usize,
    /// vowel sweeps per hit (talking bass)
    talk: bool,
    /// chance of a glide into the next note / a chirp on an entry, at flair 1
    glide: f32,
    chirp: f32,
}

pub const STYLES: [Style; 4] = [
    Style { name: "riddim", cells: ["tresillo", "son", "halftime", "syncopated"], gate: 0.55, chop_min: 0.75, decay: (0.2, 0.45), sus: (0.15, 0.8), drv: (0.4, 0.25), cut: (0.3, 0.75), pos_steps: 2, talk: false, glide: 0.15, chirp: 0.35 },
    Style { name: "brostep", cells: ["syncopated", "gallop", "tresillo", "eighths"], gate: 0.7, chop_min: 0.75, decay: (0.25, 0.5), sus: (0.3, 0.85), drv: (0.35, 0.2), cut: (0.32, 0.8), pos_steps: 3, talk: true, glide: 0.3, chirp: 0.45 },
    Style { name: "neuro", cells: ["sixteenths", "gallop", "eighths", "syncopated"], gate: 0.6, chop_min: 0.5, decay: (0.18, 0.4), sus: (0.25, 0.7), drv: (0.45, 0.2), cut: (0.28, 0.82), pos_steps: 4, talk: true, glide: 0.2, chirp: 0.25 },
    Style { name: "melodic", cells: ["eighths", "four", "dotted", "offbeat"], gate: 0.85, chop_min: 1.5, decay: (0.35, 0.6), sus: (0.5, 0.9), drv: (0.2, 0.12), cut: (0.38, 0.7), pos_steps: 2, talk: false, glide: 0.4, chirp: 0.15 },
];

/// the style for a name; "auto" reads the part: long notes want riddim chops, busy parts neuro, the rest brostep
pub fn style_for(name: &str, notes: &[(u8, f64, f64, f32)]) -> &'static Style {
    if let Some(s) = STYLES.iter().find(|s| s.name == name) {
        return s;
    }
    if notes.is_empty() {
        return &STYLES[1];
    }
    let span = (notes.last().map(|n| n.1 + n.2).unwrap_or(1.0) - notes[0].1).max(1.0);
    let dens = notes.len() as f64 / span;
    let mean = notes.iter().map(|n| n.2).sum::<f64>() / notes.len() as f64;
    if dens > 2.2 {
        &STYLES[2]
    } else if mean >= 1.2 {
        &STYLES[0]
    } else {
        &STYLES[1]
    }
}

pub struct Skin {
    pub notes: Vec<SNote>,
    /// parameter -> lane points (beat, 0..1)
    pub lanes: Vec<(&'static str, Vec<(f64, f32)>)>,
    pub summary: String,
}

fn cell_slots(name: &str) -> Vec<u32> {
    art::CELLS.iter().find(|c| c.0 == name).map(|c| c.1.bytes().enumerate().filter(|(_, b)| *b == b'x').map(|(i, _)| i as u32).collect()).unwrap_or_else(|| vec![0, 4, 8, 12])
}

/// a lane being drawn: steps and ramps appended in time order, kept a function of time
struct Lane(Vec<(f64, f32)>);
impl Lane {
    fn at(&mut self, t: f64, v: f32) {
        let v = v.clamp(0.0, 1.0);
        match self.0.last() {
            Some(l) if t <= l.0 + 0.003 => {
                let t = l.0 + 0.004;
                self.0.push((t, v));
            }
            _ => self.0.push((t, v)),
        }
    }
    /// a hard step to `v` at `t` (holding the previous value until just before)
    fn step(&mut self, t: f64, v: f32) {
        if let Some(&(_, pv)) = self.0.last() {
            self.at(t - 0.004, pv);
        }
        self.at(t, v);
    }
}

/// the next pitch above `k` that the part itself uses (its own scale), or a whole tone up
fn climb(k: i32, pcs: &[bool; 12]) -> i32 {
    for d in 1..=4 {
        if pcs[((k + d).rem_euclid(12)) as usize] {
            return k + d;
        }
    }
    k + 2
}

/// Skin the part `src` (key, start, length, velocity - absolute beats, sorted) over b0..b1. `amount` 0..1 is how hard the
/// technique is applied (depth of the envelopes, share of notes chopped), `flair` 0..1 how many pitch gestures, `cur` reads the
/// patch's current value of a parameter (the lanes centre on the sound as it is).
pub fn skin(src: &[(u8, f64, f64, f32)], b0: f64, b1: f64, style: &Style, amount: f32, flair: f32, seed: u64, cur: &dyn Fn(&str) -> f32) -> Skin {
    let amount = amount.clamp(0.0, 1.0);
    let flair = flair.clamp(0.0, 1.0);
    let mut rng = Rng::new(seed ^ 0x6A09_E667);
    let on: Vec<(f64, f64)> = src.iter().map(|n| (n.1, n.2)).collect();
    let ph = art::phrase_map(b0, b1, &on, 0.0);
    let mut pcs = [false; 12];
    for n in src {
        pcs[(n.0 % 12) as usize] = true;
    }
    let phrase_of = |t: f64| -> Phrase { ph.iter().find(|f| t >= f.b0 - 1e-6 && t < f.b1).cloned().unwrap_or(Phrase { b0, b1, letter: 0, repeat: 0, energy: 0.7, before_change: false }) };

    // ── the risers: the end of each phrase that leads into a bigger, different one (the first phrase counts: the intro riser)
    let mut risers: Vec<(f64, f64)> = vec![];
    for k in 0..ph.len().saturating_sub(1) {
        let (f, g) = (&ph[k], &ph[k + 1]);
        if f.before_change && g.letter != f.letter && (g.energy > f.energy + 0.05 || k == 0) && risers.len() < 4 {
            let len = (f.b1 - f.b0).min(32.0);
            let len = if len >= 16.0 { (len * 0.5).min(16.0).max(8.0) } else { len.min(8.0) };
            // a riser needs notes to grow from
            if src.iter().any(|n| n.1 >= f.b0 - 1e-6 && n.1 < f.b1) {
                risers.push((f.b1 - len, f.b1));
            }
        }
    }
    let in_riser = |t: f64| risers.iter().any(|r| t >= r.0 - 1e-6 && t < r.1);

    // ── the hits: notes chopped into the cell of their phrase
    let mut hits: Vec<SNote> = vec![];
    let mut chopped = 0usize;
    for (i, n) in src.iter().enumerate() {
        let (key, start, len, vel) = (n.0, n.1, n.2.max(0.03), n.3);
        if start < b0 - 1e-6 || start >= b1 || in_riser(start) {
            continue;
        }
        let f = phrase_of(start);
        let next_on = src.get(i + 1).map(|m| m.1).unwrap_or(b1);
        // a note never runs into a riser
        let rcut = risers.iter().filter(|r| r.0 > start).map(|r| r.0).fold(b1, f64::min);
        let end = (start + len).min(next_on).min(rcut);
        // the amount decides how many long notes are chopped: all of them at 1, the longest ones only at low settings
        let chop = len >= style.chop_min * (1.0 + (1.0 - amount as f64) * 2.0);
        if !chop {
            hits.push(SNote { key, start, len: (end - start).max(0.03), vel, src: i });
            continue;
        }
        chopped += 1;
        let slots = cell_slots(style.cells[f.letter as usize % 4]);
        let mut times: Vec<f64> = vec![start];
        let bar0 = ((start - b0) / 4.0).floor() * 4.0 + b0;
        let mut bar = bar0;
        while bar < end && times.len() < 256 {
            for s in &slots {
                let t = bar + *s as f64 * 0.25;
                if t > start + 0.1 && t < end - 0.06 {
                    times.push(t);
                }
            }
            bar += 4.0;
        }
        times.sort_by(f64::total_cmp);
        for (h, &t) in times.iter().enumerate() {
            let nt = times.get(h + 1).copied().unwrap_or(end);
            let last = h + 1 == times.len();
            // the last hit of a note that ends a phrase rings out; the others are gated
            let ring = last && f.before_change && end >= f.b1 - 0.5;
            let l = if ring { nt - t } else { ((nt - t) * style.gate).max(0.06) };
            // accent the note's first hit, ghost the rest a little
            let v = if h == 0 { (vel + 0.08).min(1.0) } else { vel * if h % 2 == 1 { 0.78 } else { 0.86 } };
            hits.push(SNote { key, start: t, len: l, vel: v, src: i });
        }
    }
    hits.sort_by(|a, b| a.start.total_cmp(&b.start));

    // ── flair: legato into a few next notes (the mono Growl glides), never across a rest or a big leap
    let mut glides = 0usize;
    for k in 0..hits.len().saturating_sub(1) {
        let (a, b) = (&hits[k], &hits[k + 1]);
        let gap = b.start - (a.start + a.len);
        let leap = (b.key as i32 - a.key as i32).abs();
        if a.key != b.key && leap <= 7 && gap < 0.5 && rng.chance((style.glide * flair) as f64) {
            hits[k].len = hits[k + 1].start - hits[k].start + 0.04;
            glides += 1;
        }
    }

    // ── the riser notes: per quarter of the window the cycle halves; the pitch climbs through the part's pitch classes
    let mut riser_notes: Vec<(SNote, f64)> = vec![];
    for &(rs, re) in &risers {
        // grow from the last note before the window, or the first in it
        let (root, srcn) = src.iter().enumerate().filter(|(_, n)| n.1 < re).min_by(|a, b| (a.1 .1 - rs).abs().total_cmp(&(b.1 .1 - rs).abs())).map(|(i, n)| (n.0 as i32, i)).unwrap_or((36, 0));
        let body = re - 0.25 - rs;
        let mut k = root;
        let mut count = 0usize;
        let total_cycles: f64 = [1.0, 0.5, 0.25, 0.125].iter().map(|p| (body / 4.0 / p).floor()).sum();
        let steps = 6.0;
        for (q, per) in [1.0, 0.5, 0.25, 0.125].iter().enumerate() {
            let qs = rs + body * q as f64 / 4.0;
            let qe = rs + body * (q + 1) as f64 / 4.0;
            let mut t = qs;
            while t + per <= qe + 1e-6 {
                // climb: an even rise of `steps` scale steps over all the cycles
                let want = ((count as f64 / total_cycles.max(1.0)) * steps).floor() as i32;
                while (k - root) < 24 && want > 0 && scale_steps(root, k, &pcs) < want {
                    k = climb(k, &pcs);
                }
                let v = 0.62 + 0.3 * (count as f64 / total_cycles.max(1.0)) as f32;
                riser_notes.push((SNote { key: k.clamp(12, 108) as u8, start: t, len: per * 0.92, vel: v.min(1.0), src: srcn }, (t - rs) / (re - rs)));
                count += 1;
                t += per;
            }
        }
    }

    // ── the lanes
    let base = |name: &str, d: f32| {
        let v = cur(name);
        if v.is_finite() {
            v
        } else {
            d
        }
    };
    let (mut cut, mut drv, mut amp_d, mut amp_s, mut pos, mut vow, mut bend, mut nois, mut detn) = (Lane(vec![]), Lane(vec![]), Lane(vec![]), Lane(vec![]), Lane(vec![]), Lane(vec![]), Lane(vec![]), Lane(vec![]), Lane(vec![]));
    let a = amount;
    let cut0 = base("cut", 0.4);
    let pos0 = base("pos", 0.3);
    let vow0 = base("vow", 0.5);
    let det0 = base("detn", 0.2);
    let nois0 = base("nois", 0.0);
    let (d0, s0) = (base("amp d", 0.4), base("amp s", 1.0));
    cut.at(b0, cut0);
    drv.at(b0, base("drv", 0.3));
    amp_d.at(b0, d0);
    amp_s.at(b0, s0);
    pos.at(b0, pos0);
    vow.at(b0, vow0);
    bend.at(b0, 0.5);
    nois.at(b0, nois0);
    detn.at(b0, det0);
    // positions per letter: the same material steps through the same wavetable frames
    let pos_of = |letter: u8, h: usize| -> f32 {
        let mut r = Rng::new(seed ^ (letter as u64 + 1) * 0x9E37_79B9);
        let vals: Vec<f32> = (0..style.pos_steps).map(|_| (0.12 + 0.76 * r.f() as f32)).collect();
        pos0 + (vals[h % vals.len()] - pos0) * (0.35 + 0.65 * a)
    };
    let mut chirps = 0usize;
    let mut last_letter = 255u8;
    let mut hit_in_phrase = 0usize;
    let mut cur_phrase = f64::NAN;
    let mut prev_end = f64::MIN;
    for (k, h) in hits.iter().enumerate() {
        let f = phrase_of(h.start);
        if f.b0 != cur_phrase {
            cur_phrase = f.b0;
            hit_in_phrase = 0;
            // detune moves a little with the material, never by a pitch
            if f.letter != last_letter {
                let d = det0 + ((mix(seed, f.letter) % 100) as f32 / 100.0 - 0.5) * 0.12 * a;
                detn.step(f.b0, d);
                last_letter = f.letter;
            }
        }
        let e = f.energy;
        let accent = h.vel >= 0.8 || hit_in_phrase == 0;
        let short = h.len < 0.6;
        let floor = (style.cut.0 + 0.12 * e).max(0.18).min(cut0.max(style.cut.0) + 0.1);
        let top = (floor + (style.cut.1 - floor) * (0.55 + 0.45 * e) * (0.5 + 0.5 * a) * if accent { 1.0 } else { 0.8 }).min(0.92);
        // the filter pluck: open at the hit, falling fast, closed by its end
        cut.step(h.start, top);
        cut.at(h.start + (h.len * 0.3).min(0.25), floor + (top - floor) * 0.35);
        cut.at(h.start + h.len * 0.95, floor);
        // the distortion envelope: a spike for the transient, back to the phrase's base
        let dbase = (style.drv.0 + 0.12 * e) * (0.6 + 0.4 * a);
        let spike = (dbase + style.drv.1 * if accent { 1.0 } else { 0.6 } * a).min(0.78);
        drv.step(h.start, spike);
        drv.at(h.start + (h.len * 0.4).min(0.12), dbase.min(0.7));
        // the amp: chopped hits short and dry, ringing notes open
        let (dv, sv) = if short { (style.decay.0, style.sus.0) } else { (style.decay.1, style.sus.1) };
        amp_d.step(h.start, d0 + (dv - d0) * a);
        amp_s.step(h.start, s0 + (sv - s0) * a);
        // timbre: the wavetable frame per hit, the vowel opening through the hit (talk)
        pos.step(h.start, pos_of(f.letter, hit_in_phrase));
        if style.talk {
            let v1 = (vow0 + 0.35 * a * if hit_in_phrase % 2 == 0 { 1.0 } else { -1.0 }).clamp(0.05, 0.95);
            vow.step(h.start, vow0);
            vow.at(h.start + h.len * 0.85, v1);
        }
        // a chirp on an accented entry after a rest: the bend falls from up to +2 semitones to centre within a 16th
        let after_rest = h.start - prev_end > 0.45 || hit_in_phrase == 0;
        if accent && after_rest && rng.chance((style.chirp * flair) as f64) {
            let c = 0.5 + 0.5 * (0.5 + 0.5 * flair);
            bend.step(h.start, c);
            bend.at(h.start + 0.09, 0.5);
            chirps += 1;
        }
        // the phrase's last note before a change falls off at its end (a small dip, released before the next note)
        let last_in_phrase = hits.get(k + 1).map(|n| n.start >= f.b1 - 1e-6).unwrap_or(true);
        if last_in_phrase && f.before_change && flair > 0.3 && h.len > 0.4 {
            bend.at(h.start + h.len * 0.7, 0.5);
            bend.at(h.start + h.len * 0.98, 0.5 - 0.15 * flair);
            bend.at(h.start + h.len + 0.02, 0.5);
        }
        prev_end = h.start + h.len;
        hit_in_phrase += 1;
    }
    // the risers, drawn over whatever the phrase had: nested envelopes
    for &(rs, re) in &risers {
        let pre = |l: &Lane| l.0.iter().rev().find(|p| p.0 < rs).map(|p| p.1);
        let c_end = (cut0 + 0.4).min(0.9);
        let cut_from = pre(&cut).unwrap_or(cut0);
        // the existing points inside the window are dropped: the riser owns it
        for l in [&mut cut, &mut drv, &mut amp_d, &mut amp_s, &mut pos, &mut vow, &mut bend, &mut nois] {
            l.0.retain(|p| p.0 < rs - 0.005 || p.0 >= re + 0.002);
        }
        // the lanes are rebuilt in time order: everything after the window is held aside and re-appended
        let mut tail: Vec<Vec<(f64, f32)>> = vec![];
        for l in [&mut cut, &mut drv, &mut amp_d, &mut amp_s, &mut pos, &mut vow, &mut bend, &mut nois] {
            let at = l.0.iter().position(|p| p.0 >= re).unwrap_or(l.0.len());
            tail.push(l.0.split_off(at));
        }
        amp_d.step(rs, (d0 + (style.decay.0 - d0) * 0.6).max(0.12));
        amp_s.step(rs, 0.6);
        nois.step(rs, nois0);
        let drv_from = pre(&drv).unwrap_or(0.3);
        drv.step(rs, drv_from);
        cut.step(rs, cut_from);
        for (n, prog) in riser_notes.iter().filter(|(n, _)| n.start >= rs - 1e-6 && n.start < re) {
            let p = *prog as f32;
            // the sawtooth bend inside each note: centre at the hit, rising through it, deeper as the riser grows
            bend.step(n.start, 0.5);
            bend.at(n.start + n.len * 0.95, 0.5 + (0.12 + 0.33 * p) * (0.5 + 0.5 * flair.max(0.4)));
            cut.at(n.start, (cut_from + (c_end - cut_from) * p * p).min(0.92));
            cut.at(n.start + n.len * 0.6, (cut_from + (c_end - cut_from) * p * p * 0.85).min(0.92));
        }
        // noise, drive and position climb under it; the last quarter beat is air before the drop
        nois.at(re - 0.3, (nois0 + 0.35 * a).min(0.5));
        drv.at(re - 0.3, (drv_from + 0.25 * a).min(0.75));
        pos.at(rs, pre(&pos).unwrap_or(pos0));
        pos.at(re - 0.3, (pos0 + 0.4).min(0.95));
        bend.step(re - 0.25, 0.5);
        nois.step(re - 0.25, nois0);
        for (l, t) in [&mut cut, &mut drv, &mut amp_d, &mut amp_s, &mut pos, &mut vow, &mut bend, &mut nois].into_iter().zip(tail) {
            for p in t {
                l.at(p.0, p.1);
            }
        }
    }
    let mut notes: Vec<SNote> = hits;
    notes.extend(riser_notes.into_iter().map(|r| r.0));
    notes.sort_by(|a, b| a.start.total_cmp(&b.start));
    let mut lanes: Vec<(&'static str, Vec<(f64, f32)>)> = vec![("cut", cut.0), ("drv", drv.0), ("amp d", amp_d.0), ("amp s", amp_s.0), ("pos", pos.0), ("bend", bend.0), ("detn", detn.0)];
    if style.talk {
        lanes.push(("vow", vow.0));
    }
    if !risers.is_empty() {
        lanes.push(("nois", nois.0));
    }
    for l in lanes.iter_mut() {
        l.1.retain(|p| p.0.is_finite() && p.1.is_finite());
        l.1.sort_by(|a, b| a.0.total_cmp(&b.0));
    }
    let summary = format!(
        "{} skin, {}: {} hits ({} notes gated), {} glide{}, {} chirp{}{}",
        style.name,
        art::form_text(&ph),
        notes.len(),
        chopped,
        glides,
        if glides == 1 { "" } else { "s" },
        chirps,
        if chirps == 1 { "" } else { "s" },
        if risers.is_empty() { String::new() } else { format!(", riser{} into bar {}", if risers.len() > 1 { "s" } else { "" }, risers.iter().map(|r| format!("{}", (r.1 / 4.0).round() as i64 + 1)).collect::<Vec<_>>().join(", ")) }
    );
    Skin { notes, lanes, summary }
}

/// how many of the part's scale steps lie between `root` and `k`
fn scale_steps(root: i32, k: i32, pcs: &[bool; 12]) -> i32 {
    (root + 1..=k).filter(|x| pcs[x.rem_euclid(12) as usize]).count() as i32
}

fn mix(seed: u64, letter: u8) -> u64 {
    let mut h = seed ^ (letter as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    h ^= h >> 31;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^ (h >> 29)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 16 bars of a riddim-ish part in F minor: two-beat notes (A material), a busier B, then A again, then a drop phrase
    fn part() -> Vec<(u8, f64, f64, f32)> {
        let mut v = vec![];
        for ph in 0..4 {
            let b = ph as f64 * 16.0;
            if ph == 1 {
                for k in 0..16 {
                    v.push(([41u8, 44, 46, 48][k % 4], b + k as f64, 0.5, 0.8));
                }
            } else {
                for k in 0..8 {
                    v.push(([41u8, 41, 44, 39][k % 4], b + k as f64 * 2.0, 2.0, 0.9));
                }
            }
        }
        v
    }

    #[test]
    fn long_notes_are_gated_into_hits_and_every_lane_is_sane() {
        let p = part();
        let s = skin(&p, 0.0, 64.0, &STYLES[0], 1.0, 0.5, 7, &|_| f32::NAN);
        assert!(s.notes.len() > p.len() + 20, "{} hits from {} notes", s.notes.len(), p.len());
        // pitches stay the part's own (or climb through its pitch classes in a riser)
        let pcs: Vec<u8> = p.iter().map(|n| n.0 % 12).collect();
        assert!(s.notes.iter().all(|n| pcs.contains(&(n.key % 12))), "a note outside the part's pitch classes");
        // no two hits of the gated part overlap unless it is a glide (then by at most a 32nd)
        for w in s.notes.windows(2) {
            assert!(w[0].start + w[0].len <= w[1].start + 0.05, "overlap {:?} {:?}", w[0], w[1]);
        }
        for (name, pts) in &s.lanes {
            assert!(pts.len() >= 2, "{name}");
            assert!(pts.windows(2).all(|w| w[1].0 > w[0].0), "{name}: not increasing");
            assert!(pts.iter().all(|p| (0.0..=1.0).contains(&p.1)), "{name}");
        }
        let lane = |n: &str| s.lanes.iter().find(|l| l.0 == n).map(|l| l.1.clone()).unwrap();
        // engineering: the filter never shuts, the drive never runs away, the bend rests at centre and moves at most 2 semitones
        assert!(lane("cut").iter().all(|p| p.1 >= 0.18 && p.1 <= 0.92));
        assert!(lane("drv").iter().all(|p| p.1 <= 0.78));
        let bend = lane("bend");
        assert!(bend.iter().all(|p| p.1 >= 0.3 && p.1 <= 1.0));
        // most of the time (sampled along the lane) the bend sits at centre: gestures are short
        let at = |t: f64| -> f32 {
            let k = bend.iter().position(|p| p.0 > t).unwrap_or(bend.len());
            if k == 0 || k == bend.len() {
                return bend[k.min(bend.len() - 1)].1;
            }
            let (a, b) = (bend[k - 1], bend[k]);
            a.1 + (b.1 - a.1) * ((t - a.0) / (b.0 - a.0)) as f32
        };
        let centred = (0..1280).filter(|i| (at(*i as f64 * 0.05) - 0.5).abs() < 0.02).count();
        assert!(centred > 1280 * 7 / 10, "the bend is off centre {}% of the time", 100 - centred * 100 / 1280);
        assert!(s.summary.contains("riddim skin"), "{}", s.summary);
    }

    #[test]
    fn a_riser_climbs_faster_and_higher_into_the_change() {
        let p = part();
        let s = skin(&p, 0.0, 64.0, &STYLES[1], 0.8, 0.6, 3, &|_| f32::NAN);
        assert!(s.summary.contains("riser"), "{}", s.summary);
        // the first phrase leads into the busier B: a riser at its end (bars 3-4 = beats 8..16)
        let r: Vec<&SNote> = s.notes.iter().filter(|n| n.start >= 8.0 && n.start < 16.0).collect();
        assert!(r.len() >= 12, "{} riser notes", r.len());
        let first = &r[..r.len() / 4];
        let last = &r[r.len() * 3 / 4..];
        let avg = |v: &[&SNote]| v.iter().map(|n| n.key as f64).sum::<f64>() / v.len() as f64;
        assert!(avg(last) > avg(first) + 2.0, "pitch {} -> {}", avg(first), avg(last));
        let gap = |v: &[&SNote]| (v.last().unwrap().start - v[0].start) / (v.len() - 1).max(1) as f64;
        assert!(gap(last) < gap(first) * 0.5, "speed {} -> {}", gap(first), gap(last));
        // air before the drop
        assert!(!s.notes.iter().any(|n| n.start > 15.8 && n.start < 16.0));
    }

    /// play notes through a real Growl at 140 bpm with the lanes applied every 128 frames -> mono samples
    fn play(notes: &[(u8, f64, f64, f32)], lanes: &[(&str, Vec<(f64, f32)>)], beats: f64) -> Vec<f32> {
        let sr = 44100.0f32;
        let mut g = crate::dsp::make_instrument("growl", sr).unwrap();
        g.load_preset("riddim");
        let spb = 60.0 / 140.0 * sr as f64;
        let total = (beats * spb) as usize;
        let mut ev: Vec<(usize, bool, u8, f32)> = vec![];
        for n in notes {
            ev.push(((n.1 * spb) as usize, true, n.0, n.3));
            ev.push((((n.1 + n.2) * spb) as usize, false, n.0, 0.0));
        }
        // offs before ons at the same frame
        ev.sort_by_key(|e| (e.0, e.1));
        let mut out = vec![];
        let mut buf = vec![0f32; 256];
        let mut k = 0;
        let mut f = 0usize;
        while f < total {
            let beat = f as f64 / spb;
            for (name, pts) in lanes {
                let i = pts.iter().position(|p| p.0 > beat).unwrap_or(pts.len());
                let v = if i == 0 { pts[0].1 } else if i == pts.len() { pts[i - 1].1 } else { let (a, b) = (pts[i - 1], pts[i]); a.1 + (b.1 - a.1) * ((beat - a.0) / (b.0 - a.0)) as f32 };
                g.set_param(name, v);
            }
            while k < ev.len() && ev[k].0 <= f {
                if ev[k].1 { g.note_on(ev[k].2, ev[k].3) } else { g.note_off(ev[k].2) }
                k += 1;
            }
            buf.fill(0.0);
            g.render(&mut buf);
            out.extend(buf.chunks_exact(2).map(|c| (c[0] + c[1]) * 0.5));
            f += 128;
        }
        out
    }

    /// how much the level moves at the 16th-note scale: the spread of 16th-note RMS values over their mean
    fn movement(x: &[f32]) -> f32 {
        let w = (60.0 / 140.0 / 4.0 * 44100.0) as usize;
        let r: Vec<f32> = x.chunks(w).map(|c| (c.iter().map(|v| v * v).sum::<f32>() / c.len() as f32).sqrt()).collect();
        let m = r.iter().sum::<f32>() / r.len() as f32;
        (r.iter().map(|v| (v - m).abs()).sum::<f32>() / r.len() as f32) / m.max(1e-6)
    }

    #[test]
    fn the_skinned_part_sounds_more_percussive_and_stays_clean() {
        let p: Vec<(u8, f64, f64, f32)> = part().into_iter().filter(|n| n.1 < 32.0).collect();
        let dry = play(&p, &[], 32.0);
        let s = skin(&p, 0.0, 32.0, &STYLES[0], 1.0, 0.5, 2, &|_| f32::NAN);
        let notes: Vec<(u8, f64, f64, f32)> = s.notes.iter().map(|n| (n.key, n.start, n.len, n.vel)).collect();
        let wet = play(&notes, &s.lanes, 32.0);
        assert!(wet.iter().all(|v| v.is_finite()), "NaN in the skinned audio");
        let peak = wet.iter().fold(0.0f32, |a, v| a.max(v.abs()));
        assert!(peak <= 1.0 && peak > 0.05, "peak {peak}");
        let (md, mw) = (movement(&dry), movement(&wet));
        assert!(mw > md * 1.3, "not more percussive: {md:.3} -> {mw:.3}");
    }

    #[test]
    fn auto_style_reads_the_part() {
        let long: Vec<(u8, f64, f64, f32)> = (0..8).map(|k| (40u8, k as f64 * 2.0, 2.0, 0.9)).collect();
        assert_eq!(style_for("auto", &long).name, "riddim");
        let busy: Vec<(u8, f64, f64, f32)> = (0..64).map(|k| (40u8, k as f64 * 0.25, 0.2, 0.9)).collect();
        assert_eq!(style_for("auto", &busy).name, "neuro");
        assert_eq!(style_for("melodic", &busy).name, "melodic");
    }
}
