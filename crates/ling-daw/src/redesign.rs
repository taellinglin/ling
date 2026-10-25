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
    !matches!(role_of(&n), Role::Other | Role::Pitch)
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
