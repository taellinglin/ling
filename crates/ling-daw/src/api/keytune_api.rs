//! Builtins for finding the key of a song and matching its tracks to it (`daw_key_*`).
//!
//! * `daw_key_detect` votes the notes of every melodic track and the chroma of every audio clip into one key.
//! * `daw_key_tracks` lists each track with its own key and how much of it already sits in the song's key.
//! * `daw_key_apply` moves the ticked tracks into the key: MIDI clips in another key are transposed by scale degree and stray notes
//!   snapped onto the scale; melodic audio (a voice, a lead) is tuned onto the scale with Bezier-eased glides between notes; audio that
//!   is a mix of chords is transposed to the song's key.
use super::theory_api::expand;
use super::{Daw, A, Val};
use crate::keydet;
use crate::model::*;
use crate::theory::{self, Key, N};
use crate::vocal;
use std::sync::Arc;

/// what is known about one audio clip's sound (cached: the analysis reads the whole recording)
#[derive(Clone)]
pub struct AudioKey {
    hist: [f64; 12],
    tuning: f64,
    level: f64,
    voiced: f64,
    steady: f64,
    poly: f64,
}

fn norm(h: &[f64; 12]) -> [f64; 12] {
    let s: f64 = h.iter().sum();
    let mut o = [0.0; 12];
    if s > 0.0 {
        for i in 0..12 {
            o[i] = h[i] / s;
        }
    }
    o
}

fn same_scale(a: Key, b: Key) -> bool {
    a == b || (a.minor != b.minor && { let (maj, min) = if a.minor { (b, a) } else { (a, b) }; (min.tonic + 3) % 12 == maj.tonic })
}

/// the smallest move (semitones, -6..=5) that takes a recording in key `from` into the scale of `to`
fn shift_between(from: Key, to: Key) -> i32 {
    let f = if from.minor == to.minor { from.tonic } else if from.minor { (from.tonic + 3) % 12 } else { (from.tonic + 9) % 12 } as i32;
    let d = (to.tonic as i32 - f).rem_euclid(12);
    if d > 5 { d - 12 } else { d }
}

fn is_drums(t: &Track) -> bool {
    t.inst_kind == "drums"
}

fn notes_of(t: &Track) -> Vec<N> {
    t.clips.iter().flat_map(|c| expand(c)).collect()
}

impl Daw {
    fn audio_key(&mut self, sample: &Arc<Sample>) -> AudioKey {
        let key = format!("{}|{}|{}", sample.path, sample.stereo.len(), sample.rate);
        if let Some(k) = self.key_cache.get(&key) {
            return k.clone();
        }
        let ch = keydet::chroma(&sample.stereo, sample.rate as f32);
        let (voiced, steady) = vocal::monophonic_score(&sample.stereo, sample.rate as f32);
        let k = AudioKey { hist: ch.hist, tuning: ch.tuning_cents, level: ch.level, voiced, steady, poly: ch.poly };
        self.key_cache.insert(key, k.clone());
        k
    }

    /// the audio clips of a track: (index, sample) of those long enough to say anything
    fn audio_clips(&self, t: &Track) -> Vec<(usize, Arc<Sample>)> {
        t.clips
            .iter()
            .enumerate()
            .filter_map(|(i, c)| match &c.data {
                ClipData::Audio { sample, .. } if sample.seconds() > 1.0 => Some((i, sample.clone())),
                _ => None,
            })
            .collect()
    }

    pub(super) fn key_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_key_get" => {
                // -> [tonic, minor, name] of the key the user set for the song, [] when none is set
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let v = en.project.meta.iter().find(|(k, _)| k == "song_key").map(|(_, v)| v.clone()).unwrap_or_default();
                match theory::parse_key(&v) {
                    Some(k) => ok!(Val::List(vec![Val::Num(k.tonic as f64), Val::Num(k.minor as i32 as f64), Val::Str(k.name())])),
                    None => ok!(Val::List(vec![])),
                }
            }
            "daw_key_set" => {
                // (tonic, minor) remember the song's key in the project (minor < 0: forget it)
                let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                en.project.meta.retain(|(k, _)| k != "song_key");
                if a.n(1) >= 0.0 {
                    en.project.meta.push(("song_key".to_string(), Key::new(a.u(0) as u8 % 12, a.n(1) > 0.5).name()));
                }
                drop(en);
                self.touch();
                Ok(Val::Unit)
            }
            "daw_key_detect" => {
                // -> [tonic, minor, correlation, margin, name, what it was heard from]
                let (notes, audio): (Vec<N>, Vec<Arc<Sample>>) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let mut notes = vec![];
                    let mut audio = vec![];
                    for t in &en.project.tracks {
                        if is_drums(t) {
                            continue;
                        }
                        if matches!(t.kind, TrackKind::Audio) || t.clips.iter().any(|c| matches!(c.data, ClipData::Audio { .. })) {
                            audio.extend(t.clips.iter().filter_map(|c| match &c.data {
                                ClipData::Audio { sample, .. } if sample.seconds() > 1.0 => Some(sample.clone()),
                                _ => None,
                            }));
                        }
                        notes.extend(notes_of(t));
                    }
                    (notes, audio)
                };
                let mut hist = [0.0; 12];
                let mut from = vec![];
                if !notes.is_empty() {
                    let h = norm(&theory::pc_hist(&notes, 0.0, 1e9));
                    for i in 0..12 {
                        hist[i] += h[i];
                    }
                    from.push(format!("{} notes", notes.len()));
                }
                let mut ah = [0.0; 12];
                let mut tw = 0.0;
                let mut longest: Vec<&Arc<Sample>> = audio.iter().collect();
                longest.sort_by(|x, y| y.seconds().total_cmp(&x.seconds()));
                let mut used = 0;
                for s in longest.into_iter().take(12) {
                    let k = self.audio_key(s);
                    for i in 0..12 {
                        ah[i] += k.hist[i] * k.level;
                    }
                    tw += k.level;
                    used += 1;
                }
                if tw > 0.0 {
                    let h = norm(&ah);
                    for i in 0..12 {
                        hist[i] += h[i] * 0.7;
                    }
                    from.push(format!("{used} audio clip{}", if used == 1 { "" } else { "s" }));
                }
                if hist.iter().sum::<f64>() <= 0.0 {
                    return ok!(Val::List(vec![]));
                }
                let (k, corr, margin) = theory::detect_key_hist(&hist);
                ok!(Val::List(vec![Val::Num(k.tonic as f64), Val::Num(k.minor as i32 as f64), Val::Num(corr), Val::Num(margin), Val::Str(k.name()), Val::Str(from.join(" and "))]))
            }
            "daw_key_tracks" => {
                // (tonic, minor) -> per track with sound: [index, kind 0 melodic / 1 audio / 2 drums, name, tonic, minor, correlation, % already in the key, audio is a solo line 0/1, note]
                let target = Key::new(a.u(0) as u8 % 12, a.n(1) > 0.5);
                let allowed = theory::allowed_pcs(target);
                let snapshot: Vec<(usize, u8, String, Vec<N>, Vec<Arc<Sample>>)> = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project
                        .tracks
                        .iter()
                        .enumerate()
                        .map(|(i, t)| {
                            let audio: Vec<Arc<Sample>> = self.audio_clips(t).into_iter().map(|x| x.1).collect();
                            let kind = if is_drums(t) { 2 } else if !audio.is_empty() { 1 } else { 0 };
                            (i, kind, t.name.clone(), notes_of(t), audio)
                        })
                        .collect()
                };
                let mut v = vec![];
                for (i, kind, name, notes, audio) in snapshot {
                    if kind == 2 && notes.is_empty() {
                        continue;
                    }
                    let (mut tonic, mut minor, mut corr, mut inpct, mut solo, mut note) = (0.0, 0.0, 0.0, 0.0, 0.0, String::new());
                    if kind == 1 {
                        let mut h = [0.0; 12];
                        let (mut tv, mut ts, mut tp) = (0.0f64, 0.0f64, 9.0f64);
                        for s in &audio {
                            let k = self.audio_key(s);
                            for q in 0..12 {
                                h[q] += k.hist[q] * k.level;
                            }
                            tv = k.voiced.max(tv);
                            ts = k.steady.max(ts);
                            tp = k.poly.min(tp);
                        }
                        let h = norm(&h);
                        if h.iter().sum::<f64>() > 0.0 {
                            let (k, c, _) = theory::detect_key_hist(&h);
                            tonic = k.tonic as f64;
                            minor = k.minor as i32 as f64;
                            corr = c;
                            inpct = (0..12).filter(|p| allowed[*p]).map(|p| h[p]).sum::<f64>() * 100.0;
                        }
                        solo = (tv >= 0.3 && ts >= 0.72 && tp <= 1.7) as i32 as f64;
                        note = if solo > 0.5 { "melodic audio: tuned onto the scale".to_string() } else { "audio mix: moved to the key".to_string() };
                    } else if !notes.is_empty() {
                        let h = norm(&theory::pc_hist(&notes, 0.0, 1e9));
                        if notes.len() >= 6 {
                            let (k, c, _) = theory::detect_key_hist(&h);
                            tonic = k.tonic as f64;
                            minor = k.minor as i32 as f64;
                            corr = c;
                        }
                        let (mut tot, mut inn) = (0.0, 0.0);
                        for n in &notes {
                            tot += n.len;
                            if allowed[(n.key % 12) as usize] {
                                inn += n.len;
                            }
                        }
                        inpct = if tot > 0.0 { inn / tot * 100.0 } else { 100.0 };
                        note = if kind == 2 { "drums: left alone".to_string() } else { format!("{} notes", notes.len()) };
                    } else {
                        continue;
                    }
                    v.extend([Val::Num(i as f64), Val::Num(kind as f64), Val::Str(name), Val::Num(tonic), Val::Num(minor), Val::Num(corr), Val::Num(inpct.round()), Val::Num(solo), Val::Str(note)]);
                }
                ok!(Val::List(v))
            }
            "daw_key_apply" => {
                // (tonic, minor, [track indices], strength 0..1, glide ms, ease y1, ease y2, transpose clips 0/1, snap notes 0/1, audio mode 0 auto / 1 move to the key / 2 tune to the scale)
                //  -> one sentence per track about what was done
                let target = Key::new(a.u(0) as u8 % 12, a.n(1) > 0.5);
                let allowed = theory::allowed_pcs(target);
                let tracks: Vec<usize> = match a.0.get(2) {
                    Some(Val::List(l)) => l.iter().filter_map(|x| if let Val::Num(n) = x { Some(*n as usize) } else { None }).collect(),
                    _ => vec![],
                };
                let (strength, glide) = (a.n(3).clamp(0.0, 1.0), a.n(4).clamp(5.0, 600.0));
                let ease = (a.n(5).clamp(0.0, 1.0), a.n(6).clamp(0.0, 1.2));
                let (transpose, snap, mode) = (a.n(7) > 0.5, a.n(8) > 0.5, a.u(9));
                let mut lines: Vec<Val> = vec![];
                for ti in tracks {
                    // MIDI part: quick, done under the lock
                    let (name, audio_clips, is_audio) = {
                        let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                        let Some(t) = en.project.tracks.get_mut(ti) else { continue };
                        if is_drums(t) {
                            continue;
                        }
                        let name = t.name.clone();
                        let (mut moved, mut snapped, mut clips_t) = (0usize, 0usize, 0usize);
                        for c in t.clips.iter_mut() {
                            let ClipData::Midi(ns) = &mut c.data else { continue };
                            if ns.is_empty() {
                                continue;
                            }
                            let mut nn: Vec<N> = ns.iter().map(|n| N { start: n.start, len: n.len, key: n.key, vel: n.vel }).collect();
                            if transpose && nn.len() >= 8 {
                                let (ck, corr, _) = theory::detect_key(&nn, 0.0, 1e9);
                                if corr > 0.5 && !same_scale(ck, target) {
                                    let m = theory::retune(&nn, ck, target);
                                    for (x, y) in ns.iter_mut().zip(&m) {
                                        if x.key != y.key {
                                            x.key = y.key;
                                            moved += 1;
                                        }
                                    }
                                    nn = m;
                                    clips_t += 1;
                                }
                            }
                            if snap {
                                for (x, n) in ns.iter_mut().zip(&nn) {
                                    if !allowed[(n.key % 12) as usize] {
                                        let k = vocal::nearest_allowed(n.key as f64, &allowed, None).clamp(0, 127) as u8;
                                        if k != x.key {
                                            x.key = k;
                                            snapped += 1;
                                        }
                                    }
                                }
                            }
                        }
                        if matches!(t.kind, TrackKind::Audio) || t.clips.iter().any(|c| matches!(c.data, ClipData::Audio { .. })) {
                            let clips: Vec<(usize, Arc<Sample>)> = self.audio_clips(t);
                            (name, clips, true)
                        } else {
                            let mut parts = vec![];
                            if clips_t > 0 {
                                parts.push(format!("{clips_t} clip{} moved into {} ({moved} notes)", if clips_t == 1 { "" } else { "s" }, target.name()));
                            }
                            if snapped > 0 {
                                parts.push(format!("{snapped} stray note{} snapped onto the scale", if snapped == 1 { "" } else { "s" }));
                            }
                            lines.push(Val::Str(format!("{name}: {}", if parts.is_empty() { "already in the key".to_string() } else { parts.join(", ") })));
                            (name, vec![], false)
                        }
                    };
                    if !is_audio {
                        continue;
                    }
                    // audio part: analysis and re-synthesis happen outside the lock
                    let mut parts: Vec<String> = vec![];
                    for (ci, sample) in audio_clips {
                        let info = self.audio_key(&sample);
                        let (offset_s, gain, pitch, warp) = {
                            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                            match en.project.tracks.get(ti).and_then(|t| t.clips.get(ci)).map(|c| &c.data) {
                                Some(ClipData::Audio { offset_s, gain, pitch, warp, .. }) => (*offset_s, *gain, *pitch, *warp),
                                _ => continue,
                            }
                        };
                        let solo = info.voiced >= 0.3 && info.steady >= 0.72 && info.poly <= 1.7;
                        let tune = mode == 2 || (mode == 0 && solo);
                        if tune {
                            // the clip may already be transposed: the scale as the source recording hears it
                            let semis = pitch.round() as i32;
                            let mut src_allowed = [false; 12];
                            for pc in 0..12 {
                                src_allowed[pc] = allowed[(pc as i32 + semis).rem_euclid(12) as usize];
                            }
                            let (out, rep) = vocal::tune_to_scale(&sample.stereo, sample.rate as f32, &src_allowed, strength, glide, ease);
                            if rep.notes == 0 {
                                parts.push("no clear melody to tune".to_string());
                                continue;
                            }
                            let new = self.write_tuned(&sample, out)?;
                            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                            if let Some(clip) = en.project.tracks.get_mut(ti).and_then(|t| t.clips.get_mut(ci)) {
                                clip.data = ClipData::Audio { sample: new, offset_s, gain, pitch, warp };
                            }
                            parts.push(format!("{} notes tuned onto the scale ({:.0} cents off on average, now {:.0})", rep.notes, rep.cents_before, rep.cents_after));
                        } else {
                            let h = norm(&info.hist);
                            if h.iter().sum::<f64>() <= 0.0 {
                                parts.push("silent".to_string());
                                continue;
                            }
                            let (k, corr, _) = theory::detect_key_hist(&h);
                            if corr < 0.35 {
                                parts.push("no clear key".to_string());
                                continue;
                            }
                            // where it plays now: its own key moved by the clip's pitch
                            let now = Key::new(((k.tonic as i32 + pitch.round() as i32).rem_euclid(12)) as u8, k.minor);
                            if same_scale(now, target) {
                                parts.push(format!("already in {}", now.name()));
                                continue;
                            }
                            let s = shift_between(now, target);
                            let new_pitch = pitch + s as f32 - if info.tuning.abs() > 12.0 { (info.tuning / 100.0) as f32 } else { 0.0 };
                            let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                            if let Some(Clip { data: ClipData::Audio { pitch, .. }, .. }) = en.project.tracks.get_mut(ti).and_then(|t| t.clips.get_mut(ci)) {
                                *pitch = new_pitch;
                            }
                            parts.push(format!("{} -> {}: moved {} semitone{}", now.name(), target.name(), s.abs(), if s.abs() == 1 { "" } else { "s" }) + if s < 0 { " down" } else { " up" });
                        }
                    }
                    lines.push(Val::Str(format!("{name}: {}", if parts.is_empty() { "nothing to do".to_string() } else { parts.join("; ") })));
                }
                self.touch();
                ok!(Val::List(lines))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }

    /// write tuned audio next to the project (or in the cache) and hand back the new sample
    fn write_tuned(&mut self, sample: &Arc<Sample>, out: Vec<f32>) -> Result<Arc<Sample>, String> {
        let dir = if self.path.is_empty() { std::path::PathBuf::from("cache/key") } else { std::path::Path::new(&self.path).parent().map(|p| p.join("audio")).unwrap_or_else(|| "cache/key".into()) };
        std::fs::create_dir_all(&dir).map_err(|x| x.to_string())?;
        let base = sample.name.trim_end_matches(" (in key)").trim_end_matches(" (tuned)").trim().to_string();
        let mut file = dir.join(format!("{base} in key.wav"));
        let mut k = 2;
        while file.exists() {
            file = dir.join(format!("{base} in key {k}.wav"));
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
        let new = Arc::new(Sample::new(format!("{base} (in key)"), path.clone(), sample.rate, out));
        self.samples.insert(path, new.clone());
        Ok(new)
    }
}
