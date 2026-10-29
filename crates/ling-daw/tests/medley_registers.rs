//! Mashup registers end to end: the same mashup built with register allocation off and on, both bounced to
//! renders/mashup_registers/ with their low / mid / high band levels (`render_mashup_registers`, ignored: it renders audio).
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
fn text(v: Val) -> String {
    if let Val::Str(x) = v {
        x
    } else {
        String::new()
    }
}

/// (rms dBFS, share of the energy under 60 Hz, 60..250 Hz, over 5 kHz, peak dBFS) of a 16-bit stereo WAV (one-pole band splits)
fn bands(path: &str) -> (f64, f64, f64, f64, f64) {
    let mut r = hound::WavReader::open(path).unwrap();
    let sr = r.spec().sample_rate as f64;
    let x: Vec<f64> = r.samples::<i16>().map(|v| v.unwrap() as f64 / 32768.0).collect::<Vec<_>>().chunks(2).map(|c| (c[0] + c.get(1).copied().unwrap_or(0.0)) * 0.5).collect();
    let a = |fc: f64| 1.0 - (-2.0 * std::f64::consts::PI * fc / sr).exp();
    let (a60, a250, a5k) = (a(60.0), a(250.0), a(5000.0));
    let (mut l60, mut l250, mut l5k) = (0.0, 0.0, 0.0);
    let (mut e, mut e60, mut e250, mut e5k, mut peak) = (0.0, 0.0, 0.0, 0.0, 0.0f64);
    for v in &x {
        l60 += a60 * (v - l60);
        l250 += a250 * (v - l250);
        l5k += a5k * (v - l5k);
        e += v * v;
        e60 += l60 * l60;
        e250 += (l250 - l60) * (l250 - l60);
        e5k += (v - l5k) * (v - l5k);
        peak = peak.max(v.abs());
    }
    let tot = e.max(1e-12);
    (10.0 * (e / x.len().max(1) as f64).max(1e-12).log10(), e60 / tot, e250 / tot, e5k / tot, 20.0 * peak.max(1e-9).log10())
}

#[test]
#[ignore]
fn render_mashup_registers() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    std::env::set_current_dir(lsx).unwrap();
    let src = std::env::var("MASHUP_MIDI_DIR").unwrap_or_else(|_| "projects/A.I. Love You/midi".into());
    let dir = std::env::var("MASHUP_DEMO_DIR").unwrap_or_else(|_| "renders/mashup_registers".into());
    std::fs::create_dir_all(&dir).unwrap();
    let mut report = String::new();
    for (name, regs) in [("1_registers_off", "off"), ("2_registers_guard", "guard"), ("3_registers_layer", "layer")] {
        let mut d = Daw::new();
        let opts = format!("layering=0.8;rolls=0.6;genre=dubstep;feel=0;registers={regs};story=a storm that clears");
        let msg = text(call(&mut d, "daw_song_medley_build", &[s(&src), n(4.0), n(6.0), s("smart"), s(""), s(""), n(7.0), n(140.0), s(&opts)]));
        let path = format!("{dir}/{name}.wav");
        call(&mut d, "daw_bounce", &[s(&path), n(0.0), n(0.0), n(1.0), n(16.0), s("wav"), n(0.5)]);
        let (rms, lo, lomid, hi, peak) = bands(&path);
        let line = format!("{name}: rms {rms:.1} dBFS, peak {peak:.1} dBFS, energy under 60 Hz {:.1} %, 60-250 Hz {:.1} %, over 5 kHz {:.1} %\n  {msg}\n", lo * 100.0, lomid * 100.0, hi * 100.0);
        println!("{line}");
        report.push_str(&line);
    }
    std::fs::write(format!("{dir}/summary.txt"), &report).unwrap();
}
