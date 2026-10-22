//! Builtins for the Tracker view (`daw_trk_*`): the arrangement as a grid of rows and channels, like OpenMPT.
//!
//! Every instrument track becomes as many channels as it has notes sounding at once (up to 8): notes are dealt to the lowest free channel,
//! so a chord reads as a row of notes across its channels. Audio tracks get one channel whose cells mark where clips start. A row is
//! `1 / rows-per-beat` beats; a cell holds the note starting in it, or a hold mark while an earlier note still sounds, or an off mark where
//! a note just ended. The layout is rebuilt only when the project changes.
use super::{Daw, A, Val};
use crate::model::*;
use std::sync::Arc;

const MAX_VOICES: usize = 8;
const AUDIO_KEY: i64 = 128;

#[derive(Clone)]
pub struct Ev {
    pub(super) s: f64,
    pub(super) e: f64,
    key: i64,
    vel: f32,
    clip: usize,
    note: usize,
}

pub struct Chan {
    pub(super) track: usize,
    pub(super) voice: usize,
    pub(super) evs: Vec<Ev>,
}

pub struct Layout {
    pub(super) chans: Vec<Chan>,
    pub(super) end: f64,
}

fn track_events(t: &Track) -> Vec<Ev> {
    let mut out: Vec<Ev> = vec![];
    for (ci, c) in t.clips.iter().enumerate() {
        match &c.data {
            ClipData::Midi(notes) => {
                let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
                let reps = if ll > 0.0 { ((c.len / ll).ceil() as usize).min(4096) } else { 1 };
                for k in 0..reps {
                    let base = c.start + k as f64 * ll;
                    let limit = if ll > 0.0 { ll.min(c.len - k as f64 * ll) } else { c.len };
                    for (ni, n) in notes.iter().enumerate() {
                        if n.start >= limit {
                            continue;
                        }
                        let s = base + n.start;
                        out.push(Ev { s, e: s + n.len.min(limit - n.start).max(0.02), key: n.key as i64, vel: n.vel, clip: ci, note: ni });
                    }
                }
            }
            ClipData::Audio { .. } => out.push(Ev { s: c.start, e: c.start + c.len, key: AUDIO_KEY, vel: 1.0, clip: ci, note: usize::MAX }),
        }
    }
    out.sort_by(|a, b| a.s.total_cmp(&b.s).then(a.key.cmp(&b.key)));
    out
}

fn build(p: &Project) -> Layout {
    let mut chans = vec![];
    let mut end = 0f64;
    for (ti, t) in p.tracks.iter().enumerate() {
        let evs = track_events(t);
        end = end.max(evs.iter().map(|e| e.e).fold(0.0, f64::max));
        let mut lanes: Vec<Vec<Ev>> = vec![vec![]];
        let mut last_end: Vec<f64> = vec![f64::MIN];
        if matches!(t.kind, TrackKind::Audio) {
            lanes[0] = evs;
        } else {
            for ev in evs {
                let free = last_end.iter().position(|le| *le <= ev.s + 1e-6);
                let v = match free {
                    Some(v) => v,
                    None if lanes.len() < MAX_VOICES => {
                        lanes.push(vec![]);
                        last_end.push(f64::MIN);
                        lanes.len() - 1
                    }
                    None => (0..lanes.len()).min_by(|a, b| last_end[*a].total_cmp(&last_end[*b])).unwrap_or(0),
                };
                last_end[v] = ev.e;
                lanes[v].push(ev);
            }
        }
        for (v, evs) in lanes.into_iter().enumerate() {
            chans.push(Chan { track: ti, voice: v, evs });
        }
    }
    Layout { chans, end }
}

/// cells of a block of rows: (key, velocity 0..1, hold) row by row, for the given channels (indices into the layout).
/// key -1 none, 128 = audio clip start; hold 0 none, 1 an earlier note still sounds, 2 an earlier note ended in this row
pub(super) fn page_cells(l: &Layout, beat0: f64, rows: usize, rpb: f64, chans: &[usize]) -> Vec<(i64, f32, u8)> {
    let cn = chans.len();
    let rl = 1.0 / rpb;
    let mut cells = vec![(-1i64, 0f32, 0u8); rows * cn];
    for (ci, chi) in chans.iter().enumerate() {
        let Some(ch) = l.chans.get(*chi) else { continue };
        let i0 = ch.evs.partition_point(|e| e.e <= beat0 - 1e-9);
        for ev in &ch.evs[i0..] {
            if ev.s >= beat0 + rows as f64 * rl {
                break;
            }
            let on_row = ((ev.s - beat0) / rl + 1e-6).floor();
            let off_row = ((ev.e - beat0) / rl - 1e-6).ceil();
            if on_row >= 0.0 && (on_row as usize) < rows {
                let c = &mut cells[on_row as usize * cn + ci];
                c.0 = ev.key;
                c.1 = ev.vel;
            }
            let from = (on_row + 1.0).max(0.0) as usize;
            let to = (off_row.max(0.0) as usize).min(rows);
            for r in from..to {
                let c = &mut cells[r * cn + ci];
                if c.0 < 0 && c.2 == 0 {
                    c.2 = 1;
                }
            }
            if off_row >= 0.0 && (off_row as usize) < rows && ev.key != AUDIO_KEY {
                let c = &mut cells[off_row as usize * cn + ci];
                if c.0 < 0 {
                    c.2 = 2;
                }
            }
        }
    }
    cells
}

impl Daw {
    fn trk_layout(&mut self) -> Arc<Layout> {
        if let Some((rev, l)) = &self.trk_cache {
            if *rev == self.rev {
                return l.clone();
            }
        }
        let l = {
            let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
            Arc::new(build(&en.project))
        };
        self.trk_cache = Some((self.rev, l.clone()));
        l
    }

    pub(super) fn trk_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        let l = self.trk_layout();
        match name {
            "daw_trk_channels" => {
                // -> [track, voice]* for every channel, in column order
                ok!(Val::List(l.chans.iter().flat_map(|c| [Val::Num(c.track as f64), Val::Num(c.voice as f64)]).collect()))
            }
            "daw_trk_length" => ok!(l.end),
            "daw_trk_page" => {
                // (first beat, rows, rows per beat, first channel, channel count) -> [key, velocity 0..127, hold]* row by row.
                // key -1 none, 128 = audio clip start; hold 0 none · 1 an earlier note still sounds · 2 an earlier note ended in this row
                let (beat0, rows, rpb) = (a.n(0), a.u(1).min(256), a.n(2).clamp(1.0, 16.0));
                let (c0, cn) = (a.u(3), a.u(4).min(64));
                let idx: Vec<usize> = (0..cn).map(|ci| c0 + ci).collect();
                let cells = page_cells(&l, beat0, rows, rpb, &idx);
                ok!(Val::List(cells.into_iter().flat_map(|(k, v, h)| [Val::Num(k as f64), Val::Num((v * 127.0).round().clamp(0.0, 127.0) as f64), Val::Num(h as f64)]).collect()))
            }
            "daw_trk_pdf" => {
                // (folder, base name, options "k=v;...", fonts folder, "pdf" to also print the PDF) -> files written ([] when nothing to print)
                let (dir, base, opts, fdir, mode) = (a.s(0), a.s(1), a.s(2), a.s(3), a.s(4));
                let o = super::tracker_pdf::TOpts::parse(&opts);
                let (pages, name) = {
                    let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                    (super::tracker_pdf::render_pages(&en.project, &l, &o), en.project.name.clone())
                };
                if pages.is_empty() {
                    self.status = "Export Tracker: nothing to print (no notes in that range)".into();
                    ok!(Val::List(vec![]))
                }
                let dirp = std::path::Path::new(&dir);
                std::fs::create_dir_all(dirp).map_err(|e| format!("{dir}: {e}"))?;
                let css = super::sheet_api::fonts_css(&fdir);
                let title = if o.title.is_empty() { name } else { o.title.clone() };
                let html_name = format!("{base}.html");
                std::fs::write(dirp.join(&html_name), crate::sheet::html(&title, &pages, o.landscape, &css)).map_err(|e| format!("{html_name}: {e}"))?;
                let mut out = vec![Val::Str(html_name.clone())];
                if mode.contains("pdf") {
                    let pdf = format!("{base}.pdf");
                    match super::sheet_api::html_to_pdf(&dirp.join(&html_name), &dirp.join(&pdf)) {
                        Ok(()) => out.push(Val::Str(pdf)),
                        Err(e) => {
                            self.status = format!("Tracker pages written, but the PDF failed: {e}");
                            ok!(Val::List(out))
                        }
                    }
                }
                self.status = format!("Export Tracker: {} page(s) in {dir}", pages.len());
                ok!(Val::List(out))
            }
            "daw_trk_ring" => {
                // (track, first beat, beats, slots) -> loudest note start (0..1) in each slot, all channels of the track
                let (t, beat0, beats, n) = (a.u(0), a.n(1), a.n(2).max(0.25), a.u(3).clamp(1, 128));
                let mut out = vec![0f64; n];
                for ch in l.chans.iter().filter(|c| c.track == t) {
                    let i0 = ch.evs.partition_point(|e| e.e <= beat0 - 1e-9);
                    for ev in &ch.evs[i0..] {
                        if ev.s >= beat0 + beats {
                            break;
                        }
                        if ev.s >= beat0 - 1e-9 {
                            let k = (((ev.s - beat0) / beats) * n as f64) as usize;
                            if k < n {
                                out[k] = out[k].max(ev.vel as f64);
                            }
                        }
                    }
                }
                ok!(Val::List(out.into_iter().map(Val::Num).collect()))
            }
            "daw_trk_set" => {
                // (channel, beat, row length, key (-1 = clear), velocity 0..127, length in beats (0 = one row)) -> 1 when the cell changed
                let (chi, beat, rl, key) = (a.u(0), a.n(1), a.n(2).max(0.01), a.n(3) as i64);
                let vel = (a.n(4) / 127.0).clamp(0.02, 1.0) as f32;
                let len = if a.n(5) > 0.0 { a.n(5) } else { rl };
                let Some(ch) = l.chans.get(chi) else { return ok!(false) };
                let mut en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                let Some(t) = en.project.tracks.get_mut(ch.track) else { return ok!(false) };
                if !matches!(t.kind, TrackKind::Instrument) {
                    return ok!(false);
                }
                // notes of this channel that start in the row go
                let mut kill: Vec<(usize, usize)> = ch.evs.iter().filter(|e| e.s >= beat - 1e-6 && e.s < beat + rl - 1e-6 && e.note != usize::MAX).map(|e| (e.clip, e.note)).collect();
                kill.sort_unstable();
                kill.dedup();
                let mut changed = !kill.is_empty();
                for (ci, ni) in kill.into_iter().rev() {
                    if let Some(Clip { data: ClipData::Midi(v), .. }) = t.clips.get_mut(ci) {
                        if ni < v.len() {
                            v.remove(ni);
                        }
                    }
                }
                if (0..128).contains(&key) {
                    let ci = match t.clips.iter().position(|c| matches!(c.data, ClipData::Midi(_)) && c.start <= beat + 1e-9 && beat < c.start + c.len - 1e-9) {
                        Some(i) => i,
                        None => {
                            let start = (beat / 4.0).floor() * 4.0;
                            t.clips.push(Clip { start, len: 4.0, name: "Pattern".into(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Midi(vec![]) });
                            t.clips.len() - 1
                        }
                    };
                    let c = &mut t.clips[ci];
                    let mut local = beat - c.start;
                    if c.loop_len >= 0.25 && c.loop_len < c.len {
                        local %= c.loop_len;
                    }
                    if let ClipData::Midi(v) = &mut c.data {
                        v.push(Note::new(local, len, key as u8, vel));
                    }
                    changed = true;
                }
                drop(en);
                if changed {
                    self.touch();
                }
                ok!(changed)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
