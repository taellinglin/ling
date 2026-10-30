//! Make Medley with an acapella (api/medley_api.rs, medley/acapella.rs): a synthetic vocal (Ling Voice, bounced; no recording of
//! anyone) under a mashup of the MIDI folder - its level lane at 0 where the chops carry the vocal and back up on the downbeat they
//! end, the chops on a Ling Slicer kit tuned to the chords, stressed lines with harmonies on chord tones and a vocoder fed by the
//! line, and the story, feel and sound recipes changing the build. `render_medley_popculture` (ignored) writes renders/medley_popculture/.
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
    match v {
        Val::Num(x) => x,
        _ => 0.0,
    }
}
fn text(v: Val) -> String {
    if let Val::Str(x) = v {
        x
    } else {
        String::new()
    }
}
fn list(v: Val) -> Vec<Val> {
    if let Val::List(x) = v {
        x
    } else {
        vec![]
    }
}

const MIDI: &str = "projects/A.I. Love You/midi";

/// LingStationX as the working folder (the patches, the recipes, the MIDI folder); false when it is not there
fn at_lsx() -> bool {
    let lsx = concat!(env!("CARGO_MANIFEST_DIR"), "/../../../LingStationX");
    std::env::set_current_dir(lsx).is_ok() && std::path::Path::new(MIDI).is_dir()
}

/// A synthetic acapella: Ling Voice singing two verse lines, a hook twice, two more verse lines and the hook twice again at
/// 100 bpm in A minor (two bars a line, a beat and a half of rest between), bounced to `path`.
fn make_acapella(path: &str) {
    if std::path::Path::new(path).is_file() {
        return;
    }
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_set_bpm", &[n(100.0)]);
    let v = num(call(&mut d, "daw_track_add", &[s("voice"), s("")]));
    let hook: &[(f64, f64, f64)] = &[(0.0, 0.5, 69.0), (0.5, 0.5, 72.0), (1.0, 1.0, 76.0), (2.0, 0.5, 74.0), (2.5, 0.5, 72.0), (3.0, 1.5, 69.0), (4.5, 0.5, 67.0), (5.0, 1.5, 69.0)];
    let va: &[(f64, f64, f64)] = &[(0.0, 0.5, 64.0), (0.5, 0.5, 64.0), (1.0, 0.5, 65.0), (1.5, 0.5, 64.0), (2.0, 1.0, 62.0), (3.0, 0.5, 60.0), (3.5, 0.5, 62.0), (4.0, 2.5, 64.0)];
    let vb: &[(f64, f64, f64)] = &[(0.0, 0.5, 60.0), (0.5, 0.5, 62.0), (1.0, 1.0, 64.0), (2.0, 0.5, 67.0), (2.5, 0.5, 65.0), (3.0, 1.0, 64.0), (4.0, 0.5, 62.0), (4.5, 2.0, 60.0)];
    let lines: [(&[(f64, f64, f64)], &str); 8] = [
        (va, "walking through the city lights"),
        (vb, "every window calls my name"),
        (hook, "hold on hold on to the night"),
        (hook, "hold on hold on to the night"),
        (va, "running till the morning comes"),
        (vb, "shadows dancing on the wall"),
        (hook, "hold on hold on to the night"),
        (hook, "hold on hold on to the night"),
    ];
    for (k, (notes, words)) in lines.iter().enumerate() {
        let at = 4.0 + 8.0 * k as f64;
        let c = num(call(&mut d, "daw_clip_add_midi", &[n(v), n(at), n(8.0)]));
        for (st, ln, key) in notes.iter() {
            call(&mut d, "daw_note_add", &[n(v), n(c), n(*st), n(ln * 0.95), n(*key - 12.0), n(0.85)]);
        }
        call(&mut d, "daw_lyrf_apply", &[n(v), n(c), s(words), s("fit:fit:0:0.3:all:1:en")]);
    }
    if let Some(p) = std::path::Path::new(path).parent() {
        let _ = std::fs::create_dir_all(p);
    }
    assert!(num(call(&mut d, "daw_bounce", &[s(path), n(0.0), n(72.0), n(0.5), n(16.0), s("wav")])) > 0.5);
}

fn names(d: &mut Daw) -> Vec<String> {
    (0..num(call(d, "daw_track_count", &[])) as usize).map(|i| text(call(d, "daw_track_name", &[n(i as f64)]))).collect()
}

fn track(d: &mut Daw, prefix: &str) -> Option<usize> {
    names(d).iter().position(|x| x.starts_with(prefix))
}

/// a track's lane as (beat, value) points
fn lane(d: &mut Daw, t: usize, param: &str) -> Vec<(f64, f64)> {
    let lanes: Vec<String> = list(call(d, "daw_auto_lanes", &[n(t as f64)])).into_iter().map(text).collect();
    let Some(li) = lanes.iter().position(|l| l == param) else { return vec![] };
    let p = list(call(d, "daw_auto_points", &[n(t as f64), n(li as f64)]));
    p.chunks(2).map(|c| (num(c[0].clone()), num(c[1].clone()))).collect()
}

fn at(points: &[(f64, f64)], b: f64) -> f64 {
    if points.is_empty() {
        return f64::NAN;
    }
    if b <= points[0].0 {
        return points[0].1;
    }
    for w in points.windows(2) {
        if b >= w[0].0 && b <= w[1].0 {
            let t = (b - w[0].0) / (w[1].0 - w[0].0).max(1e-9);
            return w[0].1 + (w[1].1 - w[0].1) * t;
        }
    }
    points[points.len() - 1].1
}

/// the sections on the ruler: (start, end, tag)
fn sections(d: &mut Daw) -> Vec<(f64, f64, String)> {
    text(call(d, "daw_meta_get", &[s("sections")]))
        .split(';')
        .filter_map(|it| {
            let f: Vec<&str> = it.split(':').collect();
            (f.len() >= 3).then(|| (f[0].parse().unwrap_or(0.0), f[1].parse().unwrap_or(0.0), f[2].to_string()))
        })
        .collect()
}

/// every note of a track's clips named `label` (or all with ""), absolute: (beat, len, key, lyric)
fn notes(d: &mut Daw, t: usize, label: &str) -> Vec<(f64, f64, f64, String)> {
    let mut out = vec![];
    for c in 0..num(call(d, "daw_clip_count", &[n(t as f64)])) as usize {
        if !label.is_empty() && text(call(d, "daw_clip_name", &[n(t as f64), n(c as f64)])) != label {
            continue;
        }
        let start = num(call(d, "daw_clip_get", &[n(t as f64), n(c as f64), s("start")]));
        let ns = list(call(d, "daw_clip_notes", &[n(t as f64), n(c as f64)]));
        let ly: Vec<String> = list(call(d, "daw_note_lyrics", &[n(t as f64), n(c as f64)])).into_iter().map(text).collect();
        for (i, x) in ns.chunks(7).enumerate() {
            out.push((start + num(x[0].clone()), num(x[1].clone()), num(x[2].clone()), ly.get(i).cloned().unwrap_or_default()));
        }
    }
    out
}

/// the pitch classes the medley's pitched MIDI parts (not the drums, not the chops) sound at a beat
fn harmony_at(d: &mut Daw, b: f64, cache: &mut Vec<(usize, Vec<(f64, f64, f64, String)>)>) -> Vec<u8> {
    if cache.is_empty() {
        let nm = names(d);
        for (t, name) in nm.iter().enumerate() {
            if name.starts_with("Drums") || name.starts_with("Chops") || name.starts_with("Breaks") || name.starts_with("Riser") || name.starts_with("Noise") || name.starts_with("Vocoder")
                || ling_daw::medley::cinema::KINDS.iter().any(|(_, k, _)| name == k)
            {
                // the transition effects (Ling Cinema) are sound effects, not harmony
                continue;
            }
            if num(call(d, "daw_track_get", &[n(t as f64), s("audio")])) > 0.5 {
                continue;
            }
            let v = notes(d, t, "");
            cache.push((t, v));
        }
    }
    let mut pcs = vec![];
    let mut low = 128.0f64;
    for (_, v) in cache.iter() {
        for (st, ln, k, _) in v {
            if *st <= b + 1e-6 && st + ln > b + 1e-6 {
                let pc = (*k as i64).rem_euclid(12) as u8;
                low = low.min(*k);
                if !pcs.contains(&pc) {
                    pcs.push(pc);
                }
            }
        }
    }
    // a bass alone (or a bass and one more) implies the triad over it (chops::chord_at): its third and fifth count
    if !pcs.is_empty() && pcs.len() <= 2 {
        let r = (low as i64).rem_euclid(12) as u8;
        for x in [3u8, 4, 7] {
            let pc = (r + x) % 12;
            if !pcs.contains(&pc) {
                pcs.push(pc);
            }
        }
    }
    pcs
}

/// the distance in cents from a pitch to the nearest of the pitch classes
fn cents_to(m: f32, pcs: &[u8]) -> f32 {
    pcs.iter()
        .map(|pc| {
            let x = (m - *pc as f32).rem_euclid(12.0);
            x.min(12.0 - x) * 100.0
        })
        .fold(f32::MAX, f32::min)
}

/// every pitched note of the chop track `t` (clips named `label`) played alone by the track's own Ling Slicer at its beat (its
/// auto-tune reading the song there) and measured: (beat, heard MIDI)
fn heard_chops(d: &mut Daw, t: usize, label: &str) -> Vec<(f64, f32)> {
    use ling_daw::dsp::Instrument;
    let picked: Vec<(f64, f64, f64)> = notes(d, t, label).into_iter().map(|(b, l, k, _)| (b, l, k)).collect();
    let eng = d.engine();
    let mut en = eng.lock().unwrap();
    let (sr, bpm) = (en.sr, en.project.bpm);
    assert_eq!(en.project.tracks[t].inst_kind, "slicer", "the chops are on Ling Slicer");
    let inst = en.project.tracks[t].inst.as_mut().expect("a Slicer");
    let mut out = vec![];
    for (b, len, key) in picked {
        inst.all_off();
        inst.note_beat(b);
        inst.note_on(key as u8, 0.9);
        let frames = ((len * 60.0 / bpm).clamp(0.12, 1.0) * sr as f64) as usize;
        let mut buf = vec![0f32; 512];
        let mut mono: Vec<f32> = vec![];
        while mono.len() < frames {
            buf.fill(0.0);
            inst.render(&mut buf);
            mono.extend(buf.chunks(2).map(|f| (f[0] + f[1]) * 0.5));
        }
        mono.truncate(frames);
        if let Some(f) = ling_daw::chops::chop_f0(&mono, 0, mono.len(), sr as u32) {
            out.push((b, f.midi));
        }
    }
    inst.all_off();
    out
}

fn rms(path: &str) -> f64 {
    let (x, _) = ling_daw::dsp::sampler::load_audio(path).expect("wav");
    (x.iter().map(|v| (*v as f64).powi(2)).sum::<f64>() / x.len().max(1) as f64).sqrt()
}

fn acapella() -> String {
    let p = std::env::temp_dir().join("lsx_medley_acapella").join("synth_vocal_100bpm.wav");
    let p = p.to_string_lossy().replace('\\', "/");
    make_acapella(&p);
    p
}

fn build(d: &mut Daw, opts: &str, seed: f64) -> String {
    text(call(d, "daw_song_medley_build", &[s(MIDI), n(4.0), n(8.0), s("smart"), s(""), s(""), n(seed), n(0.0), s(opts)]))
}

#[test]
fn an_acapella_sings_where_the_chops_stop_and_its_hook_gets_harmonies() {
    if !at_lsx() {
        return;
    }
    let vocal = acapella();
    let mut d = Daw::new();
    let r = build(&mut d, &format!("layering=0.8;rolls=0.9;genre=house;acap={vocal};replace=1;stress=0.9;stressways=harmonies;recipes=0;story=a hero falls and rises again"), 5.0);
    println!("{r}");
    assert!(r.contains("; acapellas: synth vocal 100bpm (100 bpm from its name"), "{r}");
    assert!(r.contains("vocal plan (the chops replace the vocal") && r.contains("stressed lines:") && r.contains("harmonies"), "{r}");
    assert!(r.contains("flow:") && r.contains("expression rules"), "{r}");
    assert_eq!(num(call(&mut d, "daw_bpm", &[])), 100.0, "the medley takes the singer's tempo");
    let at_ = track(&mut d, "Acapella · synth vocal 100bpm").expect("the acapella's track");
    let ch = track(&mut d, "Chops · synth vocal 100bpm").expect("its chops");
    let vol = lane(&mut d, at_, "vol");
    assert!(!vol.is_empty(), "a level lane");
    let level = vol.iter().map(|p| p.1).fold(0.0, f64::max);
    assert!(level > 0.5);
    // wherever the chops carry the vocal, the acapella is at 0
    let chop_notes = notes(&mut d, ch, "chops");
    assert!(chop_notes.len() > 20, "{}", chop_notes.len());
    for (b, _, _, _) in &chop_notes {
        assert_eq!(at(&vol, *b), 0.0, "the acapella is silent under the chops at {b}");
    }
    // and it comes back on a downbeat, with a fade, where a section starts
    let secs = sections(&mut d);
    let start0 = secs[0].0;
    let mut returns = 0;
    for w in vol.windows(2) {
        if w[0].1 == 0.0 && w[1].1 > 0.5 {
            let b = w[0].0;
            assert!(secs.iter().any(|x| (x.0 - b).abs() < 1e-6), "back at a section start: {b}");
            assert!(((b - start0) / 4.0).fract().abs() < 1e-6, "on a downbeat: {b}");
            let mid = at(&vol, b + 0.25);
            assert!(mid > 0.05 && mid < level - 0.05, "a fade: {mid}");
            assert!((at(&vol, b + 0.5) - level).abs() < 1e-6);
            returns += 1;
        }
    }
    assert!(returns >= 1, "{vol:?}");
    // it has its recording wherever it sings: a clip over the middle of every singing section
    let clips: Vec<(f64, f64)> = (0..num(call(&mut d, "daw_clip_count", &[n(at_ as f64)])) as usize)
        .map(|c| (num(call(&mut d, "daw_clip_get", &[n(at_ as f64), n(c as f64), s("start")])), num(call(&mut d, "daw_clip_get", &[n(at_ as f64), n(c as f64), s("end")]))))
        .collect();
    let mut sung = 0;
    for (a, b, _) in &secs {
        if at(&vol, (a + b) * 0.5) > 0.5 {
            sung += 1;
            assert!(clips.iter().any(|c| c.0 <= a + 2.0 && c.1 > a + 4.0), "a clip in the singing section at {a}: {clips:?}");
        }
    }
    assert!(sung >= 2, "{sung}");
    // the chops are on a Ling Slicer kit, tuned by it to the chords: each note played alone at its beat and measured
    assert!(r.contains("chops on Ling Slicer:"), "{r}");
    let heard = heard_chops(&mut d, ch, "chops");
    assert!(heard.len() >= 10, "{}", heard.len());
    let mut cache = vec![];
    let mut on_chord = 0;
    for (b, m) in &heard {
        let pcs = harmony_at(&mut d, *b, &mut cache);
        if pcs.is_empty() || cents_to(*m, &pcs) < 30.0 {
            on_chord += 1;
        } else if std::env::var("CHOPS_DEBUG").is_ok() {
            println!("off at {b:.2}: heard {m:.2} over {pcs:?}");
        }
    }
    println!("chops: {} measured, {on_chord} on a tone sounding with them", heard.len());
    assert!(on_chord as f64 >= 0.85 * heard.len() as f64, "{on_chord} of {}", heard.len());
    // the stressed hook: harmony voices whose notes land on tones sounding with them
    let h = track(&mut d, "Harmony · synth vocal 100bpm").expect("a harmony track");
    let (mut segs, mut fit) = (0, 0);
    for c in 0..num(call(&mut d, "daw_clip_count", &[n(h as f64)])) as usize {
        let file = text(call(&mut d, "daw_clip_file", &[n(h as f64), n(c as f64)]));
        let start = num(call(&mut d, "daw_clip_get", &[n(h as f64), n(c as f64), s("start")]));
        let (x, rate) = ling_daw::dsp::sampler::load_audio(&file).expect("harmony wav");
        let mono: Vec<f32> = x.chunks(2).map(|f| 0.5 * (f[0] + f[1])).collect();
        for sg in ling_daw::medley::acapella::segments(&x, rate, 0, x.len() / 2) {
            if sg.midi.is_none() || sg.b - sg.a < rate as usize / 10 {
                continue;
            }
            let Some(f) = ling_daw::chops::chop_f0(&mono, sg.a, sg.b, rate) else { continue };
            let b = start + (sg.a + sg.b) as f64 * 0.5 / rate as f64 * 100.0 / 60.0;
            let pcs = harmony_at(&mut d, b, &mut cache);
            segs += 1;
            if pcs.is_empty() || cents_to(f.midi, &pcs) < 35.0 {
                fit += 1;
            }
        }
    }
    println!("harmonies: {segs} notes measured, {fit} on a tone sounding with them");
    assert!(segs >= 4 && fit as f64 >= 0.8 * segs as f64, "{fit} of {segs}");
}

#[test]
fn a_vocoded_line_feeds_a_chord_synth() {
    if !at_lsx() {
        return;
    }
    let vocal = acapella();
    let mut d = Daw::new();
    let r = build(&mut d, &format!("layering=0.8;rolls=0.9;genre=house;acap={vocal};stress=0.6;stressways=vocoder;recipes=0;feel=0;story=a storm"), 9.0);
    assert!(r.contains("vocoder"), "{r}");
    let vt = track(&mut d, "Vocoder · synth vocal").expect("the carrier");
    let src = track(&mut d, "Vocoder in · synth vocal").expect("the line's track");
    assert!(num(call(&mut d, "daw_track_get", &[n(src as f64), s("mute")])) > 0.5, "the modulator is not heard itself");
    let fx: Vec<String> = (0..num(call(&mut d, "daw_fx_count", &[n(vt as f64)])) as usize).map(|i| text(call(&mut d, "daw_fx_name", &[n(vt as f64), n(i as f64)]))).collect();
    assert!(fx.iter().any(|f| f == "Vocoder"), "{fx:?}");
    let routed = (0..num(call(&mut d, "daw_route_count", &[])) as usize).any(|i| {
        num(call(&mut d, "daw_route_get", &[n(i as f64), s("kind")])) == 3.0 && num(call(&mut d, "daw_route_get", &[n(i as f64), s("from")])) as usize == src && num(call(&mut d, "daw_route_get", &[n(i as f64), s("to")])) as usize == vt
    });
    assert!(routed, "the line is the vocoder's modulator");
    // heard: the carrier speaks while the line plays, and is silent before it
    let a0 = num(call(&mut d, "daw_clip_get", &[n(src as f64), n(0.0), s("start")]));
    let a1 = num(call(&mut d, "daw_clip_get", &[n(src as f64), n(0.0), s("end")]));
    call(&mut d, "daw_track_set", &[n(vt as f64), s("solo"), n(1.0)]);
    let dir = std::env::temp_dir().join("lsx_medley_acapella");
    let on = dir.join("vocoder_on.wav").to_string_lossy().replace('\\', "/");
    let off = dir.join("vocoder_off.wav").to_string_lossy().replace('\\', "/");
    call(&mut d, "daw_bounce", &[s(&on), n(a0 + 0.5), n(a1 - 0.5), n(0.0), n(16.0), s("wav")]);
    call(&mut d, "daw_bounce", &[s(&off), n((a0 - 6.0).max(0.0)), n(a0 - 2.0), n(0.0), n(16.0), s("wav")]);
    let (ron, roff) = (rms(&on), rms(&off));
    println!("vocoder rms during the line {ron:.4}, before it {roff:.4}");
    assert!(ron > 0.004 && ron > 4.0 * roff, "{ron} vs {roff}");
}

#[test]
fn the_story_feel_and_recipes_change_the_medley() {
    if !at_lsx() {
        return;
    }
    let vocal = acapella();
    let mut a = Daw::new();
    let ra = build(&mut a, &format!("layering=0.8;rolls=0.9;genre=dubstep;feelarc=storm;arc=battle;recipes=1;acap={vocal};story=two rivals clash in a storm"), 3.0);
    let mut b = Daw::new();
    let rb = build(&mut b, &format!("layering=0.8;rolls=0.9;genre=dubstep;feelarc=lullaby;arc=dream;recipes=0;acap={vocal};story=a quiet dream at dawn"), 3.0);
    println!("{ra}\n{rb}");
    assert!(ra.contains("; sound recipes: ") && !rb.contains("; sound recipes: "), "recipes on and off");
    assert!(ra.contains("story: battle") && rb.contains("story: dream"), "the arcs");
    assert!(ra.contains("; feel: storm") && rb.contains("; feel: lullaby"), "the feels");
    let plan = |r: &str| r.split("; vocal plan").nth(1).map(|x| x.split("; ").next().unwrap_or("").to_string()).unwrap_or_default();
    assert_ne!(plan(&ra), plan(&rb), "the story moves where the vocal sings");
    // the recipes wrote some layered clips their way and set the master moves
    assert!(ra.contains("written its way") || ra.contains("master:"), "{ra}");
    let na = names(&mut a);
    let nb = names(&mut b);
    assert_ne!(na, nb);
}

#[test]
fn an_edm_medley_gets_one_hype_shout_per_drop_and_the_chops_answer_the_vocal() {
    if !at_lsx() {
        return;
    }
    let vocal = acapella();
    // the vocal sings everywhere (replace=0) and the drops and builds layer chops with it, every other one from a second acapella
    // (chopped only): those answer the singer between its lines
    let mut d = Daw::new();
    let r = build(&mut d, &format!("layering=0.8;rolls=0.9;genre=dubstep;chops=chops;acap={vocal};acapx={vocal};replace=0;stress=0;recipes=0;feel=0;hype=clean;story=a storm"), 4.0);
    println!("{r}");
    assert!(r.contains("left out to answer the vocal between its lines"), "{r}");
    let secs = sections(&mut d);
    let drops: Vec<f64> = secs.iter().skip(1).filter(|s| s.2 == "drop").map(|s| s.0).collect();
    assert!(!drops.is_empty(), "{secs:?}");
    let h = track(&mut d, "Hype Shout").expect("a hype shout track");
    assert_eq!(text(call(&mut d, "daw_track_kind", &[n(h as f64)])), "slicer");
    assert!(r.contains("hype shout: Hype MC Clean before"), "{r}");
    // one phrase per drop, ending on its downbeat
    let clips: Vec<(f64, f64)> = (0..num(call(&mut d, "daw_clip_count", &[n(h as f64)])) as usize)
        .map(|c| (num(call(&mut d, "daw_clip_get", &[n(h as f64), n(c as f64), s("start")])), num(call(&mut d, "daw_clip_get", &[n(h as f64), n(c as f64), s("end")]))))
        .collect();
    assert_eq!(clips.len(), drops.len(), "{clips:?} / {drops:?}");
    for (c, d0) in clips.iter().zip(&drops) {
        assert!((c.1 - d0).abs() < 1e-6, "the shout ends on the drop: {c:?} {d0}");
    }
    // pop gets none, and nothing is shouted unless asked
    let mut p = Daw::new();
    let rp = build(&mut p, &format!("layering=0.8;rolls=0.9;genre=pop;acap={vocal};recipes=0;feel=0;hype=clean;story=a storm"), 4.0);
    assert!(!rp.contains("hype shout") && track(&mut p, "Hype Shout").is_none(), "{rp}");
    let mut q = Daw::new();
    let rq = build(&mut q, &format!("layering=0.8;rolls=0.9;genre=dubstep;acap={vocal};recipes=0;feel=0;story=a storm"), 4.0);
    assert!(!rq.contains("hype shout") && track(&mut q, "Hype Shout").is_none(), "{rq}");
}

#[test]
fn a_medley_reports_its_edits_and_off_leaves_them_out() {
    if !at_lsx() {
        return;
    }
    let vocal = acapella();
    let mut d = Daw::new();
    let r = build(&mut d, &format!("layering=0.8;rolls=0.9;genre=dubstep;chops=chops;acap={vocal};acapx={vocal};replace=0;stress=0;recipes=0;feel=0;edits=auto;story=a storm"), 4.0);
    println!("{r}");
    assert!(r.contains("; edits: "), "{r}");
    let clause = r.split("; edits: ").nth(1).unwrap_or("").split(';').next().unwrap_or("").to_string();
    let total: usize = clause.split(", ").filter_map(|p| p.split(' ').next().and_then(|x| x.parse::<usize>().ok())).sum();
    let secs = sections(&mut d);
    assert!(total >= 2 && total <= 3 * secs.len(), "{total} edits over {} sections: {clause}", secs.len());
    // an air pocket stops every part before its drop: nothing of the medley starts in the half beat before
    if clause.contains("air pocket") {
        let drops: Vec<f64> = secs.iter().skip(1).filter(|s| s.2 == "drop").map(|s| s.0).collect();
        let nm = names(&mut d);
        let mut silent = 0;
        for d0 in &drops {
            let busy = (0..nm.len()).filter(|t| !nm[*t].starts_with("Acapella")).any(|t| notes(&mut d, t, "").iter().any(|n| n.0 >= d0 - 0.5 + 1e-6 && n.0 < d0 - 1e-6));
            silent += (!busy) as usize;
        }
        assert!(silent >= 1, "a pocket somewhere: {drops:?}");
    }
    let mut o = Daw::new();
    let ro = build(&mut o, &format!("layering=0.8;rolls=0.9;genre=dubstep;chops=chops;acap={vocal};acapx={vocal};replace=0;stress=0;recipes=0;feel=0;edits=off;story=a storm"), 4.0);
    assert!(!ro.contains("edits:") && track(&mut o, "Chops edits").is_none(), "{ro}");
}

#[test]
#[ignore]
fn render_medley_popculture() {
    if !at_lsx() {
        return;
    }
    let dir = std::env::var("MEDLEY_DEMO_DIR").unwrap_or_else(|_| "renders/medley_popculture".into());
    std::fs::create_dir_all(&dir).unwrap();
    let vocal = format!("{dir}/0_synthetic_acapella_100bpm.wav");
    make_acapella(&vocal);
    let mut d = Daw::new();
    let opts = format!("layering=0.85;rolls=0.9;genre=house;feelarc=triumph;recipes=1;acap={vocal};replace=1;stress=0.7;story=a lonely night in the city turns into a crowd singing along at dawn");
    let r = build(&mut d, &opts, 11.0);
    let mut report = format!("{r}\n\n");
    let end = num(call(&mut d, "daw_end_beat", &[]));
    call(&mut d, "daw_bounce", &[s(&format!("{dir}/1_medley.wav")), n(0.0), n(end), n(1.0), n(16.0), s("wav")]);
    // the vocal side alone: the acapella, its chops and the stressed lines
    let nm = names(&mut d);
    for (t, name) in nm.iter().enumerate() {
        let vocal_side = ["Acapella", "Chops", "Harmony", "Double", "Octave", "Telephone", "Vocoder"].iter().any(|p| name.starts_with(p));
        call(&mut d, "daw_track_set", &[n(t as f64), s("solo"), n(vocal_side as i32 as f64)]);
    }
    call(&mut d, "daw_bounce", &[s(&format!("{dir}/2_vocal_side.wav")), n(0.0), n(end), n(1.0), n(16.0), s("wav")]);
    // where the acapella sings, section by section
    let at_ = track(&mut d, "Acapella").unwrap();
    let vol = lane(&mut d, at_, "vol");
    for (a, b, tag) in sections(&mut d) {
        report.push_str(&format!("{a:>6.1}..{b:>6.1} {tag:<6} acapella level {:.2}\n", at(&vol, (a + b) * 0.5)));
    }
    let ch = track(&mut d, "Chops · ").unwrap();
    let heard = heard_chops(&mut d, ch, "chops");
    let mut cache = vec![];
    let on_chord = heard.iter().filter(|(b, m)| {
        let pcs = harmony_at(&mut d, *b, &mut cache);
        pcs.is_empty() || cents_to(*m, &pcs) < 30.0
    }).count();
    report.push_str(&format!("\nchops measured alone on their Slicer: {} pitched notes, {on_chord} within 30 cents of a tone sounding with them\n", heard.len()));
    std::fs::write(format!("{dir}/summary.txt"), &report).unwrap();
    println!("{report}");
}
