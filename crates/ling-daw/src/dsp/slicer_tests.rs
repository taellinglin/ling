use super::*;
use crate::model::{Clip, ClipData, Track, TrackKind};

const SR: f32 = 48000.0;

fn tmp(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!("ling_slicer_test_{}", std::process::id()));
    let _ = std::fs::create_dir_all(&d);
    d.join(name)
}

/// a WAV of interleaved stereo (or mono when `channels` is 1) at `rate`
fn write_wav(path: &std::path::Path, stereo: &[f32], rate: u32, channels: u16) {
    let spec = hound::WavSpec { channels, sample_rate: rate, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for f in stereo.chunks_exact(2) {
        if channels == 1 {
            w.write_sample(((f[0] + f[1]) * 0.5 * 32767.0) as i16).unwrap();
        } else {
            w.write_sample((f[0] * 32767.0) as i16).unwrap();
            w.write_sample((f[1] * 32767.0) as i16).unwrap();
        }
    }
    w.finalize().unwrap();
}

/// clicks (a 2 ms decaying burst) at `times` seconds over a faint noise floor
fn click_train(times: &[f32], secs: f32, rate: u32) -> Vec<f32> {
    let n = (secs * rate as f32) as usize;
    let mut r = crate::dsp::Rng::new(77);
    let mut out: Vec<f32> = (0..n * 2).map(|_| r.next() * 0.002).collect();
    for t in times {
        let s = (t * rate as f32) as usize;
        for i in 0..(rate as usize / 50) {
            if s + i >= n {
                break;
            }
            let x = (i as f32 * 0.9).sin() * (-(i as f32) / (rate as f32 * 0.003)).exp() * 0.8;
            out[(s + i) * 2] += x;
            out[(s + i) * 2 + 1] += x;
        }
    }
    out
}

/// render `notes` (seconds, key, vel) sample-accurately through an instrument, `secs` long
fn render_notes(inst: &mut dyn Instrument, notes: &[(f32, u8, f32, f32)], secs: f32) -> Vec<f32> {
    let n = (secs * SR) as usize;
    let mut out = vec![0.0f32; n * 2];
    let mut ev: Vec<(usize, u8, f32, bool)> = vec![];
    for (t, k, v, len) in notes {
        ev.push(((t * SR) as usize, *k, *v, true));
        ev.push((((t + len) * SR) as usize, *k, 0.0, false));
    }
    ev.sort_by_key(|e| (e.0, e.3));
    let mut pos = 0usize;
    for (at, k, v, on) in ev.into_iter().chain(std::iter::once((n, 0, 0.0, false))) {
        let at = at.min(n);
        if at > pos {
            inst.render(&mut out[pos * 2..at * 2]);
            pos = at;
        }
        if at >= n {
            break;
        }
        if on {
            inst.note_on(k, v);
        } else {
            inst.note_off(k);
        }
    }
    out
}

fn assert_near(found: &[u32], want: &[f32], rate: u32, tol_s: f32) {
    assert_eq!(found.len(), want.len(), "found {:?} want {:?}", found.iter().map(|f| *f as f32 / rate as f32).collect::<Vec<_>>(), want);
    for (f, w) in found.iter().zip(want) {
        let d = (*f as f32 / rate as f32 - w).abs();
        assert!(d < tol_s, "onset at {:.4}s, want {w:.4}s ({:.1} ms off)", *f as f32 / rate as f32, d * 1000.0);
    }
}

#[test]
fn slicing_finds_the_clicks_of_a_click_train_within_5_ms() {
    let times = [0.1f32, 0.37, 0.52, 0.8, 1.13, 1.2, 1.61, 1.9];
    for rate in [44100u32, 48000] {
        let x = click_train(&times, 2.2, rate);
        for attack in [true, false] {
            let o = onsets(&x, rate, 0.5, 0.04, attack);
            assert_near(&o, &times, rate, 0.005);
        }
    }
}

#[test]
fn slicing_finds_the_hits_of_a_rendered_drum_loop_within_5_ms() {
    // a bar of eighth notes at 120 bpm on Ling Membrane: kick, hat, snare, hat ...
    let mut d = crate::dsp::make_instrument("drums", SR).unwrap();
    let pat: [u8; 8] = [36, 42, 38, 42, 36, 36, 38, 42];
    let notes: Vec<(f32, u8, f32, f32)> = pat.iter().enumerate().map(|(i, k)| (0.05 + i as f32 * 0.25, *k, 0.9, 0.1)).collect();
    let x = render_notes(d.as_mut(), &notes, 2.3);
    let want: Vec<f32> = notes.iter().map(|n| n.0).collect();
    let o = onsets(&x, SR as u32, 0.5, 0.04, true);
    assert_near(&o, &want, SR as u32, 0.005);
}

#[test]
fn modes_cut_the_file_and_markers_edit_it() {
    let times = [0.0f32, 0.5, 1.0, 1.5];
    let x = click_train(&times, 2.0, 48000);
    let p = tmp("modes_120bpm.wav");
    write_wav(&p, &x, 48000, 2);
    let mut s = Slicer::new(SR);
    assert_eq!(s.command("open", &[], &p.to_string_lossy())[0], 4.0);
    assert!((s.src_bpm() - 120.0).abs() < 1e-6, "the tempo from the file name");
    // beats: 1/8 at 120 bpm over 2 s = 8 slices
    s.set_param("mode", 0.25);
    s.set_param("grid", 0.5);
    assert_eq!(s.slices.len(), 8);
    s.set_param("mode", 0.75);
    s.set_param("count", 4.0 / 62.0);
    assert_eq!(s.slices.len(), 6, "equal: 6 slices");
    // manual: add, move, delete
    let k = s.command("add", &[30000.0], "")[0] as usize;
    assert_eq!(s.slices.len(), 7);
    assert!((s.slices[k].start as i64 - 30000).abs() < 80);
    s.command("move", &[k as f64, 31000.0], "");
    assert!((s.slices[k].start as i64 - 31000).abs() < 2);
    s.command("del", &[k as f64], "");
    assert_eq!(s.slices.len(), 6);
    assert_eq!(s.mode(), 4, "editing markers switches to manual");
    // every slice runs to the next
    for w in s.slices.windows(2) {
        assert_eq!(w[0].end, w[1].start);
    }
}

#[test]
fn to_piano_roll_replays_the_original_timing() {
    let times = [0.05f32, 0.31, 0.55, 0.9, 1.4];
    let x = click_train(&times, 2.0, 48000);
    let p = tmp("roll.wav");
    write_wav(&p, &x, 48000, 2);
    let mut s = Slicer::new(SR);
    s.command("open", &[], &p.to_string_lossy());
    let bpm = 97.0;
    let notes = s.roll(bpm, 0, 0);
    assert_eq!(notes.len(), times.len());
    for (i, n) in notes.iter().enumerate() {
        assert_eq!(n.key, s.root() + i as u8, "slice i on key root + i");
        let secs = n.start * 60.0 / bpm;
        assert!((secs - s.slices[i].start as f64 / 48000.0).abs() < 1e-6);
        assert!((secs - times[i] as f64).abs() < 0.005);
    }
    // played back through the slicer, the hits land where they were
    let ev: Vec<(f32, u8, f32, f32)> = notes.iter().map(|n| ((n.start * 60.0 / bpm) as f32, n.key, 1.0, (n.len * 60.0 / bpm) as f32)).collect();
    let y = render_notes(&mut s, &ev, 2.0);
    let o = onsets(&y, SR as u32, 0.5, 0.04, true);
    assert_near(&o, &times, SR as u32, 0.005);
    // the variations keep the rhythm (shuffle) or add notes (stutter)
    let sh = s.roll(bpm, 1, 3);
    assert_eq!(sh.iter().map(|n| n.start).collect::<Vec<_>>(), notes.iter().map(|n| n.start).collect::<Vec<_>>());
    let mut ks: Vec<u8> = sh.iter().map(|n| n.key).collect();
    ks.sort_unstable();
    assert_eq!(ks, notes.iter().map(|n| n.key).collect::<Vec<_>>(), "the same slices, dealt again");
    assert!(s.roll(bpm, 2, 5).len() >= notes.len());
}

/// a voice-like tone at `midi` (a few harmonics), `secs` long
fn tone(midi: f32, secs: f32, rate: u32) -> Vec<f32> {
    let hz = 440.0 * 2f32.powf((midi - 69.0) / 12.0);
    let n = (secs * rate as f32) as usize;
    (0..n)
        .flat_map(|i| {
            let t = i as f32 / rate as f32;
            let env = (t / 0.005).min(1.0).min((n - i) as f32 / rate as f32 / 0.01);
            let ph = t * hz * std::f32::consts::TAU;
            let x = (ph.sin() * 0.5 + (2.0 * ph).sin() * 0.25 + (3.0 * ph).sin() * 0.12) * env * 0.6;
            [x, x]
        })
        .collect()
}

/// a tone slice (D4 + 60 cents) then a noise burst, as a file loaded into a Slicer
fn tune_slicer() -> Slicer {
    let mut x = tone(62.6, 0.6, 48000);
    x.extend(vec![0.0; 4800 * 2]);
    let mut r = crate::dsp::Rng::new(5);
    x.extend((0..48000 / 2).flat_map(|i| {
        let v = r.next() * 0.5 * (1.0 - i as f32 / 24000.0);
        [v, v]
    }));
    let p = tmp("tune.wav");
    write_wav(&p, &x, 48000, 2);
    let mut s = Slicer::new(SR);
    s.command("open", &[], &p.to_string_lossy());
    assert_eq!(s.slices.len(), 2, "{:?}", s.slices.iter().map(|s| s.start).collect::<Vec<_>>());
    s
}

fn chord(start: f64, len: f64, keys: &[u8]) -> Vec<Note> {
    keys.iter().map(|k| Note::new(start, len, *k, 0.8)).collect()
}

#[test]
fn auto_tune_moves_a_pitched_slice_to_the_chord_at_its_beat_and_follows_a_change() {
    let mut s = tune_slicer();
    let f0 = s.slices[0].f0.expect("the tone is pitched").midi;
    assert!((f0 - 62.6).abs() < 0.1, "{f0}");
    assert!(s.slices[1].f0.is_none(), "the noise has no pitch");
    // C major for a bar, then G major
    let mut h = chord(0.0, 4.0, &[48, 60, 64, 67]);
    h.extend(chord(4.0, 4.0, &[43, 59, 62, 67]));
    s.timeline = timeline(&h, 4.0);
    s.set_param("tune", 1.0);
    let root = s.root();
    let at = |s: &Slicer, i: usize, b: f64| f0 + s.shift_at(i, Some(b));
    assert!((at(&s, 0, 1.0) - 64.0).abs() < 0.12, "over C major: E ({})", at(&s, 0, 1.0));
    assert!((at(&s, 0, 5.0) - 62.0).abs() < 0.12, "over G major: D ({})", at(&s, 0, 5.0));
    // the noise stays as it is
    assert_eq!(s.shift_at(1, Some(1.0)), 0.0);
    assert_eq!(s.shift_at(1, Some(5.0)), 0.0);
    // half the amount: half way
    s.set_param("amount", 0.5);
    assert!((at(&s, 0, 1.0) - 63.3).abs() < 0.12);
    s.set_param("amount", 1.0);
    // off: as recorded
    s.set_param("tune", 0.0);
    assert_eq!(s.shift_at(0, Some(1.0)), 0.0);
    // played: the note at beat 5 sounds a D (the shifted slice rendered ahead with its length kept)
    s.set_param("tune", 1.0);
    s.set_tempo(120.0);
    s.command("sync", &[120.0, 1.0, root as f64, 5.0, root as f64], "");
    assert_eq!(s.plan.len(), 2);
    assert!(!s.cache.is_empty(), "rendered ahead");
    for (beat, want) in [(1.0, 64.0f32), (5.0, 62.0)] {
        s.all_off();
        s.note_beat(beat);
        let y = render_notes(&mut s, &[(0.0, root, 1.0, 0.5)], 0.6);
        let m = mono_of(&y);
        let p = chops::chop_f0(&m, 2400, 24000, 48000).expect("pitched").midi;
        assert!((p - want).abs() < 0.15, "beat {beat}: {p}, want {want}");
        let len = m.iter().rposition(|x| x.abs() > 0.01).unwrap_or(0) as f32 / 48000.0;
        assert!((len - 0.6).abs() < 0.05, "the length is kept: {len}");
    }
}

#[test]
fn auto_tune_follows_a_key_change() {
    let mut s = tune_slicer();
    // C major for two bars (a scale run), then F# major
    let mut h = vec![];
    for (i, k) in [60u8, 62, 64, 65, 67, 69, 71, 72].iter().enumerate() {
        h.push(Note::new(i as f64, 1.0, *k, 0.8));
    }
    for (i, k) in [66u8, 68, 70, 71, 73, 75, 77, 78].iter().enumerate() {
        h.push(Note::new(8.0 + i as f64, 1.0, *k, 0.8));
    }
    s.timeline = timeline(&h, 4.0);
    s.set_param("tune", 0.5);
    assert_eq!(s.tune_mode(), 1);
    let f0 = s.slices[0].f0.unwrap().midi;
    // D + 60 cents: D is in C major, in F# major the nearest tone is C# (61) or D# (63): D# is nearer (0.4)
    assert!((f0 + s.shift_at(0, Some(2.0)) - 62.0).abs() < 0.12, "{}", f0 + s.shift_at(0, Some(2.0)));
    let k2 = seg_at(&s.timeline, 12.0).and_then(|x| x.key).unwrap();
    assert_eq!(k2, Key::new(6, false), "{:?}", s.timeline);
    assert!((f0 + s.shift_at(0, Some(12.0)) - 63.0).abs() < 0.12, "{}", f0 + s.shift_at(0, Some(12.0)));
    assert_eq!(s.shift_at(1, Some(12.0)), 0.0);
}

#[test]
fn the_engine_tells_the_slicer_the_beat_so_tuning_follows_the_song() {
    let s = tune_slicer();
    let root = s.root();
    let mut eng = crate::engine::Engine::new(SR);
    eng.project.bpm = 120.0;
    let mut t = Track::new("slicer", TrackKind::Instrument, 0, Some(Box::new(s)));
    t.inst_kind = "slicer".into();
    let mut c = Clip::midi(0.0, 8.0, "s");
    if let ClipData::Midi(n) = &mut c.data {
        n.push(Note::new(0.0, 1.0, root, 1.0));
        n.push(Note::new(4.0, 1.0, root, 1.0));
    }
    t.clips.push(c);
    eng.project.tracks.push(t);
    let mut h = chord(0.0, 4.0, &[48, 60, 64, 67]);
    h.extend(chord(4.0, 4.0, &[43, 59, 62, 67]));
    let tl = timeline(&h, 4.0);
    let mut args = vec![tl.len() as f64];
    for g in &tl {
        args.extend([g.start, if g.end > 1e12 { -1.0 } else { g.end }, g.key.map(|k| k.tonic as f64 + if k.minor { 12.0 } else { 0.0 }).unwrap_or(-1.0), g.chord.iter().fold(0u32, |m, p| m | (1 << p)) as f64]);
    }
    let i = eng.project.tracks[0].inst.as_mut().unwrap();
    i.command("timeline", &args, "");
    i.set_param("tune", 1.0);
    i.command("sync", &[120.0, 0.0, root as f64, 4.0, root as f64], "");
    let y = eng.bounce_range(0.0, 6.0, 0.0);
    let m = mono_of(&y);
    let p1 = chops::chop_f0(&m, 2400, 24000, 48000).unwrap().midi;
    let p2 = chops::chop_f0(&m, 96000 + 2400, 96000 + 24000, 48000).unwrap().midi;
    assert!((p1 - 64.0).abs() < 0.15 && (p2 - 62.0).abs() < 0.15, "{p1} {p2}");
}

#[test]
fn a_users_file_slices_saves_reloads_and_plays_the_same() {
    // an arbitrary mono file at 22.05 kHz from a temp folder
    let times = [0.02f32, 0.4, 0.75, 1.3];
    let x = click_train(&times, 1.8, 22050);
    let dir = tmp("user_project");
    let _ = std::fs::create_dir_all(&dir);
    let wav = tmp("my own loop.wav");
    write_wav(&wav, &x, 22050, 1);
    let mut s = Slicer::new(SR);
    assert_eq!(s.command("open", &[], &wav.to_string_lossy())[0], 4.0);
    s.command("set", &[2.0, 6.0, 1.0], ""); // slice 2 reversed
    s.command("set", &[1.0, 4.0, 7.0], ""); // slice 1 up a fifth
    s.command("set", &[3.0, 2.0, -6.0], ""); // slice 3 quieter
    let root = s.root();
    let notes = s.roll(120.0, 0, 0);
    let mut eng = crate::engine::Engine::new(SR);
    eng.project.bpm = 120.0;
    let mut t = Track::new("mine", TrackKind::Instrument, 0, Some(Box::new(s)));
    t.inst_kind = "slicer".into();
    let mut c = Clip::midi(0.0, 4.0, "s");
    if let ClipData::Midi(n) = &mut c.data {
        *n = notes.clone();
    }
    t.clips.push(c);
    eng.project.tracks.push(t);
    let a = eng.bounce_range(0.0, 4.0, 0.0);
    let proj = dir.join("project.json");
    crate::project::save_with_media(&eng, &proj.to_string_lossy(), true).unwrap();
    // the file was packed into the project and the config points at it
    let doc = crate::project::read(&proj.to_string_lossy()).unwrap();
    assert_eq!(doc.tracks[0].config, "samples/my own loop.wav");
    assert!(dir.join("samples/my own loop.wav").is_file());
    let mut eng2 = crate::engine::Engine::new(SR);
    let mut cache = HashMap::new();
    crate::project::apply_doc(&mut eng2, &doc, Some(&dir), &mut cache);
    let i2 = eng2.project.tracks[0].inst.as_mut().unwrap();
    assert_eq!(i2.command("count", &[], "")[0], 4.0);
    let b = eng2.bounce_range(0.0, 4.0, 0.0);
    assert_eq!(a.len(), b.len());
    let diff = a.iter().zip(&b).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    assert!(diff < 1e-4, "the same slices play the same: {diff}");
    assert!(a.iter().any(|x| x.abs() > 0.1));
    // a missing file: the slices stay, it plays silence, it says why
    let mut doc3 = crate::project::read(&proj.to_string_lossy()).unwrap();
    doc3.tracks[0].config = "samples/gone.wav".into();
    let mut eng3 = crate::engine::Engine::new(SR);
    crate::project::apply_doc(&mut eng3, &doc3, Some(&dir), &mut cache);
    let i3 = eng3.project.tracks[0].inst.as_ref().unwrap();
    assert!(!i3.info("error").is_empty());
    let c3 = eng3.bounce_range(0.0, 2.0, 0.0);
    assert!(c3.iter().all(|x| x.is_finite() && x.abs() < 1e-6));
    let _ = root;
}

#[test]
fn every_kit_loads_and_renders_finite_audio() {
    let probe = Slicer::new(SR);
    assert_eq!(probe.presets().len(), KITS.len());
    for k in KITS.iter() {
        assert!(!k.what.is_empty() && !k.how.is_empty(), "{} has its description", k.name);
        let mut s = Slicer::new(SR);
        s.load_preset(k.name);
        assert!(s.info("error").is_empty(), "{}: {}", k.name, s.info("error"));
        assert!(!s.slices.is_empty(), "{} has slices", k.name);
        let root = s.root();
        let n = s.slices.len().min(8);
        let ev: Vec<(f32, u8, f32, f32)> = (0..n).map(|i| (i as f32 * 0.3, root + i as u8, 0.9, 0.25)).collect();
        let y = render_notes(&mut s, &ev, n as f32 * 0.3 + 0.5);
        assert!(y.iter().all(|x| x.is_finite()), "{} renders finite audio", k.name);
        let peak = y.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.01 && peak < 8.0, "{}: peak {peak}", k.name);
        assert!(!s.preset_info(k.name).is_empty());
    }
}

#[test]
fn state_round_trips_the_slices_and_their_settings() {
    let mut s = tune_slicer();
    s.command("set", &[0.0, 3.0, -0.5], "");
    s.command("set", &[1.0, 13.0, 3.0], "");
    s.command("set", &[1.0, 7.0, 0.25], "");
    s.set_param("tune", 1.0);
    let st = s.state();
    let file = s.info("config");
    let mut r = Slicer::new(SR);
    r.load_state(&st);
    r.command("load", &[], &file);
    assert_eq!(r.slices.iter().map(|x| (x.start, x.end, x.pan, x.choke, x.trim_s)).collect::<Vec<_>>(), s.slices.iter().map(|x| (x.start, x.end, x.pan, x.choke, x.trim_s)).collect::<Vec<_>>());
    assert_eq!(r.tune_mode(), 2);
    assert!(r.slices[0].f0.is_some());
}

/// not a test: how each kit's file cuts in every mode (`-- --ignored --nocapture kit_slice_counts`)
#[test]
#[ignore]
fn kit_slice_counts() {
    for k in KITS.iter() {
        let mut s = Slicer::new(SR);
        s.load_preset(k.name);
        let mut line = format!("{:<20} kit {:>3} |", k.name, s.slices.len());
        for m in [0.0, 0.25, 0.5, 0.75] {
            let mut t = Slicer::new(SR);
            t.load_preset(k.name);
            t.set_param("mode", m);
            line += &format!(" {:>3}", t.slices.len());
        }
        for g in [0.4, 0.5, 0.6] {
            let mut t = Slicer::new(SR);
            t.load_preset(k.name);
            t.set_param("mode", 0.0);
            t.set_param("gap", g);
            line += &format!(" g{g}:{:>3}", t.slices.len());
        }
        println!("{line}  bpm {:.1}", s.src_bpm());
    }
}
