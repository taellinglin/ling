//! The Growl / Prism synthesis block end to end: resample to wavetable and bounce to Sampler through the builtins (a private engine
//! renders; the project keeps the table), and what the new parts cost
//! (`cargo test --release -p ling-daw --test synth_block cost -- --ignored --nocapture`).
use ling_daw::*;
use ling_daw::{Daw, Val};

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.into())
}
fn call(d: &mut Daw, name: &str, a: &[Val]) -> Val {
    d.call(name, a).unwrap_or_else(|| panic!("{name} not handled")).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn list(v: &Val) -> Vec<Val> {
    if let Val::List(l) = v { l.clone() } else { vec![] }
}
fn num(v: &Val) -> f64 {
    if let Val::Num(x) = v { *x } else { f64::NAN }
}
fn st(v: &Val) -> String {
    match v {
        Val::Str(x) => x.clone(),
        Val::Num(x) => format!("{x}"),
        _ => String::new(),
    }
}

#[test]
fn resample_to_wavetable_and_bounce_to_sampler_through_the_builtins() {
    let dir = std::env::temp_dir().join(format!("lsx_synth_block_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = Daw::new();
    let growl = num(&call(&mut d, "daw_track_add", &[s("growl"), s("")]));
    let prism = num(&call(&mut d, "daw_track_add", &[s("prism"), s("")]));
    // a note of the Prism track becomes the Growl's table
    let r = list(&call(&mut d, "daw_wt_from_note", &[n(growl), n(prism), n(45.0), n(16.0)]));
    assert_eq!(num(&r[0]), 1.0, "{r:?}");
    assert_eq!(num(&r[1]), 16.0);
    assert!((num(&r[2]) / 110.0 - 1.0).abs() < 0.02, "pitch {}", num(&r[2]));
    assert!(std::path::Path::new(&st(&r[3])).is_file());
    assert_eq!(num(&call(&mut d, "daw_inst_get", &[n(growl), s("user")])), 1.0);
    let info = list(&call(&mut d, "daw_wt_info", &[n(growl)]));
    assert_eq!(num(&info[1]), 16.0);
    // a clip of the Prism track (rendered alone) into the Prism itself
    let c = num(&call(&mut d, "daw_clip_add_midi", &[n(prism), n(4.0), n(4.0)]));
    call(&mut d, "daw_clip_set_notes", &[n(prism), n(c), Val::List(vec![n(0.0), n(3.5), n(52.0), n(0.9), n(0.0), n(0.5), n(0.5)])]);
    let r = list(&call(&mut d, "daw_wt_from_clip", &[n(prism), n(prism), n(c), n(8.0)]));
    assert_eq!(num(&r[0]), 1.0, "{r:?}");
    assert!((num(&r[2]) / 164.81 - 1.0).abs() < 0.02, "pitch {}", num(&r[2]));
    assert_eq!(num(&call(&mut d, "daw_inst_get", &[n(prism), s("a user")])), 1.0);
    // a sample file: a known wave
    let wav = dir.join("saw.wav").to_string_lossy().into_owned();
    let spec = hound::WavSpec { channels: 1, sample_rate: 44100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(&wav, spec).unwrap();
    for i in 0..44100 {
        let ph = i as f32 * 98.0 / 44100.0;
        let x: f32 = (1..12).map(|h| (std::f32::consts::TAU * h as f32 * ph).sin() / h as f32).sum::<f32>() * 0.4;
        w.write_sample((x * 32000.0) as i16).unwrap();
    }
    w.finalize().unwrap();
    let r = list(&call(&mut d, "daw_wt_from_file", &[n(growl), s(&wav), n(4.0)]));
    assert_eq!(num(&r[0]), 1.0, "{r:?}");
    assert!((num(&r[2]) / 98.0 - 1.0).abs() < 0.01);
    // something that is not a table is refused, the slot keeps its table
    let bad = list(&call(&mut d, "daw_wt_load", &[n(growl), s(&wav)]));
    assert_eq!(num(&bad[0]), 0.0);
    assert_eq!(num(&list(&call(&mut d, "daw_wt_info", &[n(growl)]))[1]), 4.0);
    // the project keeps the table: saved (packed into the project folder) and opened again
    let proj = dir.join("p").join("song.lsx").to_string_lossy().into_owned();
    assert_eq!(num(&call(&mut d, "daw_project_save", &[s(&proj)])), 1.0);
    let mut d2 = Daw::new();
    assert_eq!(num(&call(&mut d2, "daw_project_load", &[s(&proj)])), 1.0);
    let info = list(&call(&mut d2, "daw_wt_info", &[n(growl)]));
    assert_eq!(num(&info[1]), 4.0, "{info:?}");
    assert!(st(&info[0]).replace('\\', "/").contains("/p/"), "the table lives in the project now: {info:?}");
    assert_eq!(num(&call(&mut d2, "daw_inst_get", &[n(growl), s("user")])), 1.0);
    // bounce to Sampler: a new Sampler track with the clip on one zone, rooted at its pitch, playing where the clip was
    let before = num(&call(&mut d, "daw_track_count", &[]));
    let t = num(&call(&mut d, "daw_wt_bounce_sampler", &[n(prism), n(c)]));
    assert_eq!(t, before);
    assert_eq!(st(&call(&mut d, "daw_track_kind", &[n(t)])), "sampler");
    let notes = list(&call(&mut d, "daw_clip_notes", &[n(t), n(0.0)]));
    assert!(notes.len() >= 4 && num(&notes[2]) == 52.0, "{notes:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

const SR: f32 = 48000.0;

/// microseconds of CPU per second of audio: a four-note chord held under a melody (as perf_bench plays)
fn play(inst: &mut dyn dsp::Instrument) -> f64 {
    let mut buf = vec![0f32; 512];
    let secs = 3.0;
    let blocks = (secs * SR as f64 / 256.0) as usize;
    let t0 = std::time::Instant::now();
    for b in 0..blocks {
        if b % 188 == 0 {
            for k in [48u8, 55, 60, 64] {
                inst.note_off(k);
                inst.note_on(k, 0.8);
            }
        }
        if b % 47 == 0 {
            let k = 67 + ((b / 47) % 7) as u8 * 2;
            inst.note_off(k.wrapping_sub(2));
            inst.note_on(k, 0.8);
        }
        buf.fill(0.0);
        inst.render(&mut buf);
    }
    t0.elapsed().as_secs_f64() * 1e6 / secs
}

#[test]
#[ignore]
fn cost() {
    flush_denormals();
    let rows: Vec<(&str, &str, Vec<(&str, f32)>)> = vec![
        ("growl", "mono (default)", vec![]),
        ("growl", "fm 0.5 (blend)", vec![("fm", 0.5)]),
        ("growl", "fm 1 (FM only)", vec![("fm", 1.0)]),
        ("growl", "poly 2", vec![("poly", 0.2)]),
        ("growl", "poly 4", vec![("poly", 0.6)]),
        ("growl", "poly 8", vec![("poly", 1.0)]),
        ("growl", "poly 4 + fm 1", vec![("poly", 0.6), ("fm", 1.0)]),
        ("growl", "lane div 1/8T", vec![("l1 div", 5.0 / 16.0)]),
        ("prism", "uni 7 (a uni 1)", vec![("a uni", 1.0)]),
        ("prism", "uni 7 + 7 (b on)", vec![("a uni", 1.0), ("b uni", 1.0), ("b lvl", 0.5)]),
        ("prism", "uni 12", vec![("a uni", 1.0), ("a xuni", 5.0 / 9.0)]),
        ("prism", "uni 16", vec![("a uni", 1.0), ("a xuni", 1.0)]),
        ("prism", "uni 16 + 16 (b on)", vec![("a uni", 1.0), ("a xuni", 1.0), ("b uni", 1.0), ("b xuni", 1.0), ("b lvl", 0.5)]),
        ("prism", "l2 -> l1 rate, vel -> reverb", vec![("x l2l1r", 0.8), ("x velrvw", 0.7)]),
    ];
    // a user table for the table rows
    let n = dsp::wavetable::N;
    let frames: Vec<Vec<f32>> = (0..32).map(|f| (0..n).map(|i| (1..(2 + f)).map(|h| (std::f32::consts::TAU * (h * i) as f32 / n as f32).sin() / h as f32).sum()).collect()).collect();
    let path = std::env::temp_dir().join(format!("lsx_cost_{}.wav", std::process::id())).to_string_lossy().into_owned();
    dsp::wavetable::save(&frames, &path).unwrap();
    let mut all = rows.iter().map(|(k, l, p)| (k.to_string(), l.to_string(), p.iter().map(|(a, b)| (a.to_string(), *b)).collect::<Vec<_>>(), false)).collect::<Vec<_>>();
    all.push(("growl".into(), "user table".into(), vec![], true));
    all.push(("prism".into(), "user table (a)".into(), vec![], true));
    for (kind, label, params, table) in all {
        let mut best = f64::MAX;
        for _ in 0..5 {
            let mut i = dsp::make_instrument(&kind, SR).unwrap();
            for (k, v) in &params {
                i.set_param(k, *v);
            }
            if table {
                i.command("wt_load", &[], &path);
            }
            best = best.min(play(i.as_mut()));
        }
        eprintln!("{:>6} {:<30} {:>8.0} us/sec  {:>5.2} % of a core", kind, label, best, best / 1e4);
    }
    let _ = std::fs::remove_file(&path);
}
