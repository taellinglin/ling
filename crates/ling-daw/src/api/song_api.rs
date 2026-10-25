//! File > New AI Song (`daw_song_*`): a description (genre, tempo, key, chords, structure, instruments, singer), lyrics and an optional
//! reference recording become a whole song: instruments chosen for the genre, a phrase per section for every part (repeated sections
//! repeat), drum fills, a Ling Voice singing the lyrics, filter / pump automation and articulation, section tags, then a mix target the
//! app hands to Mix AI. The plan itself is `songgen.rs`; this file builds it with the same builtins the app uses.
use super::{Daw, A, Val};
use crate::compose::Kind;
use crate::model::*;
use crate::songgen::{self, Form, RefInfo, SongPlan};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

/// references already measured, by path (a song can take a few seconds)
fn refs() -> &'static Mutex<HashMap<String, RefInfo>> {
    static R: OnceLock<Mutex<HashMap<String, RefInfo>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// guide MIDI files already read, by path
fn guides() -> &'static Mutex<HashMap<String, std::sync::Arc<crate::guide::Guide>>> {
    static G: OnceLock<Mutex<HashMap<String, std::sync::Arc<crate::guide::Guide>>>> = OnceLock::new();
    G.get_or_init(|| Mutex::new(HashMap::new()))
}

/// the guide for a MIDI path ("" or unreadable = none)
fn load_guide(path: &str) -> Option<std::sync::Arc<crate::guide::Guide>> {
    if path.trim().is_empty() {
        return None;
    }
    if let Some(g) = guides().lock().unwrap_or_else(|p| p.into_inner()).get(path) {
        return Some(g.clone());
    }
    let data = std::fs::read(path).ok()?;
    let mid = crate::midi_import::parse_any(path, &data).ok()?;
    let g = std::sync::Arc::new(crate::guide::analyze(&mid)?);
    guides().lock().unwrap_or_else(|p| p.into_inner()).insert(path.to_string(), g.clone());
    Some(g)
}

/// patch files of a plugin kind (the same folders the preset chooser shows): (clean name, path)
fn patch_files(kind: &str) -> Vec<(String, String)> {
    let dirs: &[(&str, &str)] = match kind {
        "prism" => &[("instruments/Prism", "")],
        "growl" => &[("instruments/Growl", "")],
        "chip" => &[("instruments/Chip", "")],
        "voice" => &[("instruments/Voice", "")],
        "drums" => &[("instruments", "")],
        _ => &[("instruments/Electronic GM", "Electronic: "), ("instruments/GM", "GM: "), ("instruments", "")],
    };
    let mut out = vec![];
    for (d, prefix) in dirs {
        let Ok(rd) = std::fs::read_dir(d) else { continue };
        let mut files: Vec<PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "ling").unwrap_or(false)).collect();
        files.sort();
        for p in files {
            let text = std::fs::read_to_string(&p).unwrap_or_default();
            let ptype = text.lines().find_map(|l| l.trim().strip_prefix("type").map(|r| r.trim_start_matches([' ', '=']).trim().split_whitespace().next().unwrap_or("").to_string())).unwrap_or_default();
            if *d == "instruments" && ptype != if kind == "drums" { "drums" } else { "synth" } {
                continue;
            }
            // the name inside the file, else the file name without its number / kind prefix
            let stem = p.file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
            let stem = stem.trim_start_matches(|c: char| c.is_ascii_digit()).trim();
            let stem = ["Growl ", "Voice ", "Prism ", "Chip "].iter().fold(stem.to_string(), |s, pre| s.strip_prefix(pre).map(|x| x.to_string()).unwrap_or(s));
            let inner = text.lines().find_map(|l| l.trim().strip_prefix("name").and_then(|r| r.split_once('"')).and_then(|(_, r)| r.split_once('"')).map(|(n, _)| n.to_string()));
            out.push((format!("{prefix}{}", stem), p.to_string_lossy().into_owned()));
            if let Some(n) = inner {
                out.push((format!("{prefix}{n}"), p.to_string_lossy().into_owned()));
            }
        }
    }
    out
}

/// the file of a patch the role tables name ("Electronic: EDM Grand", "Kit House 909", "wobble")
pub(super) fn find_patch(kind: &str, name: &str) -> Option<String> {
    let want = name.to_lowercase();
    let files = patch_files(kind);
    files.iter().find(|(n, _)| n.to_lowercase() == want).or_else(|| files.iter().find(|(n, _)| n.to_lowercase().ends_with(&want) || want.ends_with(&n.to_lowercase()))).map(|(_, p)| p.clone())
}

/// General MIDI program of a role, for the acoustic parts of orchestral / folk / jazz songs
fn gm_program(role: &str) -> u8 {
    match role {
        "strings" => 48,
        "brass" => 61,
        "woodwind" => 73,
        "choir" => 52,
        "piano" => 0,
        "keys" => 4,
        "guitar" => 24,
        "organ" => 19,
        "bell" => 14,
        _ => 0,
    }
}

/// (plugin kind, patch file) for a role in a genre
fn gm_file(program: u8) -> Option<String> {
    let pre = format!("{program:03} ");
    std::fs::read_dir("instruments/GM").ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().map(|f| f.to_string_lossy().starts_with(&pre)).unwrap_or(false))).map(|p| p.to_string_lossy().into_owned())
}

fn instrument_for(g: &songgen::Genre, role: &str, part: usize) -> (String, Option<String>) {
    if let Some(inst) = crate::ensemble::instrument(role) {
        if inst.kind == Kind::Drums {
            return ("drums".into(), find_patch("drums", "Kit Afro Percussion").or_else(|| find_patch("drums", g.kit)));
        }
        return ("synth".into(), gm_file(inst.gm));
    }
    if role == "drums" {
        if let Some(p) = find_patch("drums", g.kit) {
            return ("drums".into(), Some(p));
        }
        return ("drums".into(), None);
    }
    // the import tables know which patches suit a genre's role
    let ai_genre = match g.name {
        "hip hop" | "garage" | "trap" | "reggaeton" => "pop",
        "afrobeat" | "dub" => "disco",
        other => other,
    };
    // only patches that play in tune and do not clash with the chords (see soundcat.rs)
    let (plugin, patch) = crate::ai_import::pick(ai_genre, role, part, g.bpm);
    if plugin == "auto" {
        let pre = format!("{:03} ", gm_program(role) + 1);
        let gm = std::fs::read_dir("instruments/GM").ok().and_then(|rd| rd.flatten().map(|e| e.path()).find(|p| p.file_name().map(|f| f.to_string_lossy().starts_with(&pre)).unwrap_or(false)));
        return ("synth".into(), gm.map(|p| p.to_string_lossy().into_owned()));
    }
    let kind = if plugin == "drums" { "drums" } else { plugin };
    (kind.to_string(), find_patch(kind, patch))
}

/// an instrument built from a (kind, patch file) choice, for the tune check
fn build_choice(kind: &str, patch: &Option<String>, sr: f32) -> Option<Box<dyn crate::dsp::Instrument>> {
    match patch {
        Some(path) => {
            let text = std::fs::read_to_string(path).ok()?;
            crate::patch::build(&crate::patch::parse(&text), sr)
        }
        None => crate::dsp::make_instrument(kind, sr),
    }
}

/// The instrument for a part, tune-checked: the role's candidates in order (then a General MIDI sound for the role), the first that
/// plays in tune with 12-tone equal temperament. Drums, risers, voices and unpitched sounds are not judged. The second value says
/// what was swapped out and why ("" when the first choice was fine).
fn instrument_in_tune(g: &songgen::Genre, role: &str, part: usize, sr: f32) -> ((String, Option<String>), String) {
    let first = instrument_for(g, role, part);
    let kind = songgen::role_kind(role);
    if kind == Kind::Drums || role == "riser" || role == "voice" || first.0 == "voice" || first.0 == "drums" {
        return (first, String::new());
    }
    let mut cands: Vec<(String, Option<String>)> = vec![first.clone()];
    for j in 1..5 {
        let c = instrument_for(g, role, part + j);
        if !cands.contains(&c) {
            cands.push(c);
        }
    }
    let gm = ("synth".to_string(), gm_file(gm_program(role) + 1));
    if !cands.contains(&gm) {
        cands.push(gm);
    }
    let name = |c: &(String, Option<String>)| c.1.as_deref().and_then(|p| Path::new(p).file_stem()).map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_else(|| c.0.clone());
    let mut rejected: Vec<String> = vec![];
    for c in &cands {
        let Some(mut inst) = build_choice(&c.0, &c.1, sr) else { continue };
        let t = crate::tunecheck::check(inst.as_mut(), sr);
        if t.in_tune() {
            let why = if rejected.is_empty() { String::new() } else { format!("{}: {} out of tune, {} instead", title(role), rejected.join(", "), name(c)) };
            return (c.clone(), why);
        }
        rejected.push(format!("{} ({:+.0} cents)", name(c), t.cents));
    }
    (first, format!("{}: no candidate measured in tune ({})", title(role), rejected.join(", ")))
}

/// Put a part's planned chain on its track: effects it already has are reused (only re-set and automated), the rest added
fn apply_design(tr: &mut Track, pd: &crate::sounddesign::PartDesign, sr: f32) {
    for f in &pd.chain {
        let idx = match tr.effects.iter().position(|e| e.name == f.name) {
            Some(i) => i,
            None => {
                let Some(slot) = EffectSlot::new(f.name, sr) else { continue };
                tr.effects.push(slot);
                tr.effects.len() - 1
            }
        };
        for (k, v) in &f.params {
            if let Some(pi) = tr.effects[idx].params.iter().position(|p| p.0 == *k) {
                tr.effects[idx].set(pi, *v);
            }
        }
        for (k, pts) in &f.lanes {
            if let Some(pi) = tr.effects[idx].params.iter().position(|p| p.0 == *k) {
                let key = format!("fx{idx}.{pi}");
                tr.auto.retain(|l| l.param != key);
                tr.auto.push(AutoLane { param: key, points: pts.clone() });
            }
        }
    }
}

/// the project's sections ("sections" meta from the arranger tags), or the whole song as one
fn project_sections(en: &crate::engine::Engine) -> Vec<crate::sounddesign::Sec> {
    let mut v: Vec<crate::sounddesign::Sec> = vec![];
    if let Some((_, text)) = en.project.meta.iter().find(|(k, _)| k == "sections") {
        for item in text.split(';') {
            let f: Vec<&str> = item.split(':').collect();
            if f.len() >= 3 {
                if let (Ok(a), Ok(b)) = (f[0].parse::<f64>(), f[1].parse::<f64>()) {
                    v.push(crate::sounddesign::Sec { tag: f[2].to_string(), start: a, end: b });
                }
            }
        }
    }
    if v.is_empty() {
        v.push(crate::sounddesign::Sec { tag: "verse".into(), start: 0.0, end: en.project.end_beat().max(16.0) });
    }
    v
}

/// the tonic (0..11) a track's notes suggest, or of every MIDI note in the project
fn track_tonic(en: &crate::engine::Engine, t: usize) -> u8 {
    fn add(h: &mut [f64; 12], tr: &Track) {
        for c in &tr.clips {
            if let ClipData::Midi(ns) = &c.data {
                for n in ns {
                    h[(n.key % 12) as usize] += n.len.max(0.1);
                }
            }
        }
    }
    let mut h = [0f64; 12];
    if let Some(tr) = en.project.tracks.get(t) {
        add(&mut h, tr);
    }
    if h.iter().sum::<f64>() < 1.0 {
        for tr in &en.project.tracks {
            if tr.inst_kind != "drums" {
                add(&mut h, tr);
            }
        }
    }
    crate::theory::detect_key_hist(&h).0.tonic
}

fn title(role: &str) -> String {
    role.split(' ').map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

impl Daw {
    /// the notes of a clip "track:clip" as a theme (from the start of its first note, at most 8 bars); None for "" or no notes
    fn theme_of(&self, spec: &str) -> Option<std::sync::Arc<Vec<crate::theory::N>>> {
        let (t, c) = spec.split_once(':')?;
        let (t, c) = (t.trim().parse::<usize>().ok()?, c.trim().parse::<usize>().ok()?);
        let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        let clip = en.project.tracks.get(t)?.clips.get(c)?;
        let ClipData::Midi(notes) = &clip.data else { return None };
        let first = notes.iter().map(|n| n.start).fold(f64::INFINITY, f64::min);
        if !first.is_finite() {
            return None;
        }
        let base = (first / 4.0).floor() * 4.0;
        let v: Vec<crate::theory::N> = notes.iter().filter(|n| n.start - base < 32.0).map(|n| crate::theory::N { start: n.start - base, len: n.len, key: n.key, vel: n.vel }).collect();
        (!v.is_empty()).then(|| std::sync::Arc::new(v))
    }

    fn call_num(&mut self, name: &str, args: &[Val]) -> f64 {
        match self.dispatch(name, &A(args)) {
            Ok(Val::Num(x)) => x,
            _ => -1.0,
        }
    }

    pub(super) fn song_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_song_genres" => ok!(Val::List(songgen::GENRES.iter().map(|g| Val::Str(g.name.into())).collect())),
            "daw_song_reference" => {
                // (path) -> [description, bpm, key, chords, genre] or [] when it cannot be read
                let path = a.s(0);
                let cached = refs().lock().unwrap_or_else(|p| p.into_inner()).get(&path).cloned();
                let r = match cached {
                    Some(r) => r,
                    None => {
                        let s = match load_sample(&path) {
                            Ok(s) => s,
                            Err(e) => {
                                self.status = format!("Reference: {e}");
                                ok!(Val::List(vec![]))
                            }
                        };
                        let r = songgen::analyze_reference(&s.stereo, s.rate);
                        refs().lock().unwrap_or_else(|p| p.into_inner()).insert(path.clone(), r.clone());
                        r
                    }
                };
                ok!(Val::List(vec![Val::Str(songgen::describe_reference(&r)), Val::Num(r.bpm), Val::Str(r.key.map(|k| k.name()).unwrap_or_default()), Val::Str(r.prog.clone()), Val::Str(r.genre.clone())]))
            }
            "daw_song_guide" => {
                // (midi path) -> [description, bpm, key, chords, genre] or [] when it cannot be read / holds no notes
                match load_guide(&a.s(0)) {
                    Some(g) => ok!(Val::List(vec![Val::Str(g.describe()), Val::Num(g.bpm), Val::Str(g.key.name()), Val::Str(g.prog.clone()), Val::Str(g.genre.to_string())])),
                    None => {
                        self.status = "Guide: that file has no notes I can read".into();
                        ok!(Val::List(vec![]))
                    }
                }
            }
            "daw_song_model_system" => ok!(format!("You plan songs for a music program. Reply with one JSON object only, no other text.
{}{}", crate::soundcat::digest(), crate::soundcat::role_notes())),
            "daw_song_model_prompt" => {
                // (description, lyrics) -> the question for the language model: fill in the plan's fields
                let genres: Vec<&str> = songgen::GENRES.iter().map(|g| g.name).collect();
                let roles = crate::ai_import::ROLES.join(", ");
                ok!(format!(
                    "Description: {}\n{}\nFill in this JSON for the song. genre: one of [{}]. bpm: a number. key: like \"F# minor\". progression: 4 chords for the verse that suit the genre and the mood, Roman numerals in the key (upper case major, lower case minor, b for a borrowed chord) like \"i VI III VII\". chorus_progression: 4 chords for the chorus, or \"\" when it uses the verse's. structure: sections with bars like \"intro 8, verse 16, chorus 8, outro 8\". instruments: 3 to 6 of [{}]. arc: how the song feels over time, one of [release, slow burn, bittersweet, triumph, melancholy, storm, lullaby]. feel: per kind of section how hard the vocal pushes (0 restrained .. 1 forceful) and its mood (-1 sad .. 1 happy), like [{{\"section\": \"verse\", \"vocal\": 0.3, \"valence\": -0.2}}, {{\"section\": \"chorus\", \"vocal\": 0.9, \"valence\": 0.3}}].\n{{\"genre\": \"\", \"bpm\": 0, \"key\": \"\", \"progression\": \"\", \"chorus_progression\": \"\", \"structure\": \"\", \"instruments\": \"\"}}",
                    a.s(0),
                    if a.s(1).trim().is_empty() { String::new() } else { format!("The song has lyrics ({} lines).", a.s(1).lines().count()) },
                    genres.join(", "),
                    roles
                ))
            }
            "daw_song_plan" => {
                // (description, lyrics, model reply or "", reference path or "", length 0.25..2, seed) -> [plan line, notes]
                let model = crate::compose::fields_from_json(&a.s(2));
                let r = if a.s(3).is_empty() { None } else { refs().lock().unwrap_or_else(|p| p.into_inner()).get(&a.s(3)).cloned() };
                // optional 7th and 8th: a guide MIDI path and how closely to follow it (0..1); 9th: the length rule
                // ("auto", "lyrics", "midi", "duration:3:30")
                let guide = load_guide(&a.s(6));
                let fit = songgen::Fit::parse(&a.s(8));
                // 10th: the story to tell; 11th: a theme clip "track:clip"
                let mut p = songgen::plan_story(&a.s(0), &a.s(1), &model, r.as_ref(), a.n(4).max(0.25), a.n(5).max(1.0) as u64, guide.as_ref(), a.n(7), &fit, &a.s(9));
                p.theme = self.theme_of(&a.s(10));
                if p.theme.is_some() {
                    p.notes.push("the lead plays the selected theme in variations".into());
                }
                let m = songgen::metrics(&p);
                ok!(Val::List(vec![Val::Str(p.to_line()), Val::Str(p.notes.join("; ")), Val::List(m.into_iter().map(Val::Num).collect())]))
            }
            "daw_song_agent_route" => {
                // (request, has_clip, has_track) -> [tool, arg, tool, arg ...]
                let steps = crate::agent::route(&a.s(0), a.n(1) > 0.5, a.n(2) > 0.5);
                ok!(Val::List(steps.into_iter().flat_map(|s| [Val::Str(s.tool), Val::Str(s.arg)]).collect()))
            }
            "daw_song_agent_parse" => {
                let steps = crate::agent::parse_plan(&a.s(0));
                ok!(Val::List(steps.into_iter().flat_map(|s| [Val::Str(s.tool), Val::Str(s.arg)]).collect()))
            }
            "daw_song_agent_system" => ok!(crate::agent::system()),
            "daw_song_genre" => {
                // () -> the project's genre (from New AI Track), "" when unknown
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                ok!(en.project.meta.iter().find(|(k, _)| k == "ai.genre").map(|(_, v)| v.clone()).unwrap_or_default())
            }
            "daw_song_tweak" => {
                // (track, words, amount 0..1) -> a sentence: the patch's parameters moved by what the words ask for (redesign.rs)
                let (mv, said) = crate::redesign::moves(&a.s(1));
                if mv.is_empty() {
                    ok!("No sound words in that (try grittier, darker, brighter, wider, punchier, more movement ...)".to_string());
                }
                let amount = if a.n(2) > 0.0 { a.n(2) as f32 } else { 1.0 };
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(tr) = en.project.tracks.get_mut(a.u(0)) else { ok!(String::new()) };
                let Some(inst) = tr.inst.as_mut() else { ok!("This track has no instrument".to_string()) };
                let params: Vec<(String, f32)> = inst.params().into_iter().map(|(n, v)| (n.to_string(), v)).collect();
                let changes = crate::redesign::tweak(&params, &mv, amount);
                for (n, v) in &changes {
                    inst.set_param(n, *v);
                }
                let tn = tr.name.clone();
                drop(en);
                self.touch();
                ok!(format!("{tn}: {} ({} parameters)", said.join(", "), changes.len()))
            }
            "daw_song_redesign" => {
                // (track, plugin "" = keep, role "" = from the track name, words, tweak amount 0..1, seed, genre "" = the project's /
                //  the words', sound-design amount 0..1) -> a sentence. A preset of the plugin that fits the part, moved by the words, then
                //  a fresh effect chain for the style (the track's effects are replaced; Ctrl+Z, pushed by the caller, brings them back)
                let t = a.u(0);
                let (tname, kind_now) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(tr) = en.project.tracks.get(t) else { ok!(String::new()) };
                    (tr.name.clone(), tr.inst_kind.clone())
                };
                let words = a.s(3);
                let role = if a.s(2).is_empty() { tname.to_lowercase() } else { a.s(2) };
                let plugin = a.s(1);
                if !plugin.is_empty() && plugin != kind_now && kind_now != "audio" {
                    let _ = self.dispatch("daw_track_set_kind", &A(&[Val::Num(t as f64), Val::Str(plugin.clone()), Val::Str(String::new())]));
                }
                let seed = a.n(5).abs() as u64 + 1;
                let presets: Vec<String> = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.get(t).and_then(|tr| tr.inst.as_ref()).map(|i| i.presets().iter().map(|s| s.to_string()).collect()).unwrap_or_default()
                };
                // name + description, so "warm", "airy", "plucky", "deep sub" ... in the words find the presets that are so
                let kind_for_info = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.get(t).map(|tr| tr.inst_kind.clone()).unwrap_or_default()
                };
                let described: Vec<String> = presets.iter().map(|n| format!("{n}\t{}", crate::presetinfo::info(&kind_for_info, n))).collect();
                let refs: Vec<&str> = described.iter().map(|s| s.as_str()).collect();
                let pick = crate::redesign::pick_preset(&refs, &role, &words, seed).map(|p| p.split('\t').next().unwrap_or("").to_string());
                if let Some(pp) = &pick {
                    let _ = self.dispatch("daw_inst_preset", &A(&[Val::Num(t as f64), Val::Str(pp.clone())]));
                }
                let (mv, said) = crate::redesign::moves(&words);
                let amount = if a.n(4) > 0.0 { a.n(4) as f32 } else { 1.0 };
                let sr = self.out.sr;
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let genre = if !a.s(6).is_empty() {
                    a.s(6)
                } else if let Some(g) = songgen::prompt_genre(&words) {
                    g.name.to_string()
                } else {
                    en.project.meta.iter().find(|(k, _)| k == "ai.genre").map(|(_, v)| v.clone()).unwrap_or_else(|| "house".into())
                };
                let secs = project_sections(&en);
                let tonic = track_tonic(&en, t);
                let Some(tr) = en.project.tracks.get_mut(t) else { ok!(String::new()) };
                let mut changed = 0;
                if let Some(inst) = tr.inst.as_mut() {
                    let params: Vec<(String, f32)> = inst.params().into_iter().map(|(n, v)| (n.to_string(), v)).collect();
                    let ch = crate::redesign::tweak(&params, &mv, amount);
                    for (n, v) in &ch {
                        inst.set_param(n, *v);
                    }
                    changed = ch.len();
                }
                // a fresh chain: the preset's own effects (Growl presets carry one), then the style's design
                tr.effects.clear();
                tr.auto.retain(|l| !l.param.starts_with("fx"));
                if tr.inst_kind == "growl" {
                    if let Some(pp) = &pick {
                        for (fxn, ps) in crate::dsp::growl::Growl::preset_fx(pp) {
                            if let Some(mut slot) = EffectSlot::new(&fxn, sr) {
                                for (k, v) in ps {
                                    if let Some(i) = slot.params.iter().position(|p| p.0 == k) {
                                        slot.set(i, v);
                                    }
                                }
                                tr.effects.push(slot);
                            }
                        }
                    }
                }
                let has: Vec<String> = tr.effects.iter().map(|e| e.name.clone()).collect();
                let design = if a.n(7) > 0.0 { a.n(7) } else { crate::sounddesign::genre_design(&genre).max(0.35) };
                let pd = crate::sounddesign::design(&genre, &words.to_lowercase(), &role, songgen::role_kind(&role), &tr.inst_kind.clone(), design, tonic, &secs, &has);
                apply_design(tr, &pd, sr);
                let chain: Vec<String> = tr.effects.iter().map(|e| e.name.clone()).collect();
                let mut skinned = String::new();
                if let Some(style) = pd.skin {
                    for (k, s) in secs.iter().enumerate().filter(|(_, s)| crate::sounddesign::phase(&s.tag) == crate::sounddesign::Phase::Drop) {
                        let _ = super::artic_api::skin_track(&mut en, t, s.start, s.end, style, (0.5 + 0.4 * design) as f32, 0.4, seed.wrapping_add(k as u64 * 977));
                        skinned = format!(", {style} skin in the drops");
                    }
                }
                drop(en);
                self.touch();
                ok!(format!(
                    "{tname}: {}{}; effects: {}{}",
                    pick.map(|p| format!("preset '{p}'")).unwrap_or_else(|| "kept the patch".into()),
                    if said.is_empty() { String::new() } else { format!(" made {} ({changed} parameters)", said.join(", ")) },
                    if chain.is_empty() { "none".into() } else { chain.join(", ") },
                    skinned
                ))
            }
            "daw_song_harmony" => {
                // (the daw_song_metrics arguments) -> per section [tag, key name, tonic 0..11, minor 0/1, progression, steps on the wheel
                // of fifths from home] - the key path the dialog draws on the wheel of fifths
                let r = if a.s(3).is_empty() { None } else { refs().lock().unwrap_or_else(|p| p.into_inner()).get(&a.s(3)).cloned() };
                let guide = load_guide(&a.s(5));
                let base = songgen::plan_story(&a.s(1), &a.s(2), &[], r.as_ref(), 1.0, a.n(4).max(1.0) as u64, guide.as_ref(), a.n(6), &songgen::Fit::parse(&a.s(7)), &a.s(8));
                let plan = SongPlan::from_line(&a.s(0), &base);
                let mut out = vec![];
                for (si, sec) in plan.sections.iter().enumerate() {
                    let k = songgen::section_key(&plan, si);
                    // steps around the circle: the fifths between the home tonic and this one (-5..6)
                    let d = ((k.tonic as i32 - plan.key.tonic as i32).rem_euclid(12) * 7).rem_euclid(12);
                    let steps = if d > 6 { d - 12 } else { d };
                    out.extend([Val::Str(sec.tag.clone()), Val::Str(k.name()), Val::Num(k.tonic as f64), Val::Num(k.minor as u8 as f64), Val::Str(songgen::section_prog(&plan, si)), Val::Num(steps as f64)]);
                }
                ok!(Val::List(out))
            }
            "daw_tune_check" => {
                // (track) -> [median cents off, worst cents, share of pitched frames, in tune 0/1]: the track's instrument measured on a
                // copy (the playing one is not disturbed)
                let (kind, patch) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(t) = en.project.tracks.get(a.u(0)) else { return Ok(Val::List(vec![])) };
                    (t.inst_kind.clone(), t.patch.clone())
                };
                let sr = self.out.sr;
                // the track's own state on a fresh instrument of its kind
                let state: Vec<(String, f32)> = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.get(a.u(0)).and_then(|t| t.inst.as_ref().map(|i| i.state())).unwrap_or_default()
                };
                let mut inst = if kind == "sf2" { crate::dsp::make_instrument_file("sf2", sr, &patch) } else { crate::dsp::make_instrument(if kind.is_empty() { "synth" } else { &kind }, sr) };
                let Some(i) = inst.as_mut() else { return Ok(Val::List(vec![])) };
                i.load_state(&state);
                let t = crate::tunecheck::check(i.as_mut(), sr);
                ok!(Val::List(vec![Val::Num(t.cents as f64), Val::Num(t.worst as f64), Val::Num(t.pitched as f64), Val::Num(t.in_tune() as u8 as f64)]))
            }
            "daw_song_metrics" => {
                // (plan line, description, lyrics, reference, seed, guide, conform, fit, story, theme "t:c") -> the radar's seven values
                // (Energy, Beat, Harmony, Melody, Groove, Structure, Story), 0..1, for the line as it is now
                let r = if a.s(3).is_empty() { None } else { refs().lock().unwrap_or_else(|p| p.into_inner()).get(&a.s(3)).cloned() };
                let guide = load_guide(&a.s(5));
                let base = songgen::plan_story(&a.s(1), &a.s(2), &[], r.as_ref(), 1.0, a.n(4).max(1.0) as u64, guide.as_ref(), a.n(6), &songgen::Fit::parse(&a.s(7)), &a.s(8));
                let mut plan = SongPlan::from_line(&a.s(0), &base);
                plan.theme = self.theme_of(&a.s(9));
                ok!(Val::List(songgen::metrics(&plan).into_iter().map(Val::Num).collect()))
            }
            "daw_deco_clip" => {
                // (track, clip, amount 0..1, seed) -> notes after: the clip's notes dressed up by the decorator (in the key they are in)
                let (t, c, amount, seed) = (a.u(0), a.u(1), a.n(2), a.n(3).max(0.0) as u64);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(clip) = en.project.tracks.get_mut(t).and_then(|tr| tr.clips.get_mut(c)) else { return Ok(Val::Num(0.0)) };
                let ClipData::Midi(notes) = &mut clip.data else { return Ok(Val::Num(0.0)) };
                let src: Vec<crate::theory::N> = notes.iter().map(|n| crate::theory::N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect();
                if src.is_empty() {
                    return Ok(Val::Num(0.0));
                }
                let end = clip.len;
                let (key, _, _) = crate::theory::detect_key(&src, 0.0, end.max(1.0));
                let out = crate::decorate::decorate(&src, key, &[], amount, seed);
                *notes = out.iter().map(|n| Note::new(n.start, n.len, n.key, n.vel)).collect();
                let count = notes.len();
                drop(en);
                self.status = format!("Decorated: {} notes, in {}", count, key.name());
                self.touch();
                ok!(count as f64)
            }
            "daw_deco_variations" => {
                // (track, clip, count, amount, seed) -> clips added: theme and variations of the clip, one clip each, right after it
                let (t, c, count, amount, seed) = (a.u(0), a.u(1), a.u(2).clamp(2, 7), a.n(3), a.n(4).max(0.0) as u64);
                let (src, start, len) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(clip) = en.project.tracks.get(t).and_then(|tr| tr.clips.get(c)) else { return Ok(Val::Num(0.0)) };
                    let ClipData::Midi(notes) = &clip.data else { return Ok(Val::Num(0.0)) };
                    (notes.iter().map(|n| crate::theory::N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect::<Vec<_>>(), clip.start, clip.len)
                };
                if src.is_empty() {
                    return Ok(Val::Num(0.0));
                }
                let (key, _, _) = crate::theory::detect_key(&src, 0.0, len.max(1.0));
                let vars = crate::decorate::variations(&src, key, count, amount, seed);
                let vlen = (len / 4.0).ceil().max(1.0) * 4.0;
                let mut made = 0;
                for (i, (kind, notes)) in vars.iter().enumerate().skip(1) {
                    let at = start + vlen * i as f64;
                    let ci = self.call_num("daw_clip_add_midi", &[Val::Num(t as f64), Val::Num(at), Val::Num(vlen)]);
                    if ci < 0.0 {
                        continue;
                    }
                    let mut flat = vec![];
                    for n in notes {
                        flat.extend([Val::Num(n.start), Val::Num(n.len), Val::Num(n.key as f64), Val::Num(n.vel as f64), Val::Num(0.0), Val::Num(0.5), Val::Num(0.5)]);
                    }
                    let _ = self.dispatch("daw_clip_set_notes", &A(&[Val::Num(t as f64), Val::Num(ci), Val::List(flat)]));
                    let _ = self.dispatch("daw_clip_set_name", &A(&[Val::Num(t as f64), Val::Num(ci), Val::Str(format!("Var {} - {kind}", i + 1))]));
                    made += 1;
                }
                self.status = format!("Theme and {made} variation(s) in {}", key.name());
                self.touch();
                ok!(made as f64)
            }
            "daw_song_build" => {
                // (plan line, description, lyrics, reference path or "", seed) -> [tracks made, voice track (-1 none), loudness target LUFS,
                // brightness -1..1, summary]. Builds at beat 0 of the current project (the app starts a new one first when asked).
                let desc = a.s(1);
                let lyrics = a.s(2);
                let r = if a.s(3).is_empty() { None } else { refs().lock().unwrap_or_else(|p| p.into_inner()).get(&a.s(3)).cloned() };
                let guide = load_guide(&a.s(5));
                // optional 8th: the length rule the plan was made with (so an unedited structure keeps its sections)
                // optional 9th: the story; 10th: a theme clip "track:clip" the lead plays in variations
                let base = songgen::plan_story(&desc, &lyrics, &[], r.as_ref(), 1.0, a.n(4).max(1.0) as u64, guide.as_ref(), a.n(6), &songgen::Fit::parse(&a.s(7)), &a.s(8));
                let mut plan = SongPlan::from_line(&a.s(0), &base);
                plan.theme = self.theme_of(&a.s(9));
                let g = songgen::genre(&plan.genre);
                let has_vocal = !plan.singer.is_empty();
                self.call_num("daw_set_bpm", &[Val::Num(plan.bpm)]);
                self.call_num("daw_key_set", &[Val::Num(plan.key.tonic as f64), Val::Num(if plan.key.minor { 1.0 } else { 0.0 })]);
                // where every section starts
                let mut starts = vec![];
                let mut at = 0.0;
                for s in &plan.sections {
                    starts.push(at);
                    at += s.bars as f64 * 4.0;
                }
                let total = at;
                // who plays where (a small intro, a part out in the bridge, verses that grow, a full last chorus)
                let arr = songgen::arrange(&plan);
                let mut made = 0usize;
                let mut notes_lifted: Vec<String> = vec![];
                // instruments swapped by the tune check
                let mut tuned: Vec<String> = vec![];
                let mut tracks: Vec<(usize, String)> = vec![];
                for (pi, part) in plan.parts.iter().enumerate() {
                    let role = part.role.as_str();
                    let ((kind, patch), tune_note) = instrument_in_tune(g, role, pi, self.out.sr);
                    if !tune_note.is_empty() {
                        tuned.push(tune_note);
                    }
                    let t = self.call_num("daw_track_add", &[Val::Str(kind.clone()), Val::Str(patch.clone().unwrap_or_default())]);
                    if t < 0.0 {
                        continue;
                    }
                    let t = t as usize;
                    made += 1;
                    let patch_name = patch.as_deref().and_then(|p| Path::new(p).file_stem()).map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
                    let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t as f64), Val::Str(title(role))]));
                    // the patch's own effect chain (`fx Name k=v` lines: OTT, split drive, comb ...) on the new track
                    if let Some(chain) = patch.as_deref().and_then(|p| std::fs::read_to_string(p).ok()).map(|txt| crate::patch::parse(&txt).fx) {
                        let sr = self.out.sr;
                        let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(tr) = en.project.tracks.get_mut(t) {
                            for spec in chain {
                                if let Some(mut slot) = crate::model::EffectSlot::new(&spec.name.replace('_', " "), sr) {
                                    for (k, v) in spec.params {
                                        let k = k.replace('_', " ");
                                        if let Some(i) = slot.params.iter().position(|p| p.0 == k) {
                                            slot.set(i, v);
                                        }
                                    }
                                    tr.effects.push(slot);
                                }
                            }
                        }
                    }
                    for (si, sec) in plan.sections.iter().enumerate() {
                        if !arr.on[pi][si] {
                            continue;
                        }
                        let notes = songgen::section_notes(&plan, pi, sec, None);
                        if notes.is_empty() {
                            continue;
                        }
                        let c = self.call_num("daw_clip_add_midi", &[Val::Num(t as f64), Val::Num(starts[si]), Val::Num(sec.bars as f64 * 4.0)]);
                        if c < 0.0 {
                            continue;
                        }
                        let mut flat = Vec::with_capacity(notes.len() * 7);
                        for n in &notes {
                            flat.extend([Val::Num(n.start), Val::Num(n.len), Val::Num(n.key as f64), Val::Num(n.vel as f64), Val::Num(0.0), Val::Num(0.5), Val::Num(0.5)]);
                        }
                        let _ = self.dispatch("daw_clip_set_notes", &A(&[Val::Num(t as f64), Val::Num(c), Val::List(flat)]));
                        let _ = self.dispatch("daw_clip_set_name", &A(&[Val::Num(t as f64), Val::Num(c), Val::Str(sec.tag.clone())]));
                    }
                    // a bass preset that already sounds an octave down gets its part an octave up (not two octaves under the song)
                    if songgen::role_kind(role) == Kind::Bass {
                        let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(tr) = en.project.tracks.get_mut(t) {
                            if crate::model::lift_low_bass(tr, &patch_name) {
                                notes_lifted.push(title(role));
                            }
                        }
                    }
                    tracks.push((t, role.to_string()));
                }
                // the singer: a line in the singer's range in every sung section, the lyrics fitted onto it
                let mut voice_t = -1.0;
                // the emotional plan (emotion.rs): rolled with the seed, or the model's (11th argument) laid over it
                let sec_words = songgen::lyrics_for_sections(&lyrics, &plan.sections);
                let mut emo = crate::emotion::plan(&plan.sections, &plan.arc, &plan.genre, &sec_words, plan.dials.mood, plan.seed);
                if !a.s(10).trim().is_empty() {
                    crate::emotion::apply_model(&mut emo, &plan.sections, &a.s(10));
                }
                if has_vocal {
                    let path = find_patch("voice", &plan.singer).or_else(|| find_patch("voice", "aria")).unwrap_or_default();
                    let t = self.call_num("daw_track_add", &[Val::Str("voice".into()), Val::Str(path)]);
                    if t >= 0.0 {
                        made += 1;
                        voice_t = t;
                        let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("Vocal ({})", plan.singer))]));
                        let words = songgen::lyrics_for_sections(&lyrics, &plan.sections);
                        let range = songgen::singer_range(&plan.singer);
                        // the tune is a "voice" part of its own, so it has its own seed
                        let mut vplan = plan.clone();
                        vplan.parts.push(songgen::Part { role: "voice".into() });
                        let vi = vplan.parts.len() - 1;
                        for (si, sec) in plan.sections.iter().enumerate() {
                            let text = words.get(si).cloned().unwrap_or_default();
                            let sing = if lyrics.trim().is_empty() { matches!(sec.tag.as_str(), "chorus" | "drop" | "climax" | "theme") } else { !text.trim().is_empty() };
                            if !sing {
                                continue;
                            }
                            let mut notes = songgen::section_notes(&vplan, vi, sec, Some(range));
                            let c = self.call_num("daw_clip_add_midi", &[Val::Num(t), Val::Num(starts[si]), Val::Num(sec.bars as f64 * 4.0)]);
                            if c < 0.0 || notes.is_empty() {
                                continue;
                            }
                            let sung = if text.trim().is_empty() { "oh oh oh oh\nah ah ah ah".to_string() } else { text.clone() };
                            // about a note per syllable, none shorter than a sixteenth
                            songgen::fit_line(&mut notes, songgen::syllable_count(&sung).0);
                            // the feel of the section: restrained to forceful dynamics, the downbeats leaned on when it pushes
                            if let Some(f) = emo.feels.get(si) {
                                for n in notes.iter_mut() {
                                    let accent = if n.start.rem_euclid(4.0) < 0.05 { 0.08 * f.vocal } else { 0.0 };
                                    n.vel = (n.vel * (0.55 + 0.5 * f.vocal) + accent).clamp(0.15, 1.0);
                                }
                            }
                            let mut flat = Vec::with_capacity(notes.len() * 7);
                            for n in &notes {
                                flat.extend([Val::Num(n.start), Val::Num(n.len), Val::Num(n.key as f64), Val::Num(n.vel as f64), Val::Num(0.0), Val::Num(0.5), Val::Num(0.5)]);
                            }
                            let _ = self.dispatch("daw_clip_set_notes", &A(&[Val::Num(t), Val::Num(c), Val::List(flat)]));
                            let _ = self.dispatch("daw_clip_set_name", &A(&[Val::Num(t), Val::Num(c), Val::Str(sec.tag.clone())]));
                            let _ = self.dispatch("daw_lyrf_apply", &A(&[Val::Num(t), Val::Num(c), Val::Str(sung), Val::Str("fit:shape:0:0.3:all:1.0:auto".into())]));
                        }
                    }
                }
                // the performance and the room: the singer's breath / vibrato / brightness / scoops follow the feel per section, and
                // under every sung note the band softens (the lead answers in the gaps where the plan asks for room)
                if has_vocal && voice_t >= 0.0 {
                    let vt = voice_t as usize;
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    // the sung notes, absolute
                    let sung: Vec<(f64, f64)> = en.project.tracks.get(vt).map(|tr| tr.clips.iter().flat_map(|c| match &c.data { ClipData::Midi(ns) => ns.iter().map(|n| (c.start + n.start, c.start + n.start + n.len)).collect::<Vec<_>>(), _ => vec![] }).collect()).unwrap_or_default();
                    let sec_of = |beat: f64| -> usize { starts.iter().rposition(|s| *s <= beat + 1e-6).unwrap_or(0) };
                    if let Some(tr) = en.project.tracks.get_mut(vt) {
                        let names: Vec<String> = tr.inst.as_ref().map(|i| i.params().into_iter().map(|(n, _)| n.to_string()).collect()).unwrap_or_default();
                        let lanes: [(&str, fn(&crate::emotion::Feel) -> f32); 4] = [
                            ("breath", |f| 0.12 + 0.45 * f.breath),
                            ("vib depth", |f| 0.2 + 0.45 * f.vocal),
                            ("bright", |f| (0.35 + 0.4 * f.vocal + 0.1 * f.valence).clamp(0.1, 0.95)),
                            ("scoop", |f| (0.1 + 0.3 * f.vocal + if f.valence < -0.2 { 0.1 } else { 0.0 }).min(0.6)),
                        ];
                        for (pname, fun) in lanes {
                            if !names.iter().any(|n| n == pname) {
                                continue;
                            }
                            let mut pts: Vec<(f64, f32)> = vec![];
                            for (si, sec) in plan.sections.iter().enumerate() {
                                let Some(f) = emo.feels.get(si) else { continue };
                                let v = fun(f);
                                pts.push((starts[si] + if si == 0 { 0.0 } else { 0.5 }, v));
                                pts.push((starts[si] + sec.bars as f64 * 4.0 - 0.01, v));
                            }
                            tr.auto.retain(|l| l.param != pname);
                            tr.auto.push(AutoLane { param: pname.to_string(), points: pts });
                        }
                    }
                    let mut answered = 0usize;
                    for (ti, role) in &tracks {
                        let k = songgen::role_kind(role);
                        if matches!(k, Kind::Drums | Kind::Bass) || role == "riser" {
                            continue;
                        }
                        let Some(tr) = en.project.tracks.get_mut(*ti) else { continue };
                        for c in tr.clips.iter_mut() {
                            let cs = c.start;
                            let ClipData::Midi(ns) = &mut c.data else { continue };
                            let before = ns.len();
                            ns.retain_mut(|n| {
                                let (a0, a1) = (cs + n.start, cs + n.start + n.len);
                                let over: f64 = sung.iter().map(|(b0, b1)| (a1.min(*b1) - a0.max(*b0)).max(0.0)).sum();
                                if over <= 0.0 {
                                    return true;
                                }
                                let f = emo.feels.get(sec_of(a0)).copied();
                                let space = f.map(|f| f.space).unwrap_or(0.4);
                                // a lead in a section that asks for room plays only where the voice rests
                                if k == Kind::Melody && space > 0.6 && over > 0.5 * n.len {
                                    return false;
                                }
                                n.vel = (n.vel * (1.0 - 0.35 * space)).max(0.1);
                                true
                            });
                            answered += before - ns.len();
                        }
                    }
                    if answered > 0 {
                        emo.notes.push(format!("the lead answers the voice ({answered} notes moved out of its way)"));
                    }
                }
                // section tags on the ruler
                let tags: Vec<String> = plan.sections.iter().zip(&starts).map(|(s, st)| format!("{}:{}:{}", st, st + s.bars as f64 * 4.0, s.tag)).collect();
                let _ = self.dispatch("daw_arr_tags_set", &A(&[Val::Str(tags.join(";"))]));
                {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.meta.retain(|(k, _)| k != "ai.genre");
                    en.project.meta.push(("ai.genre".into(), plan.genre.clone()));
                }
                // automation: the filter opens through every build, sits low in breaks and intros, closes in the outro; club pads pump
                {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    for (t, role) in &tracks {
                        let k = songgen::role_kind(role);
                        if k == Kind::Drums {
                            continue;
                        }
                        let Some(tr) = en.project.tracks.get_mut(*t) else { continue };
                        let names: Vec<&str> = tr.inst.as_ref().map(|i| i.params().into_iter().map(|(n, _)| n).collect()).unwrap_or_default();
                        if let Some(cut) = crate::flp_import::map_param("filter cutoff", &names).filter(|p| p != "vol" && p != "pan") {
                            let cur = tr.inst.as_ref().and_then(|i| i.params().into_iter().find(|(n, _)| *n == cut).map(|(_, v)| v)).unwrap_or(0.7);
                            let open = cur.max(0.55);
                            let mut pts: Vec<(f64, f32)> = vec![];
                            for (sec, st) in plan.sections.iter().zip(&starts) {
                                let end = st + sec.bars as f64 * 4.0;
                                let (a0, a1) = match sec.tag.as_str() {
                                    "build" => (open * 0.4, open),
                                    "intro" | "break" => (open * 0.55, open * 0.55),
                                    "outro" => (open, open * 0.3),
                                    _ => (open, open),
                                };
                                pts.push((*st, a0));
                                pts.push((end - 0.01, a1));
                            }
                            tr.auto.retain(|l| l.param != cut);
                            tr.auto.push(AutoLane { param: cut.clone(), points: pts });
                        }
                        // sidechain-style pump on pads and chords in club drops (the track fader, 1/4 notes)
                        if g.form == Form::Club && matches!(k, Kind::Pad | Kind::Chords) && role != "riser" {
                            let v = (tr.volume / 1.5).clamp(0.0, 1.0);
                            let mut pts: Vec<(f64, f32)> = vec![(0.0, v)];
                            for (sec, st) in plan.sections.iter().zip(&starts) {
                                if sec.tag != "drop" {
                                    continue;
                                }
                                let mut b = *st;
                                let end = st + sec.bars as f64 * 4.0;
                                while b < end - 0.01 {
                                    pts.push((b, v * 0.35));
                                    pts.push((b + 0.45, v));
                                    b += 1.0;
                                }
                                pts.push((end, v));
                            }
                            if pts.len() > 1 {
                                tr.auto.retain(|l| l.param != "vol");
                                tr.auto.push(AutoLane { param: "vol".into(), points: pts });
                            }
                        }
                    }
                    let _ = total;
                }
                // sound design: every part's effect chain, its movement through the form, the Growl skin in the drops (sounddesign.rs)
                let mut designed: Vec<String> = vec![];
                if plan.dials.design > 0.04 {
                    use crate::sounddesign::{self as sd, Phase, Sec};
                    let secs: Vec<Sec> = plan.sections.iter().zip(&starts).map(|(s, st)| Sec { tag: s.tag.clone(), start: *st, end: st + s.bars as f64 * 4.0 }).collect();
                    let sr = self.out.sr;
                    let ldesc = desc.to_lowercase();
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    for (t, role) in &tracks {
                        let skin = {
                            let Some(tr) = en.project.tracks.get_mut(*t) else { continue };
                            let has: Vec<String> = tr.effects.iter().map(|e| e.name.clone()).collect();
                            let pd = sd::design(&plan.genre, &ldesc, role, songgen::role_kind(role), &tr.inst_kind, plan.dials.design, plan.key.tonic, &secs, &has);
                            if pd.chain.is_empty() && pd.skin.is_none() {
                                continue;
                            }
                            apply_design(tr, &pd, sr);
                            designed.push(sd::describe(&title(role), &pd));
                            pd.skin
                        };
                        if let Some(style) = skin {
                            for (k, s) in secs.iter().enumerate().filter(|(_, s)| sd::phase(&s.tag) == Phase::Drop) {
                                let _ = super::artic_api::skin_track(&mut en, *t, s.start, s.end, style, (0.5 + 0.4 * plan.dials.design) as f32, 0.4, plan.seed.wrapping_add(k as u64 * 977 + *t as u64));
                            }
                        }
                    }
                }
                // articulation (the local chooser, read from the description) on the bass and the lead
                for (t, role) in &tracks {
                    let k = songgen::role_kind(role);
                    if !matches!(k, Kind::Bass | Kind::Melody) {
                        continue;
                    }
                    let prompt = if role.contains("wobble") || role.contains("growl") { format!("{desc} wobble on the drops") } else { desc.clone() };
                    let chunks = self.call_num("daw_artic_begin", &[Val::Str("single".into()), Val::Num(*t as f64), Val::Num(2.0), Val::Num(plan.seed as f64), Val::Num(0.55), Val::Str(prompt)]);
                    let mut ci = 0.0;
                    while ci < chunks {
                        let _ = self.dispatch("daw_artic_apply", &A(&[Val::Num(ci), Val::Str(String::new())]));
                        ci += 1.0;
                    }
                    let _ = self.dispatch("daw_artic_end", &A(&[]));
                    // a bass never moves in the stereo field: its sub stays centred
                    if k == Kind::Bass {
                        let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                        if let Some(tr) = en.project.tracks.get_mut(*t) {
                            tr.auto.retain(|l| l.param != "pan");
                            tr.pan = 0.0;
                        }
                    }
                }
                self.touch();
                let bright = r.as_ref().map(|r| ((r.bright - 0.16) * 5.0).clamp(-1.0, 1.0)).unwrap_or(0.0);
                let summary = format!(
                    "{} at {:.0} bpm in {}: {} bars, {} tracks{}{}{}",
                    plan.genre,
                    plan.bpm,
                    plan.key.name(),
                    plan.bars(),
                    made,
                    if has_vocal { format!(", sung by {}", plan.singer) } else { String::new() },
                    if tuned.is_empty() { String::new() } else { format!(". Tune check: {}", tuned.join("; ")) },
                    if notes_lifted.is_empty() { String::new() } else { format!(". Bass up an octave: {}", notes_lifted.join(", ")) }
                ) + &format!(". Feel: {}", crate::emotion::summary(&emo, &plan.sections)) + &if designed.is_empty() { String::new() } else { format!(". Sound design: {}", designed.join("; ")) };
                self.status = format!("AI song: {summary}");
                ok!(Val::List(vec![Val::Num(made as f64), Val::Num(voice_t), Val::Num(g.lufs), Val::Num(bright), Val::Str(summary)]))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
