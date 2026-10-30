//! Production edits: the sampler moves a producer puts on vocal chops, chosen for the song (Vocal Chops, New AI Song's VOCAL CHOPS,
//! Make Medley; option `edits=auto|light|more|off|<list>`).
//!
//! * What there is ([`Kind`]): a reverse swell into a downbeat, a tape stop or a pitch dive at an ending, a pitch-up riser over a
//!   build, an octave-jump call, a chipmunk double of the hook (an octave up, high-passed: the pitch shift moves the formants with
//!   it), a stutter that decays like an echo, half-time and double-time phrases, a trance gate on a held vowel, a low-pass sweep
//!   across the run-up to a drop, glides between the long chops of a break, a sample flip (syllables of other words re-sequenced
//!   into a new hook melody on the chord tones), a held vowel as a pad under a break, one-shot ear candy at phrase ends, call and
//!   response between two acapellas (Make Medley), choke-group cuts, an air pocket of silence before a drop and an echo throw on
//!   the last word before a change.
//! * Where ([`plan`]): by the section's place in the song first - the run-up to a drop gets a riser over its build, else a swell
//!   and a filter sweep, and every other drop an air pocket; a section the energy falls out of (or a medley's change of song) ends
//!   in a tape stop or an echo throw; the song's last chops dive - then by a budget per section: the first chorus gets one subtle
//!   edit (ear candy, a choke cut, a stutter, an octave jump), later ones two or three of the bolder ones and never the ones the
//!   chorus before had, breaks a pad or a gate and glides or a throw, verses one throw or a fill, builds nothing more. The genre
//!   weighs every choice ([`weight`]: trance gates in the club genres, half time and chipmunk doubles in trap and bass music,
//!   glides and pads in lo-fi, nothing but throws, pads, glides and swells in the acoustic genres), and what the vocal offers
//!   decides too ([`Content`]: a pad, a gate or a riser needs a clean sustained vowel, a flip three clean words, a reverse swell
//!   the Slicer or a reversed copy). Song-wide caps keep every edit rare.
//! * How ([`apply`]): on the section's composed chop notes (Sampler slice lyrics, before chops/kit.rs turns them into a Slicer
//!   kit). Notes keep the slice lyric; an edit that needs more of the Slicer adds a flag (kit.rs: `r` reverse, `l` loop, `o`
//!   one-shot, `x` choke group 3) or a pitch curve (Note::bend: the Slicer plays it varispeed, so a tape stop slows down as it
//!   drops). Risers, pads, gates, doubles and ear candy go to an edits track of their own (a high-pass and a little reverb,
//!   [`dress_fx_track`]); sweeps are the Slicer's cutoff lane on the chop tracks, throws a Delay whose mix lane opens on the
//!   word ([`add_throws`]), air pockets cut the chops (and in a medley every part) before the drop.
use super::{genre_feel, role_of, tuned_note, Chop, Role, Tune, Vocal};
use crate::compose::Rng;
use crate::model::{AutoLane, Curve, EffectSlot, Note, Track};
use crate::theory::Key;
use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
pub enum Kind {
    Swell,
    TapeStop,
    Dive,
    Riser,
    Octave,
    Chipmunk,
    Stutter,
    HalfTime,
    DoubleTime,
    Gate,
    Sweep,
    Glide,
    Flip,
    Pad,
    Candy,
    Response,
    Choke,
    Pocket,
    Throw,
}
use Kind::*;

pub const KINDS: [Kind; 19] = [Swell, TapeStop, Dive, Riser, Octave, Chipmunk, Stutter, HalfTime, DoubleTime, Gate, Sweep, Glide, Flip, Pad, Candy, Response, Choke, Pocket, Throw];

impl Kind {
    /// the option word
    pub fn name(self) -> &'static str {
        match self {
            Swell => "swell",
            TapeStop => "tapestop",
            Dive => "dive",
            Riser => "riser",
            Octave => "octave",
            Chipmunk => "chipmunk",
            Stutter => "stutter",
            HalfTime => "halftime",
            DoubleTime => "doubletime",
            Gate => "gate",
            Sweep => "sweep",
            Glide => "glide",
            Flip => "flip",
            Pad => "pad",
            Candy => "candy",
            Response => "response",
            Choke => "choke",
            Pocket => "pocket",
            Throw => "throw",
        }
    }
    /// what the report calls one and several
    fn words(self) -> (&'static str, &'static str) {
        match self {
            Swell => ("reverse swell", "reverse swells"),
            TapeStop => ("tape stop", "tape stops"),
            Dive => ("pitch dive", "pitch dives"),
            Riser => ("pitch riser", "pitch risers"),
            Octave => ("octave jump", "octave jumps"),
            Chipmunk => ("chipmunk double", "chipmunk doubles"),
            Stutter => ("decaying stutter", "decaying stutters"),
            HalfTime => ("half-time phrase", "half-time phrases"),
            DoubleTime => ("double-time phrase", "double-time phrases"),
            Gate => ("trance gate", "trance gates"),
            Sweep => ("filter sweep", "filter sweeps"),
            Glide => ("glide phrase", "glide phrases"),
            Flip => ("sample flip", "sample flips"),
            Pad => ("vocal pad", "vocal pads"),
            Candy => ("ear candy fill", "ear candy fills"),
            Response => ("call and response", "calls and responses"),
            Choke => ("choke cut", "choke cuts"),
            Pocket => ("air pocket", "air pockets"),
            Throw => ("echo throw", "echo throws"),
        }
    }
    /// an option word (or a common name for it: "reverse", "tape stop", "formant", "trance gate", "echo" ...)
    pub fn parse(w: &str) -> Option<Kind> {
        let w: String = w.trim().to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect();
        if let Some(k) = KINDS.iter().copied().find(|k| k.name() == w) {
            return Some(k);
        }
        Some(match w.as_str() {
            "reverse" | "reverseswell" | "swells" => Swell,
            "tape" | "tapestops" => TapeStop,
            "pitchdive" | "dives" => Dive,
            "pitchup" | "risers" => Riser,
            "octavejump" | "octaves" => Octave,
            "formant" | "chipmunks" | "doubles" => Chipmunk,
            "stutters" | "echostutter" => Stutter,
            "half" => HalfTime,
            "doubletimes" => DoubleTime,
            "trancegate" | "gates" => Gate,
            "filter" | "filtersweep" | "sweeps" => Sweep,
            "porta" | "portamento" | "glides" => Glide,
            "sampleflip" | "flips" => Flip,
            "drone" | "pads" => Pad,
            "earcandy" | "fill" | "fills" => Candy,
            "callresponse" | "callandresponse" => Response,
            "chokes" => Choke,
            "airpocket" | "silence" => Pocket,
            "echo" | "echothrow" | "throws" | "delay" => Throw,
            _ => return None,
        })
    }
}

/// Which edits the option asks for: `auto` (the default: the planner chooses), `light` (half the budget), `more` (half again
/// as much), `off`, or a list of edits ("swell,tapestop,throw": only those, where they fit)
#[derive(Clone, Debug, PartialEq)]
pub struct Which {
    pub on: bool,
    pub only: Vec<Kind>,
    pub scale: f64,
}
impl Default for Which {
    fn default() -> Which {
        Which { on: true, only: vec![], scale: 1.0 }
    }
}
impl Which {
    pub fn off() -> Which {
        Which { on: false, only: vec![], scale: 0.0 }
    }
    pub fn parse(s: &str) -> Which {
        match s.trim().to_lowercase().as_str() {
            "" | "auto" | "on" | "1" | "yes" | "true" => Which::default(),
            "off" | "0" | "no" | "none" | "false" => Which::off(),
            "light" | "subtle" | "few" => Which { scale: 0.5, ..Which::default() },
            "more" | "lots" | "bold" => Which { scale: 1.5, ..Which::default() },
            list => {
                let mut only: Vec<Kind> = list.split(|c: char| c == ',' || c == '+' || c == '|' || c == '/').filter_map(Kind::parse).collect();
                only.sort();
                only.dedup();
                Which { on: !only.is_empty(), only, scale: 1.0 }
            }
        }
    }
    pub fn allows(&self, k: Kind) -> bool {
        self.on && (self.only.is_empty() || self.only.contains(&k))
    }
}

/// the genre families the edits are weighed for
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fam {
    /// house, techno, trance, garage, big room
    Club,
    /// dubstep, drum and bass, future bass, hyperpop, breakbeat
    Bass,
    /// trap, drill, hip hop, R&B
    Urban,
    Pop,
    /// lo-fi, ambient, chill, downtempo
    Chill,
    /// ballads, folk, jazz, rock, film music
    Acoustic,
}

pub fn family(genre: &str) -> Fam {
    let g = genre.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| g.contains(w));
    if has(&["lofi", "lo-fi", "ambient", "chill", "downtempo"]) {
        Fam::Chill
    } else if has(&["ballad", "folk", "jazz", "rock", "acoustic", "cinematic", "orchestral", "country", "film"]) {
        Fam::Acoustic
    } else if has(&["trap", "drill", "phonk", "hip hop", "hip-hop", "rap", "r&b", "rnb", "boom bap"]) {
        Fam::Urban
    } else if has(&["dubstep", "riddim", "drum and bass", "dnb", "jungle", "future bass", "hyperpop", "breakbeat", "breakcore", "electro"]) {
        Fam::Bass
    } else if has(&["house", "techno", "trance", "garage", "dance", "edm", "big room", "hardstyle"]) {
        Fam::Club
    } else {
        Fam::Pop
    }
}

/// How much a genre family likes an edit, 0 (never) .. 1
pub fn weight(f: Fam, k: Kind) -> f64 {
    // club, bass, urban, pop, chill, acoustic
    let w: [f64; 6] = match k {
        Swell => [1.0, 1.0, 0.8, 1.0, 0.7, 0.5],
        TapeStop => [0.5, 0.8, 1.0, 0.6, 1.0, 0.0],
        Dive => [0.6, 1.0, 0.9, 0.7, 0.8, 0.3],
        Riser => [1.0, 1.0, 0.5, 0.8, 0.3, 0.0],
        Octave => [0.7, 1.0, 0.6, 0.8, 0.3, 0.0],
        Chipmunk => [0.6, 1.0, 1.0, 0.9, 0.5, 0.0],
        Stutter => [0.8, 1.0, 0.9, 0.6, 0.3, 0.0],
        HalfTime => [0.2, 1.0, 1.0, 0.4, 0.6, 0.0],
        DoubleTime => [0.6, 1.0, 0.3, 0.6, 0.0, 0.0],
        Gate => [1.0, 0.4, 0.0, 0.4, 0.3, 0.0],
        Sweep => [1.0, 0.9, 0.5, 0.8, 0.8, 0.3],
        Glide => [0.3, 0.8, 1.0, 0.8, 1.0, 0.6],
        Flip => [0.8, 1.0, 1.0, 0.9, 0.7, 0.0],
        Pad => [0.8, 0.8, 0.5, 0.7, 1.0, 0.8],
        Candy => [0.8, 1.0, 1.0, 0.8, 0.6, 0.2],
        Response => [1.0, 1.0, 1.0, 1.0, 1.0, 1.0],
        Choke => [0.6, 1.0, 1.0, 0.5, 0.5, 0.0],
        Pocket => [0.9, 1.0, 1.0, 0.8, 0.3, 0.0],
        Throw => [0.8, 0.8, 1.0, 1.0, 1.0, 1.0],
    };
    w[f as usize]
}

/// the most of an edit in one song (the budget scales it)
fn cap(k: Kind) -> usize {
    match k {
        Dive | HalfTime => 1,
        TapeStop | Chipmunk | DoubleTime | Gate | Glide | Flip | Pad | Choke | Pocket => 2,
        Riser | Octave | Stutter | Sweep => 3,
        Swell | Candy | Response | Throw => 4,
    }
}

/// What the vocal offers the edits
#[derive(Clone, Copy, Debug, Default)]
pub struct Content {
    /// a clean sung vowel long enough to hold (a pad, a gate, a riser)
    pub sustain: bool,
    /// a pitched syllable at all (glides, octaves, doubles)
    pub pitched: bool,
    /// clean pitched words besides the hook's (a flip wants three)
    pub words: usize,
    /// a short clean hit for ear candy
    pub hits: bool,
    /// the acapellas (call and response wants two)
    pub voices: usize,
    /// the chops play on Ling Slicer: loops, reverse and one-shots work
    pub slicer: bool,
    /// a reverse swell can play (the Slicer, or reversed copies in the bank)
    pub reverse: bool,
}
impl Content {
    pub fn of(v: &Vocal, slicer: bool, voices: usize) -> Content {
        let rate = v.rate.max(1) as f64;
        let dur = |pi: usize| (v.pieces[pi].chop.end - v.pieces[pi].chop.start) as f64 / rate;
        let n = v.pieces.len();
        let sustain = (0..n).any(|p| v.pieces[p].chop.midi.is_some() && v.clean_of(p) >= 0.55 && dur(p) >= 0.18);
        let pitched = (0..n).any(|p| v.pieces[p].chop.midi.is_some() && v.clean_of(p) >= 0.35);
        let words = (0..v.words.len()).filter(|w| Some(*w) != v.hook_word && v.word_pieces(*w).any(|p| v.pieces[p].chop.midi.is_some() && v.clean_of(p) >= 0.5)).count();
        let hits = (0..n).any(|p| v.clean_of(p) >= 0.3 && dur(p) <= 0.35);
        Content { sustain, pitched, words, hits, voices, slicer, reverse: slicer || !v.rev.is_empty() }
    }
    fn offers(&self, k: Kind) -> bool {
        match k {
            Riser | Pad | Gate => self.slicer && self.sustain,
            Dive | Glide | Octave | Chipmunk => self.pitched,
            Swell => self.reverse,
            Flip => self.words >= 3,
            Candy => self.hits,
            Response => self.voices >= 2,
            _ => true,
        }
    }
}

/// A section as the planner sees it
#[derive(Clone, Debug, Default)]
pub struct Spot {
    pub tag: String,
    pub beats: f64,
    pub energy: f64,
    /// the section has chops of its own (else only what leads out of it: a swell, a sweep, a pocket)
    pub chopped: bool,
    /// the song (a medley's source file) it comes from: a change is a transition; 0 in a song
    pub song: usize,
    /// its chops build up (DJ loops, ending in a breath of silence), end in a reverse swell, bend up already
    pub build: bool,
    pub swell: bool,
    pub riser: bool,
    /// an acapella sings it (Make Medley)
    pub sings: bool,
}

fn role_ix(r: Role) -> usize {
    match r {
        Role::Hot => 0,
        Role::Build => 1,
        Role::Break => 2,
        Role::Calm => 3,
    }
}

/// The edits of every section of a song: by where each section sits, a budget per section and the genre (see the module notes).
pub fn plan(spots: &[Spot], genre: &str, w: &Which, ct: &Content, seed: u64) -> Vec<Vec<Kind>> {
    let n = spots.len();
    let mut per: Vec<Vec<Kind>> = vec![vec![]; n];
    if !w.on || n == 0 {
        return per;
    }
    let fam = family(genre);
    let wt = |k: Kind| -> f64 {
        let x = weight(fam, k);
        // an edit asked for by name plays where it fits, whatever the genre
        if w.only.contains(&k) { x.max(0.6) } else { x }
    };
    let mut rng = Rng::new(seed ^ 0xED17_5EED);
    let mut count: HashMap<Kind, usize> = HashMap::new();
    let limit = |k: Kind| ((cap(k) as f64 * w.scale.max(0.5)).round() as usize).max(1);
    let last_chopped = (0..n).rev().find(|i| spots[*i].chopped);
    let mut drops = 0usize;
    let mut last_end: Option<Kind> = None;
    let mut nth = [0usize; 4];
    let mut before: [Vec<Kind>; 4] = Default::default();
    let mut responded = false;
    for i in 0..n {
        let s = &spots[i];
        let role = role_of(&s.tag);
        let next = spots.get(i + 1);
        let mut mine: Vec<Kind> = vec![];
        let can = |k: Kind, mine: &Vec<Kind>, count: &HashMap<Kind, usize>| w.allows(k) && wt(k) > 0.0 && ct.offers(k) && count.get(&k).copied().unwrap_or(0) < limit(k) && !mine.contains(&k);
        let into_drop = next.map(|nx| role_of(&nx.tag) == Role::Hot && role != Role::Hot && (s.chopped || nx.chopped)).unwrap_or(false);
        // ── the run-up to a drop: a riser over the build, else a filter sweep and a reverse swell; every other drop an air pocket ──
        if into_drop {
            if s.build {
                if !s.riser && can(Riser, &mine, &count) && rng.chance(wt(Riser)) {
                    mine.push(Riser);
                }
            } else {
                if s.chopped && s.beats >= 8.0 && can(Sweep, &mine, &count) && rng.chance(wt(Sweep)) {
                    mine.push(Sweep);
                }
                if !s.swell && can(Swell, &mine, &count) {
                    mine.push(Swell);
                }
                // the first drop lands straight, the next one takes a breath first (a build has its own)
                if drops % 2 == 1 && can(Pocket, &mine, &count) && rng.chance(0.5 + 0.5 * wt(Pocket)) {
                    mine.push(Pocket);
                }
            }
            drops += 1;
        }
        // ── a section the energy falls out of, or a change of song: a tape stop or an echo throw on its way out ──
        let falls = next.map(|nx| nx.energy < s.energy - 0.2 || (role == Role::Hot && role_of(&nx.tag) == Role::Break)).unwrap_or(false);
        let change = next.map(|nx| nx.song != s.song).unwrap_or(false);
        if (s.chopped || s.sings) && role != Role::Build && !into_drop && (falls || change) {
            let mut opts: Vec<(Kind, f64)> = vec![];
            if s.chopped && last_end != Some(TapeStop) && can(TapeStop, &mine, &count) {
                opts.push((TapeStop, wt(TapeStop)));
            }
            if can(Throw, &mine, &count) {
                opts.push((Throw, wt(Throw)));
            }
            if let Some(k) = draw(&mut rng, &opts) {
                mine.push(k);
                last_end = Some(k);
            }
        }
        // ── the end of the song: the last chops dive (or throw) ──
        if Some(i) == last_chopped && !mine.contains(&TapeStop) && !mine.contains(&Throw) && !into_drop {
            if can(Dive, &mine, &count) && rng.chance(0.4 + 0.6 * wt(Dive)) {
                mine.push(Dive);
            } else if can(Throw, &mine, &count) {
                mine.push(Throw);
            }
        }
        // ── call and response: another acapella answers the one singing, every other section it sings alone ──
        if s.sings && !s.chopped && can(Response, &mine, &count) {
            if !responded {
                mine.push(Response);
            }
            responded = !responded;
        }
        // ── the section's own budget ──
        if s.chopped {
            let ri = role_ix(role);
            let k = nth[ri];
            nth[ri] += 1;
            let (base, cands): (f64, Vec<Kind>) = match role {
                // the first chorus is the reference: one subtle edit; the ones after it vary and get bolder
                Role::Hot if k == 0 => (1.0, vec![Candy, Choke, Stutter, Octave]),
                Role::Hot => (if s.beats >= 32.0 { 3.0 } else { 2.0 }, vec![Octave, Chipmunk, Stutter, HalfTime, DoubleTime, Gate, Flip, Candy, Choke]),
                Role::Break => (2.0, vec![Pad, Gate, Glide, Throw, Candy]),
                Role::Calm => (if s.energy >= 0.3 { 1.0 } else { 0.0 }, vec![Throw, Candy, Glide]),
                Role::Build => (0.0, vec![]),
            };
            // light rounds down (the first chorus plain), more rounds up
            let budget = if w.scale < 1.0 { (base * w.scale).floor() } else { (base * w.scale).round() } as usize;
            let mut picked: Vec<Kind> = vec![];
            for _ in 0..budget {
                // never what the last section of this role had (the repeat is a variation), unless nothing else is left
                let excl = |x: Kind, picked: &Vec<Kind>| {
                    picked.contains(&x)
                        || mine.contains(&x)
                        || (matches!(x, HalfTime) && picked.iter().any(|p| matches!(p, DoubleTime | Flip)))
                        || (matches!(x, DoubleTime | Flip) && picked.contains(&HalfTime))
                        || (matches!(x, Gate) && picked.contains(&Pad))
                        || (matches!(x, Pad) && picked.contains(&Gate))
                };
                let fresh: Vec<(Kind, f64)> = cands.iter().copied().filter(|x| !excl(*x, &picked) && !before[ri].contains(x) && can(*x, &mine, &count)).map(|x| (x, wt(x))).collect();
                let pool = if fresh.is_empty() { cands.iter().copied().filter(|x| !excl(*x, &picked) && can(*x, &mine, &count)).map(|x| (x, wt(x) * 0.5)).collect() } else { fresh };
                match draw(&mut rng, &pool) {
                    Some(x) => picked.push(x),
                    None => break,
                }
            }
            before[ri] = picked.clone();
            mine.extend(picked);
        }
        for k in &mine {
            *count.entry(*k).or_insert(0) += 1;
        }
        per[i] = mine;
    }
    per
}

/// one of `opts` by weight (None when none weighs anything)
fn draw(rng: &mut Rng, opts: &[(Kind, f64)]) -> Option<Kind> {
    let sum: f64 = opts.iter().map(|o| o.1.max(0.0)).sum();
    if sum <= 0.0 {
        return None;
    }
    let mut x = rng.f() * sum;
    for (k, w) in opts {
        x -= w.max(0.0);
        if x <= 0.0 {
            return Some(*k);
        }
    }
    opts.last().map(|o| o.0)
}

/// what a section offers its edits (notes relative to the section start, like the chops)
pub struct At<'a> {
    pub beats: f64,
    pub key: (u8, bool),
    pub energy: f64,
    pub drums: &'a [Note],
    pub harmony: &'a [Note],
    pub vocal: &'a Vocal,
    pub tune: Tune,
    pub genre: &'a str,
    pub bpm: f64,
    pub seed: u64,
    /// the breath of silence the section's build ends in (beats; 0 = none)
    pub silence: f64,
    /// the chops play on Ling Slicer (loops, reverse and one-shots work)
    pub slicer: bool,
    /// a drop follows: the last bar's roll into it is left alone
    pub next_hot: bool,
    /// how many sections of this role came before (what varies between the repeats)
    pub nth: usize,
}

/// what the edits of a section made
#[derive(Clone, Debug, Default)]
pub struct Out {
    /// the edits track's notes (risers, pads, gates, chipmunk doubles, ear candy)
    pub fx: Vec<Note>,
    /// low-pass sweeps (the Slicer's cutoff), echo throws (the Delay's mix) and air pockets, section beats (a throw may ring past
    /// the end)
    pub sweeps: Vec<(f64, f64)>,
    pub throws: Vec<(f64, f64)>,
    pub pockets: Vec<(f64, f64)>,
    pub used: Vec<Kind>,
}

/// the order edits run in: the ones that rewrite a span first, the ones that add on top, the section's end last
const ORDER: [Kind; 18] = [Flip, HalfTime, DoubleTime, Stutter, Choke, Octave, Glide, Chipmunk, Gate, Pad, Candy, Riser, Swell, TapeStop, Dive, Pocket, Sweep, Throw];

/// The planned edits `kinds` on a section's chop notes `main` / `layer` (sorted by start afterwards). An edit that finds no
/// place (no free bar, no pitched syllable, too short a section) is left out and not reported.
pub fn apply(c: &At, kinds: &[Kind], main: &mut Vec<Note>, layer: &mut Vec<Note>) -> Out {
    let mut out = Out::default();
    if c.vocal.pieces.is_empty() || c.beats < 1.0 || c.bpm <= 0.0 {
        return out;
    }
    let mut ed = Ed::new(c, main, layer);
    for k in ORDER {
        if kinds.contains(&k) && ed.run(k, &mut out) {
            out.used.push(k);
        }
    }
    main.sort_by(|a, b| a.start.total_cmp(&b.start));
    layer.sort_by(|a, b| a.start.total_cmp(&b.start));
    out.fx.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}

/// "2 reverse swells, 1 tape stop, ..." in the order of [`KINDS`]; "" when none
pub fn tally(used: &[Kind]) -> String {
    KINDS
        .iter()
        .filter_map(|k| match used.iter().filter(|u| *u == k).count() {
            0 => None,
            1 => Some(format!("1 {}", k.words().0)),
            n => Some(format!("{n} {}", k.words().1)),
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// add the flags (kit.rs: r l o x) to a chop note's slice lyric "@start:end:cents[:flags]"
pub fn flag(n: &mut Note, f: &str) {
    if !n.lyric.starts_with('@') {
        return;
    }
    if n.lyric.matches(':').count() >= 3 {
        n.lyric.push_str(f);
    } else {
        n.lyric.push(':');
        n.lyric.push_str(f);
    }
}

/// a tape stop's pitch at `g` (0 .. 1 of the way): the tape slowing to a halt, the speed falling linearly (semitones)
pub fn tape_curve(g: f64) -> f32 {
    (12.0 * (1.0 - 0.94 * g.clamp(0.0, 1.0)).log2()).max(-36.0) as f32
}

/// the bend curve of note `n` when a pitch path `f` (of the fraction 0 .. 1 of the window `w0..w1`) runs through it: sampled at
/// nine points of the note
fn curve_through(n: &Note, w0: f64, w1: f64, f: &dyn Fn(f64) -> f32) -> Curve {
    (0..=8)
        .map(|k| {
            let p = k as f64 / 8.0;
            let t = n.start + n.len * p;
            (p as f32, f(((t - w0) / (w1 - w0).max(1e-9)).clamp(0.0, 1.0)))
        })
        .collect()
}

/// notes starting in `a..b` removed, notes sounding into `a` cut there
fn clear(ns: &mut Vec<Note>, a: f64, b: f64) {
    ns.retain(|n| n.start < a - 1e-6 || n.start >= b - 1e-6);
    for n in ns.iter_mut() {
        if n.start < a - 1e-6 && n.start + n.len > a {
            n.len = (a - n.start).max(0.03);
        }
    }
}

struct Ed<'a, 'b> {
    c: &'b At<'a>,
    v: &'a Vocal,
    key: Key,
    main: &'b mut Vec<Note>,
    layer: &'b mut Vec<Note>,
    bars: usize,
    rng: Rng,
    /// spans of the main track another edit has rewritten
    taken: Vec<(f64, f64)>,
    /// slice start -> piece (the recording's and the reversed copies')
    at: HashMap<u32, usize>,
}

impl<'a, 'b> Ed<'a, 'b> {
    fn new(c: &'b At<'a>, main: &'b mut Vec<Note>, layer: &'b mut Vec<Note>) -> Ed<'a, 'b> {
        let v = c.vocal;
        let mut at: HashMap<u32, usize> = v.pieces.iter().enumerate().map(|(i, p)| (p.chop.start, i)).collect();
        for (pi, rc) in &v.rev {
            at.insert(rc.start, *pi);
        }
        Ed { c, v, key: Key::new(c.key.0, c.key.1), main, layer, bars: (c.beats / 4.0).ceil() as usize, rng: Rng::new(c.seed ^ 0xED175), taken: vec![], at }
    }

    fn busy(&self, a: f64, b: f64) -> bool {
        self.taken.iter().any(|(x, y)| a < *y - 1e-6 && b > *x + 1e-6)
    }

    fn dur_s(&self, pi: usize) -> f64 {
        let c = &self.v.pieces[pi].chop;
        (c.end - c.start) as f64 / self.v.rate.max(1) as f64
    }

    fn spb(&self) -> f64 {
        60.0 / self.c.bpm
    }

    /// the piece a chop note plays (by its slice's start; a trimmed or looped slice by the piece it lies in)
    fn piece_of(&self, n: &Note) -> Option<usize> {
        let (a, _, _) = crate::dsp::sampler::parse_slice(&n.lyric)?;
        self.at.get(&a).copied().or_else(|| self.v.pieces.iter().position(|p| a >= p.chop.start && a < p.chop.end))
    }

    /// the pitch a chop note sounds at (MIDI), None for an unpitched one
    fn heard(&self, n: &Note) -> Option<f32> {
        let f = self.v.pieces.get(self.piece_of(n)?)?.chop.midi?;
        let (_, _, cents) = crate::dsp::sampler::parse_slice(&n.lyric)?;
        Some(f + n.key as f32 - 60.0 + cents / 100.0)
    }

    /// where a syllable sung at `f0` sounds at `t`: the nearest chord tone (the key's when tuning is off)
    fn pitch_at(&self, f0: f32, t: f64) -> i32 {
        self.c.tune.nearest(f0, &self.c.tune.tones(self.key, self.c.harmony, t))
    }

    fn note(&self, chop: &Chop, at: f64, len: f64, target: i32, vel: f32) -> Note {
        tuned_note(chop, at, len, target, vel, &self.c.tune, 0).0
    }

    /// A syllable held for `len` beats at `target`: the piece itself when it is long enough (or off the Slicer), else its attack
    /// (when asked for) and then the steady middle of its vowel looped (kit flag `l`). `bend` runs over the whole hold.
    #[allow(clippy::too_many_arguments)]
    fn hold(&self, pi: usize, at: f64, len: f64, target: i32, vel: f32, bend: &Curve, attack: bool) -> Vec<Note> {
        let c = self.v.pieces[pi].chop.clone();
        let rate = self.v.rate.max(1) as f64;
        let span = c.end - c.start;
        let (a, b) = (c.start + span * 3 / 10, c.end - span / 5);
        let whole = |s: &Self| {
            let mut n = s.note(&c, at, len, target, vel);
            n.bend = bend.clone();
            vec![n]
        };
        if !self.c.slicer || self.dur_s(pi) / self.spb() >= len * 0.95 || (b - a) as f64 / rate < 0.05 {
            return whole(self);
        }
        let through = |n: &Note| -> Curve {
            if bend.is_empty() {
                return vec![];
            }
            curve_through(n, at, at + len, &|g| crate::gesture::at(bend, g as f32))
        };
        let mut out = vec![];
        let mut t = at;
        if attack {
            let al = ((a - c.start) as f64 / rate / self.spb()).max(0.05);
            let mut n = self.note(&Chop { end: a, ..c.clone() }, at, al + 0.03, target, vel);
            n.bend = through(&n);
            out.push(n);
            t = at + al;
        }
        let lc = Chop { start: a, end: b, midi: c.midi, level: c.level };
        let mut n = self.note(&lc, t, (at + len - t).max(0.05), target, vel);
        flag(&mut n, "l");
        n.bend = through(&n);
        out.push(n);
        out
    }

    /// the most sustained clean sung syllable (the hook word's first): for pads, gates and risers
    fn sustain_piece(&self) -> Option<usize> {
        let hook = self.v.hook_word.map(|w| self.v.word_pieces(w)).unwrap_or(0..0);
        (0..self.v.pieces.len())
            .filter(|p| self.v.pieces[*p].chop.midi.is_some() && self.v.clean_of(*p) >= 0.55 && self.dur_s(*p) >= 0.12)
            .max_by(|a, b| {
                let s = |p: usize| self.dur_s(p).min(0.6) * self.v.clean_of(p) as f64 * if hook.contains(&p) { 1.3 } else { 1.0 };
                s(*a).total_cmp(&s(*b))
            })
    }

    /// a short clean hit for ear candy: stressed, short, clean (pitched or not)
    fn hit_piece(&self, pitched: bool) -> Option<usize> {
        (0..self.v.pieces.len())
            .filter(|p| self.v.clean_of(*p) >= 0.3 && (!pitched || self.v.pieces[*p].chop.midi.is_some()))
            .max_by(|a, b| {
                let s = |p: usize| (0.3 + self.v.pieces[p].stress as f64) * self.v.clean_of(p) as f64 * if self.dur_s(p) <= 0.25 { 1.2 } else { 0.7 };
                s(*a).total_cmp(&s(*b))
            })
    }

    /// the hook line's last word, its syllable to repeat
    fn last_piece(&self) -> Option<usize> {
        let line = self.v.lines.get(self.v.hook_line.unwrap_or(0).min(self.v.lines.len().saturating_sub(1)))?;
        let lw = self.v.pieces[line.end - 1].word;
        let lo = self.v.word_pieces(lw).start.min(line.end.saturating_sub(3)).max(line.first);
        Some(self.v.stutter_piece(lo..line.end, line.end - 1))
    }

    /// the bars (index, start beat) of the section in the order an edit tries them: `want(bar)` ones, later phrases first on
    /// a repeat
    fn bars_where(&self, want: impl Fn(usize) -> bool) -> Vec<usize> {
        let mut v: Vec<usize> = (0..self.bars).filter(|b| want(*b) && (*b as f64 + 1.0) * 4.0 <= self.c.beats + 1e-6).collect();
        if self.c.nth % 2 == 1 {
            v.reverse();
        }
        v
    }

    fn run(&mut self, k: Kind, out: &mut Out) -> bool {
        match k {
            Swell => self.swell(),
            TapeStop => self.tape_stop(),
            Dive => self.dive(),
            Riser => self.riser(out),
            Octave => self.octave(),
            Chipmunk => self.chipmunk(out),
            Stutter => self.stutter(),
            HalfTime => self.half_time(),
            DoubleTime => self.double_time(),
            Gate => self.gate(out),
            Sweep => self.sweep(out),
            Glide => self.glide(),
            Flip => self.flip(),
            Pad => self.pad(out),
            Candy => self.candy(out),
            Choke => self.choke(),
            Pocket => self.pocket(out),
            Throw => self.throw(out),
            // the medley places the answers itself (another acapella's track): [`response_at`]
            Response => false,
        }
    }

    /// a reverse swell of the hook word ending on the downbeat after the section (the Slicer plays the slice backwards; on a
    /// Sampler the bank's reversed copy)
    fn swell(&mut self) -> bool {
        let Some(w) = self.v.hook_word else { return false };
        let r = self.v.word_pieces(w);
        let (chop, flagged) = if self.c.slicer {
            match r.clone().filter(|p| self.v.clean_of(*p) >= 0.35).max_by_key(|p| self.v.pieces[*p].chop.end - self.v.pieces[*p].chop.start) {
                Some(p) => (self.v.pieces[p].chop.clone(), true),
                None => return false,
            }
        } else {
            match self.v.rev.iter().max_by_key(|(_, c)| c.end - c.start) {
                Some((_, c)) => (c.clone(), false),
                None => return false,
            }
        };
        let dur = (chop.end - chop.start) as f64 / self.v.rate.max(1) as f64 / self.spb();
        let len = dur.min(2.0).min(self.c.beats);
        if len < 0.25 {
            return false;
        }
        let at = self.c.beats - len;
        if self.main.iter().any(|n| n.start >= at - 1e-6 && n.lyric.ends_with('r')) {
            return false;
        }
        clear(self.main, at, self.c.beats);
        let pitch = chop.midi.map(|f| self.pitch_at(f, at)).unwrap_or(60);
        // a reversed slice ends on its attack: the end of the span is what swells into the downbeat
        let mut c = chop.clone();
        let skip = (((dur - len) * self.spb()) * self.v.rate as f64) as u32;
        if flagged {
            // off the syllable's start so the kit keeps the cut (a slice starting a syllable plays all of it)
            if skip > 0 {
                c.end = c.end.saturating_sub(skip).max(c.start + 2);
                c.start += 1;
            }
        } else {
            c.start = (c.start + skip).min(c.end.saturating_sub(1));
        }
        let mut n = self.note(&c, at, len, pitch, 0.85);
        if flagged {
            flag(&mut n, "r");
        }
        self.main.push(n);
        self.taken.push((at, self.c.beats));
        true
    }

    /// the tape stop out of the section: the chops of its last 1 - 2 beats slow to a halt (varispeed: the pitch and the speed fall
    /// together), nothing starts in the last 40 % of it
    fn tape_stop(&mut self) -> bool {
        let len = ((0.8 / self.spb() * 2.0).round() / 2.0).clamp(1.0, 2.0).min(self.c.beats);
        let (w0, w1) = (self.c.beats - len, self.c.beats);
        if self.busy(w0, w1) {
            return false;
        }
        let sounding = self.main.iter().any(|n| n.start <= w0 + 0.5 && n.start + n.len > w0);
        if !sounding {
            let Some(p) = self.last_piece() else { return false };
            let f = self.v.pieces[p].chop.midi;
            let n = self.note(&self.v.pieces[p].chop, w0, len, f.map(|f| self.pitch_at(f, w0)).unwrap_or(60), 0.85);
            self.main.push(n);
        }
        self.main.retain(|n| n.start < w0 + 0.6 * len - 1e-6 || n.start >= w1);
        clear(self.layer, w0, w1);
        let last = self.main.iter().filter(|n| n.start < w1).map(|n| n.start).fold(f64::MIN, f64::max);
        for n in self.main.iter_mut().filter(|n| n.start < w1 && n.start + n.len > w0) {
            if (n.start - last).abs() < 1e-9 {
                n.len = (w1 - n.start).max(n.len);
            }
            n.bend = curve_through(n, w0, w1, &tape_curve);
        }
        self.taken.push((w0, w1));
        true
    }

    /// the last syllable of the section held and dived down an octave into the end (the end of the song)
    fn dive(&mut self) -> bool {
        let len = if self.c.bpm < 100.0 { 2.0 } else { 4.0f64 }.min(self.c.beats);
        let (w0, w1) = (self.c.beats - len, self.c.beats);
        if self.busy(w0, w1) {
            return false;
        }
        let p = match self.last_piece().filter(|p| self.v.pieces[*p].chop.midi.is_some()) {
            Some(p) => p,
            None => match self.sustain_piece() {
                Some(p) => p,
                None => return false,
            },
        };
        let Some(f) = self.v.pieces[p].chop.midi else { return false };
        clear(self.main, w0, w1);
        clear(self.layer, w0, w1);
        let bend: Curve = vec![(0.0, 0.0), (0.35, 0.0), (0.7, -4.0), (1.0, -12.0)];
        let ns = self.hold(p, w0, len - 0.05, self.pitch_at(f, w0), 0.85, &bend, true);
        self.main.extend(ns);
        self.taken.push((w0, w1));
        true
    }

    /// a sung vowel held over the build's last 4 / 8 beats on the edits track, bending up an octave into the breath before the drop
    fn riser(&mut self, out: &mut Out) -> bool {
        let Some(p) = self.sustain_piece() else { return false };
        let Some(f) = self.v.pieces[p].chop.midi else { return false };
        let end = self.c.beats - self.c.silence;
        let len = if self.c.beats >= 16.0 { 8.0 } else { 4.0f64 }.min(end);
        if len < 2.0 {
            return false;
        }
        let at = end - len;
        let bend: Curve = vec![(0.0, 0.0), (0.5, 3.0), (0.85, 8.0), (1.0, 12.0)];
        out.fx.extend(self.hold(p, at, len, self.pitch_at(f, at), 0.5 + 0.15 * self.c.energy as f32, &bend, false));
        true
    }

    /// the call of the second phrase (bar 3 of it) an octave up: the hook jumps
    fn octave(&mut self) -> bool {
        let cands = self.bars_where(|b| b % 4 == 2 && (b / 4) % 2 == 1);
        let cands = if cands.is_empty() { self.bars_where(|b| b % 4 == 2) } else { cands };
        for b in cands {
            let t0 = b as f64 * 4.0;
            if self.busy(t0, t0 + 1.0) {
                continue;
            }
            let ix: Vec<usize> = (0..self.main.len()).filter(|i| self.main[*i].start >= t0 - 1e-6 && self.main[*i].start < t0 + 1.0 && self.heard(&self.main[*i]).is_some()).collect();
            if ix.is_empty() {
                continue;
            }
            for i in ix {
                let n = &mut self.main[i];
                n.key = (n.key as i32 + 12).min(127) as u8;
            }
            clear(self.layer, t0, t0 + 1.0);
            self.taken.push((t0, t0 + 1.0));
            return true;
        }
        false
    }

    /// the calls of the later phrases doubled an octave up, quiet, on the edits track (its high-pass makes them thin and bright)
    fn chipmunk(&mut self, out: &mut Out) -> bool {
        let mut bars = self.bars_where(|b| b % 4 == 0 && b >= 4);
        if bars.is_empty() {
            bars = self.bars_where(|b| b % 4 == 0);
        }
        let mut n_ = 0;
        for b in bars.into_iter().take(2) {
            let t0 = b as f64 * 4.0;
            for n in self.main.iter().filter(|n| n.start >= t0 - 1e-6 && n.start < t0 + 2.0) {
                if self.heard(n).is_none() || n.lyric.ends_with('l') {
                    continue;
                }
                let mut d = n.clone();
                d.key = (d.key as i32 + 12).min(127) as u8;
                d.vel = (d.vel * 0.55).max(0.1);
                d.pan = 0.2;
                d.bend.clear();
                out.fx.push(d);
                n_ += 1;
            }
        }
        n_ > 0
    }

    /// a phrase's last beat as a stutter that decays like an echo: the last word's syllable on the genre's grid, each repeat
    /// quieter and shorter (the drop's own roll is left alone)
    fn stutter(&mut self) -> bool {
        let last = self.bars.saturating_sub(1);
        let keep = |b: usize| !(self.c.next_hot && b == last);
        let mut bars = self.bars_where(|b| b % 4 == 3 && (b / 4) % 2 == 1 && keep(b));
        if bars.is_empty() {
            bars = self.bars_where(|b| b % 4 == 3 && keep(b));
        }
        let Some(p) = self.last_piece() else { return false };
        let (_, _, groll) = genre_feel(self.c.genre);
        let g = if groll > 0.0 { groll } else { 0.25 };
        for b in bars {
            let (a, e) = (b as f64 * 4.0 + 3.0, b as f64 * 4.0 + 4.0);
            if self.busy(a, e) || e > self.c.beats + 1e-6 {
                continue;
            }
            clear(self.main, a, e);
            clear(self.layer, a, e);
            let n = (1.0 / g).round() as usize;
            let c = self.v.pieces[p].chop.clone();
            for j in 0..n {
                let at = a + j as f64 * g;
                let len = (g * (0.9 - 0.6 * j as f64 / n as f64)).max(g * 0.3);
                let pitch = c.midi.map(|f| self.pitch_at(f, at)).unwrap_or(60);
                let vel = (0.95 * 0.78f32.powi(j as i32)).max(0.18);
                let nt = self.note(&c, at, len, pitch, vel);
                self.main.push(nt);
            }
            self.taken.push((a, e));
            return true;
        }
        false
    }

    /// a two-bar half-time phrase: the first bar of the second phrase played at half the speed over two (the chops of the second
    /// bar make way)
    fn half_time(&mut self) -> bool {
        let t0 = if self.bars >= 8 { 16.0 } else if self.bars >= 4 && self.c.nth > 0 { 8.0 } else { return false };
        if t0 + 8.0 > self.c.beats + 1e-6 || self.busy(t0, t0 + 8.0) {
            return false;
        }
        let src: Vec<Note> = self.main.iter().filter(|n| n.start >= t0 - 1e-6 && n.start < t0 + 4.0).cloned().collect();
        if src.len() < 2 {
            return false;
        }
        clear(self.main, t0, t0 + 8.0);
        clear(self.layer, t0, t0 + 8.0);
        for mut n in src {
            n.start = t0 + 2.0 * (n.start - t0);
            n.len = (n.len * 2.0).min(2.0).min(t0 + 8.0 - n.start);
            n.bend.clear();
            self.main.push(n);
        }
        self.taken.push((t0, t0 + 8.0));
        true
    }

    /// the section's next-to-last bar in double time: its chops twice as fast, twice
    fn double_time(&mut self) -> bool {
        if self.bars < 3 {
            return false;
        }
        let t0 = (self.bars - 2) as f64 * 4.0;
        if t0 + 4.0 > self.c.beats + 1e-6 || self.busy(t0, t0 + 4.0) {
            return false;
        }
        let src: Vec<Note> = self.main.iter().filter(|n| n.start >= t0 - 1e-6 && n.start < t0 + 4.0).cloned().collect();
        if src.len() < 2 {
            return false;
        }
        clear(self.main, t0, t0 + 4.0);
        clear(self.layer, t0, t0 + 4.0);
        for half in 0..2 {
            for n in &src {
                let mut m = n.clone();
                m.start = t0 + half as f64 * 2.0 + (n.start - t0) * 0.5;
                m.len = (n.len * 0.5).max(0.06);
                m.bend.clear();
                self.main.push(m);
            }
        }
        self.taken.push((t0, t0 + 4.0));
        true
    }

    /// two bars of trance gate on the edits track: the held vowel retriggered on 16ths in a gate pattern, on the chord tones
    fn gate(&mut self, out: &mut Out) -> bool {
        const PATTERNS: [[u8; 16]; 3] = [[1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 1, 1, 0, 1, 0], [1, 1, 0, 1, 1, 0, 1, 0, 1, 1, 0, 1, 1, 0, 1, 1], [1, 0, 1, 0, 1, 1, 1, 0, 1, 0, 1, 0, 1, 1, 0, 1]];
        let Some(p) = self.sustain_piece() else { return false };
        let Some(f) = self.v.pieces[p].chop.midi else { return false };
        if self.bars < 2 || !self.c.slicer {
            return false;
        }
        // the second half of the section (two bars, on a phrase line)
        let b0 = ((self.bars / 2) / 2 * 2).min(self.bars - 2);
        let t0 = b0 as f64 * 4.0;
        if t0 + 8.0 > self.c.beats + 1e-6 {
            return false;
        }
        let pat = PATTERNS[(self.c.seed as usize + self.c.nth) % PATTERNS.len()];
        let c = self.v.pieces[p].chop.clone();
        let span = c.end - c.start;
        let lc = Chop { start: c.start + span * 3 / 10, end: c.end - span / 5, ..c };
        let e = self.c.energy as f32;
        for s in 0..32usize {
            if pat[s % 16] == 0 {
                continue;
            }
            let at = t0 + s as f64 * 0.25;
            let vel = if s % 4 == 0 { 0.72 } else { 0.58 } * (0.8 + 0.25 * e);
            let mut n = self.note(&lc, at, 0.18, self.pitch_at(f, at), vel.min(1.0));
            flag(&mut n, "l");
            out.fx.push(n);
        }
        true
    }

    /// a low-pass across the run-up to the drop: closed on the downbeat 16 bars out (or the section's start), open by the drop
    fn sweep(&mut self, out: &mut Out) -> bool {
        let a = ((self.c.beats - 16.0).max(0.0) / 4.0).floor() * 4.0;
        if self.c.beats - a < 4.0 || !self.main.iter().any(|n| n.start >= a - 1e-6) {
            return false;
        }
        out.sweeps.push((a, self.c.beats));
        true
    }

    /// glides between the long chops of the section: every other pair of long pitched notes a few semitones apart slides from the
    /// first into the second (in every other bar, at most four)
    fn glide(&mut self) -> bool {
        let mut ix: Vec<usize> = (0..self.main.len()).collect();
        ix.sort_by(|a, b| self.main[*a].start.total_cmp(&self.main[*b].start));
        let mut done = 0;
        let mut skip = false;
        for w in ix.windows(2) {
            let (p, q) = (&self.main[w[0]], &self.main[w[1]]);
            // every other bar at most, four in all
            if done >= 4 || self.busy(q.start, q.start + q.len) || ((q.start / 4.0).floor() as i64) % 2 == 1 {
                continue;
            }
            let (Some(hp), Some(hq)) = (self.heard(p), self.heard(q)) else { continue };
            let gap = q.start - (p.start + p.len);
            let d = hp - hq;
            if q.len < 0.5 || p.len < 0.25 || gap > 0.75 || d.abs() < 0.9 || d.abs() > 7.1 || !q.bend.is_empty() || q.lyric.ends_with('l') {
                continue;
            }
            skip = !skip;
            if !skip {
                continue;
            }
            let slide = (0.25 / q.len).clamp(0.08, 0.3) as f32;
            self.main[w[1]].bend = vec![(0.0, d), (slide, 0.0)];
            done += 1;
        }
        done > 0
    }

    /// a sample flip: the first bar of the second phrase rewritten as a new hook - syllables of other words, re-sequenced on a
    /// rhythm of the genre, walking the chord tones
    fn flip(&mut self) -> bool {
        let mut bars = self.bars_where(|b| b % 4 == 1 && (b / 4) % 2 == 1);
        if bars.is_empty() {
            bars = self.bars_where(|b| b % 4 == 1);
        }
        let Some(b) = bars.into_iter().find(|b| !self.busy(*b as f64 * 4.0, *b as f64 * 4.0 + 4.0)) else { return false };
        let t0 = b as f64 * 4.0;
        // one syllable of each clean sung word (the hook's last), in the order they were sung
        let mut pool: Vec<usize> = vec![];
        for w in 0..self.v.words.len() {
            if Some(w) == self.v.hook_word {
                continue;
            }
            let best = self.v.word_pieces(w).filter(|p| self.v.pieces[*p].chop.midi.is_some() && self.v.clean_of(*p) >= 0.5 && self.v.pieces[*p].stress >= 0.45).max_by(|a, b| self.v.pieces[*a].stress.total_cmp(&self.v.pieces[*b].stress));
            if let Some(p) = best {
                pool.push(p);
            }
        }
        if pool.len() < 3 {
            return false;
        }
        if pool.len() > 6 {
            let k = pool.len();
            pool = (0..6).map(|i| pool[i * k / 6]).collect();
        }
        let steps: &[u8] = match family(self.c.genre) {
            Fam::Urban => &[0, 3, 6, 10, 12],
            Fam::Bass => &[0, 2, 3, 6, 8, 10, 11, 14],
            Fam::Club => &[2, 3, 6, 7, 10, 11, 14],
            _ => &[0, 3, 6, 8, 11, 14],
        };
        let contour: [i32; 8] = [0, 1, 2, 1, 0, -1, 0, 2];
        let first = self.v.pieces[pool[0]].chop.midi.unwrap_or(64.0);
        clear(self.main, t0, t0 + 4.0);
        clear(self.layer, t0, t0 + 4.0);
        for (k, s) in steps.iter().enumerate() {
            let at = t0 + *s as f64 * 0.25;
            let next = steps.get(k + 1).map(|x| t0 + *x as f64 * 0.25).unwrap_or(t0 + 4.0);
            let pi = pool[(k + self.c.nth) % pool.len()];
            let c = self.v.pieces[pi].chop.clone();
            let f = c.midi.unwrap_or(first);
            // the chord tones around where the flip starts, stepped along the contour
            let tones = self.c.tune.tones(self.key, self.c.harmony, at);
            let base = self.c.tune.nearest(first, &tones);
            let mut ladder: Vec<i32> = (base - 12..=base + 12).filter(|p| tones.contains(&(p.rem_euclid(12) as u8))).collect();
            if ladder.is_empty() {
                ladder.push(base);
            }
            let bi = ladder.iter().position(|p| *p == base).unwrap_or(0) as i32;
            let want = ladder[(bi + contour[k % 8]).clamp(0, ladder.len() as i32 - 1) as usize];
            let target = self.c.tune.fit(f, want, &tones).0;
            let vel = if k == 0 { 0.9 } else if *s % 4 == 0 { 0.8 } else { 0.68 };
            let len = ((next - at) * 0.85).min(0.75).max(0.12);
            let n = self.note(&c, at, len, target, vel);
            self.main.push(n);
        }
        self.taken.push((t0, t0 + 4.0));
        true
    }

    /// a held vowel under the section on the edits track: one note per two bars on the chord tone nearest the sung pitch, soft
    fn pad(&mut self, out: &mut Out) -> bool {
        let Some(p) = self.sustain_piece() else { return false };
        let Some(f) = self.v.pieces[p].chop.midi else { return false };
        let mut t = 0.0;
        let mut n = 0;
        while t < self.c.beats - 2.0 && n < 4 {
            let len = (self.c.beats - t).min(8.0) - 0.25;
            out.fx.extend(self.hold(p, t, len, self.pitch_at(f, t + 0.01), 0.42, &vec![], false));
            t += 8.0;
            n += 1;
        }
        n > 0
    }

    /// one-shot ear candy at the ends of phrases (two at most, on the "and" of 4 where the chops leave room): an octave-up blip,
    /// a reversed blip, an unpitched hit - panned left and right in turn
    fn candy(&mut self, out: &mut Out) -> bool {
        let bars = self.bars_where(|b| b % 4 == 3);
        let mut made = 0usize;
        for b in bars {
            if made >= 2 {
                break;
            }
            let (a, e) = (b as f64 * 4.0 + 3.25, b as f64 * 4.0 + 4.0);
            if e > self.c.beats + 1e-6 || self.main.iter().any(|n| n.start >= a - 1e-6 && n.start < e) || out.fx.iter().any(|n| n.start >= a - 1e-6 && n.start < e) {
                continue;
            }
            let variant = (made + self.c.nth + (self.rng.next() % 2) as usize) % 3;
            let (pi, up, rev) = match variant {
                0 => (self.hit_piece(true), 12, false),
                1 if self.c.slicer => (self.hit_piece(true), 0, true),
                _ => (self.hit_piece(false), 0, false),
            };
            let Some(pi) = pi else { continue };
            let mut c = self.v.pieces[pi].chop.clone();
            let at = if rev { e - 0.5 } else { a + 0.25 };
            let len = if rev { 0.5 } else { 0.25 };
            // a reversed blip: the syllable's first half beat backwards, so it ends on its attack
            let frames = (len * self.spb() * self.v.rate as f64) as u32;
            if rev && c.end - c.start > frames + 2 {
                c.end = c.start + frames;
                c.start += 1;
            }
            let pitch = c.midi.map(|f| self.pitch_at(f, at) + up).unwrap_or(60);
            let mut n = self.note(&c, at, len, pitch, 0.55);
            flag(&mut n, if rev { "ro" } else { "o" });
            n.pan = if made % 2 == 0 { -0.45 } else { 0.45 };
            out.fx.push(n);
            made += 1;
        }
        made > 0
    }

    /// a choke-group cut: the gated bar of a phrase played so each hit rings until the next one cuts it (choke group 3)
    fn choke(&mut self) -> bool {
        for b in self.bars_where(|b| b % 4 == 1) {
            let (t0, t1) = (b as f64 * 4.0, b as f64 * 4.0 + 4.0);
            if self.busy(t0, t1) {
                continue;
            }
            let mut ix: Vec<usize> = (0..self.main.len()).filter(|i| self.main[*i].start >= t0 - 1e-6 && self.main[*i].start < t1).collect();
            if ix.len() < 3 {
                continue;
            }
            ix.sort_by(|a, b| self.main[*a].start.total_cmp(&self.main[*b].start));
            for k in 0..ix.len() {
                let next = ix.get(k + 1).map(|j| self.main[*j].start).unwrap_or(t1);
                let n = &mut self.main[ix[k]];
                n.len = (next - n.start + 0.12).min(1.0);
                flag(n, "x");
            }
            self.taken.push((t0, t1));
            return true;
        }
        false
    }

    /// an air pocket: the chops stop half a beat (a beat at a fast tempo) before the drop
    fn pocket(&mut self, out: &mut Out) -> bool {
        let p = ((0.3 / self.spb() * 2.0).round() / 2.0).clamp(0.5, 1.0);
        if self.c.beats < 4.0 || self.c.silence > 0.0 {
            return false;
        }
        let a = self.c.beats - p;
        clear(self.main, a, self.c.beats);
        clear(self.layer, a, self.c.beats);
        out.pockets.push((a, self.c.beats));
        true
    }

    /// An echo throw on the last word before the section ends (or, inside the section, before its longest rest): the delay opens on
    /// the word and closes once the echoes have rung (three beats, or where the chops come back).
    fn throw(&mut self, out: &mut Out) -> bool {
        let mut ns: Vec<&Note> = self.main.iter().collect();
        ns.sort_by(|a, b| a.start.total_cmp(&b.start));
        if ns.is_empty() {
            return false;
        }
        // the last note of the section's end, else the note before the longest rest
        let tail = ns.iter().rev().find(|n| n.start >= self.c.beats - 4.0 && n.start < self.c.beats - 0.25);
        let (n, until) = match tail {
            Some(n) => (*n, n.start + n.len + 3.0),
            None => {
                let mut best: Option<(f64, usize)> = None;
                for k in 0..ns.len() {
                    let next = ns.get(k + 1).map(|x| x.start).unwrap_or(self.c.beats);
                    let rest = next - (ns[k].start + ns[k].len);
                    if rest >= 1.5 && best.map(|b| rest > b.0).unwrap_or(true) {
                        best = Some((rest, k));
                    }
                }
                let Some((_, k)) = best else { return false };
                let next = ns.get(k + 1).map(|x| x.start).unwrap_or(self.c.beats);
                (ns[k], (ns[k].start + ns[k].len + 3.0).min(next - 0.25))
            }
        };
        if until <= n.start + 0.5 {
            return false;
        }
        out.throws.push((n.start, until));
        true
    }
}

/// Where another acapella answers the singer: a beat after each sung line ends, when the rest before the next line (or the
/// section's end) is long enough for the answer (`need` beats); at most `max`. `sung` are the lines (start, end), section beats.
pub fn response_at(sung: &[(f64, f64)], beats: f64, need: f64, max: usize) -> Vec<f64> {
    let mut lines: Vec<(f64, f64)> = sung.to_vec();
    lines.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut out = vec![];
    for (k, (_, e)) in lines.iter().enumerate() {
        if out.len() >= max {
            break;
        }
        let next = lines.get(k + 1).map(|l| l.0).unwrap_or(beats);
        // on the next half beat after the line, a little air first
        let at = ((e + 0.25) * 2.0).ceil() / 2.0;
        if at >= 0.0 && next - at >= need && at + need <= beats + 1e-6 {
            out.push(at);
        }
    }
    out
}

/// The answer of a call and response: the hook word of `v` at `at` (section beats), its syllables as sung, each on the chord tone
/// nearest its pitch; quieter than the singer.
pub fn answer_notes(v: &Vocal, at: f64, key: (u8, bool), harmony: &[Note], tune: &Tune, vel: f32) -> Vec<Note> {
    let Some(w) = v.hook_word else { return vec![] };
    let r = v.word_pieces(w);
    if r.is_empty() {
        return vec![];
    }
    let k = Key::new(key.0, key.1);
    let t0 = v.pieces[r.start].start;
    let mut out = vec![];
    for pi in r.clone() {
        let p = &v.pieces[pi];
        let off = p.start - t0;
        if off > 2.0 {
            break;
        }
        let len = v.dur_beats(pi).max(0.1).min(1.5);
        let pitch = p.chop.midi.map(|f| tune.nearest(f, &tune.tones(k, harmony, at + off))).unwrap_or(60);
        out.push(tuned_note(&p.chop, at + off, len, pitch, vel, tune, 0).0);
    }
    out
}

// ── the lanes and the tracks ──

/// the Slicer's cutoff lane for low-pass sweeps (absolute beats): open (1) everywhere else, closed to about 650 Hz where a sweep
/// starts, open again by its end
pub fn cutoff_lane(sweeps: &[(f64, f64)]) -> Vec<(f64, f32)> {
    let mut ws = sweeps.to_vec();
    ws.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut pts: Vec<(f64, f32)> = vec![(0.0, 1.0)];
    for (a, b) in ws {
        if b <= a + 0.5 {
            continue;
        }
        pts.extend([(a - 0.01, 1.0), (a, 0.45), (a + (b - a) * 0.6, 0.68), (b - 0.02, 1.0)]);
    }
    pts.sort_by(|x, y| x.0.total_cmp(&y.0));
    pts
}

/// a Delay's mix lane for echo throws (absolute beats): dry, then `level` from one echo after the word starts (so the echo is
/// the word's, not what came before it) to half a beat before the window ends, dry again at its end; overlapping throws merge
pub fn throw_lane(throws: &[(f64, f64)], echo: f64, level: f32) -> Vec<(f64, f32)> {
    let mut ws = throws.to_vec();
    ws.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut merged: Vec<(f64, f64)> = vec![];
    for (a, b) in ws {
        match merged.last_mut() {
            Some(l) if a <= l.1 => l.1 = l.1.max(b),
            _ => merged.push((a, b)),
        }
    }
    let mut pts: Vec<(f64, f32)> = vec![(0.0, 0.0)];
    for (a, b) in merged {
        let open = a + echo;
        if b <= open + 0.25 {
            continue;
        }
        pts.extend([(open - 0.01, 0.0), (open, level), ((b - 0.5).max(open + 0.01), level), (b, 0.0)]);
    }
    pts
}

/// the echo of a throw in beats: a dotted eighth in the club, bass and pop genres, a quarter elsewhere
pub fn echo_beats(genre: &str) -> f64 {
    match family(genre) {
        Fam::Club | Fam::Bass | Fam::Pop => 0.75,
        _ => 1.0,
    }
}

/// Echo throws on a track: a Delay at the end of its chain (synced to the song, feedback 0.45), dry but where `throws` (absolute
/// beats) open its mix. false when there is none to add.
pub fn add_throws(tr: &mut Track, throws: &[(f64, f64)], bpm: f64, genre: &str, sr: f32) -> bool {
    if throws.is_empty() || bpm <= 0.0 {
        return false;
    }
    let Some(mut fx) = EffectSlot::new("Delay", sr) else { return false };
    let mut beats = echo_beats(genre);
    // the Delay holds 1.5 s at most
    while beats * 60.0 / bpm > 1.5 {
        beats *= 0.5;
    }
    let secs = beats * 60.0 / bpm;
    let lane = throw_lane(throws, beats, 0.4);
    if lane.len() < 2 {
        return false;
    }
    for (k, v) in [("time", ((secs - 0.02) / 1.5).clamp(0.0, 1.0) as f32), ("feedback", 0.47), ("mix", 0.0)] {
        if let Some(pi) = fx.params.iter().position(|p| p.0 == k) {
            fx.set(pi, v);
        }
    }
    let idx = tr.effects.len();
    let Some(mi) = fx.params.iter().position(|p| p.0 == "mix") else { return false };
    tr.effects.push(fx);
    tr.auto.push(AutoLane { param: format!("fx{idx}.{mi}"), points: lane });
    true
}

/// low-pass sweeps on a Slicer track: its cutoff lane (absolute beats); false when there is none
pub fn add_sweeps(tr: &mut Track, sweeps: &[(f64, f64)]) -> bool {
    let lane = cutoff_lane(sweeps);
    if lane.len() < 2 {
        return false;
    }
    tr.auto.retain(|l| l.param != "cutoff");
    tr.auto.push(AutoLane { param: "cutoff".into(), points: lane });
    true
}

/// The edits track's sound: a high-pass (about 380 Hz: the doubles and the candy thin and bright, the pads out of the vocal's
/// way) and a little reverb; on a Slicer a softer attack (15 ms) for the pads
pub fn dress_fx_track(tr: &mut Track, sr: f32) {
    if let Some(mut fx) = EffectSlot::new("Vox Filter", sr) {
        for (k, v) in [("hp", 0.8f32), ("lp", 0.92), ("res", 0.2), ("dyn", 0.0), ("mix", 1.0)] {
            if let Some(pi) = fx.params.iter().position(|p| p.0 == k) {
                fx.set(pi, v);
            }
        }
        tr.effects.push(fx);
    }
    if let Some(mut fx) = EffectSlot::new("Vox Reverb", sr) {
        if let Some(pi) = fx.params.iter().position(|p| p.0 == "mix") {
            fx.set(pi, 0.3);
        }
        tr.effects.push(fx);
    }
    if tr.inst_kind == "slicer" {
        if let Some(i) = tr.inst.as_mut() {
            i.set_param("attack", (0.015f32 / 0.5).sqrt());
        }
    }
}

/// notes of `ns` (clip-relative, the clip at `clip0`) starting in an air pocket cut out, notes ringing into one cut short
/// (absolute pocket beats); how many went
pub fn cut_pockets(ns: &mut Vec<Note>, clip0: f64, pockets: &[(f64, f64)]) -> usize {
    let before = ns.len();
    ns.retain(|n| !pockets.iter().any(|(a, b)| clip0 + n.start >= a - 1e-6 && clip0 + n.start < b - 1e-6));
    for n in ns.iter_mut() {
        for (a, _) in pockets {
            let s = clip0 + n.start;
            if s < a - 1e-6 && s + n.len > *a {
                n.len = (a - s).max(0.02);
            }
        }
    }
    before - ns.len()
}

#[cfg(test)]
#[path = "edits_tests.rs"]
mod tests;
