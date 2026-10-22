//! Builtins for tuning a vocal recording to the notes of the score (`daw_vox_*`, see vocal.rs).
use super::theory_api::expand;
use super::{Daw, A, Val};
use crate::model::*;
use crate::vocal::{self, TuneOpts, VNote};
use std::sync::Arc;

impl Daw {
    pub(super) fn vox_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_vox_sources" => {
                // (audio track, clip) -> [track, name, notes in range]* for every melodic track with notes under the clip
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let Some(Clip { start, len, .. }) = en.project.tracks.get(a.u(0)).and_then(|t| t.clips.get(a.u(1))) else { return ok!(Val::List(vec![])) };
                let (s, e) = (*start, *start + *len);
                let mut v = vec![];
                for (ti, t) in en.project.tracks.iter().enumerate() {
                    if matches!(t.kind, TrackKind::Audio) || t.inst_kind == "drums" {
                        continue;
                    }
                    let n: usize = t.clips.iter().flat_map(|c| expand(c)).filter(|x| x.end() > s && x.start < e).count();
                    if n > 0 {
                        v.extend([Val::Num(ti as f64), Val::Str(t.name.clone()), Val::Num(n as f64)]);
                    }
                }
                ok!(Val::List(v))
            }
            "daw_vox_tune" => {
                // (audio track, clip, source track, strength, retune ms, align 0/1, max shift ms, vibrato 0/1, vibrato depth cents, vibrato rate Hz)
                //  -> [1, notes, anchors, voiced %, cents off before, cents off after] or [0, reason]
                let (t, c, src_t) = (a.u(0), a.u(1), a.u(2));
                let fail = |m: &str| Ok(Val::List(vec![Val::Num(0.0), Val::Str(m.to_string())]));
                let (sample, offset_s, gain, pitch, warp, spb, notes) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let bpm = en.project.bpm.max(20.0);
                    let Some(clip) = en.project.tracks.get(t).and_then(|x| x.clips.get(c)) else { return fail("no such clip") };
                    let ClipData::Audio { sample, offset_s, gain, pitch, warp } = &clip.data else { return fail("that is not an audio clip") };
                    let spb = if *warp > 1.0 { 60.0 / *warp as f64 } else { 60.0 / bpm };
                    let secs = sample.seconds();
                    let Some(src) = en.project.tracks.get(src_t) else { return fail("choose the track that has the notes") };
                    let shift = 2f64.powf(-(*pitch as f64) / 12.0);
                    let mut notes = vec![];
                    for sc in &src.clips {
                        for n in expand(sc) {
                            let on = *offset_s + (n.start - clip.start) * spb;
                            let off = *offset_s + (n.end() - clip.start) * spb;
                            if off > 0.0 && on < secs && n.start < clip.end() && n.end() > clip.start {
                                notes.push(VNote { on: on.max(0.0), off: off.min(secs), hz: vocal::midi_hz(n.key as f64) * shift });
                            }
                        }
                    }
                    (sample.clone(), *offset_s, *gain, *pitch, *warp, spb, notes)
                };
                let _ = spb;
                if notes.is_empty() {
                    return fail("the chosen track has no notes under this clip");
                }
                let opts = TuneOpts {
                    strength: a.n(3).clamp(0.0, 1.0),
                    retune_ms: a.n(4).clamp(0.0, 400.0),
                    align: a.n(5) > 0.5,
                    max_shift: (a.n(6) / 1000.0).clamp(0.01, 0.4),
                    vibrato: a.n(7) > 0.5,
                    vib_depth_cents: a.n(8).clamp(0.0, 100.0),
                    vib_rate: a.n(9).clamp(2.0, 9.0),
                    ..TuneOpts::default()
                };
                let (out, rep) = vocal::tune(&sample.stereo, sample.rate as f32, &notes, &opts);
                // the tuned audio is a file next to the project, so saving and re-opening keeps it
                let dir = if self.path.is_empty() { std::path::PathBuf::from("cache/vox") } else { std::path::Path::new(&self.path).parent().map(|p| p.join("audio")).unwrap_or_else(|| "cache/vox".into()) };
                std::fs::create_dir_all(&dir).map_err(|x| x.to_string())?;
                let base = sample.name.trim_end_matches(" (tuned)").trim().to_string();
                let mut file = dir.join(format!("{base} tuned.wav"));
                let mut k = 2;
                while file.exists() {
                    file = dir.join(format!("{base} tuned {k}.wav"));
                    k += 1;
                }
                let spec = hound::WavSpec { channels: 2, sample_rate: sample.rate, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
                {
                    let mut w = hound::WavWriter::create(&file, spec).map_err(|x| x.to_string())?;
                    for s in &out {
                        w.write_sample(*s).map_err(|x| x.to_string())?;
                    }
                    w.finalize().map_err(|x| x.to_string())?;
                }
                let path = file.to_string_lossy().into_owned();
                let new = Arc::new(Sample::new(format!("{base} (tuned)"), path.clone(), sample.rate, out));
                self.samples.insert(path, new.clone());
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                if let Some(clip) = en.project.tracks.get_mut(t).and_then(|x| x.clips.get_mut(c)) {
                    clip.data = ClipData::Audio { sample: new, offset_s, gain, pitch, warp };
                }
                drop(en);
                self.touch();
                ok!(Val::List(vec![Val::Num(1.0), Val::Num(rep.notes as f64), Val::Num(rep.anchors as f64), Val::Num((rep.voiced * 100.0).round()), Val::Num(rep.cents_before.round()), Val::Num(rep.cents_after.round())]))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
