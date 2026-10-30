//! Resample to wavetable and bounce to Sampler (`daw_wt_*`): any audio becomes a Growl / Prism wavetable (dsp/wavetable.rs), and a
//! clip becomes a playable Sampler track in one click.
//!
//! * `daw_wt_from_clip(target, track, clip, frames)`: an audio clip's audio as it is, a MIDI clip rendered alone through its track
//!   (instrument + effects) -> a table of `frames` frames for the target track's Growl / Prism. -> [ok, frames, pitch Hz, file]
//! * `daw_wt_from_note(target, track, key, frames)`: one note of `track` (2 beats, its whole chain) -> a table, the key as the pitch hint.
//! * `daw_wt_from_file(target, path, frames)`: a sample file (any format the Sampler reads) -> a table.
//! * `daw_wt_load(target, path)`: a wavetable file (2048-sample frames, Serum / Vital tables too) into the user slot. -> [ok, frames]
//! * `daw_wt_unload(target)`, `daw_wt_info(target)` -> [file, frames] ("" / 0 when none).
//! * `daw_wt_bounce_sampler(track, clip)`: the clip rendered alone (its track's instrument and effects) into `audio/`, then a new
//!   Sampler track that plays it on one zone over the whole keyboard (root = the pitch found, fine-tuned), with a MIDI clip that
//!   plays it where the clip was. -> the new track's index, -1 on failure.
//!
//! The tables are written into `<project>/wavetables/` (a temporary folder until the project is saved; saving packs the file into
//! the project's `samples/` like Ling Chip's samples) and the instrument keeps the path (project `config`).
use super::{Daw, A, Val};
use crate::dsp::wavetable;
use crate::model::*;
use crate::project;

/// mono mix of interleaved stereo
fn mono(st: &[f32]) -> Vec<f32> {
    st.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect()
}

fn note_name(k: u8) -> String {
    format!("{}{}", ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"][(k % 12) as usize], k as i32 / 12 - 1)
}

fn safe(s: &str) -> String {
    let t: String = s.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
    let t = t.trim().to_string();
    if t.is_empty() {
        "table".into()
    } else {
        t.chars().take(40).collect()
    }
}

impl Daw {
    pub(super) fn wt_dir(&self, sub: &str) -> std::path::PathBuf {
        if self.path.is_empty() {
            std::env::temp_dir().join(format!("lingstation_{sub}"))
        } else {
            std::path::Path::new(&self.path).parent().map(|p| p.join(sub)).unwrap_or_else(|| std::env::temp_dir().join(format!("lingstation_{sub}")))
        }
    }

    /// a file name in `dir` that is not taken yet
    pub(super) fn wt_fresh(dir: &std::path::Path, stem: &str, ext: &str) -> String {
        let _ = std::fs::create_dir_all(dir);
        let mut n = 1;
        loop {
            let name = if n == 1 { format!("{stem}.{ext}") } else { format!("{stem} {n}.{ext}") };
            let p = dir.join(&name);
            if !p.exists() {
                return p.to_string_lossy().into_owned();
            }
            n += 1;
        }
    }

    /// Render track `ti` alone in a private engine: its clip `ci` (moved to the start), or one note `key` (2 beats) when `note` is set.
    /// Returns interleaved stereo at the engine's rate and the length rendered in beats.
    pub(super) fn wt_render_alone(&mut self, ti: usize, ci: Option<usize>, note: Option<u8>) -> Result<Vec<f32>, String> {
        let sr = self.out.sr;
        let mut doc = {
            let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            project::to_doc(&en)
        };
        let mut td = doc.tracks.get(ti).cloned().ok_or("no such track")?;
        if td.kind == "audio" && note.is_some() {
            return Err("an audio track has no notes to play".into());
        }
        let (start, len) = match ci {
            Some(ci) => {
                let c = td.clips.get(ci).cloned().ok_or("no such clip")?;
                let (s, l) = (c.start, c.len.max(0.25));
                let mut c = c;
                c.start = 0.0;
                td.clips = vec![c];
                (s, l)
            }
            None => {
                td.clips.clear();
                (0.0, 2.0)
            }
        };
        td.session.clear();
        td.muted = false;
        td.solo = false;
        for a in td.auto.iter_mut() {
            for p in a.points.iter_mut() {
                p.0 -= start;
            }
        }
        doc.tracks = vec![td];
        doc.routes.clear();
        doc.master_effects.clear();
        doc.master_auto.clear();
        doc.tempo_map.clear();
        doc.limiter = false;
        doc.master_level = 1.0;
        doc.loop_on = false;
        doc.slots.clear();
        doc.scenes.clear();
        doc.session_only = false;
        let base = std::path::Path::new(&self.path).parent().map(|p| p.to_path_buf());
        let mut eng = crate::engine::Engine::new(sr);
        project::preload(&doc, base.as_deref(), &mut self.samples);
        project::apply_doc(&mut eng, &doc, base.as_deref(), &mut self.samples);
        if let (Some(k), Some(t)) = (note, eng.project.tracks.get_mut(0)) {
            let mut c = Clip::midi(0.0, len, "note");
            if let ClipData::Midi(n) = &mut c.data {
                n.push(Note::new(0.0, len, k, 0.9));
            }
            t.clips = vec![c];
        }
        let out = eng.bounce_range(0.0, len, 1.0);
        if out.iter().all(|x| x.abs() < 1e-5) {
            return Err("it renders silent".into());
        }
        Ok(out)
    }

    /// an audio clip's own audio (offset, length, gain), mono at its file's rate
    fn wt_clip_audio(&self, ti: usize, ci: usize) -> Option<(Vec<f32>, f32, String)> {
        let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        let t = en.project.tracks.get(ti)?;
        let c = t.clips.get(ci)?;
        let ClipData::Audio { sample, offset_s, gain, .. } = &c.data else { return None };
        let rate = sample.rate.max(1) as f64;
        let secs = c.len * 60.0 / en.project.bpm.max(1.0);
        let a = ((offset_s.max(0.0) * rate) as usize * 2).min(sample.stereo.len());
        let b = (a + (secs * rate) as usize * 2).min(sample.stereo.len());
        Some((mono(&sample.stereo[a..b]).into_iter().map(|x| x * gain).collect(), rate as f32, c.name.clone()))
    }

    /// cut `x` into a table, write it, load it into the target track; the reply for the builtins
    fn wt_make(&mut self, target: usize, x: &[f32], rate: f32, frames: usize, hint: Option<f32>, name: &str) -> Result<Val, String> {
        let frames = if frames == 0 { 32 } else { frames };
        let (cycles, f0) = match wavetable::analyse(x, rate, frames, hint) {
            Ok(r) => r,
            Err(e) => {
                self.status = format!("Resample to wavetable: {e}");
                return Ok(Val::List(vec![Val::Num(0.0)]));
            }
        };
        let path = Self::wt_fresh(&self.wt_dir("wavetables"), &safe(name), "wav");
        wavetable::save(&cycles, &path)?;
        let (ok, kind) = {
            let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            match en.project.tracks.get_mut(target) {
                Some(t) if t.inst_kind == "growl" || t.inst_kind == "prism" || t.inst_kind == "nova" => (t.inst.as_mut().map(|i| i.command("wt_load", &[], &path)).and_then(|r| r.first().copied()).unwrap_or(0.0), t.inst_kind.clone()),
                _ => (0.0, String::new()),
            }
        };
        let file = std::path::Path::new(&path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default();
        if ok > 0.5 {
            self.status = format!(
                "Resampled {name} into a {}-frame wavetable (pitch {:.1} Hz), saved as wavetables/{file}: {} plays it ({}; position scans the frames)",
                cycles.len(),
                f0,
                if kind == "growl" { "Ling Growl" } else if kind == "nova" { "Ling Nova" } else { "Ling Prism" },
                if kind == "growl" { "osc A, user table" } else if kind == "nova" { "osc A, the User table (any oscillator can pick it)" } else { "osc A, user table; b user for osc B" }
            );
        } else {
            self.status = format!("Wrote the wavetable {path} (the target track is not a Growl or Prism: load it from one with Load wavetable)");
        }
        self.touch();
        Ok(Val::List(vec![Val::Num(ok), Val::Num(cycles.len() as f64), Val::Num(f0 as f64), Val::Str(path)]))
    }

    pub(super) fn wt_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let sr = self.out.sr;
        match name {
            "daw_wt_from_clip" => {
                let (target, ti, ci, frames) = (a.u(0), a.u(1), a.u(2), a.u(3));
                // an audio clip as it is; a MIDI clip rendered through its track, its longest note the pitch to look for (a patch
                // with a sub an octave down repeats only every second cycle: the table follows the key, not the sub)
                let (x, rate, label, hint) = match self.wt_clip_audio(ti, ci) {
                    Some((x, rate, label)) => (x, rate, label, None),
                    None => {
                        let (label, key) = {
                            let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                            let c = en.project.tracks.get(ti).and_then(|t| t.clips.get(ci));
                            let key = c.and_then(|c| match &c.data {
                                ClipData::Midi(ns) => ns.iter().max_by(|a, b| a.len.total_cmp(&b.len)).map(|n| n.key),
                                _ => None,
                            });
                            (c.map(|c| c.name.clone()).unwrap_or_default(), key)
                        };
                        match self.wt_render_alone(ti, Some(ci), None) {
                            Ok(st) => (mono(&st), sr, label, key.map(|k| 440.0 * 2f32.powf((k as f32 - 69.0) / 12.0))),
                            Err(e) => {
                                self.status = format!("Resample to wavetable: {e}");
                                return Ok(Val::List(vec![Val::Num(0.0)]));
                            }
                        }
                    }
                };
                let label = if label.is_empty() { "clip".to_string() } else { label };
                self.wt_make(target, &x, rate, frames, hint, &label)
            }
            "daw_wt_from_note" => {
                let (target, ti, key, frames) = (a.u(0), a.u(1), a.u(2).min(127) as u8, a.u(3));
                let tname = {
                    let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.get(ti).map(|t| t.name.clone()).unwrap_or_default()
                };
                match self.wt_render_alone(ti, None, Some(key)) {
                    Ok(st) => {
                        let hint = 440.0 * 2f32.powf((key as f32 - 69.0) / 12.0);
                        self.wt_make(target, &mono(&st), sr, frames, Some(hint), &format!("{tname} {}", note_name(key)))
                    }
                    Err(e) => {
                        self.status = format!("Resample to wavetable: {e}");
                        Ok(Val::List(vec![Val::Num(0.0)]))
                    }
                }
            }
            "daw_wt_from_file" => {
                let (target, path, frames) = (a.u(0), a.s(1), a.u(2));
                match crate::dsp::sampler::load_audio(&path) {
                    Ok((st, rate)) => {
                        let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "sample".into());
                        self.wt_make(target, &mono(&st), rate as f32, frames, None, &stem)
                    }
                    Err(e) => {
                        self.status = format!("Resample to wavetable: {e}");
                        Ok(Val::List(vec![Val::Num(0.0)]))
                    }
                }
            }
            "daw_wt_load" => {
                let (target, path) = (a.u(0), a.s(1));
                let r = {
                    let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.get_mut(target).filter(|t| t.inst_kind == "growl" || t.inst_kind == "prism" || t.inst_kind == "nova").and_then(|t| t.inst.as_mut()).map(|i| i.command("wt_load", &[], &path)).unwrap_or_default()
                };
                let ok = r.first().copied().unwrap_or(0.0);
                self.status = if ok > 0.5 {
                    format!("Loaded the wavetable {} ({} frames)", std::path::Path::new(&path).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_default(), r.get(1).copied().unwrap_or(0.0))
                } else {
                    format!("{path}: not a wavetable (a WAV of 2048-sample frames) - Resample makes one from any sound")
                };
                self.touch();
                Ok(Val::List(r.into_iter().map(Val::Num).collect()))
            }
            "daw_wt_unload" => {
                let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(i) = en.project.tracks.get_mut(a.u(0)).and_then(|t| t.inst.as_mut()) {
                    i.command("unload", &[], "");
                }
                drop(en);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_wt_info" => {
                let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                let (f, n) = en
                    .project
                    .tracks
                    .get(a.u(0))
                    .filter(|t| t.inst_kind == "growl" || t.inst_kind == "prism" || t.inst_kind == "nova")
                    .and_then(|t| t.inst.as_ref())
                    .map(|i| (i.info("wavetable"), i.info("frames").parse::<f64>().unwrap_or(0.0)))
                    .unwrap_or_default();
                Ok(Val::List(vec![Val::Str(f), Val::Num(n)]))
            }
            "daw_wt_bounce_sampler" => {
                let (ti, ci) = (a.u(0), a.u(1));
                let info = {
                    let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.get(ti).and_then(|t| {
                        t.clips.get(ci).map(|c| {
                            let key = match &c.data {
                                ClipData::Midi(ns) => ns.iter().max_by(|a, b| a.len.total_cmp(&b.len)).map(|n| n.key),
                                _ => None,
                            };
                            (t.name.clone(), c.name.clone(), c.start, c.len, t.color, key)
                        })
                    })
                };
                let Some((tname, cname, start, len, color, key)) = info else { return Ok(Val::Num(-1.0)) };
                let st = match self.wt_render_alone(ti, Some(ci), None) {
                    Ok(st) => st,
                    Err(e) => {
                        self.status = format!("Bounce to Sampler: {e}");
                        return Ok(Val::Num(-1.0));
                    }
                };
                // trim the silent tail (keep 50 ms after the last sound)
                let last = st.iter().rposition(|x| x.abs() > 1e-4).map(|i| i / 2 + 1).unwrap_or(st.len() / 2);
                let keep = ((last + (sr * 0.05) as usize) * 2).min(st.len());
                let st = st[..keep].to_vec();
                let label = if cname.is_empty() { tname.clone() } else { format!("{tname} {cname}") };
                let path = Self::wt_fresh(&self.wt_dir("audio"), &safe(&format!("{label} bounce")), "wav");
                crate::export::write_audio(&path, &st, sr as u32, "wav", 24, 0.5)?;
                // the root: the pitch of the bounce (fine-tuned), else middle C
                let hint = key.map(|k| 440.0 * 2f32.powf((k as f32 - 69.0) / 12.0));
                let (root, tune) = match wavetable::detect_f0(&mono(&st), sr, hint) {
                    Some(f) => {
                        let k = 69.0 + 12.0 * (f / 440.0).log2();
                        let r = k.round().clamp(0.0, 127.0);
                        (r as u8, (r - k) * 100.0)
                    }
                    None => (60u8, 0.0),
                };
                let mut smp = crate::dsp::sampler::Sampler::new(sr);
                let mut z = crate::dsp::sampler::Zone::new(&label, &path);
                z.root = root;
                z.tune = tune;
                z.loop_end = (st.len() / 2) as u32;
                smp.add_loaded(z, st, sr as u32);
                let mut t = Track::new(&format!("{label} (sampler)"), TrackKind::Instrument, color, Some(Box::new(smp)));
                t.inst_kind = "sampler".into();
                let mut c = Clip::midi(start, len, &format!("{cname} bounce"));
                if let ClipData::Midi(n) = &mut c.data {
                    n.push(Note::new(0.0, len, root, 0.9));
                }
                t.clips.push(c);
                let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                if en.project.tracks.len() >= MAX_TRACKS {
                    return Ok(Val::Num(-1.0));
                }
                en.project.tracks.push(t);
                let n = en.project.tracks.len() - 1;
                drop(en);
                self.status = format!("Bounced {label} to a Sampler track (root {}{}): play it on the keys, or mute the original", note_name(root), if tune.abs() >= 1.0 { format!(" {:+.0} cents", tune) } else { String::new() });
                self.touch();
                Ok(Val::Num(n as f64))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
