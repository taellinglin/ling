use ling_daw::dsp::drums::DrumSynth;
use ling_daw::dsp::Instrument;
use ling_daw::patch;

fn rms(b: &[f32]) -> f32 {
    (b.iter().map(|x| x * x).sum::<f32>() / b.len().max(1) as f32).sqrt()
}

fn hit(d: &mut DrumSynth, key: u8, secs: f32) -> Vec<f32> {
    d.note_on(key, 1.0);
    let mut buf = vec![0.0; (48000.0 * secs) as usize * 2];
    d.render(&mut buf);
    buf
}

/// No GM key may leave a sustained tail: with the longest decay the last second must be (nearly) silent.
#[test]
fn every_key_dies_out_even_at_max_decay() {
    for key in 35u8..=82 {
        let mut d = DrumSynth::new(48000.0);
        for part in 0..8 {
            d.select_part(part);
            d.set_param("decay", 1.0);
        }
        let b = hit(&mut d, key, 6.0);
        assert!(b.iter().all(|x| x.is_finite()), "key {key} non-finite");
        let tail = rms(&b[b.len() - 48000 * 2..]);
        assert!(tail < 0.0015, "key {key} still sounding after 5 s: rms {tail}");
    }
}

/// Default snare / hats / clap: the noise must be gone well inside a second.
#[test]
fn default_noise_pieces_are_short() {
    for (key, limit) in [(38u8, 0.7f32), (40, 0.8), (37, 0.3), (39, 0.7), (42, 0.3), (44, 0.3), (46, 1.6), (54, 2.0), (56, 0.9)] {
        let mut d = DrumSynth::new(48000.0);
        let b = hit(&mut d, key, limit + 0.5);
        let tail = rms(&b[(limit * 48000.0) as usize * 2..]);
        let peak = b.iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak > 0.05, "key {key} silent");
        assert!(tail < peak * 0.01, "key {key}: tail {tail} vs peak {peak}");
    }
}

#[test]
fn stock_kits_load_and_play_every_piece() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../LingStationX/instruments");
    let Ok(rd) = std::fs::read_dir(&dir) else { return };
    let mut kits = 0;
    for e in rd.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        if !name.starts_with("Kit_") {
            continue;
        }
        kits += 1;
        let p = patch::parse(&std::fs::read_to_string(e.path()).unwrap());
        assert_eq!(p.kind, "drums", "{name}");
        for key in [36u8, 38, 39, 41, 45, 50, 42, 46, 49, 51] {
            let mut d = patch::build(&p, 48000.0).unwrap();
            d.note_on(key, 0.9);
            let mut buf = vec![0.0; 48000 * 2 * 3];
            d.render(&mut buf);
            assert!(buf.iter().all(|x| x.is_finite()), "{name} key {key}");
            let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs()));
            assert!(peak > 0.03 && peak <= 1.3, "{name} key {key}: peak {peak}");
        }
    }
    assert!(kits >= 8, "expected the 8 stock kits, found {kits}");
}
