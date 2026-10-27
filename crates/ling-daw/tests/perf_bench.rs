//! What the instruments and effects cost: microseconds to render one second of audio (48 kHz, 256-frame blocks) for every patch of
//! every instrument (a busy part: a four-note chord held under a melody, so 5+ voices sound) and every effect on a full-range
//! signal. Run: `cargo test --release -p ling-daw --test perf_bench -- --ignored --nocapture` (LSX_ROOT = the LingStationX folder).
//! `share %` is the share of one core one second of it takes: a Raspberry Pi 5 core is roughly 3-4x slower than a desktop one.
use ling_daw::*;

const SR: f32 = 48000.0;
const SECS: f64 = 4.0;

fn play(inst: &mut dyn dsp::Instrument) -> f64 {
    let mut buf = vec![0f32; 512];
    let blocks = (SECS * SR as f64 / 256.0) as usize;
    let t0 = std::time::Instant::now();
    for b in 0..blocks {
        // a chord every second, a melody note every quarter second
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
    t0.elapsed().as_secs_f64() * 1e6 / SECS
}

#[test]
#[ignore]
fn what_everything_costs() {
    let root = std::env::var("LSX_ROOT").unwrap_or_else(|_| concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX").into());
    // as the audio thread runs: denormals flushed
    flush_denormals();
    let _ = std::env::set_current_dir(&root);
    let mut rows: Vec<(f64, String)> = vec![];
    // BENCH_ONLY=word measures only the rows whose name has it
    let only = std::env::var("BENCH_ONLY").unwrap_or_default();
    let want = |name: &str| only.is_empty() || name.contains(only.as_str());
    for (kind, dir) in [("synth", ""), ("prism", "instruments/Prism"), ("growl", "instruments/Growl"), ("pluck", ""), ("chip", "instruments/Chip"), ("voice", ""), ("drums", "")] {
        if !want(kind) && !want("fx") && only.starts_with("fx") {
            continue;
        }
        if let Some(mut i) = dsp::make_instrument(kind, SR).filter(|_| want(&format!("{kind} (default)"))) {
            rows.push((play(i.as_mut()), format!("{kind} (default)")));
        }
        if dir.is_empty() {
            continue;
        }
        let Ok(rd) = std::fs::read_dir(dir) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "ling").unwrap_or(false)).collect();
        files.sort();
        for f in files {
            let label = format!("{kind} {}", f.file_stem().unwrap().to_string_lossy());
            if !want(&label) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&f) else { continue };
            let p = patch::parse(&text);
            let Some(mut i) = patch::build(&p, SR) else { continue };
            rows.push((play(i.as_mut()), format!("{kind} {}", f.file_stem().unwrap().to_string_lossy())));
        }
    }
    // effects on noise shaped like music (a saw chord plus noise), every effect's defaults
    let mut src = vec![0f32; (SR as f64 * SECS) as usize * 2];
    let mut r = 1u32;
    for (i, x) in src.iter_mut().enumerate() {
        r = r.wrapping_mul(1664525).wrapping_add(1013904223);
        let t = (i / 2) as f32 / SR;
        *x = 0.2 * ((t * 110.0).fract() - 0.5) + 0.2 * ((t * 164.8).fract() - 0.5) + 0.05 * ((r >> 9) as f32 / 8388608.0 - 1.0);
    }
    for name in dsp::fx::EFFECT_NAMES {
        if !want(&format!("fx {name}")) {
            continue;
        }
        let Some(mut e) = EffectSlot::new(name, SR) else { continue };
        let mut buf = src.clone();
        let t0 = std::time::Instant::now();
        for c in buf.chunks_mut(512) {
            e.process(c);
        }
        rows.push((t0.elapsed().as_secs_f64() * 1e6 / SECS, format!("fx {name}")));
    }
    rows.sort_by(|a, b| b.0.total_cmp(&a.0));
    let total: f64 = rows.iter().map(|r| r.0).sum();
    eprintln!("{:>10} {:>8}  what", "us/sec", "share %");
    for (us, name) in rows.iter() {
        eprintln!("{:>10.0} {:>8.2}  {name}", us, us / 1e4);
    }
    eprintln!("{} measured, {:.0} us/sec in all", rows.len(), total);
}

/// What the "pro" parameters of Prism and Growl cost, one at a time, against the same patch with all of them neutral
/// (`cargo test --release -p ling-daw --test perf_bench pro_block -- --ignored --nocapture`).
#[test]
#[ignore]
fn pro_block_cost() {
    flush_denormals();
    let sets: [(&str, Vec<(&str, f32)>); 2] = [
        ("prism", vec![("fmodel", 1.0), ("lad drv", 0.5), ("vsat", 0.6), ("vel cut", 1.0), ("blend", 0.8), ("osc rst", 1.0), ("l1 sync", 0.5), ("clip", 0.6), ("warm", 0.6), ("air", 0.6), ("width", 0.8), ("mono lo", 0.5)]),
        ("growl", vec![("mb drv", 0.6), ("formant", 0.8), ("ott", 0.7), ("s width", 0.8), ("mono lo", 0.5), ("sub sat", 0.6), ("tilt", 0.6), ("air", 0.6)]),
    ];
    for (kind, params) in sets {
        let run = |set: &[(&str, f32)]| -> f64 {
            let mut i = dsp::make_instrument(kind, SR).unwrap();
            if kind == "prism" {
                i.set_param("a uni", 1.0);
                i.set_param("b lvl", 0.5);
                i.set_param("b uni", 0.5);
            }
            for (k, v) in set {
                i.set_param(k, *v);
            }
            play(i.as_mut())
        };
        // neutral and the variant alternate, the best of seven each (the machine is not quiet)
        let mut rows: Vec<(String, Vec<(&str, f32)>)> = params.iter().map(|p| (p.0.to_string(), vec![*p])).collect();
        rows.push(("all on".into(), params.clone()));
        for (name, set) in rows {
            let (mut off, mut on) = (f64::MAX, f64::MAX);
            for _ in 0..7 {
                off = off.min(run(&[]));
                on = on.min(run(&set));
            }
            eprintln!("  {kind} {name:<10} neutral {off:>8.0}  on {on:>8.0} us/sec  ({:+.1}%)", (on / off - 1.0) * 100.0);
        }
    }
}
