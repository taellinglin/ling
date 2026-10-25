//! Silent tracks sleep (are not rendered): the audio must come out the same as when every track renders all the time, and faster.
use ling_daw::*;

fn build(sr: f32) -> Engine {
    let mut e = Engine::new(sr);
    let kinds = ["synth", "growl", "drums", "pluck", "synth", "growl"];
    for (i, k) in kinds.iter().enumerate() {
        let inst = dsp::make_instrument(k, sr).unwrap();
        let mut t = Track::new(&format!("t{i}"), TrackKind::Instrument, 0xff0000, Some(inst));
        t.inst_kind = k.to_string();
        // a phrase, a long rest (much longer than the sleep time), the phrase again; each track at its own time
        let mut c = Clip::midi(0.0, 64.0, "c");
        if let ClipData::Midi(n) = &mut c.data {
            for start in [i as f64 * 2.0, 40.0 + i as f64] {
                for b in 0..8 {
                    n.push(Note::new(start + b as f64 * 0.5, 0.4, 40 + (i as u8 * 5 + b as u8 * 3) % 30, 0.8));
                }
            }
        }
        t.clips.push(c);
        t.effects = ["Reverb", "Delay", "OTT"].iter().filter_map(|f| EffectSlot::new(f, sr)).collect();
        e.project.tracks.push(t);
    }
    // a muted track that nothing listens to
    e.project.tracks[5].mute = true;
    e
}

#[test]
fn sleeping_tracks_sound_the_same_and_save_time() {
    let sr = 48000.0;
    set_track_sleep(false);
    let t0 = std::time::Instant::now();
    let a = build(sr).bounce(1.0);
    let awake = t0.elapsed();
    set_track_sleep(true);
    let t1 = std::time::Instant::now();
    let b = build(sr).bounce(1.0);
    let slept = t1.elapsed();
    eprintln!("always rendering {awake:?}, with sleep {slept:?}");
    assert_eq!(a.len(), b.len());
    assert!(a.iter().any(|x| x.abs() > 0.01), "the project must make sound");
    // only what was already below -100 dB can differ
    let worst = a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
    assert!(worst < 1e-4, "sleeping changed the audio by up to {worst}");
    let rms = |v: &[f32]| (v.iter().map(|x| x * x).sum::<f32>() / v.len() as f32).sqrt();
    let diff: Vec<f32> = a.iter().zip(b.iter()).map(|(x, y)| x - y).collect();
    assert!(rms(&diff) < rms(&a) * 1e-3, "difference {} vs signal {}", rms(&diff), rms(&a));
    assert!(slept < awake, "sleep should be faster: {slept:?} vs {awake:?}");
}

#[test]
fn a_live_note_wakes_a_sleeping_track() {
    let sr = 48000.0;
    let mut e = Engine::new(sr);
    let mut t = Track::new("s", TrackKind::Instrument, 0, Some(dsp::make_instrument("synth", sr).unwrap()));
    t.inst_kind = "synth".into();
    e.project.tracks.push(t);
    let mut buf = vec![0f32; 512];
    for _ in 0..(SLEEP_AFTER as usize / 256 + 10) {
        e.render(&mut buf);
    }
    assert!(e.asleep(0), "a silent track goes to sleep");
    e.live_note(0, 60, 0.9, true);
    let mut peak = 0.0f32;
    for _ in 0..20 {
        e.render(&mut buf);
        peak = buf.iter().fold(peak, |m, x| m.max(x.abs()));
    }
    assert!(peak > 0.01 && !e.asleep(0), "the note woke it ({peak})");
}

#[test]
#[ignore]
fn which_instrument_differs() {
    let sr = 48000.0;
    for k in ["synth", "growl", "drums", "pluck", "prism", "voice", "sampler", "sf2", "guitar"] {
        if dsp::make_instrument(k, sr).is_none() {
            eprintln!("{k}: not a plain instrument kind");
            continue;
        }
        for fx in [false, true] {
            let mk = || {
                let mut e = Engine::new(sr);
                let mut t = Track::new("t", TrackKind::Instrument, 0, Some(dsp::make_instrument(k, sr).unwrap()));
                t.inst_kind = k.to_string();
                let mut c = Clip::midi(0.0, 64.0, "c");
                if let ClipData::Midi(n) = &mut c.data {
                    for start in [0.0, 40.0] {
                        for b in 0..8 {
                            n.push(Note::new(start + b as f64 * 0.5, 0.4, 45 + b as u8 * 2, 0.8));
                        }
                    }
                }
                t.clips.push(c);
                if fx {
                    t.effects = ["Reverb", "Delay", "OTT"].iter().filter_map(|f| EffectSlot::new(f, sr)).collect();
                }
                e.project.tracks.push(t);
                e
            };
            set_track_sleep(false);
            let a = mk().bounce(1.0);
            set_track_sleep(true);
            let b = mk().bounce(1.0);
            let worst = a.iter().zip(b.iter()).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
            eprintln!("{k} fx={fx}: worst diff {worst}");
        }
    }
}
