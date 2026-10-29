//! Per-note expression (gesture.rs): pitch gestures, pressure and timbre on notes - saved and loaded, kept by note edits, played
//! by the engine (the measured f0 follows the curve), exported / imported as MIDI, and written by the generators only on purpose.
use ling_daw::dsp::Instrument;
use ling_daw::model::Curve;
use ling_daw::*;

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.into())
}
fn call(d: &mut Daw, name: &str, a: &[Val]) -> Val {
    d.call(name, a).unwrap_or_else(|| panic!("{name} not handled")).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn list(v: &Val) -> Vec<f64> {
    if let Val::List(l) = v { l.iter().map(|x| if let Val::Num(y) = x { *y } else { f64::NAN }).collect() } else { vec![] }
}
fn num(v: &Val) -> f64 {
    if let Val::Num(x) = v { *x } else { f64::NAN }
}
/// the curves of one kind per note, from daw_nx_get
fn curves(d: &mut Daw, t: usize, c: usize, which: &str) -> Vec<Curve> {
    let v = list(&call(d, "daw_nx_get", &[n(t as f64), n(c as f64), s(which)]));
    let mut out = vec![];
    let mut p = 0;
    while p < v.len() {
        let k = v[p] as usize;
        out.push((0..k).map(|i| (v[p + 1 + i * 2] as f32, v[p + 2 + i * 2] as f32)).collect());
        p += 1 + k * 2;
    }
    out
}

fn saw_sampler(sr: f32) -> Box<dyn Instrument> {
    let mut sm = dsp::sampler::Sampler::new(sr);
    let n = sr as usize;
    let mut st = Vec::with_capacity(n * 2);
    for i in 0..n {
        let v = ((i as f32 * 220.0 / sr).fract() * 2.0 - 1.0) * 0.5;
        st.extend([v, v]);
    }
    let mut z = dsp::sampler::Zone::new("saw", "");
    z.root = 57;
    z.loop_mode = 1;
    z.loop_end = n as u32;
    sm.add_loaded(z, st, sr as u32);
    Box::new(sm)
}

fn inst(kind: &str, sr: f32) -> Box<dyn Instrument> {
    let mut i = if kind == "sampler" { saw_sampler(sr) } else { dsp::make_instrument(kind, sr).unwrap() };
    for (p, v) in [("vib depth", 0.0), ("vib", 0.0), ("vibrato", 0.0), ("lfo pitch", 0.0), ("drift", 0.0), ("scoop", 0.0), ("sustain", 1.0), ("amp s", 1.0)] {
        if i.get_param(p).is_some() {
            i.set_param(p, v);
        }
    }
    i
}

/// one engine, one track, the given notes at 120 bpm; the mono bounce
fn render(kind: &str, notes: Vec<model::Note>) -> Vec<f32> {
    let sr = 48000.0;
    let mut e = Engine::new(sr);
    e.project.bpm = 120.0;
    let mut t = Track::new(kind, TrackKind::Instrument, 0, Some(inst(kind, sr)));
    t.inst_kind = kind.to_string();
    let mut c = Clip::midi(0.0, 8.0, "c");
    if let ClipData::Midi(v) = &mut c.data {
        *v = notes;
    }
    t.clips.push(c);
    e.project.tracks.push(t);
    let st = e.bounce(0.2);
    st.chunks(2).map(|f| (f[0] + f[1]) * 0.5).collect()
}

fn cents(a: f32, b: f32) -> f32 {
    1200.0 * (a / b).log2()
}

#[test]
fn the_engine_plays_the_curve_on_every_instrument() {
    // a 4-beat note (2 s) at A3: from 3 below up to the note by a quarter of it, held, then 5 up by its end
    let curve: Curve = vec![(0.0, -3.0), (0.25, 0.0), (0.5, 0.0), (1.0, 5.0)];
    for kind in ["synth", "chip", "sampler", "pluck", "voice"] {
        let mut note = model::Note::new(0.0, 4.0, 57, 0.9);
        note.bend = curve.clone();
        let mono = render(kind, vec![note]);
        let p = vocal::track_pitch(&mono, 48000.0);
        let mut errs = vec![];
        for k in 1..20 {
            let f = k as f32 / 20.0;
            // the pluck decays: judge it over its first half
            if kind == "pluck" && f > 0.6 {
                continue;
            }
            let t = f as f64 * 2.0;
            let heard = p.at(t);
            if heard <= 0.0 {
                continue;
            }
            let want = 220.0 * 2f32.powf(gesture::at(&curve, f) / 12.0);
            let mut e = cents(heard, want);
            // the tracker may land an octave off on a bright wave: judge the pitch class
            while e > 600.0 {
                e -= 1200.0;
            }
            while e < -600.0 {
                e += 1200.0;
            }
            errs.push((f, e));
        }
        // the plucked string fades: its scoop and hold are judged
        assert!(errs.len() >= if kind == "pluck" { 4 } else { 6 }, "{kind}: too few voiced frames {errs:?}");
        let bad: Vec<_> = errs.iter().filter(|x| x.1.abs() > 45.0).collect();
        assert!(bad.len() * 5 <= errs.len(), "{kind}: f0 does not follow the curve: {errs:?}");
        // and the gesture really moved it: the end is ~5 semitones above the held middle
        let (mid, end) = (p.at(0.9), p.at(1.9));
        if mid > 0.0 && end > 0.0 && kind != "pluck" {
            let up = cents(end, mid);
            assert!((up - 500.0).abs() < 80.0 || (up - 500.0 + 1200.0).abs() < 80.0 || (up - 500.0 - 1200.0).abs() < 80.0, "{kind}: rose {up} cents");
        }
    }
}

#[test]
fn a_slide_reaches_the_next_note_and_a_flat_curve_changes_nothing() {
    // A3 for 2 beats sliding to C4, then C4 (detached): the end of the first note is at C4
    let mut a = model::Note::new(0.0, 2.0, 57, 0.9);
    let b = model::Note::new(2.05, 2.0, 60, 0.9);
    let mut both = vec![a.clone(), b.clone()];
    assert!(gesture::apply_shape(&mut both, 0, "slide", 0.0));
    a.bend = both[0].bend.clone();
    let mono = render("synth", vec![a, b.clone()]);
    let p = vocal::track_pitch(&mono, 48000.0);
    let (start, end) = (p.at(0.4), p.at(0.985));
    assert!(cents(start, 220.0).abs() < 30.0, "starts at A3: {start}");
    assert!(cents(end, 261.63).abs() < 60.0, "slides to C4: {end}");
    // notes with a flat (all-zero) curve render bit-identically to notes without one, on every instrument
    for kind in ["synth", "chip", "sampler", "pluck", "voice", "script"] {
        let plain = vec![model::Note::new(0.0, 1.0, 57, 0.8), model::Note::new(0.5, 1.0, 64, 0.7), model::Note::new(2.0, 1.5, 60, 0.9)];
        let mut flat = plain.clone();
        for x in flat.iter_mut() {
            x.bend = vec![(0.0, 0.0), (1.0, 0.0)];
            x.press = vec![(0.0, 0.0)];
            x.timbre = vec![(0.0, 0.5)];
        }
        let (r1, r2) = (render(kind, plain), render(kind, flat));
        assert_eq!(r1.len(), r2.len());
        let worst = r1.iter().zip(&r2).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert_eq!(worst, 0.0, "{kind}: a flat curve must not change the sound");
    }
}

#[test]
fn pressure_and_timbre_reach_the_instruments() {
    // pressure makes the synth louder, timbre (up) brighter
    let rms = |x: &[f32]| (x.iter().map(|v| v * v).sum::<f32>() / x.len().max(1) as f32).sqrt();
    let plain = render("synth", vec![model::Note::new(0.0, 2.0, 57, 0.7)]);
    let mut pn = model::Note::new(0.0, 2.0, 57, 0.7);
    pn.press = vec![(0.0, 1.0)];
    let pressed = render("synth", vec![pn]);
    assert!(rms(&pressed[4800..40000]) > rms(&plain[4800..40000]) * 1.2);
    let mut tn = model::Note::new(0.0, 2.0, 57, 0.7);
    tn.timbre = vec![(0.0, 1.0)];
    let bright = render("synth", vec![tn]);
    let hf = |x: &[f32]| rms(&x.windows(2).map(|w| w[1] - w[0]).collect::<Vec<_>>());
    assert!(hf(&bright[4800..40000]) > hf(&plain[4800..40000]) * 1.3);
}

#[test]
fn curves_are_saved_loaded_and_old_projects_still_load() {
    let sr = 48000.0;
    let mut e = Engine::new(sr);
    let mut t = Track::new("lead", TrackKind::Instrument, 0, Some(dsp::make_instrument("synth", sr).unwrap()));
    t.inst_kind = "synth".into();
    let mut c = Clip::midi(0.0, 8.0, "c");
    let mut g = model::Note::new(0.0, 1.0, 60, 0.8);
    g.bend = gesture::shape("scoop", 3.0, 1.0).unwrap();
    g.press = vec![(0.0, 0.2), (1.0, 0.9)];
    g.timbre = vec![(0.5, 0.7)];
    if let ClipData::Midi(v) = &mut c.data {
        v.push(g.clone());
        v.push(model::Note::new(1.0, 1.0, 62, 0.8));
    }
    t.clips.push(c);
    e.project.tracks.push(t);
    let doc = project::to_doc(&e);
    let json = serde_json::to_string(&doc).unwrap();
    assert_eq!(json.matches("\"bend\":[").count(), 1, "empty curves are not written");
    assert_eq!(json.matches("\"press\":[").count(), 1);
    let back: project::ProjectDoc = serde_json::from_str(&json).unwrap();
    let mut e2 = Engine::new(sr);
    project::apply_doc(&mut e2, &back, None, &mut Default::default());
    let ClipData::Midi(ns) = &e2.project.tracks[0].clips[0].data else { panic!() };
    assert_eq!(ns[0], g);
    assert!(!ns[1].has_expr());
    // a project written before the curves existed: no such fields
    let old = json.replace(&format!(",\"bend\":{}", serde_json::to_string(&g.bend).unwrap()), "").replace(&format!(",\"press\":{}", serde_json::to_string(&g.press).unwrap()), "").replace(&format!(",\"timbre\":{}", serde_json::to_string(&g.timbre).unwrap()), "");
    assert!(!old.contains("\"bend\":[") && !old.contains("\"timbre\":["));
    let od: project::ProjectDoc = serde_json::from_str(&old).unwrap();
    let mut e3 = Engine::new(sr);
    project::apply_doc(&mut e3, &od, None, &mut Default::default());
    let ClipData::Midi(ns) = &e3.project.tracks[0].clips[0].data else { panic!() };
    assert!(ns.iter().all(|x| !x.has_expr()) && ns.len() == 2);
}

#[test]
fn note_edits_keep_the_gestures() {
    let mut d = Daw::new();
    let t = num(&call(&mut d, "daw_track_add", &[s("synth")]));
    let c = num(&call(&mut d, "daw_clip_add_midi", &[n(t), n(0.0), n(8.0)]));
    for (st, key) in [(0.0, 57.0), (2.0, 60.0), (4.0, 64.0)] {
        call(&mut d, "daw_note_add", &[n(t), n(c), n(st), n(2.0), n(key), n(0.8)]);
    }
    assert_eq!(num(&call(&mut d, "daw_nx_shape", &[n(t), n(c), Val::List(vec![n(0.0), n(1.0)]), s("scoop"), n(0.0)])), 2.0);
    call(&mut d, "daw_nx_shape", &[n(t), n(c), Val::List(vec![n(2.0)]), s("riser"), n(7.0)]);
    call(&mut d, "daw_nx_draw", &[n(t), n(c), n(1.0), s("press"), n(0.0), n(0.3), n(1.0), n(0.8)]);
    let flags = list(&call(&mut d, "daw_nx_flags", &[n(t), n(c)]));
    assert_eq!(flags, vec![1.0, 3.0, 1.0]);
    let (ti, ci) = (t as usize, c as usize);
    let b0 = curves(&mut d, ti, ci, "bend");
    assert!((gesture::depth(&b0[2]) - 7.0).abs() < 1e-4);
    // move / transpose / quantize / resize: the curves stay with their notes
    call(&mut d, "daw_notes_apply", &[n(t), n(c), s("move"), Val::List(vec![n(0.0)]), Val::List(vec![n(0.0), n(57.0), n(2.0), n(60.0), n(4.0), n(64.0)]), Val::List(vec![n(2.0), n(2.0), n(2.0)]), n(0.25), n(12.0)]);
    call(&mut d, "daw_note_set", &[n(t), n(c), n(0.0), s("start"), n(0.0)]);
    call(&mut d, "daw_note_set", &[n(t), n(c), n(1.0), s("key"), n(48.0)]);
    assert_eq!(curves(&mut d, ti, ci, "bend"), b0);
    // the clipboard text copies them (the roll's copy / paste / duplicate)
    let txt = call(&mut d, "daw_nx_text", &[n(t), n(c), n(1.0)]);
    let ni = num(&call(&mut d, "daw_note_add", &[n(t), n(c), n(6.0), n(2.0), n(50.0), n(0.8)]));
    call(&mut d, "daw_nx_set_text", &[n(t), n(c), n(ni), txt]);
    let b1 = curves(&mut d, ti, ci, "bend");
    let p1 = curves(&mut d, ti, ci, "press");
    assert_eq!(b1[3], b0[1]);
    assert_eq!(p1[3], p1[1]);
    // depth and clear
    call(&mut d, "daw_nx_depth", &[n(t), n(c), Val::List(vec![n(0.0)]), n(5.0)]);
    assert!((gesture::depth(&curves(&mut d, ti, ci, "bend")[0]) - 5.0).abs() < 1e-4);
    call(&mut d, "daw_nx_clear", &[n(t), n(c), Val::List(vec![n(3.0)]), s("all")]);
    assert_eq!(list(&call(&mut d, "daw_nx_flags", &[n(t), n(c)]))[3], 0.0);
    // splitting the clip through the riser note (4..6): both halves keep the part of the curve that sounds there
    let right = num(&call(&mut d, "daw_clip_split", &[n(t), n(c), n(5.0)]));
    assert!(right > 0.0);
    let l = curves(&mut d, ti, ci, "bend");
    let r = curves(&mut d, ti, right as usize, "bend");
    let rise = gesture::shape("riser", 7.0, 2.0).unwrap();
    let lr = l.last().unwrap();
    assert!((gesture::at(lr, 1.0) - gesture::at(&rise, 0.5)).abs() < 1e-3, "{lr:?}");
    assert!((gesture::at(&r[0], 0.0) - gesture::at(&rise, 0.5)).abs() < 1e-3 && (gesture::at(&r[0], 1.0) - 7.0).abs() < 1e-3, "{r:?}");
    // the velocity line: relative keeps the accents
    let v = list(&call(&mut d, "daw_nx_vel_line", &[n(t), n(c), Val::List(vec![n(0.0), n(1.0)]), Val::List(vec![n(0.4), n(0.7)]), Val::List(vec![n(0.4), n(0.6)]), n(1.0), n(0.0)]));
    assert!((v[1] - v[0] - 0.5).abs() < 1e-5 && (v[0] - 0.25).abs() < 1e-5, "accent 0.3 + line rise 0.2: {v:?}");
}

#[test]
fn midi_round_trip_keeps_the_expression() {
    let sr = 48000.0;
    let mut e = Engine::new(sr);
    let mut t = Track::new("lead", TrackKind::Instrument, 0, Some(dsp::make_instrument("synth", sr).unwrap()));
    t.inst_kind = "synth".into();
    let mut c = Clip::midi(0.0, 8.0, "c");
    let mut a = model::Note::new(0.0, 2.0, 57, 0.8);
    a.bend = vec![(0.0, -2.0), (0.5, 0.0), (1.0, 7.0)];
    a.press = vec![(0.0, 0.0), (1.0, 1.0)];
    let mut b = model::Note::new(2.0, 2.0, 60, 0.8);
    b.timbre = vec![(0.0, 0.5), (1.0, 1.0)];
    if let ClipData::Midi(v) = &mut c.data {
        v.push(a.clone());
        v.push(b.clone());
    }
    t.clips.push(c);
    e.project.tracks.push(t);
    let dir = std::env::temp_dir().join(format!("lsx_gest_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("g.mid");
    midi_io::export_track(&e, 0, path.to_str().unwrap()).unwrap();
    let m = midi_import::parse(&std::fs::read(&path).unwrap()).unwrap();
    let _ = std::fs::remove_dir_all(&dir);
    let tr = &m.tracks[0];
    assert!(tr.bend_auto.is_empty(), "a monophonic part's wheel becomes per-note gestures");
    assert_eq!(tr.bend_range, 24.0);
    let ia = tr.notes.iter().find(|x| x.key == 57).unwrap();
    let ib = tr.notes.iter().find(|x| x.key == 60).unwrap();
    for f in [0.0f32, 0.25, 0.5, 0.75, 0.95] {
        assert!((gesture::at(&ia.bend, f) - gesture::at(&a.bend, f)).abs() < 0.15, "bend at {f}: {:?}", ia.bend);
        assert!((gesture::at(&ia.press, f) - gesture::at(&a.press, f)).abs() < 0.05, "press at {f}: {:?}", ia.press);
        assert!((gesture::at(&ib.timbre, f) - gesture::at(&b.timbre, f)).abs() < 0.05, "timbre at {f}: {:?}", ib.timbre);
    }
    assert!(ib.bend.is_empty() && ia.timbre.is_empty());
}

#[test]
fn generators_write_gestures_only_where_the_rules_say() {
    use ling_daw::compose::Kind;
    let mut seen_scoop = 0;
    let mut seen_slide = 0;
    for (prompt, genre) in [("future bass anthem in F minor", "future bass"), ("dark techno at 132 bpm", "techno"), ("dubstep with a laser lead", "dubstep"), ("lofi chill beat to study to", "lofi")] {
        let plan = songgen::plan(prompt, "", &[], None, 1.0, 11);
        assert_eq!(plan.genre, genre);
        for (pi, part) in plan.parts.iter().enumerate() {
            let kind = songgen::role_kind(&part.role);
            for sec in &plan.sections {
                let notes = songgen::section_notes(&plan, pi, sec, None);
                for (i, c) in songgen::note_gestures(&plan, pi, sec, &notes) {
                    assert!(i < notes.len());
                    let first = gesture::at(&c, 0.0);
                    let last = gesture::at(&c, 1.0);
                    if part.role == "riser" {
                        assert!(last > 6.0, "the riser rises: {c:?}");
                    } else if first < -0.5 {
                        // a scoop: future bass chords / pads only
                        assert!(genre == "future bass" && matches!(kind, Kind::Chords | Kind::Pad), "{genre} {}: scoop", part.role);
                        seen_scoop += 1;
                    } else if genre == "dubstep" {
                        assert!(kind == Kind::Melody && last <= -6.0, "{genre} {}: only the lead dives", part.role);
                    } else {
                        // a slide: an 808 or an EDM lead, to the next note within the slide range
                        assert!(genre == "future bass" && (part.role.contains("sub") || part.role.contains("lead")), "{genre} {}: slide", part.role);
                        assert!(last.abs() >= 1.0 && last.abs() <= 12.0);
                        seen_slide += 1;
                    }
                }
            }
        }
        // a lofi song (no expression rule asks) and techno have no gestures outside the riser
        if genre == "lofi" || genre == "techno" {
            for (pi, part) in plan.parts.iter().enumerate() {
                if part.role == "riser" {
                    continue;
                }
                for sec in &plan.sections {
                    let notes = songgen::section_notes(&plan, pi, sec, None);
                    assert!(songgen::note_gestures(&plan, pi, sec, &notes).is_empty(), "{genre} {}", part.role);
                }
            }
        }
    }
    assert!(seen_scoop > 0, "future bass chords scoop");
    assert!(seen_slide > 0, "808 / lead slides");
}
