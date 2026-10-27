//! Sing a melody with lyrics on Ling Voice and render it alone (an acapella): the notes (`ACA_MELODY`, JSON [[start, len, key]...]
//! in beats), the lyrics (`ACA_LYRICS`, a text file; [labels] are dropped), one syllable per note (lyricfit strict), `ACA_BPM`,
//! the singer (`ACA_SINGER`: aria, mei, kai, ren, choir ...), `ACA_SHIFT` semitones added to the notes between `ACA_SHIFT_FROM` and
//! `ACA_SHIFT_TO` beats (a verse moved into the singer's range), written to `ACA_OUT`. Run from the LingStationX folder's parent:
//! cargo test -p ling-daw --test anthem_acapella -- --ignored --nocapture
use ling_daw::{Daw, Val};

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.to_string())
}
fn call(d: &mut Daw, name: &str, a: &[Val]) -> Val {
    d.call(name, a).unwrap_or_else(|| panic!("{name} not handled")).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn var(k: &str, d: &str) -> String {
    std::env::var(k).unwrap_or_else(|_| d.to_string())
}

#[test]
#[ignore]
fn sing_it() {
    let lsx = var("ACA_LSX", concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX"));
    std::env::set_current_dir(&lsx).unwrap();
    let mel: Vec<Vec<f64>> = serde_json::from_str(&std::fs::read_to_string(var("ACA_MELODY", "")).unwrap()).unwrap();
    let lyrics: String = std::fs::read_to_string(var("ACA_LYRICS", "")).unwrap().lines().filter(|l| !l.trim_start().starts_with('[') && !l.contains("syllable") && !l.starts_with("Anthem of")).collect::<Vec<_>>().join("\n");
    let bpm: f64 = var("ACA_BPM", "120").parse().unwrap();
    let singer = var("ACA_SINGER", "aria");
    let shift: f64 = var("ACA_SHIFT", "0").parse().unwrap();
    let (from, to): (f64, f64) = (var("ACA_SHIFT_FROM", "0").parse().unwrap(), var("ACA_SHIFT_TO", "0").parse().unwrap());
    let out = var("ACA_OUT", "acapella.wav");

    let mut d = Daw::new();
    call(&mut d, "daw_set_bpm", &[n(bpm)]);
    let patch = format!("instruments/Voice/Voice_{singer}.ling");
    let t = call(&mut d, "daw_track_add", &[s("voice"), s(&patch)]);
    let Val::Num(t) = t else { panic!() };
    let end = mel.iter().map(|m| m[0] + m[1]).fold(0.0, f64::max);
    let c = call(&mut d, "daw_clip_add_midi", &[n(t), n(0.0), n((end / 4.0).ceil() * 4.0)]);
    let Val::Num(c) = c else { panic!() };
    let mut flat = vec![];
    for m in &mel {
        let key = m[2] + if m[0] >= from && m[0] < to { shift } else { 0.0 };
        // phrase-final long notes a little louder, the quick runs a little lighter
        let vel = if m[1] >= 2.0 { 0.85 } else if m[1] < 0.5 { 0.66 } else { 0.76 };
        flat.extend([n(m[0]), n(m[1] * 0.97), n(key), n(vel), n(0.0), n(0.5), n(0.5)]);
    }
    call(&mut d, "daw_clip_set_notes", &[n(t), n(c), Val::List(flat)]);
    let info = call(&mut d, "daw_lyrf_apply", &[n(t), n(c), s(&lyrics), s("fit:strict:0:0:all:1.0:auto")]);
    eprintln!("lyric fit [notes sung, syllables, lines, groups, phrases, chords, groups with a syllable]: {info:?}");
    // ACA_ENGINE=singtts: SingTTS sings the track (the DAW's own job: a new audio track, the Ling Voice track muted)
    if var("ACA_ENGINE", "") == "singtts" {
        let ok = call(&mut d, "daw_singer_singtts", &[n(t), s(&var("ACA_REFS", "")), s("models/singtts"), s(&var("ACA_PYTHON", "python")), n(var("ACA_SEED", "1").parse().unwrap()), s("en")]);
        eprintln!("singtts started: {ok:?}  {:?}", call(&mut d, "daw_status", &[]));
        let t0 = std::time::Instant::now();
        loop {
            std::thread::sleep(std::time::Duration::from_millis(500));
            let Val::List(r) = call(&mut d, "daw_rvc_poll", &[]) else { panic!() };
            let state = if let Val::Num(x) = r[0] { x } else { 0.0 };
            if state < 0.5 || state > 1.5 {
                eprintln!("singtts done in {:.0} s: {:?}", t0.elapsed().as_secs_f64(), r);
                break;
            }
        }
    }
    let r = call(&mut d, "daw_bounce", &[s(&out), n(0.0), n(end + 2.0), n(2.5), n(24.0)]);
    eprintln!("{r:?} -> {out}");
}
