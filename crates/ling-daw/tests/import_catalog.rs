//! The import's sound catalogue (soundcat.rs `catalogue`): every source is listed (Nova, the recipes, the kits, the Sampler's kits and
//! one-shots, the SoundFonts, the other synths' presets), the pickers choose from it by role and genre (a kit per genre, recipes where
//! they beat a preset), every pick round-trips to a dialog entry and a choice string, and the new choice strings import in memory.
//! Runs from LingStationX so the instruments, recipes and samples resolve.
use ling_daw::ai_import::{choose_seeded, parse_reply};
use ling_daw::midi_import::{ImportedMidi, ImportedTrack};
use ling_daw::model::Note;
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

fn part(name: &str, program: Option<u8>, notes: Vec<Note>) -> ImportedTrack {
    ImportedTrack { named: true, plugin: String::new(), name: name.into(), program, bank: 0, channel: 0, notes, volume: 1.0, pan: 0.0, vol_auto: vec![], pan_auto: vec![], bend_auto: vec![], bend_range: 2.0, samples: vec![], sampler: None, percussive: false, extra_auto: vec![] }
}

/// a small song: drums, a bass, chords, a lead, a fast arp and a pad
fn song(bpm: f64, drum_name: &str, bass_name: &str) -> ImportedMidi {
    let kit: Vec<Note> = (0..32).flat_map(|b| [(0.0, 36u8), (1.0, 38), (0.5, 42), (1.5, 42)].into_iter().map(move |(o, k)| Note::new(b as f64 * 2.0 + o, 0.1, k, 0.85))).collect();
    let bass: Vec<Note> = (0..32).map(|b| Note::new(b as f64 * 2.0, 1.5, 36 + (b % 3) as u8, 0.9)).collect();
    let lead: Vec<Note> = (0..32).map(|b| Note::new(b as f64 * 2.0, 1.0, 74 + (b % 5) as u8, 0.85)).collect();
    let arp: Vec<Note> = (0..128).map(|b| Note::new(b as f64 * 0.5, 0.2, 64 + (b % 4) as u8 * 3, 0.7)).collect();
    let pad: Vec<Note> = (0..8).flat_map(|b| [52u8, 55, 59, 62].into_iter().map(move |k| Note::new(b as f64 * 8.0, 7.8, k, 0.6))).collect();
    ImportedMidi { tracks: vec![part(drum_name, None, kit), part(bass_name, Some(38), bass), part("Lead", Some(81), lead), part("Arp", Some(5), arp), part("Pad", Some(89), pad)], bpm: Some(bpm), tempos: vec![], length_beats: 64.0 }
}

/// the dialog's list of a kind: (label, value)
fn sounds(d: &mut Daw, kind: &str) -> Vec<(String, String)> {
    list(&call(d, "daw_imp_sounds", &[s(kind), s("")])).chunks(4).map(|c| (st(&c[0]), st(&c[1]))).collect()
}

#[test]
fn the_catalogue_lists_every_source() {
    if !at_lsx() {
        return;
    }
    let mut d = Daw::new();
    assert!(num(&call(&mut d, "daw_imp_version", &[])) >= 2.0);
    let counts: Vec<(String, f64)> = list(&call(&mut d, "daw_imp_catalog", &[s("")])).chunks(2).map(|c| (st(&c[0]), num(&c[1]))).collect();
    println!("catalogue: {counts:?}");
    let count = |src: &str| counts.iter().find(|c| c.0 == src).map(|c| c.1 as usize).unwrap_or(0);
    // the recipes: every one that plays a part (master moves are not instruments)
    let lib = ling_daw::recipes::library();
    let playable = lib.recipes.iter().filter(|r| !r.is_master()).count();
    assert!(playable > 20 && count("recipe") == playable, "{} recipes, {} listed", playable, count("recipe"));
    let recipes = sounds(&mut d, "recipe");
    assert!(recipes.iter().any(|(l, v)| l.starts_with("Bass: ") && v == "recipe:riddim_gate_a"), "{recipes:?}");
    // every Kit_*.ling drum kit
    let kits = sounds(&mut d, "drums");
    let files: Vec<String> = std::fs::read_dir("instruments").unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).filter(|f| f.starts_with("Kit_") && f.ends_with(".ling")).collect();
    assert!(files.len() >= 20, "{files:?}");
    for f in &files {
        assert!(kits.iter().any(|(_, v)| v.ends_with(&format!("/{f}"))), "kit {f} is not listed: {kits:?}");
    }
    // every Ling Nova preset, labelled by its role
    let nova = sounds(&mut d, "nova");
    for p in ling_daw::dsp::nova::presets::all() {
        assert!(nova.iter().any(|(l, _)| l.ends_with(&format!(": {}", p.name))), "Nova preset {} is not listed", p.name);
    }
    assert!(nova.iter().all(|(l, _)| ["Bass: ", "Lead: ", "Chords: ", "Pad: ", "Pluck / Keys: ", "FX: ", "User: "].iter().any(|g| l.starts_with(g))), "{nova:?}");
    // the Sampler: the sample-folder kit and the shipped one-shots
    let smp = sounds(&mut d, "sampler");
    assert!(smp.iter().any(|(_, v)| v == "smpkit:"), "{smp:?}");
    let fx = std::fs::read_dir("samples/FX").map(|r| r.flatten().filter(|e| e.path().extension().map_or(false, |x| x == "wav")).count()).unwrap_or(0);
    assert_eq!(smp.iter().filter(|(_, v)| v.starts_with("smpfile:")).count(), fx);
    // SoundFonts by name, and the other synths
    if std::path::Path::new("assets/soundfonts/Ling.sf2").is_file() {
        assert!(sounds(&mut d, "sf2").iter().any(|(l, v)| l.starts_with("Ling.sf2") && v == "sf2:assets/soundfonts/Ling.sf2"));
    }
    for (src, at_least) in [("Prism", 300), ("Growl", 100), ("Voice", 7), ("Chip", 40), ("Cinema", 40), ("Nova", 100), ("kit", 20), ("Electronic GM", 128), ("GM", 128)] {
        assert!(count(src) >= at_least, "{src}: {}", count(src));
    }
    if std::fs::read_dir("instruments/User").map(|r| r.count() > 0).unwrap_or(false) {
        assert!(count("User") > 0);
    }
    // the picker's names resolve: Electronic GM without its number, a kit, a Growl preset
    assert!(!st(&call(&mut d, "daw_imp_choice", &[s("synth"), s("Electronic: Acid Bass")])).is_empty());
    assert!(st(&call(&mut d, "daw_imp_choice", &[s("drums"), s("Kit Trap")])).ends_with("Kit_Trap.ling"));
    assert!(st(&call(&mut d, "daw_imp_choice", &[s("growl"), s("wobble")])).ends_with(".ling"));
}

#[test]
fn drum_parts_get_the_kit_of_their_genre_and_recipes_play_where_they_fit() {
    if !at_lsx() {
        return;
    }
    let none = parse_reply("");
    let kit_of = |genre: &str, bpm: f64, drum_name: &str| {
        let c = choose_seeded(&song(bpm, drum_name, "Bass"), &none, genre, 3);
        (c[0].patch.clone(), c[0].why.clone())
    };
    for (genre, bpm, kit) in [("trap", 140.0, "Kit Trap"), ("house", 124.0, "Kit House 909"), ("jazz", 110.0, "Kit Brushes"), ("drum and bass", 174.0, "Kit Drum and Bass"), ("hip hop", 90.0, "Kit Boom Bap"), ("rock", 120.0, "Kit Rock"), ("techno", 132.0, "Kit Techno Hard"), ("lofi", 82.0, "Kit LoFi")] {
        let (k, why) = kit_of(genre, bpm, "Drums");
        assert_eq!(k, kit, "{genre}: {why}");
        assert!(why.contains(&format!("kit: {}", kit.trim_start_matches("Kit "))), "{why}");
    }
    // the part's own name wins: brushes in a pop song
    assert_eq!(kit_of("pop", 100.0, "Brushes").0, "Kit Brushes");
    // orchestral percussion stays on the soundfont's General MIDI kit
    let orch = choose_seeded(&song(84.0, "Drums", "Bass"), &none, "orchestral", 3);
    assert_eq!(orch[0].plugin, "auto");
    // recipes: a riddim bass, the house arp, the trance pad; never in jazz
    let dub = choose_seeded(&song(140.0, "Drums", "Riddim"), &none, "dubstep", 3);
    assert_eq!(dub[1].plugin, "recipe", "{}", dub[1].why);
    let rid = dub[1].value.trim_start_matches("recipe:");
    let rr = ling_daw::recipes::library().get(rid).cloned().expect("a recipe");
    assert!(rr.styles.iter().chain(rr.tags.iter()).any(|x| x == "riddim"), "a riddim recipe for the part called Riddim: {}", dub[1].value);
    assert!(dub[1].why.contains("recipe: "), "{}", dub[1].why);
    let house = choose_seeded(&song(124.0, "Drums", "Bass"), &none, "house", 3);
    assert_eq!(house[3].plugin, "recipe", "the house arp: {}", house[3].why);
    let trance = choose_seeded(&song(138.0, "Drums", "Bass"), &none, "trance", 3);
    assert_eq!(trance[4].plugin, "recipe", "the trance pad: {}", trance[4].why);
    let jazz = choose_seeded(&song(110.0, "Drums", "Bass"), &none, "jazz", 3);
    assert!(jazz.iter().all(|c| c.plugin != "recipe" && c.plugin != "nova"), "{:?}", jazz.iter().map(|c| &c.why).collect::<Vec<_>>());
    // a plain wobble in dubstep stays on Nova
    let wob = choose_seeded(&song(140.0, "Drums", "Wobble"), &none, "dubstep", 3);
    assert_eq!(wob[1].plugin, "nova", "{}", wob[1].why);
    for (g, c) in [("trap", choose_seeded(&song(140.0, "Drums", "808"), &none, "trap", 3)), ("house", house), ("jazz", jazz)] {
        println!("{g}:");
        for x in &c {
            println!("  part {} {:12} {:8} {:40} {}", x.part, x.role, x.plugin, x.patch, x.why);
        }
    }
}

#[test]
fn every_pick_round_trips_to_the_dialog_and_the_import() {
    if !at_lsx() {
        return;
    }
    let mut d = Daw::new();
    let none = parse_reply("");
    for (genre, bpm) in [("trap", 140.0), ("house", 124.0), ("jazz", 110.0), ("dubstep", 140.0), ("trance", 138.0), ("chiptune", 140.0), ("ambient", 72.0), ("hip hop", 90.0)] {
        for c in choose_seeded(&song(bpm, "Drums", "Riddim Bass"), &none, genre, 11) {
            if c.plugin == "auto" || c.plugin == "lib" {
                continue;
            }
            let list = sounds(&mut d, &c.plugin);
            let hit = list.iter().find(|(l, _)| *l == c.patch);
            assert!(hit.is_some(), "{genre}: {} \"{}\" is not in the dialog's list", c.plugin, c.patch);
            assert_eq!(hit.unwrap().1, c.value, "{genre}: {}", c.patch);
            assert_eq!(st(&call(&mut d, "daw_imp_choice", &[s(&c.plugin), s(&c.patch)])), c.value);
            // the value works: a patch file that exists, or a recipe the library has
            if let Some(id) = c.value.strip_prefix("recipe:") {
                assert!(ling_daw::recipes::library().get(id).is_some(), "{id}");
            } else {
                assert!(std::path::Path::new(&c.value).is_file(), "{genre}: {}", c.value);
            }
        }
    }
    // the builtin the dialog uses gives the same, six values a part
    let picks = list(&call(&mut d, "daw_imp_pick", &[s("library/LingAITune.mid"), s(""), s("trap"), n(5.0)]));
    assert!(picks.len() >= 12 && picks.len() % 6 == 0, "{picks:?}");
}

#[test]
fn recipe_kit_and_sampler_choices_import_in_memory() {
    if !at_lsx() {
        return;
    }
    let path = "library/LingAITune.mid";
    let mut d = Daw::new();
    let parts = list(&call(&mut d, "daw_midi_scan", &[s(path)]));
    let np = parts.len() / 8;
    let drum = (0..np).find(|i| num(&parts[i * 8 + 1]) == -1.0);
    let melodic: Vec<usize> = (0..np).filter(|i| num(&parts[i * 8 + 1]) >= 0.0).collect();
    assert!(melodic.len() >= 2, "{parts:?}");
    let kit = st(&call(&mut d, "daw_imp_choice", &[s("drums"), s("Kit Trap")]));
    let mut choices = vec![s("auto"); np];
    choices[melodic[0]] = s("recipe:riddim_gate_a");
    choices[melodic[1]] = s("smpfile:samples/FX/sub_drop.wav");
    if let Some(dp) = drum {
        choices[dp] = s(&kit);
    }
    let made = num(&call(&mut d, "daw_midi_import", &[s(path), n(0.0), Val::List(choices), s(""), Val::List(vec![]), s("dubstep")]));
    assert!(made >= np as f64 - 0.5, "{made}");
    let status = st(&call(&mut d, "daw_status", &[]));
    assert!(status.contains("1 sound recipe"), "{status}");
    // one track per part, in part order (the file has no audio parts and no shared kit); the parts may share a name
    assert_eq!(num(&call(&mut d, "daw_track_count", &[])) as usize, np);
    let find = |_d: &mut Daw, i: usize| Some(i);
    // the recipe's instrument and its effect chain
    let r = ling_daw::recipes::library().get("riddim_gate_a").unwrap().clone();
    let rt = find(&mut d, melodic[0]).expect("the recipe part's track");
    assert_eq!(st(&call(&mut d, "daw_track_kind", &[n(rt as f64)])), r.instrument);
    let fx: Vec<String> = (0..num(&call(&mut d, "daw_fx_count", &[n(rt as f64)])) as usize).map(|k| st(&call(&mut d, "daw_fx_name", &[n(rt as f64), n(k as f64)]))).collect();
    for step in r.fx_chain.iter().filter(|f| f.effect != "Ducker") {
        assert!(fx.contains(&step.effect), "{} missing from {fx:?}", step.effect);
    }
    // the one-shot plays in a Sampler, the kit on Ling Membrane
    let st2 = find(&mut d, melodic[1]).expect("the sampler part's track");
    assert_eq!(st(&call(&mut d, "daw_track_kind", &[n(st2 as f64)])), "sampler");
    if let Some(dp) = drum {
        let dt = find(&mut d, dp).expect("the drum track");
        assert_eq!(st(&call(&mut d, "daw_track_kind", &[n(dt as f64)])), "drums");
    }
    // a drum part as a Sampler kit from a sample folder (where this machine has one)
    let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_default();
    let folder = format!("{home}/Programs/[LingStation]/Drums (ModeAudio)");
    // a MIDI file of the library with a drum part
    let mut files: Vec<String> = ["library", "library/midi"].iter().flat_map(|d| std::fs::read_dir(d).map(|r| r.flatten().map(|e| e.path().to_string_lossy().replace('\\', "/")).filter(|p| p.ends_with(".mid")).collect::<Vec<_>>()).unwrap_or_default()).collect();
    files.sort();
    let with_drums = files.iter().find_map(|f| {
        let ps = list(&call(&mut d, "daw_midi_scan", &[s(f)]));
        let k = ps.len() / 8;
        (0..k).find(|i| num(&ps[i * 8 + 1]) == -1.0 && num(&ps[i * 8 + 3]) > 8.0).map(|i| (f.clone(), k, i))
    });
    assert!(drum.is_some() || with_drums.is_some(), "no library MIDI with drums");
    if let (Some((f, k, dp)), true) = (with_drums, std::path::Path::new(&folder).is_dir()) {
        let mut d2 = Daw::new();
        let mut ch = vec![s("auto"); k];
        ch[dp] = s(&format!("smpkit:{folder}"));
        call(&mut d2, "daw_midi_import", &[s(&f), n(0.0), Val::List(ch)]);
        let dt = (0..num(&call(&mut d2, "daw_track_count", &[])) as usize).find(|t| st(&call(&mut d2, "daw_track_kind", &[n(*t as f64)])) == "sampler").expect("a Sampler kit track");
        let zones = list(&call(&mut d2, "daw_smp_names", &[n(dt as f64)]));
        assert!(!zones.is_empty(), "the kit found no samples in {folder}");
        println!("sample kit for {f} from {folder}: {zones:?}");
    }
}
