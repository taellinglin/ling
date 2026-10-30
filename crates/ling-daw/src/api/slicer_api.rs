//! Builtins for the Ling Slicer (`daw_slc_*`, see `dsp/slicer.rs`): loading a file, the slices and their markers, the harmony the
//! auto-tune follows, writing the slices to the piano roll, and slicing a clip of the arrangement.
//!
//! * `daw_slc_new(path)` -> a new Slicer track (the file loaded and sliced when a path is given), -1 on failure.
//! * `daw_slc_open(track, path)` -> slices (-1 when the file cannot be read; the status says why).
//! * `daw_slc_slices(track)` -> per slice `slicer::FIELDS` then the pitch heard (MIDI, -1 = unpitched), its confidence and the pitch it
//!   plays at now; `daw_slc_peaks(track, n)` -> [lo, hi] * n; `daw_slc_bpm(track)` -> [file bpm (0 = unknown), frames, rate].
//! * `daw_slc_set(track, slice, field, value)`, `daw_slc_set_all(track, field, value)`, `daw_slc_add(track, frame)`,
//!   `daw_slc_move(track, slice, frame)`, `daw_slc_del(track, slice)`, `daw_slc_reslice(track)`.
//! * `daw_slc_refs(track)` -> [index, name, ...] the MIDI tracks the auto-tune can follow; `daw_slc_ref(track, ref)` (-1 = the whole
//!   song) and `daw_slc_get_ref(track)`.
//! * `daw_slc_sync(track, force)`: the harmony timeline (key sections + the chord at every change of the reference's notes, read as
//!   chops/tune.rs reads them) and the plan of the track's own notes, sent to the instrument, which renders the shifted slices ahead.
//!   Done when the project changed since the last time (the panel asks every frame), and for every Slicer track when the transport
//!   starts or a render begins (`slc_sync_all`). -> [segments, plan, prepared].
//! * `daw_slc_roll(track, variant, seed)`: a MIDI clip at the playhead's bar that plays the slices in their order and timing
//!   (variant 0), shuffled (1) or with stutters (2). -> clip index.
//! * `daw_slc_from_clip(track, clip, replace)`: the clip's audio (an audio clip as it is, a MIDI clip rendered alone through its
//!   track) written to `audio/`, a new Slicer track with it and the clip that replays it where the clip was; `replace` 1 deletes
//!   the original clip. -> the new track's index, -1 on failure.
//! * `daw_slc_timeline(track)` -> the harmony as text (start, key, chord tones per line).
//! * `daw_slc_hype(explicit)`: after New AI Song (its HYPE SHOUT switch, off by default), a hype shout kit on a new Slicer track that
//!   ends on every drop the generator swept into. -> the new track, -1 when the song has no drop. Make Medley's `hype=` option does
//!   the same on its drops ([`Daw::slc_hype_at`]).
use super::{Daw, Val, A};
use crate::dsp::slicer;
use crate::model::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

/// per track: the project revision the last sync saw
fn synced() -> &'static Mutex<HashMap<usize, u64>> {
    static S: OnceLock<Mutex<HashMap<usize, u64>>> = OnceLock::new();
    S.get_or_init(|| Mutex::new(HashMap::new()))
}

/// the tracks whose notes are harmony: instrument tracks that are not drums, not Slicers, not sound effects (Ling Cinema: sweeps,
/// impacts), not a sung part (notes with lyrics: the melody is not the chord), not muted
fn harmonic(t: &Track) -> bool {
    let sings = t.clips.iter().any(|c| !c.lyric_text.trim().is_empty() || matches!(&c.data, ClipData::Midi(ns) if ns.iter().any(|n| !n.lyric.trim().is_empty() && !n.lyric.starts_with('@'))));
    !matches!(t.kind, TrackKind::Audio) && !matches!(t.inst_kind.as_str(), "drums" | "slicer" | "cinema") && !t.mute && !sings
}

/// the notes of a track in absolute beats (loops unrolled)
fn notes_of(t: &Track) -> Vec<Note> {
    t.clips.iter().flat_map(|c| super::theory_api::expand(c)).map(|n| Note::new(n.start, n.len, n.key, n.vel)).collect()
}

/// the harmony a Slicer on track `ti` follows: its reference track (`reference` = n + 1), else every harmonic track but itself
pub fn harmony_for(p: &Project, ti: usize, reference: usize) -> Vec<Note> {
    if reference > 0 {
        return p.tracks.get(reference - 1).map(notes_of).unwrap_or_default();
    }
    p.tracks.iter().enumerate().filter(|(i, t)| *i != ti && harmonic(t)).flat_map(|(_, t)| notes_of(t)).collect()
}

/// the timeline as the numbers of the instrument's "timeline" command
fn timeline_args(tl: &[slicer::Seg]) -> Vec<f64> {
    let mut v = vec![tl.len() as f64];
    for s in tl {
        v.extend([s.start, if s.end > 1e12 { -1.0 } else { s.end }, s.key.map(|k| (k.tonic + if k.minor { 12 } else { 0 }) as f64).unwrap_or(-1.0), s.chord.iter().fold(0u32, |m, p| m | (1 << p)) as f64]);
    }
    v
}

fn mono(st: &[f32]) -> Vec<f32> {
    st.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect()
}

impl Daw {
    fn slc_cmd(&mut self, t: usize, cmd: &str, args: &[f64], text: &str) -> Vec<f64> {
        let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        en.project.tracks.get_mut(t).filter(|t| t.inst_kind == "slicer").and_then(|t| t.inst.as_mut()).map(|i| i.command(cmd, args, text)).unwrap_or_default()
    }

    /// The harmony timeline and the note plan of Slicer track `t`, sent to it (see the module notes); None when it is not a Slicer
    pub(super) fn slc_sync(&mut self, t: usize) -> Option<Vec<f64>> {
        let (tl_args, sync_args) = {
            let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            let tr = en.project.tracks.get(t).filter(|x| x.inst_kind == "slicer")?;
            let reference = tr.inst.as_ref().map(|i| i.info("ref").parse::<usize>().unwrap_or(0)).unwrap_or(0);
            let harmony = harmony_for(&en.project, t, reference);
            let tl = slicer::timeline(&harmony, 4.0);
            let mut sync = vec![en.project.bpm];
            for n in notes_of(tr) {
                sync.extend([n.start, n.key as f64]);
            }
            (timeline_args(&tl), sync)
        };
        let segs = self.slc_cmd(t, "timeline", &tl_args, "");
        let r = self.slc_cmd(t, "sync", &sync_args, "");
        synced().lock().unwrap_or_else(|p| p.into_inner()).insert(t, self.rev);
        Some(segs.into_iter().chain(r).collect())
    }

    /// every Slicer track's harmony and plan, before the transport starts or a render takes the project
    pub(super) fn slc_sync_all(&mut self) {
        let n = {
            let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            if !en.project.tracks.iter().any(|t| t.inst_kind == "slicer") {
                return;
            }
            en.project.tracks.len()
        };
        for t in 0..n {
            let _ = self.slc_sync(t);
        }
    }

    /// a new Slicer track (named after the file), the file loaded; its index
    fn slc_new_track(&mut self, path: &str, name: &str) -> Option<usize> {
        let n = match self.dispatch("daw_track_add", &A(&[Val::Str("slicer".into()), Val::Str(String::new())])) {
            Ok(Val::Num(n)) if n >= 0.0 => n as usize,
            _ => return None,
        };
        {
            let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(t) = en.project.tracks.get_mut(n) {
                t.name = if name.is_empty() { format!("Slicer {}", n + 1) } else { format!("{name} (slicer)") };
            }
        }
        if !path.is_empty() {
            let r = self.slc_cmd(n, "open", &[], path);
            if r.first().copied().unwrap_or(-1.0) < 0.0 {
                return Some(n);
            }
        }
        Some(n)
    }

    /// the notes of the slices as a clip on track `t` at `start` (beats), `len` long (0 = to the bar after the last note)
    fn slc_write_clip(&mut self, t: usize, variant: usize, seed: u64, start: f64, len: f64, name: &str) -> Option<usize> {
        let bpm = self.out.engine.lock().unwrap_or_else(|p| p.into_inner()).project.bpm;
        let flat = self.slc_cmd(t, "roll", &[bpm, variant as f64, seed as f64], "");
        if flat.len() < 4 {
            return None;
        }
        let notes: Vec<Note> = flat.chunks_exact(4).map(|c| Note::new(c[0], c[1], c[2].clamp(0.0, 127.0) as u8, c[3] as f32)).collect();
        let end = notes.iter().map(|n| n.start + n.len).fold(0.0, f64::max);
        let len = if len > 0.0 { len } else { ((end / 4.0).ceil() * 4.0).max(4.0) };
        let mut c = Clip::midi(start, len, name);
        if let ClipData::Midi(v) = &mut c.data {
            *v = notes;
        }
        let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
        let tr = en.project.tracks.get_mut(t)?;
        tr.clips.push(c);
        Some(tr.clips.len() - 1)
    }

    /// A hype shout kit on a new Slicer track, one phrase ending on the downbeat of each beat in `drops` (one per drop): the trap
    /// "yo" roll at trap / dubstep tempos (135 .. 165 bpm), the MC's "put your hands up" elsewhere (`explicit` or the clean one).
    /// -> (track, kit, the shout's span before the drop in beats); None when there is no drop or no track.
    pub(super) fn slc_hype_at(&mut self, drops: &[f64], explicit: bool) -> Option<(usize, &'static str, f64)> {
        if drops.is_empty() {
            return None;
        }
        let bpm = self.out.engine.lock().unwrap_or_else(|p| p.into_inner()).project.bpm;
        let kit = if (135.0..=165.0).contains(&bpm) { "Trap Yo Roll" } else if explicit { "Hype MC Shouts" } else { "Hype MC Clean" };
        let n = self.slc_new_track("", "")?;
        {
            let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(t) = en.project.tracks.get_mut(n) {
                t.name = format!("Hype Shout ({kit})");
                t.volume *= 0.8;
                if let Some(i) = t.inst.as_mut() {
                    i.load_preset(kit);
                }
            }
        }
        let flat = self.slc_cmd(n, "roll", &[bpm, 0.0, 0.0], "");
        let notes: Vec<Note> = flat.chunks_exact(4).map(|c| Note::new(c[0], c[1], c[2].clamp(0.0, 127.0) as u8, c[3] as f32)).collect();
        let span = notes.iter().map(|x| x.start + x.len).fold(0.0, f64::max);
        let len = ((span / 4.0).ceil() * 4.0).max(4.0);
        {
            let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            if let Some(t) = en.project.tracks.get_mut(n) {
                let mut ds: Vec<f64> = drops.to_vec();
                ds.sort_by(f64::total_cmp);
                ds.dedup_by(|x, y| (*x - *y).abs() < 1.0);
                for d in &ds {
                    let start = (d - len).max(0.0);
                    let mut c = Clip::midi(start, len, "hype");
                    if let ClipData::Midi(v) = &mut c.data {
                        // the phrase ends where the drop lands
                        *v = notes.iter().map(|x| Note { start: x.start + (len - span).max(0.0), ..x.clone() }).collect();
                    }
                    t.clips.push(c);
                }
            }
        }
        Some((n, kit, span))
    }

    pub(super) fn slc_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let t = a.u(0);
        let nums = |v: Vec<f64>| Val::List(v.into_iter().map(Val::Num).collect());
        match name {
            "daw_slc_new" => {
                let path = a.s(0);
                let stem = std::path::Path::new(&path).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let n = self.slc_new_track(&path, &stem);
                self.touch();
                Ok(Val::Num(n.map(|n| n as f64).unwrap_or(-1.0)))
            }
            "daw_slc_open" => {
                let path = a.s(1);
                let r = self.slc_cmd(t, "open", &[], &path);
                let k = r.first().copied().unwrap_or(-1.0);
                let base = std::path::Path::new(&path).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                self.status = if k < 0.0 {
                    let err = {
                        let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                        en.project.tracks.get(t).and_then(|t| t.inst.as_ref()).map(|i| i.info("error")).unwrap_or_default()
                    };
                    format!("Ling Slicer: {err}")
                } else {
                    format!("Sliced {base} into {k} slices from the root key up")
                };
                self.touch();
                Ok(Val::Num(k))
            }
            "daw_slc_slices" => Ok(nums(self.slc_cmd(t, "slices", &[], ""))),
            "daw_slc_peaks" => Ok(nums(self.slc_cmd(t, "peaks", &[a.n(1)], ""))),
            "daw_slc_bpm" => Ok(nums(self.slc_cmd(t, "bpm", &[], ""))),
            "daw_slc_set" => {
                let r = self.slc_cmd(t, "set", &[a.n(1), a.n(2), a.n(3)], "");
                self.touch();
                Ok(nums(r))
            }
            "daw_slc_set_all" => {
                let r = self.slc_cmd(t, "set_all", &[a.n(1), a.n(2)], "");
                self.touch();
                Ok(nums(r))
            }
            "daw_slc_add" | "daw_slc_move" | "daw_slc_del" | "daw_slc_reslice" => {
                let cmd = &name["daw_slc_".len()..];
                let args: Vec<f64> = (1..a.0.len()).map(|i| a.n(i)).collect();
                let r = self.slc_cmd(t, cmd, &args, "");
                self.touch();
                Ok(nums(r))
            }
            "daw_slc_refs" => {
                let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                let mut v = vec![];
                for (i, tr) in en.project.tracks.iter().enumerate() {
                    if i != t && !matches!(tr.kind, TrackKind::Audio) && !matches!(tr.inst_kind.as_str(), "drums" | "slicer") {
                        v.push(Val::Num(i as f64));
                        v.push(Val::Str(tr.name.clone()));
                    }
                }
                Ok(Val::List(v))
            }
            "daw_slc_ref" => {
                let r = if a.n(1) < 0.0 { 0.0 } else { a.n(1) + 1.0 };
                self.slc_cmd(t, "reference", &[r], "");
                self.touch();
                Ok(Val::List(self.slc_sync(t).unwrap_or_default().into_iter().map(Val::Num).collect()))
            }
            "daw_slc_get_ref" => {
                let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                let r = en.project.tracks.get(t).and_then(|t| t.inst.as_ref()).map(|i| i.info("ref").parse::<f64>().unwrap_or(0.0)).unwrap_or(0.0);
                Ok(Val::Num(r - 1.0))
            }
            "daw_slc_sync" => {
                let fresh = synced().lock().unwrap_or_else(|p| p.into_inner()).get(&t).map(|r| *r == self.rev).unwrap_or(false);
                if fresh && a.n(1) < 0.5 {
                    return Ok(Val::List(vec![]));
                }
                Ok(Val::List(self.slc_sync(t).unwrap_or_default().into_iter().map(Val::Num).collect()))
            }
            "daw_slc_timeline" => {
                let _ = self.slc_sync(t);
                let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                Ok(Val::Str(en.project.tracks.get(t).and_then(|t| t.inst.as_ref()).map(|i| i.info("timeline")).unwrap_or_default()))
            }
            "daw_slc_roll" => {
                let variant = a.u(1).min(2);
                let start = {
                    let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    (en.position / 4.0).floor().max(0.0) * 4.0
                };
                let label = ["Slices", "Slices (shuffled)", "Slices (stutter)"][variant];
                let c = self.slc_write_clip(t, variant, a.n(2).abs() as u64, start, 0.0, label);
                self.status = match c {
                    Some(_) => format!("{label}: a clip at bar {} plays slice i on key root + i - rearrange the notes to rearrange the chops", (start / 4.0) as i64 + 1),
                    None => "Ling Slicer: nothing to write (load a file first)".into(),
                };
                let _ = self.slc_sync(t);
                self.touch();
                Ok(Val::Num(c.map(|c| c as f64).unwrap_or(-1.0)))
            }
            // (explicit 0/1) -> the new track, -1 when the song has no drop: a hype shout from the sample pack on a Slicer track, ending
            // on the downbeat of every drop the generator built a noise sweep into (the "Noise Sweep" track's clips end there); the
            // trap "yo" roll at trap / dubstep tempos, the MC's "put your hands up" elsewhere
            "daw_slc_hype" => {
                let drops: Vec<f64> = {
                    let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.tracks.iter().filter(|t| t.inst_kind == "cinema" && t.name == "Noise Sweep").flat_map(|t| t.clips.iter().map(|c| c.start + c.len)).collect()
                };
                match self.slc_hype_at(&drops, a.n(0) > 0.5) {
                    Some((n, kit, _)) => {
                        self.status = format!("Hype shout: {kit} before {} drop(s)", drops.len());
                        self.touch();
                        Ok(Val::Num(n as f64))
                    }
                    None => Ok(Val::Num(-1.0)),
                }
            }
            "daw_slc_from_clip" => {
                let (ti, ci, replace) = (a.u(0), a.u(1), a.n(2) > 0.5);
                let sr = self.out.sr;
                let info = {
                    let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    let bpm = en.project.bpm.max(1.0);
                    en.project.tracks.get(ti).and_then(|t| {
                        t.clips.get(ci).map(|c| {
                            let audio = match &c.data {
                                ClipData::Audio { sample, offset_s, gain, .. } => {
                                    let rate = sample.rate.max(1) as f64;
                                    let secs = c.len * 60.0 / bpm;
                                    let a0 = ((offset_s.max(0.0) * rate) as usize * 2).min(sample.stereo.len());
                                    let b0 = (a0 + (secs * rate) as usize * 2).min(sample.stereo.len());
                                    Some((sample.stereo[a0..b0].iter().map(|x| x * gain).collect::<Vec<f32>>(), rate as u32))
                                }
                                _ => None,
                            };
                            (t.name.clone(), c.name.clone(), c.start, c.len, audio)
                        })
                    })
                };
                let Some((tname, cname, start, len, audio)) = info else { return Ok(Val::Num(-1.0)) };
                let (st, rate) = match audio {
                    Some(x) => x,
                    None => match self.wt_render_alone(ti, Some(ci), None) {
                        Ok(st) => (st, sr as u32),
                        Err(e) => {
                            self.status = format!("Slice this clip: {e}");
                            return Ok(Val::Num(-1.0));
                        }
                    },
                };
                if mono(&st).iter().all(|x| x.abs() < 1e-5) {
                    self.status = "Slice this clip: it is silent".into();
                    return Ok(Val::Num(-1.0));
                }
                let label = if cname.is_empty() { tname.clone() } else { cname.clone() };
                let safe: String = label.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).take(40).collect();
                let path = Self::wt_fresh(&self.wt_dir("audio"), &format!("{} slice", safe.trim()), "wav");
                crate::export::write_audio(&path, &st, rate, "wav", 24, 0.5)?;
                let Some(n) = self.slc_new_track(&path, &label) else { return Ok(Val::Num(-1.0)) };
                let c = self.slc_write_clip(n, 0, 0, start, len, &format!("{label} slices"));
                if replace {
                    let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    if let Some(tr) = en.project.tracks.get_mut(ti) {
                        if ci < tr.clips.len() {
                            tr.clips.remove(ci);
                        }
                    }
                }
                let k = self.slc_cmd(n, "count", &[], "").first().copied().unwrap_or(0.0);
                self.status = format!(
                    "Sliced {label} into {k} slices on a Ling Slicer track{}{}",
                    if c.is_some() { ", with a clip that replays it in place" } else { "" },
                    if replace { " (the original clip was removed)" } else { " - mute the original to hear only the slices" }
                );
                let _ = self.slc_sync(n);
                self.touch();
                Ok(Val::Num(n as f64))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
