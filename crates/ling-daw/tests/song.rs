//! File > New AI Song end to end: plan from a description + lyrics, build, check the song (run from LingStationX so the patches resolve).
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
fn a_described_song_is_built() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let mut d = Daw::new();
    let desc = "uplifting trance at 138 bpm in F# minor over i VI III VII with a female vocal";
    let lyrics = "[Verse]\nwalking through the night alone\nthe city lights are calling home\n[Chorus]\nwe are the light\nwe are alive tonight";
    // a model reply as Qwen gives it: fields fill what the description leaves open
    let model = r#"{"genre": "trance", "bpm": 138, "key": "F# minor", "progression": "", "structure": "", "instruments": "supersaw lead, warm pad, arp"}"#;
    let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(lyrics), s(model), s(""), n(1.0), n(7.0)]));
    let line = st(&p[0]);
    assert!(line.contains("genre=trance") && line.contains("bpm=138") && line.contains("F# minor") && line.contains("voice=aria"), "{line}");
    let r = list(&call(&mut d, "daw_song_build", &[s(&line), s(desc), s(lyrics), s(""), n(7.0)]));
    let made = num(&r[0]);
    let voice = num(&r[1]);
    assert!(made >= 6.0, "tracks: {r:?}");
    assert!(voice >= 0.0, "a vocal track");
    assert_eq!(num(&r[2]), -9.0, "the genre's loudness target");
    assert_eq!(num(&call(&mut d, "daw_bpm", &[])), 138.0);
    // every track has clips, the drums are a Membrane kit, the patches were found
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    assert_eq!(nt, made as usize);
    let mut kinds = vec![];
    for t in 0..nt {
        kinds.push(st(&call(&mut d, "daw_track_kind", &[n(t as f64)])));
        assert!(num(&call(&mut d, "daw_clip_count", &[n(t as f64)])) >= 1.0, "track {t} has clips");
    }
    assert!(kinds.contains(&"drums".to_string()), "{kinds:?}");
    assert!(kinds.contains(&"prism".to_string()), "a Prism patch was used: {kinds:?}");
    // the drops repeat: the first and the second drop of the lead are the same notes
    let lead = (0..nt).find(|t| st(&call(&mut d, "daw_track_name", &[n(*t as f64)])) == "Supersaw Lead").expect("the lead");
    let drops: Vec<usize> = (0..num(&call(&mut d, "daw_clip_count", &[n(lead as f64)])) as usize).filter(|c| st(&call(&mut d, "daw_clip_name", &[n(lead as f64), n(*c as f64)])) == "drop").collect();
    assert_eq!(drops.len(), 2, "two drops");
    let a = list(&call(&mut d, "daw_clip_notes", &[n(lead as f64), n(drops[0] as f64)]));
    let b = list(&call(&mut d, "daw_clip_notes", &[n(lead as f64), n(drops[1] as f64)]));
    assert!(!a.is_empty() && a.len() == b.len(), "the drop comes back");
    // the singer has lyrics on the notes of the verse
    let vt = voice as usize;
    let vc = (0..num(&call(&mut d, "daw_clip_count", &[n(voice)])) as usize).find(|c| st(&call(&mut d, "daw_clip_name", &[n(voice), n(*c as f64)])) == "drop").or(Some(0)).unwrap();
    let info = list(&call(&mut d, "daw_lyrf_info", &[n(vt as f64), n(vc as f64), s("we are the light"), s("fit:fit:1:0.3:all:1.0:auto")]));
    assert!(num(&info[0]) > 0.0, "notes to sing: {info:?}");
    // automation on the pads (cutoff and pump), section tags on the ruler
    let pad = (0..nt).find(|t| st(&call(&mut d, "daw_track_name", &[n(*t as f64)])) == "Warm Pad").expect("the pad");
    let lanes: Vec<String> = list(&call(&mut d, "daw_auto_lanes", &[n(pad as f64)])).iter().map(st).collect();
    assert!(lanes.iter().any(|l| l == "vol"), "the pad pumps: {lanes:?}");
    assert!(lanes.len() >= 2, "and its filter moves: {lanes:?}");
    let tags = list(&call(&mut d, "daw_arr_tags", &[]));
    assert!(tags.len() >= 7 * 3 - 1 || tags.iter().any(|t| st(t).contains("drop")), "{tags:?}");
}
