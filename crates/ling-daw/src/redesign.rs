//! Redesign a track's sound from words: pick the plugin's preset that fits the part, then move its parameters by what they do.
//!
//! Every plugin (Growl, Prism, Pluck, the GM synth, Sampler ...) names its parameters plainly, so their jobs are read from the names
//! (`articulate::role_of`: cutoff -> filter, drive / fold / crush -> drive, rv mix -> space ...). Words move roles: "grittier" pushes
//! every drive-like parameter up, "darker" pulls the filters down, "wider" opens width and space, "punchier" shortens the
//! envelopes. Pitch, levels, switches, matrix cells and sequencer steps are never touched (`articulate::eligible`), so a redesign
//! never detunes or mutes a patch.
use crate::articulate::{eligible, role_of, Role};

/// (words, role, how far 0..1 at amount 1, what it is called in the summary)
const WORDS: &[(&[&str], Role, f32, &str)] = &[
    (&["gritty", "grittier", "grit", "dirty", "dirtier", "distorted", "crunch", "crunchy", "nasty", "aggressive", "heavier", "heavy", "filthy", "harder", "angrier", "angry"], Role::Drive, 0.22, "more drive"),
    (&["clean", "cleaner", "smooth", "smoother", "softer", "soft", "gentle", "gentler", "sweet", "sweeter"], Role::Drive, -0.2, "less drive"),
    (&["dark", "darker", "warm", "warmer", "muffled", "deeper", "duller", "mellow", "mellower"], Role::Filter, -0.18, "darker filter"),
    (&["bright", "brighter", "crisp", "crisper", "sharper", "airy", "airier", "shiny", "open", "opener"], Role::Filter, 0.18, "brighter filter"),
    (&["resonant", "squelchy", "squelch", "acid", "peaky", "vocal", "talky"], Role::Reso, 0.2, "more resonance"),
    (&["wide", "wider", "huge", "bigger", "big", "spacious", "stereo", "lush", "lusher"], Role::Pan, 0.15, "wider"),
    (&["wide", "wider", "huge", "bigger", "spacious", "lush", "lusher"], Role::Space, 0.1, "more space"),
    (&["narrow", "narrower", "mono", "tight", "tighter", "dry", "drier", "focused"], Role::Space, -0.2, "drier"),
    (&["spacey", "spacy", "ambient", "washy", "wet", "wetter", "dreamy", "dreamier", "ethereal"], Role::Space, 0.2, "more space"),
    (&["wobbly", "wobble", "wobblier", "movement", "moving", "alive", "evolving", "animated"], Role::ModDepth, 0.2, "more movement"),
    (&["static", "still", "steady", "stable"], Role::ModDepth, -0.2, "less movement"),
    (&["faster", "quicker", "busier"], Role::ModRate, 0.15, "faster modulation"),
    (&["slower", "lazier", "calmer"], Role::ModRate, -0.15, "slower modulation"),
    (&["punchy", "punchier", "snappy", "snappier", "plucky", "pluckier", "percussive", "shorter"], Role::Env, -0.15, "snappier envelopes"),
    (&["longer", "sustained", "legato", "padlike", "swelling", "slower attack"], Role::Env, 0.15, "longer envelopes"),
    (&["metallic", "metal", "harsh", "harsher", "digital", "glassy", "complex"], Role::Timbre, 0.18, "harsher timbre"),
    (&["round", "rounder", "simple", "simpler", "pure", "purer", "hollow"], Role::Timbre, -0.18, "rounder timbre"),
];

/// genres as bundles of moves
const STYLES: &[(&[&str], &[(Role, f32)])] = &[
    (&["dubstep", "riddim", "brostep", "tearout", "neuro", "bass music"], &[(Role::Drive, 0.2), (Role::ModDepth, 0.15), (Role::Timbre, 0.1)]),
    (&["lofi", "lo-fi", "tape", "vintage"], &[(Role::Filter, -0.15), (Role::Drive, 0.06), (Role::Space, 0.08)]),
    (&["trance", "uplifting", "euphoric"], &[(Role::Space, 0.12), (Role::Filter, 0.1), (Role::Pan, 0.1)]),
    (&["techno", "industrial", "rave"], &[(Role::Drive, 0.12), (Role::Reso, 0.1), (Role::Space, -0.05)]),
    (&["ambient", "cinematic", "chill"], &[(Role::Space, 0.22), (Role::Env, 0.15), (Role::Drive, -0.15), (Role::Filter, -0.05)]),
    (&["house", "disco", "funky", "funk"], &[(Role::Env, -0.08), (Role::Filter, 0.06), (Role::Drive, 0.04)]),
    (&["synthwave", "retro", "80s", "outrun"], &[(Role::ModDepth, 0.08), (Role::Space, 0.12), (Role::Filter, -0.05)]),
];

/// The role moves a request asks for (words count once each) and what they are called.
pub fn moves(request: &str) -> (Vec<(Role, f32)>, Vec<&'static str>) {
    let p = format!(" {} ", request.to_lowercase().replace([',', '.', '!', '?', ';', ':'], " "));
    let has = |w: &str| p.contains(&format!(" {w} "));
    let mut out: Vec<(Role, f32)> = vec![];
    let mut said: Vec<&'static str> = vec![];
    let mut add = |r: Role, d: f32, out: &mut Vec<(Role, f32)>| match out.iter_mut().find(|m| m.0 == r) {
        Some(m) => m.1 = (m.1 + d).clamp(-0.45, 0.45),
        None => out.push((r, d)),
    };
    // "much" / "a lot" doubles, "a bit" / "slightly" halves
    let k = if has("much") || has("lot") || has("way") || has("very") { 1.8 } else if has("bit") || has("slightly") || has("little") || has("touch") { 0.5 } else { 1.0 };
    for (ws, r, d, name) in WORDS {
        if ws.iter().any(|w| has(w)) {
            add(*r, d * k, &mut out);
            if !said.contains(name) {
                said.push(name);
            }
        }
    }
    for (ws, ms) in STYLES {
        if ws.iter().any(|w| has(w)) {
            for (r, d) in ms.iter() {
                add(*r, d * k, &mut out);
            }
            said.push(ws[0]);
        }
    }
    (out, said)
}

/// a parameter a redesign may move: something that shapes the sound, not a level, a switch, a pitch, a matrix cell, a lane point
/// or a sequencer step
pub fn movable(name: &str) -> bool {
    let n = name.to_lowercase();
    if !eligible(&n) || n == "pan" || n.starts_with("m ") || n.starts_with("x ") || n.starts_with("t s") {
        return false;
    }
    // drawn lane points: "l1 p3"
    let b = n.as_bytes();
    if b.len() >= 5 && b[0] == b'l' && b[1].is_ascii_digit() && &n[2..4] == " p" {
        return false;
    }
    !pitch_altering(&n) && !matches!(role_of(&n), Role::Other | Role::Pitch)
}

// ── the pitch guard ───────────────────────────────────────────────────────────────────────────────────────────────────

/// Does moving this parameter change the pitch (or its steadiness)? Vibrato, sweeps, glide / portamento, the pitch envelope, bend,
/// detune and tuning, frequency shift, FM ratios, octave switches, pitch drift and every modulation route into pitch. ONE rule for
/// every path that writes parameters (tweak, redesign, automation, synth words): these are left alone unless the request asks for
/// pitch ([`request_allows_pitch`]).
pub fn pitch_altering(name: &str) -> bool {
    let n = name.trim().to_lowercase();
    if n.is_empty() {
        return false;
    }
    if role_of(&n) == Role::Pitch {
        return true;
    }
    // Growl's matrix cells "m <source><destination>": destinations p (pitch), d (detune), k (comb tune), s (frequency shift)
    if let Some(cell) = n.strip_prefix("m ") {
        return cell.len() >= 2 && cell.ends_with(|c| matches!(c, 'p' | 'd' | 'k' | 's'));
    }
    const STARTS: [&str; 9] = ["vib", "sweep", "glid", "porta", "pitch", "bend", "scoop", "det", "slide"];
    const EXACT: [&str; 17] = ["pit", "tune", "ctune", "coarse", "fine", "semi", "shift", "fshift", "oct", "ratio", "rat", "root", "scale", "stretch", "drift", "penv", "pdec"];
    n.split(|c: char| !c.is_ascii_alphanumeric()).filter(|t| !t.is_empty()).any(|t| STARTS.iter().any(|s| t.starts_with(s)) || EXACT.contains(&t) || t.ends_with("pit") || t.ends_with("bpt") || t.ends_with("det"))
}

/// Did the request ask for pitch movement (pitch, bend, vibrato, glide, a riser, a scoop)? Only then may a pitch parameter move.
pub fn request_allows_pitch(request: &str) -> bool {
    let p = format!(" {} ", request.to_lowercase().replace([',', '.', '!', '?', ';', ':', '-'], " "));
    ["pitch", "pitches", "bend", "bends", "bending", "vibrato", "riser", "risers", "glide", "glides", "gliding", "portamento", "scoop", "slide", "dive", "detune", "detuned"].iter().any(|w| p.contains(&format!(" {w} ")))
}

/// may a request move this parameter?
pub fn param_allowed(name: &str, request: &str) -> bool {
    !pitch_altering(name) || request_allows_pitch(request)
}

// ── parameters by name ("longer release", "more drive", "shorter decay") ──────────────────────────────────────────────

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Target {
    Attack,
    Decay,
    Sustain,
    Release,
    Cutoff,
    Reso,
    Drive,
    Width,
    Space,
    Vibrato,
    Glide,
    Bend,
    Detune,
}

impl Target {
    fn is_pitch(self) -> bool {
        matches!(self, Target::Vibrato | Target::Glide | Target::Bend | Target::Detune)
    }
    fn matches(self, n: &str) -> bool {
        let first = n.split(|c: char| !c.is_ascii_alphanumeric()).find(|t| !t.is_empty()).unwrap_or("");
        match self {
            Target::Attack => n == "attack" || n == "amp a",
            Target::Decay => n == "decay" || n == "amp d",
            Target::Sustain => n == "sustain" || n == "amp s",
            Target::Release => n == "release" || n == "amp r",
            Target::Cutoff => n == "cutoff" || n == "cut",
            Target::Reso => n == "reso" || n == "res",
            Target::Drive => matches!(n, "drv" | "drive" | "fold" | "shaper"),
            Target::Width => matches!(n, "wide" | "width" | "stereo"),
            Target::Space => n.contains("verb") || first == "rv" || n == "dely" || first == "echo" || n == "delay",
            Target::Vibrato => first.starts_with("vib") && !n.contains("rate") && !n.contains("delay"),
            Target::Glide => first.starts_with("glid") || first.starts_with("porta"),
            Target::Bend => n == "bend",
            Target::Detune => matches!(n, "detune" | "detn" | "det"),
        }
    }
}

/// (target, change at full strength, what it is called)
pub type ParamMove = (Target, f32, &'static str);

fn target_word(w: &str) -> Option<(Target, &'static str)> {
    Some(match w {
        "attack" => (Target::Attack, "attack"),
        "decay" => (Target::Decay, "decay"),
        "sustain" => (Target::Sustain, "sustain"),
        "release" | "tail" => (Target::Release, "release"),
        "cutoff" | "filter" => (Target::Cutoff, "cutoff"),
        "resonance" | "reso" | "resonant" => (Target::Reso, "resonance"),
        "drive" | "distortion" | "saturation" => (Target::Drive, "drive"),
        "width" | "stereo" => (Target::Width, "width"),
        "reverb" | "echo" | "delay" => (Target::Space, "reverb"),
        "vibrato" => (Target::Vibrato, "vibrato"),
        "glide" | "portamento" => (Target::Glide, "glide"),
        "bend" => (Target::Bend, "bend"),
        "detune" | "unison" => (Target::Detune, "detune"),
        _ => return None,
    })
}

/// The named parameters a request moves, with their direction: "longer release and more drive" -> release up, drive up. The
/// direction word comes up to three words before the parameter. Pitch parameters (vibrato, glide, bend, detune) are only returned
/// when the request is about pitch.
pub fn param_words(request: &str) -> Vec<ParamMove> {
    let q = request.to_lowercase().replace([',', '.', '!', '?', ';', ':', '-'], " ");
    let w: Vec<&str> = q.split_whitespace().collect();
    let up = ["more", "longer", "higher", "increase", "extra", "bigger", "slower", "add", "raise", "stronger", "heavier", "open"];
    let down = ["less", "shorter", "lower", "decrease", "reduce", "tighter", "faster", "snappier", "quicker", "smaller", "weaker", "close", "remove"];
    let k = if w.iter().any(|x| matches!(*x, "much" | "lot" | "way" | "very")) { 2.0 } else if w.iter().any(|x| matches!(*x, "bit" | "slightly" | "little" | "touch")) { 0.4 } else { 1.0 };
    let mut out: Vec<ParamMove> = vec![];
    for (i, word) in w.iter().enumerate() {
        let Some((t, label)) = target_word(word) else { continue };
        // "cutoff resonance": the resonance is the thing
        if t == Target::Cutoff && w.get(i + 1).map(|x| matches!(*x, "resonance" | "reso")).unwrap_or(false) {
            continue;
        }
        let mut sign = 0.0f32;
        for j in (i.saturating_sub(3)..i).rev() {
            if up.contains(&w[j]) {
                sign = 1.0;
                break;
            }
            if down.contains(&w[j]) {
                sign = -1.0;
                break;
            }
        }
        if sign == 0.0 && w.get(i + 1).map(|x| up.contains(x) || down.contains(x)).unwrap_or(false) {
            sign = if up.contains(&w[i + 1]) { 1.0 } else { -1.0 };
        }
        if sign == 0.0 || (t.is_pitch() && !request_allows_pitch(request)) || out.iter().any(|m| m.0 == t) {
            continue;
        }
        out.push((t, sign * 0.15 * k, label));
    }
    out
}

/// New values for the named parameters (only the ones that change). Pitch parameters pass [`param_allowed`]; a detune never moves
/// further than a small unison amount at a time.
pub fn apply_param_words(params: &[(String, f32)], mv: &[ParamMove], request: &str, amount: f32) -> Vec<(String, f32)> {
    let mut out = vec![];
    for (name, v) in params {
        let n = name.to_lowercase();
        if !v.is_finite() || n.starts_with("m ") || n.starts_with("x ") || n.starts_with("t s") || !param_allowed(&n, request) {
            continue;
        }
        for (t, d, _) in mv {
            if t.matches(&n) {
                let step = if *t == Target::Detune { (d * amount).clamp(-0.05, 0.05) } else { d * amount };
                let nv = (v + step).clamp(0.0, 1.0);
                if (nv - v).abs() > 0.004 {
                    out.push((name.clone(), nv));
                }
                break;
            }
        }
    }
    out
}

/// New values for the parameters the moves touch (only the ones that change)
pub fn tweak(params: &[(String, f32)], mv: &[(Role, f32)], amount: f32) -> Vec<(String, f32)> {
    let mut out = vec![];
    for (name, v) in params {
        if !movable(name) || !v.is_finite() {
            continue;
        }
        let role = role_of(&name.to_lowercase());
        let Some((_, d)) = mv.iter().find(|m| m.0 == role) else { continue };
        let nv = (v + d * amount).clamp(0.0, 1.0);
        if (nv - v).abs() > 0.004 {
            out.push((name.clone(), nv));
        }
    }
    out
}

/// what a part is called -> words that suit it in a preset name (the part's own word counts most)
const ROLE_WORDS: &[(&str, &[&str])] = &[
    ("sub", &["808", "deep", "boom"]),
    ("bass", &["reese", "growl", "wob", "sub", "riddim", "808"]),
    ("wobble", &["wob", "wub"]),
    ("growl", &["riddim", "neuro", "throat", "talk"]),
    ("lead", &["screech", "saw", "hook", "anthem", "laser"]),
    ("laser", &["zap", "screech"]),
    ("pad", &["haze", "choir", "drift", "strings"]),
    ("pluck", &["stab", "tick", "pop"]),
    ("arp", &["pluck", "bell"]),
    ("stab", &["chord", "hit"]),
    ("chord", &["stab", "keys"]),
    ("keys", &["piano", "ep", "organ"]),
    ("bell", &["glass", "chime"]),
    ("riser", &["sweep", "noise"]),
];

/// The plugin's preset that best fits the part and the words (the seed breaks ties, so "try again" gives another good one).
pub fn pick_preset(presets: &[&str], role: &str, request: &str, seed: u64) -> Option<String> {
    let r = role.to_lowercase();
    let q = request.to_lowercase();
    let qw: Vec<&str> = q.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).collect();
    let mut best: Option<(i64, &str)> = None;
    for (i, p) in presets.iter().enumerate() {
        let n = p.to_lowercase();
        let mut s = 0i64;
        for (k, ws) in ROLE_WORDS {
            if !r.contains(k) {
                continue;
            }
            if n.contains(k) {
                s += 40;
            } else if ws.iter().any(|w| n.contains(w)) {
                s += 15;
            }
        }
        for w in &qw {
            if n.contains(w) {
                s += 20;
            }
        }
        if s == 0 {
            continue;
        }
        // a stable shuffle of the ties
        let tie = (((i as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ seed.wrapping_mul(0xBF58_476D_1CE4_E5B9)) >> 52) as i64;
        let s = s * 10_000 + tie;
        if best.map(|b| s > b.0).unwrap_or(true) {
            best = Some((s, p));
        }
    }
    best.map(|b| b.1.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn words_move_the_right_parameters_and_never_pitch_or_levels() {
        let (mv, said) = moves("make this patch a bit grittier and darker");
        assert!(said.contains(&"more drive") && said.contains(&"darker filter"), "{said:?}");
        let params: Vec<(String, f32)> = ["drv", "cut", "res", "detn", "out", "c tune", "m l1c", "l1 p3", "fold", "rv mix", "pan", "fshift"].iter().map(|n| (n.to_string(), 0.5)).collect();
        let t = tweak(&params, &mv, 1.0);
        let get = |n: &str| t.iter().find(|x| x.0 == n).map(|x| x.1);
        assert!(get("drv").unwrap() > 0.5 && get("fold").unwrap() > 0.5, "{t:?}");
        assert!(get("cut").unwrap() < 0.5, "{t:?}");
        for n in ["detn", "out", "c tune", "m l1c", "l1 p3", "pan", "fshift"] {
            assert!(get(n).is_none(), "{n} must not move: {t:?}");
        }
        // "a bit" is half as far as plain
        let full = tweak(&params, &moves("grittier").0, 1.0);
        assert!(get("drv").unwrap() - 0.5 < full.iter().find(|x| x.0 == "drv").unwrap().1 - 0.5);
        // a genre is a bundle of moves
        assert!(moves("more dubstep").0.iter().any(|m| m.0 == Role::Drive && m.1 > 0.0));
        assert!(moves("hello").0.is_empty());
    }

    #[test]
    fn the_pitch_guard_covers_every_instrument() {
        for n in ["vib", "vib rate", "vib depth", "vib delay", "vibrato", "vibrate", "sweep", "sweep time", "glide", "glid", "t glid", "penv", "pdec", "bend", "bend range", "detune", "detn", "a det", "c tune", "tune", "fshift", "fm ratio", "b rat", "sub oct", "arp oct", "lfo pitch", "scoop", "drift", "stretch", "root", "scale", "x l1pit", "x velbpt", "m l1p", "m envs", "m whld", "coarse", "fine"] {
            assert!(pitch_altering(n), "{n} changes pitch");
        }
        for n in ["cut", "cutoff", "res", "reso", "drv", "drive", "fold", "attack", "release", "decay", "sustain", "amp a", "amp r", "rv mix", "wide", "width", "verb", "dely", "lfo cut", "lfo rate", "filt env", "fm index", "fm env", "wave", "noise", "pan", "gain", "m l1c", "x l1cut"] {
            assert!(!pitch_altering(n), "{n} does not change pitch");
        }
        // every parameter of every instrument: nothing the tweak / synth-word paths can move changes pitch
        let (mv, _) = moves("grittier darker brighter wider punchier longer shorter wobbly faster slower calmer dubstep more drive resonant spacey");
        let pw = param_words("longer release shorter decay more drive more cutoff resonance a bit wider less attack");
        assert!(pw.len() >= 4, "{pw:?}");
        for kind in ["synth", "growl", "prism", "pluck", "chip", "sampler", "voice", "drums"] {
            let Some(inst) = crate::dsp::make_instrument(kind, 44100.0) else { continue };
            let params: Vec<(String, f32)> = inst.params().into_iter().map(|(n, v)| (n.to_string(), if v.is_finite() { v.clamp(0.1, 0.9) } else { 0.5 })).collect();
            for (n, _) in tweak(&params, &mv, 1.0) {
                assert!(!pitch_altering(&n), "{kind}: tweak would move {n}");
            }
            for (n, _) in apply_param_words(&params, &pw, "longer release", 1.0) {
                assert!(!pitch_altering(&n), "{kind}: param words would move {n}");
            }
            for (n, _) in &params {
                if movable(n) {
                    assert!(!pitch_altering(n), "{kind}: {n} is movable but changes pitch");
                }
            }
        }
        // a pitch request is the only way in
        assert!(request_allows_pitch("pitch bend into the chord") && request_allows_pitch("add a riser"));
        assert!(!request_allows_pitch("filter sweep up over the build") && !request_allows_pitch("brighter, more drive"));
        let params = vec![("vib depth".to_string(), 0.2f32), ("cutoff".to_string(), 0.5)];
        let vib = vec![(Target::Vibrato, 0.15f32, "vibrato")];
        assert!(apply_param_words(&params, &vib, "brighter please", 1.0).is_empty(), "no pitch request, no vibrato");
        assert!(apply_param_words(&params, &vib, "more vibrato", 1.0).iter().any(|g| g.0 == "vib depth"));
        assert!(param_words("more brightness and drive").iter().all(|m| !m.0.is_pitch()));
    }

    #[test]
    fn named_parameters_move_the_right_way() {
        let params: Vec<(String, f32)> = [("attack", 0.5), ("release", 0.5), ("decay", 0.5), ("drive", 0.3), ("cutoff", 0.5), ("reso", 0.3)].iter().map(|(n, v)| (n.to_string(), *v)).collect();
        let r = "longer release, shorter decay, more drive, more cutoff resonance";
        let t = apply_param_words(&params, &param_words(r), r, 1.0);
        let get = |n: &str| t.iter().find(|x| x.0 == n).map(|x| x.1);
        assert!(get("release").unwrap() > 0.5 && get("decay").unwrap() < 0.5 && get("drive").unwrap() > 0.3 && get("reso").unwrap() > 0.3, "{t:?}");
        assert!(get("cutoff").is_none() && get("attack").is_none(), "{t:?}");
    }

    #[test]
    fn the_preset_fits_the_part() {
        let presets = ["wobble", "comb growl", "big room lead", "prog pluck", "sub drop", "crimson pad"];
        assert_eq!(pick_preset(&presets, "growl bass", "", 1).as_deref(), Some("comb growl"));
        assert_eq!(pick_preset(&presets, "warm pad", "", 1).as_deref(), Some("crimson pad"));
        assert_eq!(pick_preset(&presets, "supersaw lead", "", 1).as_deref(), Some("big room lead"));
        assert_eq!(pick_preset(&presets, "pluck", "", 3).as_deref(), Some("prog pluck"));
        // the words count too
        assert_eq!(pick_preset(&presets, "bass", "a wobble", 1).as_deref(), Some("wobble"));
        assert!(pick_preset(&presets, "texture", "zzz", 1).is_none());
    }
}
