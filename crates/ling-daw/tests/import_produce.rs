//! The import, produced: a library MIDI imported in memory with the Nova-aware choices (daw_ai_choose with no model reply), then
//! `daw_imp_produce` with every stage on, then Mix AI - the tracks, their Nova kinds, the sections, the ducks and the master chain
//! are checked (run from LingStationX so the patches and the library resolve).
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
fn at_lsx() -> bool {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    std::env::set_current_dir(lsx).is_ok() && std::path::Path::new("instruments/Nova").is_dir() && std::path::Path::new("library/LingAITune.mid").is_file()
}

/// the dialog's choice for a pick: what the import catalogue says the pick's label plays (a Nova preset's patch file, a kit's file ...),
/// else Auto (soundfont / GM)
fn choice_for(d: &mut Daw, plugin: &str, patch: &str) -> String {
    let v = st(&call(d, "daw_imp_choice", &[s(plugin), s(patch)]));
    if v.is_empty() { "auto".into() } else { v }
}

#[test]
fn a_library_midi_is_imported_as_dubstep_and_produced() {
    if !at_lsx() {
        return;
    }
    let path = "library/LingAITune.mid";
    let mut d = Daw::new();
    let parts = list(&call(&mut d, "daw_midi_scan", &[s(path)]));
    let np = parts.len() / 8;
    assert!(np >= 2, "{parts:?}");
    // the picker with no model answer: guessed roles, Nova presets rolled by the seed
    let picks = list(&call(&mut d, "daw_ai_choose", &[s(path), s(""), s("dubstep"), n(3.0)]));
    let mut choices = vec![s("auto"); np];
    let mut novas = 0;
    for c in picks.chunks(4) {
        let pi = num(&c[0]) as usize;
        if pi < np {
            let ch = choice_for(&mut d, &st(&c[1]), &st(&c[2]));
            novas += ch.contains("Nova") as usize;
            choices[pi] = s(&ch);
        }
    }
    assert!(novas > 0, "dubstep picks no Nova preset: {picks:?}");
    let made = num(&call(&mut d, "daw_midi_import", &[s(path), n(0.0), Val::List(choices)]));
    assert!(made >= 2.0, "{made}");
    let nt0 = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let kinds: Vec<String> = (0..nt0).map(|t| st(&call(&mut d, "daw_track_kind", &[n(t as f64)]))).collect();
    assert!(kinds.iter().any(|k| k == "nova"), "{kinds:?}");

    let rep = st(&call(&mut d, "daw_imp_produce", &[n(0.0), n(nt0 as f64), s("dubstep"), s("perf=live player;amt=0.7;seed=3;analyse=1;clean=1;roles=1;nova=1;duck=1;master=1")]));
    println!("report: {rep}");
    for w in ["Produced as dubstep", "analysed:", "performance: live player", "roles:", "master: Ling Master \"Bass Music Slam\""] {
        assert!(rep.contains(w), "missing {w:?} in {rep}");
    }
    assert!(st(&call(&mut d, "daw_meta_get", &[s("sections")])).contains(':'), "sections tagged on the ruler");
    assert_eq!(st(&call(&mut d, "daw_meta_get", &[s("ai.genre")])), "dubstep");
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let kinds: Vec<String> = (0..nt).map(|t| st(&call(&mut d, "daw_track_kind", &[n(t as f64)]))).collect();
    println!("tracks: {:?}", (0..nt).map(|t| format!("{} [{}]", st(&call(&mut d, "daw_track_name", &[n(t as f64)])), kinds[t])).collect::<Vec<_>>());
    // the master chain ends in the Ling Master
    let nfx = num(&call(&mut d, "daw_fx_count", &[n(-1.0)])) as usize;
    let master: Vec<String> = (0..nfx).map(|i| st(&call(&mut d, "daw_fx_name", &[n(-1.0), n(i as f64)]))).collect();
    assert_eq!(master.last().map(String::as_str), Some("Ling Master"), "{master:?}");
    // dubstep pumps: something ducks under the kick when the file has one
    if kinds.iter().any(|k| k == "drums") && rep.contains("ducking:") && !rep.contains("no kick") {
        assert!(num(&call(&mut d, "daw_route_count", &[])) >= 1.0, "{rep}");
    }
    // the same call again is harmless (nothing left to clean, the Ling Master kept)
    let again = st(&call(&mut d, "daw_imp_produce", &[n(0.0), n(nt0 as f64), s("dubstep"), s("clean=1;master=1")]));
    assert!(again.contains("nothing stuck") && again.contains("kept the Ling Master"), "{again}");

    // Mix AI after it, as the dialog runs it (a short analysis)
    let ms = list(&call(&mut d, "daw_imp_mix_style", &[s("dubstep")]));
    call(&mut d, "daw_mix_style", &[n(0.0), ms[0].clone(), ms[1].clone(), ms[2].clone()]);
    assert_eq!(num(&call(&mut d, "daw_mix_start", &[n(6.0)])), 1.0, "the mix did not start");
    let t0 = std::time::Instant::now();
    loop {
        let p = list(&call(&mut d, "daw_mix_poll", &[]));
        if num(&p[1]) > 0.5 {
            assert_eq!(st(&p[2]), "", "the mix failed");
            break;
        }
        assert!(t0.elapsed().as_secs() < 900, "the mix never finished");
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    let plan = list(&call(&mut d, "daw_mix_plan", &[]));
    let applied = num(&call(&mut d, "daw_mix_apply", &[s("")]));
    println!("mix: {} plan entries, {applied} applied", plan.len() / 6);
    assert!(applied >= 1.0);
}

#[test]
fn a_jazz_import_keeps_its_acoustic_sounds_and_a_gentle_master() {
    if !at_lsx() {
        return;
    }
    let path = "library/LingAITune.mid";
    let mut d = Daw::new();
    let picks = list(&call(&mut d, "daw_ai_choose", &[s(path), s(""), s("jazz"), n(3.0)]));
    assert!(picks.chunks(4).all(|c| st(&c[1]) != "nova"), "jazz picked Nova: {picks:?}");
    let made = num(&call(&mut d, "daw_midi_import", &[s(path), n(0.0)]));
    assert!(made >= 1.0);
    let rep = st(&call(&mut d, "daw_imp_produce", &[n(0.0), n(0.0), s("jazz"), s("perf=laid back;analyse=1;duck=1;master=1")]));
    assert!(rep.contains("Gentle Glue") && rep.contains("ducking: none") || rep.contains("no kick"), "{rep}");
    // the performance without Produce: only the style
    let only = st(&call(&mut d, "daw_imp_produce", &[n(0.0), n(0.0), s("jazz"), s("perf=robotic;amt=1")]));
    assert!(only.contains("performance: robotic") && !only.contains("master"), "{only}");
    assert_eq!(list(&call(&mut d, "daw_imp_perf_styles", &[])).len(), 8);
}
