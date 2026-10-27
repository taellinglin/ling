//! Helpers for asking a language model (ling-ai's in-process Qwen3) to choose the instrumentation of an imported
//! MIDI file / FL Studio project.
//!
//! A small, fast model cannot be trusted to pick one of two hundred patch names, so it is asked something much easier:
//! name the musical *role* of every part ("warm pad", "wobble bass", ...) from a short fixed list. A table maps each role to
//! real patches, the parser accepts slightly broken answers, and any part the model misses or garbles gets a guess from
//! its name and register instead. The result always covers every part with a valid plugin and patch.
//! The model itself is run by the `ling` binary's `llm_*` builtins; nothing here talks to it.
use crate::midi_import::{gm_name, ImportedMidi, ImportedTrack};

const NOTE: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

fn note_name(k: u8) -> String {
    format!("{}{}", NOTE[(k % 12) as usize], (k as i32 / 12) - 1)
}

/// Largest number of notes sounding at once, and the average over note onsets.
fn polyphony(t: &ImportedTrack) -> (usize, f32) {
    let mut ev: Vec<(f64, i32)> = Vec::with_capacity(t.notes.len() * 2);
    for n in &t.notes {
        ev.push((n.start, 1));
        ev.push((n.start + n.len, -1));
    }
    // ends before starts at the same instant
    ev.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal).then(a.1.cmp(&b.1)));
    let (mut cur, mut max, mut sum, mut onsets) = (0i32, 0i32, 0i64, 0i64);
    for (_, d) in ev {
        cur += d;
        if d > 0 {
            sum += cur as i64;
            onsets += 1;
        }
        max = max.max(cur);
    }
    (max.max(0) as usize, if onsets > 0 { sum as f32 / onsets as f32 } else { 0.0 })
}

fn describe_part(i: usize, t: &ImportedTrack, bars: f64) -> String {
    if !t.samples.is_empty() {
        return format!("Part {i}: \"{}\" - audio samples ({} placements); no instrument is needed", t.name, t.samples.len());
    }
    if t.sampler.is_some() {
        // a tracker module part: it carries its own sample(s) and is named after them
        let n = t.notes.len();
        let mut keys: Vec<u8> = t.notes.iter().map(|x| x.key).collect();
        keys.sort_unstable();
        let (lo, hi) = (keys.first().copied().unwrap_or(0), keys.last().copied().unwrap_or(0));
        let kind = if t.percussive { "a drum / percussion sample (its own sample, played at different pitches for variety)" } else { "a sampled instrument" };
        return format!("Part {i}: \"{}\" - {kind}, {n} notes, range {}-{}", t.name, note_name(lo), note_name(hi));
    }
    let n = t.notes.len();
    let fl = if t.plugin.is_empty() { String::new() } else { format!(" played by the FL Studio plugin \"{}\"{}", t.plugin, fl_plugin_hint(&t.plugin)) };
    let kind = match t.program {
        None => "drums (General MIDI percussion)".to_string(),
        Some(p) => format!("GM program {} \"{}\"{fl}", p as usize + 1, gm_name(p)),
    };
    if n == 0 {
        return format!("Part {i}: \"{}\" - {kind}, no notes", t.name);
    }
    let mut keys: Vec<u8> = t.notes.iter().map(|x| x.key).collect();
    keys.sort_unstable();
    let (lo, hi, med) = (keys[0], keys[n - 1], keys[n / 2]);
    let (maxp, avgp) = polyphony(t);
    let avg_len = t.notes.iter().map(|x| x.len).sum::<f64>() / n as f64;
    let avg_vel = t.notes.iter().map(|x| x.vel as f64).sum::<f64>() / n as f64;
    let density = n as f64 / bars.max(1.0);
    // plain words first: small models read "bass register, single notes" far better than note names and averages
    let register = match med {
        0..=44 => "bass register",
        45..=54 => "low register",
        55..=66 => "middle register",
        67..=78 => "high register",
        _ => "very high register",
    };
    let texture = if maxp <= 1 {
        "single notes".to_string()
    } else if avgp >= 2.5 {
        format!("chords of up to {maxp} notes")
    } else {
        "a few overlapping notes".to_string()
    };
    let feel = if t.program.is_none() {
        String::new()
    } else if avg_len < 0.3 {
        ", staccato".into()
    } else if avg_len > 3.0 {
        ", long sustained notes".into()
    } else {
        String::new()
    };
    let busy = if density > 12.0 { "very busy" } else if density > 5.0 { "busy" } else { "sparse" };
    format!(
        "Part {i}: \"{}\" - {kind}, {register}, {texture}{feel}, {busy} ({n} notes, range {}-{}), avg velocity {avg_vel:.2}",
        t.name,
        note_name(lo),
        note_name(hi)
    )
}

/// The parts of an imported file as text a model can reason about.
pub fn describe(file: &str, mid: &ImportedMidi) -> String {
    let bars = (mid.length_beats / 4.0).ceil().max(1.0);
    let mut s = format!("File: {file}\nTempo: {} BPM, length {} bars, {} parts.\n", mid.bpm.map(|b| format!("{b:.0}")).unwrap_or_else(|| "unknown".into()), bars, mid.tracks.len());
    for (i, t) in mid.tracks.iter().enumerate().take(40) {
        s.push_str(&describe_part(i, t, bars));
        s.push('\n');
    }
    s
}

/// What an FL Studio plugin usually is, in words a small model can use (and which LingStation plugin covers it).
pub fn fl_plugin_hint(plugin: &str) -> &'static str {
    let p = plugin.to_lowercase();
    let table: [(&[&str], &str); 14] = [
        (&["sytrus", "harmor", "morphine", "toxic biohazard", "poizone", "sawer", "autogun"], " (an FM / additive synth: leads, pads and basses; Prism is the closest)"),
        (&["3x osc", "3xosc", "fruity 3"], " (a plain oscillator: simple saw, square or sine voices; Ling Synth X is the closest)"),
        (&["fpc", "drumaxx", "fruity kick", "kick", "drum", "slicex", "grosbeat", "gross beat"], " (a drum machine or beat slicer: drums)"),
        (&["sakura", "vocodex", "vocoder", "voice"], " (a vocal or vocoder sound: Ling Voice)"),
        (&["directwave", "sampler", "soundfont", "sf2"], " (plays samples or soundfonts: acoustic instruments, use General MIDI)"),
        (&["flex"], " (a preset-based synth: leads, plucks and pads of every kind)"),
        (&["minisynth", "fruity dx10", "dx10", "dream", "plucked", "ogun", "wasp"], " (a simple synth or FM keys / pluck)"),
        (&["piano", "ep", "keys"], " (keyboards)"),
        (&["bass", "boobass", "transistor"], " (a bass instrument)"),
        (&["growl", "reese", "wobble"], " (a growling bass: Ling Growl)"),
        (&["string", "orchestra"], " (strings)"),
        (&["brass", "trumpet"], " (brass)"),
        (&["guitar"], " (guitar)"),
        (&["pad", "ambient"], " (a pad)"),
    ];
    for (keys, hint) in table {
        if keys.iter().any(|k| p.contains(k)) {
            return hint;
        }
    }
    ""
}

/// The genres the model may name (the GenreBrain): each biases which plugin plays which role.
pub const GENRES: [&str; 20] = [
    "trance", "house", "deep house", "techno", "disco", "hyperpop", "witch house", "lofi", "ambient", "pop", "rock", "jazz", "cinematic", "synthwave", "drum and bass", "dubstep", "hardstyle", "chiptune", "orchestral", "folk",
];

/// A genre from tempo, drums and harmony alone, for when no model is available or it answers badly.
pub fn guess_genre(mid: &ImportedMidi) -> &'static str {
    let bpm = mid.bpm.unwrap_or(120.0);
    let bars = (mid.length_beats / 4.0).max(1.0);
    let drums: Vec<&ImportedTrack> = mid.tracks.iter().filter(|t| t.program.is_none()).collect();
    let kicks: usize = drums.iter().flat_map(|t| t.notes.iter()).filter(|n| n.key == 35 || n.key == 36).count();
    let four = !drums.is_empty() && kicks as f64 / bars > 3.2;
    let melodic: Vec<&ImportedTrack> = mid.tracks.iter().filter(|t| t.program.is_some() && !t.notes.is_empty()).collect();
    let names = mid.tracks.iter().map(|t| format!("{} {}", t.name, t.plugin).to_lowercase()).collect::<Vec<_>>().join(" ");
    let has = |w: &[&str]| w.iter().any(|x| names.contains(x));
    let acoustic = melodic.iter().filter(|t| t.program.map_or(false, |p| p < 24 || (40..=79).contains(&p) || (24..=31).contains(&p))).count();
    if has(&["chip", "8bit", "8-bit", "nes"]) {
        return "chiptune";
    }
    if has(&["wobble", "wub", "reese", "dubstep"]) {
        return "dubstep";
    }
    if drums.is_empty() && melodic.len() <= 3 {
        return if bpm < 90.0 { "ambient" } else { "cinematic" };
    }
    if acoustic * 2 >= melodic.len() && !four {
        return if has(&["string", "orchestra", "violin", "horn", "brass"]) { "orchestral" } else if bpm < 100.0 { "folk" } else { "rock" };
    }
    if four {
        return match bpm as u32 {
            0..=108 => "disco",
            109..=122 => "deep house",
            123..=132 => "house",
            133..=146 => "trance",
            147..=162 => "hardstyle",
            _ => "drum and bass",
        };
    }
    if bpm >= 160.0 {
        return "drum and bass";
    }
    if bpm < 95.0 {
        return "lofi";
    }
    "pop"
}

/// Plugins and patches a genre prefers for a role, ahead of the general table; `None` = use the general one.
pub fn genre_candidates(genre: &str, role: &str) -> Option<&'static [(&'static str, &'static str)]> {
    let lead = matches!(role, "supersaw lead" | "synth lead" | "hyper lead" | "laser lead");
    Some(match (genre, role) {
        // the chips first; the General MIDI square and Prism's chip-like patches if a chip patch is missing
        ("chiptune", _) if lead => &[("chip", "NES Square Lead"), ("chip", "NES Duty Lead"), ("chip", "SID Saw Lead"), ("chip", "GB Pulse Lead"), ("synth", "Electronic: Square Lead")],
        ("chiptune", "arp" | "pluck") => &[("chip", "GB Arp Shimmer"), ("chip", "Shimmer Arp"), ("chip", "Hyper Arp"), ("chip", "Sparkle Sweep"), ("prism", "Chipmunk Arp")],
        ("chiptune", "rolling bass" | "sub bass" | "pluck bass") => &[("chip", "NES Triangle Bass"), ("chip", "GB Wave Bass"), ("chip", "SID Filter Bass"), ("prism", "Octave Bass")],
        ("chiptune", "warm pad" | "ambient pad" | "strings" | "dark pad") => &[("chip", "SID PWM Pad"), ("chip", "SNES Strings")],
        ("chiptune", "brass") => &[("chip", "SNES Brass")],
        // an orchestra's percussion is the General MIDI kit of the soundfont, not a house machine
        ("orchestral" | "cinematic", "drums") => &[("auto", "")],
        ("orchestral" | "cinematic" | "folk" | "jazz" | "rock", "strings" | "brass" | "woodwind" | "piano" | "guitar" | "organ" | "keys" | "bell" | "choir") => &[("auto", "")],
        ("lofi" | "jazz", "keys" | "piano") => &[("prism", "Moonlight EP"), ("prism", "Rhodes Dream")],
        ("lofi", "sub bass" | "rolling bass") => &[("prism", "Warm Sub")],
        ("lofi", "warm pad" | "ambient pad") => &[("prism", "Tape Dream"), ("prism", "Soft Pad")],
        ("ambient", "warm pad" | "dark pad" | "ambient pad") => &[("prism", "Ether Pad"), ("prism", "Tranquility Pad"), ("prism", "Atlantis Pad")],
        ("ambient", _) if lead => &[("prism", "Glass Lead With Bell"), ("prism", "Spacer Lead")],
        ("trance", _) if lead => &[("prism", "Supersaw Lead"), ("prism", "Simple Trance Lead")],
        ("trance", "warm pad" | "ambient pad" | "strings") => &[("prism", "Classic Trance Pad"), ("prism", "Uplift Pad")],
        ("trance", "rolling bass" | "sub bass") => &[("prism", "Rolling Bass"), ("prism", "Bajo Trance")],
        ("hardstyle", _) if lead => &[("prism", "Hardstyle Lead"), ("prism", "Hoover Dark")],
        ("hardstyle", "rolling bass" | "sub bass" | "pluck bass" | "growl bass") => &[("prism", "Bass Donk"), ("prism", "Mega Bass")],
        ("dubstep" | "drum and bass", "rolling bass" | "sub bass" | "wobble bass" | "growl bass") => &[("growl", "wobble"), ("growl", "neuro"), ("growl", "reese"), ("growl", "comb growl"), ("growl", "riddim fold"), ("growl", "neuro reese"), ("growl", "brass growl")],
        ("dubstep", _) if lead => &[("growl", "laser"), ("prism", "Dubstep Lead"), ("growl", "screech lead"), ("growl", "comb screech")],
        ("house" | "deep house", "rolling bass" | "sub bass") => &[("prism", "Offbeat Bass"), ("prism", "Offbeat Bass Two")],
        ("house" | "deep house", "stab chord" | "keys" | "piano") => &[("prism", "Deep Stab"), ("prism", "Dub Chord"), ("synth", "Electronic: House Piano")],
        ("disco", "rolling bass" | "sub bass" | "pluck bass") => &[("prism", "Octave Bass"), ("prism", "Funk Pluck")],
        ("disco", "strings" | "warm pad") => &[("prism", "Disco Strings")],
        ("synthwave", _) if lead => &[("prism", "Perfecto Synth"), ("prism", "Panama Synth")],
        ("synthwave", "warm pad" | "strings") => &[("prism", "Phaser Pad"), ("prism", "Dreamer Pad")],
        ("hyperpop", _) if lead => &[("prism", "Hyper Lead"), ("prism", "Sugar Stack")],
        ("witch house", "warm pad" | "dark pad" | "ambient pad" | "strings") => &[("prism", "Witch Drag"), ("prism", "Black Mass")],
        _ => return None,
    })
}

/// The vocabulary the model chooses from.
pub const ROLES: [&str; 30] = [
    "drums", "sub bass", "rolling bass", "wobble bass", "growl bass", "acid bass", "pluck bass", "supersaw lead", "synth lead", "laser lead", "vocal lead", "pluck", "arp", "warm pad",
    "dark pad", "ambient pad", "strings", "brass", "stab chord", "keys", "piano", "organ", "bell", "choir", "voice", "riser", "texture", "guitar", "woodwind", "hyper lead",
];

/// (plugin, patch name as the import dialog lists it) candidates for a role; the part number picks among them.
#[doc(hidden)]
pub fn candidates_for_tests(role: &str) -> &'static [(&'static str, &'static str)] {
    candidates(role)
}

fn candidates(role: &str) -> &'static [(&'static str, &'static str)] {
    match role {
        "sub bass" => &[("prism", "Warm Sub"), ("prism", "Void Sub")],
        "rolling bass" => &[("prism", "Rolling Bass"), ("prism", "Offbeat Bass")],
        "wobble bass" => &[("growl", "wobble"), ("growl", "reese"), ("growl", "electro wob"), ("growl", "color wobble")],
        "growl bass" => &[("growl", "growl"), ("growl", "neuro"), ("growl", "comb growl"), ("growl", "brass growl"), ("growl", "shifted growl")],
        "acid bass" => &[("synth", "Electronic: Acid Bass"), ("prism", "Bubble Bass")],
        "pluck bass" => &[("prism", "Octave Bass"), ("synth", "Electronic: Techno Pluck Bass")],
        "supersaw lead" => &[("prism", "Supersaw Lead"), ("prism", "Sugar Stack")],
        "synth lead" => &[("synth", "Electronic: Square Lead"), ("prism", "Quarter Lead")],
        "laser lead" => &[("growl", "laser"), ("growl", "stab"), ("growl", "screech lead"), ("growl", "complextro stab")],
        "vocal lead" => &[("growl", "yoi"), ("voice", "aria")],
        "pluck" => &[("prism", "Trance Pluck"), ("prism", "Deep Pluck"), ("prism", "Funk Pluck")],
        "arp" => &[("prism", "Chipmunk Arp"), ("prism", "Cursed Arp"), ("prism", "Glitch Pluck")],
        "warm pad" => &[("prism", "Uplift Pad"), ("prism", "Candy Pad"), ("prism", "Phaser Pad")],
        "dark pad" => &[("prism", "Witch Drag"), ("prism", "Dark Choir"), ("prism", "Black Mass")],
        "ambient pad" => &[("prism", "Tape Dream"), ("prism", "Analog Hum"), ("prism", "Slendro Pad"), ("prism", "Bohlen Drone")],
        "strings" => &[("prism", "Disco Strings"), ("synth", "Electronic: Supersaw Ensemble")],
        "brass" => &[("prism", "Brass Stab"), ("synth", "Electronic: Analog Brass")],
        "stab chord" => &[("prism", "Deep Stab"), ("prism", "Dub Chord")],
        "keys" => &[("prism", "Dream Keys"), ("synth", "Electronic: DX E-Piano")],
        "piano" => &[("synth", "Electronic: EDM Grand"), ("synth", "Electronic: House Piano")],
        "organ" => &[("synth", "Electronic: M1 House Organ"), ("synth", "Electronic: Italo Organ")],
        "bell" => &[("prism", "Ritual Bell"), ("prism", "19 EDO Glass")],
        "choir" => &[("voice", "choir"), ("prism", "Dark Choir")],
        "voice" => &[("voice", "aria"), ("voice", "mei"), ("voice", "kai"), ("voice", "ren")],
        "riser" => &[("growl", "riser"), ("prism", "Radio Ghost")],
        "texture" => &[("prism", "Radio Ghost"), ("prism", "Analog Hum")],
        "guitar" => &[("synth", "Electronic: Electro Steel"), ("synth", "Electronic: Funk Pluck")],
        "woodwind" => &[("synth", "Electronic: Pure Sine Lead"), ("synth", "Electronic: Breath Lead")],
        "hyper lead" => &[("prism", "Hyper Lead"), ("prism", "Sugar Stack")],
        _ => &[("prism", "Supersaw Lead")],
    }
}

/// The role a part most likely has, from its name, register and playing: used whenever the model gives none.
pub fn guess_role(t: &ImportedTrack) -> &'static str {
    if t.program.is_none() {
        return "drums";
    }
    let name = t.name.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| name.contains(w));
    let table: [(&[&str], &str); 20] = [
        (&["wobble", "wub"], "wobble bass"),
        (&["reese", "neuro", "growl"], "growl bass"),
        (&["acid", "303"], "acid bass"),
        (&["sub", "808"], "sub bass"),
        (&["bass"], "rolling bass"),
        (&["riser", "sweep", "uplift"], "riser"),
        (&["arp", "sequence"], "arp"),
        (&["bell", "chime", "glock", "mallet", "marimba"], "bell"),
        (&["choir", "aah", "ooh"], "choir"),
        (&["vox", "voice", "vocal"], "voice"),
        (&["string", "violin", "cello", "orchestra", "viola"], "strings"),
        (&["trumpet", "brass", "horn", "trombone"], "brass"),
        (&["flute", "oboe", "clarinet", "sax"], "woodwind"),
        (&["piano"], "piano"),
        (&["organ"], "organ"),
        (&["guitar", "banjo", "harp"], "guitar"),
        (&["keys", "rhodes", "epiano"], "keys"),
        (&["pad", "atmos", "ambient", "drone"], "warm pad"),
        (&["pluck", "pizz", "stab"], "pluck"),
        (&["lead", "solo"], "supersaw lead"),
    ];
    for (words, role) in table {
        if has(words) {
            return role;
        }
    }
    // otherwise by what it plays
    let n = t.notes.len().max(1) as f64;
    let mut keys: Vec<u8> = t.notes.iter().map(|x| x.key).collect();
    keys.sort_unstable();
    let med = keys.get(keys.len() / 2).copied().unwrap_or(60);
    let avg_len = t.notes.iter().map(|x| x.len).sum::<f64>() / n;
    let (maxp, _) = polyphony(t);
    if med < 48 {
        return if avg_len < 0.6 { "rolling bass" } else { "sub bass" };
    }
    if maxp >= 3 {
        return if avg_len > 1.5 { "warm pad" } else { "stab chord" };
    }
    if avg_len < 0.35 {
        return "arp";
    }
    if med >= 64 {
        "supersaw lead"
    } else {
        "pluck"
    }
}

/// Match whatever the model wrote to one of the roles (exact, containment, then shared words).
pub fn resolve_role(text: &str) -> Option<&'static str> {
    let t: String = text.trim().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ");
    let t = t.trim_matches(|c: char| !c.is_alphanumeric()).to_string();
    if t.is_empty() {
        return None;
    }
    if let Some(r) = ROLES.iter().find(|r| **r == t) {
        return Some(r);
    }
    if let Some(r) = ROLES.iter().find(|r| t.contains(**r)) {
        return Some(r);
    }
    let words: Vec<&str> = t.split(|c: char| !c.is_alphanumeric()).filter(|w| w.len() > 2).collect();
    ROLES
        .iter()
        .filter_map(|r| {
            let score = words.iter().filter(|w| r.split(' ').any(|rw| rw == **w)).count();
            (score > 0).then_some((score, *r))
        })
        .max_by_key(|x| x.0)
        .map(|x| x.1)
}

/// (plugin, patch) for a part given its role; drums pick a kit by tempo, the part number varies the pick.
pub fn role_choice(role: &str, part: usize, bpm: f64) -> (&'static str, &'static str) {
    if role == "drums" {
        return ("drums", if bpm > 0.0 && bpm < 112.0 { "Kit Electro 808" } else { "Kit House 909" });
    }
    let c = candidates(role);
    c[part % c.len()]
}

/// (plugin, patch) for `role` in `genre`: the genre's table, else the general one, keeping only patches that play in tune and do not
/// clash (soundcat); the part number picks among what is left. Drums have their own table.
pub fn pick(genre: &str, role: &str, part: usize, bpm: f64) -> (&'static str, &'static str) {
    if role == "drums" {
        return role_choice(role, part, bpm);
    }
    let from_genre = genre_candidates(genre, role).map(|l| crate::soundcat::safe_list(role, l)).filter(|l| l.iter().all(|(p, n)| crate::soundcat::pair_ok(role, p, n)));
    let list = match from_genre {
        Some(l) if !l.is_empty() => l,
        _ => crate::soundcat::safe_list(role, candidates(role)),
    };
    list[part % list.len()]
}

pub struct Choice {
    pub part: usize,
    pub role: String,
    pub plugin: String,
    pub patch: String,
    pub why: String,
}
pub struct Reply {
    pub style: String,
    pub parts: Vec<(usize, String)>,
    /// words the model gave to look for a drum sample with: (part, "crash cymbal")
    pub samples: Vec<(usize, String)>,
}

fn quoted_after(text: &str, key: &str) -> Option<String> {
    let k = text.find(key)? + key.len();
    let after = &text[k..];
    let q1 = after.find('"')?;
    let q2 = after[q1 + 1..].find('"')?;
    Some(after[q1 + 1..q1 + 1 + q2].to_string())
}

/// Read the model's answer. Tolerant on purpose: a JSON object when it is valid, otherwise any `"part": N ... "role": "x"`
/// pairs found in the text, so a slightly broken answer still yields every role the model did give.
pub fn parse_reply(raw: &str) -> Reply {
    let mut text = raw.to_string();
    while let (Some(a), Some(b)) = (text.find("<think>"), text.find("</think>")) {
        if b < a {
            break;
        }
        text.replace_range(a..b + "</think>".len(), "");
    }
    let mut r = Reply { style: String::new(), parts: vec![], samples: vec![] };
    if let (Some(a), Some(b)) = (text.find('{'), text.rfind('}')) {
        if b > a {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text[a..=b]) {
                r.style = v.get("style").and_then(|s| s.as_str()).unwrap_or("").to_string();
                if let Some(arr) = v.get("parts").and_then(|p| p.as_array()) {
                    for p in arr {
                        let part = p.get("part").and_then(|x| x.as_u64().or_else(|| x.as_f64().map(|f| f as u64)).or_else(|| x.as_str().and_then(|s| s.trim().parse().ok())));
                        let role = p.get("role").or_else(|| p.get("instrument")).and_then(|x| x.as_str());
                        if let (Some(part), Some(role)) = (part, role) {
                            r.parts.push((part as usize, role.to_string()));
                        }
                        if let (Some(part), Some(sample)) = (part, p.get("sample").and_then(|x| x.as_str())) {
                            r.samples.push((part as usize, sample.to_string()));
                        }
                    }
                }
                if !r.parts.is_empty() {
                    return r;
                }
            }
        }
    }
    // broken JSON: scan for "part": N followed by "role": "..."
    let mut pos = 0;
    while let Some(i) = text[pos..].find("\"part\"") {
        let from = pos + i + 6;
        let rest = &text[from..];
        let seg = &rest[..rest.find("\"part\"").unwrap_or(rest.len())];
        let digits: String = seg.chars().skip_while(|c| !c.is_ascii_digit()).take_while(|c| c.is_ascii_digit()).collect();
        if let (Ok(n), Some(role)) = (digits.parse::<usize>(), quoted_after(seg, "\"role\"")) {
            r.parts.push((n, role));
            if let Some(smp) = quoted_after(seg, "\"sample\"") {
                r.samples.push((n, smp));
            }
        }
        pos = from;
    }
    r.style = quoted_after(&text, "\"style\"").unwrap_or_default();
    r
}

/// Final choices for every melodic / drum part: the model's role where it gave a recognisable one, the guess otherwise.
pub fn choose(mid: &ImportedMidi, reply: &Reply) -> Vec<Choice> {
    choose_for(mid, reply, "")
}

/// like `choose`, with the song's genre (from the model's first answer or `guess_genre`) deciding between plugins
pub fn choose_for(mid: &ImportedMidi, reply: &Reply, genre: &str) -> Vec<Choice> {
    let bpm = mid.bpm.unwrap_or(0.0);
    let melodic = mid.tracks.iter().filter(|t| t.samples.is_empty() && t.program.is_some()).count();
    // a small model tends to give one role to everything: let each role cover at most a third of the parts
    let cap = (melodic / 3).max(2);
    let mut used: std::collections::HashMap<&'static str, usize> = Default::default();
    mid.tracks
        .iter()
        .enumerate()
        .filter(|(_, t)| t.samples.is_empty())
        .map(|(i, t)| {
            let from_model = reply.parts.iter().find(|(p, _)| *p == i).and_then(|(_, r)| resolve_role(r));
            // an acoustic instrument in an acoustic genre (strings, brass, woodwind, piano ...) is what its General MIDI program says it
            // is: a small model that answers "supersaw lead" for a violin is wrong, so its answer is not used for such parts
            let acoustic = matches!(genre, "orchestral" | "cinematic" | "folk" | "jazz") && t.program.map_or(false, |p| p < 80);
            let from_model = if acoustic { None } else { from_model };
            // a tracker module's percussion: look for a drum sample outside the module (the importer falls back to its own sample when none is found)
            if t.sampler.is_some() && t.percussive {
                let words = sample_words(t, reply, i);
                return Choice { part: i, role: "drum sample".to_string(), plugin: "lib".to_string(), patch: words.clone(), why: format!("drum sample: \"{words}\" from the library, else its own sample") };
            }
            let med = median_key(t);
            // a drum part is drums whatever the model says, and a melodic part is never drums
            let (role, by_model) = match (t.program.is_none(), from_model) {
                (true, _) => ("drums", true),
                (false, Some("drums")) | (false, None) => (guess_role(t), false),
                (false, Some(r)) => {
                    let bassy = r.contains("bass");
                    let wrong_register = (bassy && med >= 60) || (!bassy && med > 0 && med < 43);
                    if wrong_register || used.get(r).copied().unwrap_or(0) >= cap {
                        (guess_role(t), false)
                    } else {
                        (r, true)
                    }
                }
            };
            if !t.program.is_none() {
                *used.entry(role).or_insert(0) += 1;
            }
            let (plugin, patch) = pick(genre, role, i, bpm);
            Choice { part: i, role: role.to_string(), plugin: plugin.to_string(), patch: patch.to_string(), why: format!("{role}{}", if by_model { "" } else { " (guessed from name and range)" }) }
        })
        .collect()
}

fn median_key(t: &ImportedTrack) -> u8 {
    let mut k: Vec<u8> = t.notes.iter().map(|n| n.key).collect();
    k.sort_unstable();
    k.get(k.len() / 2).copied().unwrap_or(0)
}

/// One worked example (question, answer) built with the same code as the real question, so the shape always matches.
pub fn example() -> (String, String) {
    use crate::model::Note;
    let mk = |name: &str, program: Option<u8>, notes: Vec<Note>| ImportedTrack { named: true, plugin: String::new(), name: name.into(), program, bank: 0, channel: 0, notes, volume: 1.0, pan: 0.0, vol_auto: vec![], pan_auto: vec![], bend_auto: vec![], bend_range: 2.0, samples: vec![], sampler: None, percussive: false, extra_auto: vec![] };
    let kit: Vec<Note> = (0..64).flat_map(|b| [36u8, 42, 38, 42].into_iter().enumerate().map(move |(i, k)| Note::new(b as f64 + i as f64 * 0.25, 0.1, k, 0.8))).collect();
    let bass: Vec<Note> = (0..64).map(|b| Note::new(b as f64 * 0.5, 0.3, 36 + (b % 4) as u8 * 2, 0.9)).collect();
    let chords: Vec<Note> = (0..8).flat_map(|b| [52u8, 59, 64, 67].into_iter().map(move |k| Note::new(b as f64 * 4.0, 3.8, k, 0.7))).collect();
    let lead: Vec<Note> = (0..32).map(|b| Note::new(b as f64, 0.5, 72 + (b % 5) as u8, 0.85)).collect();
    let arp: Vec<Note> = (0..128).map(|b| Note::new(b as f64 * 0.25, 0.15, 60 + (b % 4) as u8 * 4, 0.7)).collect();
    let mid = ImportedMidi { tracks: vec![mk("Kit", None, kit), mk("Bass", Some(38), bass), mk("Chords", Some(89), chords), mk("Melody", Some(80), lead), mk("Part 5", Some(0), arp)], bpm: Some(126.0), tempos: vec![], length_beats: 32.0 };
    let q = prompt("example.mid", &mid);
    let a = r#"{"style": "house", "parts": [{"part": 0, "role": "drums"}, {"part": 1, "role": "rolling bass"}, {"part": 2, "role": "warm pad"}, {"part": 3, "role": "supersaw lead"}, {"part": 4, "role": "arp"}]}"#.to_string();
    (q, a)
}

/// The first question (GenreBrain): what kind of music is this? The answer steers the instrumentation question that follows.
pub fn genre_prompt(file: &str, mid: &ImportedMidi) -> String {
    format!("{}\nGenres to choose from: {}.\nWhat genre is this song? Answer with the genre only, as JSON like {{\"genre\": \"house\"}}.", describe(file, mid), GENRES.join(", "))
}

/// the genre a model named (exact, then containing), else `None`
pub fn parse_genre(reply: &str) -> Option<&'static str> {
    let l = reply.to_lowercase();
    let word = l.split("\"genre\"").nth(1).unwrap_or(&l).to_string();
    let mut best: Option<&'static str> = None;
    for g in GENRES {
        if word.contains(g) && best.map_or(true, |b| g.len() > b.len()) {
            best = Some(g);
        }
    }
    best
}

/// The question for the model: the parts, then the roles it may answer with.
pub fn prompt(file: &str, mid: &ImportedMidi) -> String {
    let drums = mid.tracks.iter().any(|t| t.percussive);
    format!(
        "{}\nRoles to choose from: {}.\nGive the role of every part. Use a variety of roles that fit together as one song.{}",
        describe(file, mid),
        ROLES.join(", "),
        if drums { "\nFor a drum or percussion part also give \"sample\": two or three words to look for a matching drum sample with in a sample library (like \"crash cymbal\", \"snare\", \"closed hat\", \"kick\", \"low tom\")." } else { "" }
    )
}

/// words to search a sample library with for a percussive part: the model's, else made from the part's own name
pub fn sample_words(t: &ImportedTrack, reply: &Reply, part: usize) -> String {
    if let Some((_, w)) = reply.samples.iter().find(|(p, _)| *p == part) {
        let w: String = w.chars().filter(|c| c.is_alphanumeric() || *c == ' ' || *c == '-').collect::<String>().trim().to_lowercase();
        if !w.is_empty() && w.len() < 40 {
            return w;
        }
    }
    t.name.chars().filter(|c| c.is_alphabetic() || *c == ' ' || *c == '-').collect::<String>().trim().to_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::Note;

    fn part(name: &str, program: Option<u8>, notes: Vec<Note>) -> ImportedTrack {
        ImportedTrack { named: true, plugin: String::new(), name: name.into(), program, bank: 0, channel: 0, notes, volume: 1.0, pan: 0.0, vol_auto: vec![], pan_auto: vec![], bend_auto: vec![], bend_range: 2.0, samples: vec![], sampler: None, percussive: false, extra_auto: vec![] }
    }

    #[test]
    fn parts_are_described_with_range_polyphony_and_feel() {
        let chord: Vec<Note> = (0..8).flat_map(|b| [48u8, 55, 60].into_iter().map(move |k| Note::new(b as f64 * 4.0, 4.0, k, 0.7))).collect();
        let bass: Vec<Note> = (0..32).map(|b| Note::new(b as f64, 0.2, 36, 0.9)).collect();
        let mid = ImportedMidi { tracks: vec![part("Pad", Some(89), chord), part("Sub", Some(38), bass), part("Kit", None, vec![Note::new(0.0, 0.1, 36, 1.0)])], bpm: Some(128.0), tempos: vec![], length_beats: 32.0 };
        let d = describe("song.mid", &mid);
        assert!(d.contains("128 BPM"), "{d}");
        assert!(d.contains("Part 0: \"Pad\"") && d.contains("Warm Pad") && d.contains("overlapping notes"), "{d}");
        assert!(d.contains("long sustained notes") && d.contains("middle register"), "{d}");
        assert!(d.contains("range C3-C4"), "{d}");
        assert!(d.contains("staccato") && d.contains("single notes") && d.contains("bass register") && d.contains("Synth Bass 1"), "{d}");
        assert!(d.contains("drums (General MIDI percussion)"), "{d}");
    }

    #[test]
    fn one_role_cannot_take_over_the_whole_song() {
        let notes = |k: u8| -> Vec<Note> { (0..16).map(|b| Note::new(b as f64, 0.5, k, 0.8)).collect() };
        let mid = ImportedMidi { tracks: (0..6).map(|i| part(&format!("P{i}"), Some(80), notes(if i == 0 { 30 } else { 72 }))).collect(), bpm: Some(128.0), tempos: vec![], length_beats: 16.0 };
        let reply = parse_reply(r#"{"parts":[{"part":0,"role":"supersaw lead"},{"part":1,"role":"supersaw lead"},{"part":2,"role":"supersaw lead"},{"part":3,"role":"supersaw lead"},{"part":4,"role":"supersaw lead"},{"part":5,"role":"supersaw lead"}]}"#);
        let c = choose(&mid, &reply);
        assert_eq!(c.len(), 6);
        assert!(c.iter().filter(|c| c.role == "supersaw lead" && !c.why.contains("guessed")).count() <= 2);
        assert!(!c[0].why.starts_with("supersaw lead") || c[0].why.contains("guessed"), "a lead on a bass-register part is rejected");
    }

    #[test]
    fn the_worked_example_is_a_valid_answer_to_its_own_question() {
        let (q, a) = example();
        assert!(q.contains("Part 4") && q.contains("Roles to choose from"));
        let r = parse_reply(&a);
        assert_eq!(r.parts.len(), 5);
        assert!(r.parts.iter().all(|(_, role)| resolve_role(role).is_some()));
    }

    #[test]
    fn replies_are_parsed_even_when_broken() {
        let ok = parse_reply("<think>hmm</think>Sure! ```json\n{\"style\": \"deep house\", \"parts\": [{\"part\": 0, \"role\": \"sub bass\"}, {\"part\": \"1\", \"role\": \"warm pad\"}]}\n```");
        assert_eq!(ok.style, "deep house");
        assert_eq!(ok.parts, vec![(0, "sub bass".to_string()), (1, "warm pad".to_string())]);
        // truncated mid-way: the finished pairs still count
        let cut = parse_reply("{\"style\": \"trance\", \"parts\": [{\"part\": 0, \"role\": \"supersaw lead\"}, {\"part\": 1, \"role\": \"ar");
        assert_eq!(cut.parts, vec![(0, "supersaw lead".to_string())], "{:?}", cut.parts);
        assert_eq!(cut.style, "trance");
        assert!(parse_reply("I think the bass should be deep.").parts.is_empty());
    }

    #[test]
    fn every_part_gets_a_valid_choice_whatever_the_model_says() {
        let pad: Vec<Note> = (0..4).flat_map(|b| [55u8, 59, 62].into_iter().map(move |k| Note::new(b as f64 * 4.0, 4.0, k, 0.7))).collect();
        let bass: Vec<Note> = (0..16).map(|b| Note::new(b as f64, 0.3, 36, 0.9)).collect();
        let lead: Vec<Note> = (0..16).map(|b| Note::new(b as f64, 0.5, 74, 0.9)).collect();
        let mid = ImportedMidi { tracks: vec![part("Kit", None, vec![Note::new(0.0, 0.1, 36, 1.0)]), part("Part A", Some(0), bass), part("Part B", Some(0), pad), part("Part C", Some(0), lead)], bpm: Some(128.0), tempos: vec![], length_beats: 16.0 };
        // the model contradicts itself, misses a part and invents a role: the result is still complete and sensible
        let reply = parse_reply("{\"parts\": [{\"part\": 0, \"role\": \"warm pad\"}, {\"part\": 1, \"role\": \"wobble bass\"}, {\"part\": 3, \"role\": \"totally made up\"}]}");
        let c = choose(&mid, &reply);
        assert_eq!(c.len(), 4);
        assert_eq!(c[0].plugin, "drums", "drums stay drums");
        assert_eq!((c[1].plugin.as_str(), c[1].role.as_str()), ("growl", "wobble bass"));
        assert_eq!(c[2].role, "warm pad", "a chord part with no answer falls back to range and polyphony");
        assert_eq!(c[3].role, "supersaw lead", "a high single-note line is a lead");
        for ch in &c {
            assert!(!ch.plugin.is_empty() && !ch.patch.is_empty());
        }
        assert_eq!(resolve_role("Supersaw  Lead!"), Some("supersaw lead"));
        assert_eq!(resolve_role("warm"), Some("warm pad"));
        assert_eq!(resolve_role(""), None);
    }

    #[test]
    fn every_role_maps_to_real_patch_files() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../LingStationX/instruments");
        if !root.is_dir() {
            return; // the app folder is not next to the engine in every checkout
        }
        let names = |dir: &str, strip: &str| -> Vec<String> {
            std::fs::read_dir(root.join(dir))
                .map(|rd| {
                    rd.flatten()
                        .filter_map(|f| {
                            let n = f.file_name().to_string_lossy().into_owned();
                            n.strip_suffix(".ling").map(|n| n.replace('_', " ").trim_start_matches(strip).to_string())
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        let prism = names("Prism", "Prism ");
        let growl = names("Growl", "Growl ");
        let chip = names("Chip", "Chip ");
        let voice = names("Voice", "Voice ");
        let egm: Vec<String> = names("Electronic GM", "").into_iter().map(|n| format!("Electronic: {}", n.split_once(' ').map(|x| x.1).unwrap_or(&n))).collect();
        let kits = names(".", "");
        for role in ROLES.iter().filter(|r| **r != "drums") {
            for (plugin, patch) in candidates(role) {
                let ok = match *plugin {
                    "prism" => prism.iter().any(|p| p == patch),
                    "growl" => growl.iter().any(|p| p == patch),
                    "chip" => chip.iter().any(|p| p == patch),
                    "voice" => voice.iter().any(|p| p == patch),
                    _ => egm.iter().any(|p| p == patch),
                };
                assert!(ok, "role {role}: no {plugin} patch called {patch}");
            }
        }
        for k in ["Kit Electro 808", "Kit House 909"] {
            assert!(kits.iter().any(|p| p == k), "missing kit {k}");
        }
        // the genre-specific choices point at real patches too
        for genre in GENRES {
            for role in ROLES.iter().filter(|r| **r != "drums") {
                if let Some(list) = genre_candidates(genre, role) {
                    for (plugin, patch) in list {
                        let ok = match *plugin {
                            "auto" => true,
                            "prism" => prism.iter().any(|p| p == patch),
                            "growl" => growl.iter().any(|p| p == patch),
                            "chip" => chip.iter().any(|p| p == patch),
                    "chip" => chip.iter().any(|p| p == patch),
                            "voice" => voice.iter().any(|p| p == patch),
                            _ => egm.iter().any(|p| p == patch),
                        };
                        assert!(ok, "{genre} / {role}: no {plugin} patch called {patch}");
                    }
                }
            }
        }
    }

    #[test]
    fn genre_is_parsed_and_guessed_and_steers_the_plugin() {
        assert_eq!(parse_genre("{\"genre\": \"deep house\"}"), Some("deep house"));
        assert_eq!(parse_genre("It is probably TRANCE."), Some("trance"));
        assert_eq!(parse_genre("no idea"), None);
        let notes: Vec<Note> = (0..32).map(|b| Note::new(b as f64 * 0.5, 0.3, 72, 0.8)).collect();
        let mid = ImportedMidi { tracks: vec![part("Lead", Some(80), notes)], bpm: Some(140.0), tempos: vec![], length_beats: 16.0 };
        let reply = parse_reply(r#"{"parts":[{"part":0,"role":"supersaw lead"}]}"#);
        let trance = choose_for(&mid, &reply, "trance");
        let chip = choose_for(&mid, &reply, "chiptune");
        assert_eq!(trance[0].plugin, "prism");
        assert_eq!(chip[0].plugin, "chip", "a chiptune lead is a square on Ling Chip");
        let orch = choose_for(&mid, &parse_reply(r#"{"parts":[{"part":0,"role":"strings"}]}"#), "orchestral");
        assert_eq!(orch[0].plugin, "auto", "orchestral strings stay on General MIDI");
        // a model that calls a violin a supersaw lead in an orchestral piece is overruled by the instrument itself
        let wrong = choose_for(&mid, &parse_reply(r#"{"parts":[{"part":0,"role":"supersaw lead"}]}"#), "orchestral");
        if mid.tracks[0].program.map_or(false, |p| p < 80) {
            assert_eq!(wrong[0].plugin, "auto", "{}", wrong[0].why);
        }
        // a four-on-the-floor file at 128 BPM guesses house
        let kick: Vec<Note> = (0..64).map(|b| Note::new(b as f64, 0.1, 36, 0.9)).collect();
        let m2 = ImportedMidi { tracks: vec![part("Kit", None, kick)], bpm: Some(128.0), tempos: vec![], length_beats: 64.0 };
        assert_eq!(guess_genre(&m2), "house");
        assert!(fl_plugin_hint("Sytrus").contains("Prism"));
    }
}
