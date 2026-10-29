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

/// the chord of the song at a beat (pitch classes): Am F C G a bar each from beat 16, none in the intro and in the break (96..112)
fn chord(beat: f64) -> Option<[u8; 3]> {
    if beat < 16.0 || (96.0..112.0).contains(&beat) || beat >= 144.0 {
        return None;
    }
    Some([[9, 0, 4], [5, 9, 0], [0, 4, 7], [7, 11, 2]][((beat / 4.0).floor() as usize) % 4])
}

/// How well the chops on `track` fit the song: every pitched chop note is played alone by a sampler over the bank file and its pitch
/// measured. Returns (notes measured, how many sound within 30 cents of a tone of the chord at their beat - or of the key's scale
/// where no chord plays -, median distance to it in cents, median |heard - note key| in cents).
fn measure(d: &mut Daw, track: usize, bank: &str) -> (usize, usize, f32, f32) {
    use ling_daw::dsp::Instrument;
    let (stereo, rate) = ling_daw::dsp::sampler::load_audio(bank).expect("bank");
    let mut sm = ling_daw::dsp::sampler::Sampler::new(rate as f32);
    let mut z = ling_daw::dsp::sampler::Zone::new("v", "");
    z.root = 60;
    sm.add_loaded(z, stereo, rate);
    let (mut n_meas, mut fit) = (0usize, 0usize);
    let (mut dist, mut keyerr) = (vec![], vec![]);
    let clips = num(call(d, "daw_clip_count", &[n(track as f64)])) as usize;
    for c in 0..clips {
        let start = num(call(d, "daw_clip_get", &[n(track as f64), n(c as f64), s("start")]));
        let Val::List(ns) = call(d, "daw_clip_notes", &[n(track as f64), n(c as f64)]) else { continue };
        let Val::List(ly) = call(d, "daw_note_lyrics", &[n(track as f64), n(c as f64)]) else { continue };
        for (i, l) in ly.iter().enumerate() {
            let Val::Str(l) = l else { continue };
            let Some((a, b, cents)) = ling_daw::dsp::sampler::parse_slice(l) else { continue };
            let (at, key) = (num(ns[i * 7].clone()) + start, num(ns[i * 7 + 2].clone()));
            let moved = key - 60.0 + cents as f64 / 100.0;
            let frames = ((b - a) as f64 / 2f64.powf(moved / 12.0)) as usize;
            sm.all_off();
            let mut buf = vec![0f32; 512];
            for _ in 0..8 {
                sm.render(&mut buf);
            }
            sm.note_on_lyric(key as u8, 0.9, 0.0, 0.5, 0.5, l, 1.0);
            let mut mono: Vec<f32> = vec![];
            while mono.len() < frames {
                buf.fill(0.0);
                sm.render(&mut buf);
                mono.extend(buf.chunks(2).map(|f| (f[0] + f[1]) * 0.5));
            }
            mono.truncate(frames);
            let Some(f) = ling_daw::chops::chop_f0(&mono, 0, mono.len(), rate) else { continue };
            n_meas += 1;
            let pcs: Vec<u8> = chord(at).map(|c| c.to_vec()).unwrap_or(vec![0, 2, 4, 5, 7, 9, 11]);
            let dd = pcs.iter().map(|pc| {
                let x = (f.midi - *pc as f32).rem_euclid(12.0);
                x.min(12.0 - x) * 100.0
            }).fold(f32::MAX, f32::min);
            dist.push(dd);
            fit += (dd < 30.0) as usize;
            if dd >= 30.0 && std::env::var("CHOPS_DEBUG").is_ok() {
                println!("miss at {at:.2} key {key} heard {:.2} chord {:?} lyric {l}", f.midi, chord(at));
            }
            keyerr.push(((f.midi - key as f32) * 100.0).abs());
        }
    }
    let med = |v: &mut Vec<f32>| {
        v.sort_by(f32::total_cmp);
        v.get(v.len() / 2).copied().unwrap_or(f32::NAN)
    };
    (n_meas, fit, med(&mut dist), med(&mut keyerr))
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
    // tuned to the song: the chops sound on the chord tones of the bar they play in
    assert!(msg.contains("tuned to the chords"), "{msg}");
    let (m, fit, dist, kerr) = measure(&mut d, tracks_before as usize, "cache/chops/chops_Lead_Vocal_2.wav");
    println!("tuned: {fit} of {m} measured chop notes within 30 cents of a chord tone (median {dist:.1} cents off, {kerr:.1} cents from the note key)");
    assert!(m >= 60 && fit as f64 >= 0.9 * m as f64, "{fit} of {m}");
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
    let tuned = measure(&mut d, 4, "cache/chops/chops_Lead_Vocal_2.wav");
    // the same chops as sung (tune=off), for comparison
    let mut u = song();
    let msg_u = text(call(&mut u, "daw_chopx_make", &[n(0.0), s("seed=2;density=0.7;stutter=0.6;tune=off")]));
    println!("{msg_u}");
    call(&mut u, "daw_bounce", &[s(&format!("{dir}/5_with_chops_untuned.wav")), n(0.0), n(144.0), n(2.0), n(16.0), s("wav"), n(0.5)]);
    for t in [0.0, 1.0, 2.0, 3.0] {
        call(&mut u, "daw_track_set", &[n(t), s("mute"), n(1.0)]);
    }
    call(&mut u, "daw_bounce", &[s(&format!("{dir}/6_chops_only_untuned.wav")), n(0.0), n(144.0), n(2.0), n(16.0), s("wav"), n(0.5)]);
    let untuned = measure(&mut u, 4, "cache/chops/chops_Lead_Vocal_2.wav");
    let line = |name: &str, x: (usize, usize, f32, f32)| format!("{name}: {} of {} measured chop notes within 30 cents of a tone of the chord at their beat ({:.0} %), median {:.1} cents from the nearest chord tone, {:.1} cents from the note key", x.1, x.0, 100.0 * x.1 as f64 / x.0.max(1) as f64, x.2, x.3);
    let report = format!("{msg}\n\nuntuned (tune=off): {msg_u}\n\nmeasured (each chop note rendered alone, YIN pitch of the slice):\n{}\n{}\n", line("tuned", tuned), line("untuned", untuned));
    std::fs::write(format!("{dir}/summary.txt"), &report).unwrap();
    println!("{report}");
}
