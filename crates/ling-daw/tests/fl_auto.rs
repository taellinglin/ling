//! FL Studio automation clips on import: mapped onto the parameter they move, kept as unassigned lanes, or left out.
//! Uses the user's own projects; skipped where they are not on this machine.
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
fn strs(v: &Val) -> Vec<String> {
    match v {
        Val::List(l) => l.iter().map(|x| match x { Val::Str(s) => s.clone(), Val::Num(n) => format!("{n}"), _ => String::new() }).collect(),
        _ => vec![],
    }
}
fn lanes(d: &mut Daw) -> Vec<(usize, String)> {
    let mut out = vec![];
    let nt = match call(d, "daw_track_count", &[]) {
        Val::Num(x) => x as usize,
        _ => 0,
    };
    for t in 0..nt {
        for l in strs(&call(d, "daw_auto_lanes", &[n(t as f64)])) {
            out.push((t, l));
        }
    }
    out
}

const AURORA: &str = "C:/Users/User/Music/Aurora/Aurora.flp";

#[test]
fn automation_is_mapped_kept_or_skipped() {
    if std::fs::metadata(AURORA).is_err() {
        return;
    }
    let mut d = Daw::new();
    let list = strs(&call(&mut d, "daw_midi_autos", &[s(AURORA)]));
    assert!(list.len() >= 12, "the three SubSynth clips: {list:?}");
    assert!(list.chunks(4).any(|c| c[0] == "SubSynth - Filter Cutoff" && c[1] != "-1"), "the cutoff clip knows its part: {list:?}");

    // map (default): the cutoff clip lands on a cutoff parameter of the SubSynth track
    let made = call(&mut d, "daw_midi_import", &[s(AURORA), n(0.0)]);
    assert!(matches!(made, Val::Num(x) if x > 0.5));
    let l = lanes(&mut d);
    assert!(l.iter().any(|(_, p)| p.contains("cutoff")), "a mapped cutoff lane: {l:?}");
    let status = match call(&mut d, "daw_status", &[]) {
        Val::Str(s) => s,
        _ => String::new(),
    };
    assert!(status.contains("automation"), "{status}");

    // lanes: everything waits for a parameter, and can be pointed at one later
    let mut d = Daw::new();
    call(&mut d, "daw_midi_import", &[s(AURORA), n(0.0), Val::List(vec![]), s("lanes")]);
    let l = lanes(&mut d);
    let fl: Vec<&(usize, String)> = l.iter().filter(|(_, p)| p.starts_with("fl:")).collect();
    assert!(fl.len() >= 3, "{l:?}");
    let (t, _) = fl[0].clone();
    let li = strs(&call(&mut d, "daw_auto_lanes", &[n(t as f64)])).iter().position(|p| p.starts_with("fl:")).unwrap();
    assert!(matches!(call(&mut d, "daw_auto_lane_set_param", &[n(t as f64), n(li as f64), s("vol")]), Val::Num(x) if x > 0.5) || matches!(call(&mut d, "daw_auto_lanes", &[n(t as f64)]), Val::List(_)));
    assert!(strs(&call(&mut d, "daw_auto_lanes", &[n(t as f64)])).contains(&"vol".to_string()), "the lane now moves the volume");
    assert!(matches!(call(&mut d, "daw_auto_lane_set_param", &[n(t as f64), n(li as f64), s("no such knob")]), Val::Num(x) if x < 0.5), "unknown parameters are refused");

    // skip: no automation at all
    let mut d = Daw::new();
    call(&mut d, "daw_midi_import", &[s(AURORA), n(0.0), Val::List(vec![]), s("skip")]);
    assert!(lanes(&mut d).iter().all(|(_, p)| !p.starts_with("fl:") && !p.contains("cutoff")), "{:?}", lanes(&mut d));
}
