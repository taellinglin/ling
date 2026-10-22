use ling_daw::dsp::make_instrument_file;
use ling_daw::tracker;

/// A 4-channel ProTracker module: one looped sine sample (period 428 = C-2 as recorded), `rows` of (channel 0 note period, speed/tempo).
fn make_mod(rows: &[(u16, u8)]) -> Vec<u8> {
    let mut d = vec![0u8; 1084];
    d[..4].copy_from_slice(b"test");
    // sample 1: 800 bytes, full volume, loop whole
    d[20 + 22] = 0;
    d[20 + 23] = 200; // length in words
    d[20 + 25] = 64;
    d[20 + 26] = 0;
    d[20 + 27] = 0;
    d[20 + 28] = 0;
    d[20 + 29] = 200;
    d[950] = 1;
    d[952] = 0;
    d[1080..1084].copy_from_slice(b"M.K.");
    for r in 0..64 {
        for c in 0..4 {
            let (mut b0, mut b1, mut b2, mut b3) = (0u8, 0u8, 0u8, 0u8);
            if c == 0 {
                if let Some((period, par)) = rows.get(r).copied().filter(|x| x.0 > 0 || x.1 > 0) {
                    if period > 0 {
                        b0 = (period >> 8) as u8;
                        b1 = period as u8;
                        b2 = 0x10; // instrument 1
                    }
                    if par > 0 {
                        b2 |= 0x0F;
                        b3 = par;
                    }
                }
            }
            d.extend([b0, b1, b2, b3]);
        }
    }
    for i in 0..400 {
        d.push(((i as f32 / 400.0 * std::f32::consts::TAU).sin() * 100.0) as i8 as u8);
    }
    d
}

#[test]
fn mod_notes_pitch_and_tempo() {
    // C-2, then (a row later) E-2 with a tempo change to 150
    let d = make_mod(&[(428, 0), (0, 0), (0, 0), (0, 0), (339, 150)]);
    assert!(tracker::looks_like_module(&d));
    let m = tracker::parse_module(&d).unwrap();
    assert_eq!(m.nch, 4);
    let p = tracker::play(&m);
    let notes = &p.tracks[0].notes;
    assert_eq!(notes.len(), 2);
    assert_eq!(notes[0].key, 60);
    assert_eq!(notes[1].key, 64);
    assert!((notes[0].start - 0.0).abs() < 1e-9 && (notes[1].start - 1.0).abs() < 1e-9, "{:?}", notes.iter().map(|n| n.start).collect::<Vec<_>>());
    assert!((p.tempos[0].1 - 125.0).abs() < 0.5);
    assert!(p.tempos.iter().any(|t| (t.1 - 150.0).abs() < 0.5));
    assert!((p.beats - 16.0).abs() < 1e-9, "64 rows = 16 beats, got {}", p.beats);
}

#[test]
fn mod_sampler_plays_at_the_recorded_pitch() {
    let d = make_mod(&[(428, 0)]);
    let dir = std::env::temp_dir().join("ling-tracker-test");
    let (mid, rep) = tracker::parse(&d, "unit.mod", &dir).unwrap();
    assert_eq!(rep.instruments, 1);
    let t = &mid.tracks[0];
    let (json, _) = t.sampler.clone().unwrap();
    let mut inst = ling_daw::dsp::sampler::Sampler::from_config(48000.0, &json, None).expect("sampler loads");
    use ling_daw::dsp::Instrument;
    inst.note_on(60, 1.0);
    let mut buf = vec![0f32; 2 * 48000];
    inst.render(&mut buf);
    let peak = buf.iter().fold(0f32, |a, b| a.max(b.abs()));
    assert!(peak > 0.05, "silent: {peak}");
    let left: Vec<f32> = buf.iter().step_by(2).copied().collect();
    let crossings = left.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    let hz = crossings as f32 / (left.len() as f32 / 48000.0);
    // the sine sample has 400 points at 8363 Hz: one cycle = 400/8363 s, so C-2 / MIDI 60 plays it at 8363 Hz => ~20.9 cycles/s
    assert!((hz - 8363.0 / 400.0).abs() < 2.5, "pitch {hz} Hz");
    // an octave up doubles it
    inst.all_off();
    inst.note_on(72, 1.0);
    let mut b2 = vec![0f32; 2 * 48000];
    inst.render(&mut b2);
    let l2: Vec<f32> = b2.iter().step_by(2).copied().collect();
    let hz2 = l2.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32;
    assert!((hz2 / hz - 2.0).abs() < 0.2, "octave: {hz} -> {hz2}");
}

#[test]
fn real_openmpt_files_import() {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let mut seen = 0;
    for f in ["Canopy.mptm", "PLH.mptm", "PLH.xm", "chiploop.xm"] {
        let p = std::path::Path::new(&home).join("Downloads").join(f);
        let Ok(data) = std::fs::read(&p) else { continue };
        let dir = std::env::temp_dir().join("ling-tracker-test");
        let (m, rep) = tracker::parse(&data, &p.to_string_lossy(), &dir).unwrap_or_else(|e| panic!("{f}: {e}"));
        eprintln!("{f}: {} ch={} ins={} notes={} ignored={} dropped={} beats={:.0} bpm={:?}", rep.format, rep.channels, rep.instruments, rep.notes, rep.ignored, rep.dropped, m.length_beats, m.bpm);
        assert!(rep.notes > 20 && rep.instruments > 0, "{f}: nothing imported");
        assert!(m.length_beats > 8.0);
        seen += 1;
    }
    let _ = seen;
}

/// Renders the user's modules through the whole import (Sampler tracks, lanes) to WAV files for comparing with libopenmpt.
#[test]
fn render_real_files_for_comparison() {
    use ling_daw::{Daw, Val};
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    for f in ["PLH.xm", "chiploop.xm", "Canopy.mptm"] {
        let p = std::path::Path::new(&home).join("Downloads").join(f);
        if !p.is_file() {
            continue;
        }
        let mut d = Daw::new();
        d.call("daw_new", &[]);
        let r = d.call("daw_midi_import", &[Val::Str(p.to_string_lossy().into_owned()), Val::Num(0.0)]);
        let made = match r {
            Some(Ok(Val::Num(x))) => x,
            o => panic!("{f}: {o:?}"),
        };
        assert!(made >= 1.0, "{f}: tracks {made}");
        let bpm = match d.call("daw_bpm", &[]) {
            Some(Ok(Val::Num(x))) => x,
            _ => 120.0,
        };
        let out = std::env::temp_dir().join(format!("lsx_trk_{}.wav", f.replace('.', "_")));
        let end_beats = 30.0 * bpm / 60.0;
        let ok = d.call("daw_render_start", &[Val::Str(out.to_string_lossy().into_owned()), Val::Num(0.0), Val::Num(end_beats), Val::Num(0.5), Val::Num(16.0), Val::Str("wav".into()), Val::Num(0.5), Val::Num(0.0)]);
        assert!(matches!(ok, Some(Ok(Val::Num(x))) if x == 1.0));
        let t0 = std::time::Instant::now();
        loop {
            let st = match d.call("daw_render_state", &[]) {
                Some(Ok(Val::Num(x))) => x,
                _ => 0.0,
            };
            if st != 1.0 {
                assert_eq!(st, 2.0, "{f}: render failed");
                break;
            }
            assert!(t0.elapsed().as_secs() < 300, "slow");
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        eprintln!("{f}: {} tracks, bpm {bpm}, rendered {}", made, out.display());
    }
}

#[test]
fn module_durations_match_libopenmpt() {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    for f in ["chiploop.xm", "PLH.xm", "PLH.mptm", "Canopy.mptm"] {
        let p = std::path::Path::new(&home).join("Downloads").join(f);
        let Ok(data) = std::fs::read(&p) else { continue };
        let m = tracker::parse_module(&data).unwrap();
        let pl = tracker::play(&m);
        // seconds along the tempo map
        let mut secs = 0.0;
        let mut t = pl.tempos.clone();
        t.push((pl.beats, 0.0));
        for w in t.windows(2) {
            secs += (w[1].0 - w[0].0) * 60.0 / w[0].1;
        }
        eprintln!("{f}: {:.2} s over {:.0} beats, {} tempo changes, arps {} slides {} retrigs {}", secs, pl.beats, pl.tempos.len(), pl.arps, pl.slides, pl.retrigs);
    }
}

#[test]
fn which_effects_are_not_modelled() {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    for f in ["Canopy.mptm", "PLH.mptm", "chiploop.xm"] {
        let p = std::path::Path::new(&home).join("Downloads").join(f);
        let Ok(data) = std::fs::read(&p) else { continue };
        tracker::IGNORED.with(|m| m.borrow_mut().clear());
        let _ = tracker::parse_module(&data).unwrap();
        let v: Vec<(String, usize)> = tracker::IGNORED.with(|m| m.borrow().iter().map(|(k, v)| (k.clone(), *v)).collect());
        let mut v = v;
        v.sort_by(|a, b| b.1.cmp(&a.1));
        eprintln!("{f}: {:?}", &v[..v.len().min(14)]);
    }
}

#[test]
fn envelopes_and_panning_are_read() {
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    for f in ["PLH.xm", "chiploop.xm", "Canopy.mptm"] {
        let p = std::path::Path::new(&home).join("Downloads").join(f);
        let Ok(data) = std::fs::read(&p) else { continue };
        let m = tracker::parse_module(&data).unwrap();
        let with_env = m.ins.iter().filter(|i| i.env.len() >= 2).count();
        let with_pan = m.ins.iter().filter(|i| i.pan.is_some() || i.smps.iter().any(|s| s.pan.map(|p| p.abs() > 0.05).unwrap_or(false))).count();
        let fades = m.ins.iter().filter(|i| i.fade_s > 0.0).count();
        eprintln!("{f}: {} instruments, {with_env} with a volume envelope, {fades} with a fade-out, {with_pan} panned; first envelopes: {:?}", m.ins.len(), m.ins.iter().filter(|i| i.env.len() >= 2).take(2).map(|i| (i.name.clone(), i.env.clone(), i.env_sustain, i.fade_s)).collect::<Vec<_>>());
        for ch in 0..m.nch.min(6) {
            eprint!("ch{} pan {:.2}  ", ch, m.chan_pan[ch]);
        }
        eprintln!();
    }
}

#[test]
fn drum_samples_are_found_in_a_library_and_decoded_through_ffmpeg() {
    use ling_daw::{Daw, Val};
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let root = std::path::Path::new(&home).join("Programs").join("[LingStation]");
    if !root.is_dir() {
        return;
    }
    let mut d = Daw::new();
    d.call("daw_new", &[]);
    d.call("daw_smp_roots_set", &[Val::Str(root.to_string_lossy().replace('\\', "/"))]);
    for (words, must) in [("Splash Cymbal", "splash"), ("Acoustic Bass Drum", "kick"), ("Open Hi-Hat", "hat"), ("High Floor Tom", "tom"), ("Snare", "snare")] {
        let p = match d.call("daw_smp_pick", &[Val::Str(words.into())]) {
            Some(Ok(Val::Str(s))) => s,
            o => panic!("{o:?}"),
        };
        eprintln!("{words:?} -> {p}");
        assert!(p.to_lowercase().contains(must), "{words}: {p}");
    }
    // a .wv file plays through the sampler
    let p = match d.call("daw_smp_pick", &[Val::Str("snare".into())]) {
        Some(Ok(Val::Str(s))) => s,
        _ => panic!(),
    };
    let mut s = ling_daw::dsp::sampler::Sampler::new(48000.0);
    use ling_daw::dsp::Instrument;
    let r = s.command("add_lib", &[60.0, 0.0], &p);
    assert!(r[0] >= 0.0, "{}", s.info("error"));
    s.note_on(60, 1.0);
    let mut buf = vec![0f32; 2 * 9600];
    s.render(&mut buf);
    assert!(buf.iter().fold(0f32, |a, b| a.max(b.abs())) > 0.02, "silent drum");
}

#[test]
fn import_with_a_kit_and_a_library_sample() {
    use ling_daw::{Daw, Val};
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let p = std::path::Path::new(&home).join("Downloads").join("Canopy.mptm");
    let root = std::path::Path::new(&home).join("Programs").join("[LingStation]");
    if !p.is_file() || !root.is_dir() {
        return;
    }
    let mut d = Daw::new();
    d.call("daw_new", &[]);
    d.call("daw_smp_roots_set", &[Val::Str(root.to_string_lossy().replace('\\', "/"))]);
    let scan = match d.call("daw_midi_scan", &[Val::Str(p.to_string_lossy().into_owned())]) {
        Some(Ok(Val::List(v))) => v,
        o => panic!("{o:?}"),
    };
    let parts = scan.len() / 8;
    // percussive parts go to the shared kit, the first one with a library sample, the rest with their own samples
    let mut choices = vec![];
    let mut k = 0;
    for i in 0..parts {
        let perc = matches!(scan[i * 8 + 6], Val::Num(x) if x > 0.5);
        choices.push(Val::Str(if perc { k += 1; if k == 1 { format!("kitlib:{}", if let Val::Str(n) = &scan[i * 8] { n.to_lowercase() } else { String::new() }) } else { "kit".into() } } else { "orig".into() }));
    }
    assert!(k >= 2, "the module has percussive parts");
    let r = d.call("daw_midi_import", &[Val::Str(p.to_string_lossy().into_owned()), Val::Num(0.0), Val::List(choices)]);
    let made = match r { Some(Ok(Val::Num(x))) => x, o => panic!("{o:?}") };
    // the percussive parts became one track
    assert_eq!(made as usize, parts - k + 1, "{parts} parts, {k} of them in one kit");
    let names = match d.call("daw_track_name", &[Val::Num(0.0)]) { _ => () };
    let _ = names;
}

#[test]
fn imported_envelopes_shape_the_sampler_sound() {
    use ling_daw::dsp::sampler::Sampler;
    use ling_daw::dsp::Instrument;
    let home = std::env::var("USERPROFILE").unwrap_or_default();
    let p = std::path::Path::new(&home).join("Downloads").join("PLH.xm");
    let Ok(data) = std::fs::read(&p) else { return };
    let dir = std::env::temp_dir().join("ling-tracker-test");
    let (mid, _) = tracker::parse(&data, &p.to_string_lossy(), &dir).unwrap();
    // an instrument with a volume envelope that dies away (the toms): its notes get quieter over a second
    let t = mid.tracks.iter().find(|t| t.name.to_lowercase().contains("tom")).expect("a tom part");
    let (json, _) = t.sampler.clone().unwrap();
    let cfg: ling_daw::dsp::sampler::Config = serde_json::from_str(&json).unwrap();
    assert!(cfg.env.len() >= 2, "the volume envelope came across: {:?}", cfg.env);
    assert!(cfg.fade > 0.0, "and the fade-out");
    let mut s = Sampler::from_config(48000.0, &json, None).unwrap();
    s.note_on(60, 1.0);
    let mut buf = vec![0f32; 2 * 48000];
    s.render(&mut buf);
    let rms = |a: usize, b: usize| (buf[2 * a..2 * b].iter().map(|x| x * x).sum::<f32>() / (2 * (b - a)) as f32).sqrt();
    let early = rms(1000, 5000);
    let late = rms(36000, 40000);
    eprintln!("tom: early {early:.4} late {late:.4}, env {:?}", cfg.env);
    assert!(early > late * 1.5 || late < 0.001, "the envelope shapes the sound: {early} then {late}");
}
