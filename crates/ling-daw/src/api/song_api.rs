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
            let stem = ["Growl ", "Voice ", "Prism "].iter().fold(stem.to_string(), |s, pre| s.strip_prefix(pre).map(|x| x.to_string()).unwrap_or(s));
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
fn instrument_for(g: &songgen::Genre, role: &str, part: usize) -> (String, Option<String>) {
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

fn title(role: &str) -> String {
    role.split(' ').map(|w| {
        let mut c = w.chars();
        c.next().map(|f| f.to_uppercase().collect::<String>() + c.as_str()).unwrap_or_default()
    }).collect::<Vec<_>>().join(" ")
}

impl Daw {
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
                    "Description: {}\n{}\nFill in this JSON for the song. genre: one of [{}]. bpm: a number. key: like \"F# minor\". progression: 4 chords for the verse that suit the genre and the mood, Roman numerals in the key (upper case major, lower case minor, b for a borrowed chord) like \"i VI III VII\". chorus_progression: 4 chords for the chorus, or \"\" when it uses the verse's. structure: sections with bars like \"intro 8, verse 16, chorus 8, outro 8\". instruments: 3 to 6 of [{}].\n{{\"genre\": \"\", \"bpm\": 0, \"key\": \"\", \"progression\": \"\", \"chorus_progression\": \"\", \"structure\": \"\", \"instruments\": \"\"}}",
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
                let p = songgen::plan_fit(&a.s(0), &a.s(1), &model, r.as_ref(), a.n(4).max(0.25), a.n(5).max(1.0) as u64, guide.as_ref(), a.n(7), &fit);
                ok!(Val::List(vec![Val::Str(p.to_line()), Val::Str(p.notes.join("; "))]))
            }
            "daw_song_build" => {
                // (plan line, description, lyrics, reference path or "", seed) -> [tracks made, voice track (-1 none), loudness target LUFS,
                // brightness -1..1, summary]. Builds at beat 0 of the current project (the app starts a new one first when asked).
                let desc = a.s(1);
                let lyrics = a.s(2);
                let r = if a.s(3).is_empty() { None } else { refs().lock().unwrap_or_else(|p| p.into_inner()).get(&a.s(3)).cloned() };
                let guide = load_guide(&a.s(5));
                // optional 8th: the length rule the plan was made with (so an unedited structure keeps its sections)
                let base = songgen::plan_fit(&desc, &lyrics, &[], r.as_ref(), 1.0, a.n(4).max(1.0) as u64, guide.as_ref(), a.n(6), &songgen::Fit::parse(&a.s(7)));
                let plan = SongPlan::from_line(&a.s(0), &base);
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
                let mut tracks: Vec<(usize, String)> = vec![];
                for (pi, part) in plan.parts.iter().enumerate() {
                    let role = part.role.as_str();
                    let (kind, patch) = instrument_for(g, role, pi);
                    let t = self.call_num("daw_track_add", &[Val::Str(kind.clone()), Val::Str(patch.clone().unwrap_or_default())]);
                    if t < 0.0 {
                        continue;
                    }
                    let t = t as usize;
                    made += 1;
                    let patch_name = patch.as_deref().and_then(|p| Path::new(p).file_stem()).map(|s| s.to_string_lossy().replace('_', " ")).unwrap_or_default();
                    let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t as f64), Val::Str(title(role))]));
                    let _ = patch_name;
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
                    tracks.push((t, role.to_string()));
                }
                // the singer: a line in the singer's range in every sung section, the lyrics fitted onto it
                let mut voice_t = -1.0;
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
                            let mut flat = Vec::with_capacity(notes.len() * 7);
                            for n in &notes {
                                flat.extend([Val::Num(n.start), Val::Num(n.len), Val::Num(n.key as f64), Val::Num(n.vel as f64), Val::Num(0.0), Val::Num(0.5), Val::Num(0.5)]);
                            }
                            let _ = self.dispatch("daw_clip_set_notes", &A(&[Val::Num(t), Val::Num(c), Val::List(flat)]));
                            let _ = self.dispatch("daw_clip_set_name", &A(&[Val::Num(t), Val::Num(c), Val::Str(sec.tag.clone())]));
                            let _ = self.dispatch("daw_lyrf_apply", &A(&[Val::Num(t), Val::Num(c), Val::Str(sung), Val::Str("fit:fit:1:0.3:all:1.0:auto".into())]));
                        }
                    }
                }
                // section tags on the ruler
                let tags: Vec<String> = plan.sections.iter().zip(&starts).map(|(s, st)| format!("{}:{}:{}", st, st + s.bars as f64 * 4.0, s.tag)).collect();
                let _ = self.dispatch("daw_arr_tags_set", &A(&[Val::Str(tags.join(";"))]));
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
                }
                self.touch();
                let bright = r.as_ref().map(|r| ((r.bright - 0.16) * 5.0).clamp(-1.0, 1.0)).unwrap_or(0.0);
                let summary = format!("{} at {:.0} bpm in {}: {} bars, {} tracks{}", plan.genre, plan.bpm, plan.key.name(), plan.bars(), made, if has_vocal { format!(", sung by {}", plan.singer) } else { String::new() });
                self.status = format!("AI song: {summary}");
                ok!(Val::List(vec![Val::Num(made as f64), Val::Num(voice_t), Val::Num(g.lufs), Val::Num(bright), Val::Str(summary)]))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
