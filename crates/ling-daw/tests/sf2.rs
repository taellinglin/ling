use ling_daw::dsp::sf2::{self, Sf2Player};
use ling_daw::dsp::Instrument;

fn font() -> Option<String> {
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../LingStationX/assets/soundfonts/Ling.sf2");
    if p.exists() {
        return Some(p.to_string_lossy().into_owned());
    }
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../[LingStation]/assets/synths/Ling.sf2");
    p.exists().then(|| p.to_string_lossy().into_owned())
}

#[test]
fn parses_and_every_preset_plays() {
    let Some(path) = font() else { return };
    let sf = sf2::load(&path).unwrap();
    assert!(sf.presets.len() > 100, "{} presets", sf.presets.len());
    println!("{} - {} presets, {} samples", sf.name, sf.presets.len(), sf.samples.len());
    let mut silent = vec![];
    for (i, pr) in sf.presets.iter().enumerate() {
        let mut p = Sf2Player::new(48000.0, &path).unwrap();
        p.select_preset_index(i);
        // a key inside the preset's first layer
        let l = pr.layers[0];
        let key = ((l.key_lo as u32 + l.key_hi as u32) / 2) as u8;
        p.note_on(key, 0.8);
        let mut buf = vec![0.0; 48000 * 2];
        p.render(&mut buf);
        assert!(buf.iter().all(|x| x.is_finite()), "{} non-finite", pr.name);
        let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak <= 4.0, "{} too loud: {peak}", pr.name);
        if peak < 1e-4 {
            silent.push(format!("{}:{} {}", pr.bank, pr.program, pr.name));
        }
    }
    assert!(silent.len() * 20 < sf.presets.len(), "too many silent presets: {silent:?}");
}

#[test]
fn piano_note_has_body_and_releases() {
    let Some(path) = font() else { return };
    let mut p = Sf2Player::new(48000.0, &path).unwrap();
    assert!(p.midi_select(1, 1, false), "bank 1 program 1 (PIANO)");
    p.note_on(60, 0.9);
    let mut a = vec![0.0; 24000 * 2];
    p.render(&mut a);
    p.note_off(60);
    let mut b = vec![0.0; 96000 * 2];
    p.render(&mut b);
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len() as f32).sqrt();
    assert!(rms(&a) > 0.01, "rms {}", rms(&a));
    assert!(rms(&b[b.len() - 9600..]) < rms(&a) * 0.05, "did not release");
    // a higher key must be higher in pitch: count zero crossings
    let cross = |key: u8| {
        let mut p = Sf2Player::new(48000.0, &path).unwrap();
        p.midi_select(1, 1, false);
        p.note_on(key, 0.9);
        let mut b = vec![0.0; 24000 * 2];
        p.render(&mut b);
        b.chunks(2).map(|f| f[0]).collect::<Vec<_>>().windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count()
    };
    assert!(cross(72) > cross(60) * 3 / 2, "octave up should be about twice the crossings");
}
