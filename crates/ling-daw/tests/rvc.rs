//! Voice skins through the RVC install on this machine (E:/SannyChat/talk, its LingLin model); skipped where it is not installed.
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
    match v {
        Val::List(l) => l.clone(),
        _ => vec![],
    }
}
fn num(v: &Val) -> f64 {
    if let Val::Num(x) = v { *x } else { f64::NAN }
}
fn st(v: &Val) -> String {
    if let Val::Str(x) = v { x.clone() } else { String::new() }
}

const RVC: &str = "E:/SannyChat/talk";

#[test]
fn skin_an_audio_clip_and_restore_it() {
    let info = list(&call(&mut Daw::new(), "daw_rvc_info", &[s(RVC)]));
    if st(&info[0]).is_empty() {
        return;
    }
    let mut d = Daw::new();
    let models = list(&call(&mut d, "daw_rvc_models", &[s(&format!("{RVC}/weights"))]));
    let k = models.chunks(3).position(|m| st(&m[0]) == "LingLin").expect("LingLin among the models");
    let (pth, idx) = (st(&models[k * 3 + 1]), st(&models[k * 3 + 2]));
    assert!(idx.contains("added_") && idx.ends_with(".index"), "the index from logs/LingLin: {idx}");
    // a sung line (three buzzy notes) on an audio track
    let path = std::env::temp_dir().join("lsx_rvc_voice.wav").to_string_lossy().into_owned();
    let sr = 44100u32;
    let mut v = vec![0f32; (2.0 * sr as f32) as usize * 2];
    for (on, hz) in [(0.2f32, 220f32), (0.8, 247.0), (1.4, 262.0)] {
        for i in 0..(0.45 * sr as f32) as usize {
            let t = i as f32 / sr as f32;
            let x: f32 = (1..6).map(|h| (std::f32::consts::TAU * hz * h as f32 * t).sin() / h as f32).sum::<f32>() * 0.25 * (t / 0.03).min(1.0) * ((0.45 - t) / 0.04).clamp(0.0, 1.0);
            let j = ((on * sr as f32) as usize + i) * 2;
            v[j] = x;
            v[j + 1] = x;
        }
    }
    ling_daw::export::write_audio(&path, &v, sr, "wav", 16, 1.0).unwrap();
    let t = num(&call(&mut d, "daw_track_add", &[s("audio"), s("")]));
    let c = num(&call(&mut d, "daw_clip_add_audio", &[n(t), s(&path), n(0.0)]));
    let script = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX/tools/rvc_skin.py");
    assert!(std::path::Path::new(script).is_file(), "{script}");
    let started = call(&mut d, "daw_rvc_start", &[n(t), n(c), s(RVC), s(script), s(&pth), s(&idx), n(0.0), s("rmvpe"), n(0.66), n(0.33), n(0.25), n(0.0)]);
    assert!(matches!(started, Val::Num(x) if x > 0.5), "{:?}", call(&mut d, "daw_status", &[]));
    let mut last = vec![];
    for _ in 0..600 {
        last = list(&call(&mut d, "daw_rvc_poll", &[]));
        if num(&last[0]) != 1.0 {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
    assert_eq!(num(&last[0]), 2.0, "the conversion finished and was applied: {last:?}");
    let now = st(&call(&mut d, "daw_clip_file", &[n(t), n(c)]));
    assert!(now.contains("rvc_") && now.contains("LingLin"), "the clip plays the converted voice: {now}");
    assert_eq!(st(&call(&mut d, "daw_rvc_original", &[n(t), n(c)])), path);
    let out = ling_daw::model::load_sample(&now).unwrap();
    assert!((out.seconds() - 2.0).abs() < 0.15, "same length: {}", out.seconds());
    assert!(out.stereo.iter().any(|x| x.abs() > 0.02), "not silent");
    assert!(matches!(call(&mut d, "daw_rvc_restore", &[n(t), n(c)]), Val::Num(x) if x > 0.5));
    assert_eq!(st(&call(&mut d, "daw_clip_file", &[n(t), n(c)])), path, "restored");
}
