//! Not tests: the generator of the LingStationX sample pack (`LingStationX/samples`), every sound made here.
//!
//! `cargo test -p ling-daw --lib make_sample_pack -- --ignored --nocapture` with `LSX_TTS_RAW` = the folder of raw Qwen3-TTS
//! VoiceDesign takes (`<voice>/<line>_<take>.wav`, original designed voices, see samples/README.md) and optionally `LSX_DIR`.
//! * Shouts: every take trimmed (a 5 ms pre-roll, 40 ms tail), resampled to 48 kHz, normalised to -1 dBFS, 24-bit; the variants
//!   through the DAW's own effects (Digitize telephone / megaphone, Split Drive, a Reverb throw), the demon voice (an octave down
//!   with its length kept - stretch.rs - then driven and darkened), stutters and the trap "yo" roll laid on a tempo grid.
//! * Breaks: original patterns on Ling Membrane kits, rendered three times round and the middle copy kept, so the loop wraps.
//! * Vocal Phrases: the spoken TTS phrases, and a hook sung by Ling Voice.
//! * Stabs (Prism / Growl) and FX (Ling Cinema).
use crate::dsp::{fx, make_instrument, Instrument};

const SR: f32 = 48000.0;

fn out_dir() -> std::path::PathBuf {
    super::lsx_dir().join("samples")
}

fn peak(x: &[f32]) -> f32 {
    x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
}

/// -1 dBFS peak
fn normalise(x: &mut [f32]) {
    let p = peak(x).max(1e-9);
    let g = 0.891 / p;
    for v in x.iter_mut() {
        *v *= g;
    }
}

fn fade(y: &mut [f32], fin: f32, fout: f32) {
    let n = y.len() / 2;
    let (fi, fo) = ((SR * fin) as usize, (SR * fout) as usize);
    for i in 0..n {
        let a = if i < fi { i as f32 / fi as f32 } else { 1.0 };
        let b = if n - i < fo { (n - i) as f32 / fo as f32 } else { 1.0 };
        let g = a.min(b);
        y[i * 2] *= g;
        y[i * 2 + 1] *= g;
    }
}

/// leading and trailing silence off (under -42 dB of the peak), a 5 ms pre-roll and a 40 ms tail kept, faded
fn trim(x: &[f32]) -> Vec<f32> {
    let p = peak(x).max(1e-9);
    let th = p * 0.008;
    let n = x.len() / 2;
    let first = (0..n).find(|i| x[i * 2].abs().max(x[i * 2 + 1].abs()) > th).unwrap_or(0);
    let last = (0..n).rev().find(|i| x[i * 2].abs().max(x[i * 2 + 1].abs()) > th).unwrap_or(n.saturating_sub(1));
    let a = first.saturating_sub((SR * 0.005) as usize);
    let b = (last + (SR * 0.04) as usize).min(n);
    let mut y = x[a * 2..b * 2].to_vec();
    fade(&mut y, 0.002, 0.03);
    y
}

fn load48(path: &std::path::Path) -> Vec<f32> {
    let (st, rate) = crate::dsp::sampler::load_audio(&path.to_string_lossy()).unwrap();
    if rate as f32 == SR {
        st
    } else {
        crate::stretch::resample_speed(&st, rate as f64 / SR as f64)
    }
}

fn save(rel: &str, x: &[f32]) {
    let p = out_dir().join(rel);
    let _ = std::fs::create_dir_all(p.parent().unwrap());
    let mut y = x.to_vec();
    normalise(&mut y);
    crate::export::write_audio(&p.to_string_lossy(), &y, SR as u32, "wav", 24, 0.5).unwrap();
    println!("{rel}\t{:.2}s", y.len() as f32 / 2.0 / SR);
}

/// an effect with its defaults, a preset of it and some overrides, over `x` (and `tail` seconds of what rings after it)
fn effect(name: &str, preset: &str, set: &[(&str, f32)], x: &[f32], tail: f32) -> Vec<f32> {
    let mut e = fx::make(name, SR).unwrap();
    for (k, v) in fx::defaults(name) {
        e.set_param(k, *v);
    }
    if let Some((_, ps)) = fx::presets(name).iter().find(|p| p.0 == preset) {
        for (k, v) in ps.iter() {
            e.set_param(k, *v);
        }
    }
    for (k, v) in set {
        e.set_param(k, *v);
    }
    let mut y = x.to_vec();
    y.extend(vec![0.0; (tail * SR) as usize * 2]);
    for c in y.chunks_mut(1024) {
        e.process(c);
    }
    y
}

/// the demon: an octave down with the length kept, driven, the top rolled off, a little room
fn demon(x: &[f32]) -> Vec<f32> {
    let low = crate::stretch::pitch_shift(x, -12.0);
    let d = effect("Split Drive", "", &[("drive", 0.75), ("tone", 0.45)], &low, 0.0);
    let d = effect("EQ", "", &[("low", 0.7), ("high", 0.25)], &d, 0.0);
    trim(&effect("Reverb", "", &[("mix", 0.18), ("size", 0.5)], &d, 0.6))
}

/// the first `frac` beat of `x` repeated `n` times at `bpm`, then the whole thing (a stutter into the word)
fn stutter(x: &[f32], bpm: f32, n: usize, frac: f32) -> Vec<f32> {
    let step = (60.0 / bpm * frac * SR) as usize;
    let mut out = vec![];
    for _ in 0..n {
        let mut piece = x[..(step * 2).min(x.len())].to_vec();
        piece.resize(step * 2, 0.0);
        fade(&mut piece, 0.001, 0.008);
        out.extend(piece);
    }
    out.extend_from_slice(x);
    out
}

/// one-shots laid on a grid: (beat, sound) at `bpm`, `beats` long
fn lay(hits: &[(f32, &[f32])], bpm: f32, beats: f32) -> Vec<f32> {
    let n = (beats * 60.0 / bpm * SR) as usize;
    let mut out = vec![0.0f32; n * 2];
    for (b, s) in hits {
        let at = (b * 60.0 / bpm * SR) as usize;
        for (i, v) in s.iter().enumerate() {
            if at * 2 + i < out.len() {
                out[at * 2 + i] += v;
            }
        }
    }
    out
}

/// render notes (beat, key, vel, beats long) through an instrument at `bpm`, `beats` long plus `tail` seconds
fn render(inst: &mut dyn Instrument, notes: &[(f32, u8, f32, f32)], bpm: f32, beats: f32, tail: f32) -> Vec<f32> {
    inst.set_tempo(bpm as f64);
    let spb = 60.0 / bpm * SR;
    let n = (beats * spb + tail * SR) as usize;
    let mut ev: Vec<(usize, u8, f32, bool)> = vec![];
    for (b, k, v, l) in notes {
        ev.push(((b * spb) as usize, *k, *v, true));
        ev.push((((b + l) * spb) as usize, *k, 0.0, false));
    }
    ev.sort_by_key(|e| (e.0, e.3));
    let mut out = vec![0.0f32; n * 2];
    let mut pos = 0;
    for (at, k, v, on) in ev {
        let at = at.min(n);
        if at > pos {
            inst.render(&mut out[pos * 2..at * 2]);
            pos = at;
        }
        if on {
            inst.note_on(k, v);
        } else {
            inst.note_off(k);
        }
    }
    if pos < n {
        inst.render(&mut out[pos * 2..]);
    }
    out
}

fn kit(name: &str) -> Box<dyn Instrument> {
    let p = super::lsx_dir().join("instruments").join(format!("{name}.ling"));
    let text = std::fs::read_to_string(&p).unwrap();
    crate::patch::build(&crate::patch::parse(&text), SR).unwrap()
}

/// A drum loop that wraps: the pattern three times round, the middle copy kept. Steps: (key, 16 steps a bar: X accent, x hit,
/// g ghost, . rest).
fn loop_of(kit_name: &str, steps: &[(u8, &str)], bpm: f32, bars: f32) -> Vec<f32> {
    let beats = bars * 4.0;
    let mut notes = vec![];
    for rep in 0..3 {
        for (key, pat) in steps {
            for (i, c) in pat.chars().enumerate() {
                let v = match c {
                    'X' => 1.0,
                    'x' => 0.8,
                    'g' => 0.35,
                    _ => continue,
                };
                notes.push((rep as f32 * beats + i as f32 * 0.25, *key, v, 0.2));
            }
        }
    }
    let mut d = kit(kit_name);
    let all = render(d.as_mut(), &notes, bpm, beats * 3.0, 0.0);
    let n = (beats * 60.0 / bpm * SR) as usize;
    all[n * 2..n * 4].to_vec()
}

fn cinema(preset: &str, key: u8, beats: f32, bpm: f32) -> Vec<f32> {
    let mut c = make_instrument("cinema", SR).unwrap();
    c.load_preset(preset);
    trim(&render(c.as_mut(), &[(0.0, key, 0.9, beats)], bpm, beats, 2.5))
}

#[test]
#[ignore]
fn make_sample_pack() {
    let raw = std::path::PathBuf::from(std::env::var("LSX_TTS_RAW").expect("LSX_TTS_RAW = the raw TTS takes"));
    let voices = [("mc", "MC"), ("adlib", "Ad-Libs"), ("fem", "Female MC"), ("rasp", "Raspy")];
    // ── shouts: every take cleaned
    let mut takes: std::collections::BTreeMap<(String, String), Vec<f32>> = Default::default();
    for (v, folder) in voices {
        let Ok(rd) = std::fs::read_dir(raw.join(v)) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).filter(|p| p.extension().map(|e| e == "wav").unwrap_or(false)).collect();
        files.sort();
        for f in files {
            let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
            let x = trim(&load48(&f));
            if x.len() < 2 * (SR * 0.08) as usize || x.len() > 2 * (SR * 6.0) as usize {
                println!("skipped {v}/{stem} ({:.2}s)", x.len() as f32 / 2.0 / SR);
                continue;
            }
            let (line, take) = stem.rsplit_once('_').unwrap();
            save(&format!("Shouts/{folder}/{line}_{v}_{take}.wav"), &x);
            takes.insert((v.to_string(), stem.clone()), x);
        }
    }
    let get = |v: &str, id: &str| takes.get(&(v.to_string(), id.to_string())).cloned();
    // ── the variants through the DAW's effects
    for (v, id) in [("mc", "put_your_hands_up_1"), ("mc", "make_some_noise_1"), ("fem", "are_you_ready_1"), ("mc", "lets_go_1"), ("rasp", "hands_up_1"), ("adlib", "yeah_1")] {
        let Some(x) = get(v, id) else { continue };
        let line = id.rsplit_once('_').unwrap().0;
        save(&format!("Shouts/FX Variants/{line}_{v}_telephone.wav"), &trim(&effect("Digitize", "Telephone", &[], &x, 0.1)));
        save(&format!("Shouts/FX Variants/{line}_{v}_megaphone.wav"), &trim(&effect("Digitize", "Megaphone", &[], &x, 0.1)));
        save(&format!("Shouts/FX Variants/{line}_{v}_distorted.wav"), &trim(&effect("Split Drive", "", &[("drive", 0.8)], &x, 0.0)));
        save(&format!("Shouts/FX Variants/{line}_{v}_reverb_throw.wav"), &trim(&effect("Reverb", "", &[("mix", 0.55), ("size", 0.92), ("damp", 0.3)], &x, 3.0)));
        save(&format!("Shouts/FX Variants/{line}_{v}_stutter_150bpm.wav"), &stutter(&x, 150.0, 4, 0.25));
    }
    // ── the demon voice: the MC an octave down
    for id in ["put_your_hands_up_1", "lets_go_1", "bass_1", "drop_it_1", "get_low_1", "make_some_noise_1", "yo_1", "what_1", "come_on_1", "here_we_go_1"] {
        if let Some(x) = get("mc", id) {
            let line = id.rsplit_once('_').unwrap().0;
            save(&format!("Shouts/Demon/{line}_demon.wav"), &demon(&x));
        }
    }
    // ── the trap "yo" roll on a grid: yo . yo . yo yo yo yo (quarters, then eighths), and a triplet roll
    for (v, bpm) in [("adlib", 140.0f32), ("mc", 140.0), ("fem", 150.0)] {
        let Some(yo) = get(v, "yo_1") else { continue };
        let mut short = yo[..yo.len().min((60.0 / bpm * 0.5 * SR) as usize * 2)].to_vec();
        fade(&mut short, 0.001, 0.02);
        let mut quarter = yo[..yo.len().min((60.0 / bpm * SR) as usize * 2)].to_vec();
        fade(&mut quarter, 0.001, 0.03);
        let roll = lay(&[(0.0, &quarter[..]), (1.0, &quarter[..]), (2.0, &short[..]), (2.5, &short[..]), (3.0, &short[..]), (3.5, &short[..])], bpm, 4.0);
        save(&format!("Shouts/Loops/yo_roll_{v}_{bpm}bpm.wav"), &roll);
        let t = 1.0 / 3.0;
        let trip = lay(&[(0.0, &short[..]), (t, &short[..]), (2.0 * t, &short[..]), (1.0, &short[..]), (1.0 + t, &short[..]), (1.0 + 2.0 * t, &short[..]), (2.0, &quarter[..])], bpm, 4.0);
        save(&format!("Shouts/Loops/yo_triplets_{v}_{bpm}bpm.wav"), &trip);
    }
    // ── spoken phrases
    for v in ["narr", "poet"] {
        let Ok(rd) = std::fs::read_dir(raw.join(v)) else { continue };
        let mut files: Vec<_> = rd.flatten().map(|e| e.path()).collect();
        files.sort();
        for f in files {
            let stem = f.file_stem().unwrap().to_string_lossy().into_owned();
            let (line, take) = stem.rsplit_once('_').unwrap();
            save(&format!("Vocal Phrases/{line}_{v}_{take}.wav"), &trim(&load48(&f)));
        }
    }
    // ── a sung hook (Ling Voice, Aria): "oh - ah - yeah - na - na - hey - oh - ah" at 128 bpm, two bars
    {
        let mut v = make_instrument("voice", SR).unwrap();
        v.load_preset("aria");
        let line: [(&str, u8, f32, f32); 8] = [("o", 69, 0.0, 0.75), ("a", 72, 1.0, 0.75), ("je", 76, 2.0, 1.25), ("na", 74, 3.5, 0.4), ("na", 72, 4.0, 0.75), ("hei", 69, 5.0, 0.75), ("o", 67, 6.0, 0.75), ("a", 69, 7.0, 0.9)];
        let spb = 60.0 / 128.0 * SR;
        let n = (8.0 * spb + SR * 0.5) as usize;
        let mut out = vec![0.0f32; n * 2];
        let mut pos = 0usize;
        for (lyr, key, b, l) in line {
            let at = (b * spb) as usize;
            if at > pos {
                v.render(&mut out[pos * 2..at * 2]);
                pos = at;
            }
            v.note_on_lyric(key, 0.9, 0.0, 0.5, 0.5, lyr, l * spb);
            let off = ((b + l) * spb) as usize;
            v.render(&mut out[pos * 2..off * 2]);
            pos = off;
            v.note_off(key);
        }
        v.render(&mut out[pos * 2..]);
        save("Vocal Phrases/sung_hook_aria_128bpm.wav", &effect("Reverb", "", &[("mix", 0.2), ("size", 0.5)], &out, 0.0));
    }
    // ── breaks: original programming on Ling Membrane kits (two bars, 16 steps a bar)
    save(
        "Breaks/break_amenish_170bpm.wav",
        &loop_of("Kit_Drum_and_Bass", &[(36, "X.x.......X.....X.x.....x......."), (38, "....X..g.g..X..g....X..g.g..X.gX"), (51, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.")], 170.0, 2.0),
    );
    save(
        "Breaks/break_funky_140bpm.wav",
        &loop_of("Kit_Funk", &[(36, "X......x..X.....X......x.x......"), (38, "....X..g.g..X..g....X..g....X.gg"), (42, "x.xxx.x.x.xxx.x.x.xxx.x.x.xxx.x."), (46, "..............x...............x.")], 140.0, 2.0),
    );
    save("Breaks/break_boombap_90bpm.wav", &loop_of("Kit_Boom_Bap", &[(36, "X.....x...X.....X......x..x....."), (38, "....X.......X.g.....X.......X..."), (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.")], 90.0, 2.0));
    save("Breaks/break_trap_140bpm.wav", &loop_of("Kit_Trap", &[(36, "X......x..X.....X.........x....."), (39, "........X...............X......."), (42, "x.x.x.x.x.xxx.x.x.x.x.x.xxxxx.x.")], 140.0, 2.0));
    save(
        "Breaks/break_house_124bpm.wav",
        &loop_of("Kit_House_909", &[(36, "X...X...X...X...X...X...X...X..."), (39, "....X.......X.......X.......X..."), (46, "..x...x...x...x...x...x...x...x."), (42, "x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.x.")], 124.0, 2.0),
    );
    // ── stabs: eight Prism chords (Am F C G Am F G E) and Growl bass hits
    let chords: [[u8; 4]; 8] = [[57, 60, 64, 69], [53, 57, 60, 65], [55, 60, 64, 67], [55, 59, 62, 67], [57, 60, 64, 69], [53, 57, 60, 65], [55, 59, 62, 67], [52, 56, 59, 64]];
    {
        let mut p = make_instrument("prism", SR).unwrap();
        p.load_preset("Brass Stab");
        let notes: Vec<(f32, u8, f32, f32)> = chords.iter().enumerate().flat_map(|(i, c)| c.iter().map(move |k| (i as f32 * 1.5, *k, 0.9, 0.5))).collect();
        save("Stabs/stabs_prism_minor_chords.wav", &trim(&render(p.as_mut(), &notes, 120.0, 12.0, 1.0)));
        let mut p2 = make_instrument("prism", SR).unwrap();
        p2.load_preset("Deep Stab");
        let n2: Vec<(f32, u8, f32, f32)> = chords.iter().take(4).enumerate().flat_map(|(i, c)| c.iter().map(move |k| (i as f32, *k, 0.9, 0.3))).collect();
        save("Stabs/stabs_prism_deep_120bpm.wav", &render(p2.as_mut(), &n2, 120.0, 4.0, 0.0));
        let mut g = make_instrument("growl", SR).unwrap();
        g.load_preset("stab");
        let hits: Vec<(f32, u8, f32, f32)> = [41u8, 41, 44, 39, 41, 46, 44, 36].iter().enumerate().map(|(i, k)| (i as f32, *k, 0.9, 0.4)).collect();
        save("Stabs/growl_bass_hits_f_minor.wav", &trim(&render(g.as_mut(), &hits, 120.0, 8.0, 0.8)));
    }
    // ── FX from Ling Cinema: one-shots, and a row of hits to slice
    let one = [("Trailer Hit", 36u8, 1.0f32), ("Sub Drop", 36, 2.0), ("White Noise Riser", 60, 16.0), ("Noise Downlifter", 60, 8.0), ("Laser Zap", 72, 1.0), ("Data Burst", 60, 1.0), ("Fast Whoosh", 60, 2.0), ("Reverse Cymbal", 60, 8.0), ("Deep Braam", 36, 6.0), ("Tape Stop", 48, 3.0)];
    let mut row = vec![];
    for (name, key, beats) in one {
        let x = cinema(name, key, beats, 120.0);
        save(&format!("FX/{}.wav", name.to_lowercase().replace(' ', "_")), &x);
        if ["Trailer Hit", "Sub Drop", "Laser Zap", "Data Burst", "Fast Whoosh", "Tape Stop"].contains(&name) {
            let mut y = x.clone();
            y.truncate((SR * 1.6) as usize * 2);
            fade(&mut y, 0.0, 0.08);
            y.resize((SR * 1.8) as usize * 2, 0.0);
            row.extend(y);
        }
    }
    save("FX/fx_hits_cinema.wav", &row);
}
