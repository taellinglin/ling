//! The project agent: a request in plain words ("add a harmony", "make it sound more dubstep", "make this patch grittier",
//! "redo the instrumentation of this track") becomes a list of steps, each one a tool the app already has. A language model can
//! pick the steps (its JSON reply is read by `parse_plan`, every tool checked against the list); without one, or when its reply is
//! no use, `route` reads the words itself.
use crate::songgen::prompt_genre;

/// (tool, what it does - for the model and the help text, what it needs: "clip", "track" or "song")
pub const TOOLS: &[(&str, &str, &str)] = &[
    ("harmony", "add a harmonized part under the selected notes / clip (arg: how, e.g. 'thirds above', 'choir')", "clip"),
    ("accompany", "write a new accompanying part on a new track over the selected clip (arg: bass | chords | pad | arp | melody | drums, plus how)", "clip"),
    ("redesign", "redo the instrumentation and sound design of the track: a fitting preset, reshaped by the words, a new effect chain (arg: words)", "track"),
    ("tweak", "change the selected track's patch by words: grittier, darker, wider, punchier ... (arg: words)", "track"),
    ("restyle", "make the song (or the selected track) sound like a genre: patches, effect chains, Growl skin on basses (arg: genre)", "song"),
    ("skin", "re-perform a Ling Growl track as a dubstep bass: gating, envelopes, risers (arg: riddim | brostep | neuro | melodic | auto)", "track"),
    ("automate", "write automation lanes on the track's parameters (arg: how, e.g. 'tense build into the drop')", "track"),
    ("decorate", "dress up the selected clip in the key: passing tones, turns, voice leading", "clip"),
    ("variations", "write theme and variations after the selected clip", "clip"),
    ("tempo", "set the tempo (arg: bpm)", "song"),
    ("tunecheck", "check every instrument is in tune", "song"),
    ("mix", "open AI Mix & Master", "song"),
    ("undo", "undo the last change", "song"),
];

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub tool: String,
    pub arg: String,
}

fn step(tool: &str, arg: &str) -> Step {
    Step { tool: tool.into(), arg: arg.trim().into() }
}

/// Read a request into steps (in the order they should run). `has_clip` / `has_track`: what is selected.
pub fn route(request: &str, has_clip: bool, has_track: bool) -> Vec<Step> {
    let p = format!(" {} ", request.to_lowercase().replace([',', '.', '!', '?', ';', ':'], " "));
    let has = |w: &str| p.contains(&format!(" {w} ")) || (w.contains(' ') && p.contains(w));
    let any = |ws: &[&str]| ws.iter().any(|w| has(w));
    let mut out: Vec<Step> = vec![];
    if any(&["undo", "revert", "go back", "take that back"]) {
        return vec![step("undo", "")];
    }
    // tempo
    let words: Vec<&str> = p.split_whitespace().collect();
    for (i, w) in words.iter().enumerate() {
        let num = w.trim_end_matches("bpm");
        if let Ok(v) = num.parse::<f64>() {
            let tempo_word = w.ends_with("bpm") || words.get(i + 1).map(|n| n.starts_with("bpm")).unwrap_or(false) || any(&["tempo"]);
            if tempo_word && (40.0..=240.0).contains(&v) {
                out.push(step("tempo", &format!("{v}")));
            }
        }
    }
    if out.is_empty() && has("tempo") && any(&["faster", "speed up", "up"]) {
        out.push(step("tempo", "+8%"));
    } else if out.is_empty() && has("tempo") && any(&["slower", "slow down", "down"]) {
        out.push(step("tempo", "-8%"));
    }
    if any(&["harmony", "harmonies", "harmonize", "harmonise", "harmonized", "thirds", "sixths", "backing vocals"]) {
        out.push(step("harmony", request));
    }
    let parts = [("bass", &["bassline", "bass line", "a bass", "add bass", "bass part"][..]), ("chords", &["chords", "chord part", "comping", "stabs"][..]), ("pad", &["a pad", "pads", "add pad", "strings"][..]), ("arp", &["arp", "arpeggio", "arpeggios"][..]), ("melody", &["counter melody", "countermelody", "a melody", "add melody", "lead line"][..]), ("drums", &["drums", "a beat", "drum part", "percussion"][..])];
    let mut wrote_part = false;
    for (part, ws) in parts {
        if any(ws) && !has("no") {
            out.push(step("accompany", &format!("{part} {request}")));
            wrote_part = true;
        }
    }
    if !wrote_part && any(&["accompaniment", "accompany", "accompanying", "backing"]) {
        out.push(step("accompany", &format!("chords {request}")));
        out.push(step("accompany", &format!("bass {request}")));
    }
    if any(&["decorate", "ornament", "ornaments", "embellish", "fancier", "busier melody"]) {
        out.push(step("decorate", ""));
    }
    if any(&["variations", "variation", "theme and variations", "develop the theme"]) {
        out.push(step("variations", ""));
    }
    if any(&["growl technique", "skin it", "chop it", "gate it", "gated", "more percussive", "riddim it"]) {
        let style = ["riddim", "brostep", "neuro", "melodic"].iter().find(|s| has(s)).copied().unwrap_or("auto");
        out.push(step("skin", style));
    }
    if any(&["automate", "automation", "filter sweep", "sweeps", "movement in the"]) {
        out.push(step("automate", request));
    }
    let redo = any(&["redo", "re-do", "new instrument", "different instrument", "change the instrument", "instrumentation", "redesign", "re-design", "another sound", "new sound", "different sound", "new patch", "swap the"]);
    let genre = prompt_genre(request).map(|g| g.name);
    let (mv, _) = crate::redesign::moves(request);
    if let Some(g) = genre {
        if any(&["make it", "sound like", "sound more", "more", "into", "turn it", "style", "restyle", "vibe", "feel"]) || out.is_empty() {
            // "make it ..." is the song; "this", "this track", "the patch" is the selection
            let mine = has_track && any(&["this", "track", "patch"]);
            out.push(step(if mine { "redesign" } else { "restyle" }, if mine { request } else { g }));
        }
    } else if redo {
        out.push(step("redesign", request));
    } else if !mv.is_empty() {
        out.push(step("tweak", request));
    }
    if any(&["tune check", "in tune", "out of tune", "detuned"]) {
        out.push(step("tunecheck", ""));
    }
    if any(&["mix it", "master it", "mixdown", "mastering", "mix and master", "louder"]) {
        out.push(step("mix", ""));
    }
    // what needs a selection: say so instead of guessing
    let _ = (has_clip, has_track);
    out
}

/// A model's reply (`[{"tool": "...", "arg": "..."}]`, tolerant of prose and fences) -> the steps it named that exist.
pub fn parse_plan(reply: &str) -> Vec<Step> {
    let Some(a) = reply.find('[') else { return vec![] };
    let Some(b) = reply.rfind(']') else { return vec![] };
    if b <= a {
        return vec![];
    }
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&reply[a..=b]) else { return vec![] };
    let Some(arr) = v.as_array() else { return vec![] };
    arr.iter()
        .filter_map(|o| {
            let tool = o.get("tool")?.as_str()?.trim().to_lowercase();
            if !TOOLS.iter().any(|t| t.0 == tool) {
                return None;
            }
            let arg = match o.get("arg") {
                Some(serde_json::Value::String(s)) => s.clone(),
                Some(serde_json::Value::Number(n)) => n.to_string(),
                _ => String::new(),
            };
            Some(Step { tool, arg })
        })
        .take(6)
        .collect()
}

/// The system prompt for a model choosing the steps
pub fn system() -> String {
    let mut s = String::from("You edit a music project in LingStationX. Choose the tools that do what the user asks, in order. Reply with JSON only: a list like [{\"tool\": \"harmony\", \"arg\": \"thirds above\"}]. Tools:\n");
    for (t, d, needs) in TOOLS {
        s.push_str(&format!("- {t} (needs a selected {needs}): {d}\n"));
    }
    s.push_str("Use as few tools as the request needs. If nothing fits, reply [].");
    s
}

/// what the selection is, for the model: "selected: track 'Growl Bass' (growl), clip 'drop' (16 bars)"
pub fn describe_steps(steps: &[Step]) -> String {
    steps.iter().map(|s| if s.arg.is_empty() { s.tool.clone() } else { format!("{} ({})", s.tool, s.arg) }).collect::<Vec<_>>().join(", then ")
}

#[cfg(test)]
mod tests {
    use super::*;
    fn tools(v: &[Step]) -> Vec<&str> {
        v.iter().map(|s| s.tool.as_str()).collect()
    }

    #[test]
    fn requests_become_steps() {
        assert_eq!(tools(&route("add a harmony in thirds", true, true)), vec!["harmony"]);
        assert_eq!(tools(&route("make it sound more dubstep", false, false)), vec!["restyle"]);
        assert_eq!(route("make it sound more dubstep", false, false)[0].arg, "dubstep");
        // with a track selected, "it" is still the song, "this track" is the track
        assert_eq!(tools(&route("make it sound more dubstep", true, true)), vec!["restyle"]);
        assert_eq!(tools(&route("make this track more dubstep", true, true)), vec!["redesign"]);
        assert_eq!(tools(&route("make this patch grittier", false, true)), vec!["tweak"]);
        assert_eq!(tools(&route("redo the instrumentation of this track", false, true)), vec!["redesign"]);
        let acc = route("give it some accompaniment", true, true);
        assert_eq!(tools(&acc), vec!["accompany", "accompany"]);
        assert!(acc[0].arg.starts_with("chords") && acc[1].arg.starts_with("bass"));
        assert_eq!(tools(&route("add a bassline and an arp", true, true)), vec!["accompany", "accompany"]);
        assert_eq!(route("set the tempo to 140 bpm", false, false), vec![step("tempo", "140")]);
        assert_eq!(tools(&route("undo that", false, false)), vec!["undo"]);
        assert_eq!(tools(&route("riddim it", false, true)), vec!["skin"]);
        assert_eq!(route("riddim it", false, true)[0].arg, "riddim");
        assert!(route("what a lovely day", false, false).is_empty());
    }

    #[test]
    fn a_models_plan_is_read_and_checked() {
        let r = "Sure!\n```json\n[{\"tool\": \"tweak\", \"arg\": \"grittier\"}, {\"tool\": \"launch_rockets\"}, {\"tool\": \"tempo\", \"arg\": 150}]\n```";
        let p = parse_plan(r);
        assert_eq!(p, vec![step("tweak", "grittier"), step("tempo", "150")]);
        assert!(parse_plan("no json").is_empty());
        assert!(system().contains("harmony") && system().contains("restyle"));
    }
}
