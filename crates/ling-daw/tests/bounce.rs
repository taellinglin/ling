use ling_daw::*;
use ling_daw::dsp::drums::DrumSynth;
use ling_daw::dsp::synth::LingSynth;
use std::sync::Arc;

fn project(sr: f32) -> Engine {
    let mut e = Engine::new(sr);
    let mut syn = Track::new("Synth", TrackKind::Instrument, 0xff8040, Some(Box::new(LingSynth::new(sr))));
    let mut c = Clip::midi(0.0, 4.0, "riff");
    if let ClipData::Midi(n) = &mut c.data {
        for i in 0..4 {
            n.push(Note::new(i as f64, 0.5, 48 + i as u8 * 3, 0.9));
        }
    }
    syn.clips.push(c);
    syn.effects.push(EffectSlot::new("Delay", sr).unwrap());
    let mut dr = Track::new("Drums", TrackKind::Instrument, 0x40a0ff, Some(Box::new(DrumSynth::new(sr))));
    let mut c = Clip::midi(0.0, 4.0, "beat");
    if let ClipData::Midi(n) = &mut c.data {
        for i in 0..8 {
            n.push(Note::new(i as f64 * 0.5, 0.1, if i % 4 == 0 { 36 } else if i % 4 == 2 { 38 } else { 42 }, 1.0));
        }
    }
    dr.clips.push(c);
    // audio clip: 1 s 440 Hz sine
    let stereo: Vec<f32> = (0..44100).flat_map(|i| { let s = (i as f32 * 440.0 * std::f32::consts::TAU / 44100.0).sin() * 0.5; [s, s] }).collect();
    let smp = Arc::new(Sample::new("sine".into(), String::new(), 44100, stereo));
    let mut au = Track::new("Audio", TrackKind::Audio, 0x60ff60, None);
    au.clips.push(Clip { start: 2.0, len: 2.0, name: "sine".into(), loop_len: 0.0, lyric_text: String::new(), lyric_lang: String::new(), tag: String::new(), data: ClipData::Audio { sample: smp, offset_s: 0.0, gain: 1.0, pitch: 0.0, warp: 0.0 } });
    e.project.tracks.extend([syn, dr, au]);
    e.project.master_effects.push(EffectSlot::new("Compressor", sr).unwrap());
    e
}

#[test]
fn bounce_has_content_in_all_sections() {
    let mut e = project(48000.0);
    let out = e.bounce(1.0);
    let secs = out.len() as f32 / 2.0 / 48000.0;
    assert!((secs - 3.0).abs() < 0.05, "length {secs}");
    assert!(out.iter().all(|x| x.is_finite()));
    let rms = |a: f32, b: f32| {
        let s = &out[(a * 48000.0) as usize * 2..(b * 48000.0) as usize * 2];
        (s.iter().map(|x| x * x).sum::<f32>() / s.len() as f32).sqrt()
    };
    assert!(rms(0.0, 0.5) > 0.01, "first beat silent");
    assert!(rms(1.0, 1.5) > 0.01, "audio clip region silent");
    assert!(rms(2.5, 3.0) < rms(0.0, 0.5), "tail should be quieter");
    e.bounce_wav(std::env::temp_dir().join("lsx_test.wav").to_str().unwrap(), 1.0).unwrap();
}

#[test]
fn mute_and_solo() {
    let mut e = project(48000.0);
    for t in e.project.tracks.iter_mut() {
        t.mute = true;
    }
    let out = e.bounce(0.0);
    assert!(out.iter().all(|x| x.abs() < 1e-6));
}
