//! Performance (clip-launcher) helpers: build scenes from the arrangement.
use crate::model::*;

/// Slice `[a, b)` (arrangement beats) out of a track's clips into one launcher clip.
fn slice_track(t: &Track, a: f64, b: f64, bpm: f64) -> Option<Clip> {
    let len = b - a;
    let mut notes: Vec<Note> = vec![];
    let mut audio: Option<(std::sync::Arc<Sample>, f64, f32, f32, f32)> = None;
    for c in &t.clips {
        if c.end() <= a || c.start >= b {
            continue;
        }
        match &c.data {
            ClipData::Midi(n) => {
                for x in n {
                    let s = c.start + x.start;
                    let e = s + x.len;
                    if s < b && e > a && x.start < c.len {
                        let ns = (s - a).max(0.0);
                        let ne = (e.min(c.end()).min(b) - a).max(ns + 0.03);
                        let mut y = x.clone();
                        y.start = ns;
                        y.len = ne - ns;
                        notes.push(y);
                    }
                }
            }
            ClipData::Audio { sample, offset_s, gain, pitch, warp } => {
                // a launcher clip always starts at its first frame: only take audio that is already playing at `a`
                if audio.is_none() && c.start <= a + 0.01 {
                    let lead = (a - c.start).max(0.0);
                    let secs_per_beat = if *warp > 1.0 { 60.0 / *warp as f64 } else { 60.0 / bpm.max(1.0) };
                    audio = Some((sample.clone(), *offset_s + lead * secs_per_beat, *gain, *pitch, *warp));
                }
            }
        }
    }
    if let Some((sample, off, gain, pitch, warp)) = audio {
        return Some(Clip { start: 0.0, len, name: sample.name.clone(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample, offset_s: off, gain, pitch, warp } });
    }
    if notes.is_empty() {
        return None;
    }
    Some(Clip { start: 0.0, len, name: "Slice".into(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Midi(notes) })
}

/// mode 0 = smart (split where the arrangement changes), 1 = per bar, 2 = per phrase (4 bars).
pub fn auto_scenes(p: &mut Project, mode: u8) {
    let end = p.end_beat().ceil().min(4096.0);
    if end <= 0.0 {
        return;
    }
    let mut cuts: Vec<f64> = vec![0.0];
    match mode {
        1 => {
            let mut b = 4.0;
            while b < end {
                cuts.push(b);
                b += 4.0;
            }
        }
        2 => {
            let mut b = 16.0;
            while b < end {
                cuts.push(b);
                b += 16.0;
            }
        }
        3 => {
            // where the section tags (intro, verse, chorus ...) begin; one-bar fills do not cut a scene
            let text = p.meta.iter().find(|(k, _)| k == "sections").map(|(_, v)| v.clone()).unwrap_or_default();
            let mut starts: Vec<f64> = text
                .split(';')
                .filter_map(|part| {
                    let f: Vec<&str> = part.split(':').collect();
                    if f.len() == 3 && f[2] != "fill" {
                        f[0].parse::<f64>().ok()
                    } else {
                        None
                    }
                })
                .collect();
            starts.sort_by(|a, b| a.total_cmp(b));
            for x in starts {
                let x = (x / 4.0).round() * 4.0;
                if x > *cuts.last().unwrap() + 7.9 && x < end - 3.9 {
                    cuts.push(x);
                }
            }
        }
        _ => {
            let mut pts: Vec<f64> = vec![];
            for t in &p.tracks {
                for c in &t.clips {
                    pts.push(c.start);
                    pts.push(c.end());
                }
            }
            pts.retain(|x| x.is_finite());
            pts.sort_by(|a, b| a.total_cmp(b));
            for x in pts {
                let x = (x / 4.0).round() * 4.0; // snap to bars
                if x > *cuts.last().unwrap() + 3.9 && x < end - 3.9 {
                    cuts.push(x);
                }
            }
        }
    }
    cuts.push(end);
    for t in p.tracks.iter_mut() {
        t.session.clear();
    }
    p.slots.clear();
    p.scenes.clear();
    for (si, w) in cuts.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        p.scenes.push(format!("Scene {}", si + 1));
        for ti in 0..p.tracks.len() {
            if let Some(c) = slice_track(&p.tracks[ti], a, b, p.bpm) {
                p.tracks[ti].session.push(c);
                let idx = (p.tracks[ti].session.len() - 1) as i32;
                p.set_slot(si, ti, idx);
            }
        }
    }
}
