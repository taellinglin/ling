//! What AI Song reads from its sources (songanalysis.rs, transcribe.rs, api/songan_api.rs) end to end: a guide MIDI with a known form
//! (labels, energy curve, story, hook, the plan taking its structure), and an acapella sung by Ling Voice and bounced to a WAV
//! (its melody recovered, chords fitted, the song planned on its layout and built, the vocal placed aligned, chopped, and stepping
//! aside for the chops). Run from LingStationX (the patches): cargo test -p ling-daw --test song_sources
use ling_daw::theory::{Key, N};
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

fn lsx() -> bool {
    std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX")).is_ok()
}

fn tmp(name: &str) -> String {
    let d = std::env::temp_dir().join("ling_song_sources");
    let _ = std::fs::create_dir_all(&d);
    d.join(name).to_string_lossy().replace('\\', "/")
}

// ── a tiny Standard MIDI File writer ─────────────────────────────────────────

fn vlq(mut v: u32) -> Vec<u8> {
    let mut out = vec![(v & 0x7f) as u8];
    v >>= 7;
    while v > 0 {
        out.insert(0, ((v & 0x7f) as u8) | 0x80);
        v >>= 7;
    }
    out
}

/// (name, channel, notes (start beats, len beats, key, velocity 0..1)) at `bpm`, 480 ticks a beat
fn smf(tracks: &[(&str, u8, Vec<N>)], bpm: f64) -> Vec<u8> {
    let mut out = b"MThd".to_vec();
    out.extend([0, 0, 0, 6, 0, 1]);
    out.extend(((tracks.len() + 1) as u16).to_be_bytes());
    out.extend(480u16.to_be_bytes());
    let mut chunk = |body: Vec<u8>| {
        out.extend(b"MTrk");
        out.extend((body.len() as u32).to_be_bytes());
        out.extend(body);
    };
    let us = (60_000_000.0 / bpm) as u32;
    let mut t0 = vec![0u8, 0xff, 0x51, 3];
    t0.extend(&us.to_be_bytes()[1..]);
    t0.extend([0, 0xff, 0x2f, 0]);
    chunk(t0);
    for (name, ch, notes) in tracks {
        let mut ev: Vec<(u32, u8, u8, u8)> = vec![];
        for nt in notes {
            let a = (nt.start * 480.0).round() as u32;
            let b = ((nt.start + nt.len) * 480.0).round() as u32;
            ev.push((a, 1, nt.key, (nt.vel * 127.0).round().clamp(1.0, 127.0) as u8));
            ev.push((b.max(a + 1), 0, nt.key, 0));
        }
        ev.sort_by_key(|e| (e.0, e.1));
        let mut body = vec![0u8, 0xff, 0x03, name.len() as u8];
        body.extend(name.as_bytes());
        let mut last = 0u32;
        for (t, on, k, v) in ev {
            body.extend(vlq(t - last));
            last = t;
            body.extend([if on == 1 { 0x90 | ch } else { 0x80 | ch }, k, v]);
        }
        body.extend([0, 0xff, 0x2f, 0]);
        chunk(body);
    }
    out
}

fn nt(start: f64, len: f64, key: u8, vel: f32) -> N {
    N { start, len, key, vel }
}

/// intro 4 (a pad), verse 8, pre 4 (climbing), chorus 8 (drums, loud, high), verse 8, pre 4, chorus 8, outro 4 (the pad); A minor
/// verses, a C major chorus
fn guide_song() -> Vec<(&'static str, u8, Vec<N>)> {
    let form: [(&str, usize); 8] = [("intro", 4), ("verse", 8), ("pre", 4), ("chorus", 8), ("verse", 8), ("pre", 4), ("chorus", 8), ("outro", 4)];
    let (mut pad, mut bass, mut lead, mut drums) = (vec![], vec![], vec![], vec![]);
    let mut bar = 0usize;
    for (tag, bars) in form {
        for k in 0..bars {
            let b0 = (bar + k) as f64 * 4.0;
            match tag {
                "intro" | "outro" => {
                    for key in [57u8, 60, 64] {
                        pad.push(nt(b0, 4.0, key, 0.35));
                    }
                }
                "verse" => {
                    let (root, tri) = if k % 2 == 0 { (45u8, [57u8, 60, 64]) } else { (50, [62, 65, 69]) };
                    for key in tri {
                        pad.push(nt(b0, 4.0, key, 0.5));
                    }
                    bass.push(nt(b0, 2.0, root, 0.6));
                    bass.push(nt(b0 + 2.0, 2.0, root, 0.6));
                    for (i, d) in [0u8, 3, 5, 3].iter().enumerate() {
                        lead.push(nt(b0 + i as f64, 0.9, 69 + d, 0.55));
                    }
                }
                "pre" => {
                    for key in [65u8, 69, 72] {
                        pad.push(nt(b0, 4.0, key, 0.6));
                    }
                    let steps = [4, 4, 8, 8][k.min(3)];
                    for i in 0..steps {
                        lead.push(nt(b0 + i as f64 * 4.0 / steps as f64, 4.0 / steps as f64 * 0.9, 72 + (i % 4) as u8, 0.5 + 0.07 * k as f32));
                    }
                    for i in 0..4 {
                        drums.push(nt(b0 + i as f64, 0.05, 38, 0.4 + 0.08 * k as f32));
                    }
                    bass.push(nt(b0, 4.0, 41, 0.7));
                }
                _ => {
                    let (root, tri) = if k % 2 == 0 { (48u8, [60u8, 64, 67]) } else { (43, [67, 71, 74]) };
                    for key in tri {
                        pad.push(nt(b0, 4.0, key, 0.85));
                    }
                    for i in 0..8 {
                        bass.push(nt(b0 + i as f64 * 0.5, 0.45, root, 0.95));
                        drums.push(nt(b0 + i as f64 * 0.5, 0.1, 42, 0.7));
                    }
                    for i in 0..4 {
                        drums.push(nt(b0 + i as f64, 0.1, 36, 1.0));
                    }
                    drums.push(nt(b0 + 1.0, 0.1, 38, 0.95));
                    drums.push(nt(b0 + 3.0, 0.1, 38, 0.95));
                    for (i, d) in [0u8, 4, 7, 12, 7, 4, 2, 0].iter().enumerate() {
                        lead.push(nt(b0 + i as f64 * 0.5, 0.45, 72 + d, 0.95));
                    }
                }
            }
        }
        bar += bars;
    }
    vec![("Pad", 0, pad), ("Bass", 1, bass), ("Lead", 2, lead), ("Drums", 9, drums)]
}

#[test]
fn a_guide_midi_gives_its_structure_story_and_hook_to_the_plan() {
    if !lsx() {
        return;
    }
    let path = tmp("guide_form.mid");
    std::fs::write(&path, smf(&guide_song(), 124.0)).unwrap();
    let mut d = Daw::new();
    let g = list(&call(&mut d, "daw_song_guide", &[s(&path)]));
    assert!(!g.is_empty(), "{:?}", call(&mut d, "daw_status", &[]));
    let info = list(&call(&mut d, "daw_song_an_info", &[s("midi"), s(&path)]));
    assert_eq!(info.len(), 11, "{info:?}");
    let structure = st(&info[0]);
    assert_eq!(structure, "intro 4, verse 8, pre 4, chorus 8, verse 8, pre 4, chorus 8, outro 4");
    let story = st(&info[1]);
    let arc = st(&info[2]);
    assert!(story.len() > 30 && story.contains("chorus"), "{story}");
    assert!(["rise", "fall", "journey", "dream", "battle", "hero"].contains(&arc.as_str()), "{arc}");
    assert!(ling_daw::emotion::ARCS.contains(&st(&info[3]).as_str()), "{:?}", info[3]);
    let curve: Vec<f64> = list(&info[4]).iter().map(num).collect();
    assert_eq!(curve.len(), 48);
    let mean = |a: usize, b: usize| curve[a..b].iter().sum::<f64>() / (b - a) as f64;
    assert!(mean(16, 24) > mean(4, 12), "the chorus is louder than the verse: {curve:?}");
    assert!(st(&info[10]).starts_with("notes:"), "the hook as a theme: {:?}", info[10]);
    // the plan takes the form (a pre-chorus is planned as a build) and tells the story
    let p = list(&call(&mut d, "daw_song_plan", &[s("house"), s(""), s(""), s(""), n(1.0), n(3.0), s(&path), n(0.5), s("auto"), s(&story)]));
    let line = st(&p[0]);
    assert!(line.contains("structure=intro 4, verse 8, build 4, chorus 8, verse 8, build 4, chorus 8, outro 4"), "{line}");
    assert!(line.contains(&format!("story={arc}")), "{line}");
    assert!(line.contains("bpm=124"), "{line}");
    // the hook as the theme the lead plays
    let p2 = list(&call(&mut d, "daw_song_plan", &[s("house"), s(""), s(""), s(""), n(1.0), n(3.0), s(&path), n(0.5), s("auto"), s(""), s(&st(&info[10]))]));
    assert!(st(&p2[1]).contains("theme"), "{}", st(&p2[1]));
}

/// the acapella's melody (beats, len, key): verse, chorus (higher, louder), 8 bars of rest (the band's drop), verse, chorus
fn melody() -> Vec<(f64, f64, u8)> {
    let verse: [(f64, f64, u8); 12] = [
        (0.0, 1.0, 69), (1.0, 0.5, 72), (1.5, 0.5, 71), (2.0, 2.0, 69),
        (4.0, 1.0, 67), (5.0, 1.0, 69), (6.0, 2.0, 64),
        (8.0, 0.5, 69), (8.5, 0.5, 72), (9.0, 1.0, 76), (10.0, 2.0, 74), (12.0, 3.0, 69),
    ];
    let chorus: [(f64, f64, u8); 10] = [
        (0.0, 1.0, 76), (1.0, 1.0, 77), (2.0, 2.0, 76),
        (4.0, 0.5, 74), (4.5, 0.5, 72), (5.0, 1.0, 74), (6.0, 2.0, 72),
        (8.0, 2.0, 76), (10.0, 2.0, 79), (12.0, 3.0, 76),
    ];
    let mut v = vec![];
    for (base, part) in [(0.0, &verse[..]), (16.0, &chorus[..]), (64.0, &verse[..]), (80.0, &chorus[..])] {
        for (st, l, k) in part {
            v.push((base + st, *l, *k));
        }
    }
    v
}

const LEAD: f64 = 4.0;

/// Ling Voice sings the melody (from beat LEAD at 120 bpm) alone; the WAV's path
fn sing(path: &str) -> String {
    let mut d = Daw::new();
    call(&mut d, "daw_set_bpm", &[n(120.0)]);
    let t = num(&call(&mut d, "daw_track_add", &[s("voice"), s("instruments/Voice/Voice_aria.ling")]));
    let mel = melody();
    let end = mel.iter().map(|m| m.0 + m.1).fold(0.0, f64::max) + LEAD;
    let c = num(&call(&mut d, "daw_clip_add_midi", &[n(t), n(0.0), n((end / 4.0).ceil() * 4.0)]));
    let mut flat = vec![];
    for m in &mel {
        let chorus = (m.0 / 16.0) as i32 % 2 == 1;
        flat.extend([n(m.0 + LEAD), n(m.1 * 0.92), n(m.2 as f64 - 12.0), n(if chorus { 0.95 } else { 0.7 }), n(0.0), n(0.5), n(0.5)]);
    }
    call(&mut d, "daw_clip_set_notes", &[n(t), n(c), Val::List(flat)]);
    let lyrics = "na da la da na da la da na da la da\nla la da na la la da na la na\nna da la da na da la da na da la da\nla la da na la la da na la na";
    call(&mut d, "daw_lyrf_apply", &[n(t), n(c), s(lyrics), s("fit:strict:0:0:all:1.0:auto")]);
    call(&mut d, "daw_bounce", &[s(path), n(0.0), n(end + 2.0), n(1.0), n(16.0)]);
    lyrics.to_string()
}

#[test]
fn an_acapella_is_transcribed_planned_on_and_placed_with_chops() {
    if !lsx() {
        return;
    }
    let path = tmp("acapella_120.wav");
    let lyrics = sing(&path);
    // the melody recovered: >= 85 % of the notes within a semitone, onsets within a sixteenth
    let smp = ling_daw::model::load_sample(&path).unwrap();
    let a = ling_daw::transcribe::analyze(&smp.stereo, smp.rate, 0.0).expect("notes");
    assert!((a.bpm / 120.0 - 1.0).abs() < 0.02, "tempo {}", a.bpm);
    let mel = melody();
    let first = a.notes.first().map(|x| x.start).unwrap_or(0.0);
    let shift = mel[0].0 - first;
    let mut hit = 0;
    for m in &mel {
        if a.notes.iter().any(|x| (x.start + shift - m.0).abs() <= 0.25 + 1e-6 && (x.key as i32 - (m.2 as i32 - 12)).abs() <= 1) {
            hit += 1;
        }
    }
    let share = hit as f64 / mel.len() as f64;
    eprintln!("transcribed {} notes, {hit} of {} known found ({:.0}%), key {}, tuning {:+.0} cents, origin {:.3} s; {} {:?}", a.notes.len(), mel.len(), share * 100.0, a.key.name(), a.tuning_cents, a.origin_s, a.structure_text(), a.shape.letters);
    assert!(share >= 0.85, "{share}: {:?}", a.notes.iter().map(|x| (x.start + shift, x.key)).collect::<Vec<_>>());
    assert!(matches!((a.key.tonic, a.key.minor), (9, true) | (0, false)), "{}", a.key.name());
    // chords in the key that hold the strong-beat notes
    let mut strong = 0;
    let mut inside = 0;
    for x in &a.notes {
        let b = x.start.rem_euclid(4.0);
        if b < 0.01 || (b - 2.0).abs() < 0.01 {
            strong += 1;
            if let Some(c) = a.chords.iter().find(|c| x.start >= c.start - 1e-6 && x.start < c.start + c.len) {
                if c.pcs.contains(&(x.key % 12)) {
                    inside += 1;
                }
            }
        }
    }
    assert!(inside as f64 >= 0.7 * strong as f64, "{inside} of {strong}");
    let key = a.key;
    assert!(a.chords.iter().all(|c| c.pcs.iter().all(|p| key.accepts(*p) || Key::new(key.tonic, true).accepts(*p))));

    // the dialog's way: read it, plan on its layout, build, place it, chop it
    let mut d = Daw::new();
    let r = list(&call(&mut d, "daw_song_an_acapella", &[s(&path), n(0.0), n(0.0)]));
    assert!(r.len() >= 8, "{:?}", call(&mut d, "daw_status", &[]));
    eprintln!("acapella: {} | {}", st(&r[0]), st(&r[6]));
    let guide = format!("acapella:{path}");
    let p = list(&call(&mut d, "daw_song_plan", &[s("future bass"), s(""), s(""), s(""), n(1.0), n(5.0), s(&guide), n(0.25), s("midi"), s("")]));
    let line = st(&p[0]);
    assert!(line.contains("bpm=120"), "{line}");
    assert!(line.contains("structure=intro 4,"), "{line}");
    let line = line.replace("movements=auto", "movements=1").replace("voice=aria", "voice=none");
    let structure = line.split(';').find_map(|f| f.trim().strip_prefix("structure=").map(str::to_string)).unwrap();
    let keyname = line.split(';').find_map(|f| f.trim().strip_prefix("key=").map(str::to_string)).unwrap();
    let b = list(&call(&mut d, "daw_song_build", &[s(&line), s("future bass"), s(""), s(""), n(5.0), s(&guide), n(0.25), s("midi"), s("")]));
    assert!(num(&b[0]) >= 3.0, "{b:?}");
    let pl = list(&call(&mut d, "daw_song_an_place", &[s(&path), n(120.0), s(&keyname), s(&structure), n(1.0), s(&lyrics)]));
    assert_eq!(pl.len(), 4, "{:?}", call(&mut d, "daw_status", &[]));
    let (at, mt) = (num(&pl[0]), num(&pl[1]));
    eprintln!("{}", st(&pl[3]));
    // aligned: the first sung note in the audio lands on the melody track's first note, which is bar 5 (after the 4-bar intro)
    let start = num(&call(&mut d, "daw_clip_get", &[n(at), n(0.0), s("start")]));
    let off = num(&call(&mut d, "daw_clip_get", &[n(at), n(0.0), s("offset")]));
    let heard = start + (a.first_s - off) * 2.0;
    let mstart = num(&call(&mut d, "daw_clip_get", &[n(mt), n(0.0), s("start")]));
    let notes = list(&call(&mut d, "daw_clip_notes", &[n(mt), n(0.0)]));
    let m0 = mstart + num(&notes[0]);
    assert!((m0 - 16.0).abs() < 0.26, "melody starts at {m0}");
    assert!((heard - m0).abs() < 0.25, "the vocal is heard at beat {heard:.2}, the melody says {m0:.2}");
    // the chops cut it, and it steps aside where they carry it
    let ch = st(&call(&mut d, "daw_chopx_make", &[n(at), s("sections=auto;seed=1")]));
    assert!(!ch.is_empty(), "{:?}", call(&mut d, "daw_status", &[]));
    let duck = st(&call(&mut d, "daw_song_an_duck", &[n(at)]));
    eprintln!("chops: {ch}\nduck: {duck}");
    let lanes: Vec<String> = list(&call(&mut d, "daw_auto_lanes", &[n(at)])).iter().map(st).collect();
    if duck.contains("replace") {
        assert!(lanes.iter().any(|l| l == "vol"), "{lanes:?}");
    }
}
