//! Articulations for automation: what a parameter *does* (its role), the common patterns producers draw on it (wobble, pump, riser,
//! per-note plucks ...), and how to lay one out so it lands on the beat and on the notes.
//!
//! The model (or, without one, the local chooser) only picks `pattern / rate / lo / hi` for each parameter from the vocabulary here.
//! The curve itself is always rendered by `render`, so a small model can never write a broken lane: every value is finite, 0..1 and
//! sorted, and everything is aligned to bars.
use crate::compose::Rng;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Role {
    Filter,
    Reso,
    Drive,
    Pitch,
    Mix,
    Space,
    ModDepth,
    ModRate,
    Timbre,
    Pan,
    Env,
    Other,
}

impl Role {
    /// what the parameter is, in words a model understands
    pub fn word(self) -> &'static str {
        match self {
            Role::Filter => "filter / brightness",
            Role::Reso => "resonance",
            Role::Drive => "drive / distortion",
            Role::Pitch => "pitch / tuning",
            Role::Mix => "effect mix / amount",
            Role::Space => "space / reverb / delay",
            Role::ModDepth => "modulation depth",
            Role::ModRate => "modulation speed",
            Role::Timbre => "tone colour / waveform / vowel",
            Role::Pan => "stereo position",
            Role::Env => "envelope time",
            Role::Other => "other",
        }
    }
    /// how far the parameter may be pushed (1 = freely, small = a little goes a long way)
    pub fn scale(self) -> f32 {
        match self {
            Role::Filter | Role::Timbre | Role::ModDepth | Role::Mix => 1.0,
            Role::Reso | Role::Drive => 0.8,
            Role::Space | Role::Other | Role::ModRate | Role::Pan => 0.6,
            Role::Pitch | Role::Env => 0.3,
        }
    }
    /// ordering when a cap forces a choice: the parameters that shape a sound most audibly first
    fn priority(self) -> u8 {
        match self {
            Role::Filter => 0,
            Role::Reso => 1,
            Role::Drive => 2,
            Role::Timbre => 3,
            Role::Mix => 4,
            Role::ModDepth => 5,
            Role::Space => 6,
            Role::Pan => 7,
            Role::Pitch => 8,
            Role::ModRate => 9,
            Role::Env => 10,
            Role::Other => 11,
        }
    }
}

pub fn priority(r: Role) -> u8 {
    r.priority()
}

fn has(n: &str, words: &[&str]) -> bool {
    words.iter().any(|w| n.contains(w))
}

/// Guess a parameter's job from its name (instrument and effect parameters in this DAW are named plainly: "cut", "squash", "rv mix").
pub fn role_of(name: &str) -> Role {
    let n = name.to_lowercase();
    // a route into pitch first: "x velpit" would otherwise read as a filter ("lp" in "velpit")
    if is_pitch_route(&n) {
        return Role::Pitch;
    }
    if n == "pan" || has(&n, &["width", "stereo", "spread"]) {
        Role::Pan
    } else if has(&n, &["reso", "emph", " q", "peak"]) || n == "q" || n == "res" || n.ends_with(" res") {
        Role::Reso
    } else if has(&n, &["cut", "freq", "bright", "tone", "tilt", "lp", "hp", "filter", "flt", "bright", "bandw", "lowpass", "high", "low", "mid"]) {
        Role::Filter
    } else if has(&n, &["drive", "drv", "fold", "dist", "squash", "warm", "sat", "crush", "grit", "clip", "shaper", "gate", "bite", "asym"]) {
        Role::Drive
    } else if has(&n, &["pitch", "tune", "det", "coarse", "fine", "semi", "bend", "shift", "oct"]) || is_pitch_route(&n) {
        Role::Pitch
    } else if has(&n, &["mix", "wet", "dry", "amount", "blend", "send"]) {
        Role::Mix
    } else if has(&n, &["size", "room", "feedback", "fdbk", "delay", "time", "decay", "damp", "pre", "tail", "rv", "dl", "hall"]) && !has(&n, &["vib", "amp", "attack"]) {
        Role::Space
    } else if has(&n, &["depth", "dpth", "fmod", "intensity", "mod", "amt"]) {
        Role::ModDepth
    } else if has(&n, &["rate", "speed", "lfo", "tempo", "vib"]) {
        Role::ModRate
    } else if has(&n, &["attack", "release", "sustain", "amp", "env", "glide", "hold"]) {
        Role::Env
    } else if has(&n, &["vowel", "vow", "nois", "comb", "gender", "shape", "wave", "morph", "table", "formant", "pos", "scan", "talk", "sample", "xsyn", "grain", "noise", "sub", "uni", "wid", "harm", "ring", "voice", "choir", "open", "fry", "nasal", "breath", "whisp", "tilt", "spec", "cons"]) {
        Role::Timbre
    } else {
        Role::Other
    }
}

/// A modulation route into pitch: Prism's matrix cells "x <source><pit|det|bpt>" (velocity / wheel / an envelope -> pitch,
/// detune, bend), and any other name that ends in a pitch target the same way ("vel pit", "whl pit").
fn is_pitch_route(n: &str) -> bool {
    let t = n.trim();
    ["pit", "det", "bpt"].iter().any(|d| t.ends_with(d) && (t.starts_with("x ") || t.contains(' ')))
}

/// Parameters that are switches, levels the mix depends on, steps of a sequence, or anything that moves the pitch: never
/// automated by the generators (a pitch, a detune or a route into pitch drawn as a wobble makes the whole track seasick).
pub fn eligible(name: &str) -> bool {
    let n = name.to_lowercase();
    // pitch, detune and every matrix route into them (Growl's "m velp", "m l1p" ... as well as Prism's): never articulated
    if role_of(&n) == Role::Pitch || crate::redesign::pitch_altering(&n) {
        return false;
    }
    if n == "vol" || n == "gain" || n == "level" || n == "master" || n == "voices" || n == "tempo" || n == "bpm" || n == "mute" || n == "uni" || n == "penv" || n == "pdec" {
        return false;
    }
    if n.starts_with("arp") || has(&n, &["gain", "volume", "mute", "bypass", "hold", "snd", "preset", "mode", "part", "sel", "t len", "output", "out "]) {
        return false;
    }
    // a step of the Talk sequencer ("t s0".."t s15"), a matrix cell ("m3 4")
    if n.starts_with("t s") && n[3..].chars().all(|c| c.is_ascii_digit()) {
        return false;
    }
    true
}

// ── patterns ──────────────────────────────────────────────────────────────────

/// (name, what it does) as the model is told
pub const PATTERNS: [(&str, &str); 20] = [
    ("wobble", "smooth LFO up and down every cycle"),
    ("pump", "ducks on every beat then recovers, sidechain style"),
    ("stutter", "hard on/off gating"),
    ("sweep", "slow rise and fall over a few bars"),
    ("riser", "builds up over each phrase then resets"),
    ("build", "one long build across the whole section"),
    ("fade", "one long fall across the whole section"),
    ("swell", "each note swells up and back"),
    ("pluck", "each note starts high and decays"),
    ("open", "each note starts closed and opens"),
    ("gate", "high while a note plays, low between notes"),
    ("steps", "a staircase climbing every cycle"),
    ("random", "sample-and-hold jumps"),
    ("breathe", "very slow gentle movement"),
    ("drift", "slow random walk"),
    ("groove", "gates in the rhythm of the music itself (the notes' own groove), fills into each new phrase"),
    ("arc", "each phrase rises to its climax and falls back, like a singer's line"),
    ("answer", "call and response: the first half of a phrase moves, the second answers it upside down"),
    ("accel", "nested: an LFO that speeds up and climbs through the phrase into the next section, the intro riser"),
    ("hold", "leave it alone"),
];

pub fn known(p: &str) -> Option<&'static str> {
    PATTERNS.iter().find(|(n, _)| *n == p).map(|(n, _)| *n)
}

/// words a model might use for a pattern
pub fn pattern_from_word(w: &str) -> Option<&'static str> {
    let w = w.trim().to_lowercase();
    if let Some(p) = known(&w) {
        return Some(p);
    }
    Some(match w.as_str() {
        "lfo" | "sine" | "wub" | "wobbles" | "vibrato" | "tremolo" | "oscillate" => "wobble",
        "duck" | "sidechain" | "ducking" | "pumping" => "pump",
        "gated" | "chop" | "stutters" | "trance gate" => "stutter",
        "sweeps" | "triangle" | "tri" | "filter sweep" => "sweep",
        "rise" | "ramp" | "ramp up" | "rising" | "risers" | "up" | "crescendo" => "riser",
        "builds" | "long build" | "buildup" => "build",
        "fade out" | "fall" | "down" | "decrescendo" | "decay" | "fadeout" => "fade",
        "swells" | "swell per note" => "swell",
        "plucks" | "stab" | "staccato" | "decays" => "pluck",
        "opens" | "wah" | "per note open" => "open",
        "note gate" | "gates" => "gate",
        "step" | "staircase" | "stairs" | "arp" => "steps",
        "s&h" | "sample and hold" | "randomize" | "noise" => "random",
        "breath" | "breathing" | "slow" | "subtle" => "breathe",
        "wander" | "walk" | "random walk" | "evolve" | "evolving" => "drift",
        "none" | "static" | "off" | "keep" => "hold",
        "rhythm" | "rhythmic gate" | "euclid" | "euclidean" | "tresillo" | "syncopated" | "grooves" => "groove",
        "phrase" | "contour" | "arch" | "line" | "arcs" => "arc",
        "call" | "response" | "call and response" | "answers" | "question" => "answer",
        "accelerate" | "accelerating" | "speed up" | "rush" | "nested" | "intro riser" | "accelerando" => "accel",
        _ => return None,
    })
}

pub fn note_aware(p: &str) -> bool {
    matches!(p, "swell" | "pluck" | "open" | "gate")
}

fn rhythmic(p: &str) -> bool {
    matches!(p, "wobble" | "pump" | "stutter" | "steps" | "random" | "groove")
}

/// the patterns that suit each role, most natural first
pub fn role_patterns(r: Role) -> &'static [&'static str] {
    match r {
        Role::Filter => &["arc", "wobble", "groove", "sweep", "riser", "pluck", "open", "accel", "pump", "steps", "breathe", "gate"],
        Role::Reso => &["sweep", "arc", "breathe", "wobble", "riser", "drift"],
        Role::Drive => &["swell", "groove", "pump", "arc", "riser", "accel", "breathe", "build", "drift"],
        Role::Pitch => &["drift", "breathe", "wobble"],
        Role::Mix => &["swell", "arc", "breathe", "riser", "answer", "pump", "build", "drift"],
        Role::Space => &["breathe", "arc", "riser", "drift", "sweep", "build"],
        Role::ModDepth => &["sweep", "accel", "riser", "arc", "wobble", "steps", "swell", "build"],
        Role::ModRate => &["sweep", "accel", "steps", "drift", "riser"],
        Role::Timbre => &["sweep", "answer", "arc", "drift", "steps", "random", "breathe", "riser", "swell", "open"],
        Role::Pan => &["wobble", "answer", "drift", "sweep", "breathe"],
        Role::Env => &["drift", "breathe"],
        Role::Other => &["drift", "breathe", "sweep", "random", "swell"],
    }
}

// ── what the user's words ask for ──────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub struct Intent {
    /// overrides the amount slider when the words say subtle / wild (0 = no opinion)
    pub amount: f32,
    /// cycle length multiplier: below 1 faster, above 1 slower
    pub speed: f64,
    /// patterns the words suggest
    pub bias: Vec<&'static str>,
    /// more variation between phrases
    pub fills: bool,
}

pub fn read_prompt(prompt: &str) -> Intent {
    let p = prompt.to_lowercase();
    let any = |ws: &[&str]| ws.iter().any(|w| p.contains(w));
    let mut it = Intent { amount: 0.0, speed: 1.0, bias: vec![], fills: true };
    if any(&["subtle", "gentle", "soft", "light", "tiny", "slight", "calm"]) {
        it.amount = 0.3;
    }
    if any(&["wild", "extreme", "crazy", "insane", "intense", "heavy", "aggressive", "brutal", "massive", "huge"]) {
        it.amount = 1.0;
    }
    if any(&["slow", "lazy", "ambient", "dreamy", "floaty", "evolving", "long"]) {
        it.speed = 2.0;
    }
    if any(&["fast", "frantic", "rapid", "busy", "hyper", "quick", "short"]) {
        it.speed = 0.5;
    }
    let mut add = |ws: &[&str], pats: &[&'static str]| {
        if ws.iter().any(|w| p.contains(w)) {
            for q in pats {
                if !it.bias.contains(q) {
                    it.bias.push(q);
                }
            }
        }
    };
    add(&["build", "riser", "tension", "climb", "rise", "anticipation", "lift"], &["riser", "build"]);
    add(&["drop", "dubstep", "bass music", "wub", "wobble", "growl"], &["wobble", "pump", "stutter"]);
    add(&["pump", "sidechain", "edm", "house", "club"], &["pump"]);
    add(&["glitch", "stutter", "chop", "idm", "broken"], &["stutter", "steps", "random"]);
    add(&["breath", "organic", "evolv", "ambient", "pad", "cinematic", "drone", "dreamy"], &["breathe", "drift", "sweep", "swell"]);
    add(&["pluck", "staccato", "rhythmic", "bouncy", "tight", "percussive", "stab"], &["pluck", "gate", "open"]);
    add(&["swell", "epic", "cinematic", "dramatic", "emotional"], &["swell", "build"]);
    add(&["fade", "outro", "end", "down"], &["fade"]);
    add(&["sweep", "filter", "trance"], &["sweep", "steps"]);
    add(&["random", "chaos", "chaotic", "unpredictable"], &["random", "drift", "steps"]);
    add(&["groove", "rhythm", "syncop", "funk", "euclid", "tresillo", "dubstep", "percussive"], &["groove"]);
    add(&["call", "answer", "response", "conversation", "dialog"], &["answer"]);
    add(&["accel", "speed up", "rush", "intro", "nested", "tension"], &["accel"]);
    add(&["phrase", "melodic", "expressive", "musical", "singing", "line", "intelligent"], &["arc"]);
    if any(&["steady", "even", "consistent", "no fill", "no fills"]) {
        it.fills = false;
    }
    it
}

// ── choosing (the part a model normally does) ─────────────────────────────────

#[derive(Clone, Debug)]
pub struct Spec {
    pub pattern: &'static str,
    /// beats per cycle
    pub rate: f64,
    pub lo: f32,
    pub hi: f32,
}

const RATES: [f64; 8] = [0.25, 0.5, 1.0, 2.0, 4.0, 8.0, 16.0, 32.0];

pub fn snap_rate(r: f64) -> f64 {
    let r = if r.is_finite() { r.clamp(0.25, 32.0) } else { 1.0 };
    *RATES.iter().min_by(|a, b| (r / **a).ln().abs().total_cmp(&(r / **b).ln().abs())).unwrap_or(&1.0)
}

fn default_rate(p: &str, rng: &mut Rng, speed: f64) -> f64 {
    let pick = |rng: &mut Rng, v: &[f64]| v[(rng.next() % v.len() as u64) as usize];
    let r = match p {
        "wobble" => pick(rng, &[0.5, 1.0, 1.0, 2.0]),
        "pump" => 1.0,
        "stutter" => pick(rng, &[0.25, 0.5]),
        "sweep" => pick(rng, &[8.0, 16.0]),
        "steps" => pick(rng, &[4.0, 8.0]),
        "random" => pick(rng, &[1.0, 2.0, 4.0]),
        "breathe" => pick(rng, &[8.0, 16.0]),
        "drift" => 4.0,
        "answer" => pick(rng, &[1.0, 2.0]),
        "accel" => 4.0,
        "arc" | "groove" => 16.0,
        _ => 8.0,
    };
    // pump stays on the beat whatever the words say
    if p == "pump" {
        r
    } else {
        snap_rate(r * speed)
    }
}

/// Pick a pattern for a parameter of this role without a model: from what the words suggested when it suits the role, else from the role's own list.
pub fn choose_local(role: Role, intent: &Intent, rng: &mut Rng, has_notes: bool) -> &'static str {
    let list = role_patterns(role);
    let ok = |p: &&str| has_notes || !note_aware(p);
    let from_words: Vec<&'static str> = intent.bias.iter().copied().filter(|p| list.contains(p) || matches!(*p, "riser" | "build" | "fade" | "arc" | "accel") && role != Role::Env && role != Role::Pitch).filter(ok).collect();
    if !from_words.is_empty() && rng.chance(0.7) {
        return from_words[(rng.next() % from_words.len() as u64) as usize];
    }
    let cands: Vec<&'static str> = list.iter().copied().filter(ok).collect();
    // earlier entries are likelier
    let k = (rng.f() * rng.f() * cands.len() as f64) as usize;
    cands[k.min(cands.len() - 1)]
}

/// The value window for a pattern around the parameter's current value `cur`: `amount` 0..1 says how far to go.
pub fn window(pattern: &str, cur: f32, amount: f32, role: Role) -> (f32, f32) {
    let cur = if cur.is_finite() { cur.clamp(0.0, 1.0) } else { 0.5 };
    let half = (0.08 + 0.4 * amount.clamp(0.0, 1.0)) * role.scale();
    let (lo, hi) = match pattern {
        "riser" | "build" | "accel" => (cur - half * 0.3, cur + half * 2.0),
        "arc" => (cur - half * 0.8, cur + half * 1.4),
        "groove" => (cur - half * 1.6, cur + half * 0.4),
        "fade" => (cur - half * 2.0, cur + half * 0.2),
        "pluck" => (cur - half * 0.6, cur + half * 1.5),
        "open" | "swell" => (cur - half * 1.0, cur + half * 1.6),
        "gate" | "stutter" | "pump" => (cur - half * 1.6, cur + half * 0.4),
        _ => (cur - half, cur + half),
    };
    fit(lo, hi)
}

/// shift a window into 0..1 without squeezing it, so a parameter near an edge still moves
fn fit(mut lo: f32, mut hi: f32) -> (f32, f32) {
    let w = (hi - lo).clamp(0.04, 1.0);
    if lo < 0.0 {
        lo = 0.0;
        hi = w;
    }
    if hi > 1.0 {
        hi = 1.0;
        lo = 1.0 - w;
    }
    (lo.clamp(0.0, 1.0), hi.clamp(0.0, 1.0))
}

/// A full spec for one parameter: the model's pick where it gave one and it made sense, local choices for the rest.
pub fn spec_for(role: Role, cur: f32, amount: f32, intent: &Intent, pick: Option<&Pick>, rng: &mut Rng, has_notes: bool) -> Spec {
    let amount = if intent.amount > 0.0 { intent.amount } else { amount };
    // a pick that makes no sense for the parameter's job (a wobble on a waveform, a per-note pattern without notes) is not trusted at all
    let sane = pick.and_then(|p| pattern_from_word(&p.pattern)).filter(|p| (has_notes || !note_aware(p)) && (*p == "hold" || role_patterns(role).contains(p) || intent.bias.contains(p)));
    let pick = if sane.is_some() { pick } else { None };
    let pat = sane.unwrap_or_else(|| choose_local(role, intent, rng, has_notes));
    let rate = match pick.and_then(|p| p.rate) {
        Some(r) if r.is_finite() && r > 0.0 && pat != "pump" => snap_rate(r * intent.speed),
        _ => default_rate(pat, rng, intent.speed),
    };
    let (wlo, whi) = window(pat, cur, amount, role);
    // the model's own range is kept when it is a real range, but never beyond what the role can take
    let (lo, hi) = match pick.map(|p| (p.lo, p.hi)) {
        // (a range that does not even include where the parameter sits is a guess: the local window is used instead)
        Some((Some(lo), Some(hi))) if lo.is_finite() && hi.is_finite() && (hi - lo).abs() >= 0.05 && lo.min(hi) <= cur + 0.15 && lo.max(hi) >= cur - 0.15 => {
            let (lo, hi) = (lo.clamp(0.0, 1.0).min(hi.clamp(0.0, 1.0)), lo.clamp(0.0, 1.0).max(hi.clamp(0.0, 1.0)));
            let reach = (0.15 + 0.85 * role.scale() * (0.4 + 0.6 * amount.clamp(0.0, 1.0))).min(1.0);
            let mid = (lo + hi) * 0.5;
            let half = ((hi - lo) * 0.5).min(reach * 0.5 + 0.1);
            fit(mid - half, mid + half)
        }
        _ => (wlo, whi),
    };
    Spec { pattern: pat, rate, lo, hi }
}

// ── what the model says ──────────────────────────────────────────────────────

#[derive(Clone, Debug, Default)]
pub struct Pick {
    pub p: usize,
    pub pattern: String,
    pub rate: Option<f64>,
    pub lo: Option<f32>,
    pub hi: Option<f32>,
}

fn num(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => {
            let s = s.trim();
            if let Some((a, b)) = s.split_once('/') {
                let (a, b) = (a.trim().parse::<f64>().ok()?, b.trim().parse::<f64>().ok()?);
                return if b != 0.0 { Some(a / b) } else { None };
            }
            s.parse::<f64>().ok()
        }
        _ => None,
    }
}

fn pick_of(o: &serde_json::Map<String, serde_json::Value>) -> Option<Pick> {
    let get = |names: &[&str]| names.iter().find_map(|n| o.get(*n));
    let p = get(&["p", "param", "index", "i", "id"]).and_then(num)?;
    if !(p >= 0.0 && p < 1000.0) {
        return None;
    }
    let pattern = get(&["pattern", "pat", "shape", "articulation", "type"]).and_then(|v| v.as_str()).unwrap_or("").to_string();
    Some(Pick { p: p as usize, pattern, rate: get(&["rate", "beats", "period", "every"]).and_then(num), lo: get(&["lo", "low", "min", "from"]).and_then(num).map(|x| x as f32), hi: get(&["hi", "high", "max", "to"]).and_then(num).map(|x| x as f32) })
}

/// Read the model's reply: a JSON array of objects (or several objects, or an object holding an array), tolerant of text around it.
pub fn parse_reply(reply: &str) -> Vec<Pick> {
    let mut out: Vec<Pick> = vec![];
    let mut take = |v: &serde_json::Value, out: &mut Vec<Pick>| {
        if let serde_json::Value::Object(o) = v {
            if let Some(p) = pick_of(o) {
                if !out.iter().any(|q| q.p == p.p) {
                    out.push(p);
                }
            }
        }
    };
    // the whole thing, or the span from the first bracket to the last
    let spans: Vec<&str> = {
        let mut s = vec![reply];
        if let (Some(a), Some(b)) = (reply.find('['), reply.rfind(']')) {
            if a < b {
                s.push(&reply[a..=b]);
            }
        }
        s
    };
    for text in spans {
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(text.trim()) {
            match &v {
                serde_json::Value::Array(a) => a.iter().for_each(|x| take(x, &mut out)),
                serde_json::Value::Object(o) => {
                    if let Some(serde_json::Value::Array(a)) = o.values().find(|x| x.is_array()) {
                        a.iter().for_each(|x| take(x, &mut out));
                    } else {
                        take(&v, &mut out);
                    }
                }
                _ => {}
            }
            if !out.is_empty() {
                return out;
            }
        }
    }
    // a reply cut short by the token limit, or with prose around it: every complete {...} on its own
    let b: Vec<char> = reply.chars().collect();
    let (mut depth, mut start) = (0i32, 0usize);
    for (i, c) in b.iter().enumerate() {
        match c {
            '{' => {
                if depth == 0 {
                    start = i;
                }
                depth += 1;
            }
            '}' if depth > 0 => {
                depth -= 1;
                if depth == 0 {
                    let s: String = b[start..=i].iter().collect();
                    if let Ok(v) = serde_json::from_str::<serde_json::Value>(&s) {
                        take(&v, &mut out);
                    }
                }
            }
            _ => {}
        }
    }
    out
}

// ── the prompt ───────────────────────────────────────────────────────────────

pub fn system() -> String {
    "You are a sound designer who writes parameter automation for a DAW. You are given one track, its tempo, the length of the section, a request and numbered parameters with what each one does. For every parameter pick an articulation pattern, how many beats one cycle lasts, and the low and high value (0 to 1) it moves between. Match the pattern to the job of the parameter: filters wobble, sweep or pluck, drive pumps or swells, mix and space breathe or rise, pitch only drifts a little. Do not give every parameter the same pattern. Answer with one JSON array and nothing else.".to_string()
}

pub fn pattern_list() -> String {
    PATTERNS.iter().map(|(n, d)| format!("{n} ({d})")).collect::<Vec<_>>().join(", ")
}

pub fn example() -> String {
    format!(
        "<|user|>\nTrack: Bass (Ling Growl), 140 BPM, 16 bars. Request: tense build into a drop\nPatterns: {}\nCycle lengths in beats: 0.25 0.5 1 2 4 8 16\nParameters:\n0 cut (filter / brightness) now 0.40\n1 res (resonance) now 0.25\n2 drive (drive / distortion) now 0.30\n3 rv mix (effect mix / amount) now 0.10\n4 a det (pitch / tuning) now 0.50\n<|assistant|>\n[{{\"p\":0,\"pattern\":\"riser\",\"rate\":16,\"lo\":0.2,\"hi\":0.95}},{{\"p\":1,\"pattern\":\"sweep\",\"rate\":8,\"lo\":0.25,\"hi\":0.7}},{{\"p\":2,\"pattern\":\"swell\",\"rate\":4,\"lo\":0.3,\"hi\":0.8}},{{\"p\":3,\"pattern\":\"breathe\",\"rate\":8,\"lo\":0.1,\"hi\":0.4}},{{\"p\":4,\"pattern\":\"drift\",\"rate\":4,\"lo\":0.45,\"hi\":0.55}}]\n",
        pattern_list()
    )
}

pub fn user_prompt(track: &str, bpm: f64, bars: f64, request: &str, params: &[(String, Role, f32)]) -> String {
    let mut s = format!("Track: {track}, {} BPM, {} bars. Request: {}\nPatterns: {}\nCycle lengths in beats: 0.25 0.5 1 2 4 8 16\nParameters:\n", bpm.round() as i64, bars.round() as i64, if request.trim().is_empty() { "something musical and varied" } else { request.trim() }, pattern_list());
    for (i, (n, r, v)) in params.iter().enumerate() {
        s.push_str(&format!("{i} {n} ({}) now {v:.2}\n", r.word()));
    }
    s
}

// ── the curves ──────────────────────────────────────────────────────────────

pub struct Ctx<'a> {
    pub b0: f64,
    pub b1: f64,
    /// bars in a phrase (4 .. 64), 0 = found from the music (see [`phrase_map`])
    pub phrase: f64,
    /// (onset, length) of every note, sorted by onset, in beats
    pub notes: &'a [(f64, f64)],
    /// the onsets of the rest of the song (the drums first): the groove a lane locks to when its own track has no notes
    pub groove: &'a [f64],
    pub seed: u32,
    pub fills: bool,
}

fn finish(mut pts: Vec<(f64, f32)>) -> Vec<(f64, f32)> {
    pts.retain(|p| p.0.is_finite() && p.1.is_finite());
    pts.sort_by(|a, b| a.0.total_cmp(&b.0));
    // two points at the same time make a step: keep the later, nudged apart so the lane stays a function of time
    let mut out: Vec<(f64, f32)> = Vec::with_capacity(pts.len());
    for p in pts {
        let v = p.1.clamp(0.0, 1.0);
        match out.last() {
            Some(l) if p.0 - l.0 < 0.004 => {
                let t = l.0 + 0.004;
                out.push((t, v));
            }
            _ => out.push((p.0, v)),
        }
    }
    out
}

// ── form: the phrases of the music, which ones repeat, where it changes ──────────

/// phrase lengths the form is built from, in bars
pub const PHRASE_BARS: [f64; 5] = [4.0, 8.0, 16.0, 32.0, 64.0];

/// one stretch of music the automation treats as one gesture
#[derive(Clone, Debug, PartialEq)]
pub struct Phrase {
    pub b0: f64,
    pub b1: f64,
    /// the material: phrases with the same letter get the same gesture (A A B A ...)
    pub letter: u8,
    /// how many times the letter was heard before (0 = its first statement)
    pub repeat: u8,
    /// 0..1: the arc over the whole range (quiet start, peak about two thirds in) mixed with how busy the notes are
    pub energy: f32,
    /// the next phrase is new material, or this is the last one: fills and turnarounds go at its end
    pub before_change: bool,
}

/// the 16th-note onset slots of the notes in b0..b1, relative to b0: a phrase's rhythmic fingerprint
fn print(notes: &[(f64, f64)], b0: f64, b1: f64) -> Vec<u32> {
    let mut v: Vec<u32> = notes.iter().filter(|n| n.0 >= b0 - 1e-6 && n.0 < b1 - 1e-6).map(|n| ((n.0 - b0) * 4.0).round().max(0.0) as u32).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// how alike two fingerprints are (Jaccard, 1 = the same rhythm)
fn similar(a: &[u32], b: &[u32]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    let (mut i, mut j, mut both) = (0, 0, 0usize);
    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            both += 1;
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }
    both as f64 / (a.len() + b.len() - both) as f64
}

/// how good a place `t` is for a phrase to start: nothing held across it, a rest before it, a note on it
fn boundary(notes: &[(f64, f64)], t: f64) -> f64 {
    if notes.is_empty() {
        return 0.5;
    }
    let (mut held, mut onset, mut last_end) = (false, false, f64::MIN);
    for &(on, len) in notes {
        if on > t + 0.3 {
            break;
        }
        if on < t - 0.05 {
            last_end = last_end.max(on + len);
            if on + len > t + 0.05 {
                held = true;
            }
        }
        if (on - t).abs() <= 0.25 {
            onset = true;
        }
    }
    let on = if onset { 0.2 } else { 0.0 };
    if held {
        return 0.1 + on;
    }
    let rest = if last_end == f64::MIN { 1.0 } else { ((t - last_end) / 2.0).clamp(0.0, 1.0) };
    (0.4 + 0.4 * rest + on).min(1.0)
}

/// the phrase length (beats) the music suggests: boundaries that fall in rests, neighbouring phrases that repeat, about 8 bars
fn auto_len(notes: &[(f64, f64)], b0: f64, b1: f64) -> f64 {
    let len = b1 - b0;
    let mut best = (f64::MIN, 16.0f64.min(len));
    for bars in PHRASE_BARS {
        let p = bars * 4.0;
        if p > len + 1e-9 && bars > 4.0 {
            continue;
        }
        let n = (len / p).floor().max(1.0);
        let (mut q, mut c) = (0.0, 0usize);
        let mut t = b0 + p;
        while t < b1 - 1e-9 {
            q += boundary(notes, t);
            c += 1;
            t += p;
        }
        let q = if c == 0 { 0.5 } else { q / c as f64 };
        // repetition: how much of the music is an earlier phrase again (A A B A at its own length explains half of itself)
        let mut prints: Vec<Vec<u32>> = vec![];
        let mut t = b0;
        while t + p <= b1 + 1e-9 && prints.len() < 32 {
            prints.push(print(notes, t, t + p));
            t += p;
        }
        let rep = if prints.len() < 2 {
            0.0
        } else {
            (1..prints.len()).filter(|&k| (0..k).any(|j| similar(&prints[k], &prints[j]) >= 0.8)).count() as f64 / (prints.len() - 1) as f64
        };
        let score = q + 0.4 * rep - 0.08 * (bars / 8.0).log2().abs() - if n > 16.0 { 0.3 } else { 0.0 };
        if score > best.0 + 1e-9 {
            best = (score, p.min(len));
        }
    }
    best.1
}

/// The form of b0..b1: phrases (`bars` long, or the length the notes suggest when 0), lettered by what repeats - a phrase whose
/// rhythm matches an earlier one is the same material. Without notes the phrases follow the common A A B A shape.
pub fn phrase_map(b0: f64, b1: f64, notes: &[(f64, f64)], bars: f64) -> Vec<Phrase> {
    let len = b1 - b0;
    if !(len > 0.5) {
        return vec![];
    }
    let notes: Vec<(f64, f64)> = notes.iter().copied().filter(|n| n.0 >= b0 - 1e-6 && n.0 < b1).collect();
    let plen = if bars >= 1.0 { (bars.round() * 4.0).clamp(4.0, 256.0) } else { auto_len(&notes, b0, b1) }.min(len.max(1.0));
    let mut cuts = vec![b0];
    let mut t = b0 + plen;
    // a tail shorter than a bar joins the last phrase
    while t < b1 - 3.99 && cuts.len() < 512 {
        cuts.push(t);
        t += plen;
    }
    cuts.push(b1);
    let n = cuts.len() - 1;
    let prints: Vec<Vec<u32>> = (0..n).map(|k| print(&notes, cuts[k], cuts[k + 1])).collect();
    let mut letters = vec![0u8; n];
    if notes.is_empty() {
        for (k, l) in letters.iter_mut().enumerate() {
            *l = ((k / 4) % 2 * 2 + [0, 0, 1, 0][k % 4]) as u8;
        }
    } else {
        // each letter's first statement: a later phrase that is at least 60 % the same rhythm is that material again
        let mut firsts: Vec<usize> = vec![];
        for k in 0..n {
            let best = firsts.iter().enumerate().map(|(l, &f)| (l, similar(&prints[k], &prints[f]))).max_by(|a, b| a.1.total_cmp(&b.1));
            match best {
                Some((l, s)) if s >= 0.6 || firsts.len() >= 26 => letters[k] = l as u8,
                _ => {
                    letters[k] = firsts.len() as u8;
                    firsts.push(k);
                }
            }
        }
    }
    let dens: Vec<f64> = (0..n).map(|k| prints[k].len() as f64 / (cuts[k + 1] - cuts[k]).max(1.0)).collect();
    let dmax = dens.iter().copied().fold(0.0, f64::max);
    let mut seen = [0u8; 32];
    (0..n)
        .map(|k| {
            let f = ((cuts[k] + cuts[k + 1]) * 0.5 - b0) / len;
            let arc = 0.3 + 0.7 * (std::f64::consts::PI * f.powf(1.9)).sin();
            let e = if dmax > 0.0 { 0.6 * arc + 0.4 * dens[k] / dmax } else { arc };
            let li = letters[k] as usize % 32;
            let repeat = seen[li];
            seen[li] = seen[li].saturating_add(1);
            Phrase { b0: cuts[k], b1: cuts[k + 1], letter: letters[k], repeat, energy: e.clamp(0.0, 1.0) as f32, before_change: k + 1 == n || letters[k + 1] != letters[k] }
        })
        .collect()
}

/// "8-bar phrases: A A B A" - the form as the status line shows it
pub fn form_text(ph: &[Phrase]) -> String {
    let Some(f) = ph.first() else { return String::new() };
    let bars = ((f.b1 - f.b0) / 4.0).round();
    let mut s: String = ph.iter().take(16).map(|p| ((b'A' + p.letter.min(25)) as char).to_string()).collect::<Vec<_>>().join(" ");
    if ph.len() > 16 {
        s.push_str(" ...");
    }
    format!("{bars}-bar phrases: {s}")
}

// ── rhythm: one-bar cells on the 16th grid ──────────────────────────────────────

/// the vocabulary rhythmic lanes are built from (x = open), sparse to busy
pub const CELLS: [(&str, &str); 11] = [
    ("halftime", "x.......x......."),
    ("four", "x...x...x...x..."),
    ("offbeat", "..x...x...x...x."),
    ("tresillo", "x..x..x.x..x..x."),
    ("son", "x..x..x...x.x..."),
    ("euclid5", "x..x..x..x..x..."),
    ("dotted", "x..x..x..x..x..x"),
    ("syncopated", "x..x...x..x.x..."),
    ("eighths", "x.x.x.x.x.x.x.x."),
    ("gallop", "x.xxx.xxx.xxx.xx"),
    ("sixteenths", "xxxxxxxxxxxxxxxx"),
];

fn cell(name: &str) -> Vec<u32> {
    CELLS.iter().find(|c| c.0 == name).map(|c| c.1.bytes().enumerate().filter(|(_, b)| *b == b'x').map(|(i, _)| i as u32).collect()).unwrap_or_else(|| vec![0, 4, 8, 12])
}

/// The groove of some onsets: the 16th slots of the bar that at least half the busy bars play. None if it says nothing (fewer
/// than two slots, or every slot).
pub fn groove_cell(onsets: &[f64], b0: f64, b1: f64) -> Option<Vec<u32>> {
    let mut hist = [0usize; 16];
    let mut bars = std::collections::BTreeSet::new();
    for &on in onsets.iter().filter(|o| **o >= b0 - 1e-6 && **o < b1) {
        let rel = on - b0;
        let slot = ((rel.rem_euclid(4.0)) * 4.0).round() as usize % 16;
        let bar = (rel / 4.0).floor() as i64;
        if bars.insert((bar, slot)) {
            hist[slot] += 1;
        }
    }
    let busy = bars.iter().map(|b| b.0).collect::<std::collections::BTreeSet<_>>().len();
    if busy < 2 {
        return None;
    }
    let v: Vec<u32> = (0..16).filter(|s| hist[*s as usize] * 2 >= busy).collect();
    if v.len() < 2 || v.len() >= 16 {
        None
    } else {
        Some(v)
    }
}

/// a small stable hash for per-letter choices: the same letter always picks the same thing
fn mix(seed: u32, letter: u8, salt: u32) -> u32 {
    let mut h = seed.wrapping_mul(0x9E37_79B1) ^ (letter as u32).wrapping_mul(0x85EB_CA6B) ^ salt.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 15;
    h = h.wrapping_mul(0x2C1B_3C6D);
    h ^ (h >> 12)
}

/// the cell a phrase's material gets: the music's own groove for the first material, then cells by energy (busier when louder)
fn cell_for(ph: &Phrase, groove: &Option<Vec<u32>>, seed: u32) -> Vec<u32> {
    if ph.letter == 0 {
        if let Some(g) = groove {
            return g.clone();
        }
    }
    // the pool slides from the sparse cells to the busy ones with the energy
    let lo = ((ph.energy * 6.0) as usize).min(6);
    let pool = &CELLS[lo..(lo + 5).min(CELLS.len())];
    cell(pool[mix(seed, ph.letter, 3) as usize % pool.len()].0)
}

/// a gate opening on every slot of `slots` across one bar starting at `t`: open for most of the gap to the next slot (at most a beat)
fn gate_bar(t: f64, slots: &[u32], top: f32, lo: f32, end: f64, out: &mut Vec<(f64, f32)>) {
    for (i, &s) in slots.iter().enumerate() {
        let on = t + s as f64 * 0.25;
        if on >= end - 1e-6 {
            break;
        }
        let next = slots.get(i + 1).map(|n| t + *n as f64 * 0.25).unwrap_or(t + 4.0 + slots[0] as f64 * 0.25);
        let open = ((next - on) * 0.7).clamp(0.08, 1.0);
        let off = (on + open).min(end - 0.01);
        out.push((on - 0.006, lo));
        out.push((on, top));
        out.push((off, top));
        out.push((off + 0.01, lo));
    }
}

/// Lay a pattern out over `ctx.b0..ctx.b1`, phrase by phrase: the same material gets the same gesture, new material a
/// contrasting one, the depth follows the energy of the form, fills land before each change and risers lead into it. Empty for "hold".
pub fn render(spec: &Spec, ctx: &Ctx) -> Vec<(f64, f32)> {
    let (b0, b1) = (ctx.b0, ctx.b1.min(ctx.b0 + 2048.0));
    if !(b1 > b0 + 0.5) || spec.pattern == "hold" {
        return vec![];
    }
    let ph = phrase_map(b0, b1, ctx.notes, ctx.phrase);
    // the structure is drawn while the lane stays light (a few points per beat); a very long range at a fast rate is laid out flat
    // gates draw every bar: a very long range is laid out flat
    let gated = matches!(spec.pattern, "stutter" | "groove" | "pump");
    if ph.is_empty() || (gated && (b1 - b0) * 10.0 > 3000.0) {
        return render_flat(spec, ctx);
    }
    let (lo, hi) = (spec.lo.clamp(0.0, 1.0), spec.hi.clamp(0.0, 1.0));
    // a range written upside down (lo above hi) is the same range
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let span = hi - lo;
    let mut rate = spec.rate.clamp(0.125, 64.0);
    // a fast cycle over a long range would be thousands of points: slow it down until the lane stays light
    let per_cycle = match spec.pattern {
        "wobble" | "breathe" | "answer" => 9.0,
        "random" => 8.0,
        "accel" => 14.0,
        _ => 0.0,
    };
    while per_cycle > 0.0 && (b1 - b0) / rate * per_cycle > 600.0 && rate < 64.0 {
        rate *= 2.0;
    }
    let mut out: Vec<(f64, f32)> = vec![];
    let mut p = spec.pattern;
    let notes: Vec<(f64, f64)> = ctx.notes.iter().copied().filter(|n| n.0 >= b0 - 1e-6 && n.0 < b1).collect();
    if note_aware(p) && (notes.len() > 1800 || notes.is_empty()) {
        p = "wobble";
    }
    // the depth of a phrase: quiet phrases move less (at least 45 % of the window), loud ones the whole way
    let depth = |e: f32| 0.45 + 0.55 * e;
    let shape = |name: &str, s: f64, e: f64, period: f64, l: f32, h: f32, seed: u32, out: &mut Vec<(f64, f32)>| {
        out.extend(crate::api::auto_shape_points(name, s, e, period, l, h, seed));
    };
    // the onsets the lane can lock to: the track's own notes, else the song's
    let own: Vec<f64> = notes.iter().map(|n| n.0).collect();
    let groove = groove_cell(&own, b0, b1).or_else(|| groove_cell(ctx.groove, b0, b1));
    let n = ph.len();
    match p {
        "wobble" | "breathe" | "random" | "drift" => {
            for (k, f) in ph.iter().enumerate() {
                let d = depth(f.energy);
                let (l, h) = (lo + span * (1.0 - d) * 0.5, hi - span * (1.0 - d) * 0.5);
                // new material gets its own speed: half or double, chosen once per letter
                let r = if f.letter == 0 || p == "breathe" { rate } else { snap_rate(rate * [1.0, 2.0, 0.5, 1.0][mix(ctx.seed, f.letter, 5) as usize % 4]) };
                // the same letter replays the same values: the seed is the letter's, not the phrase's
                let sd = mix(ctx.seed, f.letter, 11);
                let fill = ctx.fills && f.before_change && k + 1 < n && rhythmic(p) && f.b1 - f.b0 >= 16.0;
                let cut = if fill { f.b1 - 4.0 } else { f.b1 };
                match p {
                    "drift" => {
                        let mut rng = Rng::new(sd as u64 + 17);
                        let mut v = (l + h) * 0.5;
                        let step = (r * 0.5).clamp(1.0, 8.0);
                        let mut t = f.b0;
                        while t < f.b1 - 1e-6 && out.len() < 3000 {
                            out.push((t, v));
                            v = (v + ((rng.f() as f32) - 0.5) * (h - l) * 0.9).clamp(l, h);
                            t += step;
                        }
                    }
                    _ => shape(if p == "random" { "random" } else { "sine" }, f.b0, cut, r, l, h, sd, &mut out),
                }
                if fill {
                    // the turnaround: twice as fast, full depth, into the change
                    shape(if p == "random" { "random" } else { "sine" }, cut, f.b1, (r * 0.5).max(0.125), lo, hi, sd ^ 0x55, &mut out);
                }
            }
        }
        "stutter" | "groove" => {
            for (k, f) in ph.iter().enumerate() {
                let top = lo + span * depth(f.energy);
                let slots = if p == "groove" { cell_for(f, &groove, ctx.seed) } else { cell(["sixteenths", "eighths", "gallop", "syncopated"][mix(ctx.seed, f.letter, 7) as usize % 4]) };
                let mut t = f.b0;
                while t < f.b1 - 1e-6 && out.len() < 5000 {
                    let last_bar = t + 4.0 >= f.b1 - 1e-6;
                    let fill = ctx.fills && last_bar && f.before_change && k + 1 < n;
                    let s = if fill { cell(if p == "groove" { "gallop" } else { "sixteenths" }) } else { slots.clone() };
                    gate_bar(t, &s, if fill { hi } else { top }, lo, f.b1, &mut out);
                    t += 4.0;
                }
            }
        }
        "pump" => {
            // ducks on the kick when the song has one (the groove), else on every beat; the last bar before a change pumps in eighths
            let kick = groove_cell(ctx.groove, b0, b1).filter(|g| g.len() <= 8).unwrap_or_else(|| vec![0, 4, 8, 12]);
            for (k, f) in ph.iter().enumerate() {
                let floor = hi - span * depth(f.energy);
                let mut t = f.b0;
                while t < f.b1 - 1e-6 && out.len() < 6000 {
                    let fill = ctx.fills && t + 4.0 >= f.b1 - 1e-6 && f.before_change && k + 1 < n;
                    let slots = if fill { cell("eighths") } else { kick.clone() };
                    for (i, &sl) in slots.iter().enumerate() {
                        let on = t + sl as f64 * 0.25;
                        let next = slots.get(i + 1).map(|x| t + *x as f64 * 0.25).unwrap_or(t + 4.0).min(f.b1);
                        if on >= f.b1 - 1e-6 {
                            break;
                        }
                        out.push((on, floor));
                        out.push((on + (next - on) * 0.18, floor + (hi - floor) * 0.35));
                        out.push((next - 0.004, hi));
                    }
                    t += 4.0;
                }
            }
        }
        "sweep" | "arc" => {
            for f in &ph {
                let d = depth(f.energy);
                let top = lo + span * d;
                let len = f.b1 - f.b0;
                // the climax: where the phrase's notes are busiest, else two thirds in; a sweep turns at the middle
                let peak = if p == "sweep" {
                    0.5
                } else {
                    let pn: Vec<f64> = notes.iter().filter(|n| n.0 >= f.b0 && n.0 < f.b1).map(|n| (n.0 - f.b0) / len).collect();
                    if pn.len() >= 4 {
                        let mut best = (0usize, 0.62f64);
                        for c in 1..8 {
                            let x = c as f64 / 8.0;
                            let m = pn.iter().filter(|q| (**q - x).abs() < 0.125).count();
                            if m > best.0 || (m == best.0 && (x - 0.62).abs() < (best.1 - 0.62).abs()) {
                                best = (m, x);
                            }
                        }
                        best.1.clamp(0.3, 0.8)
                    } else {
                        0.62
                    }
                };
                // a sweep over a long phrase cycles every 8 bars; odd letters sweep the other way
                let cyc = if p == "sweep" && len > 32.0 { 32.0 } else { len };
                let inv = p == "sweep" && f.letter % 2 == 1;
                let mut s = f.b0;
                while s < f.b1 - 1e-6 {
                    let e = (s + cyc).min(f.b1);
                    for kk in 0..=12 {
                        let x = kk as f64 / 12.0;
                        let y = if x <= peak { (x / peak).powf(1.4) } else { 1.0 - ((x - peak) / (1.0 - peak)).powf(0.8) };
                        let y = (0.5 - 0.5 * (std::f64::consts::PI * y).cos()) as f32;
                        let v = lo + (top - lo) * y;
                        out.push((s + (e - s) * x * 0.995, if inv { lo + top - v } else { v }));
                    }
                    s = e;
                }
            }
        }
        "steps" => {
            for f in &ph {
                let top = lo + span * depth(f.energy);
                let bars = (((f.b1 - f.b0) / 4.0).round() as usize).clamp(1, 64);
                let stepn = bars.clamp(2, 8);
                let per = (f.b1 - f.b0) / stepn as f64;
                let down = f.letter % 2 == 1;
                for i in 0..stepn {
                    let fr = i as f32 / (stepn - 1) as f32;
                    let v = lo + (top - lo) * if down { 1.0 - fr } else { fr };
                    out.push((f.b0 + per * i as f64, v));
                    out.push((f.b0 + per * (i + 1) as f64 - 0.01, v));
                }
            }
        }
        "riser" | "accel" => {
            // only into a change: the last half of a phrase that leads into new material (at most 8 bars), resting low otherwise
            let lead: Vec<usize> = (0..n).filter(|k| ph[*k].before_change && *k + 1 < n).collect();
            let lead = if lead.is_empty() { (0..n).collect::<Vec<_>>() } else { lead };
            for (k, f) in ph.iter().enumerate() {
                let rest = lo + span * 0.12 * f.energy;
                if !lead.contains(&k) {
                    out.push((f.b0, rest));
                    out.push((f.b1 - 0.02, rest));
                    continue;
                }
                let len = (f.b1 - f.b0).min(32.0).max(((f.b1 - f.b0) * 0.5).min(32.0));
                let s = f.b1 - len;
                if s > f.b0 + 1e-6 {
                    out.push((f.b0, rest));
                    out.push((s, rest));
                }
                if p == "riser" {
                    for kk in 0..=10 {
                        let x = kk as f64 / 10.0;
                        out.push((s + len * x * 0.985, rest + (hi - rest) * (x * x) as f32));
                    }
                } else {
                    // nested envelopes: the centre climbs, the swing widens, and the LFO doubles its speed every quarter
                    let mut per = rate.max(1.0);
                    for q in 0..4 {
                        let qs = s + len * q as f64 / 4.0;
                        let qe = s + len * (q + 1) as f64 / 4.0;
                        let c0 = q as f32 / 4.0;
                        let mut t = qs;
                        while t < qe - 1e-6 && out.len() < 6000 {
                            let e = (t + per).min(qe);
                            for kk in 0..=6 {
                                let x = kk as f64 / 6.0;
                                let g = c0 + 0.25 * ((t - qs + (e - t) * x) / (qe - qs)) as f32;
                                let centre = rest + (hi - rest) * g * g;
                                let swing = (hi - rest) * (0.08 + 0.22 * g);
                                let v = centre + swing * (std::f64::consts::TAU * x).sin() as f32;
                                out.push((t + (e - t) * x * 0.99, v));
                            }
                            t = e;
                        }
                        per = (per * 0.5).max(0.125);
                    }
                }
            }
        }
        "answer" => {
            for f in &ph {
                let d = depth(f.energy);
                let mid = f.b0 + ((f.b1 - f.b0) * 0.5 / 4.0).round().max(1.0) * 4.0;
                let mid = mid.min(f.b1);
                let sd = mix(ctx.seed, f.letter, 13);
                let (l, h) = (lo + span * (1.0 - d) * 0.5, hi - span * (1.0 - d) * 0.5);
                let mut call = crate::api::auto_shape_points("sine", f.b0, mid, rate, l, h, sd);
                out.append(&mut call);
                if mid < f.b1 - 1e-6 {
                    // the answer: the same motion upside down and smaller, ending at rest
                    let c = (l + h) * 0.5;
                    let resp = crate::api::auto_shape_points("sine", mid, f.b1, rate, l, h, sd);
                    out.extend(resp.into_iter().map(|(t, v)| (t, c - (v - c) * 0.6)));
                }
            }
        }
        "build" => {
            for k in 0..=16 {
                let f = k as f64 / 16.0;
                out.push((b0 + (b1 - b0) * f, lo + span * f.powf(1.6) as f32));
            }
        }
        "fade" => {
            for k in 0..=12 {
                let f = k as f64 / 12.0;
                out.push((b0 + (b1 - b0) * f, hi - span * f.powf(0.8) as f32));
            }
        }
        "swell" | "pluck" | "open" | "gate" => {
            out.push((b0, if p == "gate" || p == "open" || p == "swell" { lo } else { hi }));
            for (k, &(on, len)) in notes.iter().enumerate() {
                let next = notes.get(k + 1).map(|n| n.0).unwrap_or(b1);
                let room = (next - on).max(0.02) - 0.01;
                let len = len.min(room.max(0.03)).max(0.02);
                // the phrase's energy sets how far each note opens; a phrase's first note is accented
                let f = ph.iter().find(|f| on >= f.b0 - 1e-6 && on < f.b1).cloned().unwrap_or(Phrase { b0, b1, letter: 0, repeat: 0, energy: 1.0, before_change: false });
                let first = (on - f.b0).abs() < 0.3;
                let top = if first { hi } else { lo + span * depth(f.energy) };
                match p {
                    "swell" => {
                        out.push((on, lo));
                        out.push((on + len * 0.5, top));
                        out.push((on + len, lo));
                    }
                    "pluck" => {
                        out.push((on, top));
                        out.push((on + (len * 0.7).min(0.75), lo + (top - lo) * 0.15));
                        out.push((on + len, lo));
                    }
                    "open" => {
                        out.push((on, lo));
                        out.push((on + len * 0.9, top));
                    }
                    _ => {
                        out.push((on - 0.006, lo));
                        out.push((on, top));
                        out.push((on + len * 0.9, top));
                        out.push((on + len * 0.9 + 0.01, lo));
                    }
                }
            }
        }
        _ => return render_flat(spec, ctx),
    }
    finish(out)
}

/// The pattern repeated evenly over the range in 8-bar blocks (the original layout): used for very long ranges at fast rates.
fn render_flat(spec: &Spec, ctx: &Ctx) -> Vec<(f64, f32)> {
    let (b0, b1) = (ctx.b0, ctx.b1.min(ctx.b0 + 2048.0));
    if !(b1 > b0 + 0.5) || spec.pattern == "hold" {
        return vec![];
    }
    let (lo, hi) = (spec.lo.clamp(0.0, 1.0), spec.hi.clamp(0.0, 1.0));
    // a range written upside down (lo above hi) is the same range
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let span = hi - lo;
    let mut rate = spec.rate.clamp(0.125, 64.0);
    // a fast cycle over a long range would be thousands of points: slow it down until the lane stays light to draw and to play
    let per_cycle = match spec.pattern {
        "wobble" | "breathe" => 9.0,
        "stutter" => 4.0,
        "sweep" | "pump" => 3.0,
        "steps" => 16.0,
        "random" => 8.0,
        _ => 0.0,
    };
    while per_cycle > 0.0 && (b1 - b0) / rate * per_cycle > 600.0 && rate < 64.0 {
        rate *= 2.0;
    }
    let block = if ctx.phrase >= 1.0 { ctx.phrase * 4.0 } else { 32.0 };
    let mut out: Vec<(f64, f32)> = vec![];
    let mut p = match spec.pattern {
        "groove" => "stutter",
        "arc" => "sweep",
        "answer" => "wobble",
        "accel" => "riser",
        q => q,
    };
    // notes inside the range
    let notes: Vec<(f64, f64)> = ctx.notes.iter().copied().filter(|n| n.0 >= b0 - 1e-6 && n.0 < b1).collect();
    if note_aware(p) && (notes.len() > 1800 || notes.is_empty()) {
        p = "wobble";
    }
    let shape = |name: &str, s: f64, e: f64, period: f64, out: &mut Vec<(f64, f32)>| {
        out.extend(crate::api::auto_shape_points(name, s, e, period, lo, hi, ctx.seed));
    };
    match p {
        "wobble" | "stutter" | "steps" | "random" | "breathe" | "sweep" => {
            let name = match p {
                "wobble" | "breathe" => "sine",
                "stutter" => "pulse",
                "sweep" => "tri",
                "steps" => "steps",
                _ => "random",
            };
            // the last bar of every phrase runs at double speed: a fill
            let fill_ok = ctx.fills && rhythmic(p) && block >= 16.0 && (b1 - b0) / rate * per_cycle < 300.0;
            let mut s = b0;
            while s < b1 - 1e-9 {
                let e = (s + block).min(b1);
                if fill_ok && e - s >= block - 1e-6 {
                    shape(name, s, e - 4.0, rate, &mut out);
                    shape(name, e - 4.0, e, (rate * 0.5).max(0.125), &mut out);
                } else {
                    shape(name, s, e, rate, &mut out);
                }
                s = e;
            }
        }
        "pump" => {
            let mut t = b0;
            while t < b1 - 1e-9 && out.len() < 6000 {
                let e = (t + rate).min(b1);
                out.push((t, lo));
                out.push((t + (e - t) * 0.18, lo + span * 0.35));
                out.push((e - 0.004, hi));
                t = e;
            }
        }
        "riser" => {
            let mut s = b0;
            while s < b1 - 1e-9 {
                let e = (s + block).min(b1);
                for k in 0..=8 {
                    let f = k as f64 / 8.0;
                    out.push((s + (e - s) * f * 0.985, lo + span * (f * f) as f32));
                }
                s = e;
            }
        }
        "build" => {
            for k in 0..=16 {
                let f = k as f64 / 16.0;
                out.push((b0 + (b1 - b0) * f, lo + span * f.powf(1.6) as f32));
            }
        }
        "fade" => {
            for k in 0..=12 {
                let f = k as f64 / 12.0;
                out.push((b0 + (b1 - b0) * f, hi - span * f.powf(0.8) as f32));
            }
        }
        "drift" => {
            let mut rng = Rng::new(ctx.seed as u64 + 17);
            let mut v = (lo + hi) * 0.5;
            let step = (rate * 0.5).clamp(1.0, 8.0);
            let mut t = b0;
            while t < b1 + step && out.len() < 3000 {
                out.push((t.min(b1), v));
                v = (v + ((rng.f() as f32) - 0.5) * span * 0.9).clamp(lo, hi);
                t += step;
            }
        }
        "swell" | "pluck" | "open" | "gate" => {
            out.push((b0, if p == "gate" || p == "open" || p == "swell" { lo } else { hi }));
            for (k, &(on, len)) in notes.iter().enumerate() {
                let next = notes.get(k + 1).map(|n| n.0).unwrap_or(b1);
                // the shape must finish before the next note starts
                let room = (next - on).max(0.02) - 0.01;
                let len = len.min(room.max(0.03)).max(0.02);
                match p {
                    "swell" => {
                        out.push((on, lo));
                        out.push((on + len * 0.5, hi));
                        out.push((on + len, lo));
                    }
                    "pluck" => {
                        out.push((on, hi));
                        out.push((on + (len * 0.7).min(0.75), lo + span * 0.15));
                        out.push((on + len, lo));
                    }
                    "open" => {
                        out.push((on, lo));
                        out.push((on + len * 0.9, hi));
                    }
                    _ => {
                        out.push((on - 0.006, lo));
                        out.push((on, hi));
                        out.push((on + len * 0.9, hi));
                        out.push((on + len * 0.9 + 0.01, lo));
                    }
                }
            }
        }
        _ => {}
    }
    finish(out)
}

// ── tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(notes: &[(f64, f64)]) -> Ctx<'_> {
        Ctx { b0: 0.0, b1: 64.0, phrase: 8.0, notes, groove: &[], seed: 5, fills: true }
    }

    #[test]
    fn roles_follow_the_names() {
        assert_eq!(role_of("cut"), Role::Filter);
        assert_eq!(role_of("cutoff"), Role::Filter);
        assert_eq!(role_of("res"), Role::Reso);
        assert_eq!(role_of("drive"), Role::Drive);
        assert_eq!(role_of("squash"), Role::Drive);
        assert_eq!(role_of("rv mix"), Role::Mix);
        assert_eq!(role_of("rv size"), Role::Space);
        assert_eq!(role_of("a det"), Role::Pitch);
        assert_eq!(role_of("pan"), Role::Pan);
        assert_eq!(role_of("vib rate"), Role::ModRate);
        assert_eq!(role_of("attack"), Role::Env);
        assert_eq!(role_of("vowel"), Role::Timbre);
    }

    #[test]
    fn switches_levels_and_sequence_steps_are_left_alone() {
        for n in ["gain", "vol", "mute", "hold", "voices", "tempo", "t s3", "t s15", "bypass"] {
            assert!(!eligible(n), "{n}");
        }
        for n in ["cut", "res", "drive", "t rate", "sync", "talk", "x m1cut", "x velamp"] {
            assert!(eligible(n), "{n}");
        }
        // pitch is never drawn: tuning, detune, glide-free bends, and the mod matrix into pitch / detune / bend
        for n in ["x velpit", "x whlpit", "x m1pit", "x l1det", "x keybpt", "a det", "b semi", "pitch", "fine", "tune"] {
            assert!(!eligible(n), "{n}");
        }
    }

    #[test]
    fn every_pattern_renders_sorted_finite_unit_values() {
        let notes: Vec<(f64, f64)> = (0..64).map(|i| (i as f64, 0.75)).collect();
        for (name, _) in PATTERNS {
            let spec = Spec { pattern: name, rate: 2.0, lo: 0.2, hi: 0.8 };
            let pts = render(&spec, &ctx(&notes));
            if name == "hold" {
                assert!(pts.is_empty());
                continue;
            }
            assert!(pts.len() >= 2, "{name}: {} points", pts.len());
            assert!(pts.windows(2).all(|w| w[1].0 > w[0].0), "{name}: not strictly increasing");
            assert!(pts.iter().all(|p| p.0.is_finite() && p.1.is_finite() && (0.0..=1.0).contains(&p.1)), "{name}");
            assert!(pts.iter().all(|p| p.0 >= -0.01 && p.0 <= 64.1), "{name}: outside the range");
            let (mn, mx) = pts.iter().fold((1.0f32, 0.0f32), |a, p| (a.0.min(p.1), a.1.max(p.1)));
            assert!(mx - mn > 0.1, "{name} does not move ({mn}..{mx})");
        }
    }

    #[test]
    fn a_fast_cycle_over_a_long_range_stays_light() {
        for name in ["wobble", "stutter", "sweep", "steps", "random", "pump", "breathe"] {
            let c = Ctx { b0: 0.0, b1: 1024.0, phrase: 8.0, notes: &[], groove: &[], seed: 1, fills: true };
            let pts = render(&Spec { pattern: name, rate: 0.25, lo: 0.1, hi: 0.9 }, &c);
            assert!(pts.len() < 1000, "{name}: {} points", pts.len());
        }
    }

    #[test]
    fn note_patterns_follow_the_notes_and_fall_back_without_them() {
        let notes = [(0.0, 1.0), (2.0, 1.0), (4.0, 0.5)];
        let c = Ctx { b0: 0.0, b1: 8.0, phrase: 2.0, notes: &notes, groove: &[], seed: 1, fills: false };
        let pts = render(&Spec { pattern: "pluck", rate: 1.0, lo: 0.1, hi: 0.9 }, &c);
        // the lane peaks at each onset
        for (on, _) in notes {
            assert!(pts.iter().any(|p| (p.0 - on).abs() < 0.02 && p.1 > 0.85), "no peak at {on}");
        }
        let none: [(f64, f64); 0] = [];
        let c2 = Ctx { notes: &none, ..c };
        assert!(render(&Spec { pattern: "pluck", rate: 1.0, lo: 0.1, hi: 0.9 }, &c2).len() > 4, "falls back to a wobble");
    }

    #[test]
    fn the_replies_of_a_model_are_read_tolerantly() {
        let good = r#"[{"p":0,"pattern":"riser","rate":16,"lo":0.2,"hi":0.95},{"p":1,"pattern":"Sweep","rate":"8","lo":0.25,"hi":0.7}]"#;
        let v = parse_reply(good);
        assert_eq!(v.len(), 2);
        assert_eq!(v[0].rate, Some(16.0));
        assert_eq!(v[1].rate, Some(8.0));
        let prose = "Sure! Here you go:\n```json\n[{\"p\":2,\"pattern\":\"wobble\",\"rate\":\"1/2\",\"lo\":0.1,\"hi\":0.9}]\n```";
        assert_eq!(parse_reply(prose)[0].rate, Some(0.5));
        // cut off by the token limit: the complete objects survive
        let cut = r#"[{"p":0,"pattern":"pump","rate":1,"lo":0.1,"hi":0.6},{"p":1,"pattern":"wob"#;
        assert_eq!(parse_reply(cut).len(), 1);
        assert!(parse_reply("no json at all").is_empty());
        assert!(parse_reply("").is_empty());
        let wrapped = r#"{"lanes":[{"param":3,"shape":"duck","min":0.2,"max":0.8}]}"#;
        let w = parse_reply(wrapped);
        assert_eq!(w[0].p, 3);
        assert_eq!(pattern_from_word(&w[0].pattern), Some("pump"));
    }

    #[test]
    fn a_nonsense_pick_still_gives_a_sane_spec() {
        let it = read_prompt("");
        let mut rng = Rng::new(1);
        for role in [Role::Filter, Role::Pitch, Role::Mix, Role::Env, Role::Pan, Role::Other] {
            for cur in [0.0f32, 0.5, 1.0, f32::NAN] {
                let pick = Pick { p: 0, pattern: "zzz".into(), rate: Some(f64::NAN), lo: Some(f32::INFINITY), hi: Some(-5.0) };
                let s = spec_for(role, cur, 0.5, &it, Some(&pick), &mut rng, false);
                assert!(!note_aware(s.pattern), "no notes, so no per-note pattern");
                assert!(s.lo >= 0.0 && s.hi <= 1.0 && s.hi - s.lo >= 0.039, "{role:?} {cur}: {} {}", s.lo, s.hi);
                assert!(s.rate.is_finite() && s.rate >= 0.25);
            }
        }
    }

    #[test]
    fn pitch_moves_less_than_a_filter_and_amount_widens_it() {
        let w = |role, amount| {
            let (l, h) = window("wobble", 0.5, amount, role);
            h - l
        };
        assert!(w(Role::Pitch, 0.5) < 0.5 * w(Role::Filter, 0.5));
        assert!(w(Role::Filter, 1.0) > w(Role::Filter, 0.1));
        // a parameter at its edge still moves
        let (l, h) = window("wobble", 0.0, 0.8, Role::Filter);
        assert!(l == 0.0 && h > 0.3);
    }

    #[test]
    fn the_words_steer_the_choice() {
        let i = read_prompt("tense build into a drop, aggressive and fast");
        assert!(i.bias.contains(&"riser") && i.bias.contains(&"wobble"));
        assert_eq!(i.amount, 1.0);
        assert_eq!(i.speed, 0.5);
        let i = read_prompt("subtle slow ambient pad");
        assert_eq!(i.amount, 0.3);
        assert_eq!(i.speed, 2.0);
        assert!(i.bias.contains(&"breathe"));
        // the bias is honoured often for a filter
        let mut rng = Rng::new(9);
        let it = read_prompt("wobble dubstep");
        let n = (0..200).filter(|_| matches!(choose_local(Role::Filter, &it, &mut rng, true), "wobble" | "pump" | "stutter")).count();
        assert!(n > 100, "only {n}/200 followed the words");
    }

    /// a 4-bar riff (A), the same again, a different 4 bars (B), A again: the form is found from the rhythm alone
    fn aaba() -> Vec<(f64, f64)> {
        let a: Vec<f64> = vec![0.0, 1.0, 1.5, 2.0, 3.0, 4.0, 5.5, 6.0, 7.0, 8.0, 9.0, 10.5, 11.0, 12.0, 14.0];
        let b: Vec<f64> = vec![0.0, 0.75, 1.5, 2.25, 3.0, 4.5, 6.75, 8.0, 8.25, 9.75, 12.0, 13.5, 15.0];
        let mut v = vec![];
        for (k, part) in [&a, &a, &b, &a].iter().enumerate() {
            for on in part.iter() {
                v.push((k as f64 * 16.0 + on, 0.4));
            }
        }
        v
    }

    #[test]
    fn the_form_is_read_from_the_notes() {
        let notes = aaba();
        let ph = phrase_map(0.0, 64.0, &notes, 0.0);
        assert_eq!(ph.len(), 4, "{ph:?}");
        assert_eq!(ph.iter().map(|p| p.letter).collect::<Vec<_>>(), vec![0, 0, 1, 0]);
        assert_eq!(ph.iter().map(|p| p.repeat).collect::<Vec<_>>(), vec![0, 1, 0, 2]);
        assert!(!ph[0].before_change && ph[1].before_change && ph[2].before_change);
        assert_eq!(form_text(&ph), "4-bar phrases: A A B A");
        // the length asked for wins
        assert_eq!(phrase_map(0.0, 256.0, &notes, 16.0).len(), 4);
        assert_eq!(phrase_map(0.0, 256.0, &[], 64.0).len(), 1);
        // without notes: A A B A by default and the arc peaks after the middle
        let none = phrase_map(0.0, 128.0, &[], 4.0);
        assert_eq!(none.iter().map(|p| p.letter).collect::<Vec<_>>(), vec![0, 0, 1, 0, 2, 2, 3, 2]);
        let peak = none.iter().enumerate().max_by(|a, b| a.1.energy.total_cmp(&b.1.energy)).unwrap().0;
        assert!((4..=6).contains(&peak), "peak at {peak}");
    }

    #[test]
    fn repeats_get_the_same_gesture_and_new_material_a_new_one() {
        let notes = aaba();
        let c = Ctx { b0: 0.0, b1: 64.0, phrase: 0.0, notes: &notes, groove: &[], seed: 3, fills: false };
        for pat in ["random", "groove", "wobble", "steps"] {
            let pts = render(&Spec { pattern: pat, rate: 1.0, lo: 0.1, hi: 0.9 }, &c);
            let seg = |s: f64| -> Vec<(i64, i64)> { pts.iter().filter(|p| p.0 > s + 0.01 && p.0 < s + 15.9).map(|p| (((p.0 - s) * 100.0).round() as i64, (p.1 * 1000.0).round() as i64)).collect() };
            let (a1, a2, b) = (seg(0.0), seg(16.0), seg(32.0));
            // the energy differs between the two A phrases, so compare the timing (the rhythm of the gesture), and B differs in
            // timing or values
            let times = |v: &[(i64, i64)]| v.iter().map(|x| x.0).collect::<Vec<_>>();
            assert_eq!(times(&a1), times(&a2), "{pat}: the A phrases move differently");
            assert!(times(&a1) != times(&b) || a1 != b, "{pat}: B is the same as A");
        }
    }

    #[test]
    fn the_groove_locks_to_the_music_and_risers_lead_into_changes() {
        // a tresillo bass: the groove pattern gates exactly on its onsets
        let mut notes = vec![];
        for bar in 0..16 {
            for s in [0.0, 0.75, 1.5, 2.5, 3.0] {
                notes.push((bar as f64 * 4.0 + s, 0.25));
            }
        }
        let g = groove_cell(&notes.iter().map(|n| n.0).collect::<Vec<_>>(), 0.0, 64.0).unwrap();
        assert_eq!(g, vec![0, 3, 6, 10, 12]);
        let c = Ctx { b0: 0.0, b1: 64.0, phrase: 8.0, notes: &notes, groove: &[], seed: 1, fills: false };
        let pts = render(&Spec { pattern: "groove", rate: 16.0, lo: 0.0, hi: 1.0 }, &c);
        for on in [4.0, 4.75, 5.5, 6.5, 7.0] {
            assert!(pts.iter().any(|p| (p.0 - on).abs() < 0.001 && p.1 > 0.4), "no gate at {on}");
        }
        // A A B A without notes: the riser climbs only at the end of the second A (into B) and of B (into A)
        let c = Ctx { b0: 0.0, b1: 64.0, phrase: 4.0, notes: &[], groove: &[], seed: 1, fills: true };
        for pat in ["riser", "accel"] {
            let pts = render(&Spec { pattern: pat, rate: 2.0, lo: 0.0, hi: 1.0 }, &c);
            let top = |s: f64, e: f64| pts.iter().filter(|p| p.0 >= s && p.0 < e).map(|p| p.1).fold(0.0f32, f32::max);
            assert!(top(0.0, 16.0) < 0.2, "{pat}: the first A rises");
            assert!(top(24.0, 32.0) > 0.7, "{pat}: no rise into B");
            assert!(top(40.0, 48.0) > 0.7, "{pat}: no rise back into A");
        }
        // accel: the LFO speeds up through the rise (more turning points in the last quarter than the first)
        let pts = render(&Spec { pattern: "accel", rate: 2.0, lo: 0.0, hi: 1.0 }, &c);
        let turns = |s: f64, e: f64| pts.windows(3).filter(|w| w[1].0 >= s && w[1].0 < e && (w[1].1 - w[0].1) * (w[2].1 - w[1].1) < 0.0).count();
        assert!(turns(30.0, 32.0) > turns(24.0, 26.0), "{} vs {}", turns(30.0, 32.0), turns(24.0, 26.0));
    }

    #[test]
    fn the_pump_ducks_on_the_kick() {
        let kick: Vec<f64> = (0..16).flat_map(|b| [b as f64 * 4.0, b as f64 * 4.0 + 2.5]).collect();
        let c = Ctx { b0: 0.0, b1: 32.0, phrase: 8.0, notes: &[], groove: &kick, seed: 1, fills: false };
        let pts = render(&Spec { pattern: "pump", rate: 1.0, lo: 0.0, hi: 1.0 }, &c);
        assert!(pts.iter().any(|p| (p.0 - 2.5).abs() < 0.001 && p.1 < 0.6), "no duck on the kick at 2.5");
        assert!(!pts.iter().any(|p| (p.0 - 1.0).abs() < 0.001 && p.1 < 0.6), "a duck without a kick at 1");
    }

    #[test]
    fn notes_closer_than_the_shortest_shape_do_not_break_the_note_patterns() {
        // legato glides from the Growl skin put note starts 0.04 beats apart
        let notes = [(0.0, 0.5), (0.04, 0.5), (0.06, 0.2), (1.0, 0.5)];
        for p in ["pluck", "swell", "open", "gate"] {
            for c in [Ctx { b0: 0.0, b1: 8.0, phrase: 2.0, notes: &notes, groove: &[], seed: 1, fills: false }] {
                let pts = render(&Spec { pattern: p, rate: 1.0, lo: 0.1, hi: 0.9 }, &c);
                assert!(pts.windows(2).all(|w| w[1].0 > w[0].0), "{p}");
                let flat = render_flat(&Spec { pattern: p, rate: 1.0, lo: 0.1, hi: 0.9 }, &c);
                assert!(flat.iter().all(|q| q.1.is_finite()), "{p}");
            }
        }
    }

    #[test]
    fn rates_snap_to_musical_values() {
        assert_eq!(snap_rate(1.1), 1.0);
        assert_eq!(snap_rate(3.0), 4.0);
        assert_eq!(snap_rate(f64::NAN), 1.0);
        assert_eq!(snap_rate(1000.0), 32.0);
    }
}
