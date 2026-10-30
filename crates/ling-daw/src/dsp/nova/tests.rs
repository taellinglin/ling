//! Ling Nova's tests: every preset renders sane audio and ends, band-limiting holds at high notes, unison and warps do what they
//! say, the matrix routes, the per-note hooks work, and it is fast enough.
use super::*;
use crate::dsp::wavetable::fft;

const SR: f32 = 48000.0;

fn play(n: &mut Nova, keys: &[u8], secs: f32, hold: f32) -> Vec<f32> {
    let total = (SR * secs) as usize;
    let mut out = vec![0f32; total * 2];
    for &k in keys {
        n.note_on(k, 0.9);
    }
    let off = (hold * SR) as usize * 2;
    let mut i = 0;
    while i < out.len() {
        if i >= off && i < off + 512 {
            for &k in keys {
                n.note_off(k);
            }
        }
        let k = 512.min(out.len() - i);
        n.render(&mut out[i..i + k]);
        i += k;
    }
    out
}
fn rms(v: &[f32]) -> f32 {
    (v.iter().map(|x| x * x).sum::<f32>() / v.len().max(1) as f32).sqrt()
}
fn peak(v: &[f32]) -> f32 {
    v.iter().fold(0.0f32, |m, x| m.max(x.abs()))
}
fn left(v: &[f32]) -> Vec<f32> {
    v.chunks(2).map(|c| c[0]).collect()
}
/// the power spectrum (Hann window) of 8192 samples from `at`
fn spectrum(x: &[f32], at: usize) -> Vec<f32> {
    let n = 8192;
    let mut re: Vec<f32> = (0..n).map(|i| x.get(at + i).copied().unwrap_or(0.0) * (0.5 - 0.5 * (TAU * i as f32 / n as f32).cos())).collect();
    let mut im = vec![0.0f32; n];
    fft(&mut re, &mut im, false);
    (0..n / 2).map(|k| re[k] * re[k] + im[k] * im[k]).collect()
}
fn band(sp: &[f32], lo: f32, hi: f32) -> f32 {
    let bin = SR / 8192.0;
    sp.iter().enumerate().filter(|(k, _)| *k as f32 * bin >= lo && (*k as f32 * bin) < hi).map(|(_, p)| p).sum()
}
fn centroid(sp: &[f32]) -> f32 {
    let bin = SR / 8192.0;
    let (mut a, mut b) = (0.0f32, 0.0f32);
    for (k, p) in sp.iter().enumerate().skip(1) {
        a += k as f32 * bin * p;
        b += p;
    }
    a / b.max(1e-12)
}
/// a plain, dry Nova: oscillator A only, no filter, no effects, no sub
fn plain() -> Nova {
    let mut n = Nova::new(SR);
    n.set_param("f1 on", 0.0);
    n.set_param("e1 r", 0.3);
    n
}
fn sel_v(name: &str, choice: &str) -> f32 {
    let l = layout();
    let s = &l.specs[l.index[name]];
    let i = s.opts.iter().position(|o| *o == choice).unwrap();
    i as f32 / (s.opts.len() - 1) as f32
}

#[test]
fn the_layout_is_consistent_and_every_preset_parses_cleanly() {
    let l = layout();
    assert!(l.n() > 400);
    let mut names: Vec<&str> = l.specs.iter().map(|s| s.name).collect();
    names.sort();
    names.dedup();
    assert_eq!(names.len(), l.n(), "duplicate parameter names");
    assert!(l.ndst() > 100, "{} destinations", l.ndst());
    let mut errs = vec![];
    let _ = presets::parse_checked(include_str!("../../nova_presets.txt"), &mut errs);
    assert!(errs.is_empty(), "{errs:#?}");
}

#[test]
fn every_preset_has_a_description_usage_notes_and_its_macros() {
    let all = presets::all();
    assert!(all.len() >= 100, "{} presets", all.len());
    for c in ["Bass", "Lead", "Chords", "Pluck", "Pad", "FX"] {
        assert!(all.iter().any(|p| p.cat == c), "{c}");
    }
    for fam in ["growl", "riddim", "colour", "neuro", "reese", "wobble", "fm", "talk", "sub", "mid", "screech", "supersaw", "brass"] {
        assert!(all.iter().any(|p| p.usage.family == fam), "family {fam}");
    }
    let l = layout();
    for p in all {
        let u = &p.usage;
        assert!(!p.desc.is_empty(), "{}: no description", p.name);
        assert!(!u.role.is_empty() && !u.play.is_empty() && !u.notes.is_empty() && !u.mix.is_empty() && !u.pattern.is_empty(), "{}: usage", p.name);
        assert!(!u.genres.is_empty() && u.lo < u.hi && !u.style.is_empty(), "{}", p.name);
        assert!(!u.auto.is_empty(), "{}: no macro moves", p.name);
        // the first macro is wired to something
        let m1 = sel_v("x1 src", "m1");
        let wired = l.mat.iter().any(|m| (p.vals[m.src] - m1).abs() < 1e-4 && (p.vals[m.amt] - 0.5).abs() > 0.002);
        assert!(wired, "{}: macro 1 moves nothing", p.name);
    }
}

#[test]
fn every_preset_renders_finite_sane_audio_and_its_voices_end() {
    let mut bad: Vec<String> = vec![];
    for p in presets::all() {
        let mut n = Nova::new(SR);
        n.set_tempo(140.0);
        n.load_preset(&p.name);
        let key = ((p.usage.lo as u16 + p.usage.hi as u16) / 2) as u8;
        let keys: Vec<u8> = if p.usage.style == "chords" { vec![key, key + 4, key + 7] } else { vec![key] };
        let o = play(&mut n, &keys, 2.0, 1.0);
        let (pk, r) = (peak(&o), rms(&o));
        let l = left(&o);
        let dc = l.iter().sum::<f32>() / l.len() as f32;
        if !o.iter().all(|x| x.is_finite()) || !(pk > 0.02 && pk <= 1.0) || r < 0.004 || dc.abs() > 0.02 {
            bad.push(format!("{}: peak {pk} rms {r} dc {dc}", p.name));
        }
        // after the release, the voices are free and the tails die away
        let mut t = 0.0;
        let mut ch = vec![0f32; 1024];
        while n.voices.iter().any(|v| v.on) && t < 12.0 {
            n.render(&mut ch);
            t += 512.0 / SR;
        }
        if n.voices.iter().any(|v| v.on) {
            bad.push(format!("{}: a voice hangs", p.name));
        }
        let mut z = vec![0f32; (SR * 4.0) as usize * 2];
        for ch in z.chunks_mut(512) {
            n.render(ch);
        }
        let late = rms(&z[z.len() - 9600..]);
        if late > 2e-3 {
            bad.push(format!("{}: still sounding {late}", p.name));
        }
    }
    assert!(bad.is_empty(), "{bad:#?}");
}

#[test]
fn high_notes_stay_band_limited() {
    // aliases land between the harmonics of a periodic tone: the energy away from the harmonics, against all of it
    let inharm = |sp: &[f32], f0: f32| {
        let bin = SR / 8192.0;
        let (mut off, mut all) = (0.0f32, 0.0f32);
        for (k, p) in sp.iter().enumerate().skip(2) {
            let f = k as f32 * bin;
            let h = (f / f0).round().max(1.0);
            all += p;
            if (f - h * f0).abs() > bin * 3.0 {
                off += p;
            }
        }
        off / all.max(1e-12)
    };
    // a saw at C9 (8.4 kHz): its third harmonic would fold back without the mip levels
    let mut n = plain();
    n.set_param("a pos", 2.0 / 3.0);
    let o = play(&mut n, &[120], 0.5, 0.5);
    let sp = spectrum(&left(&o), 4000);
    let a = inharm(&sp, midi_hz(120.0));
    assert!(a < 1e-3, "plain saw: {a}");
    // a hard sync warp, which runs oversampled with a soft restart
    for key in [84u8, 96] {
        let mut n = plain();
        n.set_param("a pos", 2.0 / 3.0);
        n.set_param("a w1 type", sel_v("a w1 type", "sync"));
        n.set_param("a w1 amt", 0.6);
        let o = play(&mut n, &[key], 0.5, 0.5);
        let sp = spectrum(&left(&o), 4000);
        let a = inharm(&sp, midi_hz(key as f32));
        assert!(a < 0.01, "sync at {key}: {a}");
    }
    // a full unison stack up high: nothing past the top of the band
    let mut n = plain();
    n.set_param("a pos", 2.0 / 3.0);
    n.set_param("a uni", 1.0);
    n.set_param("a det", 0.6);
    let o = play(&mut n, &[115], 0.5, 0.5);
    let sp = spectrum(&left(&o), 4000);
    let air = band(&sp, 22000.0, 24000.0) / band(&sp, 20.0, 24000.0);
    assert!(air < 1e-3, "unison: {air}");
}

#[test]
fn unison_widens_and_warps_change_the_spectrum() {
    let side = |o: &[f32]| rms(&o.chunks(2).map(|c| c[0] - c[1]).collect::<Vec<_>>()) / rms(o).max(1e-9);
    let mut n = plain();
    n.set_param("a pos", 2.0 / 3.0);
    let one = play(&mut n, &[48], 0.6, 0.6);
    let mut n = plain();
    n.set_param("a pos", 2.0 / 3.0);
    n.set_param("a uni", 8.0 / 15.0);
    n.set_param("a det", 0.5);
    n.set_param("a width", 1.0);
    let many = play(&mut n, &[48], 0.6, 0.6);
    assert!(side(&one) < 0.01, "one voice is mono: {}", side(&one));
    assert!(side(&many) > 0.3, "unison is wide: {}", side(&many));
    // the detune spreads the energy around each harmonic: less of it on the exact harmonic bins
    let sp1 = spectrum(&left(&one), 8000);
    let sp2 = spectrum(&left(&many), 8000);
    let f0 = midi_hz(48.0);
    let near = |sp: &[f32]| band(sp, f0 * 4.0 - 3.0, f0 * 4.0 + 3.0) / band(sp, f0 * 3.5, f0 * 4.5).max(1e-12);
    assert!(near(&sp2) < near(&sp1) * 0.8, "{} vs {}", near(&sp2), near(&sp1));
    // a sine with a fold warp, and with a sync warp, is brighter than the sine
    let c = |w: &str, a: f32| {
        let mut n = plain();
        n.set_param("a pos", 0.0);
        if !w.is_empty() {
            n.set_param("a w1 type", sel_v("a w1 type", w));
            n.set_param("a w1 amt", a);
        }
        centroid(&spectrum(&left(&play(&mut n, &[48], 0.5, 0.5)), 6000))
    };
    let base = c("", 0.0);
    for w in ["fold", "sync", "bend+", "pwm", "quantize", "crush", "asym"] {
        let x = c(w, if matches!(w, "quantize" | "crush") { 0.9 } else { 0.6 });
        let need = if matches!(w, "quantize" | "crush" | "asym") { 1.15 } else { 1.5 };
        assert!(x > base * need, "{w}: centroid {x} vs sine {base}");
    }
    // FM from B: sidebands
    let mut n = plain();
    n.set_param("a pos", 0.0);
    n.set_param("b on", 1.0);
    n.set_param("b level", 0.0);
    n.set_param("b table", 0.0);
    n.set_param("b pos", 0.0);
    n.set_param("a w1 type", sel_v("a w1 type", "fm"));
    n.set_param("a w1 amt", 0.5);
    let fm = centroid(&spectrum(&left(&play(&mut n, &[48], 0.5, 0.5)), 6000));
    assert!(fm > base * 1.5, "fm {fm} vs {base}");
}

#[test]
fn the_matrix_routes_lfos_and_macros() {
    let l = layout();
    // a macro on the filter cutoff: closed is darker than open
    let bright = |m1: f32| {
        let mut n = Nova::new(SR);
        n.set_param("f1 type", sel_v("f1 type", "lp 24"));
        n.set_param("f1 cut", 0.3);
        n.set_param("a pos", 2.0 / 3.0);
        n.set_param("x1 src", sel_v("x1 src", "m1"));
        n.set_param("x1 dst", sel_v("x1 dst", "f1 cut"));
        n.set_param("x1 amt", 1.0);
        n.set_param("m1", m1);
        centroid(&spectrum(&left(&play(&mut n, &[48], 0.5, 0.5)), 6000))
    };
    assert!(bright(1.0) > bright(0.0) * 2.0, "{} {}", bright(1.0), bright(0.0));
    // an LFO on the amp at 1/4 notes, 120 bpm: the level moves at 2 Hz
    let mut n = plain();
    n.set_tempo(120.0);
    n.set_param("l1 div", sel_v("l1 div", "1/4"));
    n.set_param("x1 src", sel_v("x1 src", "l1"));
    n.set_param("x1 dst", sel_v("x1 dst", "amp"));
    n.set_param("x1 amt", 0.0);
    let o = left(&play(&mut n, &[60], 1.0, 1.0));
    let blk: Vec<f32> = o.chunks(1200).map(rms).collect();
    let (mx, mn) = (blk[4..].iter().cloned().fold(0.0, f32::max), blk[4..].iter().cloned().fold(1.0, f32::min));
    assert!(mn < mx * 0.25, "the LFO ducks the level: {mn} {mx}");
    // the aux source scales a slot: with the macro at 0 the LFO does nothing
    n.set_param("x1 aux", sel_v("x1 aux", "m2"));
    n.set_param("m2", 0.0);
    n.all_off();
    let o = left(&play(&mut n, &[60], 1.0, 1.0));
    let blk: Vec<f32> = o.chunks(1200).map(rms).collect();
    let (mx, mn) = (blk[4..].iter().cloned().fold(0.0, f32::max), blk[4..].iter().cloned().fold(1.0, f32::min));
    assert!(mn > mx * 0.8, "aux at 0 holds it still: {mn} {mx}");
    // a global destination: a macro opens the reverb's mix
    let mut n = plain();
    n.set_param("rev on", 1.0);
    n.set_param("rev mix", 0.0);
    n.set_param("x1 src", sel_v("x1 src", "m4"));
    n.set_param("x1 dst", sel_v("x1 dst", "rev mix"));
    n.set_param("x1 amt", 1.0);
    n.set_param("m4", 1.0);
    let o = play(&mut n, &[60], 1.5, 0.2);
    let tail = rms(&o[(SR * 0.6) as usize * 2..]);
    assert!(tail > 1e-3, "the reverb tail rings: {tail}");
    assert!(l.dest_global[l.dst_of[l.fx[10].mix]]);
}

#[test]
fn per_note_pitch_pressure_and_timbre_hooks_work() {
    let zc = |v: &[f32]| v.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    let mut n = plain();
    n.set_param("a pos", 0.0);
    n.note_on(57, 0.9);
    let mut a = vec![0f32; 9600];
    n.render(&mut a);
    n.note_pitch(57, 12.0);
    let mut warm = vec![0f32; 9600];
    n.render(&mut warm);
    let mut b = vec![0f32; 9600];
    n.render(&mut b);
    let (za, zb) = (zc(&left(&a)), zc(&left(&b)));
    assert!((zb as f32 / za as f32 - 2.0).abs() < 0.1, "an octave up: {za} -> {zb}");
    // pressure: louder
    let mut n = plain();
    n.note_on(57, 0.9);
    let mut a = vec![0f32; 9600];
    n.render(&mut a);
    n.note_pressure(57, 1.0);
    n.render(&mut vec![0f32; 4800]);
    let mut b = vec![0f32; 9600];
    n.render(&mut b);
    assert!(rms(&b) > rms(&a) * 1.15, "{} {}", rms(&b), rms(&a));
    // timbre as a matrix source on the cutoff
    let mut n = Nova::new(SR);
    n.set_param("a pos", 2.0 / 3.0);
    n.set_param("f1 cut", 0.3);
    n.set_param("x1 src", sel_v("x1 src", "timbre"));
    n.set_param("x1 dst", sel_v("x1 dst", "f1 cut"));
    n.set_param("x1 amt", 1.0);
    n.note_on(48, 0.9);
    n.note_timbre(48, 0.0);
    let mut a = vec![0f32; 16384];
    n.render(&mut a);
    n.note_timbre(48, 1.0);
    n.render(&mut vec![0f32; 4800]);
    let mut b = vec![0f32; 16384];
    n.render(&mut b);
    assert!(centroid(&spectrum(&left(&b), 0)) > centroid(&spectrum(&left(&a), 0)) * 1.5);
}

#[test]
fn mono_legato_glides_and_polyphony_steals() {
    let zc = |v: &[f32]| v.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count();
    let mut n = plain();
    n.set_param("a pos", 0.0);
    n.set_param("voice mode", sel_v("voice mode", "legato"));
    n.set_param("porta", 0.5);
    n.note_on(48, 0.9);
    n.render(&mut vec![0f32; 9600]);
    let env_before = n.voices[0].env[0].stage;
    n.note_on(60, 0.9);
    // legato: no new attack, the same voice glides
    assert_eq!(n.voices.iter().filter(|v| v.on).count(), 1);
    assert_eq!(n.voices[0].env[0].stage, env_before);
    let mut a = vec![0f32; 4800];
    n.render(&mut a);
    let mut b = vec![0f32; 48000];
    n.render(&mut b);
    let za = zc(&left(&a)) as f32 / 0.05;
    let zb = zc(&left(&b[b.len() - 9600..])) as f32 / 0.1;
    assert!(za < zb * 0.85, "it glides up: {za} Hz then {zb} Hz");
    // back to the held key on release
    n.note_off(60);
    assert_eq!(n.voices[0].key, 48);
    // poly with a limit of 4: a fifth note steals, never more than 4 sound for long
    let mut n = plain();
    n.set_param("voices", 3.0 / 15.0);
    for k in [48, 52, 55, 59, 62] {
        n.note_on(k, 0.8);
    }
    n.render(&mut vec![0f32; 4800]);
    assert_eq!(n.voices.iter().filter(|v| v.on).count(), 4);
}

#[test]
fn state_round_trips_and_the_plots_have_their_lengths() {
    let mut a = Nova::new(SR);
    a.load_preset("Yoi Talker");
    let st = a.state();
    assert!(st.len() < 200, "{} lines", st.len());
    let mut b = Nova::new(SR);
    b.load_state(&st);
    assert_eq!(a.values(), b.values());
    for w in ["wave a", "wave b", "env1", "env2", "lfo1", "lfo4", "filter1", "filter2"] {
        let p = a.plot(w, 64);
        assert_eq!(p.len(), 64, "{w}");
        assert!(p.iter().all(|x| x.is_finite() && *x >= 0.0 && *x <= 1.0), "{w}");
    }
    assert_eq!(a.plot("stack a", 256).len(), 256);
    // a low-pass response falls
    let mut n = Nova::new(SR);
    n.set_param("f1 type", sel_v("f1 type", "lp 24"));
    n.set_param("f1 cut", 0.4);
    let r = n.plot("filter1", 40);
    assert!(r[2] > r[38] + 0.3, "{r:?}");
}

#[test]
fn the_effect_rack_reorders_and_every_effect_stays_finite() {
    let l = layout();
    let mut n = plain();
    for (k, _) in super::layout::FX.iter().enumerate() {
        n.set_param(l.specs[l.fx[k].on].name, 1.0);
    }
    n.set_param("dist drive", 0.8);
    let o = play(&mut n, &[48, 55], 1.0, 0.5);
    assert!(o.iter().all(|x| x.is_finite()) && peak(&o) <= 1.0 && peak(&o) > 0.01);
    let before = n.command("fx_order", &[], "");
    n.command("fx_move", &[0.0, 1.0], "");
    let after = n.command("fx_order", &[], "");
    assert_eq!(before[0], 0.0);
    assert_eq!(after[1], 0.0, "{after:?}");
    for mode in super::layout::DIST_MODES {
        let mut n = plain();
        n.set_param("dist on", 1.0);
        n.set_param("dist mode", sel_v("dist mode", mode));
        n.set_param("dist drive", 0.9);
        let o = play(&mut n, &[40], 0.4, 0.3);
        assert!(o.iter().all(|x| x.is_finite()) && peak(&o) > 0.01, "{mode}");
    }
}

#[test]
fn every_filter_type_passes_sound() {
    for (i, f) in FTYPES.iter().enumerate() {
        let mut n = Nova::new(SR);
        n.set_param("a pos", 2.0 / 3.0);
        n.set_param("f1 type", i as f32 / (FTYPES.len() - 1) as f32);
        n.set_param("f1 cut", 0.5);
        n.set_param("f1 res", 0.7);
        n.set_param("f1 drive", 0.3);
        let o = play(&mut n, &[45], 0.5, 0.4);
        assert!(o.iter().all(|x| x.is_finite()), "{f}");
        assert!(rms(&o) > 0.003 && peak(&o) <= 1.0, "{f}: {}", rms(&o));
    }
}

#[test]
fn eight_voices_of_sixteen_unison_render_faster_than_real_time() {
    let mut n = Nova::new(SR);
    n.set_param("a uni", 1.0);
    n.set_param("a det", 0.4);
    n.set_param("f1 type", sel_v("f1 type", "ladder"));
    n.set_param("voices", 1.0);
    for k in 0..8 {
        n.note_on(48 + k * 3, 0.8);
    }
    let secs = 2.0;
    let mut buf = vec![0f32; 512];
    let t = std::time::Instant::now();
    for _ in 0..((SR * secs) as usize / 256) {
        n.render(&mut buf);
    }
    let took = t.elapsed().as_secs_f32();
    let mut n3 = Nova::new(SR);
    for o in ["a", "b", "c"] {
        n3.set_param(&format!("{o} on"), 1.0);
        n3.set_param(&format!("{o} uni"), 1.0);
        n3.set_param(&format!("{o} w1 type"), sel_v("a w1 type", "sync"));
        n3.set_param(&format!("{o} w1 amt"), 0.3);
    }
    n3.set_param("voices", 1.0);
    for k in 0..8 {
        n3.note_on(48 + k * 3, 0.8);
    }
    let t3 = std::time::Instant::now();
    for _ in 0..((SR * secs) as usize / 256) {
        n3.render(&mut buf);
    }
    let took3 = t3.elapsed().as_secs_f32();
    println!("nova cpu: 8 voices x 16 unison, one oscillator: {:.1}% of real time; three oscillators with sync warps (2x oversampled): {:.1}%", took / secs * 100.0, took3 / secs * 100.0);
    if !cfg!(debug_assertions) {
        assert!(took < secs, "{took} s for {secs} s");
    }
}

/// not a test: one patch file per preset into $NOVA_OUT (instruments/Nova), and the guide into $NOVA_GUIDE (a .md path)
#[test]
#[ignore]
fn dump_preset_patches() {
    if let Ok(dir) = std::env::var("NOVA_OUT") {
        std::fs::create_dir_all(&dir).unwrap();
        for p in presets::all() {
            let mut n = Nova::new(44100.0);
            n.load_preset(&p.name);
            let mut body = String::new();
            for (k, v) in n.state() {
                body.push_str(&format!("{}={:.4}\n", k.replace(' ', "_"), v));
            }
            let text = format!("# Ling Nova patch - edit freely; load from the Instrument tab\n# {}\nname = \"{}\"\ntype = nova\n{}", p.desc, p.name, body);
            std::fs::write(std::path::Path::new(&dir).join(format!("Nova_{}.ling", p.name.replace(' ', "_"))), text).unwrap();
        }
    }
    if let Ok(path) = std::env::var("NOVA_GUIDE") {
        std::fs::write(path, presets::guide_markdown()).unwrap();
    }
}

/// not a test: the demo renders (`NOVA_RENDERS=<dir>`): a phrase per preset, and a bass-music drop riff that trades growl, riddim
/// and colour-bass patches (the way the generators switch them), all synthesised here
#[test]
#[ignore]
fn render_demos() {
    let Ok(dir) = std::env::var("NOVA_RENDERS") else { return };
    std::fs::create_dir_all(&dir).unwrap();
    let sr = 48000.0f32;
    let save = |name: &str, data: &[f32]| {
        let pk = data.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-9);
        let g = if pk > 0.89 { 0.89 / pk } else { 1.0 };
        let v: Vec<f32> = data.iter().map(|x| x * g).collect();
        let path = std::path::Path::new(&dir).join(format!("{name}.wav"));
        crate::export::write_audio(&path.to_string_lossy(), &v, sr as u32, "wav", 16, 0.0).unwrap();
    };
    // (start beat, length beats, keys)
    type Ev = (f32, f32, Vec<u8>);
    let render = |n: &mut Nova, evs: &[Ev], bpm: f32, beats: f32, macros: &dyn Fn(f32, &mut Nova)| -> Vec<f32> {
        n.set_tempo(bpm as f64);
        let spb = 60.0 / bpm * sr;
        let total = (beats * spb) as usize + (sr * 2.0) as usize;
        let mut out = vec![0f32; total * 2];
        let mut pos = 0usize;
        let block = 256usize;
        while pos < total {
            let b0 = pos as f32 / spb;
            let b1 = (pos + block) as f32 / spb;
            macros(b0, n);
            for (s, l, ks) in evs {
                if *s + *l >= b0 && *s + *l < b1 {
                    for k in ks {
                        n.note_off(*k);
                    }
                }
            }
            for (s, _, ks) in evs {
                if *s >= b0 && *s < b1 {
                    for k in ks {
                        n.note_on(*k, 0.9);
                    }
                }
            }
            let k = block.min(total - pos);
            n.render(&mut out[pos * 2..(pos + k) * 2]);
            pos += k;
        }
        out
    };
    let none = |_: f32, _: &mut Nova| {};
    for p in presets::all() {
        let u = &p.usage;
        let mid = ((u.lo as u16 + u.hi as u16) / 2) as u8;
        let root = (mid - (mid % 12) + 4).clamp(u.lo, u.hi);
        let evs: Vec<Ev> = match u.style.as_str() {
            "chords" => vec![(0.0, 1.5, vec![root, root + 3, root + 7, root + 10]), (2.0, 1.5, vec![root - 4, root, root + 3, root + 7]), (4.0, 0.5, vec![root - 2, root + 2, root + 5, root + 9]), (4.75, 0.5, vec![root - 2, root + 2, root + 5, root + 9]), (5.5, 2.0, vec![root - 2, root + 2, root + 5, root + 9])],
            "stab16" | "stab8" => {
                let st = if u.style == "stab16" { 0.25 } else { 0.5 };
                let mut v: Vec<Ev> = vec![(0.0, 1.5, vec![root])];
                let mut b = 2.0;
                while b < 7.0 {
                    v.push((b, st * 0.7, vec![if (b * 4.0) as i32 % 5 == 3 { root + 7 } else { root }]));
                    b += st;
                }
                v
            }
            "pluck" => (0..16).map(|i| (i as f32 * 0.5, 0.3, vec![root + [0, 3, 7, 10, 12, 10, 7, 3][i % 8]])).collect(),
            "riser" => vec![(0.0, 7.0, vec![root])],
            _ => vec![(0.0, 3.0, vec![root]), (3.5, 0.5, vec![root + 3]), (4.0, 3.5, vec![root + 7])],
        };
        let mut n = Nova::new(sr);
        n.load_preset(&p.name);
        let o = if u.style == "riser" && p.name != "Impact Drop" {
            render(&mut n, &evs, 140.0, 8.0, &|b: f32, n: &mut Nova| n.set_param("m1", (b / 7.0).min(1.0)))
        } else {
            render(&mut n, &evs, 140.0, 8.0, &none)
        };
        save(&format!("preset_{}", p.name.replace(' ', "_")), &o);
    }
    // the drop: 8 bars at 140, a different patch every bar, the growl macro moving on 1/8 notes
    let bpm = 140.0;
    let order = ["Yoi Talker", "Hollow Riddim", "Yoi Talker", "Prism Colour Bass", "Grind Mouth", "Square Stomp", "Wet Gargle", "Prism Colour Bass"];
    let spb = 60.0 / bpm * sr;
    let total = (32.0 * spb) as usize + (sr * 2.0) as usize;
    let mut mix = vec![0f32; total * 2];
    for (bar, name) in order.iter().enumerate() {
        let b0 = bar as f32 * 4.0;
        let root = 28u8; // E1
        let p = presets::find(name).unwrap();
        let evs: Vec<Ev> = match p.usage.style.as_str() {
            "chords" => vec![(0.0, 0.4, vec![40, 47, 52, 55]), (0.75, 0.4, vec![40, 47, 52, 55]), (1.5, 0.9, vec![38, 45, 50, 54]), (2.5, 0.4, vec![43, 50, 55, 59]), (3.0, 0.9, vec![45, 52, 57, 60])],
            "stab16" => (0..14).filter(|i| i % 4 != 2).map(|i| (i as f32 * 0.25, 0.18, vec![root])).collect(),
            _ => vec![(0.0, 1.4, vec![root]), (1.5, 0.4, vec![root]), (2.0, 0.2, vec![root + 12]), (2.5, 0.9, vec![root]), (3.5, 0.4, vec![root + 7])],
        };
        let mut n = Nova::new(sr);
        n.load_preset(name);
        let o = render(&mut n, &evs, bpm, 4.0, &|b: f32, n: &mut Nova| {
            // the drop's macro moves: the growl on 1/8 notes, the grit up
            let step = ((b * 2.0).floor() as i32 % 4) as usize;
            n.set_param("m2", [0.9, 0.4, 0.7, 0.2][step]);
            n.set_param("m3", 0.75);
        });
        let s0 = (b0 * spb) as usize * 2;
        for (i, x) in o.iter().enumerate() {
            if s0 + i < mix.len() {
                mix[s0 + i] += x;
            }
        }
    }
    save("demo_drop_growl_riddim_colour_switching", &mix);
}

/// not a test: prints the level of a preset with parts of it switched off (`NOVA_PROBE=<preset>`)
#[test]
#[ignore]
fn probe_preset_levels() {
    let Ok(name) = std::env::var("NOVA_PROBE") else { return };
    let key: u8 = std::env::var("NOVA_KEY").ok().and_then(|k| k.parse().ok()).unwrap_or(64);
    for off in ["", "f1 on", "dist on", "ott on", "img on", "a w1 amt", "rev on", "dly on", "hyp on"] {
        let mut n = Nova::new(SR);
        n.load_preset(&name);
        if !off.is_empty() {
            n.set_param(off, 0.0);
        }
        let o = play(&mut n, &[key], 1.0, 1.0);
        println!("{name} without {off:10}: rms {:.1} dB peak {:.2}", 20.0 * rms(&o).max(1e-9).log10(), peak(&o));
    }
}

/// not a test: each preset's loudness (as presetinfo measures it) and its level knob, tab-separated (`NOVA_LEVELS=<file>`), so
/// the preset file's levels can be set to sit together
#[test]
#[ignore]
fn measure_preset_levels() {
    let Ok(out) = std::env::var("NOVA_LEVELS") else { return };
    let mut lines = vec![];
    for p in presets::all() {
        let mut n = Nova::new(44100.0);
        n.load_preset(&p.name);
        let f = crate::presetinfo::analyze(&mut n, 44100.0);
        lines.push(format!("{}\t{:.2}\t{:.4}", p.name, f.rms_db, n.value("level").unwrap_or(0.0)));
    }
    std::fs::write(out, lines.join("\n")).unwrap();
}
