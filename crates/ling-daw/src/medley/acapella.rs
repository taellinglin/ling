//! Acapellas in a mashup (Make Medley > ACAPELLAS): whole vocal recordings played under the medley the way a live mashup uses them.
//!
//! * Tempo: from the file name ("128bpm", "bpm 128"), else measured from the onsets ([`detect_bpm`]); the medley takes the first main
//!   acapella's tempo unless one is asked for, and every other acapella is stretched to it ([`tempo_ratio`]: the nearest of the tempo,
//!   double or half; WSOLA, `stretch.rs`).
//! * Key: from the sung pitches ([`key_of`]); the mashup's home key is the first main acapella's, so the MIDI parts move to the voice
//!   (they can, the recording cannot). Every section's excerpt is still fitted to the section's key by whole semitones with the
//!   medley's `fit_shift` (in its scale, consonant with what plays), pre-rendered per shift.
//! * Excerpts ([`excerpts`]): each section reads whole lines of the acapella from a bar line of its own grid ([`bar_phase`]): the
//!   hook in the hot sections, the lines in order elsewhere, a pickup kept before the downbeat.
//! * The plan ([`plan_vox`]): which sections are chops only (the acapella's level at 0, the chops carry the vocal), the full vocal,
//!   or both layered - by the section, the story's feel and the "chops replace the vocal" switch - and the level lane
//!   ([`vol_lane`]): down to 0 on the downbeat the chops start, back up on the downbeat they end with a short fade.
//! * Stressed lines ([`choose`]): the hook, the repeated phrases and the story's peak get backing harmonies ([`harmonize`]: tuned
//!   copies on the chord tones a third or so above / below, segment by segment), a vocoder (the line as the modulator of a chord
//!   synth), a double or an octave layer, a filtered telephone line, or a stutter of its first syllable.
use crate::chops::{self, Vocal};
use crate::emotion::Feel;
use crate::model::Note;
use crate::theory::Key;

/// a tempo written in a file name: "song_128bpm", "Vox 92 BPM", "bpm-140" (50 .. 220)
pub fn bpm_from_name(name: &str) -> Option<f64> {
    let l = name.to_lowercase();
    let b = l.as_bytes();
    let num = |s: usize, e: usize| -> Option<f64> { l.get(s..e).and_then(|t| t.parse::<f64>().ok()).filter(|v| (50.0..=220.0).contains(v)) };
    let mut from = 0;
    while let Some(p) = l[from..].find("bpm") {
        let at = from + p;
        let mut j = at;
        while j > 0 && matches!(b[j - 1], b' ' | b'_' | b'-') {
            j -= 1;
        }
        let mut k = j;
        while k > 0 && (b[k - 1].is_ascii_digit() || b[k - 1] == b'.') {
            k -= 1;
        }
        if let Some(v) = (k < j).then(|| num(k, j)).flatten() {
            return Some(v);
        }
        let mut s = at + 3;
        while s < b.len() && matches!(b[s], b' ' | b'_' | b'-') {
            s += 1;
        }
        let mut e = s;
        while e < b.len() && (b[e].is_ascii_digit() || b[e] == b'.') {
            e += 1;
        }
        if let Some(v) = (e > s).then(|| num(s, e)).flatten() {
            return Some(v);
        }
        from = at + 3;
    }
    None
}

/// The tempo of a recording from its onsets: the autocorrelation of the rise in level (5 ms frames) over 70 .. 180 bpm with its
/// double (a lag that also repeats at twice its length), a mild preference around 115 bpm; (bpm, confidence 0..1). None when the
/// recording is too short or has no pulse.
pub fn detect_bpm(stereo: &[f32], rate: u32) -> Option<(f64, f64)> {
    let hop = (rate as usize / 200).max(1);
    let frames = stereo.len() / 2;
    if frames < rate as usize * 4 {
        return None;
    }
    let n = frames / hop;
    let mut rms = vec![0f32; n];
    for (i, r) in rms.iter_mut().enumerate() {
        let mut e = 0.0;
        for f in i * hop..((i + 1) * hop).min(frames) {
            let m = 0.5 * (stereo[f * 2] + stereo[f * 2 + 1]);
            e += m * m;
        }
        *r = (e / hop as f32).sqrt();
    }
    let flux: Vec<f64> = (0..n).map(|i| if i == 0 { 0.0 } else { (rms[i] - rms[i - 1]).max(0.0) as f64 }).collect();
    let mean = flux.iter().sum::<f64>() / n as f64;
    let x: Vec<f64> = flux.iter().map(|v| v - mean).collect();
    let ac = |lag: usize| -> f64 { (0..n.saturating_sub(lag)).map(|i| x[i] * x[i + lag]).sum::<f64>() };
    let zero = ac(0).max(1e-12);
    let fps = rate as f64 / hop as f64;
    let (lo, hi) = ((fps * 60.0 / 180.0) as usize, (fps * 60.0 / 70.0).ceil() as usize);
    let mut best = (f64::MIN, 0usize);
    let mut sc = vec![0f64; hi + 2];
    for (lag, s) in sc.iter_mut().enumerate().take(hi + 2).skip(lo.saturating_sub(1).max(1)) {
        let bpm = 60.0 * fps / lag as f64;
        let prior = (-(bpm / 115.0).log2().powi(2) / 0.5).exp();
        *s = (ac(lag) + 0.5 * ac(2 * lag)) / zero * (0.75 + 0.25 * prior);
    }
    for lag in lo.max(1)..=hi {
        if sc[lag] > best.0 {
            best = (sc[lag], lag);
        }
    }
    if best.1 == 0 || best.0 <= 0.0 {
        return None;
    }
    // parabolic peak for a fraction of a frame
    let l = best.1;
    let (a, b, c) = (sc[l - 1], sc[l], sc[l + 1]);
    let den = a - 2.0 * b + c;
    let off = if den.abs() > 1e-12 { (0.5 * (a - c) / den).clamp(-0.5, 0.5) } else { 0.0 };
    let bpm = 60.0 * fps / (l as f64 + off);
    Some((bpm, best.0.clamp(0.0, 1.0)))
}

/// The tempo the recording is read at for a medley at `dst` bpm: its own, double or half (the nearest), and the duration multiplier
/// that plays it at `dst` (1 = as recorded).
pub fn tempo_ratio(src: f64, dst: f64) -> (f64, f64) {
    if src <= 1.0 || dst <= 1.0 {
        return (dst, 1.0);
    }
    let c = [src, src * 2.0, src * 0.5].into_iter().min_by(|a, b| (a / dst).ln().abs().total_cmp(&(b / dst).ln().abs())).unwrap_or(src);
    (c, c / dst)
}

/// the recording at the medley's tempo (`stretch` = duration multiplier) moved by `semis`, peaks raised to about 0.9 when quiet
pub fn prepare(stereo: &[f32], stretch: f64, semis: f64) -> Vec<f32> {
    let mut out = if (stretch - 1.0).abs() < 1e-4 && semis.abs() < 1e-3 { stereo.to_vec() } else { crate::stretch::process(stereo, stretch, semis) };
    let peak = out.iter().fold(0f32, |m, x| m.max(x.abs()));
    if peak > 1e-6 && peak < 0.7 {
        let g = 0.9 / peak;
        out.iter_mut().for_each(|x| *x *= g);
    }
    out
}

/// the key the vocal sings in: its pieces' pitches weighted by their length (theory::detect_key_hist)
pub fn key_of(v: &Vocal) -> Option<(u8, bool)> {
    let mut h = [0f64; 12];
    for p in &v.pieces {
        if let Some(m) = p.chop.midi {
            h[(m.round() as i32).rem_euclid(12) as usize] += p.len.max(0.1);
        }
    }
    if h.iter().sum::<f64>() <= 0.0 {
        return None;
    }
    let k = crate::theory::detect_key_hist(&h).0;
    Some((k.tonic, k.minor))
}

/// The bar grid of the vocal in its own beats: the eighth-note phase its syllables sit on, then which of the four beats is the
/// downbeat (most lines start on it, or with a pickup of up to three quarters of a beat before it). Returns the beat of a bar line.
pub fn bar_phase(v: &Vocal) -> f64 {
    if v.pieces.is_empty() {
        return 0.0;
    }
    let mut best = (f64::MIN, 0.0);
    for k in 0..32 {
        let ph = k as f64 / 64.0;
        let s: f64 = v.pieces.iter().map(|p| p.stress.max(0.2) as f64 * (2.0 * std::f64::consts::PI * 2.0 * (p.start - ph)).cos()).sum();
        if s > best.0 + 1e-9 {
            best = (s, ph);
        }
    }
    let ph = best.1;
    let mut bb = (f64::MIN, 0.0);
    for beat in 0..4 {
        let at = ph + beat as f64;
        let s: f64 = v
            .lines
            .iter()
            .map(|l| {
                let d = (l.start - at).rem_euclid(4.0);
                if d < 0.05 || d > 3.95 {
                    1.0
                } else if d >= 3.25 {
                    0.7
                } else if d < 0.3 {
                    0.4
                } else {
                    0.0
                }
            })
            .sum();
        if s > bb.0 + 1e-9 {
            bb = (s, at);
        }
    }
    bb.1
}

/// for every line, how many other lines sing about the same thing (the same number of syllables give or take one, the same pitches
/// within a semitone for three quarters of them, about as long) - the repeated phrases of a recording nobody wrote down
pub fn line_repeats(v: &Vocal) -> Vec<usize> {
    let seq = |li: usize| -> (Vec<f32>, f64) {
        let l = &v.lines[li];
        (v.pieces[l.first..l.end].iter().filter_map(|p| p.chop.midi).collect(), l.stop - l.start)
    };
    let all: Vec<(Vec<f32>, f64)> = (0..v.lines.len()).map(seq).collect();
    (0..all.len())
        .map(|i| {
            (0..all.len())
                .filter(|j| *j != i)
                .filter(|j| {
                    let (a, la) = &all[i];
                    let (b, lb) = &all[*j];
                    if a.is_empty() || b.is_empty() || (a.len() as i64 - b.len() as i64).abs() > 1 || !(0.75..=1.33).contains(&(la / lb.max(1e-6))) {
                        return false;
                    }
                    let m = a.len().min(b.len());
                    let same = (0..m).filter(|k| (a[*k] - b[*k]).abs() <= 1.0).count();
                    same as f64 >= 0.75 * m as f64
                })
                .count()
        })
        .collect()
}

/// what a section does with the acapella
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Vox {
    /// the acapella at 0, the chops carry the vocal
    Chops,
    /// the acapella as sung, no chops
    Full,
    /// the acapella and the chops layered
    Both,
}
impl Vox {
    pub fn sings(self) -> bool {
        self != Vox::Chops
    }
    pub fn chops(self) -> bool {
        self != Vox::Full
    }
    pub fn name(self) -> &'static str {
        match self {
            Vox::Chops => "chops only",
            Vox::Full => "full vocal",
            Vox::Both => "vocal + chops",
        }
    }
}

/// a section as the plan sees it
#[derive(Clone, Debug)]
pub struct SecIn {
    pub tag: String,
    /// a loop roll between two slots (its tag says build or break)
    pub roll: bool,
    pub feel: Feel,
}

/// Which sections are chops only, the full vocal, or both. With `replace` (chops replace the vocal): intros, builds and drops are
/// chops (the last drop of two or more layers the vocal back in: the payoff), verses and breakdowns sing, the roll out of a drop
/// stays chops, an outro is chops; the story moves it - a section whose feeling pushes the voice (vocal >= 0.7) sings even in an
/// intro / outro, a drop that pushes it (>= 0.85) layers it, and a calm instrumental breath (vocal < 0.25, arousal < 0.3) in a break
/// leaves it to the chops. At least one section sings. Without `replace`: every section sings, builds and drops layer the chops.
pub fn plan_vox(secs: &[SecIn], replace: bool) -> Vec<Vox> {
    let n = secs.len();
    let is_drop = |s: &SecIn| !s.roll && s.tag == "drop";
    if !replace {
        return secs.iter().map(|s| if s.tag == "build" || is_drop(s) { Vox::Both } else { Vox::Full }).collect();
    }
    let drops = secs.iter().filter(|s| is_drop(s)).count();
    let last_drop = secs.iter().rposition(is_drop);
    let mut out: Vec<Vox> = secs
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let f = &s.feel;
            match s.tag.as_str() {
                "build" => Vox::Chops,
                _ if s.roll => Vox::Chops,
                "drop" => {
                    if (Some(i) == last_drop && drops >= 2) || f.vocal >= 0.85 {
                        Vox::Both
                    } else {
                        Vox::Chops
                    }
                }
                "intro" | "outro" => {
                    if f.vocal >= 0.7 {
                        Vox::Full
                    } else {
                        Vox::Chops
                    }
                }
                "break" => {
                    if f.vocal < 0.25 && f.arousal < 0.3 {
                        Vox::Chops
                    } else {
                        Vox::Full
                    }
                }
                _ => Vox::Full,
            }
        })
        .collect();
    if n > 0 && !out.iter().any(|v| v.sings()) {
        if let Some(i) = (0..n).filter(|i| secs[*i].tag != "build" && !secs[*i].roll).max_by(|a, b| secs[*a].feel.vocal.total_cmp(&secs[*b].feel.vocal)) {
            out[i] = if secs[i].tag == "drop" { Vox::Both } else { Vox::Full };
        }
    }
    out
}

/// where an acapella plays in a section: the clip at `at` (project beats) for `len` beats, reading the prepared recording from
/// beat `from` (its own grid, the medley's tempo); `pickup` beats of it sound before the section's downbeat
#[derive(Clone, Debug, PartialEq)]
pub struct Excerpt {
    pub at: f64,
    pub from: f64,
    pub len: f64,
    pub pickup: f64,
    pub lines: Vec<usize>,
}

/// The excerpt of every section (start beat, length, hot = sings the hook): the hot sections take the hook (its repeats in turn), the
/// others the lines in order (wrapping round), each from the bar line its first line belongs to (a pickup of up to 3/4 beat kept),
/// as many whole lines as fit before the next one's pickup would cross the section's end. None for a recording with no lines.
pub fn excerpts(v: &Vocal, phase: f64, secs: &[(f64, f64, bool)], hooks: &[usize], total_beats: f64) -> Vec<Option<Excerpt>> {
    let nl = v.lines.len();
    let mut out = vec![];
    let mut ptr = 0usize;
    let mut hk = 0usize;
    for (start, beats, hot) in secs {
        if nl == 0 || *beats < 1.0 {
            out.push(None);
            continue;
        }
        let first = if *hot && !hooks.is_empty() {
            hk += 1;
            hooks[(hk - 1) % hooks.len()]
        } else {
            if ptr >= nl {
                ptr = 0;
            }
            ptr
        };
        let ls = v.lines[first].start;
        let bar = phase + 4.0 * ((ls - phase + 0.75) / 4.0).floor();
        let pickup = (bar - ls).max(0.0);
        let mut last = first;
        while last + 1 < nl && v.lines[last + 1].start < bar + beats - 0.75 - 1e-6 && v.lines[last + 1].stop <= bar + beats + 0.25 {
            last += 1;
        }
        let end = (v.lines[last].stop + 0.5).min(bar + beats).min(total_beats.max(bar + 0.25));
        let len = (end - bar).max(0.25) + pickup;
        out.push(Some(Excerpt { at: start - pickup, from: bar - pickup, len, pickup, lines: (first..=last).collect() }));
        if !*hot {
            ptr = last + 1;
        }
    }
    out
}

/// The acapella's level lane over the sections (start, end) and their plan: `level` where it sings, 0 where the chops carry it -
/// down within the last quarter beat before the downbeat the chops start, back up from the downbeat they end over `fade` beats -
/// and 0 inside `holes` (a line played filtered instead, short fades).
pub fn vol_lane(spans: &[(f64, f64)], plan: &[Vox], level: f32, fade: f64, holes: &[(f64, f64)]) -> Vec<(f64, f32)> {
    let mut pts: Vec<(f64, f32)> = vec![];
    if spans.is_empty() {
        return pts;
    }
    let on = |i: usize| plan.get(i).map(|v| v.sings()).unwrap_or(false);
    pts.push((spans[0].0, if on(0) { level } else { 0.0 }));
    for i in 1..spans.len() {
        let b = spans[i].0;
        match (on(i - 1), on(i)) {
            (true, false) => {
                pts.push((b - 0.25, level));
                pts.push((b, 0.0));
            }
            (false, true) => {
                pts.push((b, 0.0));
                pts.push((b + fade.max(0.02), level));
            }
            _ => {}
        }
    }
    let end = spans[spans.len() - 1].1;
    pts.push((end, if on(spans.len() - 1) { level } else { 0.0 }));
    for (h0, h1) in holes {
        let Some(i) = spans.iter().position(|s| *h0 >= s.0 - 1e-6 && *h0 < s.1) else { continue };
        if !on(i) {
            continue;
        }
        pts.retain(|p| p.0 < h0 - 0.06 || p.0 > h1 + 0.06);
        pts.extend([(h0 - 0.05, level), (*h0, 0.0), (*h1, 0.0), (h1 + 0.05, level)]);
    }
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    pts.dedup_by(|b, a| (b.0 - a.0).abs() < 1e-9);
    pts
}

/// how a line is stressed
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Stress {
    /// tuned copies on the chord tones above (and below, when it is loud)
    Harmony,
    /// the line as the modulator of a chord synth's vocoder
    Vocoder,
    /// a second take: the line again a few ms late, panned
    Double,
    /// the line an octave down, quiet, under it
    Octave,
    /// the line band-passed like a phone (the clean vocal out for it)
    Telephone,
    /// its first syllable stuttered into it on 1/16s
    Stutter,
}
impl Stress {
    pub fn name(self) -> &'static str {
        match self {
            Stress::Harmony => "harmonies",
            Stress::Vocoder => "vocoder",
            Stress::Double => "double",
            Stress::Octave => "octave layer",
            Stress::Telephone => "telephone",
            Stress::Stutter => "stutter",
        }
    }
}

/// which stresses the user allows
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Ways {
    Auto,
    Harmonies,
    Vocoder,
    Doubles,
}
impl Ways {
    pub fn parse(s: &str) -> Ways {
        match s.trim().to_lowercase().as_str() {
            "harmonies" | "harmony" | "harm" => Ways::Harmonies,
            "vocoder" | "voc" => Ways::Vocoder,
            "doubles" | "double" | "dbl" | "octave" => Ways::Doubles,
            _ => Ways::Auto,
        }
    }
}

/// a line that sings in the medley and could be stressed
#[derive(Clone, Debug)]
pub struct Cand {
    pub sec: usize,
    pub line: usize,
    /// project beats
    pub start: f64,
    pub end: f64,
    pub hook: bool,
    pub repeats: usize,
    pub feel: Feel,
    pub vox: Vox,
    /// the last section that sings
    pub last: bool,
}

/// The lines to stress and how (`amount` 0..1: the share of the singing lines, at least one when above 0, at most two per
/// section): the hook (x3), its repeats, the story's push (the feeling's vocal and arousal), the vocal layered in a drop and the
/// last singing section score. Auto picks by the feeling: a layered drop or a dark tense line is vocoded, the hook in a bright or
/// neutral mood gets harmonies, an intense line a double (the hook an octave layer), a calm one the telephone, a repeated phrase a
/// stutter; the same treatment twice in a row moves to the next one that fits. Returns (candidate, stress, why).
pub fn choose(c: &[Cand], amount: f64, ways: Ways) -> Vec<(usize, Stress, &'static str)> {
    if c.is_empty() || amount <= 0.0 {
        return vec![];
    }
    let score = |x: &Cand| -> f64 {
        1.0 + 3.0 * x.hook as i32 as f64 + 0.7 * x.repeats.min(3) as f64 + 1.2 * x.feel.vocal as f64 + 0.8 * x.feel.arousal as f64 + if x.vox == Vox::Both { 1.0 } else { 0.0 } + if x.last { 0.8 } else { 0.0 }
    };
    let want = ((amount.clamp(0.0, 1.0) * c.len() as f64 * 0.6).round() as usize).max(1);
    let mut idx: Vec<usize> = (0..c.len()).collect();
    idx.sort_by(|a, b| score(&c[*b]).total_cmp(&score(&c[*a])).then(a.cmp(b)));
    let mut picked: Vec<usize> = vec![];
    for i in idx {
        if picked.len() >= want {
            break;
        }
        if picked.iter().filter(|p| c[**p].sec == c[i].sec).count() >= 2 {
            continue;
        }
        picked.push(i);
    }
    picked.sort_by(|a, b| c[*a].start.total_cmp(&c[*b].start));
    let mut out: Vec<(usize, Stress, &'static str)> = vec![];
    for i in picked {
        let x = &c[i];
        let f = &x.feel;
        let prefs: Vec<(Stress, &'static str)> = match ways {
            Ways::Harmonies => vec![(Stress::Harmony, "harmonies asked for")],
            Ways::Vocoder => vec![(Stress::Vocoder, "vocoder asked for")],
            Ways::Doubles => vec![if x.hook && f.arousal >= 0.6 { (Stress::Octave, "the hook, loud") } else { (Stress::Double, "doubles asked for") }],
            Ways::Auto => {
                let mut p: Vec<(Stress, &'static str)> = vec![];
                if x.vox == Vox::Both || (f.tension >= 0.6 && f.valence < 0.1) {
                    p.push((Stress::Vocoder, if x.vox == Vox::Both { "the vocal back in a drop" } else { "a dark, tense line" }));
                }
                if x.hook && f.valence >= -0.1 {
                    p.push((Stress::Harmony, "the hook"));
                }
                if f.arousal >= 0.65 {
                    p.push(if x.hook { (Stress::Octave, "the hook at the peak") } else { (Stress::Double, "an intense line") });
                }
                if f.arousal < 0.35 {
                    p.push((Stress::Telephone, "a distant, quiet line"));
                }
                if x.repeats >= 1 {
                    p.push((Stress::Stutter, "a repeated phrase"));
                }
                if x.feel.vocal >= 0.5 {
                    p.push((Stress::Harmony, "the story's push"));
                }
                p.push((Stress::Double, "a line to lift"));
                p
            }
        };
        let last = out.last().map(|o| o.1);
        let pick = prefs.iter().find(|p| Some(p.0) != last).or(prefs.first()).copied().unwrap_or((Stress::Double, ""));
        out.push((i, pick.0, pick.1));
    }
    out
}

/// the note a harmony voice sings over a voice heard at `heard` (MIDI): the first chord tone 3 .. 7 semitones above (or below)
pub fn harmony_note(heard: f32, tones: &[u8], up: bool) -> Option<i32> {
    let r = heard.round() as i32;
    let range: Vec<i32> = if up { (r + 3..=r + 7).collect() } else { (r - 7..=r - 3).rev().collect() };
    range.into_iter().find(|p| tones.contains(&(p.rem_euclid(12) as u8)))
}

/// a steady stretch of the line: frames [a, b) of the recording and its pitch (MIDI), None when unvoiced
#[derive(Clone, Debug, PartialEq)]
pub struct Seg {
    pub a: usize,
    pub b: usize,
    pub midi: Option<f32>,
}

/// The line `stereo[a..b]` cut into steady notes: the pitch track (vocal::track_pitch, 5 ms) split where the pitch moves more than
/// 0.7 semitone from the note's median for 15 ms or the voicing changes; stretches under 40 ms join their neighbour.
pub fn segments(stereo: &[f32], rate: u32, a: usize, b: usize) -> Vec<Seg> {
    let frames = stereo.len() / 2;
    let (a, b) = (a.min(frames), b.min(frames));
    if b <= a + 64 {
        return vec![];
    }
    let mono: Vec<f32> = (a..b).map(|f| 0.5 * (stereo[f * 2] + stereo[f * 2 + 1])).collect();
    let p = crate::vocal::track_pitch(&mono, rate as f32);
    let hop = ((p.hop * rate as f64).round() as usize).max(1);
    let m: Vec<Option<f32>> = p.f0.iter().map(|f| (*f > 60.0 && *f < 1400.0).then(|| 69.0 + 12.0 * (*f / 440.0).log2())).collect();
    let med = |v: &[f32]| -> f32 {
        let mut s = v.to_vec();
        s.sort_by(f32::total_cmp);
        s.get(s.len() / 2).copied().unwrap_or(0.0)
    };
    let len = m.len();
    let mut segs: Vec<(usize, usize, Vec<f32>)> = vec![];
    let mut s = 0usize;
    let mut vals: Vec<f32> = m.first().copied().flatten().into_iter().collect();
    let mut j = 1usize;
    while j < len {
        if m[j].is_some() != m[s].is_some() {
            segs.push((s, j, std::mem::take(&mut vals)));
            s = j;
            vals.extend(m[j]);
            j += 1;
            continue;
        }
        if let Some(v) = m[j] {
            if vals.len() >= 3 && j + 3 <= len {
                let md = med(&vals);
                if (j..j + 3).all(|k| m[k].map(|w| (w - md).abs() > 0.7).unwrap_or(false)) {
                    segs.push((s, j, std::mem::take(&mut vals)));
                    s = j;
                }
            }
            vals.push(v);
        }
        j += 1;
    }
    segs.push((s, len, vals));
    // join the short ones
    let min = (0.04 / p.hop).ceil() as usize;
    let mut out: Vec<(usize, usize, Vec<f32>)> = vec![];
    for s in segs {
        if s.1 <= s.0 {
            continue;
        }
        match out.last_mut() {
            Some(l) if s.1 - s.0 < min || l.1 - l.0 < min => {
                l.1 = s.1;
                l.2.extend(s.2);
            }
            _ => out.push(s),
        }
    }
    out.into_iter()
        .map(|(s, e, v)| {
            let mut v = v;
            v.sort_by(f32::total_cmp);
            let midi = (v.len() >= 3 && v.len() * 2 >= (e - s)).then(|| v[v.len() / 2]);
            Seg { a: a + s * hop, b: (a + e * hop).min(b), midi }
        })
        .collect()
}

/// A harmony voice: every steady note of the line moved to `target(segment, heard pitch)` (None = left out), cut with 15 ms of
/// margin, shifted keeping its length (stretch::pitch_shift, padded so the WSOLA has room), faded 10 ms at both ends and added in
/// place; an unvoiced stretch (a consonant, a breath) takes the shift of the note after it (else before it). The output is as long as
/// `stereo`; frames outside [a, b) are silent. Also the (segment, shift in semitones) used.
pub fn harmonize(stereo: &[f32], rate: u32, a: usize, b: usize, target: &dyn Fn(&Seg) -> Option<f32>) -> (Vec<f32>, Vec<(Seg, f32)>) {
    let frames = stereo.len() / 2;
    let mut out = vec![0f32; stereo.len()];
    let segs = segments(stereo, rate, a, b);
    let shifts: Vec<Option<f32>> = segs.iter().map(|s| s.midi.and_then(|m| target(s).map(|t| t - m))).collect();
    let mut used = vec![];
    let margin = (rate as f64 * 0.015) as usize;
    let fade = (rate as f64 * 0.010) as usize;
    for (k, s) in segs.iter().enumerate() {
        let sh = match (s.midi, shifts[k]) {
            (Some(_), Some(x)) => Some(x),
            (Some(_), None) => None,
            (None, _) => shifts[k + 1..].iter().zip(&segs[k + 1..]).find(|(_, g)| g.midi.is_some()).and_then(|(x, _)| *x).or_else(|| shifts[..k].iter().rev().zip(segs[..k].iter().rev()).find(|(_, g)| g.midi.is_some()).and_then(|(x, _)| *x)),
        };
        let Some(sh) = sh else { continue };
        let (s0, s1) = (s.a.saturating_sub(margin), (s.b + margin).min(frames));
        if s1 <= s0 + 8 {
            continue;
        }
        let n = s1 - s0;
        let pad = 4200usize.saturating_sub(n) / 2 + 64;
        let mut seg = vec![0f32; pad * 2];
        seg.extend_from_slice(&stereo[s0 * 2..s1 * 2]);
        seg.extend(vec![0f32; pad * 2]);
        let y = crate::stretch::pitch_shift(&seg, sh as f64);
        for i in 0..n {
            let src = pad + i;
            if src * 2 + 1 >= y.len() {
                break;
            }
            let w = if i < fade { i as f32 / fade as f32 } else if i + fade > n { (n - i) as f32 / fade as f32 } else { 1.0 };
            out[(s0 + i) * 2] += y[src * 2] * w;
            out[(s0 + i) * 2 + 1] += y[src * 2 + 1] * w;
        }
        used.push((s.clone(), sh));
    }
    (out, used)
}

/// the notes the vocal sings between two of its beats (for the key fit): a note per pitched piece, relative to `from`
pub fn sung_notes(v: &Vocal, from: f64, to: f64) -> Vec<Note> {
    v.pieces
        .iter()
        .filter(|p| p.start >= from - 1e-6 && p.start < to && p.chop.midi.is_some())
        .map(|p| Note::new(p.start - from, p.len.max(0.1), p.chop.midi.map(|m| m.round().clamp(0.0, 127.0) as u8).unwrap_or(60), 0.8))
        .collect()
}

/// a stutter into a line: its first pitched syllable three times on the 1/16s before the line (tuned to the chord there, the
/// acapella's own shift kept), louder each time; notes relative to `sec0`
pub fn stutter_into(v: &Vocal, line: usize, at: f64, sec0: f64, key: (u8, bool), harmony: &[Note], shift: i32, tune: &chops::Tune) -> Vec<Note> {
    let Some(l) = v.lines.get(line) else { return vec![] };
    let Some(pi) = (l.first..l.end).find(|p| v.pieces[*p].chop.midi.is_some()) else { return vec![] };
    let p = &v.pieces[pi];
    let k = Key::new(key.0, key.1);
    let mut out = vec![];
    for j in 0..3 {
        let t = at - (3 - j) as f64 * 0.25;
        if t < sec0 {
            continue;
        }
        let rel = t - sec0;
        let f0 = p.chop.midi.unwrap_or(60.0);
        let tones = tune.tones(k, harmony, rel);
        let target = tune.nearest(f0 + shift as f32, &tones);
        let mut c = p.chop.clone();
        let dur = 0.23 * 60.0 / v.bpm.max(1.0);
        c.end = (c.start + (dur * v.rate as f64) as u32).min(c.end).max(c.start + 1);
        out.push(chops::tuned_note(&c, rel, 0.22, target, 0.55 + 0.15 * j as f32, tune, 0).0);
    }
    out
}

/// the chord a vocoder's carrier holds at `t` (pitch classes from the harmony, else the key's triad), voiced from 55 up with the
/// root an octave below
pub fn carrier_chord(key: (u8, bool), harmony: &[Note], t: f64) -> Vec<u8> {
    let k = Key::new(key.0, key.1);
    let pcs = chops::chord_at(k, harmony, t).unwrap_or_else(|| {
        let sc = k.scale();
        vec![sc[0], sc[2], sc[4]]
    });
    let root = harmony.iter().filter(|n| n.start <= t + 1e-6 && n.start + n.len > t + 1e-6).map(|n| n.key).min().map(|b| b % 12).unwrap_or(pcs[0]);
    let mut v: Vec<u8> = pcs.iter().map(|p| 55 + (*p + 12 - 7) % 12).collect();
    v.push(43 + (root + 12 - 7) % 12);
    v.sort_unstable();
    v.dedup();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn feel(vocal: f32, arousal: f32, valence: f32, tension: f32) -> Feel {
        Feel { valence, arousal, tension, vocal, breath: 0.3, space: 0.4 }
    }

    #[test]
    fn tempo_from_names_and_ratios() {
        assert_eq!(bpm_from_name("my_vocal_128bpm"), Some(128.0));
        assert_eq!(bpm_from_name("Vox 92 BPM dry"), Some(92.0));
        assert_eq!(bpm_from_name("acapella bpm-140"), Some(140.0));
        assert_eq!(bpm_from_name("acapella"), None);
        assert_eq!(bpm_from_name("track 12 bpm"), None);
        let (c, r) = tempo_ratio(87.0, 174.0);
        assert_eq!(c, 174.0);
        assert!((r - 1.0).abs() < 1e-9);
        let (c, r) = tempo_ratio(100.0, 125.0);
        assert_eq!(c, 100.0);
        assert!((r - 0.8).abs() < 1e-9, "a 100 bpm vocal plays 0.8 as long at 125");
    }

    #[test]
    fn the_pulse_of_clicks_is_measured() {
        let rate = 22050u32;
        for bpm in [96.0f64, 124.0, 140.0] {
            let secs = 12.0;
            let mut x = vec![0f32; (secs * rate as f64) as usize * 2];
            let beat = 60.0 / bpm;
            let mut t = 0.0;
            let mut i = 0;
            while t < secs - 0.2 {
                let f0 = (t * rate as f64) as usize;
                let amp = if i % 2 == 0 { 0.8 } else { 0.5 };
                for k in 0..(0.12 * rate as f64) as usize {
                    let v = amp * (1.0 - k as f32 / (0.12 * rate as f32)) * (k as f32 * 0.07).sin();
                    if (f0 + k) * 2 + 1 < x.len() {
                        x[(f0 + k) * 2] += v;
                        x[(f0 + k) * 2 + 1] += v;
                    }
                }
                t += beat / 2.0;
                i += 1;
            }
            let (got, conf) = detect_bpm(&x, rate).unwrap();
            let (c, _) = tempo_ratio(got, bpm);
            assert!((c / bpm - 1.0).abs() < 0.02, "{bpm}: measured {got} (conf {conf})");
        }
    }

    fn secs(tags: &[(&str, bool)], f: Feel) -> Vec<SecIn> {
        tags.iter().map(|(t, r)| SecIn { tag: t.to_string(), roll: *r, feel: f }).collect()
    }

    #[test]
    fn the_chops_replace_the_vocal_until_it_comes_back() {
        let f = feel(0.5, 0.5, 0.0, 0.4);
        let s = secs(&[("intro", false), ("verse", false), ("build", true), ("drop", false), ("break", true), ("break", false), ("build", true), ("drop", false), ("outro", false)], f);
        let p = plan_vox(&s, true);
        assert_eq!(p, vec![Vox::Chops, Vox::Full, Vox::Chops, Vox::Chops, Vox::Chops, Vox::Full, Vox::Chops, Vox::Both, Vox::Chops]);
        // without the switch the vocal sings everywhere, layered with the chops in builds and drops
        let q = plan_vox(&s, false);
        assert!(q.iter().all(|v| v.sings()) && q[3] == Vox::Both && q[1] == Vox::Full);
        // the story pushes the voice into the intro; a quiet breath in a break leaves it to the chops
        let mut s2 = s.clone();
        s2[0].feel = feel(0.8, 0.4, 0.2, 0.3);
        s2[5].feel = feel(0.1, 0.2, 0.0, 0.1);
        let p2 = plan_vox(&s2, true);
        assert_eq!(p2[0], Vox::Full);
        assert_eq!(p2[5], Vox::Chops);
        // something always sings
        let s3 = secs(&[("intro", false), ("build", true), ("drop", false)], feel(0.3, 0.5, 0.0, 0.3));
        assert!(plan_vox(&s3, true).iter().any(|v| v.sings()));
    }

    #[test]
    fn the_level_drops_to_zero_under_the_chops_and_fades_back_on_the_downbeat() {
        let spans = [(0.0, 16.0), (16.0, 48.0), (48.0, 80.0), (80.0, 112.0)];
        let plan = [Vox::Full, Vox::Chops, Vox::Chops, Vox::Full];
        let lane = crate::model::AutoLane { param: "vol".into(), points: vol_lane(&spans, &plan, 0.9, 0.5, &[(4.0, 6.0)]) };
        assert!((lane.value_at(2.0).unwrap() - 0.9).abs() < 1e-6);
        assert_eq!(lane.value_at(5.0).unwrap(), 0.0, "a telephone line takes the clean vocal out");
        assert!((lane.value_at(8.0).unwrap() - 0.9).abs() < 1e-6);
        assert!((lane.value_at(15.74).unwrap() - 0.9).abs() < 0.01);
        for b in [16.0, 30.0, 48.0, 70.0, 79.99, 80.0] {
            assert_eq!(lane.value_at(b).unwrap(), 0.0, "{b}");
        }
        assert!(lane.value_at(80.25).unwrap() > 0.3 && lane.value_at(80.25).unwrap() < 0.9, "fading in");
        assert!((lane.value_at(80.5).unwrap() - 0.9).abs() < 1e-6 && (lane.value_at(100.0).unwrap() - 0.9).abs() < 1e-6);
    }

    #[test]
    fn stressed_lines_follow_the_hook_and_the_feeling() {
        let c = vec![
            Cand { sec: 1, line: 0, start: 16.0, end: 20.0, hook: false, repeats: 0, feel: feel(0.3, 0.3, 0.0, 0.3), vox: Vox::Full, last: false },
            Cand { sec: 1, line: 1, start: 20.0, end: 24.0, hook: true, repeats: 2, feel: feel(0.6, 0.5, 0.3, 0.3), vox: Vox::Full, last: false },
            Cand { sec: 5, line: 1, start: 90.0, end: 94.0, hook: true, repeats: 2, feel: feel(0.9, 0.9, 0.2, 0.7), vox: Vox::Both, last: true },
            Cand { sec: 5, line: 2, start: 94.0, end: 98.0, hook: false, repeats: 0, feel: feel(0.9, 0.9, 0.2, 0.7), vox: Vox::Both, last: true },
        ];
        let p = choose(&c, 0.9, Ways::Auto);
        assert!(p.iter().any(|x| x.0 == 1 && x.1 == Stress::Harmony), "{p:?}");
        assert!(p.iter().any(|x| x.0 == 2 && x.1 == Stress::Vocoder), "{p:?}");
        assert!(!p.iter().any(|x| x.0 == 0), "the plain verse line is left alone: {p:?}");
        assert!(choose(&c, 0.0, Ways::Auto).is_empty());
        assert!(choose(&c, 1.0, Ways::Harmonies).iter().all(|x| x.1 == Stress::Harmony));
        assert_eq!(harmony_note(64.1, &[0, 4, 7], true), Some(67));
        assert_eq!(harmony_note(64.1, &[0, 4, 7], false), Some(60));
        assert_eq!(harmony_note(62.0, &[0, 4, 7], true), Some(67));
    }

    fn sine_line(rate: u32, notes: &[(f64, f64, f32)]) -> Vec<f32> {
        let end = notes.iter().map(|n| n.1).fold(0.0, f64::max) + 0.3;
        let mut x = vec![0f32; (end * rate as f64) as usize * 2];
        let mut ph = 0f64;
        for (s, e, m) in notes {
            let hz = 440.0 * 2f64.powf((*m as f64 - 69.0) / 12.0);
            let (a, b) = ((s * rate as f64) as usize, (e * rate as f64) as usize);
            for i in a..b {
                let env = ((i - a) as f32 / 400.0).min(1.0) * ((b - i) as f32 / 400.0).min(1.0);
                ph += hz / rate as f64;
                let v = 0.4 * env * ((ph * std::f64::consts::TAU).sin() as f32 + 0.3 * (ph * 2.0 * std::f64::consts::TAU).sin() as f32 + 0.15 * (ph * 3.0 * std::f64::consts::TAU).sin() as f32);
                x[i * 2] = v;
                x[i * 2 + 1] = v;
            }
        }
        x
    }

    #[test]
    fn a_harmony_lands_on_its_targets_note_by_note() {
        let rate = 22050u32;
        let x = sine_line(rate, &[(0.1, 0.6, 64.2), (0.6, 1.1, 62.0), (1.2, 1.9, 60.0)]);
        let segs = segments(&x, rate, 0, x.len() / 2);
        let pitched: Vec<&Seg> = segs.iter().filter(|s| s.midi.is_some()).collect();
        assert_eq!(pitched.len(), 3, "{segs:?}");
        let tones = [0u8, 4, 7];
        let (y, used) = harmonize(&x, rate, 0, x.len() / 2, &|s: &Seg| s.midi.and_then(|m| harmony_note(m, &tones, true)).map(|p| p as f32));
        assert_eq!(used.iter().filter(|u| u.0.midi.is_some()).count(), 3, "{used:?}");
        let mono: Vec<f32> = y.chunks(2).map(|c| 0.5 * (c[0] + c[1])).collect();
        for (s, sh) in used.iter().filter(|u| u.0.midi.is_some()) {
            let want = s.midi.unwrap() + sh;
            assert!(tones.contains(&((want.round() as i32).rem_euclid(12) as u8)));
            let f = crate::chops::chop_f0(&mono, s.a + 1500, s.b - 1500, rate).expect("voiced");
            assert!((f.midi - want).abs() < 0.25, "harmony at {} wanted {want}", f.midi);
        }
        // nothing outside the line
        assert!(y[..(0.05 * rate as f64) as usize * 2].iter().all(|v| v.abs() < 1e-6));
    }

    #[test]
    fn excerpts_read_whole_lines_from_the_bar() {
        // a vocal of four 2-bar lines, each starting half a beat before its bar (a pickup)
        let mut v = Vocal { rate: 22050, bpm: 120.0, ..Vocal::default() };
        for l in 0..4 {
            let bar = 8.0 * l as f64 + 4.0;
            for k in 0..4 {
                let st = bar - 0.5 + k as f64 * 1.5;
                v.pieces.push(chops::Piece { chop: chops::Chop { start: 0, end: 1, midi: Some(60.0 + k as f32), level: 0.5 }, text: String::new(), word: v.pieces.len(), nth: 0, of: 1, start: st, len: 1.2, key: 60, stress: 0.8 });
            }
            let n = v.pieces.len();
            v.lines.push(chops::Line { first: n - 4, end: n, start: bar - 0.5, stop: bar - 0.5 + 4.5 + 1.2 });
        }
        let ph = bar_phase(&v);
        assert!((ph.rem_euclid(4.0) - 0.0).abs() < 1e-6, "bar phase {ph}");
        let ex = excerpts(&v, ph, &[(100.0, 16.0, false), (116.0, 8.0, true), (124.0, 16.0, false)], &[3], 40.0);
        let e0 = ex[0].as_ref().unwrap();
        assert_eq!(e0.lines, vec![0, 1]);
        assert!((e0.pickup - 0.5).abs() < 1e-9 && (e0.at - 99.5).abs() < 1e-9 && (e0.from - 3.5).abs() < 1e-9, "{e0:?}");
        assert!(e0.len <= 16.5 + 1e-9);
        let e1 = ex[1].as_ref().unwrap();
        assert_eq!(e1.lines, vec![3], "the hot section sings the hook");
        let e2 = ex[2].as_ref().unwrap();
        assert_eq!(e2.lines, vec![2, 3], "the lines go on in order");
        let reps = line_repeats(&v);
        assert!(reps.iter().all(|r| *r == 3), "four lines sung alike: {reps:?}");
    }
}
