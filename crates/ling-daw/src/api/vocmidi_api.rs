//! Vocal to MIDI (`daw_vm_*`): an imported vocal becomes a MIDI clip of the notes that were sung, linked to the recording. The notes are
//! edited like any others in the piano roll (move, stretch, scale snap, quantize); "render" then re-sings the recording through them with
//! the vocal tuner (pitch-synchronous resynthesis: the formants stay, only pitch and timing move), with vibrato, tremolo, fades and a
//! temperament on top, onto a paired audio track.
//!
//! The link lives in the project meta as `vox.<id>` = `key=value;...` and in the MIDI clip's tag (`vox:<id>`), so it is saved with the project.
use super::{Daw, A, Val};
use crate::model::*;
use crate::api::PALETTE;
use crate::vocal::{self, TuneOpts, VNote};
use std::sync::Arc;

pub(super) const SCALES: [(&str, [bool; 12]); 9] = [
    ("major", [true, false, true, false, true, true, false, true, false, true, false, true]),
    ("minor", [true, false, true, true, false, true, false, true, true, false, true, false]),
    ("harmonic minor", [true, false, true, true, false, true, false, true, true, false, false, true]),
    ("dorian", [true, false, true, true, false, true, false, true, false, true, true, false]),
    ("mixolydian", [true, false, true, false, true, true, false, true, false, true, true, false]),
    ("pentatonic major", [true, false, true, false, true, false, false, true, false, true, false, false]),
    ("pentatonic minor", [true, false, false, true, false, true, false, true, false, false, true, false]),
    ("blues", [true, false, false, true, false, true, true, true, false, false, true, false]),
    ("chromatic", [true; 12]),
];

fn kv_get(text: &str, key: &str) -> Option<String> {
    text.split(';').find_map(|p| p.split_once('=').filter(|(k, _)| *k == key).map(|(_, v)| v.to_string()))
}
fn kv_num(text: &str, key: &str, d: f64) -> f64 {
    kv_get(text, key).and_then(|v| v.parse().ok()).unwrap_or(d)
}
fn kv_set(text: &str, key: &str, val: &str) -> String {
    let mut parts: Vec<String> = text.split(';').filter(|p| !p.is_empty() && !p.starts_with(&format!("{key}="))).map(|s| s.to_string()).collect();
    parts.push(format!("{key}={val}"));
    parts.join(";")
}

fn vox_id(tag: &str) -> Option<String> {
    tag.strip_prefix("vox:").map(|s| s.to_string())
}

fn meta_get(p: &Project, id: &str) -> Option<String> {
    let key = format!("vox.{id}");
    p.meta.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone())
}
fn meta_put(p: &mut Project, id: &str, v: String) {
    let key = format!("vox.{id}");
    if let Some(e) = p.meta.iter_mut().find(|(k, _)| *k == key) {
        e.1 = v;
    } else {
        p.meta.push((key, v));
    }
}

fn tune_opts(link: &str) -> TuneOpts {
    TuneOpts {
        strength: kv_num(link, "strength", 1.0).clamp(0.0, 1.0),
        retune_ms: kv_num(link, "retune", 25.0).clamp(0.0, 400.0),
        align: kv_num(link, "align", 1.0) > 0.5,
        max_shift: (kv_num(link, "maxshift", 120.0) / 1000.0).clamp(0.01, 0.4),
        vibrato: kv_num(link, "vdepth", 0.0) > 0.5,
        vib_depth_cents: kv_num(link, "vdepth", 0.0).clamp(0.0, 100.0),
        vib_rate: kv_num(link, "vrate", 5.5).clamp(2.0, 9.0),
        vib_min_len: 0.25,
        trem_depth: kv_num(link, "tdepth", 0.0).clamp(0.0, 1.0),
        trem_rate: kv_num(link, "trate", 5.0).clamp(0.5, 12.0),
        fade_in: kv_num(link, "fin", 0.0).clamp(0.0, 2.0),
        fade_out: kv_num(link, "fout", 0.0).clamp(0.0, 2.0),
    }
}

impl Daw {
    pub(super) fn vm_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_vm_scales" => ok!(Val::List(SCALES.iter().map(|s| Val::Str(s.0.to_string())).collect())),
            "daw_vm_temperaments" => ok!(Val::List(vocal::TEMPERAMENT_NAMES.iter().map(|s| Val::Str(s.to_string())).collect())),
            "daw_vm_convert" => {
                // (audio track, clip) -> index of the new MIDI track, or -1 (see the status line)
                let (t, c) = (a.u(0), a.u(1));
                let (sample, offset_s, start, len, spb, tname, cname) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let bpm = en.project.bpm.max(20.0);
                    let Some(tr) = en.project.tracks.get(t) else { return ok!(-1.0) };
                    let Some(clip) = tr.clips.get(c) else { return ok!(-1.0) };
                    let ClipData::Audio { sample, offset_s, warp, .. } = &clip.data else {
                        self.status = "Vocal to MIDI: select an audio clip".into();
                        return ok!(-1.0);
                    };
                    let spb = if *warp > 1.0 { 60.0 / *warp as f64 } else { 60.0 / bpm };
                    (sample.clone(), *offset_s, clip.start, clip.len, spb, tr.name.clone(), clip.name.clone())
                };
                let mono = vocal::mono_of(&sample.stereo);
                if mono.len() < (sample.rate as usize / 5) {
                    self.status = "Vocal to MIDI: that clip is too short".into();
                    return ok!(-1.0);
                }
                let pitch = vocal::track_pitch(&mono, sample.rate as f32);
                let mut found = vocal::detect_notes(&pitch);
                // loudness of every note (RMS over its span) to velocity
                let sr = sample.rate as f64;
                let rms: Vec<f64> = found
                    .iter()
                    .map(|n| {
                        let (i0, i1) = ((n.on * sr) as usize, ((n.off * sr) as usize).min(mono.len()));
                        if i1 <= i0 { 0.0 } else { (mono[i0..i1].iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / (i1 - i0) as f64).sqrt() }
                    })
                    .collect();
                let top = rms.iter().cloned().fold(1e-6, f64::max);
                for (n, r) in found.iter_mut().zip(&rms) {
                    n.vel = (0.35 + 0.65 * (r / top).clamp(0.0, 1.0)) as f32;
                }
                let clip_secs = len * spb;
                let notes: Vec<Note> = found
                    .iter()
                    .filter(|n| n.off > offset_s && n.on < offset_s + clip_secs && n.midi >= 0.0 && n.midi < 128.0)
                    .map(|n| {
                        let on = (n.on - offset_s).max(0.0);
                        let off = (n.off - offset_s).min(clip_secs);
                        Note::new(on / spb, ((off - on) / spb).max(0.05), n.midi.round().clamp(0.0, 127.0) as u8, n.vel)
                    })
                    .collect();
                if notes.is_empty() {
                    self.status = "Vocal to MIDI: no sung notes found in that clip".into();
                    return ok!(-1.0);
                }
                let sr_f = { eng.lock().unwrap_or_else(|p| p.into_inner()).sr };
                let id = format!("{}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() % 100000000).unwrap_or(0));
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let n_tr = en.project.tracks.len();
                if n_tr >= MAX_TRACKS {
                    self.status = "Vocal to MIDI: no room for another track".into();
                    return ok!(-1.0);
                }
                let inst = crate::dsp::make_instrument("synth", sr_f);
                let mut mt = Track::new(&format!("{tname} MIDI"), TrackKind::Instrument, PALETTE[n_tr % 7], inst);
                mt.inst_kind = "synth".into();
                // the notes are an editing surface: the vocal itself is what you hear until the tuned version replaces it
                mt.mute = true;
                let mut mc = Clip::midi(start, len, &format!("{cname} notes"));
                mc.tag = format!("vox:{id}");
                let n_notes = notes.len();
                mc.data = ClipData::Midi(notes);
                mt.clips.push(mc);
                en.project.tracks.push(mt);
                let link = format!("src={};off={};spb={};tname={};cstart={}", sample.path, offset_s, spb, tname, start);
                meta_put(&mut en.project, &id, link);
                drop(en);
                self.touch();
                self.status = format!("Vocal to MIDI: {n_notes} notes from {cname}");
                ok!(n_tr)
            }
            "daw_vm_info" => {
                // (track, clip) -> [] when the clip is not a vocal's notes, else [id, strength, retune ms, align, vibrato cents, vibrato Hz,
                //                  tremolo depth, tremolo Hz, fade in s, fade out s, temperament, tonic, notes, rendered 0/1]
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(clip) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return ok!(Val::List(vec![])) };
                let Some(id) = vox_id(&clip.tag) else { return ok!(Val::List(vec![])) };
                let link = meta_get(&en.project, &id).unwrap_or_default();
                let notes = if let ClipData::Midi(n) = &clip.data { n.len() } else { 0 };
                let tn = en.project.tracks.get(a.u(0)).map(|t| t.name.clone()).unwrap_or_default();
                let rendered = en.project.tracks.iter().any(|t| t.name == format!("{tn} voice"));
                let g = |k: &str, d: f64| Val::Num(kv_num(&link, k, d));
                ok!(Val::List(vec![
                    Val::Str(id),
                    g("strength", 1.0),
                    g("retune", 25.0),
                    g("align", 1.0),
                    g("vdepth", 0.0),
                    g("vrate", 5.5),
                    g("tdepth", 0.0),
                    g("trate", 5.0),
                    g("fin", 0.0),
                    g("fout", 0.0),
                    g("temp", 0.0),
                    g("troot", 0.0),
                    Val::Num(notes as f64),
                    Val::Num(rendered as u8 as f64),
                ]))
            }
            "daw_vm_set" => {
                // (track, clip, key, value): strength retune align vdepth vrate tdepth trate fin fout temp troot maxshift
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(id) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))).and_then(|c| vox_id(&c.tag)) else { return ok!(false) };
                let link = meta_get(&en.project, &id).unwrap_or_default();
                let key = a.s(2);
                if !["strength", "retune", "align", "vdepth", "vrate", "tdepth", "trate", "fin", "fout", "temp", "troot", "maxshift"].contains(&key.as_str()) {
                    return ok!(false);
                }
                let v = kv_set(&link, &key, &format!("{}", a.n(3)));
                meta_put(&mut en.project, &id, v);
                drop(en);
                self.touch();
                ok!(true)
            }
            "daw_vm_snap" => {
                // (track, clip, scale index, tonic 0..11, strength 0..1) -> notes moved; each note goes to the nearest note the scale allows
                let (t, c, sc, root, strength) = (a.u(0), a.u(1), a.u(2).min(SCALES.len() - 1), (a.n(3) as i64).rem_euclid(12) as i32, a.n(4).clamp(0.0, 1.0));
                let mut allowed = [false; 12];
                for (k, on) in SCALES[sc].1.iter().enumerate() {
                    allowed[(k as i32 + root).rem_euclid(12) as usize] = *on;
                }
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(ns), .. }) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) else { return ok!(0.0) };
                let mut moved = 0;
                let mut prev: Option<i32> = None;
                ns.sort_by(|x, y| x.start.total_cmp(&y.start));
                for n in ns.iter_mut() {
                    let want = vocal_nearest(n.key as f64, &allowed, prev);
                    prev = Some(want);
                    let new = (n.key as f64 + (want as f64 - n.key as f64) * strength).round().clamp(0.0, 127.0) as u8;
                    if new != n.key {
                        n.key = new;
                        moved += 1;
                    }
                }
                drop(en);
                if moved > 0 {
                    self.touch();
                }
                ok!(moved)
            }
            "daw_vm_quantize" => {
                // (track, clip, grid in beats, strength 0..1, also snap the note ends 0/1) -> notes moved
                let (t, c, grid, strength, ends) = (a.u(0), a.u(1), a.n(2).max(0.03125), a.n(3).clamp(0.0, 1.0), a.n(4) > 0.5);
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { data: ClipData::Midi(ns), .. }) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) else { return ok!(0.0) };
                let mut moved = 0;
                for n in ns.iter_mut() {
                    let end = n.start + n.len;
                    let s2 = n.start + ((n.start / grid).round() * grid - n.start) * strength;
                    let e2 = if ends { end + (((end / grid).round() * grid).max(s2 + grid * 0.25) - end) * strength } else { end + (s2 - n.start) };
                    if (s2 - n.start).abs() > 1e-6 || (e2 - end).abs() > 1e-6 {
                        n.start = s2.max(0.0);
                        n.len = (e2 - s2).max(0.05);
                        moved += 1;
                    }
                }
                drop(en);
                if moved > 0 {
                    self.touch();
                }
                ok!(moved)
            }
            "daw_vm_render" => {
                // (track, clip) -> [1, notes, anchors, voiced %, cents off before, cents off after] or [0, reason]
                let (t, c) = (a.u(0), a.u(1));
                let fail = |m: &str| Ok(Val::List(vec![Val::Num(0.0), Val::Str(m.to_string())]));
                let (id, link, notes_beats, mstart, mlen, tname, mname, mcol) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(tr) = en.project.tracks.get(t) else { return fail("no such track") };
                    let Some(clip) = tr.clips.get(c) else { return fail("no such clip") };
                    let Some(id) = vox_id(&clip.tag) else { return fail("that clip is not a vocal's notes") };
                    let ClipData::Midi(ns) = &clip.data else { return fail("that clip is not MIDI") };
                    (id.clone(), meta_get(&en.project, &id).unwrap_or_default(), ns.clone(), clip.start, clip.len, tr.name.clone(), clip.name.clone(), tr.color)
                };
                let src = kv_get(&link, "src").unwrap_or_default();
                let offset_s = kv_num(&link, "off", 0.0);
                let spb = kv_num(&link, "spb", 0.5).max(0.05);
                let sample = match self.load_sample_cached(&src) {
                    Ok(s) => s,
                    Err(e) => return fail(&format!("the recording is gone ({e})")),
                };
                let (temp, troot) = (kv_num(&link, "temp", 0.0) as usize, kv_num(&link, "troot", 0.0) as i32);
                let vnotes: Vec<VNote> = notes_beats
                    .iter()
                    .map(|n| {
                        let on = offset_s + n.start * spb;
                        VNote { on, off: on + n.len * spb, hz: vocal::key_hz(n.key as f64, temp, troot) }
                    })
                    .collect();
                if vnotes.is_empty() {
                    return fail("the clip has no notes");
                }
                let opts = tune_opts(&link);
                let (out, rep) = vocal::tune(&sample.stereo, sample.rate as f32, &vnotes, &opts);
                // write the audio next to the project (so saving and re-opening keeps it) and put it on the paired track
                let dir = if self.path.is_empty() { std::path::PathBuf::from("cache/vox") } else { std::path::Path::new(&self.path).parent().map(|p| p.join("audio")).unwrap_or_else(|| "cache/vox".into()) };
                std::fs::create_dir_all(&dir).map_err(|x| x.to_string())?;
                let file = dir.join(format!("vocal_{id}.wav"));
                let spec = hound::WavSpec { channels: 2, sample_rate: sample.rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
                {
                    let mut w = hound::WavWriter::create(&file, spec).map_err(|x| x.to_string())?;
                    for s in &out {
                        w.write_sample(*s).map_err(|x| x.to_string())?;
                    }
                    w.finalize().map_err(|x| x.to_string())?;
                }
                let path = file.to_string_lossy().into_owned();
                let new = Arc::new(Sample::new(format!("{mname} (tuned)"), path.clone(), sample.rate, out));
                self.samples.insert(path, new.clone());
                let voice_name = format!("{tname} voice");
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let vt = match en.project.tracks.iter().position(|x| x.name == voice_name) {
                    Some(i) => i,
                    None => {
                        let n = en.project.tracks.len();
                        if n >= MAX_TRACKS {
                            return fail("no room for the tuned voice track");
                        }
                        en.project.tracks.push(Track::new(&voice_name, TrackKind::Audio, mcol, None));
                        n
                    }
                };
                en.project.tracks[vt].clips.clear();
                en.project.tracks[vt].clips.push(Clip {
                    start: mstart,
                    len: mlen,
                    name: format!("{mname} (tuned)"),
                    loop_len: 0.0,
                    lyric_text: String::new(),
                    lyric_lang: String::new(),
                    tag: String::new(),
                    data: ClipData::Audio { sample: new, offset_s, gain: 1.0, pitch: 0.0, warp: kv_num(&link, "warp", 0.0) as f32 },
                });
                // the untouched recording gives way to the tuned one (once; later you may unmute it to compare)
                if kv_get(&link, "muted").is_none() {
                    let sname = kv_get(&link, "tname").unwrap_or_default();
                    if let Some(st) = en.project.tracks.iter_mut().find(|x| x.name == sname && matches!(x.kind, TrackKind::Audio)) {
                        st.mute = true;
                    }
                    let l2 = kv_set(&link, "muted", "1");
                    meta_put(&mut en.project, &id, l2);
                }
                drop(en);
                self.touch();
                self.status = format!("Vocal tuned: {} notes, {:.0} cents off before, {:.0} after", rep.notes, rep.cents_before, rep.cents_after);
                ok!(Val::List(vec![Val::Num(1.0), Val::Num(rep.notes as f64), Val::Num(rep.anchors as f64), Val::Num((rep.voiced * 100.0).round()), Val::Num(rep.cents_before.round()), Val::Num(rep.cents_after.round())]))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}

fn vocal_nearest(m: f64, allowed: &[bool; 12], prev: Option<i32>) -> i32 {
    vocal::nearest_allowed(m, allowed, prev)
}
