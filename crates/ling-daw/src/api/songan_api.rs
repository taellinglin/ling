//! What AI Song reads from its sources beyond tempo, key and chords (`daw_song_an_*`, routed in api.rs before the other `daw_song_`
//! builtins): the labelled structure, energy curve, emotional story, hook, groove and parts of a guide MIDI; the sections, builds /
//! drops, vocal presence, loudness / brightness / width, swing and bass / lead palette of a reference recording (songanalysis.rs);
//! and an ACAPELLA (transcribe.rs) - its melody, fitted chords, tempo, key and layout become a guide ("acapella:<path>" wherever a
//! guide path goes, see song_api's `load_guide`), and after the build the vocal itself is placed on an audio track aligned to the song
//! (stretched to the plan's tempo, moved by whole semitones into its key when they differ), with its melody and fitted chords on
//! muted tracks beside it. After Vocal Chops, the acapella steps aside where the chop builds play and comes back on the drop.
use super::{Daw, A, Val};
use crate::guide::Guide;
use crate::model::*;
use crate::songanalysis::{self, AudioInfo, Shape};
use crate::songgen::{self, RefInfo};
use crate::transcribe::{self, Acapella};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};

fn lock<T>(m: &'static Mutex<T>) -> MutexGuard<'static, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

/// a reference's extra reading, by path
fn refs() -> &'static Mutex<HashMap<String, Arc<AudioInfo>>> {
    static R: OnceLock<Mutex<HashMap<String, Arc<AudioInfo>>>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// guide MIDI files read for their shape, by path
fn mids() -> &'static Mutex<HashMap<String, Arc<Guide>>> {
    static M: OnceLock<Mutex<HashMap<String, Arc<Guide>>>> = OnceLock::new();
    M.get_or_init(|| Mutex::new(HashMap::new()))
}

/// acapellas read, by path, and the guide each one makes
fn acas() -> &'static Mutex<HashMap<String, (Arc<Acapella>, Arc<Guide>)>> {
    static C: OnceLock<Mutex<HashMap<String, (Arc<Acapella>, Arc<Guide>)>>> = OnceLock::new();
    C.get_or_init(|| Mutex::new(HashMap::new()))
}

/// song_api's reference reader: songgen's measure, then the labelled sections and the rest of the recording's reading (kept here)
pub(super) fn analyze_reference(path: &str, stereo: &[f32], rate: u32) -> RefInfo {
    let mut r = songgen::analyze_reference(stereo, rate);
    let info = songanalysis::enrich_reference(&mut r, stereo, rate);
    lock(refs()).insert(path.to_string(), Arc::new(info));
    r
}

/// the guide for "acapella:<path>" (None for anything else): the acapella read with the tempo found in it when it was not read yet
pub(super) fn guide_for(spec: &str) -> Option<Arc<Guide>> {
    let path = spec.strip_prefix("acapella:")?;
    if let Some((_, g)) = lock(acas()).get(path) {
        return Some(g.clone());
    }
    let s = load_sample(path).ok()?;
    let a = transcribe::analyze(&s.stereo, s.rate, 0.0)?;
    let g = Arc::new(crate::guide::from_acapella(&a));
    lock(acas()).insert(path.to_string(), (Arc::new(a), g.clone()));
    Some(g)
}

fn midi_guide(path: &str) -> Option<Arc<Guide>> {
    if let Some(g) = lock(mids()).get(path) {
        return Some(g.clone());
    }
    let data = std::fs::read(path).ok()?;
    let mid = crate::midi_import::parse_any(path, &data).ok()?;
    let g = Arc::new(crate::guide::analyze(&mid)?);
    lock(mids()).insert(path.to_string(), g.clone());
    Some(g)
}

/// at most 64 points of a curve (means of equal slices)
fn curve(v: &[f64]) -> Vec<Val> {
    let n = v.len();
    if n <= 64 {
        return v.iter().map(|x| Val::Num((x * 1000.0).round() / 1000.0)).collect();
    }
    (0..64).map(|i| {
        let (a, b) = (i * n / 64, ((i + 1) * n / 64).max(i * n / 64 + 1));
        Val::Num(((v[a..b].iter().sum::<f64>() / (b - a) as f64) * 1000.0).round() / 1000.0)
    }).collect()
}

/// a hook as a theme spec for daw_song_plan ("notes:start,len,key,vel;...") or ""
fn hook_spec(s: &Shape) -> String {
    if s.hook_count < 2 || s.hook.is_empty() {
        return String::new();
    }
    format!("notes:{}", s.hook.iter().map(|n| format!("{:.3},{:.3},{},{:.2}", n.start, n.len, n.key, n.vel)).collect::<Vec<_>>().join(";"))
}

/// the start beat of every section of a structure text ("intro 8, verse 16, ...")
fn sections_of_text(text: &str) -> Vec<(String, f64, f64)> {
    let mut at = 0.0;
    let mut out = vec![];
    for item in text.split(',') {
        let w: Vec<&str> = item.split_whitespace().collect();
        if w.len() >= 2 {
            if let Ok(b) = w[1].parse::<f64>() {
                out.push((w[0].to_lowercase(), at, at + b * 4.0));
                at += b * 4.0;
            }
        }
    }
    out
}

/// where generated audio goes: the project's samples folder, or cache/acapella for an unsaved project
fn out_dir(project: &str) -> PathBuf {
    if project.is_empty() {
        PathBuf::from("cache/acapella")
    } else {
        Path::new(project).parent().map(|p| p.join("samples")).unwrap_or_else(|| PathBuf::from("cache/acapella"))
    }
}

impl Daw {
    pub(super) fn songan_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_song_an_info" => {
                // (source "midi" | "ref" | "aca", path) -> [structure, story, story arc, emotional arc, energy curve (<= 64 values 0..1),
                // details, palette words, loudness LUFS (0 = none), brightness target -1..1, width target 0..1 (-1 = none), hook theme spec]
                // or [] when the source was not read
                let (src, path) = (a.s(0), a.s(1));
                let row = |shape: &Shape, structure: String, details: String, palette: String, lufs: f64, bright: f64, width: f64| {
                    Val::List(vec![
                        Val::Str(structure),
                        Val::Str(shape.story.clone()),
                        Val::Str(shape.arc.clone()),
                        Val::Str(shape.emo_arc.clone()),
                        Val::List(curve(&shape.energy)),
                        Val::Str(details),
                        Val::Str(palette),
                        Val::Num(lufs),
                        Val::Num(bright),
                        Val::Num(width),
                        Val::Str(hook_spec(shape)),
                    ])
                };
                match src.as_str() {
                    "midi" => match midi_guide(&path) {
                        Some(g) if !g.shape.sections.is_empty() => ok!(row(&g.shape, g.shape.structure_text(), g.shape.details(), String::new(), 0.0, 0.0, -1.0)),
                        _ => ok!(Val::List(vec![])),
                    },
                    "ref" => {
                        let Some(i) = lock(refs()).get(&path).cloned() else { ok!(Val::List(vec![])) };
                        if i.shape.sections.is_empty() {
                            ok!(Val::List(vec![]))
                        }
                        ok!(row(&i.shape, i.shape.structure_text(), i.details(), i.palette_words(), i.lufs as f64, i.bright_target, i.width_target))
                    }
                    _ => {
                        let Some((ac, _)) = lock(acas()).get(&path).cloned() else { ok!(Val::List(vec![])) };
                        ok!(row(&ac.shape, ac.structure_text(), ac.describe(), String::new(), 0.0, 0.0, -1.0))
                    }
                }
            }
            "daw_song_an_acapella" => {
                // (path, tempo to use or 0 = find it, the reference's tempo or 0) -> [description, bpm, key, chords, chorus chords, structure,
                // story, a note about the tempo] or [] (the status line says why). Reads the vocal's melody, fits chords, finds the layout;
                // the guide it makes is "acapella:<path>". A reference tempo within 12 % (or twice / half of it) becomes the song's tempo
                // (the vocal is stretched to it when it is placed).
                let path = a.s(0);
                let s = match load_sample(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        self.status = format!("Acapella: {e}");
                        ok!(Val::List(vec![]))
                    }
                };
                if s.stereo.len() as f64 / 2.0 / s.rate as f64 > 900.0 {
                    self.status = "Acapella: over 15 minutes".into();
                    ok!(Val::List(vec![]))
                }
                let Some(ac) = transcribe::analyze(&s.stereo, s.rate, a.n(1).max(0.0)) else {
                    self.status = "Acapella: no sung notes found".into();
                    ok!(Val::List(vec![]))
                };
                let mut g = crate::guide::from_acapella(&ac);
                let mut note = format!("{:.0} bpm from the acapella", ac.bpm);
                let rb = a.n(2);
                if rb > 40.0 && a.n(1) <= 0.0 {
                    let fold = [rb, rb * 2.0, rb * 0.5].into_iter().min_by(|x, y| (x / ac.bpm).ln().abs().total_cmp(&(y / ac.bpm).ln().abs())).unwrap_or(rb);
                    if (fold / ac.bpm - 1.0).abs() <= 0.12 {
                        g.bpm = fold.round();
                        note = format!("{:.0} bpm from the reference (the vocal is stretched {:+.1}%)", g.bpm, (ac.bpm / g.bpm - 1.0) * 100.0);
                    }
                }
                let out = vec![
                    Val::Str(ac.describe()),
                    Val::Num(ac.bpm),
                    Val::Str(ac.key.name()),
                    Val::Str(ac.prog.clone()),
                    Val::Str(ac.chorus_prog.clone()),
                    Val::Str(ac.structure_text()),
                    Val::Str(ac.shape.story.clone()),
                    Val::Str(note),
                ];
                lock(acas()).insert(path, (Arc::new(ac), Arc::new(g)));
                ok!(Val::List(out))
            }
            "daw_song_an_place" => {
                // (acapella path, the plan's bpm, the plan's key name, the plan's structure "intro 4, verse 16 ...", layout 1 = the song was
                // planned on the acapella's own layout / 0 = a guide MIDI's (the vocal starts at the first section after the intro), lyrics
                // or "") -> [audio track, melody track, chords track, sentence] or [] (status). Places the vocal, stretched to the bpm and
                // moved into the key by whole semitones when they differ.
                let path = a.s(0);
                let Some((ac, _)) = lock(acas()).get(&path).cloned() else {
                    self.status = "Acapella: read it first".into();
                    ok!(Val::List(vec![]))
                };
                let s = match load_sample(&path) {
                    Ok(s) => s,
                    Err(e) => {
                        self.status = format!("Acapella: {e}");
                        ok!(Val::List(vec![]))
                    }
                };
                let bpm = if a.n(1) > 20.0 { a.n(1) } else { ac.bpm };
                let key = crate::theory::parse_key(&a.s(2)).unwrap_or(ac.key);
                let semis = crate::guide::key_shift(ac.key, key);
                let ratio = ac.bpm / bpm;
                // song beat of acapella grid beat 0
                let secs_list = sections_of_text(&a.s(3));
                let base = if a.n(4) > 0.5 || secs_list.is_empty() {
                    ac.lead_in as f64 * 4.0
                } else {
                    let first = secs_list.iter().find(|s| s.0 != "intro").map(|s| s.1).unwrap_or(0.0);
                    let sung_bar = ac.notes.first().map(|n| (n.start / 4.0).floor() * 4.0).unwrap_or(0.0);
                    first - sung_bar
                };
                // the clip starts just before the first sung note (the chops pair the audio with the melody by that)
                let mut trim = (ac.first_s - 0.15).max(0.0);
                let beat_of = |t: f64| base + (t - ac.origin_s) * ac.bpm / 60.0;
                if beat_of(trim) < 0.0 {
                    trim = ac.origin_s - base * 60.0 / ac.bpm;
                }
                let processed: Vec<f32> = if (ratio - 1.0).abs() > 0.004 || semis != 0 { crate::stretch::process(&s.stereo, ratio, semis as f64) } else { s.stereo.clone() };
                let stem = Path::new(&path).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_else(|| "acapella".into());
                let dir = out_dir(&self.path);
                let _ = std::fs::create_dir_all(&dir);
                let file = dir.join(format!("acapella_{}_{:.0}bpm{}.wav", stem.replace(' ', "_"), bpm, if semis != 0 { format!("_{semis:+}st") } else { String::new() }));
                if let Err(e) = super::song_api::write_wav16(&file, &processed, s.rate) {
                    self.status = format!("Acapella: {e}");
                    ok!(Val::List(vec![]))
                }
                let file_s = file.to_string_lossy().replace('\\', "/");
                let t = self.dispatch("daw_track_add", &A(&[Val::Str("audio".into()), Val::Str(String::new())])).ok().and_then(|v| if let Val::Num(x) = v { Some(x) } else { None }).unwrap_or(-1.0);
                if t < 0.0 {
                    ok!(Val::List(vec![]))
                }
                let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(t), Val::Str(format!("Acapella · {stem}"))]));
                let start = beat_of(trim).max(0.0);
                let c = match self.dispatch("daw_clip_add_audio", &A(&[Val::Num(t), Val::Str(file_s.clone()), Val::Num(start)])) {
                    Ok(Val::Num(c)) if c >= 0.0 => c as usize,
                    _ => {
                        ok!(Val::List(vec![]))
                    }
                };
                let out_secs = processed.len() as f64 / 2.0 / s.rate as f64;
                {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(cl) = en.project.tracks.get_mut(t as usize).and_then(|tr| tr.clips.get_mut(c)) {
                        if let ClipData::Audio { offset_s, .. } = &mut cl.data {
                            *offset_s = trim * ratio;
                        }
                        cl.len = ((out_secs - trim * ratio) * bpm / 60.0).max(0.25);
                        cl.name = format!("Acapella ({})", stem);
                    }
                }
                // the melody and the fitted chords beside it (muted: unmute to hear them, or to sing the melody with Ling Voice)
                let mut made = vec![];
                let notes: Vec<crate::theory::N> = ac.notes.iter().map(|n| crate::theory::N { start: n.start + base, key: (n.key as i32 + semis).clamp(0, 127) as u8, ..*n }).collect();
                let (chords, _) = transcribe::voiced(&ac.chords, 0.55);
                let chords: Vec<crate::theory::N> = chords.into_iter().map(|n| crate::theory::N { start: n.start + base, key: (n.key as i32 + semis).clamp(0, 127) as u8, ..n }).filter(|n| n.start >= 0.0).collect();
                for (kind, label, ns) in [("voice", "Acapella", notes), ("synth", "Acapella chords (fitted)", chords)] {
                    let ns: Vec<crate::theory::N> = ns.into_iter().filter(|n| n.start >= 0.0).collect();
                    if ns.is_empty() {
                        made.push(-1.0);
                        continue;
                    }
                    let mt = self.dispatch("daw_track_add", &A(&[Val::Str(kind.into()), Val::Str(String::new())])).ok().and_then(|v| if let Val::Num(x) = v { Some(x) } else { None }).unwrap_or(-1.0);
                    if mt < 0.0 {
                        made.push(-1.0);
                        continue;
                    }
                    let _ = self.dispatch("daw_track_set_name", &A(&[Val::Num(mt), Val::Str(label.into())]));
                    let c0 = (ns[0].start / 4.0).floor() * 4.0;
                    let c1 = ns.iter().map(|n| n.start + n.len).fold(0.0, f64::max);
                    let mc = match self.dispatch("daw_clip_add_midi", &A(&[Val::Num(mt), Val::Num(c0), Val::Num(((c1 - c0) / 4.0).ceil().max(1.0) * 4.0)])) {
                        Ok(Val::Num(x)) if x >= 0.0 => x,
                        _ => {
                            made.push(mt);
                            continue;
                        }
                    };
                    let mut flat = Vec::with_capacity(ns.len() * 7);
                    for n in &ns {
                        flat.extend([Val::Num(n.start - c0), Val::Num(n.len), Val::Num(n.key as f64), Val::Num(n.vel as f64), Val::Num(0.0), Val::Num(0.5), Val::Num(0.5)]);
                    }
                    let _ = self.dispatch("daw_clip_set_notes", &A(&[Val::Num(mt), Val::Num(mc), Val::List(flat)]));
                    let _ = self.dispatch("daw_clip_set_name", &A(&[Val::Num(mt), Val::Num(mc), Val::Str(label.into())]));
                    if kind == "voice" && !a.s(5).trim().is_empty() {
                        let _ = self.dispatch("daw_lyrf_apply", &A(&[Val::Num(mt), Val::Num(mc), Val::Str(a.s(5)), Val::Str("fit:fit:0:0.3:all:1.0:auto".into())]));
                    }
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(tr) = en.project.tracks.get_mut(mt as usize) {
                        tr.mute = true;
                    }
                    made.push(mt);
                }
                self.touch();
                let said = format!(
                    "the acapella on its own track{}{}, its melody ({} notes) and fitted chords on muted tracks",
                    if (ratio - 1.0).abs() > 0.004 { format!(", stretched {:+.1}% to {:.0} bpm", (ratio - 1.0) * 100.0, bpm) } else { String::new() },
                    if semis != 0 { format!(", moved {semis:+} semitone(s) into {}", key.name()) } else { String::new() },
                    ac.notes.len()
                );
                self.status = format!("AI song: {said}");
                ok!(Val::List(vec![Val::Num(t), Val::Num(made[0]), Val::Num(made[1]), Val::Str(said)]))
            }
            "daw_song_an_duck" => {
                // (acapella audio track) -> sentence. After Vocal Chops, "chops replace the vocal" the medley's way
                // (medley::acapella::plan_vox / vol_lane): builds and drops are chops only (the last of two or more drops layers the
                // vocal back in), verses / choruses / breakdowns sing; a section where no chop plays always sings. A vol lane on the track.
                use crate::medley::acapella::{plan_vox, vol_lane, SecIn, Vox};
                let t = a.u(0);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let mut secs: Vec<(f64, f64, String)> = vec![];
                if let Some((_, text)) = en.project.meta.iter().find(|(k, _)| k == "sections") {
                    for item in text.split(';') {
                        let f: Vec<&str> = item.split(':').collect();
                        if f.len() >= 3 {
                            if let (Ok(x), Ok(y)) = (f[0].parse::<f64>(), f[1].parse::<f64>()) {
                                if y > x {
                                    secs.push((x, y, f[2].to_string()));
                                }
                            }
                        }
                    }
                }
                secs.sort_by(|x, y| x.0.total_cmp(&y.0));
                // the chop notes, absolute
                let chop: Vec<f64> = en.project.tracks.iter().filter(|tr| (tr.name.starts_with("Chops") || tr.name.starts_with("Vocal Chops"))).flat_map(|tr| tr.clips.iter().flat_map(|c| match &c.data {
                    ClipData::Midi(ns) => ns.iter().filter(|n| n.start < c.len).map(|n| c.start + n.start).collect::<Vec<_>>(),
                    _ => vec![],
                })).collect();
                if chop.is_empty() || secs.is_empty() {
                    ok!("no chops to make room for")
                }
                let sections: Vec<songgen::Section> = secs.iter().map(|s| songgen::Section { tag: s.2.clone(), bars: ((s.1 - s.0) / 4.0).round().max(1.0) as u32, energy: 0.6 }).collect();
                let emo = crate::emotion::plan(&sections, "auto", "pop", &[], 0.5, 1);
                let ins: Vec<SecIn> = secs.iter().enumerate().map(|(i, s)| SecIn { tag: songanalysis::plan_tag(&s.2).to_string(), roll: false, feel: emo.feels.get(i).copied().unwrap_or(crate::emotion::Feel { valence: 0.0, arousal: 0.5, tension: 0.4, vocal: 0.5, breath: 0.3, space: 0.4 }) }).collect();
                let mut plan = plan_vox(&ins, true);
                for (i, s) in secs.iter().enumerate() {
                    if !chop.iter().any(|b| *b >= s.0 && *b < s.1) {
                        plan[i] = Vox::Full;
                    }
                }
                let Some(tr) = en.project.tracks.get_mut(t) else { ok!("") };
                let vol = tr.volume;
                let spans: Vec<(f64, f64)> = secs.iter().map(|s| (s.0, s.1)).collect();
                let out: Vec<String> = secs.iter().zip(&plan).filter(|(_, v)| !v.sings()).map(|(s, _)| s.2.clone()).collect();
                if out.is_empty() {
                    ok!("the chops play beside the acapella")
                }
                let pts = vol_lane(&spans, &plan, vol, 0.25, &[]);
                tr.auto.retain(|l| l.param != "vol");
                tr.auto.push(AutoLane { param: "vol".into(), points: pts });
                drop(en);
                self.touch();
                ok!(format!("the chops replace the acapella in: {} (it is back after them)", out.join(", ")))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn structure_text_gives_section_beats() {
        let v = sections_of_text("intro 4, verse 16, chorus 8");
        assert_eq!(v, vec![("intro".to_string(), 0.0, 16.0), ("verse".to_string(), 16.0, 80.0), ("chorus".to_string(), 80.0, 112.0)]);
    }
}
