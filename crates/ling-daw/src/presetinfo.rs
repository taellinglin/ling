//! What every built-in preset sounds like, measured - and the level trim that makes an instrument's presets sit together.
//!
//! `analyze` renders a preset (a held C3, then C4) and measures it: loudness and peak, attack time, how much of the level it keeps
//! (plucky / decaying / sustained), brightness (spectral centroid), movement (how much the tone and level change over time), noise
//! (spectral flatness), stereo width, low end, and whether it plays in tune with 12-tone equal temperament. `words` turns that into
//! plain words and a likely use (bass / pad / lead / pluck / fx), which the preset lists show and the generators (Redo Instrument &
//! Sound, the agent) match against.
//!
//! The table (`preset_info.tsv`: kind, preset, description, trim in dB) is written by the ignored `write_preset_info` test from
//! LingStationX and compiled in. `trim_db` is the gain that brings a preset to the median loudness of its instrument (within +-9 dB,
//! never past -1 dBFS peak); `apply_trim` sets it on the instrument's own level knob when the preset or its patch file loads.
use crate::dsp::Instrument;
use std::collections::HashMap;
use std::sync::OnceLock;

#[derive(Clone, Debug, Default)]
pub struct Features {
    pub rms_db: f32,
    pub peak_db: f32,
    pub attack_s: f32,
    pub sustain: f32,
    pub centroid: f32,
    /// the centroid over the note's frequency at C4: about 1-2 dark, 2-4 warm, 4-8 bright, above airy
    pub bright: f32,
    /// share of the energy an octave and more below the note at C3 (a sub)
    pub sub: f32,
    pub movement: f32,
    pub noise: f32,
    pub width: f32,
    pub lows: f32,
    pub pitched: f32,
    pub in_tune: bool,
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-9).log10()
}

/// render `secs` of a held note then `rel` of release
fn play(inst: &mut dyn Instrument, key: u8, secs: f32, rel: f32, sr: f32) -> (Vec<f32>, usize) {
    inst.all_off();
    let mut out = vec![0f32; ((secs + rel) * sr) as usize * 2];
    let on = (secs * sr) as usize * 2;
    inst.note_on(key, 0.85);
    let mut i = 0;
    while i < out.len() {
        if i >= on && i < on + 512 {
            inst.note_off(key);
        }
        let n = 512.min(out.len() - i);
        inst.render(&mut out[i..i + n]);
        i += n;
    }
    inst.all_off();
    (out, on)
}

fn spectrum(mono: &[f32], sr: f32, below: f32) -> (f32, f32, f32) {
    // centroid, flatness and the share of energy below `below` Hz: a Hann-windowed DFT of the middle 2048 samples at evenly
    // spaced bins (so no part of the spectrum is counted more often than another) up to 16 kHz
    let n = 2048usize;
    if mono.len() < n {
        return (0.0, 0.0, 0.0);
    }
    let start = mono.len() / 2 - n / 2;
    let x: Vec<f32> = (0..n).map(|k| mono[start + k] * (0.5 - 0.5 * (std::f32::consts::TAU * k as f32 / n as f32).cos())).collect();
    let df = sr / n as f32;
    let top = ((16000.0f32.min(sr * 0.45)) / df) as usize;
    let (mut num, mut den, mut low, mut logsum, mut cnt) = (0.0f64, 0.0f64, 0.0f64, 0.0f64, 0usize);
    for b in 1..top {
        let w = std::f32::consts::TAU * b as f32 / n as f32;
        let (mut re, mut im) = (0.0f32, 0.0f32);
        // rotate a phasor instead of calling cos / sin per sample
        let (c, s) = (w.cos(), w.sin());
        let (mut pr, mut pi) = (1.0f32, 0.0f32);
        for v in &x {
            re += v * pr;
            im += v * pi;
            let t = pr * c - pi * s;
            pi = pr * s + pi * c;
            pr = t;
        }
        let p = (re * re + im * im) as f64 + 1e-15;
        let f = b as f32 * df;
        num += f as f64 * p;
        den += p;
        logsum += p.ln();
        cnt += 1;
        if f < below {
            low += p;
        }
    }
    let geo = (logsum / cnt.max(1) as f64).exp();
    let arith = den / cnt.max(1) as f64;
    ((num / den.max(1e-15)) as f32, (geo / arith.max(1e-15)).clamp(0.0, 1.0) as f32, (low / den.max(1e-15)) as f32)
}

/// Measure a preset (the instrument already has it loaded).
pub fn analyze(inst: &mut dyn Instrument, sr: f32) -> Features {
    let mut f = Features::default();
    let (a, on) = play(inst, 48, 1.2, 0.6, sr);
    let mono: Vec<f32> = a.chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
    let side: Vec<f32> = a.chunks(2).map(|c| (c[0] - c[1]) * 0.5).collect();
    let held = &mono[..on / 2];
    let win = (sr * 0.02) as usize;
    let env: Vec<f32> = held.chunks(win).map(|c| (c.iter().map(|x| x * x).sum::<f32>() / c.len() as f32).sqrt()).collect();
    let mx = env.iter().copied().fold(0.0f32, f32::max);
    if mx < 1e-5 {
        f.rms_db = -120.0;
        f.peak_db = -120.0;
        return f;
    }
    f.peak_db = db(a.iter().fold(0.0f32, |m, x| m.max(x.abs())));
    // loudness: the louder half of the held part (a pluck is judged by its body, not its silence)
    let mut sorted = env.clone();
    sorted.sort_by(|x, y| y.total_cmp(x));
    let top = &sorted[..(sorted.len() / 2).max(1)];
    f.rms_db = db((top.iter().map(|x| x * x).sum::<f32>() / top.len() as f32).sqrt());
    f.attack_s = env.iter().position(|e| *e >= mx * 0.7).unwrap_or(0) as f32 * 0.02;
    let tail = &env[env.len() * 4 / 5..];
    f.sustain = tail.iter().sum::<f32>() / tail.len().max(1) as f32 / mx;
    let sm = (side.iter().map(|x| x * x).sum::<f32>() / side.len() as f32).sqrt();
    let mm = (mono.iter().map(|x| x * x).sum::<f32>() / mono.len() as f32).sqrt();
    f.width = sm / mm.max(1e-9);
    // energy below 150 Hz, and below three quarters of C3 (a sub an octave down)
    f.lows = spectrum(held, sr, 150.0).2;
    f.sub = spectrum(held, sr, 130.81 * 0.75).2;
    // brightness and noise at C4
    let (b, onb) = play(inst, 60, 0.8, 0.1, sr);
    let mb: Vec<f32> = b[..onb].chunks(2).map(|c| (c[0] + c[1]) * 0.5).collect();
    let (cen, flat, _) = spectrum(&mb, sr, 150.0);
    f.centroid = cen;
    f.bright = cen / 261.63;
    f.noise = flat;
    // movement: how the level and the brightness change across the held note (zero crossings stand in for the brightness)
    let lv: Vec<f32> = env[env.len() / 5..].to_vec();
    let lm = lv.iter().sum::<f32>() / lv.len().max(1) as f32;
    let lvar = (lv.iter().map(|x| (x - lm).powi(2)).sum::<f32>() / lv.len().max(1) as f32).sqrt() / lm.max(1e-9);
    let zc: Vec<f32> = held.chunks(win * 3).map(|c| c.windows(2).filter(|w| w[0] <= 0.0 && w[1] > 0.0).count() as f32).collect();
    let zm = zc.iter().sum::<f32>() / zc.len().max(1) as f32;
    let zvar = (zc.iter().map(|x| (x - zm).powi(2)).sum::<f32>() / zc.len().max(1) as f32).sqrt() / zm.max(1.0);
    f.movement = if f.sustain > 0.3 { lvar.max(zvar) } else { zvar };
    inst.all_off();
    f
}

/// the tune check on a fresh copy of the preset (state left by other renders confuses the pitch tracker); `make` builds it
pub fn tuning(inst: &mut dyn Instrument, sr: f32) -> (f32, bool) {
    let t = crate::tunecheck::check(inst, sr);
    (t.pitched, t.in_tune())
}

/// the part a preset is for, from its category or its name ("" when neither says)
pub fn usage_hint(name: &str, category: &str) -> &'static str {
    let s = format!("{} {}", category.to_lowercase(), name.to_lowercase());
    let has = |ws: &[&str]| ws.iter().any(|w| s.contains(w));
    if has(&["drum", "kick", "snare", "hat", "perc", "sfx", "fx", "riser", "laser", "zap", "explosion", "coin", "jump", "hit", "noise", "alarm", "warp", "rocket", "game over", "menu", "select", "spring", "fireball", "1-up", "power up", "pickup"]) {
        "fx / percussion"
    } else if has(&["bass", "sub", "808", "reese", "wob", "riddim", "growl"]) {
        "bass"
    } else if has(&["pad", "string", "choir", "drone", "haze", "atmos", "ambient"]) {
        "pad"
    } else if has(&["pluck", "arp", "bell", "keys", "piano", "stab", "harp", "guitar", "koto", "kalimba", "mallet", "chord"]) {
        "pluck / keys"
    } else if has(&["lead", "screech", "saw", "hook", "anthem", "brass", "flute", "voice", "vox", "talk"]) {
        "lead"
    } else {
        ""
    }
}

/// plain words for the measurements, with the part it is for first (from the hint, else measured)
pub fn words(f: &Features, hint: &str) -> String {
    if f.rms_db < -100.0 {
        return "silent at a held C3 (needs its own trigger or settings)".into();
    }
    let mut w: Vec<&str> = vec![];
    let unpitched = f.pitched < 0.3 || f.noise > 0.45;
    let usage = if !hint.is_empty() {
        hint
    } else if unpitched {
        "fx / percussion"
    } else if f.sub > 0.2 && f.width < 0.25 {
        "bass"
    } else if f.sustain < 0.18 {
        "pluck / keys"
    } else if f.attack_s > 0.2 || (f.sustain > 0.55 && f.width > 0.3) {
        "pad"
    } else {
        "lead"
    };
    w.push(usage);
    w.push(if f.bright < 2.0 {
        "dark"
    } else if f.bright < 4.0 {
        "warm"
    } else if f.bright < 8.0 {
        "bright"
    } else {
        "airy / very bright"
    });
    w.push(if f.sustain < 0.18 {
        "plucky"
    } else if f.sustain < 0.5 {
        "decaying"
    } else {
        "sustained"
    });
    if f.attack_s > 0.2 {
        w.push("slow attack");
    }
    if f.movement > 0.25 {
        w.push("moving");
    }
    if f.noise > 0.3 && !unpitched {
        w.push("breathy / noisy");
    }
    if f.width > 0.35 {
        w.push("wide");
    } else if f.width < 0.05 {
        w.push("mono");
    }
    if f.sub > 0.25 {
        w.push("deep sub");
    }
    // only a steady, tonal sound is judged for tuning (modulated growls fool the pitch tracker)
    if !f.in_tune && !unpitched && f.pitched > 0.6 && f.movement < 0.2 {
        w.push("microtonal / detuned (not 12-TET)");
    }
    w.join(", ")
}

// ── the table ─────────────────────────────────────────────────────────────────────

/// (description, trim dB) by "kind\tpreset"
fn table() -> &'static HashMap<String, (String, f32)> {
    static T: OnceLock<HashMap<String, (String, f32)>> = OnceLock::new();
    T.get_or_init(|| {
        let mut m = HashMap::new();
        for line in include_str!("preset_info.tsv").lines() {
            let f: Vec<&str> = line.split('\t').collect();
            if f.len() >= 4 {
                m.insert(format!("{}\t{}", f[0], f[1].to_lowercase()), (f[2].to_string(), f[3].parse().unwrap_or(0.0)));
            }
        }
        m
    })
}

/// The description of a built-in preset ("" when it is not in the table)
pub fn info(kind: &str, preset: &str) -> String {
    table().get(&format!("{kind}\t{}", preset.to_lowercase())).map(|x| x.0.clone()).unwrap_or_default()
}

pub fn trim_db(kind: &str, preset: &str) -> f32 {
    table().get(&format!("{kind}\t{}", preset.to_lowercase())).map(|x| x.1).unwrap_or(0.0)
}

/// the instrument's level knob and how it maps to gain: (name, exponent of the gain law)
pub fn level_param(kind: &str) -> Option<(&'static str, f32)> {
    match kind {
        "growl" | "prism" | "chip" => Some((if kind == "chip" { "level" } else { "out" }, 2.0)),
        "pluck" => Some(("level", 1.0)),
        _ => None,
    }
}

/// Set a loaded preset's level to where the table puts it (nothing for presets without a trim)
pub fn apply_trim(inst: &mut dyn Instrument, kind: &str, preset: &str) {
    let t = trim_db(kind, preset);
    if t.abs() < 0.05 {
        return;
    }
    let Some((name, law)) = level_param(kind) else { return };
    let Some(v) = inst.get_param(name) else { return };
    let g = 10f32.powf(t / 20.0);
    let nv = (v * g.powf(1.0 / law)).clamp(0.02, 1.0);
    inst.set_param(name, nv);
}

/// a preset's own hand-written description, where the instrument has one (Growl presets: the comment under the name)
fn hand_description(kind: &str, preset: &str) -> String {
    if kind == "growl" {
        let text = include_str!("growl_presets.txt");
        let head = format!("[{preset}]");
        if let Some(i) = text.find(&head) {
            let after = &text[i + head.len()..];
            if let Some(l) = after.lines().nth(1) {
                let c = l.trim().trim_start_matches('#').trim();
                if !c.is_empty() && !c.starts_with("Ling Growl patch") {
                    return c.to_string();
                }
            }
        }
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_measurements_tell_a_pluck_from_a_pad_and_a_bass() {
        let sr = 44100.0;
        let mut p = crate::dsp::make_instrument("pluck", sr).unwrap();
        p.load_preset("Muted Pluck");
        let f = analyze(p.as_mut(), sr);
        assert!(f.sustain < 0.3, "{f:?}");
        assert!(words(&f, "").contains("pluck"), "{}", words(&f, ""));
        let mut c = crate::dsp::make_instrument("chip", sr).unwrap();
        c.load_preset("Explosion");
        let fx = analyze(c.as_mut(), sr);
        assert!(words(&fx, usage_hint("Explosion", "SFX")).starts_with("fx"), "{}", words(&fx, ""));
        assert_eq!(usage_hint("Rolling Bass", "basses"), "bass");
        assert_eq!(usage_hint("Uplift Pad", "pads"), "pad");
    }

    /// not a test: measures every built-in preset of Growl, Prism, Pluck and Chip and writes src/preset_info.tsv (run from
    /// LingStationX: `PRESET_INFO=<ling-daw src dir>`)
    #[test]
    #[ignore]
    fn write_preset_info() {
        let Ok(out) = std::env::var("PRESET_INFO") else { return };
        if let Ok(root) = std::env::var("LSX_ROOT") {
            let _ = std::env::set_current_dir(root);
        }
        let sr = 44100.0;
        let mut lines: Vec<String> = vec![];
        for kind in ["growl", "prism", "pluck", "chip"] {
            let probe = crate::dsp::make_instrument(kind, sr).unwrap();
            let names: Vec<String> = probe.presets().iter().map(|s| s.to_string()).collect();
            let given: HashMap<String, String> = probe.preset_categories().into_iter().collect();
            let mut rows: Vec<(String, String, Features)> = vec![];
            for n in &names {
                let mut inst = crate::dsp::make_instrument(kind, sr).unwrap();
                inst.load_preset(n);
                let mut f = analyze(inst.as_mut(), sr);
                let mut fresh = crate::dsp::make_instrument(kind, sr).unwrap();
                fresh.load_preset(n);
                let (pitched, in_tune) = tuning(fresh.as_mut(), sr);
                f.pitched = pitched;
                f.in_tune = in_tune;
                let hand = { let h = probe.preset_info(n); if h.is_empty() { hand_description(kind, n) } else { h } };
                let cat = given.get(n).cloned().unwrap_or_default();
                let mut d = words(&f, usage_hint(n, &cat));
                if !cat.is_empty() && kind == "prism" {
                    d = format!("{} - {d}", cat.to_lowercase());
                }
                if !hand.is_empty() {
                    d = format!("{hand} ({d})");
                }
                rows.push((n.clone(), d, f));
            }
            // the instrument's median loudness is the target; silent ones are left alone
            let mut l: Vec<f32> = rows.iter().map(|r| r.2.rms_db).filter(|x| *x > -100.0).collect();
            l.sort_by(f32::total_cmp);
            let target = l.get(l.len() / 2).copied().unwrap_or(-18.0);
            for (n, d, f) in rows {
                // only the outliers: more than 3 dB from the instrument's median, brought to 2 dB of it (at most 6 dB), never past
                // -1 dBFS peak; everything else stays as its designer set it
                let off = target - f.rms_db;
                let mut t = if f.rms_db > -100.0 && off.abs() > 3.0 { (off - off.signum() * 2.0).clamp(-6.0, 6.0) } else { 0.0 };
                t = t.min(-1.0 - f.peak_db).max(-6.0);
                if t.abs() < 1.0 {
                    t = 0.0;
                }
                lines.push(format!("{kind}\t{n}\t{}\t{t:.1}", d.replace('\t', " ")));
            }
        }
        std::fs::write(std::path::Path::new(&out).join("preset_info.tsv"), lines.join("\n") + "\n").unwrap();
    }
}
