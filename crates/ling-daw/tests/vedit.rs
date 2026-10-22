//! The inline vocal editor (daw_ve_*): analyse a sung line, move one note's pitch, stretch one and move another, render, and check the
//! result with the pitch tracker.
use ling_daw::{vocal, Daw, Val};

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.into())
}
fn call(d: &mut Daw, name: &str, a: &[Val]) -> Val {
    d.call(name, a).unwrap_or_else(|| panic!("{name} not handled")).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn nums(v: &Val) -> Vec<f64> {
    match v {
        Val::List(l) => l.iter().map(|x| if let Val::Num(x) = x { *x } else { f64::NAN }).collect(),
        _ => vec![],
    }
}

const SR: u32 = 44100;
/// three "sung" notes (a buzzy tone with a few harmonics and a soft attack), 0.45 s each with 0.12 s of silence between
const NOTES: [(f64, f64); 3] = [(0.20, 220.0), (0.77, 246.94), (1.34, 261.63)];
const LEN: f64 = 0.45;

fn voice() -> Vec<f32> {
    let total = 2.2;
    let mut out = vec![0.0f32; (total * SR as f64) as usize * 2];
    for (on, hz) in NOTES {
        let i0 = (on * SR as f64) as usize;
        let n = (LEN * SR as f64) as usize;
        let mut ph = 0.0f64;
        for k in 0..n {
            let t = k as f64 / SR as f64;
            let env = (t / 0.03).min(1.0) * ((LEN - t) / 0.04).clamp(0.0, 1.0);
            ph += hz / SR as f64;
            let x = (1..=6).map(|h| (std::f64::consts::TAU * ph * h as f64).sin() / h as f64).sum::<f64>() * 0.25 * env;
            out[(i0 + k) * 2] = x as f32;
            out[(i0 + k) * 2 + 1] = x as f32;
        }
    }
    out
}

/// median pitch (fractional MIDI) of `stereo` between a and b seconds
fn pitch_between(stereo: &[f32], a: f64, b: f64) -> f64 {
    let p = vocal::track_pitch(&vocal::mono_of(stereo), SR as f32);
    let mut v: Vec<f64> = (((a / p.hop) as usize)..((b / p.hop) as usize).min(p.f0.len())).map(|j| p.f0[j]).filter(|f| *f > 0.0).map(|f| 69.0 + 12.0 * (f as f64 / 440.0).log2()).collect();
    assert!(v.len() > 5, "no pitch between {a} and {b}");
    v.sort_by(|x, y| x.total_cmp(y));
    v[v.len() / 2]
}

/// the first time after `from` the level rises above a quarter of its peak
fn onset_after(stereo: &[f32], from: f64) -> f64 {
    let mono = vocal::mono_of(stereo);
    let lv = vocal::level_track(&mono, SR as f32, 0.005);
    let top = lv.iter().cloned().fold(0.0, f32::max);
    let j0 = (from / 0.005) as usize;
    let j = (j0..lv.len()).find(|j| lv[*j] > top * 0.25).expect("no onset");
    j as f64 * 0.005
}

#[test]
fn edit_pitch_time_and_render() {
    let path = std::env::temp_dir().join("lsx_vedit_voice.wav").to_string_lossy().into_owned();
    ling_daw::export::write_audio(&path, &voice(), SR, "wav", 32, 1.0).unwrap();
    let mut d = Daw::new();
    let t = match call(&mut d, "daw_track_add", &[s("audio"), s("")]) {
        Val::Num(x) => x,
        _ => panic!(),
    };
    let c = match call(&mut d, "daw_clip_add_audio", &[n(t), s(&path), n(0.0)]) {
        Val::Num(x) => x,
        v => panic!("clip_add_audio gave {v:?}"),
    };
    let found = match call(&mut d, "daw_ve_analyze", &[n(t), n(c)]) {
        Val::Num(x) => x,
        _ => panic!(),
    };
    assert_eq!(found, 3.0, "three notes sung");
    let b = nums(&call(&mut d, "daw_ve_blobs", &[n(t), n(c)]));
    assert_eq!(b.len(), 27);
    for (i, (on, hz)) in NOTES.iter().enumerate() {
        let want = 69.0 + 12.0 * (hz / 440.0f64).log2();
        assert!((b[i * 9 + 4] - want).abs() < 0.3, "note {i}: pitch {} vs {want}", b[i * 9 + 4]);
        assert!((b[i * 9] - on).abs() < 0.06, "note {i}: starts at {} vs {on}", b[i * 9]);
    }
    // the curve of a note follows its pitch
    let cv = nums(&call(&mut d, "daw_ve_curve", &[n(t), n(c), n(1.0), n(20.0)]));
    assert_eq!(cv.len(), 40);
    let mid = cv[20];
    assert!((mid - b[9 + 4]).abs() < 0.5, "curve at {mid}");

    // first note up a whole tone; second note 0.15 s longer (the third has to move out of the way); third 0.1 s later
    call(&mut d, "daw_ve_set", &[n(t), n(c), s("0"), s("dshift"), n(2.0)]);
    let off1 = b[9 + 3];
    call(&mut d, "daw_ve_set", &[n(t), n(c), s("2"), s("move"), n(0.25)]);
    call(&mut d, "daw_ve_set", &[n(t), n(c), s("1"), s("off"), n(off1 + 0.15)]);
    let b2 = nums(&call(&mut d, "daw_ve_blobs", &[n(t), n(c)]));
    assert!((b2[5] - 2.0).abs() < 1e-6);
    assert!((b2[9 + 3] - (off1 + 0.15)).abs() < 1e-3, "second note ends at {}", b2[9 + 3]);
    assert!((b2[18 + 2] - (b[18 + 2] + 0.25)).abs() < 1e-3, "third note starts at {}", b2[18 + 2]);

    let r = nums(&call(&mut d, "daw_ve_render", &[n(t), n(c)]));
    assert_eq!(r[0], 1.0, "render failed: {r:?}");
    let file = match call(&mut d, "daw_clip_file", &[n(t), n(c)]) {
        Val::Str(p) => p,
        _ => panic!(),
    };
    assert!(file.contains("vocal_edit_"), "the clip plays the render ({file})");
    let out = ling_daw::model::load_sample(&file).unwrap();
    let orig = ling_daw::model::load_sample(&path).unwrap();
    assert!(out.stereo.len() > orig.stereo.len(), "the render got longer");
    // pitch: note 1 up two semitones, note 2 untouched
    let p0 = pitch_between(&out.stereo, b2[2] + 0.08, b2[3] - 0.08);
    assert!((p0 - (b[4] + 2.0)).abs() < 0.35, "first note at {p0}, wanted {}", b[4] + 2.0);
    let p1 = pitch_between(&out.stereo, b2[9 + 2] + 0.08, b2[9 + 3] - 0.08);
    assert!((p1 - b[9 + 4]).abs() < 0.35, "second note moved to {p1}");
    // the stretched second note still sounds near its new end, and the third starts where it was put
    let p1e = pitch_between(&out.stereo, b2[9 + 3] - 0.12, b2[9 + 3] - 0.03);
    assert!((p1e - b[9 + 4]).abs() < 0.5, "end of the stretched note at {p1e}");
    let on3 = onset_after(&out.stereo, b2[9 + 3] + 0.02);
    assert!((on3 - b2[18 + 2]).abs() < 0.06, "third note starts at {on3}, wanted {}", b2[18 + 2]);

    // correct to C major: B stays, A (shifted to B) stays; reset brings the original back
    let k = match call(&mut d, "daw_ve_correct", &[n(t), n(c), s(""), n(1.0), n(0.5), n(0.0), n(0.0)]) {
        Val::Num(x) => x,
        _ => panic!(),
    };
    assert!(k >= 1.0);
    call(&mut d, "daw_ve_reset", &[n(t), n(c), s("")]);
    let r = nums(&call(&mut d, "daw_ve_render", &[n(t), n(c)]));
    assert_eq!(r[0], 1.0);
    let back = match call(&mut d, "daw_clip_file", &[n(t), n(c)]) {
        Val::Str(p) => p,
        _ => panic!(),
    };
    assert_eq!(back, path, "an untouched edit plays the recording itself");

    // notes to MIDI
    let mt = match call(&mut d, "daw_ve_to_midi", &[n(t), n(c)]) {
        Val::Num(x) => x,
        _ => panic!(),
    };
    assert!(mt > t);
    let notes = nums(&call(&mut d, "daw_clip_notes", &[n(mt), n(0.0)]));
    assert_eq!(notes.len(), 21, "three notes");
    assert_eq!(notes[2], 57.0);
}
