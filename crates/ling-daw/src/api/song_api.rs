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
/// what was swapped out and why ("" when the first choice was fine). `roll` (from the seed) varies the patch within the plugin.
fn instrument_in_tune(g: &songgen::Genre, role: &str, part: usize, roll: usize, sr: f32) -> ((String, Option<String>), String) {
    // the seed rolls among the role's patches on the same plugin (never onto a fallback plugin)
    let first = {
        let b = instrument_for(g, role, part);
        let r = instrument_for(g, role, part + roll);
        if r.0 == b.0 && r.1.is_some() { r } else { b }
    };
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

/// interleaved stereo as a 16-bit WAV
fn write_wav16(path: &Path, stereo: &[f32], rate: u32) -> Result<(), String> {
    let spec = hound::WavSpec { channels: 2, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).map_err(|e| e.to_string())?;
    for x in stereo {
        w.write_sample((x.clamp(-1.0, 1.0) * 32767.0) as i16).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

/// every MIDI / tracker file in a folder, chopped into phrases (sorted by file name, then position: the order decoding relies on)
fn medley_phrases(dir: &str, bars: f64) -> (Vec<crate::medley::Phrase>, usize) {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)
        .map(|rd| rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| ["mid", "midi", "mod", "xm", "s3m", "it"].contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false)).collect())
        .unwrap_or_default();
    files.sort();
    let mut out = vec![];
    let mut used = 0;
    for f in &files {
        let Ok(data) = std::fs::read(f) else { continue };
        let path = f.to_string_lossy().into_owned();
        let Ok(m) = crate::midi_import::parse_any(&path, &data) else { continue };
        let name = f.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let ph = crate::medley::chop(&m, &name, bars);
        if !ph.is_empty() {
            used += 1;
        }
        out.extend(ph);
    }
    (out, used)
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
        // or the notes themselves, "notes:start,len,key,vel;..." (a song cycle's motif, cycle.rs)
        if let Some(v) = crate::cycle::parse_notes_spec(spec) {
            return Some(std::sync::Arc::new(v));
        }
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
                    "Description: {}\n{}\nFill in this JSON for the song. genre: one of [{}]. bpm: a number. key: like \"F# minor\". progression: 4 chords for the verse that suit the genre and the mood, Roman numerals in the key (upper case major, lower case minor, b for a borrowed chord) like \"i VI III VII\". chorus_progression: 4 chords for the chorus, or \"\" when it uses the verse's. structure: sections with bars like \"intro 8, verse 16, chorus 8, outro 8\". instruments: 3 to 6 of [{}]. arc: how the song feels over time, one of [release, slow burn, bittersweet, triumph, melancholy, storm, lullaby]. intro: how the song opens, one of [swell, solo, beat, theme, cold, drone] (pads fading in, one instrument alone, the groove first, the hook first, a cold open on the chorus, a drone). feel: per kind of section how hard the vocal pushes (0 restrained .. 1 forceful) and its mood (-1 sad .. 1 happy), like [{{\"section\": \"verse\", \"vocal\": 0.3, \"valence\": -0.2}}, {{\"section\": \"chorus\", \"vocal\": 0.9, \"valence\": 0.3}}].\n{{\"genre\": \"\", \"bpm\": 0, \"key\": \"\", \"progression\": \"\", \"chorus_progression\": \"\", \"structure\": \"\", \"instruments\": \"\"}}",
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
                crate::sounddesign::apply_inst(tr, &pd);
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
            "daw_song_medley_scan" => {
                // (folder, phrase bars 0 = auto) -> [files used, phrases, key name, median bpm]
                let (ph, files) = medley_phrases(&a.s(0), a.n(1));
                if ph.is_empty() {
                    ok!(Val::List(vec![Val::Num(0.0), Val::Num(0.0), Val::Str(String::new()), Val::Num(0.0)]));
                }
                let (t, m) = crate::medley::common_key(&ph);
                let mut b: Vec<f64> = ph.iter().map(|p| p.bpm).collect();
                b.sort_by(f64::total_cmp);
                ok!(Val::List(vec![Val::Num(files as f64), Val::Num(ph.len() as f64), Val::Str(crate::theory::Key { tonic: t, minor: m }.name()), Val::Num(b[b.len() / 2].round())]))
            }
            "daw_song_medley_build" => {
                // (folder, phrase bars 0 = auto, phrases, mode "smart"|"random"|"cipher", passphrase, message, seed, bpm 0 = the files',
                // options "layering=0..1;rolls=0..1;arc=<feel>;genre=<sounds>;chops=off|chops|breakcore;chopn=<count>;choplen=<seconds>;chopvar=0..1;chopick=random|best;vocal=<file>;story=<text, last>") -> a sentence. The story / feel becomes
                // an energy and mood curve and a path on the wheel of fifths (medley.rs mashup); each slot is the spine phrase's hook
                // (named "<file> #<n> · <id>": what the cipher reads) with parts of other phrases layered on it, loop rolls between the
                // slots with a filter contour on every track, and every clip on its category's next patch (a track per patch).
                let (ph, files) = medley_phrases(&a.s(0), a.n(1));
                if ph.len() < 2 {
                    ok!("Not enough phrases: the folder needs MIDI files with notes".to_string());
                }
                let mode = match a.s(3).as_str() {
                    "random" => crate::medley::Mode::Random,
                    "cipher" => crate::medley::Mode::Cipher,
                    _ => crate::medley::Mode::Smart,
                };
                let pass = a.s(4);
                let message = a.s(5);
                let seed = a.n(6).abs() as u64;
                let opts = a.s(8);
                let (head, story) = match opts.find("story=") {
                    Some(i) => (opts[..i].to_string(), opts[i + 6..].trim().to_string()),
                    None => (opts.clone(), String::new()),
                };
                let opt = |k: &str| head.split(';').filter_map(|kv| kv.split_once('=')).find(|(kk, _)| kk.trim() == k).map(|(_, v)| v.trim().to_string());
                let num = |k: &str| opt(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.0).clamp(0.0, 1.0);
                let (layering, rolls) = (num("layering"), num("rolls"));
                let g = songgen::genre(&opt("genre").filter(|x| !x.is_empty()).unwrap_or_else(|| "pop".into()));
                let arc = crate::medley::resolve_arc(&opt("arc").unwrap_or_default(), &story, seed);
                let lean = if story.is_empty() { 0.0 } else { crate::emotion::lyric_lean(&story).0 as f64 * 0.5 };
                let key = crate::medley::common_key(&ph);
                let count = (a.n(2).max(2.0) as usize).min(512);
                let order: Vec<usize> = if mode == crate::medley::Mode::Cipher && !message.is_empty() {
                    crate::medley::encode(&message, ph.len(), &pass)
                } else {
                    let curve: Vec<f64> = crate::medley::movement(&arc, count, lean).iter().map(|x| x.0).collect();
                    crate::medley::order_to(&ph, count, mode, seed, &pass, key, Some(&curve))
                };
                let mut bpms: Vec<f64> = ph.iter().map(|p| p.bpm).collect();
                bpms.sort_by(f64::total_cmp);
                let bpm = if a.n(7) > 20.0 { a.n(7) } else { bpms[bpms.len() / 2] };
                // the arrangement smarts: registers (a muddy bass up, a shrill lead down, every layer an octave of its own), phrase
                // alignment, a breakdown before the last drop, chord-aware fitting, call and response (medley.rs mashup_with)
                let flag = |k: &str| opt(k).map(|v| !matches!(v.as_str(), "0" | "off" | "false" | "no")).unwrap_or(true);
                let smarts = crate::medley::Smarts {
                    registers: crate::medley::Registers::parse(&opt("registers").unwrap_or_default()),
                    genre: g.name.to_string(),
                    align: flag("align"),
                    breakdown: flag("breakdown"),
                    answer: flag("answer"),
                    clash: flag("clash"),
                };
                let mut mash = crate::medley::mashup_with(&ph, &order, &arc, lean, layering, rolls, seed, key, &smarts);
                // vocal chops (and, in the breakcore style, programmed breaks in place of the files' drums)
                let style = crate::chops::Style::parse(&opt("chops").unwrap_or_default());
                if style == crate::chops::Style::Breakcore {
                    for s in mash.slots.iter_mut() {
                        s.layers.retain(|l| l.spine || l.cat != crate::medley::Cat::Drums);
                    }
                }
                // an instrument for every clip: its category's role for the slot's energy and mood, the next patch each time
                const KITS: [&str; 7] = ["Kit House 909", "Kit Electro 808", "Kit UK Garage", "Kit Trap", "Kit New Wave", "Kit Hyperpop", "Kit Boom Bap"];
                let mut wanted: Vec<(crate::medley::Cat, String, Option<String>)> = vec![];
                let mut wanted_role: Vec<&'static str> = vec![];
                let mut assign: Vec<Vec<usize>> = vec![];
                for s in &mash.slots {
                    let mut row = vec![];
                    for l in &s.layers {
                        let k = l.pick;
                        let (kind, patch) = if l.cat == crate::medley::Cat::Drums {
                            let kits: Vec<&str> = std::iter::once(g.kit).chain(KITS.iter().copied().filter(|x| *x != g.kit)).collect();
                            ("drums".to_string(), find_patch("drums", kits[k % kits.len()]))
                        } else {
                            instrument_for(g, l.role, k / 3)
                        };
                        let idx = match wanted.iter().position(|w| w.0 == l.cat && w.1 == kind && w.2 == patch) {
                            Some(i) => i,
                            None if wanted.len() < 24 => {
                                wanted.push((l.cat, kind, patch));
                                wanted_role.push(l.role);
                                wanted.len() - 1
                            }
                            // enough tracks: the category's first
                            None => match wanted.iter().position(|w| w.0 == l.cat) {
                                Some(i) => i,
                                None => {
                                    wanted.push((l.cat, kind, patch));
                                    wanted_role.push(l.role);
                                    wanted.len() - 1
                                }
                            },
                        };
                        row.push(idx);
                    }
                    assign.push(row);
                }
                let mut tracks: Vec<Option<usize>> = vec![];
                for (cat, kind, patch) in &wanted {
                    let t = self.call_num("daw_track_add", &[Val::Str(kind.clone()), Val::Str(patch.clone().unwrap_or_default())]);
                    let stem = patch.as_deref().and_then(|p| Path::new(p).file_stem()).map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_else(|| kind.clone());
                    if t >= 0.0 {
                        let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("{} · {}", cat.name(), stem))]));
                        tracks.push(Some(t as usize));
                    } else {
                        tracks.push(None);
                    }
                }
                // the vocal: the chosen file, else a vocal audio file in the folder, else syllables sung by Ling Voice (saved next to
                // the project); cut into chops, tuned on a Sampler (chops.rs)
                let mut chop_cfg: Option<(String, Vec<crate::chops::Chop>)> = None;
                let mut chop_src = String::new();
                if style != crate::chops::Style::Off {
                    let is_audio = |p: &Path| p.extension().map(|e| ["wav", "mp3", "ogg", "flac", "aiff", "m4a"].contains(&e.to_string_lossy().to_lowercase().as_str())).unwrap_or(false);
                    let mut src = opt("vocal").filter(|v| !v.is_empty() && Path::new(v).is_file());
                    if src.is_none() {
                        src = std::fs::read_dir(a.s(0)).ok().and_then(|rd| rd.flatten().map(|e| e.path()).filter(|p| is_audio(p)).min()).map(|p| p.to_string_lossy().into_owned());
                    }
                    let got: Option<(String, Vec<f32>, u32)> = match src {
                        Some(f) => crate::dsp::sampler::load_audio(&f).ok().map(|(d, r)| (f, d, r)),
                        None => {
                            let sr = self.out.sr;
                            let d = crate::chops::sung_syllables(sr, seed);
                            let dir = if self.path.is_empty() { PathBuf::from("cache/chops") } else { Path::new(&self.path).parent().map(|p| p.join("samples")).unwrap_or_else(|| PathBuf::from("cache/chops")) };
                            let _ = std::fs::create_dir_all(&dir);
                            let f = dir.join(format!("ling_voice_chops_{}.wav", seed % 3));
                            write_wav16(&f, &d, sr as u32).ok().map(|_| (f.to_string_lossy().replace('\\', "/"), d, sr as u32))
                        }
                    };
                    if let Some((file, stereo, rate)) = got {
                        let fnum = |k: &str, d: f64| opt(k).and_then(|v| v.parse::<f64>().ok()).unwrap_or(d);
                        let co = crate::chops::Opts {
                            count: fnum("chopn", 8.0).clamp(1.0, 64.0) as usize,
                            length: fnum("choplen", 0.25).clamp(0.05, 1.5),
                            vary: fnum("chopvar", 0.5).clamp(0.0, 1.0),
                            random: opt("chopick").map(|v| v != "best").unwrap_or(true),
                        };
                        let ch = crate::chops::pick_chops(&stereo, rate, &co, seed);
                        if !ch.is_empty() {
                            let cfg = crate::chops::config(&file);
                            chop_src = Path::new(&file).file_stem().map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
                            chop_cfg = Some((serde_json::to_string(&cfg).unwrap_or_default(), ch));
                        }
                    }
                }
                // each slot's chops (main, layer) and breaks, composed against its layers
                let mut chop_notes: Vec<(Vec<Note>, Vec<Note>, Vec<Note>)> = vec![];
                for (si, s) in mash.slots.iter().enumerate() {
                    let br = if style == crate::chops::Style::Breakcore { crate::chops::breaks(s.beats, s.energy, bpm, seed.wrapping_add(si as u64)) } else { vec![] };
                    let (mut drums, mut harmony, mut lead) = (br.clone(), vec![], vec![]);
                    for l in &s.layers {
                        let ns = crate::medley::layer_notes(&ph, l, s.beats);
                        match l.cat {
                            crate::medley::Cat::Drums => drums.extend(ns),
                            crate::medley::Cat::Bass | crate::medley::Cat::Pad | crate::medley::Cat::Stab => harmony.extend(ns),
                            _ => lead.extend(ns),
                        }
                    }
                    let (m, y) = match &chop_cfg {
                        Some((_, ch)) => crate::chops::compose(&crate::chops::Ctx { beats: s.beats, key: s.key, energy: s.energy, drums: &drums, harmony: &harmony, lead: &lead, style, seed: seed.wrapping_mul(31).wrapping_add(si as u64), chops: ch }),
                        None => (vec![], vec![]),
                    };
                    chop_notes.push((m, y, br));
                }
                // their tracks: the chops, their layer, the breaks
                let mut extra: [Option<usize>; 3] = [None, None, None];
                if chop_cfg.is_some() {
                    for (i, nm) in [format!("Chops · {chop_src}"), format!("Chops layer · {chop_src}")].into_iter().enumerate() {
                        let t = self.call_num("daw_track_add", &[Val::Str("sampler".into()), Val::Str(String::new())]);
                        if t >= 0.0 {
                            let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(nm)]));
                            extra[i] = Some(t as usize);
                        }
                    }
                }
                if chop_notes.iter().any(|c| !c.2.is_empty()) {
                    let kit = if g.kit.contains("Drum and Bass") { "Kit Drum and Bass" } else { "Kit Industrial" };
                    let t = self.call_num("daw_track_add", &[Val::Str("drums".into()), Val::Str(find_patch("drums", kit).unwrap_or_default())]);
                    if t >= 0.0 {
                        let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("Breaks · {kit}"))]));
                        extra[2] = Some(t as usize);
                    }
                }
                // a riser into every drop a loop roll builds to
                let riser_into: Vec<usize> = (0..mash.slots.len()).filter(|i| mash.slots[*i].roll_after == crate::medley::Roll::Shrink && mash.slots.get(i + 1).map(|x| x.energy >= 0.7).unwrap_or(false)).collect();
                let mut riser_t: Option<usize> = None;
                if !riser_into.is_empty() && flag("risers") {
                    let (kind, patch) = instrument_for(g, "riser", 0);
                    let t = self.call_num("daw_track_add", &[Val::Str(kind), Val::Str(patch.unwrap_or_default())]);
                    if t >= 0.0 {
                        let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str("Riser".into())]));
                        riser_t = Some(t as usize);
                    }
                }
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                if let Some((json, _)) = &chop_cfg {
                    for t in extra[..2].iter().flatten() {
                        if let Ok(smp) = crate::dsp::sampler::Sampler::from_config(self.out.sr, json, None) {
                            if let Some(tr) = en.project.tracks.get_mut(*t) {
                                tr.inst = Some(Box::new(smp));
                                tr.inst_kind = "sampler".into();
                            }
                        }
                    }
                }
                let empty = en.project.tracks.iter().all(|t| t.clips.is_empty());
                let start0 = (en.project.end_beat() / 4.0).ceil() * 4.0;
                if empty {
                    en.project.bpm = bpm.clamp(40.0, 240.0);
                }
                let mut at = start0;
                let mut secs: Vec<(f64, f64, &str)> = vec![];
                let mut spans: Vec<(f64, crate::medley::Roll)> = vec![];
                let ns = mash.slots.len();
                for (si, s) in mash.slots.iter().enumerate() {
                    for (li, l) in s.layers.iter().enumerate() {
                        let Some(t) = tracks[assign[si][li]] else { continue };
                        let p = &ph[l.phrase];
                        let name = if l.spine { format!("{} #{} · {}", p.file, p.index + 1, crate::medley::short_id(p.id)) } else { format!("{} #{} {}", p.file, p.index + 1, l.cat.name()) };
                        let mut c = Clip::midi(at, s.beats, &name);
                        if let ClipData::Midi(v) = &mut c.data {
                            v.extend(crate::medley::layer_notes(&ph, l, s.beats));
                        }
                        if let Some(tr) = en.project.tracks.get_mut(t) {
                            tr.clips.push(c);
                        }
                    }
                    for (k, label) in [(0usize, "chops"), (1, "chops layer"), (2, "breaks")] {
                        let notes = match k {
                            0 => &chop_notes[si].0,
                            1 => &chop_notes[si].1,
                            _ => &chop_notes[si].2,
                        };
                        if let (Some(t), false) = (extra[k], notes.is_empty()) {
                            let mut c = Clip::midi(at, s.beats, label);
                            if let ClipData::Midi(v) = &mut c.data {
                                v.extend(notes.iter().cloned());
                            }
                            if let Some(tr) = en.project.tracks.get_mut(t) {
                                tr.clips.push(c);
                            }
                        }
                    }
                    secs.push((at, at + s.beats, crate::medley::slot_tag(si, ns, s.energy)));
                    at += s.beats;
                    // a loop roll: this slot's layers shrinking into a lift, or the next slot's growing back out of a drop
                    let src = match s.roll_after {
                        crate::medley::Roll::Shrink => Some(si),
                        crate::medley::Roll::Grow => Some(si + 1).filter(|x| *x < ns),
                        crate::medley::Roll::None => None,
                    };
                    if let Some(ri) = src {
                        let rs = &mash.slots[ri];
                        for (li, l) in rs.layers.iter().enumerate() {
                            let Some(t) = tracks[assign[ri][li]] else { continue };
                            let p = &ph[l.phrase];
                            let mut c = Clip::midi(at, crate::medley::ROLL_BEATS, &format!("{} #{} roll", p.file, p.index + 1));
                            if let ClipData::Midi(v) = &mut c.data {
                                v.extend(crate::medley::roll_notes(&crate::medley::layer_notes(&ph, l, rs.beats), rs.beats, s.roll_after));
                            }
                            if let Some(tr) = en.project.tracks.get_mut(t) {
                                tr.clips.push(c);
                            }
                        }
                        for (k, label) in [(0usize, "chops roll"), (1, "chops layer roll"), (2, "breaks roll")] {
                            let notes = match k {
                                0 => &chop_notes[ri].0,
                                1 => &chop_notes[ri].1,
                                _ => &chop_notes[ri].2,
                            };
                            if let (Some(t), false) = (extra[k], notes.is_empty()) {
                                let mut c = Clip::midi(at, crate::medley::ROLL_BEATS, label);
                                if let ClipData::Midi(v) = &mut c.data {
                                    v.extend(crate::medley::roll_notes(notes, rs.beats, s.roll_after));
                                }
                                if let Some(tr) = en.project.tracks.get_mut(t) {
                                    tr.clips.push(c);
                                }
                            }
                        }
                        if let (Some(t), true) = (riser_t, riser_into.contains(&si)) {
                            let mut c = Clip::midi(at, crate::medley::ROLL_BEATS, "riser");
                            if let ClipData::Midi(v) = &mut c.data {
                                v.extend(crate::medley::riser_notes(mash.slots[si + 1].key, crate::medley::ROLL_BEATS));
                            }
                            if let Some(tr) = en.project.tracks.get_mut(t) {
                                tr.clips.push(c);
                            }
                        }
                        spans.push((at, s.roll_after));
                        secs.push((at, at + crate::medley::ROLL_BEATS, if s.roll_after == crate::medley::Roll::Shrink { "build" } else { "break" }));
                        at += crate::medley::ROLL_BEATS;
                    }
                }
                // per-role sound design and space (sounddesign.rs, space.rs): the genre's chain on every medley track over the medley's
                // sections (builds narrow and push, drops open, breaks pull back); a breakcore mashup is designed as breakbeat, which
                // keeps it dry (a tight room at most, none on the kick and the bass)
                let design_amt = opt("design").and_then(|v| v.parse::<f64>().ok()).unwrap_or_else(|| crate::sounddesign::genre_design(g.name).max(0.3)).clamp(0.0, 1.0);
                let mut design_note = String::new();
                if design_amt > 0.0 {
                    let dg = if style == crate::chops::Style::Breakcore { "breakbeat" } else { g.name };
                    let dsecs: Vec<crate::sounddesign::Sec> = secs.iter().map(|(a0, b0, t0)| crate::sounddesign::Sec { tag: t0.to_string(), start: *a0, end: *b0 }).collect();
                    let sr = self.out.sr;
                    let mut jobs: Vec<(usize, &str)> = tracks.iter().zip(&wanted_role).filter_map(|(t, r)| t.map(|t| (t, *r))).collect();
                    if let Some(t) = extra[2] {
                        jobs.push((t, "drums"));
                    }
                    if let Some(t) = riser_t {
                        jobs.push((t, "riser"));
                    }
                    for (t, role) in jobs {
                        let Some(tr) = en.project.tracks.get_mut(t) else { continue };
                        let has: Vec<String> = tr.effects.iter().map(|e| e.name.clone()).collect();
                        let inst = tr.inst_kind.clone();
                        let pd = crate::sounddesign::design_at(dg, &story.to_lowercase(), role, songgen::role_kind(role), &inst, design_amt, key.0, &dsecs, &has, bpm);
                        apply_design(tr, &pd, sr);
                        crate::sounddesign::apply_inst(tr, &pd);
                    }
                    design_note = format!("; sound design: {dg} ({} space)", crate::space::profile(dg).tier());
                }
                // the feel: the story / arc's emotional plan over the medley's sections, every medley track played with it
                let feel_amt = opt("feel").and_then(|v| v.parse::<f64>().ok()).unwrap_or(0.7).clamp(0.0, 1.0) as f32;
                let mut feel_note = String::new();
                if feel_amt > 0.0 {
                    let fsecs: Vec<crate::feel::Sec> = secs.iter().map(|(a0, b0, t0)| crate::feel::Sec { start: *a0, end: *b0, tag: t0.to_string() }).collect();
                    let which: Vec<usize> = tracks.iter().chain(extra.iter()).flatten().copied().collect();
                    let words: Vec<String> = vec![story.clone(); fsecs.len()];
                    let plan = crate::feel::plan_for(&en.project.tracks, &fsecs, &crate::feel::emotion_arc(&mash.arc), g.name, &words, key.1, seed);
                    let _ = crate::feel::perform(&mut en.project.tracks, &which, &fsecs, &plan, feel_amt);
                    let sections: Vec<songgen::Section> = fsecs.iter().map(|s| songgen::Section { tag: s.tag.clone(), bars: ((s.end - s.start) / 4.0).round().max(1.0) as u32, energy: 0.5 }).collect();
                    feel_note = format!("; feel: {} ({})", plan.arc, crate::emotion::summary(&plan, &sections));
                }
                // the rolls' filter contour on every medley track
                if !spans.is_empty() {
                    let (hp, lp) = crate::medley::roll_contour(&spans);
                    let sr = self.out.sr;
                    for t in tracks.iter().chain(extra.iter()).flatten() {
                        let Some(tr) = en.project.tracks.get_mut(*t) else { continue };
                        let Some(mut fx) = EffectSlot::new("Vox Filter", sr) else { continue };
                        for (k, v) in [("hp", 0.0f32), ("lp", 1.0), ("res", 0.3), ("dyn", 0.0), ("mix", 1.0)] {
                            if let Some(pi) = fx.params.iter().position(|p| p.0 == k) {
                                fx.set(pi, v);
                            }
                        }
                        let idx = tr.effects.len();
                        let hpi = fx.params.iter().position(|p| p.0 == "hp");
                        let lpi = fx.params.iter().position(|p| p.0 == "lp");
                        tr.effects.push(fx);
                        if let Some(pi) = hpi {
                            tr.auto.push(AutoLane { param: format!("fx{idx}.{pi}"), points: hp.clone() });
                        }
                        if let Some(pi) = lpi {
                            tr.auto.push(AutoLane { param: format!("fx{idx}.{pi}"), points: lp.clone() });
                        }
                    }
                }
                if empty {
                    let text = secs.iter().map(|(a, b, t)| format!("{a}:{b}:{t}")).collect::<Vec<_>>().join(";");
                    match en.project.meta.iter_mut().find(|(k, _)| k == "sections") {
                        Some(slot) => slot.1 = text,
                        None => en.project.meta.push(("sections".into(), text)),
                    }
                }
                drop(en);
                self.touch();
                let mut path: Vec<String> = vec![];
                for s in &mash.slots {
                    let k = crate::theory::Key { tonic: s.key.0, minor: s.key.1 }.name();
                    if path.last() != Some(&k) {
                        path.push(k);
                    }
                }
                let layers: usize = mash.slots.iter().map(|s| s.layers.len()).sum();
                let foreign = mash.slots.iter().flat_map(|s| s.layers.iter().map(move |l| (l.phrase, s.spine))).filter(|(a, b)| ph[*a].file != ph[*b].file).count();
                ok!(format!(
                    "{}: {} phrases from {} files ({} in all) at {:.0} bpm, {} bars; story: {}; keys: {}; {} clips ({} layered from other files) on {} instruments; {} loop rolls{}{}{}{}; order: {}{}",
                    if layering > 0.05 { "Mashup" } else { "Medley" },
                    order.len(),
                    files,
                    ph.len(),
                    bpm,
                    ((at - start0) / 4.0).round(),
                    mash.arc,
                    path.join(" > "),
                    layers,
                    foreign,
                    tracks.iter().flatten().count(),
                    spans.len(),
                    match (&chop_cfg, extra[2].is_some()) {
                        (Some((_, ch)), br) => format!("; vocal chops: {} chops at the transients of {chop_src} ({} pitched){}", ch.len(), ch.iter().filter(|c| c.midi.is_some()).count(), if br { ", with breaks" } else { "" }),
                        (None, true) => "; breaks".to_string(),
                        (None, false) if style != crate::chops::Style::Off => "; no vocal chops (no usable vocal found)".to_string(),
                        _ => String::new(),
                    },
                    feel_note,
                    if mash.log.is_empty() { String::new() } else { format!("; arrangement: {}", crate::medley::summarize(&mash.log)) },
                    design_note,
                    a.s(3),
                    if mode == crate::medley::Mode::Cipher && !message.is_empty() { format!(", the order spells a {}-character message (decode with the same folder and passphrase)", message.chars().count()) } else { String::new() }
                ))
            }
            "daw_song_feel" => {
                // (arc or words ("" / "auto" = read from the music), amount 0..1, seed, first track, track count (<= 0 = all), words) ->
                // a sentence. The tracks are played with an emotional plan (feel.rs): sections from the ruler's tags (or found from the
                // music, and then tagged), the arc named or read from the music, dynamics / articulation / room for the lead per
                // section, a level lane and a filter lane where there are none.
                let arc_in = a.s(0);
                let amount = if a.n(1) > 0.0 { a.n(1).clamp(0.0, 1.0) as f32 } else { 0.7 };
                let seed = a.n(2).abs() as u64;
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n = en.project.tracks.len();
                let first = (a.n(3).max(0.0) as usize).min(n);
                let which: Vec<usize> = if a.n(4) <= 0.0 { (0..n).collect() } else { (first..(first + a.n(4) as usize).min(n)).collect() };
                if which.is_empty() {
                    ok!("No tracks to play with feel".to_string());
                }
                let end = en.project.end_beat();
                let tagged = en.project.meta.iter().any(|(k, v)| k == "sections" && !v.trim().is_empty());
                let secs: Vec<crate::feel::Sec> = if tagged {
                    project_sections(&en).into_iter().map(|s| crate::feel::Sec { start: s.start, end: s.end, tag: s.tag }).collect()
                } else {
                    crate::feel::infer_sections(&en.project.tracks, end)
                };
                if !tagged {
                    let text = secs.iter().map(|s| format!("{}:{}:{}", s.start, s.end, s.tag)).collect::<Vec<_>>().join(";");
                    en.project.meta.push(("sections".into(), text));
                }
                // the mode, for an arc read from the music
                let mut h = [0f64; 12];
                for t in en.project.tracks.iter().filter(|t| t.inst_kind != "drums") {
                    for c in &t.clips {
                        if let ClipData::Midi(ns) = &c.data {
                            for x in ns {
                                h[(x.key % 12) as usize] += x.len.max(0.1);
                            }
                        }
                    }
                }
                let minor = crate::theory::detect_key_hist(&h).0.minor;
                let genre = en.project.meta.iter().find(|(k, _)| k == "ai.genre").map(|(_, v)| v.clone()).unwrap_or_else(|| "pop".into());
                let arc = match arc_in.trim().to_lowercase().as_str() {
                    "" | "auto" | "music" => String::new(),
                    x => crate::feel::arc_from_words(x).map(str::to_string).unwrap_or_else(|| crate::feel::emotion_arc(x)),
                };
                let words: Vec<String> = vec![a.s(5); secs.len()];
                let plan = crate::feel::plan_for(&en.project.tracks, &secs, &arc, &genre, &words, minor, seed);
                let done = crate::feel::perform(&mut en.project.tracks, &which, &secs, &plan, amount);
                let sections: Vec<songgen::Section> = secs.iter().map(|s| songgen::Section { tag: s.tag.clone(), bars: ((s.end - s.start) / 4.0).round().max(1.0) as u32, energy: 0.5 }).collect();
                let summary = crate::emotion::summary(&plan, &sections);
                en.project.meta.retain(|(k, _)| k != "feel.arc");
                en.project.meta.push(("feel.arc".into(), plan.arc.clone()));
                drop(en);
                self.touch();
                ok!(format!(
                    "Feel: {}{} over {} sections ({}){}{}",
                    plan.arc,
                    if arc.is_empty() { " (read from the music)" } else { "" },
                    secs.len(),
                    summary,
                    if done.is_empty() { String::new() } else { format!("; {}", done.join("; ")) },
                    if tagged { String::new() } else { "; sections tagged on the ruler".to_string() }
                ))
            }
            "daw_song_medley_decode" => {
                // (folder, phrase bars, passphrase) -> the message the project's medley clips spell ("" when they spell none)
                let (ph, _) = medley_phrases(&a.s(0), a.n(1));
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut seen: Vec<(f64, String)> = vec![];
                for t in &en.project.tracks {
                    for c in &t.clips {
                        if let Some((_, id)) = c.name.rsplit_once("· ") {
                            if !seen.iter().any(|s| (s.0 - c.start).abs() < 1e-6) {
                                seen.push((c.start, id.trim().to_string()));
                            }
                        }
                    }
                }
                drop(en);
                seen.sort_by(|x, y| x.0.total_cmp(&y.0));
                let idx: Vec<usize> = seen.iter().filter_map(|(_, id)| ph.iter().position(|p| crate::medley::short_id(p.id) == *id)).collect();
                ok!(crate::medley::decode(&idx, ph.len(), &a.s(2)).unwrap_or_default())
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
            "daw_cycle_shapes" => ok!(Val::List(crate::cycle::SHAPES.iter().map(|s| Val::Str(s.to_string())).collect())),
            "daw_cycle_plan" => {
                // (description, lyrics of all the songs, songs (0 = one per lyric block, at least 4), shape ("" = from the words /
                // dice), length scale, seed) -> [summary, shape, songs]; each song [title, plan line, lyrics, story, theme spec,
                // chapter, key, bpm, arc] - build one with daw_song_build(line, description, lyrics, "", seed, "", 0, "", story, theme)
                let length = if a.n(4) > 0.0 { a.n(4) } else { 1.0 };
                let c = crate::cycle::plan(&a.s(0), &a.s(1), a.u(2), a.s(3).trim(), length, a.n(5).max(1.0) as u64);
                let songs: Vec<Val> = c
                    .songs
                    .iter()
                    .map(|s| {
                        Val::List(vec![
                            Val::Str(s.title.clone()),
                            Val::Str(s.line.clone()),
                            Val::Str(s.lyrics.clone()),
                            Val::Str(s.story.clone()),
                            Val::Str(s.theme.clone()),
                            Val::Str(s.chapter.clone()),
                            Val::Str(s.key.name()),
                            Val::Num(s.bpm),
                            Val::Str(s.arc.clone()),
                        ])
                    })
                    .collect();
                ok!(Val::List(vec![Val::Str(c.summary()), Val::Str(c.shape.clone()), Val::List(songs)]))
            }
            "daw_song_express" => {
                // (voice track, amount 0..1, emotional arc ("" = auto), seed) -> summary: perform a sung track of any project line by
                // line and word by word - the sections from the ruler's tags, their feelings planned over them (emotion.rs)
                let t = a.u(0);
                let amount = a.n(1).clamp(0.0, 1.0) as f32;
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let secs = project_sections(&en);
                let sections: Vec<songgen::Section> = secs.iter().map(|s| songgen::Section { tag: s.tag.clone(), bars: ((s.end - s.start) / 4.0).round().max(1.0) as u32, energy: 0.5 }).collect();
                let words: Vec<String> = en
                    .project
                    .tracks
                    .get(t)
                    .map(|tr| secs.iter().map(|s| tr.clips.iter().filter(|c| c.start >= s.start - 1e-6 && c.start < s.end).map(|c| c.lyric_text.clone()).collect::<Vec<_>>().join("
")).collect())
                    .unwrap_or_default();
                let seed = a.n(3).max(1.0) as u64;
                let emo = crate::emotion::plan(&sections, a.s(2).trim(), "", &words, 0.4, seed);
                let spans: Vec<crate::expression::Span> = secs.iter().zip(&emo.feels).map(|(s, f)| crate::expression::Span { start: s.start, end: s.end, feel: *f }).collect();
                let bpm = en.project.bpm;
                let Some(tr) = en.project.tracks.get_mut(t) else { return Err("no such track".into()) };
                let said = crate::expression::perform_track(tr, &spans, bpm, amount, seed);
                drop(en);
                self.touch();
                let out = if said.is_empty() { "no sung notes on that track".to_string() } else { format!("{}; {}", crate::emotion::summary(&emo, &sections), said.join("; ")) };
                self.status = format!("Expression: {out}");
                ok!(out)
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
                // the movements' tempo map: the opening's tempo, a ritardando / accelerando or a plain change at every movement
                let _ = self.dispatch("daw_tempo_clear", &A(&[]));
                let tempo_map = songgen::tempo_changes(&plan);
                for (beat, bpm) in &tempo_map {
                    let _ = self.dispatch("daw_tempo_add", &A(&[Val::Num(*beat), Val::Num(*bpm)]));
                }
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
                    let ((kind, patch), tune_note) = instrument_in_tune(g, role, pi, (plan.seed % 3) as usize, self.out.sr);
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
                        // the performance line by line and word by word (expression.rs): optional 12th argument, how far it goes
                        // (0 = the flat per-section lanes above only; missing = 0.7)
                        let amount = if a.0.len() > 11 { a.n(11).clamp(0.0, 1.0) as f32 } else { 0.7 };
                        if amount > 0.0 {
                            let spans: Vec<crate::expression::Span> = plan
                                .sections
                                .iter()
                                .enumerate()
                                .filter_map(|(si, sec)| emo.feels.get(si).map(|f| crate::expression::Span { start: starts[si], end: starts[si] + sec.bars as f64 * 4.0, feel: *f }))
                                .collect();
                            let said = crate::expression::perform_track(tr, &spans, plan.bpm, amount, plan.seed);
                            emo.notes.extend(said);
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
                            for (si, (sec, st)) in plan.sections.iter().zip(&starts).enumerate() {
                                let end = st + sec.bars as f64 * 4.0;
                                // a pad-led movement keeps its filter half closed (smooth), the rest as the tag says
                                let pad_led = songgen::movement_of(&plan, si).map(|m| m.texture == "pads").unwrap_or(false);
                                let (a0, a1) = match sec.tag.as_str() {
                                    t if songgen::is_build_tag(t) => (open * 0.3, open),
                                    "intro" | "break" => (open * 0.55, open * 0.55),
                                    "outro" => (open, open * 0.3),
                                    _ if pad_led => (open * 0.6, open * 0.6),
                                    _ => (open, open),
                                };
                                pts.push((*st, a0));
                                // a build opens slowly first and fast at the end (the filter follows the roll), closing again in the gap
                                if songgen::is_build_tag(&sec.tag) {
                                    let gap = songgen::gap_after(&plan, si);
                                    let span = end - st - gap;
                                    pts.push((st + span * 0.5, a0 + (a1 - a0) * 0.22));
                                    pts.push((st + span * 0.8, a0 + (a1 - a0) * 0.55));
                                    pts.push((st + span - 0.01, a1));
                                    if gap > 0.0 {
                                        pts.push((st + span, a1 * 0.5));
                                    }
                                }
                                pts.push((end - 0.01, a1));
                            }
                            tr.auto.retain(|l| l.param != cut);
                            tr.auto.push(AutoLane { param: cut.clone(), points: pts });
                        }
                        // the riser swells through every build (louder as the roll speeds up), is gone in the gap and fades as the
                        // downlifter after the drop hits
                        if role == "riser" {
                            let v = (tr.volume / 1.5).clamp(0.0, 1.0);
                            let mut pts: Vec<(f64, f32)> = vec![(0.0, v)];
                            for (si, (sec, st)) in plan.sections.iter().zip(&starts).enumerate() {
                                let end = st + sec.bars as f64 * 4.0;
                                if songgen::is_build_tag(&sec.tag) {
                                    let span = end - st - songgen::gap_after(&plan, si);
                                    pts.push((*st, v * 0.2));
                                    pts.push((st + span * 0.6, v * 0.5));
                                    pts.push((st + span - 0.01, v));
                                    pts.push((end - 0.01, 0.0));
                                    if plan.sections.get(si + 1).map(|n| n.bars >= 2).unwrap_or(false) {
                                        pts.push((end, v));
                                        pts.push((end + 7.0, v * 0.1));
                                    }
                                }
                            }
                            if pts.len() > 1 {
                                tr.auto.retain(|l| l.param != "vol");
                                tr.auto.push(AutoLane { param: "vol".into(), points: pts });
                            }
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
                            let pd = sd::design_at(&plan.genre, &ldesc, role, songgen::role_kind(role), &tr.inst_kind, plan.dials.design, plan.key.tonic, &secs, &has, plan.bpm);
                            if pd.chain.is_empty() && pd.skin.is_none() {
                                continue;
                            }
                            apply_design(tr, &pd, sr);
                            sd::apply_inst(tr, &pd);
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
                ) + &{
                    let mv = songgen::movements(&plan);
                    if mv.len() > 1 {
                        format!(". Movements: {} ({} tempo changes)", mv.iter().map(|m| format!("{} {:.0} bpm", m.name, m.bpm)).collect::<Vec<_>>().join(" > "), tempo_map.len())
                    } else {
                        String::new()
                    }
                } + &format!(". Feel: {}", crate::emotion::summary(&emo, &plan.sections)) + &if designed.is_empty() { String::new() } else { format!(". Sound design: {}", designed.join("; ")) };
                self.status = format!("AI song: {summary}");
                ok!(Val::List(vec![Val::Num(made as f64), Val::Num(voice_t), Val::Num(g.lufs), Val::Num(bright), Val::Str(summary)]))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
