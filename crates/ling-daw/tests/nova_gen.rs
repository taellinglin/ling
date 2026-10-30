//! Ling Nova in the generators: a generated EDM song and an EDM medley play their electronic parts on Nova presets, with the
//! macros moving differently from section to section (run from LingStationX so the patches resolve).
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
    std::env::set_current_dir(lsx).is_ok() && std::path::Path::new("instruments/Nova").is_dir()
}

/// (track, lane name, points) of every macro lane on a Nova track
fn nova_macro_lanes(d: &mut Daw) -> Vec<(usize, String, Vec<(f64, f64)>)> {
    let nt = num(&call(d, "daw_track_count", &[])) as usize;
    let mut out = vec![];
    for t in 0..nt {
        if st(&call(d, "daw_track_kind", &[n(t as f64)])) != "nova" {
            continue;
        }
        let lanes: Vec<String> = list(&call(d, "daw_auto_lanes", &[n(t as f64)])).iter().map(st).collect();
        for (li, l) in lanes.iter().enumerate() {
            if l.len() == 2 && l.starts_with('m') {
                let p = list(&call(d, "daw_auto_points", &[n(t as f64), n(li as f64)]));
                out.push((t, l.clone(), p.chunks(2).map(|c| (num(&c[0]), num(&c[1]))).collect()));
            }
        }
    }
    out
}

/// does a lane take clearly different values somewhere?
fn moves(points: &[(f64, f64)]) -> bool {
    let (lo, hi) = points.iter().fold((1.0f64, 0.0f64), |(a, b), p| (a.min(p.1), b.max(p.1)));
    hi - lo > 0.3
}

#[test]
fn edm_songs_play_nova_with_macros_moving_by_section() {
    if !at_lsx() {
        return;
    }
    for (desc, seed) in [("heavy dubstep at 140 bpm with a big drop", 3.0), ("future bass at 150 bpm, bright and emotional", 5.0), ("bass house at 124 bpm", 2.0), ("drum and bass at 174 bpm", 4.0)] {
        let mut d = Daw::new();
        let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(1.0), n(seed)]));
        let line = st(&p[0]);
        let r = list(&call(&mut d, "daw_song_build", &[s(&line), s(desc), s(""), s(""), n(seed)]));
        assert!(num(&r[0]) >= 4.0, "{desc}: {r:?}");
        let lanes = nova_macro_lanes(&mut d);
        assert!(!lanes.is_empty(), "{desc}: no Nova track with macro lanes ({line})");
        assert!(lanes.iter().any(|(_, _, p)| moves(p)), "{desc}: the macros never move");
    }
}

#[test]
fn a_dubstep_song_trades_drop_bars_between_nova_basses() {
    if !at_lsx() {
        return;
    }
    let mut d = Daw::new();
    let desc = "riddim dubstep at 140 bpm";
    let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(1.0), n(9.0)]));
    let line = st(&p[0]);
    let _ = call(&mut d, "daw_song_build", &[s(&line), s(desc), s(""), s(""), n(9.0)]);
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let names: Vec<String> = (0..nt).map(|t| st(&call(&mut d, "daw_track_name", &[n(t as f64)]))).collect();
    let sibs: Vec<&String> = names.iter().filter(|x| x.contains(" · ")).collect();
    assert!(!sibs.is_empty(), "sibling presets for the switching: {names:?}");
}

#[test]
fn an_edm_medley_uses_nova() {
    let midi = "projects/A.I. Love You/midi";
    if !at_lsx() || !std::path::Path::new(midi).is_dir() {
        return;
    }
    let mut d = Daw::new();
    let _ = call(&mut d, "daw_song_medley_build", &[s(midi), n(4.0), n(8.0), s("smart"), s(""), s(""), n(5.0), n(0.0), s("genre=dubstep")]);
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let kinds: Vec<String> = (0..nt).map(|t| st(&call(&mut d, "daw_track_kind", &[n(t as f64)]))).collect();
    assert!(kinds.iter().any(|k| k == "nova"), "{kinds:?}");
    let lanes = nova_macro_lanes(&mut d);
    assert!(lanes.iter().any(|(_, _, p)| moves(p)), "the medley moves the Nova macros");
}

/// build a song from a description (the plan, then the build); returns the summary
fn song(d: &mut Daw, desc: &str, seed: f64, length: f64) -> String {
    let p = list(&call(d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(length), n(seed)]));
    let r = list(&call(d, "daw_song_build", &[s(&st(&p[0])), s(desc), s(""), s(""), n(seed)]));
    st(&r[4])
}

/// the project's sections from the ruler's tags
fn sections(d: &Daw) -> Vec<ling_daw::sounddesign::Sec> {
    let eng = d.engine();
    let en = eng.lock().unwrap();
    let text = en.project.meta.iter().find(|(k, _)| k == "sections").map(|(_, v)| v.clone()).unwrap_or_default();
    text.split(';')
        .filter_map(|it| {
            let f: Vec<&str> = it.split(':').collect();
            (f.len() >= 3).then(|| ling_daw::sounddesign::Sec { tag: f[2].to_string(), start: f[0].parse().unwrap_or(0.0), end: f[1].parse().unwrap_or(0.0) })
        })
        .collect()
}

#[test]
fn a_dubstep_song_layers_one_sub_under_its_growls() {
    if !at_lsx() {
        return;
    }
    let mut d = Daw::new();
    let summary = song(&mut d, "heavy dubstep at 140 bpm with a big drop", 3.0, 1.0);
    assert!(summary.contains("song variant"), "{summary}");
    let eng = d.engine();
    let en = eng.lock().unwrap();
    let mut owners = vec![];
    let mut mids = 0;
    let mut top = false;
    for tr in en.project.tracks.iter().filter(|t| t.inst_kind == "nova") {
        let Some(p) = ling_daw::novagen::preset_of_patch(&tr.patch) else { continue };
        if p.usage.role != "bass" {
            continue;
        }
        let sub_on = tr.inst.as_ref().and_then(|i| i.get_param("sub on")).unwrap_or(0.0) > 0.5;
        if ling_daw::novagen::is_sub(p) || sub_on {
            owners.push(tr.name.clone());
            continue;
        }
        // every other bass is a mid or top layer: its lows cut by a 24 dB Param EQ band
        let eq = tr.effects.iter().find(|e| e.name == "Param EQ").unwrap_or_else(|| panic!("{}: no Param EQ", tr.name));
        let ty = eq.params.iter().find(|q| q.0 == "b1 type").map(|q| q.1).unwrap_or(0.0);
        assert!((ty - 6.5 / 8.0).abs() < 1e-3, "{}: b1 type {ty}", tr.name);
        mids += 1;
        top |= tr.name.starts_with("Top Layer");
    }
    assert_eq!(owners.len(), 1, "one track owns the sub: {owners:?}");
    assert!(owners[0].to_lowercase().contains("sub"), "{owners:?}");
    assert!(mids >= 2 && top, "mid layers and a top layer ({mids}, top {top})");
    // the sub layer plays one note at a time and ducks under the kick
    let sub = en.project.tracks.iter().find(|t| t.name.starts_with("Sub Layer")).expect("a sub layer");
    assert!(sub.effects.iter().any(|e| e.name == "Ducker"), "the sub ducks");
    let mut notes: Vec<(f64, f64)> = sub
        .clips
        .iter()
        .flat_map(|c| match &c.data {
            ling_daw::model::ClipData::Midi(v) => v.iter().map(|x| (c.start + x.start, c.start + x.start + x.len)).collect::<Vec<_>>(),
            _ => vec![],
        })
        .collect();
    notes.sort_by(|a, b| a.0.total_cmp(&b.0));
    assert!(notes.len() >= 4, "{notes:?}");
    assert!(notes.windows(2).all(|w| w[0].1 <= w[1].0 + 1e-6), "one sub note at a time");
}

#[test]
fn nova_lanes_in_built_songs_stay_inside_each_sections_budget() {
    if !at_lsx() {
        return;
    }
    for (desc, seed) in [("heavy dubstep at 140 bpm with a big drop", 3.0), ("future bass at 150 bpm, bright and emotional", 5.0)] {
        let mut d = Daw::new();
        let _ = song(&mut d, desc, seed, 1.0);
        let secs = sections(&d);
        assert!(secs.len() >= 4, "{desc}: {secs:?}");
        let eng = d.engine();
        let en = eng.lock().unwrap();
        let layout = ling_daw::dsp::nova::layout::layout();
        let mut checked = 0;
        for tr in en.project.tracks.iter().filter(|t| t.inst_kind == "nova") {
            let lanes: Vec<&ling_daw::model::AutoLane> = tr.auto.iter().filter(|l| layout.index.contains_key(l.param.as_str())).collect();
            for sec in &secs {
                let (most, cap) = ling_daw::novagen::budget(ling_daw::sounddesign::phase(&sec.tag));
                let spans: Vec<(String, f32)> = lanes.iter().map(|l| (l.param.clone(), ling_daw::novagen::moving_span(l, sec.start, sec.end))).collect();
                assert!(spans.iter().filter(|x| x.1 > 0.02).count() <= most, "{desc} {} {}: {spans:?}", tr.name, sec.tag);
                assert!(spans.iter().all(|x| x.1 <= cap + 1e-3), "{desc} {} {}: {spans:?}", tr.name, sec.tag);
            }
            checked += lanes.len();
        }
        assert!(checked >= 4, "{desc}: Nova lanes to check ({checked})");
    }
}

#[test]
fn different_seeds_play_different_nova_presets_and_the_plan_names_them() {
    if !at_lsx() {
        return;
    }
    let mut seen = std::collections::BTreeSet::new();
    for seed in 1..7 {
        let mut d = Daw::new();
        let p = list(&call(&mut d, "daw_song_plan", &[s("heavy dubstep at 140 bpm"), s(""), s(""), s(""), n(1.0), n(seed as f64)]));
        let notes = st(&p[1]);
        let nova = notes.split("; ").find(|x| x.starts_with("Nova presets:")).unwrap_or_else(|| panic!("seed {seed}: {notes}")).to_string();
        seen.insert(nova);
    }
    assert!(seen.len() >= 4, "{seen:#?}");
}

#[test]
fn songs_beyond_edm_play_only_nova_presets_that_fit() {
    if !at_lsx() {
        return;
    }
    for (desc, seed, want) in [("pop song at 112 bpm", 2.0, true), ("ambient drone", 2.0, true), ("synthwave at 104 bpm", 4.0, true), ("jazz trio at 110 bpm", 3.0, false)] {
        let mut d = Daw::new();
        let summary = song(&mut d, desc, seed, 0.5);
        let eng = d.engine();
        let en = eng.lock().unwrap();
        let genre = en.project.meta.iter().find(|(k, _)| k == "ai.genre").map(|(_, v)| v.clone()).unwrap_or_default();
        let nova: Vec<&ling_daw::model::Track> = en.project.tracks.iter().filter(|t| t.inst_kind == "nova").collect();
        assert_eq!(!nova.is_empty(), want, "{desc}: {:?} ({summary})", nova.iter().map(|t| &t.name).collect::<Vec<_>>());
        for tr in nova {
            let p = ling_daw::novagen::preset_of_patch(&tr.patch).unwrap();
            assert!(matches!(p.usage.role.as_str(), "pad" | "pluck" | "chord" | "lead"), "{desc} {}: {}", tr.name, p.name);
            let fits = ling_daw::songgen::GENRES.iter().find(|g| g.name == genre).map(|g| g.roles.iter().any(|r| ling_daw::novagen::palette(&genre, r).contains(&p.name.as_str()))).unwrap_or(false);
            assert!(fits, "{desc} ({genre}) {}: {} is not in the palette", tr.name, p.name);
        }
    }
}

/// not a test: short songs rendered for measuring (`NOVA_SONG_RENDERS=<dir>`): the mix as <name>.wav, the stems in <name>_stems/,
/// the track list in <name>.txt - measured, never played
#[test]
#[ignore]
fn render_nova_songs() {
    let Ok(dir) = std::env::var("NOVA_SONG_RENDERS") else { return };
    if !at_lsx() {
        return;
    }
    std::fs::create_dir_all(&dir).unwrap();
    let wait = |d: &mut Daw| {
        for _ in 0..6000 {
            if num(&call(d, "daw_render_state", &[])) >= 2.0 {
                let _ = call(d, "daw_render_ack", &[]);
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
        panic!("render did not finish");
    };
    let only = std::env::var("NOVA_SONG_ONLY").unwrap_or_default();
    for (name, desc, seed) in [
        ("dubstep", "heavy dubstep at 140 bpm in F minor with a big drop", 3.0),
        ("future_bass", "future bass at 150 bpm, bright and emotional", 5.0),
        ("bass_house", "bass house at 124 bpm", 2.0),
        ("dnb", "drum and bass at 174 bpm", 4.0),
        ("pop", "pop song at 112 bpm", 2.0),
        ("synthwave", "synthwave at 104 bpm", 4.0),
    ] {
        if !only.is_empty() && !only.split(',').any(|o| o == name) {
            continue;
        }
        let mut d = Daw::new();
        let summary = song(&mut d, desc, seed, 0.5);
        let end = num(&call(&mut d, "daw_end_beat", &[]));
        let mut info = format!("{desc}\n{summary}\nend beat {end}\n");
        {
            let eng = d.engine();
            let en = eng.lock().unwrap();
            info.push_str(&format!("bpm {}\nsections {}\n", en.project.bpm, en.project.meta.iter().find(|(k, _)| k == "sections").map(|(_, v)| v.clone()).unwrap_or_default()));
            for (i, tr) in en.project.tracks.iter().enumerate() {
                let preset = ling_daw::novagen::preset_of_patch(&tr.patch).map(|p| p.name.clone()).unwrap_or_default();
                let fx: Vec<&str> = tr.effects.iter().map(|e| e.name.as_str()).collect();
                info.push_str(&format!("{i}\t{}\t{}\t{preset}\tvol {:.2}\t{}\n", tr.name, tr.inst_kind, tr.volume, fx.join(",")));
            }
        }
        std::fs::write(format!("{dir}/{name}.txt"), info).unwrap();
        let mix = format!("{dir}/{name}.wav");
        assert_eq!(num(&call(&mut d, "daw_render_start", &[s(&mix), n(0.0), n(end), n(1.0), n(16.0), s("wav"), n(0.5)])), 1.0);
        wait(&mut d);
        let sd = format!("{dir}/{name}_stems");
        let _ = std::fs::remove_dir_all(&sd);
        std::fs::create_dir_all(&sd).unwrap();
        assert_eq!(num(&call(&mut d, "daw_render_start", &[s(&format!("{sd}/mix.wav")), n(0.0), n(end), n(1.0), n(16.0), s("wav"), n(0.5), n(1.0)])), 1.0);
        wait(&mut d);
    }
}
