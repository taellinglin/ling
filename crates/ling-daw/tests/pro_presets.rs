//! Measures the "Radio" preset files of Prism and Growl (instruments/Prism/*Radio*.ling, instruments/Growl/*Radio*.ling): every one
//! must load, make sound, stay finite and keep its peaks below -1 dBFS on a typical chord (Prism) / a low riff (Growl); the table of
//! levels, brightness and width is printed.  `cargo test --release -p ling-daw --test pro_presets -- --nocapture`
//! (LSX_ROOT = the LingStationX folder)
use ling_daw::*;

const SR: f32 = 48000.0;

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

fn files(root: &str, dir: &str) -> Vec<std::path::PathBuf> {
    let Ok(rd) = std::fs::read_dir(format!("{root}/instruments/{dir}")) else { return vec![] };
    let mut v: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "ling").unwrap_or(false) && p.file_name().unwrap().to_string_lossy().contains("Radio") && !p.file_name().unwrap().to_string_lossy().contains("Ghost")).collect();
    v.sort();
    v
}

/// (peak dBFS, rms dBFS of the loud half, side/mid ratio, share of energy above 4 kHz) for a held chord then its release
fn chord(inst: &mut dyn dsp::Instrument, keys: &[u8], vel: f32) -> (f32, f32, f32, f32) {
    inst.all_off();
    for k in keys {
        inst.note_on(*k, vel);
    }
    let mut out = vec![0f32; (SR * 3.0) as usize * 2];
    let mut i = 0;
    while i < out.len() {
        if i == (SR * 2.0) as usize * 2 {
            for k in keys {
                inst.note_off(*k);
            }
        }
        let n = 512.min(out.len() - i);
        inst.render(&mut out[i..i + n]);
        i += n;
    }
    inst.all_off();
    assert!(out.iter().all(|x| x.is_finite()), "non-finite output");
    let peak = out.iter().fold(0f32, |m, x| m.max(x.abs()));
    let held = &out[..(SR * 2.0) as usize * 2];
    let win = (SR * 0.05) as usize * 2;
    let mut env: Vec<f32> = held.chunks(win).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt()).collect();
    env.sort_by(|a, b| b.total_cmp(a));
    let top = &env[..env.len() / 2];
    let rms = (top.iter().map(|x| x * x).sum::<f32>() / top.len() as f32).sqrt();
    let (mut m2, mut s2) = (0f32, 0f32);
    for f in held.chunks_exact(2) {
        m2 += (f[0] + f[1]).powi(2) * 0.25;
        s2 += (f[0] - f[1]).powi(2) * 0.25;
    }
    // energy above 4 kHz by a first difference twice (a crude high-pass)
    let mono: Vec<f32> = held.chunks_exact(2).map(|f| (f[0] + f[1]) * 0.5).collect();
    let hp: Vec<f32> = mono.windows(3).map(|w| w[2] - 2.0 * w[1] + w[0]).collect();
    let (eh, et) = (hp.iter().map(|x| x * x).sum::<f32>(), mono.iter().map(|x| x * x).sum::<f32>() * 16.0);
    (db(peak), db(rms), (s2 / m2.max(1e-12)).sqrt(), (eh / et.max(1e-12)).min(1.0))
}

#[test]
fn radio_presets_are_sane() {
    let root = std::env::var("LSX_ROOT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX").into());
    audio::flush_denormals();
    let mut bad = vec![];
    let mut tune_bad: Vec<String> = vec![];
    let mut n = 0;
    for (kind, dir, keys) in [("prism", "Prism", vec![48u8, 55, 60, 64]), ("growl", "Growl", vec![40u8])] {
        let list = files(&root, dir);
        for f in list {
            let text = std::fs::read_to_string(&f).unwrap();
            let p = patch::parse(&text);
            assert_eq!(p.kind, kind, "{}", f.display());
            let mut inst = patch::build(&p, SR).unwrap_or_else(|| panic!("{} does not build", f.display()));
            // every key in the file must be a real parameter of the instrument
            let names: Vec<&str> = inst.params().iter().map(|x| x.0).collect();
            for (k, _) in &p.params {
                assert!(names.contains(&k.as_str()), "{}: unknown parameter '{k}'", f.display());
            }
            let (pk, rms, side, hf) = chord(inst.as_mut(), &keys, 0.9);
            // a high chord too (the top of the range must not clip either)
            let hi_keys: Vec<u8> = keys.iter().map(|k| k + 12).collect();
            let (pk2, _, _, _) = chord(inst.as_mut(), &hi_keys, 1.0);
            // the pitch is steady and on the note (no vibrato, no sweep): the tune check on a fresh copy
            let mut fresh = patch::build(&p, SR).unwrap();
            let t = tunecheck::check(fresh.as_mut(), SR);
            eprintln!("{kind:<6} {:<34} peak {pk:>6.1} / {pk2:>6.1} dBFS  rms {rms:>6.1}  side/mid {side:>4.2}  hf {:>5.1} %  tune {}", p.name, hf * 100.0, if t.in_tune() { format!("in tune ({:+.0}c)", t.cents) } else { format!("OFF {:+.0}c worst {:.0} pitched {:.2}", t.cents, t.worst, t.pitched) });
            if !t.in_tune() {
                tune_bad.push(p.name.clone());
            }
            n += 1;
            if pk > -1.0 || pk2 > -1.0 || rms < -40.0 {
                bad.push(format!("{}: peak {pk:.1}/{pk2:.1} rms {rms:.1}", p.name));
            }
        }
    }
    eprintln!("{n} radio presets measured");
    eprintln!("not in tune (unpitched sounds are exempt in the check): {tune_bad:?}");
    assert!(bad.is_empty(), "levels: {bad:?}");
}
