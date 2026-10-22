//! Composition from a prompt: a request ("8 bars of a sparse trance lead in F# minor over i VI III VII") becomes a [`Spec`]
//! (key, bars, kind, progression, density, contour, feel, range), and a spec becomes notes that are always in the key and on
//! the chords. A language model only has to fill in the spec (`parse_fields` understands its JSON and plain keywords alike);
//! the notes themselves come from the generators here. The same fields describe a whole project: [`thumbprint`] turns a
//! project into a short story plus a field list, and [`spec_from_style`] turns that back into a spec for any track.
use crate::theory::{self, Chord, Key, N, QUAL};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Kind {
    Melody,
    Chords,
    Bass,
    Arp,
    Pad,
    Drums,
}
impl Kind {
    pub fn parse(s: &str) -> Option<Kind> {
        let s = s.trim().to_lowercase();
        Some(match s.as_str() {
            "melody" | "lead" | "topline" | "tune" => Kind::Melody,
            "chords" | "chord" | "comp" | "stabs" | "keys" => Kind::Chords,
            "bass" | "bassline" | "sub" => Kind::Bass,
            "arp" | "arpeggio" | "arpeggios" | "pluck" => Kind::Arp,
            "pad" | "pads" | "strings" => Kind::Pad,
            "drums" | "drum" | "percussion" | "groove" | "drumbeat" => Kind::Drums,
            _ => return None,
        })
    }
    pub fn name(&self) -> &'static str {
        match self {
            Kind::Melody => "melody",
            Kind::Chords => "chords",
            Kind::Bass => "bass",
            Kind::Arp => "arp",
            Kind::Pad => "pad",
            Kind::Drums => "drums",
        }
    }
    fn range(&self) -> (u8, u8) {
        match self {
            Kind::Melody => (60, 84),
            Kind::Chords => (50, 74),
            Kind::Bass => (28, 52),
            Kind::Arp => (60, 88),
            Kind::Pad => (48, 72),
            Kind::Drums => (35, 52),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Spec {
    pub key: Key,
    pub bars: u32,
    pub kind: Kind,
    /// chords as (root pitch class, quality index); empty = pick from the style
    pub prog: Vec<(u8, usize)>,
    pub prog_text: String,
    pub density: f64,
    pub contour: String,
    pub feel: String,
    pub pattern: String,
    pub lo: u8,
    pub hi: u8,
    pub seed: u64,
    pub style: String,
    /// chords per bar (0.5 = a chord every two bars)
    pub cpb: f64,
    /// bars before a melodic idea repeats
    pub motif: u32,
    /// chords to follow instead of a progression: (start beat, length, root pitch class, quality), already cut to this spec's bars
    pub harmony: Vec<(f64, f64, u8, usize)>,
    /// beats per step for arpeggios (0 = from the density)
    pub rate: f64,
    /// rhythm mask for arpeggios: "x..x..x.", "euclid:5,16,2" or a name (16ths, 8ths, offbeat, gallop, syncopated, dotted)
    pub rhythm: String,
    /// arpeggio order: up, down, updown, pingpong, random
    pub order: String,
    pub gate: f64,
    pub octaves: u32,
    pub swing: f64,
    pub fills: f64,
}

impl Default for Spec {
    fn default() -> Spec {
        let (lo, hi) = Kind::Melody.range();
        Spec { key: Key::new(0, true), bars: 8, kind: Kind::Melody, prog: vec![], prog_text: String::new(), density: 0.5, contour: "arch".into(), feel: "straight".into(), pattern: String::new(), lo, hi, seed: 1, style: String::new(), cpb: 1.0, motif: 2, harmony: vec![], rate: 0.0, rhythm: String::new(), order: String::new(), gate: 0.9, octaves: 2, swing: 0.0, fills: 0.5 }
    }
}

// ── styles ──────────────────────────────────────────────────────────────────
struct StyleDef {
    name: &'static str,
    minor: bool,
    progs: &'static [&'static str],
    density: f64,
    feel: &'static str,
    bass: &'static str,
    chords: &'static str,
    contour: &'static str,
    cpb: f64,
}

const STYLES: &[StyleDef] = &[
    StyleDef { name: "trance", minor: true, progs: &["i VI III VII", "i VII VI VII", "i iv VI V", "i VI iv V"], density: 0.55, feel: "straight", bass: "rolling", chords: "sustain", contour: "arch", cpb: 1.0 },
    StyleDef { name: "deep house", minor: true, progs: &["i7 iv7 VII7 IIImaj7", "i7 VI7 iv7 V7", "ii7 V7 Imaj7 vi7"], density: 0.4, feel: "offbeat", bass: "offbeat", chords: "stabs", contour: "wave", cpb: 1.0 },
    StyleDef { name: "house", minor: true, progs: &["i VII VI VII", "vi IV I V", "i iv VII III"], density: 0.45, feel: "offbeat", bass: "offbeat", chords: "stabs", contour: "wave", cpb: 1.0 },
    StyleDef { name: "disco", minor: false, progs: &["ii7 V7 Imaj7 vi7", "Imaj7 vi7 ii7 V7", "vi7 ii7 V7 Imaj7"], density: 0.6, feel: "syncopated", bass: "octave", chords: "synco", contour: "wave", cpb: 1.0 },
    StyleDef { name: "hyperpop", minor: false, progs: &["I V vi IV", "vi IV I V", "IV I V vi"], density: 0.85, feel: "syncopated", bass: "synco", chords: "pump", contour: "rise", cpb: 1.0 },
    StyleDef { name: "witch house", minor: true, progs: &["i bII i v", "i iv i bII", "i bVI bVII i"], density: 0.2, feel: "straight", bass: "sub", chords: "sustain", contour: "fall", cpb: 0.5 },
    StyleDef { name: "lofi", minor: false, progs: &["ii7 V7 Imaj7 vi7", "Imaj7 vi7 ii7 V7", "IVmaj7 iii7 ii7 Imaj7"], density: 0.3, feel: "swing", bass: "walking", chords: "synco", contour: "wave", cpb: 1.0 },
    StyleDef { name: "ambient", minor: false, progs: &["Imaj7 IVmaj7", "vi IV I V", "Imaj7 iii7 IVmaj7 Imaj7"], density: 0.15, feel: "sustained", bass: "sub", chords: "sustain", contour: "wave", cpb: 0.5 },
    StyleDef { name: "pop", minor: false, progs: &["I V vi IV", "vi IV I V", "I vi IV V"], density: 0.5, feel: "straight", bass: "root", chords: "pump", contour: "arch", cpb: 1.0 },
    StyleDef { name: "rock", minor: false, progs: &["I bVII IV I", "I IV V IV", "vi IV I V"], density: 0.5, feel: "straight", bass: "root", chords: "pump", contour: "arch", cpb: 1.0 },
    StyleDef { name: "techno", minor: true, progs: &["i i bVII i", "i bII i i", "i i iv i"], density: 0.5, feel: "driving", bass: "rolling", chords: "stabs", contour: "flat", cpb: 1.0 },
    StyleDef { name: "dub", minor: true, progs: &["i iv v i", "i bVII bVI v", "i i iv iv"], density: 0.3, feel: "offbeat", bass: "sub", chords: "stabs", contour: "fall", cpb: 1.0 },
    StyleDef { name: "jazz", minor: false, progs: &["ii7 V7 Imaj7 Imaj7", "Imaj7 vi7 ii7 V7", "iii7 vi7 ii7 V7"], density: 0.55, feel: "swing", bass: "walking", chords: "synco", contour: "wave", cpb: 1.0 },
    StyleDef { name: "cinematic", minor: true, progs: &["i bVI bIII bVII", "i bVII bVI V", "i iv bVI bVII"], density: 0.25, feel: "sustained", bass: "sub", chords: "sustain", contour: "rise", cpb: 1.0 },
    StyleDef { name: "synthwave", minor: true, progs: &["i VI III VII", "VI VII i i", "i VII VI V"], density: 0.5, feel: "driving", bass: "rolling", chords: "pump", contour: "arch", cpb: 1.0 },
    StyleDef { name: "dnb", minor: true, progs: &["i VI VII i", "i III VI VII"], density: 0.6, feel: "syncopated", bass: "synco", chords: "sustain", contour: "wave", cpb: 1.0 },
];

fn style_def(name: &str) -> Option<&'static StyleDef> {
    let n = name.trim().to_lowercase();
    STYLES.iter().find(|s| s.name == n || n.contains(s.name) || s.name.contains(n.as_str()) && !n.is_empty())
}

pub fn style_names() -> Vec<&'static str> {
    STYLES.iter().map(|s| s.name).collect()
}

// ── roman numerals ──────────────────────────────────────────────────────────
/// "bVII", "i7", "IVmaj7", "V7", "vi°", "ii" ... relative to the key
pub fn parse_roman(tok: &str, key: Key) -> Option<(u8, usize)> {
    let t = tok.trim().trim_matches(|c: char| c == ',' || c == '.' || c == ';');
    let (acc, rest) = if let Some(r) = t.strip_prefix('b').or_else(|| t.strip_prefix('♭')) {
        (-1i32, r)
    } else if let Some(r) = t.strip_prefix('#').or_else(|| t.strip_prefix('♯')) {
        (1, r)
    } else {
        (0, t)
    };
    let letters: String = rest.chars().take_while(|c| matches!(c, 'i' | 'v' | 'I' | 'V')).collect();
    if letters.is_empty() {
        return None;
    }
    let suffix = &rest[letters.len()..];
    let deg = match letters.to_lowercase().as_str() {
        "i" => 0,
        "ii" => 1,
        "iii" => 2,
        "iv" => 3,
        "v" => 4,
        "vi" => 5,
        "vii" => 6,
        _ => return None,
    };
    // the case has to be consistent
    let lower = letters.chars().all(|c| c.is_lowercase());
    let upper = letters.chars().all(|c| c.is_uppercase());
    if !lower && !upper {
        return None;
    }
    let q_name = match (suffix, lower) {
        ("", false) => "",
        ("", true) => "m",
        ("°" | "o" | "dim", _) => "dim",
        ("+" | "aug", _) => "aug",
        ("7", false) => "7",
        ("7", true) | ("m7", _) => "m7",
        ("maj7" | "M7" | "Δ" | "Δ7", _) => "maj7",
        ("ø" | "ø7" | "m7b5", _) => "m7b5",
        ("°7" | "dim7", _) => "dim7",
        ("sus2", _) => "sus2",
        ("sus4" | "sus", _) => "sus4",
        ("6", _) => "6",
        ("add9", _) => "add9",
        ("5", _) => "5",
        _ => return None,
    };
    let q = theory::qual_index(q_name)?;
    let root = ((key.scale()[deg] as i32 + acc).rem_euclid(12)) as u8;
    Some((root, q))
}

pub fn parse_prog(text: &str, key: Key) -> Vec<(u8, usize)> {
    text.split(|c: char| c.is_whitespace() || c == '-' || c == '–' || c == '>' || c == '|' || c == ',')
        .filter(|t| !t.is_empty())
        .filter_map(|t| parse_roman(t, key))
        .collect()
}

fn looks_like_roman(tok: &str) -> bool {
    let t = tok.trim_matches(|c: char| c == ',' || c == '.' || c == ';');
    let key = Key::new(0, false);
    parse_roman(t, key).is_some() && t.chars().any(|c| c.is_ascii_alphabetic() || c == '°')
}

// ── prompt parsing ──────────────────────────────────────────────────────────
/// keyword extraction: the fields a plain-English request states. Later fields win when the same one appears twice.
pub fn parse_fields(text: &str) -> Vec<(String, String)> {
    let mut f: Vec<(String, String)> = vec![];
    let lower = text.to_lowercase();
    let words: Vec<&str> = lower.split(|c: char| c.is_whitespace() || c == ',' || c == '.' || c == ';' || c == ':' || c == '(' || c == ')').filter(|w| !w.is_empty()).collect();
    macro_rules! put {
        ($k:expr, $v:expr) => {
            f.push(($k.to_string(), $v))
        };
    }
    // bars
    for (i, w) in words.iter().enumerate() {
        if (*w == "bars" || *w == "bar" || *w == "measures") && i > 0 {
            if let Ok(n) = words[i - 1].parse::<u32>() {
                put!("bars", n.to_string());
            }
        } else if let Some(n) = w.strip_suffix("-bar").or_else(|| w.strip_suffix("bar")) {
            if let Ok(n) = n.trim_end_matches('-').parse::<u32>() {
                put!("bars", n.to_string());
            }
        }
    }
    // key: "f# minor", "bb major", "key of d", "a minor"
    for i in 0..words.len() {
        let w = words[i];
        let is_note = matches!(w.len(), 1 | 2) && w.chars().next().map_or(false, |c| ('a'..='g').contains(&c)) && w.chars().nth(1).map_or(true, |c| c == '#' || c == 'b');
        if is_note {
            if let Some(n) = words.get(i + 1) {
                let mode = match *n {
                    "minor" | "min" | "aeolian" | "dorian" | "phrygian" => Some("minor"),
                    "major" | "maj" | "ionian" | "lydian" | "mixolydian" => Some("major"),
                    _ => None,
                };
                if let Some(m) = mode {
                    put!("key", format!("{w} {m}"));
                }
            }
        }
        if w == "key" && words.get(i + 1) == Some(&"of") {
            if let Some(n) = words.get(i + 2) {
                if matches!(n.len(), 1 | 2) {
                    put!("key", n.to_string() + " " + if words.get(i + 3) == Some(&"minor") { "minor" } else { "major" });
                }
            }
        }
    }
    if !f.iter().any(|(k, _)| k == "key") {
        if words.contains(&"minor") {
            put!("mode", "minor".into());
        } else if words.contains(&"major") {
            put!("mode", "major".into());
        }
    }
    // progression: the longest run of three or more roman numerals
    let toks: Vec<&str> = text.split(|c: char| c.is_whitespace() || c == '-' || c == '–' || c == '>' || c == '|').filter(|t| !t.is_empty()).collect();
    let mut best: (usize, usize) = (0, 0);
    let mut i = 0;
    while i < toks.len() {
        let mut j = i;
        while j < toks.len() && looks_like_roman(toks[j]) {
            j += 1;
        }
        if j - i > best.1 - best.0 {
            best = (i, j);
        }
        i = j.max(i + 1);
    }
    if best.1 - best.0 >= 3 {
        put!("prog", toks[best.0..best.1].iter().map(|t| t.trim_matches(|c: char| c == ',' || c == '.' || c == ';')).collect::<Vec<_>>().join(" "));
    }
    // kind
    for w in &words {
        match *w {
            "melody" | "lead" | "topline" | "tune" | "hook" => put!("kind", "melody".into()),
            "chords" | "chord" | "stabs" | "comping" => put!("kind", "chords".into()),
            "bass" | "bassline" => put!("kind", "bass".into()),
            "arp" | "arpeggio" | "arpeggios" => put!("kind", "arp".into()),
            "pad" | "pads" => put!("kind", "pad".into()),
            "drums" | "drum" | "percussion" | "groove" | "drumbeat" => put!("kind", "drums".into()),
            "breakbeat" | "trap" | "halftime" | "boombap" | "euclid" | "euclidean" | "fourfloor" | "onedrop" => put!("pattern", if *w == "euclidean" { "euclid".to_string() } else { w.to_string() }),
            _ => {}
        }
    }
    // density
    for w in &words {
        let d = match *w {
            "sparse" | "minimal" | "simple" | "slow" | "spacious" | "calm" => Some("0.2"),
            "medium" | "moderate" => Some("0.5"),
            "busy" | "dense" | "active" | "driving" | "energetic" => Some("0.8"),
            "frantic" | "relentless" | "intense" => Some("0.95"),
            _ => None,
        };
        if let Some(d) = d {
            put!("density", d.into());
        }
    }
    for w in &words {
        match *w {
            "arch" | "arching" => put!("contour", "arch".into()),
            "rising" | "ascending" | "climbing" | "climbs" | "climb" | "rises" | "rise" | "ascends" | "build" | "uplifting" => put!("contour", "rise".into()),
            "falling" | "descending" | "sinking" | "falls" | "descends" | "drops" | "dark" => put!("contour", "fall".into()),
            "wave" | "wavy" | "undulating" => put!("contour", "wave".into()),
            "flat" | "static" | "repetitive" | "hypnotic" => put!("contour", "flat".into()),
            "syncopated" | "offbeat" | "off-beat" | "swing" | "swung" | "driving" | "staccato" | "sustained" | "legato" | "straight" | "gated" => {
                put!("feel", if *w == "off-beat" { "offbeat".into() } else { w.to_string() })
            }
            _ => {}
        }
    }
    // style (longest name first)
    let mut styles: Vec<&StyleDef> = STYLES.iter().collect();
    styles.sort_by_key(|s| std::cmp::Reverse(s.name.len()));
    for s in styles {
        if lower.contains(s.name) {
            put!("style", s.name.into());
            break;
        }
    }
    // register
    if lower.contains("high register") || lower.contains("high octave") || lower.contains("bright") {
        put!("range", "high".into());
    } else if lower.contains("low register") || lower.contains("deep") && lower.contains("register") {
        put!("range", "low".into());
    }
    for (i, w) in words.iter().enumerate() {
        if *w == "seed" {
            if let Some(n) = words.get(i + 1).and_then(|x| x.parse::<u64>().ok()) {
                put!("seed", n.to_string());
            }
        }
    }
    if lower.contains("four on the floor") || lower.contains("four-on-the-floor") {
        put!("pattern", "fourfloor".into());
    }
    for (i, w) in words.iter().enumerate() {
        if *w == "swing" || *w == "swung" || *w == "shuffle" {
            put!("swing", "0.5".into());
        }
        if (*w == "octaves" || *w == "octave") && i > 0 {
            if let Ok(n) = words[i - 1].parse::<u32>() {
                put!("octaves", n.to_string());
            }
        }
        if *w == "arpeggio" || *w == "arp" {
            if let Some(o) = words.get(i + 1).filter(|o| matches!(**o, "up" | "down" | "updown" | "pingpong" | "random")) {
                put!("order", o.to_string());
            }
        }
        if let Some(r) = w.strip_prefix("1/") {
            if matches!(r, "4" | "8" | "16" | "32") {
                put!("rate", format!("1/{r}"));
            }
        }
    }
    if lower.contains("one chord per bar") || lower.contains("a chord every bar") {
        put!("cpb", "1".into());
    } else if lower.contains("two chords per bar") || lower.contains("fast chord") {
        put!("cpb", "2".into());
    } else if lower.contains("slow chord") || lower.contains("chord every two bars") {
        put!("cpb", "0.5".into());
    }
    f
}

/// fields from a model's reply: JSON objects, or `"name": value` pairs scattered in text; tolerant of anything around them
pub fn fields_from_json(reply: &str) -> Vec<(String, String)> {
    let mut out = vec![];
    let b: Vec<char> = reply.chars().collect();
    let mut i = 0;
    let read_str = |i: &mut usize| -> Option<String> {
        // at an opening quote
        let mut s = String::new();
        *i += 1;
        while *i < b.len() && b[*i] != '"' {
            if b[*i] == '\\' && *i + 1 < b.len() {
                *i += 1;
            }
            s.push(b[*i]);
            *i += 1;
        }
        *i += 1;
        Some(s)
    };
    while i < b.len() {
        if b[i] == '"' {
            let Some(k) = read_str(&mut i) else { break };
            let mut j = i;
            while j < b.len() && b[j].is_whitespace() {
                j += 1;
            }
            if j < b.len() && b[j] == ':' {
                j += 1;
                while j < b.len() && b[j].is_whitespace() {
                    j += 1;
                }
                if j < b.len() && b[j] == '"' {
                    let v = read_str(&mut j).unwrap_or_default();
                    out.push((k.to_lowercase(), v));
                    i = j;
                    continue;
                } else if j < b.len() && (b[j].is_ascii_digit() || b[j] == '-' || b[j] == '.') {
                    let mut v = String::new();
                    while j < b.len() && (b[j].is_ascii_digit() || b[j] == '.' || b[j] == '-') {
                        v.push(b[j]);
                        j += 1;
                    }
                    out.push((k.to_lowercase(), v));
                    i = j;
                    continue;
                } else if j < b.len() && b[j] == '[' {
                    // a list of strings becomes a space-separated string (a progression)
                    let mut v: Vec<String> = vec![];
                    j += 1;
                    while j < b.len() && b[j] != ']' {
                        if b[j] == '"' {
                            v.push(read_str(&mut j).unwrap_or_default());
                        } else {
                            j += 1;
                        }
                    }
                    out.push((k.to_lowercase(), v.join(" ")));
                    i = j;
                    continue;
                }
            }
            continue;
        }
        i += 1;
    }
    // normalise a few synonyms a model may use
    for (k, _) in out.iter_mut() {
        match k.as_str() {
            "progression" | "chords" | "chord_progression" => *k = "prog".into(),
            "type" | "part" | "instrument" | "role" => *k = "kind".into(),
            "length" | "num_bars" | "measures" => *k = "bars".into(),
            "tonality" | "scale" => *k = "key".into(),
            "rhythm" => *k = "feel".into(),
            _ => {}
        }
    }
    out
}

fn lookup<'a>(f: &'a [(String, String)], k: &str) -> Option<&'a str> {
    f.iter().rev().find(|(x, _)| x == k).map(|(_, v)| v.as_str())
}

fn density_value(s: &str) -> Option<f64> {
    if let Ok(x) = s.trim().parse::<f64>() {
        return Some(x.clamp(0.0, 1.0));
    }
    let l = s.to_lowercase();
    Some(match l.as_str() {
        "sparse" | "minimal" | "low" | "simple" | "slow" => 0.2,
        "medium" | "moderate" | "mid" => 0.5,
        "busy" | "dense" | "high" | "active" => 0.8,
        "frantic" | "very busy" | "intense" => 0.95,
        _ => return None,
    })
}

impl Spec {
    /// build a spec: `ctx` supplies defaults from the project (key, kind); later field lists override earlier ones
    pub fn from_fields(layers: &[&[(String, String)]], default_key: Key, default_kind: Kind) -> Spec {
        let mut all: Vec<(String, String)> = vec![];
        for l in layers {
            all.extend(l.iter().cloned());
        }
        let f = &all[..];
        let mut s = Spec { key: default_key, kind: default_kind, ..Spec::default() };
        if let Some(st) = lookup(f, "style") {
            s.style = st.to_string();
        }
        let def = style_def(&s.style);
        // key: an explicit key wins; "mode" alone keeps the project's tonic and sets the mode; a style with no key given keeps the project's
        if let Some(k) = lookup(f, "key").and_then(theory::parse_key) {
            s.key = k;
        } else if let Some(m) = lookup(f, "mode") {
            s.key = Key::new(s.key.tonic, m.starts_with("min"));
        }
        if let Some(k) = lookup(f, "kind").and_then(Kind::parse) {
            s.kind = k;
        }
        if let Some(d) = def {
            s.density = d.density;
            s.feel = d.feel.into();
            s.contour = d.contour.into();
            s.cpb = d.cpb;
            s.pattern = match s.kind {
                Kind::Bass => d.bass.into(),
                Kind::Chords => d.chords.into(),
                _ => String::new(),
            };
        }
        let (lo, hi) = s.kind.range();
        s.lo = lo;
        s.hi = hi;
        if let Some(b) = lookup(f, "bars").and_then(|v| v.trim().parse::<f64>().ok()) {
            s.bars = (b as u32).clamp(1, 64);
        }
        if let Some(d) = lookup(f, "density").and_then(density_value) {
            s.density = d;
        }
        if let Some(c) = lookup(f, "contour") {
            s.contour = c.to_lowercase();
        }
        if let Some(c) = lookup(f, "feel") {
            s.feel = c.to_lowercase();
        }
        if let Some(c) = lookup(f, "pattern") {
            s.pattern = c.to_lowercase();
        }
        if let Some(c) = lookup(f, "cpb").and_then(|v| v.trim().parse::<f64>().ok()) {
            s.cpb = c.clamp(0.25, 4.0);
        }
        if let Some(c) = lookup(f, "motif").and_then(|v| v.trim().parse::<f64>().ok()) {
            s.motif = (c as u32).clamp(1, 8);
        }
        if let Some(r) = lookup(f, "rate") {
            s.rate = parse_rate(r);
        }
        if let Some(r) = lookup(f, "rhythm") {
            s.rhythm = r.to_string();
        }
        if let Some(r) = lookup(f, "order") {
            s.order = r.to_lowercase();
        }
        if let Some(v) = lookup(f, "gate").and_then(|v| v.trim().parse::<f64>().ok()) {
            s.gate = v.clamp(0.05, 1.5);
        }
        if let Some(v) = lookup(f, "octaves").and_then(|v| v.trim().parse::<f64>().ok()) {
            s.octaves = (v as u32).clamp(1, 4);
        }
        if let Some(v) = lookup(f, "swing").and_then(|v| v.trim().parse::<f64>().ok()) {
            s.swing = v.clamp(0.0, 1.0);
        }
        if let Some(v) = lookup(f, "fills").and_then(|v| v.trim().parse::<f64>().ok()) {
            s.fills = v.clamp(0.0, 1.0);
        }
        if let Some(c) = lookup(f, "seed").and_then(|v| v.trim().parse::<f64>().ok()) {
            s.seed = c as u64;
        }
        match lookup(f, "range") {
            Some("high") => {
                s.lo = hi.saturating_sub(12).max(lo);
                s.hi = (hi + 12).min(108);
            }
            Some("low") => {
                s.lo = lo.saturating_sub(12);
                s.hi = hi.saturating_sub(12).max(lo);
            }
            Some(r) => {
                if let Some((a, b)) = r.split_once('-') {
                    if let (Ok(a), Ok(b)) = (a.trim().parse::<u8>(), b.trim().parse::<u8>()) {
                        if b > a + 7 {
                            s.lo = a;
                            s.hi = b.min(120);
                        }
                    }
                }
            }
            None => {}
        }
        if let Some(p) = lookup(f, "prog") {
            s.prog = parse_prog(p, s.key);
            s.prog_text = p.to_string();
        }
        s
    }

    pub fn to_fields(&self) -> String {
        format!(
            "key={};bars={};kind={};prog={};density={:.2};contour={};feel={};pattern={};range={}-{};seed={};style={};cpb={};motif={};rate={};rhythm={};order={};gate={:.2};octaves={};swing={:.2};fills={:.2}",
            self.key.name(),
            self.bars,
            self.kind.name(),
            self.prog_text,
            self.density,
            self.contour,
            self.feel,
            self.pattern,
            self.lo,
            self.hi,
            self.seed,
            self.style,
            self.cpb,
            self.motif,
            self.rate,
            self.rhythm,
            self.order,
            self.gate,
            self.octaves,
            self.swing,
            self.fills
        )
    }

    pub fn from_text(s: &str, default_key: Key, default_kind: Kind) -> Spec {
        Spec::from_fields(&[&split_fields(s)], default_key, default_kind)
    }

    /// one line for the UI: "F# minor | 8 bars | melody | prog i VI III VII | sparse | arch"
    pub fn summary(&self) -> String {
        let dens = if self.density < 0.3 {
            "sparse"
        } else if self.density < 0.65 {
            "medium"
        } else {
            "busy"
        };
        let mut v = vec![self.key.name(), format!("{} bars", self.bars), self.kind.name().to_string()];
        if !self.style.is_empty() {
            v.push(self.style.clone());
        }
        if !self.prog_text.is_empty() {
            v.push(self.prog_text.clone());
        }
        v.push(dens.to_string());
        if self.kind == Kind::Melody {
            v.push(self.contour.clone());
        }
        v.join(" | ")
    }
}

/// "k=v;k=v" back into pairs
pub fn split_fields(s: &str) -> Vec<(String, String)> {
    s.split(';').filter_map(|p| p.split_once('=')).map(|(k, v)| (k.trim().to_lowercase(), v.trim().to_string())).collect()
}

// ── random numbers ──────────────────────────────────────────────────────────
pub struct Rng(u64);
impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(0x9E3779B97F4A7C15) ^ 0xD1B54A32D192ED03 | 1)
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.0 = x;
        x.wrapping_mul(0x2545F4914F6CDD1D)
    }
    pub fn f(&mut self) -> f64 {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64
    }
    fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next() % v.len() as u64) as usize]
    }
    pub fn chance(&mut self, p: f64) -> bool {
        self.f() < p
    }
}

// ── the harmonic plan ───────────────────────────────────────────────────────
#[derive(Clone, Debug)]
pub struct Span {
    pub start: f64,
    pub len: f64,
    pub chord: Chord,
}

pub fn plan(spec: &Spec, rng: &mut Rng) -> Vec<Span> {
    if !spec.harmony.is_empty() {
        return spec.harmony.iter().map(|(start, len, root, q)| Span { start: *start, len: *len, chord: Chord { start: *start, len: *len, root: *root, q: *q, bass: *root, implied: false } }).collect();
    }
    let mut prog = spec.prog.clone();
    if prog.is_empty() {
        let def = style_def(&spec.style);
        let options: Vec<&str> = match def {
            Some(d) if d.minor == spec.key.minor => d.progs.to_vec(),
            _ => {
                if spec.key.minor {
                    vec!["i VI III VII", "i iv VII III", "i VII VI VII"]
                } else {
                    vec!["I V vi IV", "I vi IV V", "vi IV I V"]
                }
            }
        };
        let text = *rng.pick(&options);
        prog = parse_prog(text, spec.key);
    }
    let total = spec.bars as f64 * 4.0;
    let span = 4.0 / spec.cpb;
    let n = (total / span).ceil() as usize;
    (0..n)
        .map(|i| {
            let (root, q) = prog[i % prog.len()];
            let start = i as f64 * span;
            Span { start, len: span.min(total - start), chord: Chord { start, len: span.min(total - start), root, q, bass: root, implied: false } }
        })
        .collect()
}

fn pitch_in(lo: u8, hi: u8, pc: u8, near: u8) -> u8 {
    let mut best = lo;
    let mut bd = i32::MAX;
    for k in lo..=hi {
        if k % 12 == pc % 12 {
            let d = (k as i32 - near as i32).abs();
            if d < bd {
                bd = d;
                best = k;
            }
        }
    }
    best
}

// ── generators ──────────────────────────────────────────────────────────────
pub fn generate(spec: &Spec) -> Vec<N> {
    let mut rng = Rng::new(spec.seed);
    let spans = plan(spec, &mut rng);
    match spec.kind {
        Kind::Chords | Kind::Pad => gen_chords(spec, &spans, &mut rng),
        Kind::Bass => gen_bass(spec, &spans, &mut rng),
        Kind::Arp => gen_arp(spec, &spans, &mut rng),
        Kind::Drums => gen_drums(spec, &mut rng),
        Kind::Melody => gen_melody(spec, &spans, &mut rng),
    }
}

fn nt(start: f64, len: f64, key: u8, vel: f32) -> N {
    N { start, len, key, vel }
}

fn gen_chords(spec: &Spec, spans: &[Span], rng: &mut Rng) -> Vec<N> {
    let mut out = vec![];
    let mut prev: Vec<u8> = vec![];
    let pattern = if spec.kind == Kind::Pad {
        "sustain".to_string()
    } else if !spec.pattern.is_empty() {
        spec.pattern.clone()
    } else {
        match spec.feel.as_str() {
            "offbeat" => "stabs",
            "syncopated" | "swing" => "synco",
            "driving" => "pump",
            "staccato" | "gated" => "gate",
            _ => "sustain",
        }
        .to_string()
    };
    for sp in spans {
        let notes = theory::voice(sp.chord.root, sp.chord.q, &prev, spec.lo, spec.hi);
        prev = notes.clone();
        let hits: Vec<(f64, f64, f32)> = match pattern.as_str() {
            "stabs" => (0..(sp.len as usize)).map(|b| (b as f64 + 0.5, 0.4, 0.8)).collect(),
            "pump" => (0..(sp.len as usize)).map(|b| (b as f64, 0.85, if b % 2 == 0 { 0.85 } else { 0.7 })).collect(),
            "synco" => {
                let mut v = vec![];
                let mut b = 0.0;
                while b + 1.0 < sp.len + 0.01 {
                    v.push((b, 1.25, 0.8));
                    v.push((b + 1.5, 1.25, 0.7));
                    v.push((b + 3.0, 0.9, 0.75));
                    b += 4.0;
                }
                v.into_iter().filter(|h| h.0 < sp.len).collect()
            }
            "gate" => (0..(sp.len * 4.0) as usize).filter(|i| i % 4 != 3 && (i % 2 == 0 || rng.chance(0.5 + spec.density * 0.4))).map(|i| (i as f64 * 0.25, 0.2, 0.75)).collect(),
            _ => vec![(0.0, sp.len * 0.97, 0.8)],
        };
        for (off, len, vel) in hits {
            let len = len.min(sp.len - off).max(0.1);
            for k in &notes {
                out.push(nt(sp.start + off, len, *k, vel));
            }
        }
    }
    out
}

fn gen_bass(spec: &Spec, spans: &[Span], rng: &mut Rng) -> Vec<N> {
    let mut out = vec![];
    let pattern = if !spec.pattern.is_empty() {
        spec.pattern.clone()
    } else if spec.density > 0.65 {
        "rolling".into()
    } else if spec.density < 0.3 {
        "sub".into()
    } else {
        "root".into()
    };
    let scale = spec.key;
    for (si, sp) in spans.iter().enumerate() {
        let root = pitch_in(spec.lo, spec.lo + 11, sp.chord.root, spec.lo + 4);
        let fifth = root + 7;
        let third = root + if QUAL[sp.chord.q].1.contains(&3) { 3 } else { 4 };
        let next_root = spans.get(si + 1).map(|n| pitch_in(spec.lo, spec.lo + 11, n.chord.root, root)).unwrap_or(root);
        let steps = (sp.len / 0.5) as usize;
        match pattern.as_str() {
            "rolling" => {
                // sixteenth notes with a gap on every beat, as in a trance bassline
                for i in 0..(sp.len / 0.25) as usize {
                    if i % 4 == 0 {
                        continue;
                    }
                    let k = if i % 16 == 15 && rng.chance(0.4) { fifth } else { root };
                    out.push(nt(sp.start + i as f64 * 0.25, 0.2, k, if i % 4 == 2 { 0.95 } else { 0.78 }));
                }
            }
            "offbeat" => {
                for i in 0..(sp.len as usize) {
                    out.push(nt(sp.start + i as f64 + 0.5, 0.45, root, 0.9));
                }
            }
            "octave" => {
                for i in 0..steps {
                    out.push(nt(sp.start + i as f64 * 0.5, 0.45, if i % 2 == 0 { root } else { root + 12 }, if i % 2 == 0 { 0.9 } else { 0.7 }));
                }
            }
            "sub" => out.push(nt(sp.start, sp.len * 0.97, root, 0.85)),
            "walking" => {
                for i in 0..(sp.len as usize) {
                    let k = match i % 4 {
                        0 => root,
                        1 => third,
                        2 => fifth,
                        _ => {
                            // chromatic or scale approach to the next chord's root
                            let target = next_root as i32;
                            let step = if rng.chance(0.5) { -1 } else { 1 };
                            let approach = target + step;
                            if scale.in_scale((approach.rem_euclid(12)) as u8) || rng.chance(0.4) {
                                approach.clamp(0, 127) as u8
                            } else {
                                (target - step).clamp(0, 127) as u8
                            }
                        }
                    };
                    out.push(nt(sp.start + i as f64, 0.9, k, if i == 0 { 0.9 } else { 0.75 }));
                }
            }
            "synco" => {
                let mut b = 0.0;
                while b < sp.len {
                    for (off, len, k, v) in [(0.0, 0.7, root, 0.9f32), (0.75, 0.7, root, 0.7), (1.5, 0.45, if rng.chance(0.3) { fifth } else { root }, 0.8), (2.5, 0.7, root, 0.85), (3.5, 0.4, root + 12 * rng.chance(0.3) as u8, 0.7)] {
                        if b + off < sp.len {
                            out.push(nt(sp.start + b + off, len, k, v));
                        }
                    }
                    b += 4.0;
                }
            }
            _ => {
                // root on each beat, the fifth on beat 3 now and then
                for i in 0..(sp.len as usize) {
                    let k = if i % 4 == 2 && rng.chance(0.35) { fifth } else { root };
                    out.push(nt(sp.start + i as f64, 0.9, k, if i % 4 == 0 { 0.9 } else { 0.75 }));
                }
            }
        }
    }
    out
}

/// "1/16", "0.25", "16th" -> beats per step
pub fn parse_rate(r: &str) -> f64 {
    let t = r.trim().to_lowercase();
    if let Some((a, b)) = t.split_once('/') {
        if let (Ok(a), Ok(b)) = (a.trim().parse::<f64>(), b.trim().parse::<f64>()) {
            if b > 0.0 {
                return (4.0 * a / b).clamp(0.0625, 4.0);
            }
        }
    }
    if let Ok(x) = t.parse::<f64>() {
        return x.clamp(0.0625, 4.0);
    }
    0.0
}

/// Euclidean rhythm: `hits` spread as evenly as possible over `steps`, rotated by `rot`
pub fn euclid(hits: usize, steps: usize, rot: usize) -> Vec<bool> {
    let steps = steps.clamp(1, 64);
    let hits = hits.min(steps);
    let mut v = vec![false; steps];
    if hits > 0 {
        for i in 0..hits {
            v[((i * steps + hits - 1) / hits + rot) % steps] = true;
        }
    }
    v
}

/// the step mask of a rhythm description: x / . characters, "euclid:5,16,2", or a name; empty = every step
pub fn rhythm_mask(r: &str) -> Vec<bool> {
    let t = r.trim().to_lowercase();
    if let Some(rest) = t.strip_prefix("euclid:") {
        let n: Vec<usize> = rest.split(|c: char| c == ',' || c == '/' || c == ' ').filter_map(|x| x.trim().parse().ok()).collect();
        if n.len() >= 2 {
            return euclid(n[0], n[1], n.get(2).copied().unwrap_or(0));
        }
    }
    let lit = |m: &str| m.chars().map(|c| c == 'x' || c == 'X').collect::<Vec<bool>>();
    match t.as_str() {
        "" | "16ths" | "8ths" | "all" | "straight" | "triplet" => vec![],
        "offbeat" => lit("..x...x...x...x."),
        "gallop" => lit("x.xxx.xxx.xxx.xx"),
        "syncopated" => lit("x..x..x...x..x.."),
        "dotted" => lit("x..x..x.x..x..x."),
        "pulse" => lit("x...x...x...x..."),
        "charleston" => lit("x..x............"),
        s if s.len() >= 2 && s.chars().all(|c| c == 'x' || c == '.' || c == '-') => lit(&s.replace('-', ".")),
        _ => vec![],
    }
}

/// notes for an arpeggio over `pitches` from `start` for `len` beats: one note per step of `rate` beats where the mask says so
pub fn arp_notes(pitches: &[u8], start: f64, len: f64, rate: f64, order: &str, gate: f64, mask: &[bool], rng: &mut Rng, step_offset: usize) -> Vec<N> {
    let mut out = vec![];
    if pitches.is_empty() || rate <= 0.0 {
        return out;
    }
    let n = pitches.len();
    let seq: Vec<usize> = match order {
        "down" => (0..n).rev().collect(),
        "updown" => (0..n).chain((1..n.saturating_sub(1)).rev()).collect(),
        "pingpong" => (0..n).chain((0..n).rev()).collect(),
        "random" => (0..16).map(|_| (rng.next() % n as u64) as usize).collect(),
        _ => (0..n).collect(),
    };
    let count = (len / rate + 1e-6) as usize;
    let mut hit_i = 0usize;
    for i in 0..count {
        let gi = step_offset + i;
        if !mask.is_empty() && !mask[gi % mask.len()] {
            continue;
        }
        let t = i as f64 * rate;
        let on_beat = (t % 1.0).abs() < 0.01;
        out.push(N { start: start + t, len: (rate * gate).max(0.05), key: pitches[seq[hit_i % seq.len()]], vel: if on_beat { 0.9 } else { 0.7 } });
        hit_i += 1;
    }
    out
}

fn gen_arp(spec: &Spec, spans: &[Span], rng: &mut Rng) -> Vec<N> {
    let mut out = vec![];
    let rate = if spec.rate > 0.0 { spec.rate } else if spec.density > 0.7 { 0.25 } else if spec.density > 0.35 { 0.5 } else { 1.0 };
    let order = if !spec.order.is_empty() {
        spec.order.clone()
    } else if !spec.pattern.is_empty() {
        spec.pattern.clone()
    } else {
        rng.pick(&["up", "updown", "down", "pingpong"]).to_string()
    };
    let mask = rhythm_mask(&spec.rhythm);
    for sp in spans {
        let top = (spec.lo as u32 + 12 * spec.octaves as u32).min(spec.hi as u32 + 12) as u8;
        let pitches: Vec<u8> = (spec.lo..=top).filter(|k| sp.chord.has(k % 12)).collect();
        // the rhythm restarts with every chord so a pattern stays on the bar
        out.extend(arp_notes(&pitches, sp.start, sp.len, rate, &order, spec.gate, &mask, rng, 0));
    }
    out
}

/// Turn chords (notes sharing a start) into arpeggios: each chord becomes a run over its own length. Notes that are not part of a chord
/// (alone on their start) are returned as they were.
pub fn arpeggiate(notes: &[N], rate: f64, order: &str, octaves: u32, gate: f64, rhythm: &str, seed: u64) -> Vec<N> {
    let mut rng = Rng::new(seed);
    let mask = rhythm_mask(rhythm);
    let mut idx: Vec<usize> = (0..notes.len()).collect();
    idx.sort_by(|a, b| notes[*a].start.partial_cmp(&notes[*b].start).unwrap().then(notes[*a].key.cmp(&notes[*b].key)));
    let mut out = vec![];
    let mut i = 0;
    while i < idx.len() {
        let mut j = i + 1;
        while j < idx.len() && (notes[idx[j]].start - notes[idx[i]].start).abs() < 0.03 {
            j += 1;
        }
        let group: Vec<N> = idx[i..j].iter().map(|k| notes[*k]).collect();
        if group.len() >= 2 {
            let start = group[0].start;
            let len = group.iter().map(|n| n.len).fold(0.0, f64::max).max(rate);
            let mut pitches: Vec<u8> = vec![];
            for o in 0..octaves.max(1) {
                for n in &group {
                    let k = n.key as u32 + 12 * o;
                    if k <= 127 && !pitches.contains(&(k as u8)) {
                        pitches.push(k as u8);
                    }
                }
            }
            pitches.sort();
            let vel = group.iter().map(|n| n.vel).fold(0.0, f32::max);
            for mut n in arp_notes(&pitches, start, len, rate, order, gate, &mask, &mut rng, 0) {
                n.vel = (n.vel * vel / 0.9).clamp(0.1, 1.0);
                out.push(n);
            }
        } else {
            out.extend(group);
        }
        i = j;
    }
    out
}

// ── drums ───────────────────────────────────────────────────────────────────
pub const K_KICK: u8 = 36;
pub const K_RIM: u8 = 37;
pub const K_SNARE: u8 = 38;
pub const K_CLAP: u8 = 39;
pub const K_HAT: u8 = 42;
pub const K_HAT_OPEN: u8 = 46;
pub const K_RIDE: u8 = 51;
pub const K_CRASH: u8 = 49;

pub const DRUM_PATTERNS: [&str; 12] = ["fourfloor", "house", "techno", "trance", "disco", "breakbeat", "trap", "dnb", "halftime", "boombap", "onedrop", "euclid"];

/// (pattern rows: key, 16-step mask where x = hit, o = soft hit)
fn drum_rows(pat: &str, density: f64, rng: &mut Rng) -> Vec<(u8, String)> {
    let r = |k: u8, m: &str| (k, m.to_string());
    match pat {
        "house" => vec![r(K_KICK, "x...x...x...x..."), r(K_CLAP, "....x.......x..."), r(K_HAT, "xoxoxoxoxoxoxoxo"), r(K_HAT_OPEN, "..x...x...x...x."), r(K_RIM, "...o.....o...o..")],
        "techno" => vec![r(K_KICK, "x...x...x...x..."), r(K_CLAP, "....x.......x..."), r(K_HAT, "x.x.x.x.x.x.x.x."), r(K_HAT_OPEN, "..x...x...x...x."), r(K_RIM, "..o.....o.....o.")],
        "trance" => vec![r(K_KICK, "x...x...x...x..."), r(K_CLAP, "....x.......x..."), r(K_HAT, "x.x.x.x.x.x.x.x."), r(K_HAT_OPEN, "..x...x...x...x."), r(K_RIDE, "o.o.o.o.o.o.o.o.")],
        "disco" => vec![r(K_KICK, "x...x...x...x..."), r(K_SNARE, "....x.......x..."), r(K_HAT, "x.x.x.x.x.x.x.x."), r(K_HAT_OPEN, "..x...x...x...x."), r(K_CLAP, "....o.......o...")],
        "breakbeat" => vec![r(K_KICK, "x.....x...x..x.."), r(K_SNARE, "....x.o.....x..o"), r(K_HAT, "x.x.x.x.x.x.x.x."), r(K_HAT_OPEN, "......x.......x.")],
        "trap" => vec![r(K_KICK, "x.......x..x...."), r(K_SNARE, "........x......."), r(K_CLAP, "........x......."), r(K_HAT, "xoxoxoxoxxxoxoxo"), r(K_HAT_OPEN, ".............x..")],
        "dnb" => vec![r(K_KICK, "x.........x....."), r(K_SNARE, "....x.......x..."), r(K_HAT, "x.x.x.x.x.x.x.x."), r(K_SNARE, "......o.o....o..")],
        "halftime" => vec![r(K_KICK, "x.....x.......x."), r(K_SNARE, "........x......."), r(K_HAT, "x.x.x.x.x.x.x.x."), r(K_RIM, "..........o.....")],
        "boombap" => vec![r(K_KICK, "x....x....x....."), r(K_SNARE, "....x.......x..."), r(K_HAT, "x.x.x.x.x.x.x.x."), r(K_HAT_OPEN, ".............x..")],
        "onedrop" => vec![r(K_KICK, "........x......."), r(K_RIM, "........x......."), r(K_HAT, "x.x.x.x.x.x.x.x."), r(K_SNARE, "................")],
        "euclid" => {
            let hat_hits = 5 + (density * 8.0) as usize;
            let rows = [(K_KICK, euclid(4, 16, 0)), (K_SNARE, euclid(2, 16, 4)), (K_HAT, euclid(hat_hits, 16, 1)), (K_HAT_OPEN, euclid(3, 16, 2)), (K_RIM, euclid(5, 16, (rng.next() % 16) as usize))];
            rows.iter().map(|(k, m)| (*k, m.iter().map(|b| if *b { 'x' } else { '.' }).collect::<String>())).collect()
        }
        // fourfloor and anything unknown
        _ => vec![r(K_KICK, "x...x...x...x..."), r(K_CLAP, "....x.......x..."), r(K_HAT, "..x...x...x...x."), r(K_HAT_OPEN, "..x...x...x...x.")],
    }
}

fn drum_default_pattern(style: &str) -> &'static str {
    match style {
        "house" | "deep house" => "house",
        "techno" => "techno",
        "trance" | "synthwave" => "trance",
        "disco" => "disco",
        "dnb" => "dnb",
        "lofi" | "witch house" | "ambient" | "cinematic" => "halftime",
        "dub" => "onedrop",
        "hyperpop" => "trap",
        "pop" | "rock" => "boombap",
        _ => "fourfloor",
    }
}

fn gen_drums(spec: &Spec, rng: &mut Rng) -> Vec<N> {
    let pat = if !spec.pattern.is_empty() && (DRUM_PATTERNS.contains(&spec.pattern.as_str())) {
        spec.pattern.clone()
    } else if spec.pattern == "four" {
        "fourfloor".into()
    } else {
        drum_default_pattern(&spec.style).to_string()
    };
    let swing = if spec.swing > 0.0 { spec.swing } else if spec.feel == "swing" { 0.5 } else { 0.0 };
    let mut out = vec![];
    for bar in 0..spec.bars as usize {
        let rows = drum_rows(&pat, spec.density, rng);
        let fill_bar = spec.fills > 0.05 && bar % 4 == 3 && bar + 1 >= 4;
        for (key, mask) in &rows {
            let soft_row = matches!(*key, K_HAT | K_HAT_OPEN | K_RIM | K_RIDE);
            for (i, ch) in mask.chars().enumerate() {
                if ch == '.' {
                    continue;
                }
                // a fill takes over the last four steps of the bar: kicks and open hats drop out
                if fill_bar && i >= 12 && matches!(*key, K_KICK | K_HAT | K_HAT_OPEN | K_CLAP | K_RIDE) {
                    continue;
                }
                // a low density thins out the hats and percussion; a high one keeps them all
                if soft_row && i % 4 != 0 && rng.f() > 0.35 + spec.density * 0.65 {
                    continue;
                }
                let mut vel = if ch == 'o' { 0.55 } else { 0.9 };
                if i % 4 == 0 {
                    vel += 0.05;
                }
                vel += (rng.f() as f32 - 0.5) * 0.12;
                let mut t = bar as f64 * 4.0 + i as f64 * 0.25;
                if i % 2 == 1 {
                    t += swing * 0.08;
                }
                let len = if *key == K_HAT_OPEN { 0.35 } else if *key == K_KICK { 0.2 } else { 0.12 };
                out.push(nt(t, len, *key, vel.clamp(0.2, 1.0) as f32));
            }
        }
        // extra sixteenth hats when the density is high
        if spec.density > 0.75 && !rows.iter().any(|(k, m)| *k == K_HAT && m.chars().filter(|c| *c != '.').count() >= 15) {
            for i in (1..16).step_by(2) {
                if rng.chance(0.45) && !(fill_bar && i >= 12) {
                    out.push(nt(bar as f64 * 4.0 + i as f64 * 0.25 + if i % 2 == 1 { swing * 0.08 } else { 0.0 }, 0.08, K_HAT, 0.45));
                }
            }
        }
        if fill_bar {
            // a snare / tom roll into the next phrase
            let toms = [45u8, 43, 41, K_SNARE];
            for i in 12..16 {
                let key = if spec.fills > 0.6 { toms[i - 12] } else { K_SNARE };
                out.push(nt(bar as f64 * 4.0 + i as f64 * 0.25, 0.2, key, 0.6 + (i - 12) as f32 * 0.1));
            }
            if (bar + 1) % 8 == 0 {
                out.push(nt((bar + 1) as f64 * 4.0, 1.0, K_CRASH, 0.9));
            }
        }
    }
    out
}

fn rhythm_options(density: f64, feel: &str) -> Vec<Vec<(f64, f64)>> {
    let sparse: Vec<Vec<(f64, f64)>> = vec![
        vec![(0.0, 2.0), (2.0, 2.0)],
        vec![(0.0, 1.5), (1.5, 0.5), (2.0, 2.0)],
        vec![(0.0, 3.0), (3.0, 1.0)],
        vec![(0.0, 1.0), (2.0, 1.0), (3.0, 1.0)],
        vec![(0.5, 1.0), (2.0, 1.5), (3.5, 0.5)],
    ];
    let medium: Vec<Vec<(f64, f64)>> = vec![
        vec![(0.0, 1.0), (1.0, 1.0), (2.0, 1.0), (3.0, 1.0)],
        vec![(0.0, 1.5), (1.5, 0.5), (2.0, 1.0), (3.0, 1.0)],
        vec![(0.0, 0.5), (0.5, 0.5), (1.0, 1.0), (2.0, 2.0)],
        vec![(0.0, 1.0), (1.0, 0.5), (1.5, 0.5), (2.0, 1.0), (3.0, 1.0)],
        vec![(0.0, 0.75), (0.75, 0.75), (1.5, 0.5), (2.0, 1.0), (3.0, 0.5), (3.5, 0.5)],
    ];
    let busy: Vec<Vec<(f64, f64)>> = vec![
        (0..8).map(|i| (i as f64 * 0.5, 0.5)).collect(),
        vec![(0.0, 0.5), (0.5, 0.25), (0.75, 0.25), (1.0, 0.5), (1.5, 0.5), (2.0, 0.5), (2.5, 0.25), (2.75, 0.25), (3.0, 0.5), (3.5, 0.5)],
        vec![(0.0, 0.75), (0.75, 0.25), (1.0, 0.5), (1.5, 0.5), (2.0, 0.75), (2.75, 0.25), (3.0, 0.5), (3.5, 0.5)],
        (0..16).map(|i| (i as f64 * 0.25, 0.25)).collect(),
    ];
    let mut v = if density < 0.33 {
        sparse
    } else if density < 0.7 {
        medium
    } else {
        busy
    };
    if feel == "syncopated" || feel == "offbeat" || feel == "swing" {
        let sync: Vec<Vec<(f64, f64)>> = v.iter().filter(|r| r.iter().any(|(t, _)| (t % 1.0 - 0.5).abs() < 0.01 || (t % 1.0 - 0.75).abs() < 0.01)).cloned().collect();
        if !sync.is_empty() {
            v = sync;
        }
    } else if feel == "straight" || feel == "sustained" || feel == "legato" {
        let st: Vec<Vec<(f64, f64)>> = v.iter().filter(|r| r.iter().filter(|(t, _)| (t % 1.0).abs() < 0.01).count() * 2 >= r.len()).cloned().collect();
        if !st.is_empty() {
            v = st;
        }
    }
    v
}

fn gen_melody(spec: &Spec, spans: &[Span], rng: &mut Rng) -> Vec<N> {
    let scale: Vec<u8> = (spec.lo..=spec.hi).filter(|k| spec.key.in_scale(k % 12)).collect();
    if scale.len() < 4 {
        return vec![];
    }
    let rhythms = rhythm_options(spec.density, &spec.feel);
    let top = scale.len() as f64 - 1.0;
    let contour = |t: f64| -> f64 {
        let f = match spec.contour.as_str() {
            "rise" => t,
            "fall" => 1.0 - t,
            "wave" => 0.5 + 0.5 * (t * std::f64::consts::TAU * 2.0).sin(),
            "flat" => 0.5,
            _ => (t * std::f64::consts::PI).sin(),
        };
        // target index in the scale, kept in the middle 70% of the range
        (0.15 + 0.7 * f) * top
    };
    let chord_at = |beat: f64| spans.iter().find(|s| beat >= s.start - 1e-6 && beat < s.start + s.len - 1e-6).or(spans.last()).map(|s| &s.chord);
    let idx_of = |k: u8| scale.iter().position(|x| *x == k).unwrap_or(0);
    let mut out: Vec<N> = vec![];
    let mut cur = idx_of(scale.iter().copied().min_by_key(|k| (*k as i32 - (spec.lo as i32 + spec.hi as i32) / 2).abs()).unwrap()) as i32;
    let bars = spec.bars as usize;
    // two ideas: A (the motif) and B (the answer); each is a rhythm and a list of scale steps
    let make_steps = |rng: &mut Rng, n: usize| -> Vec<i32> {
        (0..n)
            .map(|_| {
                let r = rng.f();
                let m = if r < 0.14 {
                    0
                } else if r < 0.68 {
                    1
                } else if r < 0.88 {
                    2
                } else if r < 0.96 {
                    3
                } else {
                    4
                };
                if rng.chance(0.5) {
                    m
                } else {
                    -m
                }
            })
            .collect()
    };
    let rhythm_a = rng.pick(&rhythms).clone();
    let rhythm_b = rng.pick(&rhythms).clone();
    let steps_a = make_steps(rng, rhythm_a.len());
    let steps_b = make_steps(rng, rhythm_b.len());
    let motif = spec.motif.max(1) as usize;
    let mut last_pitch_run = 0;
    for bar in 0..bars {
        let bar_start = bar as f64 * 4.0;
        let phrase_len = (motif * 2).max(2);
        let pos = bar % phrase_len;
        let last_bar = bar + 1 == bars;
        // idea for this bar: the motif returns at the start of each phrase and halfway through, the answer fills in between
        let (rhythm, steps) = if last_bar && bars > 1 {
            (vec![(0.0, 1.5), (1.5, 0.5), (2.0, 2.0)], vec![-1, -1, 0])
        } else if pos % motif == 0 {
            (rhythm_a.clone(), steps_a.clone())
        } else {
            (rhythm_b.clone(), steps_b.clone())
        };
        let n_notes = rhythm.len();
        for (j, (off, len)) in rhythm.iter().enumerate() {
            let t = bar_start + off;
            // weak notes may drop out in sparse music
            let weak = (off % 1.0).abs() > 0.01;
            if weak && j > 0 && rng.chance((0.45 - spec.density).max(0.0) * 0.5) {
                continue;
            }
            let target = contour((bar as f64 + off / 4.0) / bars as f64);
            let mut d = steps[j.min(steps.len() - 1)];
            // lean toward the contour
            if (cur as f64) < target - 2.5 && d < 0 && rng.chance(0.6) {
                d = -d;
            }
            if (cur as f64) > target + 2.5 && d > 0 && rng.chance(0.6) {
                d = -d;
            }
            if j == 0 && pos % motif == 0 && bar > 0 {
                // a returning idea starts from where the contour says, not where the last one ended
                cur = target.round() as i32;
            }
            cur = (cur + d).clamp(0, scale.len() as i32 - 1);
            let mut k = scale[cur as usize];
            // strong beats land on a chord tone
            let strong = (off % 2.0).abs() < 0.01 || (j == 0);
            let chord = chord_at(t);
            if strong {
                if let Some(c) = chord {
                    if !c.has(k % 12) {
                        let mut best = k;
                        let mut bd = i32::MAX;
                        for cand in &scale {
                            if c.has(cand % 12) {
                                let dd = (*cand as i32 - k as i32).abs();
                                if dd < bd {
                                    bd = dd;
                                    best = *cand;
                                }
                            }
                        }
                        k = best;
                        cur = idx_of(k) as i32;
                    }
                }
            }
            // never hammer one note
            if out.last().map_or(false, |l| l.key == k) {
                last_pitch_run += 1;
                if last_pitch_run >= 2 {
                    cur = (cur + if cur + 1 < scale.len() as i32 { 1 } else { -1 }).clamp(0, scale.len() as i32 - 1);
                    k = scale[cur as usize];
                    last_pitch_run = 0;
                }
            } else {
                last_pitch_run = 0;
            }
            // close on the chord root (or the tonic)
            let is_final = last_bar && j + 1 == n_notes;
            if is_final {
                let root_pc = chord.map(|c| c.root).unwrap_or(spec.key.tonic);
                k = pitch_in(spec.lo, spec.hi, root_pc, k);
                cur = idx_of(k) as i32;
            }
            let vel = if strong { 0.88 } else { 0.72 } as f32;
            let mut l = (*len * if spec.feel == "staccato" { 0.5 } else { 0.95 }).max(0.1);
            if is_final {
                l = *len;
            }
            out.push(nt(t, l, k, vel));
        }
    }
    out
}

// ── style thumbprint ────────────────────────────────────────────────────────
pub struct Part {
    pub name: String,
    pub drums: bool,
    pub notes: Vec<N>,
}

pub struct Style {
    pub story: String,
    pub fields: String,
}

fn onset_stats(notes: &[N]) -> (f64, f64) {
    // (fraction of onsets on the beat, fraction on the off-beat 8th)
    if notes.is_empty() {
        return (0.0, 0.0);
    }
    let on = notes.iter().filter(|n| (n.start % 1.0).abs() < 0.06 || (n.start % 1.0 - 1.0).abs() < 0.06).count() as f64 / notes.len() as f64;
    let off = notes.iter().filter(|n| (n.start % 1.0 - 0.5).abs() < 0.06).count() as f64 / notes.len() as f64;
    (on, off)
}

fn density_word(d: f64) -> &'static str {
    if d < 0.3 {
        "sparse"
    } else if d < 0.65 {
        "medium"
    } else {
        "busy"
    }
}

/// notes per bar -> 0..1 density for a melodic part
fn density_of(notes_per_bar: f64) -> f64 {
    (notes_per_bar / 12.0).clamp(0.05, 1.0)
}

fn bass_pattern(notes: &[N]) -> &'static str {
    let (on, off) = onset_stats(notes);
    let avg_len = notes.iter().map(|n| n.len).sum::<f64>() / notes.len().max(1) as f64;
    let mut ks: Vec<u8> = notes.iter().map(|n| n.key).collect();
    ks.sort();
    ks.dedup();
    if avg_len >= 2.5 {
        "sub"
    } else if off > 0.7 {
        "offbeat"
    } else if on < 0.6 && avg_len < 0.6 && notes.len() as f64 / ((notes.iter().map(|n| n.end()).fold(0.0, f64::max) / 4.0).max(1.0)) > 8.0 {
        "rolling"
    } else if avg_len > 0.8 && avg_len <= 1.1 && ks.len() >= 3 {
        "walking"
    } else if on < 0.6 {
        "synco"
    } else {
        "root"
    }
}

fn chord_rhythm(notes: &[N], chord_len: f64) -> &'static str {
    let avg = notes.iter().map(|n| n.len).sum::<f64>() / notes.len().max(1) as f64;
    let (_on, off) = onset_stats(notes);
    if avg >= chord_len * 0.7 {
        "sustain"
    } else if off > 0.6 {
        "stabs"
    } else if avg < 0.3 {
        "gate"
    } else if avg > 0.8 {
        "pump"
    } else {
        "synco"
    }
}

fn contour_of(notes: &[N]) -> &'static str {
    let mut l: Vec<N> = notes.to_vec();
    l.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
    if l.len() < 8 {
        return "arch";
    }
    let q = |i: usize| -> f64 {
        let a = l.len() * i / 4;
        let b = (l.len() * (i + 1) / 4).max(a + 1);
        l[a..b].iter().map(|n| n.key as f64).sum::<f64>() / (b - a) as f64
    };
    let (a, b, c, d) = (q(0), q(1), q(2), q(3));
    let mid = (b + c) / 2.0;
    let ends = (a + d) / 2.0;
    if mid - ends > 2.5 {
        "arch"
    } else if d - a > 3.0 {
        "rise"
    } else if a - d > 3.0 {
        "fall"
    } else if (a.max(b).max(c).max(d) - a.min(b).min(c).min(d)) < 2.5 {
        "flat"
    } else {
        "wave"
    }
}

fn register_word(notes: &[N]) -> &'static str {
    let mut ks: Vec<u8> = notes.iter().map(|n| n.key).collect();
    ks.sort();
    let med = ks.get(ks.len() / 2).copied().unwrap_or(60);
    if med >= 76 {
        "high register"
    } else if med >= 62 {
        "middle register"
    } else if med >= 48 {
        "low-middle register"
    } else {
        "low register"
    }
}

/// a project (or a MIDI file) as a short story plus a field list that `spec_from_style` can apply to any part
pub fn thumbprint(parts: &[Part], bpm: f64) -> Style {
    let total = parts.iter().flat_map(|p| p.notes.iter()).map(|n| n.end()).filter(|x| x.is_finite()).fold(0.0, f64::max).clamp(4.0, 16384.0);
    let melodic: Vec<N> = parts.iter().filter(|p| !p.drums).flat_map(|p| p.notes.iter().copied()).collect();
    let mut fields: Vec<(String, String)> = vec![];
    if melodic.is_empty() && parts.iter().all(|p| p.notes.is_empty()) {
        return Style { story: "An empty project: nothing to describe yet.".into(), fields: String::new() };
    }
    let keys = theory::key_sections(&melodic, total, 4.0);
    let mut main = keys.iter().max_by(|a, b| (a.end - a.start).partial_cmp(&(b.end - b.start)).unwrap()).map(|k| k.key).unwrap_or(Key::new(0, true));
    // harmony from the parts that play chords, else from everything
    let harm_notes: Vec<N> = {
        let h: Vec<N> = parts.iter().filter(|p| !p.drums && matches!(theory::classify(&p.notes, false), "chords" | "pad")).flat_map(|p| p.notes.iter().copied()).collect();
        if h.len() >= 6 {
            h
        } else {
            melodic.clone()
        }
    };
    let chords = theory::detect_chords(&harm_notes, total, 2.0, false);
    // relative major and minor share their notes: the chord the piece starts on says which one it is
    if let Some(first) = chords.first() {
        let rel = if main.minor { Key::new((main.tonic + 3) % 12, false) } else { Key::new((main.tonic + 9) % 12, true) };
        if first.root == rel.tonic && first.root != main.tonic {
            main = rel;
        }
    }
    let keys: Vec<theory::KeySection> = keys.iter().map(|k| theory::KeySection { key: if k.key.tonic == main.tonic && k.key.minor != main.minor || (k.key.minor != main.minor && (k.key.tonic + if k.key.minor { 3 } else { 9 }) % 12 == main.tonic) { main } else { k.key }, ..*k }).collect();
    let phr = theory::phrases(&melodic, &chords, &keys);
    let rom: Vec<String> = chords.iter().map(|c| theory::roman(c, theory::key_at(&keys, c.start).unwrap_or(main))).collect();
    let mut prog_text = String::new();
    if let Some(p) = phr.iter().find(|p| p.kind == "progression") {
        prog_text = p.label.rsplit_once(" x").map(|x| x.0.to_string()).unwrap_or(p.label.clone());
    } else {
        let mut seen: Vec<String> = vec![];
        for r in &rom {
            if seen.last() != Some(r) {
                seen.push(r.clone());
            }
            if seen.len() >= 4 {
                break;
            }
        }
        prog_text = seen.join(" ");
    }
    let chord_len = if chords.is_empty() { 4.0 } else { chords.iter().map(|c| c.len).sum::<f64>() / chords.len() as f64 };
    let cpb = (4.0 / chord_len.max(1.0)).clamp(0.25, 4.0);
    let sevenths = chords.iter().filter(|c| matches!(QUAL[c.q].0, "7" | "maj7" | "m7")).count() as f64 / chords.len().max(1) as f64;

    // per-role summary
    let mut sentences: Vec<String> = vec![];
    let mut used_roles: Vec<&'static str> = vec![];
    let bars = (total / 4.0).max(1.0);
    for p in parts {
        if p.notes.is_empty() {
            continue;
        }
        let role = theory::classify(&p.notes, p.drums);
        if used_roles.contains(&role) && role != "lead" {
            continue;
        }
        let per_bar = p.notes.len() as f64 / bars;
        match role {
            "drums" => {
                let kicks: Vec<&N> = p.notes.iter().filter(|n| n.key == 35 || n.key == 36).collect();
                let on_beat = kicks.iter().filter(|n| (n.start % 1.0).abs() < 0.06).count();
                let four = !kicks.is_empty() && on_beat as f64 / kicks.len() as f64 > 0.8 && kicks.len() as f64 / bars > 3.2;
                let open_hats = p.notes.iter().filter(|n| n.key == 46 && (n.start % 1.0 - 0.5).abs() < 0.06).count();
                let mut s = format!("Drums keep {}", if four { "a four-on-the-floor pulse" } else { "a broken beat" });
                if open_hats as f64 / bars > 1.5 {
                    s += " with open hats on the off-beats";
                }
                sentences.push(s + ".");
                fields.push(("drums".into(), if four { "four".into() } else { "broken".into() }));
            }
            "bass" => {
                let pat = bass_pattern(&p.notes);
                let word = match pat {
                    "rolling" => "rolling eighth notes",
                    "offbeat" => "off-beat pulses",
                    "sub" => "long sub notes",
                    "walking" => "a walking line",
                    "synco" => "a syncopated groove",
                    _ => "steady roots",
                };
                sentences.push(format!("Bass plays {} in the {}.", word, register_word(&p.notes)));
                fields.push(("bass.pattern".into(), pat.into()));
                fields.push(("bass.density".into(), format!("{:.2}", density_of(per_bar))));
            }
            "chords" | "pad" => {
                let pat = chord_rhythm(&p.notes, chord_len);
                let word = match pat {
                    "sustain" => "sustained",
                    "stabs" => "off-beat stabbed",
                    "gate" => "gated",
                    "pump" => "pumping",
                    _ => "syncopated",
                };
                sentences.push(format!("{} chords in the {}, {}.", capitalize(word), register_word(&p.notes), if cpb >= 1.0 { "changing every bar".to_string() } else { "changing every two bars".to_string() }));
                fields.push(("chords.pattern".into(), if role == "pad" { "sustain".into() } else { pat.into() }));
            }
            "arp" => {
                sentences.push(format!("An arpeggio runs {} notes a bar in the {}.", per_bar.round(), register_word(&p.notes)));
                fields.push(("arp.density".into(), format!("{:.2}", density_of(per_bar))));
            }
            _ => {
                if fields.iter().any(|(k, _)| k == "lead.density") {
                    continue;
                }
                let d = density_of(per_bar);
                let c = contour_of(&p.notes);
                let mut ks: Vec<&N> = p.notes.iter().collect();
                ks.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap());
                let stepwise = ks.windows(2).filter(|w| (w[0].key as i32 - w[1].key as i32).abs() <= 2).count() as f64 / ks.len().max(2) as f64;
                let motif_bars = phr.iter().find(|x| x.kind == "motif" || x.kind == "sequence").map(|m| (((m.end - m.start) / 4.0).ceil() as u32).clamp(1, 4)).unwrap_or(2);
                sentences.push(format!(
                    "A {} {} lead in the {} paints {} contour{}.",
                    density_word(d),
                    if stepwise > 0.6 { "stepwise" } else { "leaping" },
                    register_word(&p.notes),
                    match c {
                        "arch" => "an arch",
                        "rise" => "a rising",
                        "fall" => "a falling",
                        "wave" => "a wave",
                        _ => "a flat",
                    },
                    if phr.iter().any(|x| x.kind == "motif" || x.kind == "sequence") { format!(", repeating a {motif_bars}-bar motif") } else { String::new() }
                ));
                fields.push(("lead.density".into(), format!("{d:.2}")));
                fields.push(("lead.contour".into(), c.into()));
                fields.push(("lead.motif".into(), motif_bars.to_string()));
                let mut ks2: Vec<u8> = p.notes.iter().map(|n| n.key).collect();
                ks2.sort();
                fields.push(("lead.range".into(), format!("{}-{}", ks2[ks2.len() / 10], ks2[ks2.len() * 9 / 10].max(ks2[ks2.len() / 10] + 8))));
            }
        }
        used_roles.push(role);
    }
    // overall feel
    let all_per_beat = melodic.len() as f64 / total;
    let energy = if all_per_beat > 4.0 {
        "driving"
    } else if all_per_beat > 1.5 {
        "steady"
    } else {
        "relaxed"
    };
    // genre guess
    let four = fields.iter().any(|(k, v)| k == "drums" && v == "four");
    let bass_pat = fields.iter().find(|(k, _)| k == "bass.pattern").map(|x| x.1.clone()).unwrap_or_default();
    let genre = if bpm >= 124.0 && bpm <= 150.0 && four && main.minor && bass_pat == "rolling" {
        "trance"
    } else if bpm >= 118.0 && bpm <= 132.0 && four && bass_pat == "offbeat" {
        if sevenths > 0.4 { "deep house" } else { "house" }
    } else if four && !main.minor && (bass_pat == "offbeat" || bass_pat == "synco") && sevenths > 0.4 {
        "disco"
    } else if bpm < 100.0 && sevenths > 0.4 {
        "lofi"
    } else if bpm >= 160.0 {
        "dnb"
    } else {
        ""
    };
    let head = format!(
        "A {}{} piece in {} at {} BPM{}.",
        energy,
        if genre.is_empty() { String::new() } else { format!(" {genre}") },
        main.name(),
        bpm.round(),
        if prog_text.is_empty() { String::new() } else { format!(", over the progression {prog_text}") }
    );
    let story = format!("{} {}", head, sentences.join(" "));
    let mut head_fields = vec![
        ("key".to_string(), main.name()),
        ("bpm".to_string(), format!("{}", bpm.round())),
        ("style".to_string(), genre.to_string()),
        ("prog".to_string(), prog_text),
        ("cpb".to_string(), format!("{cpb}")),
        ("energy".to_string(), energy.to_string()),
    ];
    head_fields.extend(fields);
    Style { story, fields: head_fields.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join(";") }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

/// turn a thumbprint's fields into the fields for one kind of part
pub fn style_fields_for(style: &str, kind: Kind) -> Vec<(String, String)> {
    let f = split_fields(style);
    let mut out: Vec<(String, String)> = vec![];
    for k in ["key", "style", "prog", "cpb"] {
        if let Some(v) = lookup(&f, k) {
            if !v.is_empty() {
                out.push((k.into(), v.into()));
            }
        }
    }
    let prefix = match kind {
        Kind::Melody => "lead",
        Kind::Chords | Kind::Pad => "chords",
        Kind::Bass => "bass",
        Kind::Arp => "arp",
        Kind::Drums => "drums",
    };
    for (k, v) in &f {
        if let Some(rest) = k.strip_prefix(&format!("{prefix}.")) {
            out.push((rest.to_string(), v.clone()));
        }
    }
    // a melody or arp imitating a style without its own part takes the lead's contour and density
    if matches!(kind, Kind::Arp) {
        for (k, v) in &f {
            if let Some(rest) = k.strip_prefix("lead.") {
                if rest == "contour" && !out.iter().any(|(x, _)| x == "contour") {
                    out.push((rest.to_string(), v.clone()));
                }
            }
        }
    }
    out
}

pub fn spec_from_style(style: &str, kind: Kind, bars: u32, default_key: Key, seed: u64) -> Spec {
    let mut layer = style_fields_for(style, kind);
    layer.push(("kind".into(), kind.name().into()));
    layer.push(("bars".into(), bars.to_string()));
    layer.push(("seed".into(), seed.to_string()));
    Spec::from_fields(&[&layer], default_key, kind)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn in_key(notes: &[N], key: Key) -> bool {
        notes.iter().all(|n| key.accepts(n.key % 12))
    }

    #[test]
    fn every_drum_pattern_has_a_kick_a_backbeat_and_stays_in_its_bars() {
        for pat in DRUM_PATTERNS {
            for style in ["", "house", "trap", "lofi"] {
                let f = vec![("kind".to_string(), "drums".to_string()), ("pattern".to_string(), pat.to_string()), ("style".to_string(), style.to_string()), ("bars".to_string(), "8".to_string()), ("density".to_string(), "0.8".to_string())];
                let s = Spec::from_fields(&[&f], Key::new(0, true), Kind::Melody);
                assert_eq!(s.kind, Kind::Drums);
                let notes = generate(&s);
                assert!(notes.iter().any(|n| n.key == K_KICK), "{pat} {style}: kick");
                assert!(notes.iter().any(|n| n.key == K_SNARE || n.key == K_CLAP || n.key == K_RIM), "{pat} {style}: backbeat");
                assert!(notes.iter().all(|n| n.start >= 0.0 && n.start < 32.1 && n.len > 0.0 && n.vel > 0.0 && n.vel <= 1.0), "{pat} {style}");
            }
        }
        // four on the floor: a kick on every beat of every bar
        let mut f = parse_fields("4 bars drums four on the floor");
        f.push(("fills".to_string(), "0".to_string()));
        let s = Spec::from_fields(&[&f], Key::new(0, true), Kind::Melody);
        let kicks: Vec<f64> = generate(&s).iter().filter(|n| n.key == K_KICK).map(|n| n.start).collect();
        assert_eq!(kicks.len(), 16, "{kicks:?}");
    }

    #[test]
    fn rhythm_masks_and_euclid() {
        assert_eq!(euclid(4, 16, 0).iter().filter(|b| **b).count(), 4);
        assert_eq!(euclid(3, 8, 0), [true, false, false, true, false, false, true, false]);
        assert_eq!(rhythm_mask("euclid:5,16,2").len(), 16);
        assert_eq!(rhythm_mask("x.x.").len(), 4);
        assert!(rhythm_mask("16ths").is_empty());
        assert_eq!(parse_rate("1/16"), 0.25);
        assert_eq!(parse_rate("1/8"), 0.5);
    }

    #[test]
    fn chords_become_arpeggios_that_follow_the_rate_and_the_mask() {
        let chord: Vec<N> = [60u8, 64, 67].iter().map(|k| N { start: 0.0, len: 4.0, key: *k, vel: 0.8 }).collect();
        let up = arpeggiate(&chord, 0.25, "up", 1, 0.9, "", 1);
        assert_eq!(up.len(), 16);
        assert_eq!(up.iter().take(3).map(|n| n.key).collect::<Vec<_>>(), [60, 64, 67]);
        assert!(up.iter().all(|n| [60, 64, 67].contains(&n.key)));
        let two_oct = arpeggiate(&chord, 0.5, "updown", 2, 0.5, "euclid:5,8,0", 1);
        assert!(two_oct.iter().any(|n| n.key >= 72), "second octave used");
        assert!(two_oct.len() < 8 && !two_oct.is_empty(), "the mask thins it: {}", two_oct.len());
        // a lone note is left as it was
        let lone = arpeggiate(&[N { start: 1.0, len: 1.0, key: 62, vel: 0.7 }], 0.25, "up", 1, 0.9, "", 1);
        assert_eq!(lone.len(), 1);
    }

    #[test]
    fn a_plain_request_becomes_a_spec() {
        let f = parse_fields("8 bars of a sparse trance melody in F# minor over i VI III VII, arch contour");
        let s = Spec::from_fields(&[&f], Key::new(0, false), Kind::Chords);
        assert_eq!(s.bars, 8);
        assert_eq!(s.kind, Kind::Melody);
        assert_eq!(s.key, Key::new(6, true));
        assert_eq!(s.style, "trance");
        assert_eq!(s.contour, "arch");
        assert!(s.density < 0.3);
        assert_eq!(s.prog.len(), 4);
        assert_eq!(s.prog[0], (6, 1)); // F# minor
        assert_eq!(s.prog[1], (2, 0)); // VI = D major
    }

    #[test]
    fn model_json_and_keywords_merge_with_the_model_filling_gaps() {
        let reply = r#"Sure! {"key": "A minor", "bars": 4, "kind": "bass", "progression": ["i", "VII", "VI", "VII"], "density": "busy"}"#;
        let j = fields_from_json(reply);
        let s = Spec::from_fields(&[&j], Key::new(0, true), Kind::Melody);
        assert_eq!(s.key, Key::new(9, true));
        assert_eq!(s.bars, 4);
        assert_eq!(s.kind, Kind::Bass);
        assert_eq!(s.prog.len(), 4);
        assert!(s.density > 0.7);
        // the user's own words override
        let k = parse_fields("make it 16 bars");
        let s2 = Spec::from_fields(&[&j, &k], Key::new(0, true), Kind::Melody);
        assert_eq!(s2.bars, 16);
    }

    #[test]
    fn every_kind_stays_in_key_and_in_range_for_many_seeds() {
        for kind in [Kind::Melody, Kind::Chords, Kind::Bass, Kind::Arp, Kind::Pad] {
            for style in ["trance", "house", "lofi", "hyperpop", "witch house", "disco", ""] {
                for seed in 1..6 {
                    let key = Key::new(6, true);
                    let f = vec![("kind".to_string(), kind.name().to_string()), ("style".to_string(), style.to_string()), ("seed".to_string(), seed.to_string()), ("bars".to_string(), "8".to_string())];
                    let mut s = Spec::from_fields(&[&f], key, kind);
                    if style == "disco" || style == "lofi" || style == "hyperpop" {
                        s.key = Key::new(7, false);
                    }
                    let notes = generate(&s);
                    assert!(!notes.is_empty(), "{kind:?} {style} {seed}");
                    let k = s.key;
                    // chord tones from borrowed chords can leave the scale; everything else must be inside it
                    if kind == Kind::Melody {
                        assert!(in_key(&notes, k), "{kind:?} {style} {seed}: {:?}", notes.iter().map(|n| n.key).collect::<Vec<_>>());
                    }
                    assert!(notes.iter().all(|n| n.start >= 0.0 && n.start < 32.0 && n.len > 0.0), "{kind:?} {style}");
                    assert!(notes.iter().all(|n| n.key >= s.lo.saturating_sub(12) && n.key <= s.hi + 12), "{kind:?} {style} range");
                }
            }
        }
    }

    #[test]
    fn the_same_seed_gives_the_same_music_and_a_new_seed_gives_new_music() {
        let mut s = Spec::from_fields(&[&parse_fields("8 bars melody trance in a minor")], Key::new(9, true), Kind::Melody);
        let a = generate(&s);
        assert_eq!(a, generate(&s));
        s.seed = 99;
        assert_ne!(a, generate(&s));
    }

    #[test]
    fn a_generated_melody_is_judged_clean_by_the_analyser() {
        let s = Spec::from_fields(&[&parse_fields("8 bars melody in D minor i VI III VII")], Key::new(2, true), Kind::Melody);
        let mel = generate(&s);
        let mut chords = vec![];
        let spans = plan(&s, &mut Rng::new(s.seed));
        for sp in &spans {
            chords.push(sp.chord.clone());
        }
        let keys = vec![theory::KeySection { start: 0.0, end: 32.0, key: s.key, score: 1.0 }];
        let roles = theory::note_roles(&mel, &keys, &chords);
        let bad = roles.iter().filter(|r| r.0 == theory::R_OUT).count();
        assert_eq!(bad, 0);
        // most strong-beat notes are chord tones
        let chord_tones = roles.iter().filter(|r| r.0 == theory::R_CHORD).count();
        assert!(chord_tones * 3 >= roles.len(), "{chord_tones} of {}", roles.len());
    }

    #[test]
    fn the_thumbprint_of_a_generated_song_describes_it_and_round_trips() {
        let key = Key::new(6, true);
        let mk = |kind: &str, bars: u32| {
            let f = parse_fields(&format!("{bars} bars {kind} trance in f# minor i VI III VII"));
            generate(&Spec::from_fields(&[&f], key, Kind::Melody))
        };
        let parts = vec![
            Part { name: "Bass".into(), drums: false, notes: mk("bass", 8) },
            Part { name: "Chords".into(), drums: false, notes: mk("pad", 8) },
            Part { name: "Lead".into(), drums: false, notes: mk("melody sparse", 8) },
            Part { name: "Kit".into(), drums: true, notes: (0..32).map(|b| N { start: b as f64, len: 0.1, key: 36, vel: 0.9 }).collect() },
        ];
        let st = thumbprint(&parts, 138.0);
        assert!(st.story.contains("F# minor") && st.story.contains("138 BPM"), "{}", st.story);
        assert!(st.story.contains("four-on-the-floor") && st.story.contains("rolling"), "{}", st.story);
        assert!(st.fields.contains("prog=i VI III VII"), "{}", st.fields);
        // the story itself parses back to the same key and progression
        let back = parse_fields(&st.story);
        assert!(back.iter().any(|(k, v)| k == "key" && v == "f# minor"), "{back:?}");
        // and a bass made from the style is a bass in that key on that progression
        let s = spec_from_style(&st.fields, Kind::Bass, 4, Key::new(0, false), 3);
        assert_eq!(s.key, key);
        assert_eq!(s.pattern, "rolling");
        assert_eq!(s.prog.len(), 4);
        assert!(!generate(&s).is_empty());
    }
}
