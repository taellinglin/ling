//! Vocal Chops end to end: a Ling Voice part with fitted lyrics, drums, bass and pad over tagged sections -> daw_chopx_make ->
//! sampler tracks of syllable slices, builds with a high-pass lane, the kick ducking them. `render_chops_demo` (ignored) bounces the
//! whole song to renders/chops_demo/.
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
fn num(v: Val) -> f64 {
    if let Val::Num(x) = v {
        x
    } else {
        f64::NAN
    }
}
fn text(v: Val) -> String {
    if let Val::Str(x) = v {
        x
    } else {
        String::new()
    }
}

/// A 126 bpm song: intro, verse (the vocal sings), a build, a drop, a break, a drop - the chops come after.
fn song() -> Daw {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_set_bpm", &[n(126.0)]);
    call(&mut d, "daw_meta_set", &[s("sections"), s("0:16:intro;16:48:verse;48:64:build;64:96:drop;96:112:break;112:144:drop")]);
    // the vocal: two 16-beat phrases of 18 notes in A minor, fitted to the words
    let v = num(call(&mut d, "daw_track_add", &[s("voice"), s("")])) as usize;
    call(&mut d, "daw_track_set_name", &[n(v as f64), s("Lead Vocal")]);
    let rel: [(f64, f64, f64); 18] = [
        (0.0, 0.5, 69.0), (0.5, 0.5, 69.0), (1.0, 0.5, 67.0), (1.5, 0.5, 69.0), (2.0, 0.5, 72.0), (2.5, 1.5, 76.0), (4.0, 1.0, 72.0), (5.0, 2.0, 69.0),
        (8.0, 0.75, 67.0), (8.75, 0.75, 69.0), (9.5, 0.75, 67.0), (10.25, 0.75, 69.0), (11.0, 0.5, 72.0), (11.5, 0.5, 76.0), (12.0, 0.5, 74.0), (12.5, 0.5, 72.0),
        (13.0, 1.0, 69.0), (14.0, 2.0, 67.0),
    ];
    for (k, at) in [16.0, 32.0].iter().enumerate() {
        let c = num(call(&mut d, "daw_clip_add_midi", &[n(v as f64), n(*at), n(16.0)])) as usize;
        for (st, ln, key) in rel {
            call(&mut d, "daw_note_add", &[n(v as f64), n(c as f64), n(st), n(ln * 0.95), n(key - 12.0 + if k == 1 { 0.0 } else { 0.0 }), n(0.85)]);
        }
        call(&mut d, "daw_lyrf_apply", &[n(v as f64), n(c as f64), s("hey hey I need your love tonight\nfalling falling in love with you tonight"), s("fit:fit:0:0.3:all:1:en")]);
    }
    // drums
    let dr = num(call(&mut d, "daw_track_add", &[s("drums"), s("")])) as usize;
    let c = num(call(&mut d, "daw_clip_add_midi", &[n(dr as f64), n(0.0), n(144.0)])) as usize;
    let hit = |d: &mut Daw, t: f64, k: f64, v: f64, l: f64| {
        call(d, "daw_note_add", &[n(dr as f64), n(c as f64), n(t), n(l), n(k), n(v)]);
    };
    for b in 0..36 {
        let t0 = b as f64 * 4.0;
        let sec = if t0 < 16.0 { 0 } else if t0 < 48.0 { 1 } else if t0 < 64.0 { 2 } else if t0 < 96.0 { 3 } else if t0 < 112.0 { 4 } else { 3 };
        for i in 0..8 {
            hit(&mut d, t0 + i as f64 * 0.5 + 0.25 * (sec == 3) as u8 as f64 * 0.0, 42.0, if i % 2 == 0 { 0.5 } else { 0.35 }, 0.1);
        }
        if sec == 4 {
            continue;
        }
        for i in 0..4 {
            if sec >= 1 {
                hit(&mut d, t0 + i as f64, 36.0, 0.95, 0.2);
            }
        }
        if sec >= 1 {
            hit(&mut d, t0 + 1.0, 38.0, 0.85, 0.2);
            hit(&mut d, t0 + 3.0, 38.0, 0.85, 0.2);
        }
        if sec == 3 {
            for i in 0..4 {
                hit(&mut d, t0 + i as f64 + 0.5, 46.0, 0.5, 0.2);
            }
            hit(&mut d, t0 + 1.0, 39.0, 0.6, 0.2);
        }
        if sec == 2 && b % 4 == 3 {
            for i in 0..16 {
                hit(&mut d, t0 + 2.0 + i as f64 * 0.125, 38.0, 0.4 + 0.5 * i as f64 / 16.0, 0.1);
            }
        }
    }
    // bass and pad: Am F C G, a bar each
    let bs = num(call(&mut d, "daw_track_add", &[s("synth"), s("")])) as usize;
    let pd = num(call(&mut d, "daw_track_add", &[s("synth"), s("")])) as usize;
    let bc = num(call(&mut d, "daw_clip_add_midi", &[n(bs as f64), n(16.0), n(128.0)])) as usize;
    let pc = num(call(&mut d, "daw_clip_add_midi", &[n(pd as f64), n(16.0), n(128.0)])) as usize;
    let roots = [45.0, 41.0, 48.0, 43.0];
    let tri = [[0.0, 3.0, 7.0], [0.0, 4.0, 7.0], [0.0, 4.0, 7.0], [0.0, 4.0, 7.0]];
    for b in 0..32 {
        let t0 = b as f64 * 4.0;
        if (80.0..96.0).contains(&t0) {
            continue;
        }
        let r = roots[b % 4];
        for i in 0..8 {
            call(&mut d, "daw_note_add", &[n(bs as f64), n(bc as f64), n(t0 + i as f64 * 0.5), n(0.4), n(r), n(0.8)]);
        }
        for k in tri[b % 4] {
            call(&mut d, "daw_note_add", &[n(pd as f64), n(pc as f64), n(t0), n(3.8), n(r + 12.0 + k), n(0.5)]);
        }
    }
    d
}

#[test]
fn chopping_a_ling_voice_part_makes_syllable_chops_with_builds() {
    let mut d = song();
    let tracks_before = num(call(&mut d, "daw_track_count", &[]));
    // the vocal is found, with its notes
    let info = call(&mut d, "daw_chopx_info", &[n(0.0)]);
    let Val::List(info) = info else { panic!("info") };
    assert_eq!(text(info[0].clone()), "midi");
    assert_eq!(num(info[1].clone()), 36.0);
    // the sections come back with a role each
    let Val::List(secs) = call(&mut d, "daw_chopx_sections", &[]) else { panic!("sections") };
    assert_eq!(secs.len(), 6 * 5);
    assert_eq!(text(secs[3].clone()), "calm");
    assert_eq!(text(secs[13 + 0].clone()), "build");
    let msg = text(call(&mut d, "daw_chopx_make", &[n(0.0), s("seed=2;density=0.7;stutter=0.6")]));
    println!("{msg}");
    assert!(msg.starts_with("Vocal chops:"), "{msg:?} / {}", text(call(&mut d, "daw_status", &[])));
    assert!(msg.contains("builds"));
    let after = num(call(&mut d, "daw_track_count", &[]));
    assert!(after >= tracks_before + 1.0);
    let name = text(call(&mut d, "daw_track_name", &[n(tracks_before)]));
    assert!(name.starts_with("Vocal Chops"), "{name}");
    // chops in the build, the two drops and the break-into-drop; none in the intro
    let clips = num(call(&mut d, "daw_clip_count", &[n(tracks_before)]));
    assert!(clips >= 4.0, "{clips}");
    // clip 0 is the reverse swell out of the verse, clip 1 the build
    let Val::List(notes) = call(&mut d, "daw_clip_notes", &[n(tracks_before), n(1.0)]) else { panic!("notes") };
    assert!(notes.len() >= 7 * 20, "{}", notes.len());
    let lyrics = call(&mut d, "daw_note_lyrics", &[n(tracks_before), n(0.0)]);
    let Val::List(ly) = lyrics else { panic!() };
    assert!(ly.iter().all(|l| matches!(l, Val::Str(s) if s.starts_with('@'))), "every chop note carries a slice");
    // the original vocal is untouched
    assert_eq!(num(call(&mut d, "daw_clip_count", &[n(0.0)])), 2.0);
    let _ = std::fs::remove_file("cache/chops/chops_Lead_Vocal_2.wav");
}

#[test]
#[ignore]
fn render_chops_demo() {
    let dir = std::env::var("CHOPS_DEMO_DIR").unwrap_or_else(|_| "C:/Users/User/Programs/LingStationX/renders/chops_demo".into());
    std::fs::create_dir_all(&dir).unwrap();
    let mut d = song();
    // the song before the chops, for comparison
    call(&mut d, "daw_bounce", &[s(&format!("{dir}/1_before_chops.wav")), n(0.0), n(144.0), n(2.0), n(16.0), s("wav"), n(0.5)]);
    let msg = text(call(&mut d, "daw_chopx_make", &[n(0.0), s("seed=2;density=0.7;stutter=0.6")]));
    std::fs::write(format!("{dir}/summary.txt"), &msg).unwrap();
    println!("{msg}");
    call(&mut d, "daw_bounce", &[s(&format!("{dir}/2_with_chops.wav")), n(0.0), n(144.0), n(2.0), n(16.0), s("wav"), n(0.5)]);
    // the chops with the drums, then alone (tracks: 0 vocal, 1 drums, 2 bass, 3 pad, 4.. chops)
    for t in [0.0, 2.0, 3.0] {
        call(&mut d, "daw_track_set", &[n(t), s("mute"), n(1.0)]);
    }
    call(&mut d, "daw_bounce", &[s(&format!("{dir}/3_chops_and_drums.wav")), n(0.0), n(144.0), n(2.0), n(16.0), s("wav"), n(0.5)]);
    call(&mut d, "daw_track_set", &[n(1.0), s("mute"), n(1.0)]);
    call(&mut d, "daw_bounce", &[s(&format!("{dir}/4_chops_only.wav")), n(0.0), n(144.0), n(2.0), n(16.0), s("wav"), n(0.5)]);
}
