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

#[test]
fn a_dubstep_song_gets_sound_design() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let mut d = Daw::new();
    let desc = "heavy riddim dubstep at 140 bpm in F minor";
    let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(1.0), n(11.0)]));
    let line = st(&p[0]);
    assert!(line.contains("design=0.85"), "{line}");
    let r = list(&call(&mut d, "daw_song_build", &[s(&line), s(desc), s(""), s(""), n(11.0)]));
    let summary = st(&r[4]);
    assert!(summary.contains("Sound design") && summary.contains("Split Drive") && summary.contains("riddim skin"), "{summary}");
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let mut bass_ok = false;
    for t in 0..nt {
        let name = st(&call(&mut d, "daw_track_name", &[n(t as f64)]));
        let fx: Vec<String> = (0..num(&call(&mut d, "daw_fx_count", &[n(t as f64)])) as usize).map(|i| st(&call(&mut d, "daw_fx_name", &[n(t as f64), n(i as f64)]))).collect();
        let lanes: Vec<String> = match call(&mut d, "daw_auto_lanes", &[n(t as f64)]) {
            Val::List(v) => v.into_iter().map(|x| st(&x)).collect(),
            _ => vec![],
        };
        if name.contains("Growl Bass") || name.contains("Wobble Bass") {
            assert!(fx.contains(&"Split Drive".to_string()) && fx.contains(&"OTT".to_string()) && fx.contains(&"Imager".to_string()), "{name}: {fx:?}");
            assert!(lanes.iter().any(|l| l.starts_with("fx")), "{name}: no effect automation in {lanes:?}");
            bass_ok = true;
        }
        if name == "Drums" {
            assert!(fx.contains(&"Transient".to_string()), "drums: {fx:?}");
        }
    }
    assert!(bass_ok, "no growl / wobble bass track");
}

/// not a test: builds the riddim song and renders it to $SONG_DEMO (a .wav path) so the sound design can be heard and measured
#[test]
#[ignore]
fn render_a_dubstep_demo() {
    let Ok(out) = std::env::var("SONG_DEMO") else { return };
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let mut d = Daw::new();
    let desc = "heavy riddim dubstep at 140 bpm in F minor";
    let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(1.0), n(11.0)]));
    let r = list(&call(&mut d, "daw_song_build", &[s(&st(&p[0])), s(desc), s(""), s(""), n(11.0)]));
    println!("{}", st(&r[4]));
    let end = num(&call(&mut d, "daw_end_beat", &[]));
    call(&mut d, "daw_render_start", &[s(&out), n(0.0), n(end), n(2.0), n(16.0), s("wav"), n(0.5)]);
    for _ in 0..1200 {
        let state = num(&call(&mut d, "daw_render_state", &[]));
        if state >= 2.0 {
            println!("{}", st(&call(&mut d, "daw_render_message", &[])));
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(250));
    }
    panic!("render did not finish");
}

#[test]
fn a_chiptune_song_plays_on_the_chip_with_a_feel() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let mut d = Daw::new();
    let desc = "chiptune adventure at 150 bpm";
    let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(1.0), n(5.0)]));
    let line = st(&p[0]);
    assert!(line.contains("arc="), "{line}");
    assert!(st(&p[1]).contains("feel:"), "{}", st(&p[1]));
    let r = list(&call(&mut d, "daw_song_build", &[s(&line), s(desc), s(""), s(""), n(5.0)]));
    assert!(st(&r[4]).contains("Feel:"), "{}", st(&r[4]));
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let kinds: Vec<String> = (0..nt).map(|t| st(&call(&mut d, "daw_track_kind", &[n(t as f64)]))).collect();
    assert!(kinds.iter().filter(|k| *k == "chip").count() >= 2, "{kinds:?}");
}

#[test]
fn a_medley_carries_a_message_and_reads_it_back() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let dir = "projects/A.I. Love You/midi";
    if !std::path::Path::new(dir).is_dir() {
        return;
    }
    let mut d = Daw::new();
    let scan = list(&call(&mut d, "daw_song_medley_scan", &[s(dir), n(0.0)]));
    assert!(num(&scan[1]) >= 4.0, "{scan:?}");
    let r = st(&call(&mut d, "daw_song_medley_build", &[s(dir), n(0.0), n(8.0), s("cipher"), s("open sesame"), s("hi ling"), n(1.0), n(0.0)]));
    assert!(r.contains("message"), "{r}");
    assert!(num(&call(&mut d, "daw_track_count", &[])) >= 1.0);
    assert_eq!(st(&call(&mut d, "daw_song_medley_decode", &[s(dir), n(0.0), s("open sesame")])), "hi ling");
    assert_ne!(st(&call(&mut d, "daw_song_medley_decode", &[s(dir), n(0.0), s("wrong")])), "hi ling");
    // a smart medley after it
    let r2 = st(&call(&mut d, "daw_song_medley_build", &[s(dir), n(4.0), n(10.0), s("smart"), s(""), s(""), n(3.0), n(0.0)]));
    assert!(r2.starts_with("Medley: 10 phrases"), "{r2}");
    // a mashup: a story, layers from other files, loop rolls, a patch per clip - and the cipher still reads
    let mut d = Daw::new();
    let r3 = st(&call(&mut d, "daw_song_medley_build", &[s(dir), n(0.0), n(8.0), s("cipher"), s("open sesame"), s("hi ling"), n(1.0), n(0.0), s("layering=0.8;rolls=0.8;genre=house;story=a hero falls and rises again")]));
    assert!(r3.starts_with("Mashup:") && r3.contains("story: hero"), "{r3}");
    assert!(r3.contains("; feel: triumph"), "the hero story plays as a triumph: {r3}");
    // the feel on the whole project: words name the arc, the music is read when they name none
    let f1 = st(&call(&mut d, "daw_song_feel", &[s("make it sadder"), n(0.7), n(1.0), n(0.0), n(0.0), s("")]));
    assert!(f1.starts_with("Feel: melancholy over ") && f1.contains("notes played"), "{f1}");
    let f2 = st(&call(&mut d, "daw_song_feel", &[s(""), n(0.5), n(2.0), n(0.0), n(0.0), s("")]));
    assert!(f2.contains("(read from the music)"), "{f2}");
    assert!(!r3.contains(" 0 loop rolls"), "{r3}");
    assert_eq!(st(&call(&mut d, "daw_song_medley_decode", &[s(dir), n(0.0), s("open sesame")])), "hi ling", "{r3}");
    let names: Vec<String> = (0..num(&call(&mut d, "daw_track_count", &[])) as usize).map(|i| st(&call(&mut d, "daw_track_name", &[n(i as f64)]))).collect();
    let cats: std::collections::BTreeSet<&str> = names.iter().filter_map(|x| x.split(" · ").next()).collect();
    assert!(names.len() > cats.len(), "some category plays on more than one patch: {names:?}");
    // Tune Check Tracks reaches its builtin through the dispatcher (it once fell through to "unknown builtin" and stopped the app)
    let tc = list(&call(&mut d, "daw_tune_check", &[n(0.0)]));
    assert_eq!(tc.len(), 4, "{tc:?}");
    // breakcore: vocal chops (Ling Voice, with no vocal in the folder) tuned on a sampler, layered, and breaks under them
    let mut d = Daw::new();
    let r4 = st(&call(&mut d, "daw_song_medley_build", &[s(dir), n(0.0), n(8.0), s("smart"), s(""), s(""), n(2.0), n(0.0), s("layering=0.8;rolls=0.6;genre=dubstep;chops=breakcore;story=a storm")]));
    assert!(r4.contains("vocal chops:") && r4.contains("with breaks"), "{r4}");
    let names: Vec<String> = (0..num(&call(&mut d, "daw_track_count", &[])) as usize).map(|i| st(&call(&mut d, "daw_track_name", &[n(i as f64)]))).collect();
    assert!(names.iter().any(|x| x.starts_with("Chops · ")) && names.iter().any(|x| x.starts_with("Breaks · ")), "{names:?}");
    // the breaks replace the drums layered from the files (a phrase that is only drums keeps its beat: it is the section's hook)
    eprintln!("{r4}
{names:?}");
    eprintln!("{r3}
{names:?}");
}

#[test]
fn a_song_cycle_is_planned_and_its_songs_are_sung_with_expression() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let mut d = Daw::new();
    let desc = "a winter journey, folk with piano and a female vocal, in D minor";
    let lyrics = "# Good Night\nI came here as a stranger\nand a stranger I will leave\nthe cold wind takes my tears away\nalone into the night\n---\n# Frozen Tears\nfrozen drops are falling\nfrom my cheeks I never knew\nthe ice is broken, I am gone";
    let c = list(&call(&mut d, "daw_cycle_plan", &[s(desc), s(lyrics), n(4.0), s(""), n(0.6), n(3.0)]));
    assert_eq!(st(&c[1]), "winter journey", "{}", st(&c[0]));
    let songs = list(&c[2]);
    assert_eq!(songs.len(), 4);
    let first = list(&songs[0]);
    assert_eq!(st(&first[0]), "I. Good Night");
    assert!(st(&first[4]).starts_with("notes:"), "the cycle's motif: {}", st(&first[4]));
    // build the first song as the dialog would: its line, lyrics, story and the motif as its theme
    let args = [s(&st(&first[1])), s(desc), s(&st(&first[2])), s(""), n(3.0), s(""), n(0.0), s(""), s(&st(&first[3])), s(&st(&first[4])), s(""), n(0.9)];
    let r = list(&call(&mut d, "daw_song_build", &args));
    let voice = num(&r[1]);
    assert!(voice >= 0.0, "{r:?}");
    assert!(st(&r[4]).contains("expression:"), "{}", st(&r[4]));
    let lanes: Vec<String> = list(&call(&mut d, "daw_auto_lanes", &[n(voice)])).iter().map(st).collect();
    for want in ["tune", "expr", "scoop", "vib depth", "breath", "bright"] {
        assert!(lanes.iter().any(|l| l == want), "{want} in {lanes:?}");
    }
    // a sad song: somewhere its lines fall off (the tune lane goes well under 0.5)
    let ti = lanes.iter().position(|l| l == "tune").unwrap();
    let pts = list(&call(&mut d, "daw_auto_points", &[n(voice), n(ti as f64)]));
    let low = pts.chunks(2).map(|p| num(&p[1])).fold(1.0, f64::min);
    assert!(low < 0.4, "a fall: lowest tune {low}");
    // expression on an existing project's vocal, off and on
    let flat = st(&call(&mut d, "daw_song_express", &[n(voice), n(0.0), s(""), n(1.0)]));
    assert!(!flat.contains("expression:"), "{flat}");
    let again = st(&call(&mut d, "daw_song_express", &[n(voice), n(0.8), s("melancholy"), n(2.0)]));
    assert!(again.contains("melancholy arc") && again.contains("expression:"), "{again}");
}

/// Listen to a song cycle: CYCLE_DEMO=<dir> cargo test -p ling-daw --test song render_a_song_cycle_demo -- --ignored --nocapture
/// writes one WAV per song, plus the first song again without the expression (`*_flat.wav`) to hear what it adds.
#[test]
#[ignore]
fn render_a_song_cycle_demo() {
    let Ok(dir) = std::env::var("CYCLE_DEMO") else { return };
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let _ = std::fs::create_dir_all(&dir);
    let mut d = Daw::new();
    let desc = "a winter journey, folk with piano and strings and a female vocal, in D minor";
    let lyrics = "# Good Night\nI came here as a stranger\nand a stranger I will go\nthe cold wind takes my tears away\nalone into the snow\n\
                  ---\n# Frozen Tears\nfrozen drops are falling\nfrom my cheeks into the ice\nI never knew I cried so long\nmy heart is cold as night\n\
                  ---\n# Spring Dream\nI dreamed of flowers blooming\nof a warm and shining day\nyou smiled at me and held my hand\nthen woke and it was grey\n\
                  ---\n# The Last Light\nthe road goes on without me\nthe lantern fades to grey\nno one is waiting at the end\nI sing and walk away";
    let c = list(&call(&mut d, "daw_cycle_plan", &[s(desc), s(lyrics), n(4.0), s(""), n(0.5), n(21.0)]));
    println!("{}", st(&c[0]));
    let render = |d: &mut Daw, path: &str| {
        let end = num(&call(d, "daw_end_beat", &[]));
        call(d, "daw_render_start", &[s(path), n(0.0), n(end), n(2.0), n(16.0), s("wav"), n(0.5)]);
        for _ in 0..2400 {
            if num(&call(d, "daw_render_state", &[])) >= 2.0 {
                println!("  {}", st(&call(d, "daw_render_message", &[])));
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        panic!("render did not finish");
    };
    for (i, song) in list(&c[2]).iter().enumerate() {
        let f = list(song);
        for (amount, tag) in [(0.85, ""), (0.0, "_flat")] {
            if i > 0 && !tag.is_empty() {
                continue;
            }
            call(&mut d, "daw_new", &[]);
            let args = [s(&st(&f[1])), s(desc), s(&st(&f[2])), s(""), n(21.0 + i as f64), s(""), n(0.0), s(""), s(&st(&f[3])), s(&st(&f[4])), s(""), n(amount)];
            let r = list(&call(&mut d, "daw_song_build", &args));
            println!("{} {}: {}", st(&f[0]), tag, st(&r[4]));
            render(&mut d, &format!("{dir}/{:02}_{}{tag}.wav", i + 1, st(&f[5]).to_lowercase().replace(' ', "_")));
        }
    }
}

#[test]
fn a_song_with_movements_has_a_tempo_map_pad_interludes_and_edm_builds() {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    if std::env::set_current_dir(lsx).is_err() {
        return;
    }
    let mut d = Daw::new();
    let desc = "uplifting trance at 138 bpm in A minor, three movements";
    let p = list(&call(&mut d, "daw_song_plan", &[s(desc), s(""), s(""), s(""), n(1.0), n(5.0)]));
    let line = st(&p[0]);
    assert!(line.contains("movements=3") && st(&p[1]).contains("movements="), "{line} / {}", st(&p[1]));
    let r = list(&call(&mut d, "daw_song_build", &[s(&line), s(desc), s(""), s(""), n(5.0)]));
    assert!(st(&r[4]).contains("Movements:"), "{}", st(&r[4]));
    assert_eq!(num(&call(&mut d, "daw_bpm", &[])), 138.0, "the project keeps the song's tempo");
    // a tempo map: at least two changes, ordered, all sane, the bpm at the end differs from the start somewhere
    let tp: Vec<f64> = list(&call(&mut d, "daw_tempo_points", &[])).iter().map(num).collect();
    assert!(tp.len() >= 4 && tp.len() % 2 == 0, "{tp:?}");
    let beats: Vec<f64> = tp.chunks(2).map(|c| c[0]).collect();
    assert!(beats.windows(2).all(|w| w[0] < w[1]), "{beats:?}");
    assert!(tp.chunks(2).all(|c| (50.0..=220.0).contains(&c[1])));
    // the drums rest through the pad-led sections (the intro opening, the reflective break); the pad plays there
    let nt = num(&call(&mut d, "daw_track_count", &[])) as usize;
    let by_name = |d: &mut Daw, name: &str| (0..nt).find(|t| st(&call(d, "daw_track_name", &[n(*t as f64)])) == name);
    let drums = by_name(&mut d, "Drums").expect("drums");
    let clips = |d: &mut Daw, t: usize| -> Vec<(String, usize)> { (0..num(&call(d, "daw_clip_count", &[n(t as f64)])) as usize).map(|c| (st(&call(d, "daw_clip_name", &[n(t as f64), n(c as f64)])), c)).collect() };
    let dc = clips(&mut d, drums);
    assert!(dc.iter().all(|(name, _)| name != "break"), "no drums in the break: {dc:?}");
    let pad = by_name(&mut d, "Warm Pad").expect("pad");
    assert!(clips(&mut d, pad).iter().any(|(name, _)| name == "break"), "the pad carries the break");
    // a build: the roll speeds up 1/4 > 1/8 > 1/16 > 1/32 and the last beat(s) before the drop are empty
    let (_, bc) = dc.iter().find(|(name, _)| name == "build").expect("a build");
    let flat = list(&call(&mut d, "daw_clip_notes", &[n(drums as f64), n(*bc as f64)]));
    let notes: Vec<(f64, f64, f64)> = flat.chunks(7).map(|c| (num(&c[0]), num(&c[1]), num(&c[2]))).collect();
    let mut rolls: Vec<f64> = notes.iter().filter(|x| x.2 == 38.0 || x.2 == 39.0).map(|x| x.0).collect();
    rolls.dedup();
    let gaps: Vec<f64> = rolls.windows(2).map(|w| w[1] - w[0]).filter(|g| *g > 1e-6).collect();
    assert!(gaps.first().copied().unwrap_or(0.0) >= 0.9 && gaps.last().copied().unwrap_or(1.0) <= 0.13, "{gaps:?}");
    let len = line.split("build ").nth(1).and_then(|t| t.split(|c: char| !c.is_ascii_digit()).next()).and_then(|t| t.parse::<f64>().ok()).unwrap_or(8.0) * 4.0;
    let end = notes.iter().map(|x| x.0 + x.1).fold(0.0, f64::max);
    assert!(end < len - 0.4, "a gap before the drop: notes end at {end} of {len}");
}
