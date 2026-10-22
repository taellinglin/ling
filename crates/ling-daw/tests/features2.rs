use ling_daw::dsp::synth::LingSynth;
use ling_daw::*;
use std::sync::Arc;

const SR: f32 = 48000.0;

fn sine_sample(freq: f32, secs: f32, rate: u32) -> Arc<Sample> {
    let stereo: Vec<f32> = (0..(secs * rate as f32) as usize)
        .flat_map(|i| {
            let s = (i as f32 * freq * std::f32::consts::TAU / rate as f32).sin() * 0.5;
            [s, s]
        })
        .collect();
    Arc::new(Sample::new("sine".into(), String::new(), rate, stereo))
}

fn rms(v: &[f32]) -> f32 {
    (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
}

#[test]
fn export_formats_write_valid_files() {
    let data: Vec<f32> = (0..48000).flat_map(|i| {
        let s = (i as f32 * 440.0 * std::f32::consts::TAU / 48000.0).sin() * 0.4;
        [s, s * 0.5]
    }).collect();
    let dir = std::env::temp_dir();
    for (name, fmt, bits) in [("lsx_t.wav", "wav", 16u16), ("lsx_t24.wav", "wav", 24), ("lsx_t32.wav", "wav", 32), ("lsx_t.flac", "flac", 16), ("lsx_t.ogg", "ogg", 16)] {
        let path = dir.join(name).to_string_lossy().into_owned();
        export::write_audio(&path, &data, 48000, fmt, bits, 0.5).unwrap_or_else(|e| panic!("{name}: {e}"));
        let bytes = std::fs::read(&path).unwrap();
        assert!(bytes.len() > 1000, "{name} too small: {}", bytes.len());
        match fmt {
            "wav" => assert_eq!(&bytes[..4], b"RIFF"),
            "flac" => assert_eq!(&bytes[..4], b"fLaC"),
            _ => assert_eq!(&bytes[..4], b"OggS"),
        }
    }
    // the wav round-trips
    let path = dir.join("lsx_t.wav").to_string_lossy().into_owned();
    let d = ling_music::decode::load(&path).unwrap();
    assert!((d.duration - 1.0).abs() < 0.02);
    // flac and ogg decode through symphonia as well
    let mut r = claxon::FlacReader::open(dir.join("lsx_t.flac")).expect("flac");
    assert_eq!(r.samples().filter(|s| s.is_ok()).count(), 96000);
    let d = ling_music::decode::load(&dir.join("lsx_t.ogg").to_string_lossy()).unwrap_or_else(|e| panic!("ogg: {e}"));
    assert!((d.duration - 1.0).abs() < 0.1, "ogg duration {}", d.duration);
}

#[test]
fn warped_clip_follows_tempo() {
    // A 1 s sine recorded at "120 bpm" played in a 60 bpm project should stretch to 2 s.
    let smp = sine_sample(440.0, 1.0, 44100);
    let mut e = Engine::new(SR);
    e.project.bpm = 60.0;
    let mut t = Track::new("a", TrackKind::Audio, 0, None);
    t.clips.push(Clip { start: 0.0, len: 4.0, name: "s".into(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: smp.clone(), offset_s: 0.0, gain: 1.0, pitch: 0.0, warp: 120.0 } });
    e.project.tracks.push(t);
    e.project.prepare_audio();
    // wait for the background render
    let key = variant_key(0.0, 120.0, 60.0);
    assert_eq!(key, (0, 2000));
    for _ in 0..200 {
        if smp.cached(key).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let v = smp.cached(key).expect("variant never rendered");
    assert!((v.seconds() - 2.0).abs() < 0.05, "stretched length {}", v.seconds());
    e.play();
    let mut out = vec![0.0; (SR * 3.0) as usize * 2];
    for ch in out.chunks_mut(1024) {
        e.render(ch);
    }
    let in_first_1_5s = rms(&out[..(1.5 * SR) as usize * 2]);
    let after_2_5s = rms(&out[(2.5 * SR) as usize * 2..]);
    assert!(in_first_1_5s > 0.1, "no audio in the stretched region");
    assert!(after_2_5s < 0.02, "audio continued past its stretched end: {after_2_5s}");
}

#[test]
fn pitch_variant_transposes_without_changing_length() {
    let smp = sine_sample(440.0, 1.0, 44100);
    let key = variant_key(12.0, 0.0, 120.0);
    assert_eq!(key, (1200, 1000));
    request_variant(&smp, key);
    for _ in 0..200 {
        if smp.cached(key).is_some() {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    let v = smp.cached(key).expect("variant");
    assert!((v.seconds() - 1.0).abs() < 0.03);
}

#[test]
fn performance_recording_writes_arrangement_clips() {
    let mut e = Engine::new(SR);
    e.project.bpm = 120.0;
    let mut t = Track::new("s", TrackKind::Instrument, 0, Some(Box::new(LingSynth::new(SR))));
    t.inst_kind = "synth".into();
    let mut c = Clip::midi(0.0, 2.0, "loop");
    if let ClipData::Midi(n) = &mut c.data {
        n.push(Note::new(0.0, 0.5, 60, 0.9));
    }
    t.session.push(c);
    e.project.tracks.push(t);
    e.project.set_slot(0, 0, 0);
    e.project.quant = 0.0;
    e.perf_rec = true;
    e.launch_scene(0);
    let mut out = vec![0.0; (SR * 4.2) as usize * 2]; // 8.4 beats
    for ch in out.chunks_mut(1024) {
        e.render(ch);
    }
    e.stop_track(0);
    let mut out2 = vec![0.0; (SR * 0.1) as usize * 2];
    e.render(&mut out2);
    let clips = &e.project.tracks[0].clips;
    assert!(clips.len() >= 4, "expected >=4 recorded loop copies, got {}", clips.len());
    assert!(clips.iter().all(|c| matches!(c.data, ClipData::Midi(_))));
    assert!((clips[1].start - clips[0].start - 2.0).abs() < 0.01);
}

#[test]
fn looped_midi_clip_repeats_its_content() {
    let mut e = Engine::new(SR);
    e.project.bpm = 120.0;
    let mut t = Track::new("s", TrackKind::Instrument, 0, Some(Box::new(LingSynth::new(SR))));
    t.inst_kind = "synth".into();
    let mut c = Clip::midi(0.0, 8.0, "loop");
    c.loop_len = 2.0; // 2 beats of content repeated over 8 beats
    if let ClipData::Midi(n) = &mut c.data {
        n.push(Note::new(0.0, 0.4, 60, 0.9));
    }
    t.clips.push(c);
    e.project.tracks.push(t);
    e.play();
    let mut out = vec![0.0; (SR * 4.0) as usize * 2];
    for ch in out.chunks_mut(1024) {
        e.render(ch);
    }
    let win = |a: f32, b: f32| rms(&out[(a * SR) as usize * 2..(b * SR) as usize * 2]);
    for k in 0..4 {
        // a hit every 1 s (2 beats @120)
        assert!(win(k as f32, k as f32 + 0.15) > 0.02, "hit {k} missing");
    }
}

#[test]
fn looped_audio_clip_wraps() {
    let smp = sine_sample(440.0, 1.0, 44100);
    let mut e = Engine::new(SR);
    e.project.bpm = 60.0; // 1 beat = 1 s
    let mut t = Track::new("a", TrackKind::Audio, 0, None);
    // the source is 1 s long; the clip is 3 beats with a 1-beat loop
    let mut c = Clip { start: 0.0, len: 3.0, name: "s".into(), loop_len: 1.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: smp, offset_s: 0.0, gain: 1.0, pitch: 0.0, warp: 0.0 } };
    c.loop_len = 1.0;
    t.clips.push(c);
    e.project.tracks.push(t);
    e.play();
    let mut out = vec![0.0; (SR * 4.0) as usize * 2];
    for ch in out.chunks_mut(1024) {
        e.render(ch);
    }
    let win = |a: f32, b: f32| rms(&out[(a * SR) as usize * 2..(b * SR) as usize * 2]);
    assert!(win(2.2, 2.8) > 0.1, "third repetition missing");
    assert!(win(3.2, 3.9) < 0.01, "audio continued past clip end");
}
