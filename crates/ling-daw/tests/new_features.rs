//! Tests for the newest builtins: batch note edits, structure conformer, effect automation helpers, Rainbow Hypergrowl.
use ling_daw::{Daw, Val};

fn n(x: f64) -> Val {
    Val::Num(x)
}
fn s(x: &str) -> Val {
    Val::Str(x.into())
}
fn l(v: &[f64]) -> Val {
    Val::List(v.iter().map(|x| Val::Num(*x)).collect())
}
fn call(d: &mut Daw, name: &str, a: &[Val]) -> Val {
    d.call(name, a).unwrap_or_else(|| panic!("{name} not handled")).unwrap_or_else(|e| panic!("{name}: {e}"))
}
fn nums(v: &Val) -> Vec<f64> {
    match v {
        Val::List(l) => l.iter().map(|x| if let Val::Num(x) = x { *x } else { f64::NAN }).collect(),
        _ => vec![],
    }
}
fn num(v: Val) -> f64 {
    if let Val::Num(x) = v {
        x
    } else {
        f64::NAN
    }
}

fn song_with_notes(count: usize) -> Daw {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(0.0), n(0.0), n(64.0)]);
    for i in 0..count {
        call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(i as f64 * 0.25), n(0.25), n(40.0 + (i % 30) as f64), n(0.8)]);
    }
    d
}

#[test]
fn a_whole_selection_moves_resizes_and_stretches_in_one_call() {
    let count = 2000;
    let mut d = song_with_notes(count);
    let notes = nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)]));
    let origin: Vec<f64> = (0..count).flat_map(|i| [notes[i * 7], notes[i * 7 + 2]]).collect();
    let lens: Vec<f64> = (0..count).map(|i| notes[i * 7 + 1]).collect();
    let sel: Vec<f64> = (0..count).map(|i| i as f64).collect();
    let t0 = std::time::Instant::now();
    call(&mut d, "daw_notes_apply", &[n(0.0), n(0.0), s("move"), l(&sel), l(&origin), l(&lens), n(2.0), n(5.0)]);
    let took = t0.elapsed();
    let after = nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)]));
    assert!((after[0] - 2.0).abs() < 1e-9 && (after[2] - 45.0).abs() < 1e-9, "{:?}", &after[..7]);
    assert!((after[(count - 1) * 7] - ((count - 1) as f64 * 0.25 + 2.0)).abs() < 1e-9);
    assert!(took.as_millis() < 200, "a 2000-note move took {took:?}");
    // moving is always from the origin: the same call again does not pile up
    call(&mut d, "daw_notes_apply", &[n(0.0), n(0.0), s("move"), l(&sel), l(&origin), l(&lens), n(2.0), n(5.0)]);
    assert!((nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)]))[0] - 2.0).abs() < 1e-9);
    // below zero is clamped
    call(&mut d, "daw_notes_apply", &[n(0.0), n(0.0), s("move"), l(&sel), l(&origin), l(&lens), n(-9.0), n(-99.0)]);
    let a2 = nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)]));
    assert!(a2[0] >= 0.0 && a2[2] >= 0.0);
    // resize and stretch work from the original lengths / starts
    call(&mut d, "daw_notes_apply", &[n(0.0), n(0.0), s("resize"), l(&sel[..10]), l(&origin), l(&lens), n(0.5), n(0.25)]);
    let a3 = nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)]));
    assert!((a3[1] - 0.75).abs() < 1e-9 && (a3[10 * 7 + 1] - 0.25).abs() < 1e-9);
    call(&mut d, "daw_notes_apply", &[n(0.0), n(0.0), s("stretch"), l(&sel), l(&origin), l(&lens), n(0.0), n(2.0)]);
    let a4 = nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)]));
    assert!((a4[7] - 0.5).abs() < 1e-9 && (a4[8] - 0.5).abs() < 1e-9, "{:?}", &a4[7..14]);
}

#[test]
fn rainbow_hypergrowl_is_an_effect_with_defaults_and_automatable_params() {
    let mut d = song_with_notes(4);
    let types = call(&mut d, "daw_fx_types", &[]);
    assert!(matches!(&types, Val::List(v) if v.iter().any(|x| matches!(x, Val::Str(n) if n == "Rainbow Hypergrowl"))));
    // new tracks already carry a default compressor, so the new effect is the one after it
    let before = num(call(&mut d, "daw_fx_count", &[n(0.0)]));
    call(&mut d, "daw_fx_add", &[n(0.0), s("Rainbow Hypergrowl")]);
    assert_eq!(num(call(&mut d, "daw_fx_count", &[n(0.0)])), before + 1.0);
    let fi = before;
    let params = call(&mut d, "daw_fx_params", &[n(0.0), n(fi)]);
    assert!(matches!(&params, Val::List(v) if v.len() == 104));
    // a lane for its "squash" knob (index 35), and the default to reset to
    assert!(num(call(&mut d, "daw_auto_lane_add", &[n(0.0), s(&format!("fx{fi}.35"))])) >= 0.0);
    assert!((num(call(&mut d, "daw_fx_default", &[n(0.0), n(fi), n(35.0)])) - 0.5).abs() < 1e-6);
    assert_eq!(num(call(&mut d, "daw_fx_default", &[n(0.0), n(fi), n(99.0)])), -1.0);
    call(&mut d, "daw_fx_set", &[n(0.0), n(fi), n(35.0), n(0.1)]);
}

#[test]
fn master_effects_can_be_automated_too() {
    let mut d = song_with_notes(4);
    call(&mut d, "daw_fx_add", &[n(-1.0), s("Reverb")]);
    assert_eq!(num(call(&mut d, "daw_auto_lane_add", &[n(-1.0), s("fx0.0")])), 0.0);
    // not a real master effect parameter: refused
    assert_eq!(num(call(&mut d, "daw_auto_lane_add", &[n(-1.0), s("fx7.0")])), -1.0);
    call(&mut d, "daw_auto_set_points", &[n(-1.0), n(0.0), l(&[0.0, 0.0, 4.0, 1.0])]);
    assert_eq!(nums(&call(&mut d, "daw_auto_points", &[n(-1.0), n(0.0)])), vec![0.0, 0.0, 4.0, 1.0]);
    // turning the master reverb by hand is recorded against the master lane's parameter name
    call(&mut d, "daw_fx_set", &[n(-1.0), n(0.0), n(0.0), n(0.9)]);
}

#[test]
fn defaults_and_shapes() {
    let mut d = song_with_notes(4);
    // a fresh synth's value for a parameter, and -1 for a name it does not have
    let p = call(&mut d, "daw_inst_params", &[n(0.0)]);
    let first = match &p {
        Val::List(v) => match &v[0] {
            Val::Str(x) => x.clone(),
            _ => panic!(),
        },
        _ => panic!(),
    };
    assert!(num(call(&mut d, "daw_inst_default", &[n(0.0), s(&first)])) >= 0.0);
    assert_eq!(num(call(&mut d, "daw_inst_default", &[n(0.0), s("no such knob")])), -1.0);
    // shapes: a sine LFO over 8 beats with a 4 beat period
    call(&mut d, "daw_auto_lane_add", &[n(0.0), s("vol")]);
    let cnt = num(call(&mut d, "daw_auto_shape", &[n(0.0), n(0.0), s("sine"), n(0.0), n(8.0), n(4.0), n(0.2), n(0.8), n(1.0)]));
    assert!(cnt >= 16.0, "{cnt}");
    let pts = nums(&call(&mut d, "daw_auto_points", &[n(0.0), n(0.0)]));
    assert!(pts.chunks(2).all(|c| c[1] >= 0.199 && c[1] <= 0.801 && c[0] >= 0.0 && c[0] <= 8.0));
    assert!(pts.chunks(2).any(|c| (c[1] - 0.8).abs() < 1e-3) && pts.chunks(2).any(|c| (c[1] - 0.2).abs() < 1e-3));
    // a second shape over a later stretch keeps the first
    call(&mut d, "daw_auto_shape", &[n(0.0), n(0.0), s("up"), n(8.0), n(16.0), n(4.0), n(0.0), n(1.0), n(1.0)]);
    let pts2 = nums(&call(&mut d, "daw_auto_points", &[n(0.0), n(0.0)]));
    assert!(pts2.len() > pts.len() && pts2[..4] == pts[..4]);
    for sh in ["tri", "down", "pulse", "steps", "random"] {
        assert!(num(call(&mut d, "daw_auto_shape", &[n(0.0), n(0.0), s(sh), n(0.0), n(8.0), n(2.0), n(0.0), n(1.0), n(3.0)])) > 1.0, "{sh}");
    }
}

#[test]
fn structure_conformer_turns_a_long_backing_part_into_a_repeating_page() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(0.0), n(0.0), n(32.0)]);
    // 8 bars of a 2 bar chord figure, drifting a little
    for rep in 0..4 {
        let o = rep as f64 * 8.0;
        for (t, ks) in [(0.0, [48.0, 55.0, 60.0]), (4.0, [45.0, 52.0, 57.0])] {
            for k in ks {
                call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(o + t), n(3.5), n(k), n(0.7)]);
            }
        }
    }
    let scan = call(&mut d, "daw_arr_struct_scan", &[n(3.0)]);
    let items = match &scan {
        Val::List(v) => v.clone(),
        _ => vec![],
    };
    assert_eq!(items.len() % 5, 0);
    assert!(items.chunks(5).any(|c| matches!(&c[3], Val::Str(k) if k == "pages")), "{scan:?}");
    let applied = num(call(&mut d, "daw_arr_struct_apply", &[n(3.0)]));
    assert!(applied >= 1.0);
    let after = nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)]));
    assert!(after.len() >= 7 * 12 && after.iter().all(|x| x.is_finite()));
}

#[test]
fn harmonized_part_is_built_in_the_chosen_tracks_chords() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    for _ in 0..3 {
        call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    }
    for t in 0..2 {
        call(&mut d, "daw_clip_add_midi", &[n(t as f64), n(0.0), n(8.0)]);
    }
    // track 0: the melody C D E F G E D C; track 1: C / F / G / C chords; track 2 receives the harmony
    for (i, k) in [72.0, 74.0, 76.0, 77.0, 79.0, 76.0, 74.0, 72.0].iter().enumerate() {
        call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(i as f64), n(0.9), n(*k), n(0.8)]);
    }
    for (bar, ch) in [[48.0, 52.0, 55.0], [53.0, 57.0, 60.0], [43.0, 47.0, 50.0], [48.0, 52.0, 55.0]].iter().enumerate() {
        for k in ch {
            call(&mut d, "daw_note_add", &[n(1.0), n(0.0), n(bar as f64 * 2.0), n(2.0), n(*k), n(0.7)]);
        }
    }
    assert!(matches!(call(&mut d, "daw_th_harm_summary", &[s("choir soft")]), Val::Str(x) if x.contains("3 voices")));
    // two selected notes only, one voice
    let made = num(call(&mut d, "daw_th_harmonize", &[n(0.0), n(0.0), l(&[2.0, 3.0]), n(1.0), s("thirds"), n(2.0)]));
    assert_eq!(made, 2.0);
    let notes = nums(&call(&mut d, "daw_clip_notes", &[n(2.0), n(0.0)]));
    assert_eq!(notes.len(), 14);
    assert!(notes[2] < 76.0 && notes[9] < 77.0, "below the melody notes: {notes:?}");
    // no selection = the whole clip, two voices, off every other track
    let made = num(call(&mut d, "daw_th_harmonize", &[n(0.0), n(0.0), l(&[]), n(-1.0), s("two voices"), n(2.0)]));
    assert_eq!(made, 16.0);
    // nothing to write into: refused
    assert_eq!(num(call(&mut d, "daw_th_harmonize", &[n(0.0), n(0.0), l(&[]), n(1.0), s(""), n(9.0)])), 0.0);
}

#[test]
fn growl_presets_random_reset_and_the_live_view() {
    let mut d = song_with_notes(4);
    let before = num(call(&mut d, "daw_fx_count", &[n(0.0)]));
    call(&mut d, "daw_fx_add", &[n(0.0), s("Rainbow Hypergrowl")]);
    let fi = before;
    let names = call(&mut d, "daw_fx_presets", &[s("Rainbow Hypergrowl")]);
    assert!(matches!(&names, Val::List(v) if v.len() >= 8 && matches!(&v[1], Val::Str(x) if x == "Hypergrowl")));
    assert!(matches!(call(&mut d, "daw_fx_presets", &[s("Reverb")]), Val::List(v) if v.is_empty()));
    let get = |d: &mut Daw, i: usize| nums(&call(d, "daw_fx_params", &[n(0.0), n(fi)]))[i * 2 + 1];
    assert_eq!(num(call(&mut d, "daw_fx_preset", &[n(0.0), n(fi), n(5.0)])), 1.0);
    assert!((get(&mut d, 25) - 0.55).abs() < 1e-6, "Octave Shatter turns the grain mix up"); // gmix
    assert!((get(&mut d, 30) - 0.75).abs() < 1e-6); // gpitch
    call(&mut d, "daw_fx_random", &[n(0.0), n(fi), n(7.0)]);
    let r1 = nums(&call(&mut d, "daw_fx_params", &[n(0.0), n(fi)]));
    assert!(r1.chunks(2).all(|c| c[1].is_finite() && c[1] >= 0.0 && c[1] <= 1.0), "{r1:?}");
    assert_eq!(get(&mut d, 43), 1.0, "mix is not randomised");
    call(&mut d, "daw_fx_reset", &[n(0.0), n(fi)]);
    assert!((get(&mut d, 25) - 0.0).abs() < 1e-6 && (get(&mut d, 6) - 0.7083).abs() < 1e-4);
    assert_eq!(num(call(&mut d, "daw_fx_load", &[n(0.0), n(fi), s("C:/no/such/file.wav")])), 0.0);
    assert_eq!(num(call(&mut d, "daw_fx_load", &[n(0.0), n(fi), s("")])), 1.0, "back to the built-in sample");
    assert!(matches!(call(&mut d, "daw_fx_source", &[n(0.0), n(fi)]), Val::Str(x) if x.is_empty()));
    // the live view: 24 band gains, the motion and grain meters, the sample's 24 gains and its reading position
    assert!(matches!(call(&mut d, "daw_fx_view", &[n(0.0), n(fi)]), Val::List(v) if v.len() == 51));
    assert!(matches!(call(&mut d, "daw_fx_view", &[n(0.0), n(0.0)]), Val::List(v) if v.is_empty()), "the compressor has no display");
}

#[test]
fn clips_scenes_and_session_clips_are_named_after_their_sections() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    for _ in 0..2 {
        call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    }
    call(&mut d, "daw_track_set_name", &[n(0.0), s("Violin_Track 5")]);
    for t in 0..2 {
        for k in 0..3 {
            call(&mut d, "daw_clip_add_midi", &[n(t as f64), n(k as f64 * 16.0), n(16.0)]);
            call(&mut d, "daw_note_add", &[n(t as f64), n(k as f64), n(0.0), n(2.0), n(60.0), n(0.8)]);
        }
    }
    call(&mut d, "daw_arr_tags_set", &[s("0:16:verse;16:32:chorus;32:48:verse")]);
    let named = num(call(&mut d, "daw_arr_name", &[n(1.0)]));
    assert_eq!(named, 6.0);
    let names: Vec<String> = (0..3).map(|c| if let Val::Str(x) = call(&mut d, "daw_clip_name", &[n(0.0), n(c as f64)]) { x } else { String::new() }).collect();
    assert_eq!(names, vec!["Violin - Verse 1", "Violin - Chorus", "Violin - Verse 2"]);
    // scenes and session clips
    call(&mut d, "daw_perf_auto", &[n(2.0)]);
    let nsc = num(call(&mut d, "daw_perf_scenes", &[]));
    assert!(nsc >= 2.0);
    call(&mut d, "daw_arr_name", &[n(6.0)]);
    let info = call(&mut d, "daw_perf_slot_info", &[n(0.0), n(0.0)]);
    assert!(matches!(&info, Val::List(v) if matches!(&v[0], Val::Str(x) if x.starts_with("Violin - "))), "{info:?}");
}

#[test]
fn a_solid_clip_is_sliced_into_looping_patterns() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(0.0), n(0.0), n(80.0)]);
    // 8 bars of a 2-bar figure, a gap, then 4 more bars of the figure, all in ONE clip
    for rep in 0..4 {
        let o = rep as f64 * 8.0;
        for (t, k) in [(0.0, 60.0), (1.0, 64.0), (2.0, 67.0), (3.0, 64.0), (4.0, 62.0), (5.0, 65.0), (6.0, 69.0), (7.0, 65.0)] {
            call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(o + t), n(0.4), n(k), n(0.8)]);
        }
    }
    for rep in 0..2 {
        let o = 48.0 + rep as f64 * 8.0;
        for (t, k) in [(0.0, 60.0), (1.0, 64.0), (2.0, 67.0), (3.0, 64.0), (4.0, 62.0), (5.0, 65.0), (6.0, 69.0), (7.0, 65.0)] {
            call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(o + t), n(0.4), n(k), n(0.8)]);
        }
    }
    let scan = call(&mut d, "daw_arr_slice_scan", &[n(8.0)]);
    assert!(matches!(&scan, Val::List(v) if v.len() == 5), "{scan:?}");
    let made = num(call(&mut d, "daw_arr_slice_apply", &[n(8.0)]));
    assert_eq!(made, 2.0, "two clips: the figure, a gap, the figure");
    assert_eq!(num(call(&mut d, "daw_clip_count", &[n(0.0)])), 2.0);
    let c0 = num(call(&mut d, "daw_clip_get", &[n(0.0), n(0.0), s("loop")]));
    assert!(c0 > 7.9 && c0 < 8.1, "the first clip loops a 2-bar page: {c0}");
    assert_eq!(num(call(&mut d, "daw_clip_get", &[n(0.0), n(0.0), s("notes")])), 8.0, "one page of notes, not 32");
    assert_eq!(num(call(&mut d, "daw_clip_get", &[n(0.0), n(1.0), s("start")])), 48.0);
    // the sound is unchanged: the clips unroll to the same notes
    // (a second run finds nothing left to slice)
    assert_eq!(num(call(&mut d, "daw_arr_slice_apply", &[n(8.0)])), 0.0);
}

// ── Articulate: automation for all / random / one track's parameters ─────────────────────────────

fn articulate_song() -> Daw {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    for k in ["growl", "synth", "drums"] {
        call(&mut d, "daw_track_add", &[s(k), s("")]);
    }
    for t in 0..3 {
        call(&mut d, "daw_clip_add_midi", &[n(t as f64), n(0.0), n(32.0)]);
        for i in 0..16 {
            call(&mut d, "daw_note_add", &[n(t as f64), n(0.0), n(i as f64 * 2.0), n(1.0), n(40.0 + (i % 5) as f64 * 3.0), n(0.8)]);
        }
    }
    call(&mut d, "daw_fx_add", &[n(0.0), s("Delay")]);
    d
}

fn lanes(d: &mut Daw, t: f64) -> Vec<String> {
    match call(d, "daw_auto_lanes", &[n(t)]) {
        Val::List(v) => v.into_iter().filter_map(|x| if let Val::Str(s) = x { Some(s) } else { None }).collect(),
        _ => vec![],
    }
}

fn check_lane(d: &mut Daw, t: f64, li: usize) {
    let pts = nums(&call(d, "daw_auto_points", &[n(t), n(li as f64)]));
    assert!(pts.len() >= 4 && pts.len() % 2 == 0, "lane {li} has {} numbers", pts.len());
    assert!(pts.iter().all(|x| x.is_finite()));
    let beats: Vec<f64> = pts.chunks(2).map(|c| c[0]).collect();
    assert!(beats.windows(2).all(|w| w[1] > w[0]), "lane {li}: beats not increasing");
    assert!(pts.chunks(2).all(|c| c[1] >= -1.0001 && c[1] <= 1.0001), "lane {li}: value out of range");
}

#[test]
fn articulate_one_track_writes_lanes_without_a_model() {
    let mut d = articulate_song();
    let chunks = num(call(&mut d, "daw_artic_begin", &[s("single"), n(0.0), n(0.0), n(3.0), n(0.5), s("tense build into a drop")]));
    assert!(chunks >= 2.0, "a Growl track has many parameters: {chunks} chunks");
    let mut wrote = 0.0;
    for c in 0..chunks as usize {
        let q = call(&mut d, "daw_artic_prompt", &[n(c as f64)]);
        assert!(matches!(&q, Val::Str(x) if x.contains("tense build") && x.contains("Parameters:") && x.contains("Growl")), "{q:?}");
        wrote += num(call(&mut d, "daw_artic_apply", &[n(c as f64), s("")]));
    }
    assert!(wrote > 10.0, "{wrote} lanes");
    let names = lanes(&mut d, 0.0);
    assert_eq!(names.len() as f64, wrote);
    // nothing on the other tracks, no switches or levels
    assert!(lanes(&mut d, 1.0).is_empty() && lanes(&mut d, 2.0).is_empty());
    for bad in ["gain", "vol", "hold", "voices", "t s3"] {
        assert!(!names.iter().any(|x| x == bad), "{bad} was automated");
    }
    for li in 0..names.len() {
        check_lane(&mut d, 0.0, li);
    }
    let info = call(&mut d, "daw_artic_info", &[]);
    assert!(matches!(&info, Val::Str(x) if x.contains("lanes")), "{info:?}");
    call(&mut d, "daw_artic_end", &[]);
}

#[test]
fn articulate_all_covers_every_melodic_track_and_effects_but_not_drums() {
    let mut d = articulate_song();
    let chunks = num(call(&mut d, "daw_artic_begin", &[s("all"), n(0.0), n(0.0), n(1.0), n(0.6), s("")])) as usize;
    for c in 0..chunks {
        call(&mut d, "daw_artic_apply", &[n(c as f64), s("")]);
    }
    assert!(lanes(&mut d, 0.0).iter().any(|x| x.starts_with("fx0.")), "the Delay's parameters are automated too");
    assert!(lanes(&mut d, 0.0).len() > 10 && lanes(&mut d, 1.0).len() > 4);
    assert!(lanes(&mut d, 2.0).is_empty(), "drum kits are left out of the all/random modes");
    // pan lanes hold -1..1
    for t in [0.0, 1.0] {
        let names = lanes(&mut d, t);
        for (li, nm) in names.iter().enumerate() {
            check_lane(&mut d, t, li);
            let _ = nm;
        }
    }
}

#[test]
fn articulate_random_picks_the_asked_number_and_is_repeatable() {
    let run = |seed: f64| -> Vec<(f64, Vec<String>)> {
        let mut d = articulate_song();
        let chunks = num(call(&mut d, "daw_artic_begin", &[s("random"), n(0.0), n(5.0), n(seed), n(0.5), s("")])) as usize;
        for c in 0..chunks {
            call(&mut d, "daw_artic_apply", &[n(c as f64), s("")]);
        }
        (0..3).map(|t| (t as f64, lanes(&mut d, t as f64))).collect()
    };
    let a = run(7.0);
    let total: usize = a.iter().map(|x| x.1.len()).sum();
    assert_eq!(total, 5, "{a:?}");
    assert_eq!(a, run(7.0), "same seed, same lanes");
    assert_ne!(a, run(8.0), "another seed, other parameters");
}

#[test]
fn articulate_follows_the_models_picks_and_survives_garbage() {
    let mut d = articulate_song();
    call(&mut d, "daw_artic_begin", &[s("single"), n(1.0), n(0.0), n(2.0), n(0.5), s("")]);
    let reply = r#"[{"p":0,"pattern":"stutter","rate":0.25,"lo":0.1,"hi":0.9},{"p":1,"pattern":"fade","lo":0.0,"hi":1.0}]"#;
    assert!(num(call(&mut d, "daw_artic_apply", &[n(0.0), s(reply)])) >= 1.0);
    let names = lanes(&mut d, 1.0);
    assert!(!names.is_empty());
    // garbage replies, a chunk that does not exist, no job at all
    for junk in ["", "{{{{", "[]", "[{\"p\":99}]", "[{\"p\":0,\"pattern\":[1,2]}]", "\u{0}\u{1}"] {
        let mut d2 = articulate_song();
        call(&mut d2, "daw_artic_begin", &[s("single"), n(1.0), n(0.0), n(2.0), n(0.5), s("")]);
        assert!(num(call(&mut d2, "daw_artic_apply", &[n(0.0), s(junk)])) >= 1.0, "{junk:?}: the local chooser fills in");
    }
    assert_eq!(num(call(&mut d, "daw_artic_apply", &[n(99.0), s("")])), 0.0);
    call(&mut d, "daw_artic_end", &[]);
    assert_eq!(num(call(&mut d, "daw_artic_apply", &[n(0.0), s("")])), 0.0);
}

#[test]
fn articulate_leaves_lanes_the_user_drew_alone() {
    let mut d = articulate_song();
    let li = num(call(&mut d, "daw_auto_lane_add", &[n(1.0), s("cutoff")]));
    assert!(li >= 0.0, "the synth has a cutoff parameter");
    call(&mut d, "daw_auto_set_points", &[n(1.0), n(li), l(&[0.0, 0.2, 4.0, 0.9, 8.0, 0.3])]);
    let before = nums(&call(&mut d, "daw_auto_points", &[n(1.0), n(0.0)]));
    let chunks = num(call(&mut d, "daw_artic_begin", &[s("single"), n(1.0), n(0.0), n(1.0), n(0.5), s("")])) as usize;
    for c in 0..chunks {
        call(&mut d, "daw_artic_apply", &[n(c as f64), s("")]);
    }
    {
        assert_eq!(nums(&call(&mut d, "daw_auto_points", &[n(1.0), n(0.0)])), before, "the hand-drawn lane is untouched");
        let info = call(&mut d, "daw_artic_info", &[]);
        assert!(matches!(&info, Val::Str(x) if x.contains("left alone")), "{info:?}");
    }
}

#[test]
fn articulate_does_not_let_a_model_paste_one_answer_everywhere() {
    let mut d = articulate_song();
    call(&mut d, "daw_artic_begin", &[s("single"), n(0.0), n(0.0), n(4.0), n(0.5), s("")]);
    let one = r#"{"p":K,"pattern":"wobble","rate":16,"lo":0.2,"hi":0.8}"#;
    let reply = format!("[{}]", (0..10).map(|k| one.replace('K', &k.to_string())).collect::<Vec<_>>().join(","));
    call(&mut d, "daw_artic_apply", &[n(0.0), s(&reply)]);
    let names = lanes(&mut d, 0.0);
    let mut shapes = std::collections::HashSet::new();
    for li in 0..names.len() {
        let pts = nums(&call(&mut d, "daw_auto_points", &[n(0.0), n(li as f64)]));
        shapes.insert(pts.len());
    }
    assert!(names.len() >= 8 && shapes.len() >= 3, "{} lanes with {} different shapes", names.len(), shapes.len());
}

#[test]
fn derive_prompts_name_the_drum_piece_and_the_example_uses_its_own_parameters() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("drums"), s("")]);
    call(&mut d, "daw_inst_select_part", &[n(0.0), n(1.0)]);
    let q = match call(&mut d, "daw_derive_prompt", &[n(0.0), n(3.0)]) {
        Val::Str(x) => x,
        o => panic!("{o:?}"),
    };
    let ex = match call(&mut d, "daw_derive_example", &[n(0.0), n(3.0)]) {
        Val::Str(x) => x,
        o => panic!("{o:?}"),
    };
    assert!(q.contains("Snare"), "the model must know which drum it is changing: {q}");
    assert!(q.contains("\"tune\"") && !q.contains("\"level\""), "drum levels are not a sound change: {q}");
    // the worked example speaks about the same instrument, with the same names, and its answer changes the values
    assert!(ex.contains("Snare") && ex.contains("<|assistant|>") && ex.contains("\"tune\""), "{ex}");
    let (ask, answer) = ex.split_once("<|assistant|>").unwrap();
    assert!(ask.contains("\"tune\": ") && answer.contains("\"tune\": "));
    assert_ne!(ask.split("Request").next().unwrap().split_once('{').unwrap().1, answer.split_once('{').unwrap().1.trim_end().trim_end_matches('}'), "the answer must differ from the question");
    // no instrument, no prompt
    assert!(matches!(call(&mut d, "daw_derive_example", &[n(9.0), n(0.0)]), Val::Str(x) if x.is_empty()));
}

#[test]
fn auto_points_range_returns_only_the_visible_slice_with_neighbours() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    let li = num(call(&mut d, "daw_auto_lane_add", &[n(0.0), s("cutoff")]));
    assert!(li >= 0.0);
    assert_eq!(num(call(&mut d, "daw_auto_lane_empty", &[n(0.0), n(li)])), 1.0);
    let pts: Vec<f64> = (0..400).flat_map(|i| [i as f64, (i % 10) as f64 / 10.0]).collect();
    call(&mut d, "daw_auto_set_points", &[n(0.0), n(li), l(&pts)]);
    assert_eq!(num(call(&mut d, "daw_auto_lane_empty", &[n(0.0), n(li)])), 0.0);
    let r = nums(&call(&mut d, "daw_auto_points_range", &[n(0.0), n(li), n(100.5), n(110.5)]));
    // first index, then pairs: beats 100 (the neighbour before) .. 111 (the neighbour after)
    assert_eq!(r[0], 100.0);
    assert_eq!(r[1], 100.0);
    assert_eq!(r[r.len() - 2], 111.0);
    assert!(r.len() < 40, "{} numbers for a 10-beat window", r.len());
    // before the first point and past the last: the edge points only
    let r = nums(&call(&mut d, "daw_auto_points_range", &[n(0.0), n(li), n(-50.0), n(-40.0)]));
    assert_eq!(r, vec![0.0, 0.0, 0.0], "{r:?}");
    let r = nums(&call(&mut d, "daw_auto_points_range", &[n(0.0), n(li), n(900.0), n(910.0)]));
    assert_eq!(r[0], 399.0);
    assert_eq!(r.len(), 3);
    // nothing for a lane that does not exist
    assert_eq!(nums(&call(&mut d, "daw_auto_points_range", &[n(0.0), n(9.0), n(0.0), n(10.0)])), vec![0.0]);
}

// ── Fit lyrics to a clip ─────────────────────────────────────────────────────────────────────────

fn strs(v: &Val) -> Vec<String> {
    match v {
        Val::List(l) => l.iter().map(|x| if let Val::Str(s) = x { s.clone() } else { String::new() }).collect(),
        _ => vec![],
    }
}

#[test]
fn fitting_lyrics_to_a_chord_clip_through_the_builtins() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("voice"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(0.0), n(0.0), n(16.0)]);
    // two sustained chords, then a little run of short notes
    for (st, len, keys) in [(0.0, 4.0, [60.0, 64.0, 67.0]), (4.0, 4.0, [62.0, 65.0, 69.0])] {
        for k in keys {
            call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(st), n(len), n(k), n(0.8)]);
        }
    }
    for (i, k) in [72.0, 71.0, 69.0, 67.0].iter().enumerate() {
        call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(8.0 + i as f64), n(1.0), n(*k), n(0.8)]);
    }
    let raw = "[Verse]\nAm F\nWonderful tonight, my darling";
    let opts = "fit:fit:0:0.3:all:1:auto";
    let info = nums(&call(&mut d, "daw_lyrf_info", &[n(0.0), n(0.0), s(raw), s(opts)]));
    // [sung notes, syllables, lines, groups, phrases, chords, bearing]
    assert_eq!(info.len(), 7, "{info:?}");
    assert_eq!(info[3], 6.0, "two chords + four notes = six groups");
    assert_eq!(info[5], 2.0, "two chords");
    assert_eq!(info[2], 1.0, "the chord line and the label are gone");
    // info alone changes nothing
    assert!(strs(&call(&mut d, "daw_note_lyrics", &[n(0.0), n(0.0)])).iter().all(|x| x.is_empty()));
    let r = nums(&call(&mut d, "daw_lyrf_apply", &[n(0.0), n(0.0), s(raw), s(opts)]));
    assert_eq!(r[0], 10.0, "every one of the ten notes got a lyric");
    let ly = strs(&call(&mut d, "daw_note_lyrics", &[n(0.0), n(0.0)]));
    assert_eq!(ly[0], ly[1]);
    assert_eq!(ly[1], ly[2], "the chord sings one syllable together");
    assert_ne!(ly[0], ly[3]);
    assert!(ly.iter().all(|x| !x.is_empty()));
    // the text and the options stay on the clip so it can be fitted again
    let t = strs(&call(&mut d, "daw_lyric_text", &[n(0.0), n(0.0)]));
    assert_eq!(t[0], raw);
    assert_eq!(t[1], opts);
    // looped over a longer run of notes: the lyrics come round again
    let r2 = nums(&call(&mut d, "daw_lyrf_apply", &[n(0.0), n(0.0), s("la li"), s("fit:fit:1:0:all:1:auto")]));
    assert_eq!(r2[0], 10.0);
    let ly = strs(&call(&mut d, "daw_note_lyrics", &[n(0.0), n(0.0)]));
    assert!(ly.iter().all(|x| !x.starts_with('~')), "{ly:?}");
    // not a MIDI clip / no such clip / empty text: harmless
    assert_eq!(nums(&call(&mut d, "daw_lyrf_apply", &[n(5.0), n(0.0), s("x"), s("fit")])), Vec::<f64>::new());
    call(&mut d, "daw_lyrf_apply", &[n(0.0), n(0.0), s(""), s("fit")]);
    assert!(strs(&call(&mut d, "daw_note_lyrics", &[n(0.0), n(0.0)])).iter().all(|x| x.is_empty()));
    // the helpers for the model
    assert!(matches!(call(&mut d, "daw_lyrf_prompt", &[s("hello world")]), Val::Str(x) if x.contains("hello world") && x.contains("<|assistant|>")));
    assert!(matches!(call(&mut d, "daw_lyrf_accept", &[s("hello world"), s("hel-lo world")]), Val::Str(x) if x == "hel-lo world"));
    assert!(matches!(call(&mut d, "daw_lyrf_prepare", &[s("[x]\nwonderful")]), Val::Str(x) if x == "won-der-ful"));
}

// ── Match tracks to the song's key ──────────────────────────────────────────────────────────

fn write_wav(path: &std::path::Path, f: impl Fn(f32) -> f32, secs: f32) {
    let spec = hound::WavSpec { channels: 2, sample_rate: 44100, bits_per_sample: 16, sample_format: hound::SampleFormat::Int };
    let mut w = hound::WavWriter::create(path, spec).unwrap();
    for i in 0..(44100.0 * secs) as usize {
        let v = (f(i as f32 / 44100.0).clamp(-1.0, 1.0) * 20000.0) as i16;
        w.write_sample(v).unwrap();
        w.write_sample(v).unwrap();
    }
    w.finalize().unwrap();
}

fn tone(freq: f32, t: f32) -> f32 {
    (1..=4).map(|h| (std::f32::consts::TAU * freq * h as f32 * t).sin() / (h * h) as f32).sum::<f32>()
}

#[test]
fn matching_tracks_to_the_songs_key() {
    let dir = std::env::temp_dir().join(format!("keytune_test_{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let hz = |m: f32| 440.0 * 2f32.powf((m - 69.0) / 12.0);
    // a mix: Dm Gm A Dm, 0.9 s each (D minor)
    let chords: [&[f32]; 4] = [&[38.0, 57.0, 62.0, 65.0], &[43.0, 58.0, 62.0, 67.0], &[45.0, 57.0, 61.0, 64.0], &[38.0, 57.0, 62.0, 65.0]];
    write_wav(&dir.join("mix.wav"), |t| {
        let c = chords[((t / 0.9) as usize).min(3)];
        c.iter().map(|m| tone(hz(*m), t)).sum::<f32>() * 0.06
    }, 3.6);
    // a solo line in C major, every note 35 cents sharp, one chromatic C#
    let line = [60.0f32, 62.0, 64.0, 61.0, 67.0, 65.0, 64.0, 62.0];
    write_wav(&dir.join("solo.wav"), |t| {
        let i = ((t / 0.5) as usize).min(line.len() - 1);
        let env = ((t % 0.5) / 0.02).min(1.0).min((0.5 - t % 0.5) / 0.03).max(0.0);
        tone(hz(line[i] + 0.35), t) * 0.2 * env
    }, 4.0);

    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("audio"), s("")]);
    call(&mut d, "daw_track_add", &[s("audio"), s("")]);
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    call(&mut d, "daw_track_add", &[s("drums"), s("")]);
    assert!(num(call(&mut d, "daw_clip_add_audio", &[n(0.0), s(dir.join("mix.wav").to_str().unwrap()), n(0.0)])) >= 0.0);
    assert!(num(call(&mut d, "daw_clip_add_audio", &[n(1.0), s(dir.join("solo.wav").to_str().unwrap()), n(0.0)])) >= 0.0);
    // a melody in D major, one note a semitone off the scale
    call(&mut d, "daw_clip_add_midi", &[n(2.0), n(0.0), n(16.0)]);
    for (i, k) in [62.0, 64.0, 66.0, 67.0, 69.0, 71.0, 73.0, 74.0, 73.0, 71.0, 69.0, 66.0].iter().enumerate() {
        call(&mut d, "daw_note_add", &[n(2.0), n(0.0), n(i as f64), n(1.0), n(*k), n(0.8)]);
    }
    call(&mut d, "daw_clip_add_midi", &[n(3.0), n(0.0), n(4.0)]);
    call(&mut d, "daw_note_add", &[n(3.0), n(0.0), n(0.0), n(0.5), n(36.0), n(0.9)]);

    // the song's key from everything that plays
    let k = call(&mut d, "daw_key_detect", &[]);
    let Val::List(kv) = &k else { panic!("{k:?}") };
    assert_eq!(kv.len(), 6, "{kv:?}");
    // it can be remembered in the project
    call(&mut d, "daw_key_set", &[n(0.0), n(0.0)]);
    assert!(matches!(call(&mut d, "daw_key_get", &[]), Val::List(l) if l.len() == 3 && l[2] == Val::Str("C major".into())));
    call(&mut d, "daw_key_set", &[n(0.0), n(-1.0)]);
    assert!(matches!(call(&mut d, "daw_key_get", &[]), Val::List(l) if l.is_empty()));

    // the track list: C major as the target
    let tr = call(&mut d, "daw_key_tracks", &[n(0.0), n(0.0)]);
    let Val::List(items) = &tr else { panic!() };
    assert_eq!(items.len() % 9, 0);
    let rows: Vec<&[Val]> = items.chunks(9).collect();
    assert_eq!(rows.len(), 4, "two audio tracks, a synth and a drum track");
    assert_eq!(rows[0][1], Val::Num(1.0), "the mix is audio");
    assert_eq!(rows[3][1], Val::Num(2.0), "the drum track is marked");
    assert_eq!(rows[1][7], Val::Num(1.0), "the solo line is melodic audio");
    assert_eq!(rows[0][7], Val::Num(0.0), "the chord mix is not");
    // the synth melody is D major: C major differs on F# and C#
    assert!(matches!(&rows[2][6], Val::Num(p) if *p < 80.0), "{:?}", rows[2][6]);

    let r = call(&mut d, "daw_key_apply", &[n(0.0), n(0.0), l(&[0.0, 1.0, 2.0, 3.0]), n(1.0), n(80.0), n(0.0), n(1.0), n(1.0), n(1.0), n(0.0)]);
    let Val::List(lines) = &r else { panic!() };
    let text: Vec<String> = lines.iter().map(|x| if let Val::Str(s) = x { s.clone() } else { String::new() }).collect();
    assert!(text.iter().any(|x| x.contains("D minor") && x.contains("C major") && x.contains("5 semitones down")), "the chord mix moves from D minor: {text:?}");
    assert!(text.iter().any(|x| x.contains("notes tuned")), "the solo is tuned: {text:?}");
    assert!(!text.iter().any(|x| x.starts_with("Drums")), "{text:?}");
    // the mix now plays 5 semitones lower
    assert!((num(call(&mut d, "daw_clip_get", &[n(0.0), n(0.0), s("pitch")])) + 5.0).abs() < 0.6);
    // every melody note is now in C major
    let notes = nums(&call(&mut d, "daw_clip_notes", &[n(2.0), n(0.0)]));
    let major = [0, 2, 4, 5, 7, 9, 11];
    for c in notes.chunks(7) {
        assert!(major.contains(&((c[2] as i32) % 12)), "note {} is not in C major", c[2]);
    }
    // running it again changes nothing
    let r2 = call(&mut d, "daw_key_apply", &[n(0.0), n(0.0), l(&[2.0]), n(1.0), n(80.0), n(0.0), n(1.0), n(1.0), n(1.0), n(0.0)]);
    assert!(matches!(&r2, Val::List(l) if matches!(&l[0], Val::Str(x) if x.contains("already in the key"))), "{r2:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

// ── note-triggered sidechain ducking ───────────────────────────────────────────────────────────

fn rms_at(buf: &[f32], t: f64, len: f64) -> f64 {
    let a = (t * 48000.0) as usize * 2;
    let b = ((t + len) * 48000.0) as usize * 2;
    let w = &buf[a.min(buf.len())..b.min(buf.len())];
    (w.iter().map(|x| (*x as f64).powi(2)).sum::<f64>() / w.len().max(1) as f64).sqrt()
}

fn duck_song() -> Daw {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_set_bpm", &[n(120.0)]);
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    call(&mut d, "daw_track_add", &[s("drums"), s("")]);
    // a held bass note, and a kit: kicks on beats 0 2 4 6, snares on 1 3 5 7
    call(&mut d, "daw_clip_add_midi", &[n(0.0), n(0.0), n(8.0)]);
    call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(0.0), n(8.0), n(48.0), n(0.8)]);
    call(&mut d, "daw_clip_add_midi", &[n(1.0), n(0.0), n(8.0)]);
    for b in 0..8 {
        let key = if b % 2 == 0 { 36.0 } else { 38.0 };
        call(&mut d, "daw_note_add", &[n(1.0), n(0.0), n(b as f64), n(0.25), n(key), n(1.0)]);
    }
    // the kit is only the trigger: silence it, ducking must still work (a muted trigger track keys)
    call(&mut d, "daw_track_set", &[n(1.0), s("mute"), n(1.0)]);
    d
}

fn bounce(d: &mut Daw) -> Vec<f32> {
    let eng = d.engine();
    let mut en = eng.lock().unwrap();
    en.bounce_range(0.0, 8.0, 0.0)
}

#[test]
fn the_kick_of_a_kit_ducks_the_bass_and_the_snare_does_not() {
    let mut d = duck_song();
    let plain = bounce(&mut d);
    assert!(rms_at(&plain, 0.5, 0.02) > 0.01, "the bass sounds");
    // keyed from the kick notes only
    let r = nums(&call(&mut d, "daw_duck_add", &[n(1.0), n(0.0), s("36,35")]));
    assert_eq!(r.len(), 2, "route and effect index: {r:?}");
    let ducked = bounce(&mut d);
    // beats are 0.5 s apart: kicks at 0, 1, 2, 3 s, snares at 0.5, 1.5, 2.5 s
    let ratio = |t: f64| rms_at(&ducked, t, 0.02) / rms_at(&plain, t, 0.02).max(1e-9);
    for kick in [1.0, 2.0, 3.0] {
        assert!(ratio(kick + 0.012) < 0.45, "the bass dips right after the kick at {kick}: {}", ratio(kick + 0.012));
        assert!(ratio(kick + 0.45) > 0.9, "and has come back before the next beat: {}", ratio(kick + 0.45));
    }
    for snare in [0.5, 1.5, 2.5] {
        assert!(ratio(snare + 0.02) > 0.85, "a snare does not duck: {}", ratio(snare + 0.02));
    }
    // every hit ducks when no notes are named
    call(&mut d, "daw_duck_add", &[n(1.0), n(0.0), s("")]);
    let all = bounce(&mut d);
    assert!(rms_at(&all, 1.5 + 0.012, 0.02) / rms_at(&plain, 1.5 + 0.012, 0.02) < 0.45, "now the snare ducks too");
}

#[test]
fn a_ducks_depth_and_release_are_settable_and_remembered() {
    let mut d = duck_song();
    call(&mut d, "daw_duck_add", &[n(1.0), n(0.0), s("36")]);
    let plain_depth = nums(&call(&mut d, "daw_duck_get", &[n(0.0)]));
    assert_eq!(plain_depth.len(), 5);
    assert!((plain_depth[0] - 13.5).abs() < 0.2, "{plain_depth:?}");
    assert!(num(call(&mut d, "daw_duck_set", &[n(0.0), n(24.0), n(2.0), n(20.0), n(180.0), n(0.3)])) > 0.5);
    let got = nums(&call(&mut d, "daw_duck_get", &[n(0.0)]));
    assert!((got[0] - 24.0).abs() < 0.2 && (got[1] - 2.0).abs() < 0.3 && (got[2] - 20.0).abs() < 0.5 && (got[3] - 180.0).abs() < 2.0 && (got[4] - 0.3).abs() < 0.01, "{got:?}");
    // deeper and shorter: the dip is deeper and gone sooner
    let ducked = bounce(&mut d);
    let base = {
        let mut p = duck_song();
        bounce(&mut p)
    };
    let r = |t: f64| rms_at(&ducked, t, 0.02) / rms_at(&base, t, 0.02).max(1e-9);
    assert!(r(1.0 + 0.012) < 0.15, "24 dB is deep: {}", r(1.0 + 0.012));
    assert!(r(1.0 + 0.3) > 0.97, "180 ms is short: {}", r(1.0 + 0.3));
    // the preview curve
    let shape = nums(&call(&mut d, "daw_duck_shape", &[n(24.0), n(2.0), n(20.0), n(180.0), n(0.3), n(32.0)]));
    assert_eq!(shape.len(), 32);
    assert!(shape[0] > 0.99 && shape.iter().cloned().fold(1.0, f64::min) < 0.1 && shape[31] > 0.99);
    // the list, then a save and a reload keep it
    let list = call(&mut d, "daw_duck_list", &[]);
    let Val::List(l) = &list else { panic!() };
    assert_eq!(l.len(), 5);
    assert_eq!(l[3], Val::Str("36".into()));
    let path = std::env::temp_dir().join(format!("duck_test_{}.json", std::process::id()));
    call(&mut d, "daw_project_save_copy", &[s(path.to_str().unwrap())]);
    let mut d2 = Daw::new();
    assert!(num(call(&mut d2, "daw_project_load", &[s(path.to_str().unwrap())])) > 0.5);
    assert_eq!(call(&mut d2, "daw_duck_list", &[]), list);
    let again = nums(&call(&mut d2, "daw_duck_get", &[n(0.0)]));
    assert!((again[0] - 24.0).abs() < 0.2);
    let _ = std::fs::remove_file(&path);
    // removing it takes the Ducker with it
    assert!(num(call(&mut d2, "daw_duck_remove", &[n(0.0), n(1.0)])) > 0.5);
    assert!(matches!(call(&mut d2, "daw_duck_list", &[]), Val::List(l) if l.is_empty()));
    assert!(nums(&call(&mut d2, "daw_duck_get", &[n(0.0)])).is_empty());
}

#[test]
fn the_duck_suggestions_know_a_kit_from_a_synth_and_refuse_loops() {
    let mut d = duck_song();
    let Val::List(k) = call(&mut d, "daw_duck_keys", &[n(1.0)]) else { panic!() };
    assert_eq!(k[0], Val::Str("Kick".into()));
    assert_eq!(k[1], Val::Str("35,36".into()));
    assert_eq!(k[2], Val::Num(4.0), "four kicks play");
    assert_eq!(k[5], Val::Num(4.0), "four snares play");
    let Val::List(k2) = call(&mut d, "daw_duck_keys", &[n(0.0)]) else { panic!() };
    assert_eq!(k2[0], Val::Str("Every note".into()));
    // a track cannot duck itself, and a feedback loop is refused
    assert!(matches!(call(&mut d, "daw_duck_add", &[n(0.0), n(0.0), s("")]), Val::List(l) if l.is_empty()));
    call(&mut d, "daw_route_add", &[n(0.0), n(0.0), n(1.0)]);
    assert!(matches!(call(&mut d, "daw_duck_add", &[n(1.0), n(0.0), s("36")]), Val::List(l) if l.is_empty()), "the send 0 -> 1 makes 1 -> 0 a loop");
}

#[test]
fn tracker_view_rows_channels_and_cell_edits() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    let t = num(call(&mut d, "daw_track_add", &[s("synth"), s("")]));
    let c = num(call(&mut d, "daw_clip_add_midi", &[n(t), n(0.0), n(4.0)]));
    // a chord on beat 0 (3 notes -> 3 channels) and a note held across beats 1..3
    for k in [60.0, 64.0, 67.0] {
        call(&mut d, "daw_note_add", &[n(t), n(c), n(0.0), n(0.5), n(k), n(0.9)]);
    }
    call(&mut d, "daw_note_add", &[n(t), n(c), n(1.0), n(2.0), n(72.0), n(0.5)]);
    assert_eq!(nums(&call(&mut d, "daw_trk_channels", &[])).len() / 2, 3);
    // page: 8 rows at 2 rows per beat
    let page = nums(&call(&mut d, "daw_trk_page", &[n(0.0), n(8.0), n(2.0), n(0.0), n(3.0)]));
    let cell = |row: usize, ch: usize| -> (f64, f64, f64) {
        let i = (row * 3 + ch) * 3;
        (page[i], page[i + 1], page[i + 2])
    };
    let mut keys: Vec<f64> = (0..3).map(|ch| cell(0, ch).0).collect();
    keys.sort_by(|a, b| a.total_cmp(b));
    assert_eq!(keys, vec![60.0, 64.0, 67.0]);
    assert_eq!(cell(1, 0).2 + cell(1, 1).2 + cell(1, 2).2, 6.0, "the chord ended: an off mark on each of its three channels");
    let held = (0..3).map(|ch| cell(2, ch)).find(|c| c.0 == 72.0).expect("note on row 2");
    assert_eq!(held.1, 64.0);
    assert!((0..3).any(|ch| cell(3, ch).2 == 1.0), "sustain mark while the long note rings");
    // edit: clear the chord's first channel cell, then put a new note on beat 3
    assert_eq!(num(call(&mut d, "daw_trk_set", &[n(0.0), n(0.0), n(0.5), n(-1.0), n(0.0), n(0.0)])), 1.0);
    assert_eq!(num(call(&mut d, "daw_clip_get", &[n(t), n(c), s("notes")])), 3.0);
    assert_eq!(num(call(&mut d, "daw_trk_set", &[n(0.0), n(3.0), n(0.5), n(65.0), n(100.0), n(0.0)])), 1.0);
    assert_eq!(num(call(&mut d, "daw_clip_get", &[n(t), n(c), s("notes")])), 4.0);
    assert_eq!(nums(&call(&mut d, "daw_trk_ring", &[n(t), n(0.0), n(4.0), n(4.0)])).len(), 4);
}
