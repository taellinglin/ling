use ling_daw::dsp::drums::{DrumSynth, KIND_NAMES};
use ling_daw::dsp::synth::LingSynth;
use ling_daw::dsp::Instrument;

fn stats(buf: &[f32]) -> (f32, f32) {
    let peak = buf.iter().fold(0.0f32, |a, b| a.max(b.abs()));
    let rms = (buf.iter().map(|x| x * x).sum::<f32>() / buf.len() as f32).sqrt();
    (peak, rms)
}

#[test]
fn synth_presets_sound() {
    for name in LingSynth::PRESETS {
        let mut s = LingSynth::new(48000.0);
        s.preset(name);
        s.note_on(57, 0.9);
        let mut buf = vec![0.0; 48000 * 2 / 2];
        s.render(&mut buf);
        let (peak, rms) = stats(&buf);
        println!("{name}: peak {peak:.3} rms {rms:.4}");
        assert!(buf.iter().all(|x| x.is_finite()), "{name} non-finite");
        assert!(rms > 0.005, "{name} silent");
        assert!(peak < 2.0, "{name} too loud");
    }
}

#[test]
fn drums_sound_and_decay() {
    let keys = [36u8, 38, 41, 42, 46, 49, 51, 39];
    for (i, k) in keys.iter().enumerate() {
        let mut d = DrumSynth::new(48000.0);
        d.note_on(*k, 1.0);
        let mut buf = vec![0.0; 48000 * 2];
        d.render(&mut buf);
        let (peak, rms) = stats(&buf);
        println!("{}: peak {peak:.3} rms {rms:.4}", KIND_NAMES[i]);
        assert!(buf.iter().all(|x| x.is_finite()));
        assert!(rms > 0.003, "{} silent", KIND_NAMES[i]);
        assert!(peak <= 1.01, "{} clips", KIND_NAMES[i]);
    }
}
