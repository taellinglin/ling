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
    ("feel", "play the whole song with an emotional arc: dynamics, articulation, room for the lead, level and filter rides per section (arg: release | slow burn | bittersweet | triumph | melancholy | storm | lullaby, or words like 'sadder', 'calmer')", "song"),
    ("tempo", "set the tempo (arg: bpm)", "song"),
    ("tunecheck", "check every instrument is in tune", "song"),
    ("mix", "open AI Mix & Master", "song"),
    ("undo", "undo the last change", "song"),
    ("repeat_section", "repeat / duplicate a section (arg: the words, e.g. 'repeat the second chorus twice', 'duplicate bars 9 to 16'); everything after it, the automation and the tags move down", "song"),
    ("remove_section", "remove a section and close the gap (arg: e.g. 'remove the intro', 'cut the bridge')", "song"),
    ("swap_sections", "swap two sections, or move one before / after another (arg: e.g. 'swap the verse and the chorus', 'move the bridge before the last chorus')", "song"),
    ("resize_section", "extend (the material loops) or shorten a section (arg: e.g. 'extend the intro by 4 bars', 'shorten the verse to 8 bars')", "song"),
    ("add_build", "insert a build (8 bars by default) before the next drop / chorus: the material before it thinned, a snare roll, rising velocity and filter (arg: e.g. 'add an 8 bar build before the second drop')", "song"),
    ("add_breakdown", "make a breakdown: drums and bass out, pads carry it (arg: e.g. 'make the bridge a breakdown', 'add a breakdown before the drop')", "song"),
    ("section_tempo", "set the tempo of one section, or from a bar (arg: e.g. 'set the bridge to 90 bpm', 'from bar 17 go to 150 bpm', 'slow the outro 10%')", "song"),
    ("drum_feel", "half-time or double-time feel for the drums, in a section or everywhere (arg: e.g. 'half-time drums in the bridge', 'double-time the last chorus')", "song"),
    ("layer", "layer a harmony variant of the melody on a new track: thirds, sixths, fifths, octave double or a choir pad (arg: e.g. 'layer sixths below', 'add an octave double', 'choir layer')", "track"),
    ("synth", "change named synth parameters by words on the selected track: longer release, shorter decay, more drive, more cutoff resonance, wider (arg: words). Never pitch parameters unless asked", "track"),
    ("shape", "draw automation by words on the track over a section: filter sweep up over the build, volume swell, pump (sidechain feel), fade out, open the filter in the drop (arg: words)", "track"),
    ("part", "take a part out of a section, bring it back, or solo it (arg: e.g. 'drop the bass out of the second verse', 'solo the lead in the bridge')", "song"),
    ("gesture", "write per-note pitch gestures on a track's notes, in a section or everywhere: scoops into the chords, slides between notes / 808 glides, falls at phrase ends, dives, a pitch riser on the last note (arg: the user's words, e.g. 'scoop into the chords in the drop', 'add 808 glides to the bass'). Only when the user names a scoop, slide, glide, fall, dive, bend or pitch riser", "track"),
    ("recipe", "put a sound recipe on a track: an EDM production sound with its instrument, effect chain, mix and how it is played (arg: the user's words, e.g. 'make the bass a riddim growl', 'use a reese', 'give the lead a supersaw recipe and write the part', 'list recipes')", "track"),
];

/// the tools that are `daw_arrx_run` in the engine (the app calls one builtin for all of them)
pub fn is_arrangement_tool(tool: &str) -> bool {
    matches!(tool, "repeat_section" | "remove_section" | "swap_sections" | "move_section" | "resize_section" | "add_build" | "add_breakdown" | "section_tempo" | "drum_feel" | "layer" | "synth" | "shape" | "part" | "gesture")
}

#[derive(Clone, Debug, PartialEq)]
pub struct Step {
    pub tool: String,
    pub arg: String,
}

fn step(tool: &str, arg: &str) -> Step {
    Step { tool: tool.into(), arg: arg.trim().into() }
}

/// Does a request ask for a sound recipe? It names recipes, or it asks to make / use / give a part a sound a recipe is named for
/// ("a riddim growl", "a reese", "a supersaw"). `p` is the request lowercased with spaces around.
fn recipe_request(p: &str) -> bool {
    if p.contains(" recipe ") || p.contains(" recipes ") {
        return true;
    }
    let asks = [" use a ", " use an ", " use the ", " make the ", " give the ", " turn the ", " put a ", " switch the ", " swap the "].iter().any(|w| p.contains(w));
    let part = [" bass ", " lead ", " pad ", " chords ", " chord ", " arp ", " pluck ", " sub ", " riser ", " it "].iter().any(|w| p.contains(w));
    // the sound words of the recipes (not genre words: "make it more dubstep" restyles the song)
    let sound = ["riddim growl", " growl ", " reese ", " wobble ", " supersaw ", " screech ", " 808 ", " sine sub ", " colour bass ", " color bass ", " noise sweep ", " pluck arp ", " chord stab ", " stabs "].iter().any(|w| p.contains(w));
    asks && sound && (part || p.contains(" use "))
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
    // ── sound recipes: "list recipes", "make the bass a riddim growl", "use a reese", "give the lead a supersaw recipe" ──
    if recipe_request(&p) {
        return vec![step("recipe", request)];
    }
    // ── the arrangement, layers, named synth parameters, automation shapes, parts in sections ──
    let sec_word = any(&["verse", "verses", "chorus", "choruses", "hook", "drop", "drops", "bridge", "intro", "outro", "breakdown", "section", "sections", "bars", "bar", "refrain", "ending"]);
    let tag_word = any(&["verse", "verses", "chorus", "choruses", "hook", "drop", "drops", "bridge", "intro", "outro", "breakdown", "refrain", "ending"]);
    let track_word = any(&["bass", "drums", "drum", "lead", "pad", "pads", "chords", "arp", "melody", "vocals", "vocal", "kick", "keys", "strings", "guitar", "piano"]);
    let adds = any(&["add", "insert", "make", "put", "write", "give", "create"]);
    let automating = any(&["automate", "automation"]);
    let part_out = track_word && sec_word && (any(&["out", "mute", "unmute", "silence", "solo", "only", "remove", "delete", "cut", "mute the"]) || (has("bring") && (has("back") || has("in"))));
    let feel = any(&["half-time", "half time", "halftime", "double-time", "double time", "doubletime"]);
    let sect_tempo = (any(&["tempo", "bpm"]) && (tag_word || any(&["from bar", "from beat"]))) || (tag_word && any(&["slow down", "speed up", "slower", "faster"]) && !track_word && !automating);
    let mut arr = false;
    if part_out {
        out.push(step("part", request));
        arr = true;
    } else if feel {
        out.push(step("drum_feel", request));
        arr = true;
    } else if (has("build") || any(&["buildup", "build-up", "build up"])) && adds && !automating && !any(&["filter", "volume"]) {
        out.push(step("add_build", request));
        arr = true;
    } else if has("breakdown") && adds && !part_out {
        out.push(step("add_breakdown", request));
        arr = true;
    } else if sec_word && any(&["swap", "switch", "reorder", "exchange"]) {
        out.push(step("swap_sections", request));
        arr = true;
    } else if tag_word && has("move") && any(&["before", "after"]) {
        out.push(step("move_section", request));
        arr = true;
    } else if sec_word && any(&["repeat", "duplicate", "twice", "again"]) && !track_word {
        out.push(step("repeat_section", request));
        arr = true;
    } else if tag_word && any(&["extend", "lengthen", "shorten", "trim"]) || (tag_word && has("make") && has("bars") && !any(&["build", "tempo", "bpm"])) {
        out.push(step("resize_section", request));
        arr = true;
    } else if sec_word && any(&["remove", "delete", "cut", "skip", "get rid of", "lose"]) && !track_word {
        out.push(step("remove_section", request));
        arr = true;
    }
    // a per-note pitch gesture named on purpose (scoop, slide / glide, fall, dive, pitch riser): the pitch guard's one way in
    let gest = !arr && crate::redesign::gesture_request(request).is_some() && !any(&["glide time", "portamento time", "more glide", "less glide", "longer glide", "shorter glide"]);
    if gest {
        out.push(step("gesture", request));
    }
    let shape = !arr && !gest && any(&["filter sweep", "sweep up", "sweep down", "open the filter", "open up the filter", "close the filter", "filter open", "volume swell", "swell", "crescendo", "fade out", "fade in", "fade-out", "fadeout", "pump", "pumping", "sidechain", "side-chain", "riser", "muffle", "filter up", "filter down"]);
    if shape {
        out.push(step("shape", request));
    }
    let layer = !arr && (any(&["layer", "layered", "layering", "octave double", "double the melody", "double the lead", "octave up", "octave below", "fifths", "power chords", "choir layer", "choir pad", "pad layer", "stack"]) || (has("octave") && any(&["double", "doubling", "add"])));
    if layer {
        out.push(step("layer", request));
    }
    let named = if arr || shape || layer || gest { vec![] } else { crate::redesign::param_words(request) };
    if !named.is_empty() {
        out.push(step("synth", request));
    }
    // tempo
    let words: Vec<&str> = p.split_whitespace().collect();
    if sect_tempo && !arr {
        out.push(step("section_tempo", request));
    } else {
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
    }
    if !layer && any(&["harmony", "harmonies", "harmonize", "harmonise", "harmonized", "thirds", "sixths", "backing vocals"]) {
        out.push(step("harmony", request));
    }
    let parts = [("bass", &["bassline", "bass line", "a bass", "add bass", "bass part"][..]), ("chords", &["chords", "chord part", "comping", "stabs"][..]), ("pad", &["a pad", "pads", "add pad", "strings"][..]), ("arp", &["arp", "arpeggio", "arpeggios"][..]), ("melody", &["counter melody", "countermelody", "a melody", "add melody", "lead line"][..]), ("drums", &["drums", "a beat", "drum part", "percussion"][..])];
    let mut wrote_part = false;
    for (part, ws) in parts {
        if any(ws) && !has("no") && !(part_out || feel || shape || layer || gest) {
            out.push(step("accompany", &format!("{part} {request}")));
            wrote_part = true;
        }
    }
    if !wrote_part && !(part_out || feel || shape || layer || gest) && any(&["accompaniment", "accompany", "accompanying", "backing"]) {
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
    if !shape && any(&["automate", "automation", "filter sweep", "sweeps", "movement in the"]) {
        out.push(step("automate", request));
    }
    let redo = !(arr || shape || layer || gest) && any(&["redo", "re-do", "new instrument", "different instrument", "change the instrument", "instrumentation", "redesign", "re-design", "another sound", "new sound", "different sound", "new patch", "swap the"]);
    let genre = if arr || shape || layer || gest || !named.is_empty() || sect_tempo { None } else { prompt_genre(request).map(|g| g.name) };
    let (mv, _) = crate::redesign::moves(request);
    if let Some(g) = genre {
        if any(&["make it", "sound like", "sound more", "more", "into", "turn it", "style", "restyle", "vibe", "feel"]) || out.is_empty() {
            // "make it ..." is the song; "this", "this track", "the patch" is the selection
            let mine = has_track && any(&["this", "track", "patch"]);
            out.push(step(if mine { "redesign" } else { "restyle" }, if mine { request } else { g }));
        }
    } else if redo {
        out.push(step("redesign", request));
    } else if !mv.is_empty() && named.is_empty() && !arr && !shape && !gest {
        out.push(step("tweak", request));
    }
    // the feel: words that name an emotion or an arc (not a sound: "darker" / "brighter" alone tweak a patch)
    if any(&["sadder", "happier", "melancholy", "melancholic", "triumphant", "triumph", "emotional", "emotion", "mood", "moodier", "calmer", "stormy", "angrier", "bittersweet", "lullaby", "dreamier", "heroic", "epic", "slow burn", "more intense", "less intense", "nostalgic", "uplifting", "heartbreaking"]) {
        let arg = crate::feel::arc_from_words(request).unwrap_or("auto");
        out.push(step("feel", arg));
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
    let mut s = String::from("You edit a music project in LingStationX. Choose the tools that do what the user asks, in order. Reply with JSON only: a list like [{\"tool\": \"harmony\", \"arg\": \"thirds above\"}]. For the section tools the arg is the user's own words. Never touch pitch (vibrato, sweep, glide, detune, bend) unless the user asks for pitch, a bend or a riser. Tools:\n");
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
        assert_eq!(route("make it sadder", false, false), vec![step("feel", "melancholy")]);
        assert_eq!(route("give the song a triumphant mood", false, false), vec![step("feel", "triumph")]);
        assert!(!tools(&route("make this patch feel grittier", false, true)).contains(&"feel"));
        let acc = route("give it some accompaniment", true, true);
        assert_eq!(tools(&acc), vec!["accompany", "accompany"]);
        assert!(acc[0].arg.starts_with("chords") && acc[1].arg.starts_with("bass"));
        assert_eq!(tools(&route("add a bassline and an arp", true, true)), vec!["accompany", "accompany"]);
        assert_eq!(route("set the tempo to 140 bpm", false, false), vec![step("tempo", "140")]);
        assert_eq!(tools(&route("undo that", false, false)), vec!["undo"]);
        assert_eq!(tools(&route("riddim it", false, true)), vec!["skin"]);
        assert_eq!(route("riddim it", false, true)[0].arg, "riddim");
        assert!(route("what a lovely day", false, false).is_empty());
        // sound recipes
        for r in ["make the bass a riddim growl", "use a reese", "give the lead a supersaw recipe", "list recipes", "use a wobble on the bass", "show me the sound recipes"] {
            assert_eq!(tools(&route(r, false, true)), vec!["recipe"], "{r}");
        }
        assert_eq!(route("use a reese", false, true)[0].arg, "use a reese");
        // genre words still restyle, "riddim it" still skins
        assert_eq!(tools(&route("make it sound more dubstep", false, false)), vec!["restyle"]);
        assert_eq!(tools(&route("riddim it", false, true)), vec!["skin"]);
        assert!(TOOLS.iter().any(|t| t.0 == "recipe") && system().contains("recipe"));
    }

    #[test]
    fn named_pitch_gestures_reach_the_gesture_tool_only() {
        for r in ["scoop into the chords in the drop", "add 808 glides to the bass", "make the screech dive at the end of the fill", "let the lead notes fall off at the end of phrases", "slide between the bass notes", "put a pitch riser on the last note"] {
            assert_eq!(tools(&route(r, true, true)), vec!["gesture"], "{r}");
        }
        // a riser alone is still the automation riser; sound words never write gestures
        assert!(!tools(&route("add a riser before the drop", true, true)).contains(&"gesture"));
        for r in ["make this patch grittier", "make it sound more dubstep", "longer release"] {
            assert!(!tools(&route(r, true, true)).contains(&"gesture"), "{r}");
        }
        assert!(is_arrangement_tool("gesture") && TOOLS.iter().any(|t| t.0 == "gesture"));
    }

    #[test]
    fn arrangement_phrases_reach_their_tools() {
        let one = |r: &str| -> Vec<String> { route(r, false, true).iter().map(|s| s.tool.clone()).collect() };
        assert_eq!(one("repeat the second chorus twice"), vec!["repeat_section"]);
        assert_eq!(one("duplicate the last verse"), vec!["repeat_section"]);
        assert_eq!(one("remove the intro"), vec!["remove_section"]);
        assert_eq!(one("cut the bridge"), vec!["remove_section"]);
        assert_eq!(one("swap the verse and the chorus"), vec!["swap_sections"]);
        assert_eq!(one("move the bridge before the last chorus"), vec!["move_section"]);
        assert_eq!(one("extend the intro by 4 bars"), vec!["resize_section"]);
        assert_eq!(one("shorten the verse to 8 bars"), vec!["resize_section"]);
        assert_eq!(one("add an 8 bar build before the second drop"), vec!["add_build"]);
        assert_eq!(one("make a breakdown in the bridge"), vec!["add_breakdown"]);
        assert_eq!(one("set the bridge to 90 bpm"), vec!["section_tempo"]);
        assert_eq!(one("from bar 17 go to 150 bpm"), vec!["section_tempo"]);
        assert_eq!(one("slow down the outro"), vec!["section_tempo"]);
        assert_eq!(one("half-time drums in the bridge"), vec!["drum_feel"]);
        assert_eq!(one("double time the last chorus"), vec!["drum_feel"]);
        assert_eq!(one("drop the bass out of the second verse"), vec!["part"]);
        assert_eq!(one("take the drums out of the intro"), vec!["part"]);
        assert_eq!(one("solo the lead in the bridge"), vec!["part"]);
        assert_eq!(one("layer sixths below the melody"), vec!["layer"]);
        assert_eq!(one("add an octave double"), vec!["layer"]);
        assert_eq!(one("add a choir pad layer"), vec!["layer"]);
        assert_eq!(one("longer release and shorter decay"), vec!["synth"]);
        assert_eq!(one("more drive and more cutoff resonance"), vec!["synth"]);
        assert_eq!(one("filter sweep up over the build"), vec!["shape"]);
        assert_eq!(one("fade out the pad"), vec!["shape"]);
        assert_eq!(one("add a volume swell into the drop"), vec!["shape"]);
        assert_eq!(one("sidechain pump on the chords"), vec!["shape"]);
        assert_eq!(one("open the filter in the drop"), vec!["shape"]);
        // the old phrases still go where they went
        assert_eq!(one("add a harmony in thirds"), vec!["harmony"]);
        assert_eq!(one("add a bassline and an arp"), vec!["accompany", "accompany"]);
        assert_eq!(one("make this patch grittier"), vec!["tweak"]);
        assert_eq!(one("automate a build"), vec!["automate"]);
        assert_eq!(one("set the tempo to 140 bpm"), vec!["tempo"]);
        for t in ["repeat_section", "remove_section", "swap_sections", "resize_section", "add_build", "add_breakdown", "section_tempo", "drum_feel", "layer", "synth", "shape", "part"] {
            assert!(TOOLS.iter().any(|x| x.0 == t), "{t} is listed for the model");
        }
        assert!(system().contains("Never touch pitch"));
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
