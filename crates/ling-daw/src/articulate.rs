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
    if n == "pan" || has(&n, &["width", "stereo", "spread"]) {
        Role::Pan
    } else if has(&n, &["reso", "emph", " q", "peak"]) || n == "q" || n == "res" || n.ends_with(" res") {
        Role::Reso
    } else if has(&n, &["cut", "freq", "bright", "tone", "tilt", "lp", "hp", "filter", "flt", "bright", "bandw", "lowpass", "high", "low", "mid"]) {
        Role::Filter
    } else if has(&n, &["drive", "fold", "dist", "squash", "warm", "sat", "crush", "grit", "clip", "shaper", "gate", "bite"]) {
        Role::Drive
    } else if has(&n, &["pitch", "tune", "det", "coarse", "fine", "semi", "bend", "shift", "oct"]) {
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
    } else if has(&n, &["vowel", "gender", "shape", "wave", "morph", "table", "formant", "pos", "scan", "talk", "sample", "xsyn", "grain", "noise", "sub", "uni", "wid", "harm", "ring", "voice", "choir", "open", "fry", "nasal", "breath", "whisp", "tilt", "spec", "cons"]) {
        Role::Timbre
    } else {
        Role::Other
    }
}

/// Parameters that are switches, levels the mix depends on, or steps of a sequence: never automated by the generators.
pub fn eligible(name: &str) -> bool {
    let n = name.to_lowercase();
    if n == "vol" || n == "gain" || n == "level" || n == "master" || n == "voices" || n == "tempo" || n == "bpm" || n == "mute" {
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
pub const PATTERNS: [(&str, &str); 16] = [
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
        _ => return None,
    })
}

pub fn note_aware(p: &str) -> bool {
    matches!(p, "swell" | "pluck" | "open" | "gate")
}

fn rhythmic(p: &str) -> bool {
    matches!(p, "wobble" | "pump" | "stutter" | "steps" | "random")
}

/// the patterns that suit each role, most natural first
pub fn role_patterns(r: Role) -> &'static [&'static str] {
    match r {
        Role::Filter => &["wobble", "sweep", "riser", "pluck", "open", "pump", "steps", "breathe", "gate"],
        Role::Reso => &["sweep", "breathe", "wobble", "riser", "drift"],
        Role::Drive => &["swell", "pump", "riser", "breathe", "build", "drift"],
        Role::Pitch => &["drift", "breathe", "wobble"],
        Role::Mix => &["swell", "breathe", "riser", "pump", "build", "drift"],
        Role::Space => &["breathe", "riser", "drift", "sweep", "build"],
        Role::ModDepth => &["sweep", "riser", "wobble", "steps", "swell", "build"],
        Role::ModRate => &["sweep", "steps", "drift", "riser"],
        Role::Timbre => &["sweep", "drift", "steps", "random", "breathe", "riser", "swell", "open"],
        Role::Pan => &["wobble", "drift", "sweep", "breathe"],
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
    let from_words: Vec<&'static str> = intent.bias.iter().copied().filter(|p| list.contains(p) || matches!(*p, "riser" | "build" | "fade") && role != Role::Env && role != Role::Pitch).filter(ok).collect();
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
        "riser" | "build" => (cur - half * 0.3, cur + half * 2.0),
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
    /// beats in a phrase: risers restart and fills land at the end of each
    pub block: f64,
    /// (onset, length) of every note, sorted by onset, in beats
    pub notes: &'a [(f64, f64)],
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

/// Lay a pattern out over `ctx.b0..ctx.b1`. Empty for "hold".
pub fn render(spec: &Spec, ctx: &Ctx) -> Vec<(f64, f32)> {
    let (b0, b1) = (ctx.b0, ctx.b1.min(ctx.b0 + 2048.0));
    if !(b1 > b0 + 0.5) || spec.pattern == "hold" {
        return vec![];
    }
    let (lo, hi) = (spec.lo.clamp(0.0, 1.0), spec.hi.clamp(0.0, 1.0));
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
    let block = ctx.block.max(4.0);
    let mut out: Vec<(f64, f32)> = vec![];
    let mut p = spec.pattern;
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
                let len = len.clamp(0.05, room.max(0.03));
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
        Ctx { b0: 0.0, b1: 64.0, block: 32.0, notes, seed: 5, fills: true }
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
        for n in ["cut", "res", "drive", "t rate", "sync", "talk"] {
            assert!(eligible(n), "{n}");
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
            let c = Ctx { b0: 0.0, b1: 1024.0, block: 32.0, notes: &[], seed: 1, fills: true };
            let pts = render(&Spec { pattern: name, rate: 0.25, lo: 0.1, hi: 0.9 }, &c);
            assert!(pts.len() < 1000, "{name}: {} points", pts.len());
        }
    }

    #[test]
    fn note_patterns_follow_the_notes_and_fall_back_without_them() {
        let notes = [(0.0, 1.0), (2.0, 1.0), (4.0, 0.5)];
        let c = Ctx { b0: 0.0, b1: 8.0, block: 8.0, notes: &notes, seed: 1, fills: false };
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

    #[test]
    fn rates_snap_to_musical_values() {
        assert_eq!(snap_rate(1.1), 1.0);
        assert_eq!(snap_rate(3.0), 4.0);
        assert_eq!(snap_rate(f64::NAN), 1.0);
        assert_eq!(snap_rate(1000.0), 32.0);
    }
}
