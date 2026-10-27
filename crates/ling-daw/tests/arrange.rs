//! The project agent's arrangement builtins (`daw_arrx_*`): repeat / remove / swap / resize / build / breakdown / tempo / drums /
//! layers / synth words / automation words, through the Daw API.
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
fn starts(d: &mut Daw, t: usize) -> Vec<f64> {
    let c = num(&call(d, "daw_clip_count", &[n(t as f64)])) as usize;
    (0..c).map(|i| num(&call(d, "daw_clip_get", &[n(t as f64), n(i as f64), s("start")]))).collect()
}
fn tags(d: &mut Daw) -> Vec<(f64, f64, String)> {
    let v = list(&call(d, "daw_arr_tags", &[]));
    v.chunks(3).map(|c| (num(&c[0]), num(&c[1]), st(&c[2]))).collect()
}
fn notes(d: &mut Daw, t: usize, c: usize) -> Vec<(f64, f64, u8)> {
    let v = list(&call(d, "daw_clip_notes", &[n(t as f64), n(c as f64)]));
    v.chunks(7).map(|x| (num(&x[0]), num(&x[1]), num(&x[2]) as u8)).collect()
}

/// tracks: 0 Drums, 1 Bass, 2 Pad, 3 Lead; sections intro 0-16, verse 16-48, chorus 48-80
fn song() -> Daw {
    let mut d = Daw::new();
    let names = [("drums", "Drums"), ("synth", "Bass"), ("synth", "Pad"), ("synth", "Lead")];
    for (i, (k, nm)) in names.iter().enumerate() {
        let t = num(&call(&mut d, "daw_track_add", &[s(k)])) as usize;
        assert_eq!(t, i);
        call(&mut d, "daw_track_set_name", &[n(t as f64), s(nm)]);
        for (start, len) in [(0.0, 16.0), (16.0, 32.0), (48.0, 32.0)] {
            let c = num(&call(&mut d, "daw_clip_add_midi", &[n(t as f64), n(start), n(len)]));
            for bar in 0..(len / 4.0) as usize {
                let b = bar as f64 * 4.0;
                let add = |d: &mut Daw, st_: f64, l: f64, key: f64| {
                    call(d, "daw_note_add", &[n(t as f64), n(c), n(st_), n(l), n(key), n(0.8)]);
                };
                match i {
                    0 => {
                        for beat in [0.0, 2.0] {
                            add(&mut d, b + beat, 0.5, 36.0);
                        }
                        for beat in [1.0, 3.0] {
                            add(&mut d, b + beat, 0.5, 38.0);
                        }
                        for e in 0..8 {
                            add(&mut d, b + e as f64 * 0.5, 0.25, 42.0);
                        }
                    }
                    1 => add(&mut d, b, 4.0, 36.0),
                    2 => add(&mut d, b, 4.0, 60.0),
                    _ => {
                        for (k, key) in [60.0, 62.0, 64.0, 67.0].iter().enumerate() {
                            add(&mut d, b + k as f64, 1.0, *key);
                        }
                    }
                }
            }
        }
    }
    call(&mut d, "daw_arr_tags_set", &[s("0:16:intro;16:48:verse;48:80:chorus")]);
    d
}

fn song_end(d: &mut Daw) -> f64 {
    let nt = num(&call(d, "daw_track_count", &[])) as usize;
    let mut e: f64 = 0.0;
    for t in 0..nt {
        let c = num(&call(d, "daw_clip_count", &[n(t as f64)])) as usize;
        for i in 0..c {
            e = e.max(num(&call(d, "daw_clip_get", &[n(t as f64), n(i as f64), s("end")])));
        }
    }
    e
}

#[test]
fn a_section_repeats_with_everything_after_it_moved() {
    let mut d = song();
    // a lane and a tempo change inside the chorus
    call(&mut d, "daw_arrx_part", &[n(3.0), n(48.0), n(80.0), n(0.0)]);
    call(&mut d, "daw_tempo_add", &[n(60.0), n(150.0)]);
    let end0 = song_end(&mut d);
    let r = call(&mut d, "daw_arrx_repeat", &[n(48.0), n(80.0), n(1.0)]);
    assert_eq!(num(&r), 112.0);
    assert_eq!(song_end(&mut d), end0 + 32.0);
    for t in 0..4 {
        assert_eq!(starts(&mut d, t), vec![0.0, 16.0, 48.0, 80.0], "track {t}");
    }
    // the copy has the same notes
    assert_eq!(notes(&mut d, 3, 2), notes(&mut d, 3, 3));
    let tg = tags(&mut d);
    assert_eq!(tg, vec![(0.0, 16.0, "intro".to_string()), (16.0, 48.0, "verse".to_string()), (48.0, 80.0, "chorus".to_string()), (80.0, 112.0, "chorus".to_string())]);
    // the tempo change was copied into the repeat, the tempo before it carries on
    let tp = list(&call(&mut d, "daw_tempo_points", &[]));
    let pairs: Vec<(f64, f64)> = tp.chunks(2).map(|c| (num(&c[0]), num(&c[1]))).collect();
    assert!(pairs.contains(&(60.0, 150.0)) && pairs.contains(&(92.0, 150.0)) && pairs.contains(&(80.0, 120.0)), "{pairs:?}");
    // the volume lane (the lead is out in the chorus) follows: out again in the copy, back at the end
    let pts = list(&call(&mut d, "daw_auto_points", &[n(3.0), n(0.0)]));
    let pts: Vec<(f64, f64)> = pts.chunks(2).map(|c| (num(&c[0]), num(&c[1]))).collect();
    assert!(pts.iter().any(|(b, v)| (*b - 111.98).abs() < 0.1 && *v == 0.0), "{pts:?}");
    assert!(pts.last().map(|p| (p.0 - 112.01).abs() < 0.1 && p.1 > 0.1).unwrap_or(false), "{pts:?}");
}

#[test]
fn a_section_goes_and_the_gap_closes() {
    let mut d = song();
    let end0 = song_end(&mut d);
    call(&mut d, "daw_arrx_remove", &[n(16.0), n(48.0)]);
    assert_eq!(song_end(&mut d), end0 - 32.0);
    for t in 0..4 {
        assert_eq!(starts(&mut d, t), vec![0.0, 16.0], "track {t}");
    }
    assert_eq!(tags(&mut d), vec![(0.0, 16.0, "intro".to_string()), (16.0, 48.0, "chorus".to_string())]);
}

#[test]
fn two_sections_swap_and_one_moves() {
    let mut d = song();
    let verse_notes = notes(&mut d, 3, 1);
    // verse (32 beats) and chorus (32 beats) trade places
    call(&mut d, "daw_arrx_swap", &[n(16.0), n(48.0), n(48.0), n(80.0)]);
    let tg = tags(&mut d);
    assert_eq!(tg, vec![(0.0, 16.0, "intro".to_string()), (16.0, 48.0, "chorus".to_string()), (48.0, 80.0, "verse".to_string())]);
    assert_eq!(notes(&mut d, 3, 2), verse_notes, "the verse's notes now sit third");
    // unequal lengths: intro (16) and verse (32) swap: nothing is lost, tags follow
    let mut d = song();
    let end0 = song_end(&mut d);
    call(&mut d, "daw_arrx_swap", &[n(0.0), n(16.0), n(16.0), n(48.0)]);
    assert_eq!(song_end(&mut d), end0);
    assert_eq!(tags(&mut d)[0], (0.0, 32.0, "verse".to_string()));
    assert_eq!(tags(&mut d)[1], (32.0, 48.0, "intro".to_string()));
    assert_eq!(starts(&mut d, 1), vec![0.0, 32.0, 48.0]);
    // move the chorus before the verse
    let mut d = song();
    call(&mut d, "daw_arrx_move", &[n(48.0), n(80.0), n(16.0)]);
    assert_eq!(tags(&mut d), vec![(0.0, 16.0, "intro".to_string()), (16.0, 48.0, "chorus".to_string()), (48.0, 80.0, "verse".to_string())]);
}

#[test]
fn a_section_gets_longer_or_shorter() {
    let mut d = song();
    let end0 = song_end(&mut d);
    // the intro grows by 4 bars (16 beats): its 4 bars loop once more
    let r = call(&mut d, "daw_arrx_resize", &[n(0.0), n(16.0), n(32.0)]);
    assert_eq!(num(&r), 32.0);
    assert_eq!(song_end(&mut d), end0 + 16.0);
    assert_eq!(starts(&mut d, 3), vec![0.0, 16.0, 32.0, 64.0]);
    let tg = tags(&mut d);
    assert_eq!(tg[0], (0.0, 32.0, "intro".to_string()));
    assert_eq!(tg[1], (32.0, 64.0, "verse".to_string()));
    assert_eq!(notes(&mut d, 3, 0), notes(&mut d, 3, 1));
    // the verse is cut to 4 bars
    call(&mut d, "daw_arrx_resize", &[n(32.0), n(64.0), n(16.0)]);
    assert_eq!(song_end(&mut d), end0);
    let tg = tags(&mut d);
    assert_eq!(tg[1], (32.0, 48.0, "verse".to_string()));
    assert_eq!(tg[2].0, 48.0);
}

#[test]
fn a_build_is_inserted_before_the_drop() {
    let mut d = song();
    let end0 = song_end(&mut d);
    // 2 bars (8 beats) before the chorus at 48
    let r = list(&call(&mut d, "daw_arrx_build", &[n(48.0), n(2.0)]));
    assert_eq!((num(&r[0]), num(&r[1])), (48.0, 56.0));
    assert_eq!(song_end(&mut d), end0 + 8.0);
    let tg = tags(&mut d);
    assert!(tg.contains(&(48.0, 56.0, "build".to_string())), "{tg:?}");
    assert!(tg.contains(&(56.0, 88.0, "chorus".to_string())), "{tg:?}");
    // the drums got a snare roll in the second half of the build, the bass is out of the first half
    let drums = starts(&mut d, 0).iter().position(|x| *x == 48.0).expect("a drum clip at the build");
    let dn = notes(&mut d, 0, drums);
    let rolls = dn.iter().filter(|x| x.2 == 38 && x.0 >= 4.0).count();
    assert!(rolls >= 5, "snare roll: {rolls}");
    assert!(!dn.iter().any(|x| x.2 == 36 && x.0 < 4.0), "no kick in the first half");
    let bass = starts(&mut d, 1).iter().position(|x| *x == 48.0).unwrap();
    assert!(notes(&mut d, 1, bass).iter().all(|x| x.0 >= 4.0), "bass thinned");
    // velocities rise through the build on the lead
    let lead = starts(&mut d, 3).iter().position(|x| *x == 48.0).unwrap();
    let v = list(&call(&mut d, "daw_clip_notes", &[n(3.0), n(lead as f64)]));
    let vels: Vec<f64> = v.chunks(7).map(|c| num(&c[3])).collect();
    assert!(vels.first().unwrap() < vels.last().unwrap(), "{vels:?}");
}

#[test]
fn a_breakdown_takes_drums_and_bass_out() {
    let mut d = song();
    let out = list(&call(&mut d, "daw_arrx_breakdown", &[n(16.0), n(48.0)]));
    let names: Vec<String> = out.iter().map(st).collect();
    assert!(names.contains(&"Drums".to_string()) && names.contains(&"Bass".to_string()), "{names:?}");
    assert_eq!(starts(&mut d, 0), vec![0.0, 48.0]);
    assert_eq!(starts(&mut d, 1), vec![0.0, 48.0]);
    assert_eq!(starts(&mut d, 2), vec![0.0, 16.0, 48.0], "the pad stays");
    assert!(tags(&mut d).contains(&(16.0, 48.0, "breakdown".to_string())));
}

#[test]
fn a_section_has_its_own_tempo() {
    let mut d = song();
    call(&mut d, "daw_arrx_tempo", &[n(16.0), n(48.0), n(90.0)]);
    assert_eq!(num(&call(&mut d, "daw_bpm_at", &[n(10.0)])), 120.0);
    assert_eq!(num(&call(&mut d, "daw_bpm_at", &[n(20.0)])), 90.0);
    assert_eq!(num(&call(&mut d, "daw_bpm_at", &[n(50.0)])), 120.0);
    // from a beat on, to the end
    call(&mut d, "daw_arrx_tempo", &[n(48.0), n(-1.0), n(150.0)]);
    assert_eq!(num(&call(&mut d, "daw_bpm_at", &[n(70.0)])), 150.0);
    assert_eq!(num(&call(&mut d, "daw_bpm_at", &[n(30.0)])), 90.0);
    // by words
    let mut d = song();
    let r = st(&call(&mut d, "daw_arrx_run", &[s("section_tempo"), s("set the verse to 100 bpm"), n(-1.0), n(-1.0), n(-1.0), n(1.0)]));
    assert!(r.contains("100"), "{r}");
    assert_eq!(num(&call(&mut d, "daw_bpm_at", &[n(20.0)])), 100.0);
}

#[test]
fn drums_go_half_time_and_double_time() {
    let mut d = song();
    let n0 = notes(&mut d, 0, 1);
    assert!(n0.iter().any(|x| x.2 == 38 && x.0 == 1.0));
    call(&mut d, "daw_arrx_drumfeel", &[n(16.0), n(48.0), n(0.0)]);
    let h = notes(&mut d, 0, 1);
    assert!(h.iter().all(|x| !(x.2 == 38 && (x.0 % 4.0) == 3.0)), "no snare on 4");
    assert!(h.iter().any(|x| x.2 == 38 && (x.0 % 4.0) == 2.0), "snare on 3");
    assert!(h.iter().filter(|x| x.2 == 42).count() < n0.iter().filter(|x| x.2 == 42).count());
    // double time: the bar plays twice as fast, twice
    let mut d = song();
    let n0 = notes(&mut d, 0, 0);
    call(&mut d, "daw_arrx_drumfeel", &[n(0.0), n(16.0), n(1.0)]);
    let dd = notes(&mut d, 0, 0);
    assert_eq!(dd.len(), n0.len() * 2);
    assert!(dd.iter().any(|x| x.2 == 38 && x.0 == 0.5) && dd.iter().any(|x| x.2 == 38 && x.0 == 2.5));
}

#[test]
fn a_part_leaves_a_section_and_comes_back() {
    let mut d = song();
    let r = st(&call(&mut d, "daw_arrx_run", &[s("part"), s("drop the bass out of the verse"), n(-1.0), n(-1.0), n(-1.0), n(1.0)]));
    assert!(r.contains("Bass"), "{r}");
    let pts = list(&call(&mut d, "daw_auto_points", &[n(1.0), n(0.0)]));
    let pts: Vec<(f64, f64)> = pts.chunks(2).map(|c| (num(&c[0]), num(&c[1]))).collect();
    assert!(pts.iter().any(|(b, v)| (*b - 16.0).abs() < 0.1 && *v == 0.0) && pts.iter().any(|(b, v)| (*b - 47.98).abs() < 0.1 && *v == 0.0), "{pts:?}");
    assert!(pts.last().unwrap().1 > 0.1, "the bass is back after the verse");
    // solo: the others duck
    let r = st(&call(&mut d, "daw_arrx_run", &[s("part"), s("solo the lead in the chorus"), n(-1.0), n(-1.0), n(-1.0), n(1.0)]));
    assert!(r.contains("alone"), "{r}");
    let pts = list(&call(&mut d, "daw_auto_points", &[n(2.0), n(0.0)]));
    assert!(!pts.is_empty(), "the pad is ducked");
}

#[test]
fn a_layer_is_written_on_a_new_track() {
    let mut d = song();
    let nt0 = num(&call(&mut d, "daw_track_count", &[]));
    let r = list(&call(&mut d, "daw_arrx_layer", &[n(3.0), n(1.0), s("thirds")]));
    assert_eq!(num(&r[0]), 4.0);
    assert_eq!(st(&r[2]), "thirds");
    assert_eq!(num(&call(&mut d, "daw_track_count", &[])), nt0 + 1.0);
    let src = notes(&mut d, 3, 1);
    let lay = notes(&mut d, 4, 0);
    assert_eq!(src.len(), lay.len());
    // C major: C D E G -> E F G B (a diatonic third above)
    let keys: Vec<u8> = lay.iter().take(4).map(|x| x.2).collect();
    assert_eq!(keys, vec![64, 65, 67, 71]);
    // sixths below, octave, fifths, choir
    for (kind, label) in [("sixths below", "sixths"), ("octave up", "octave"), ("fifths", "fifths"), ("choir", "choir")] {
        let r = list(&call(&mut d, "daw_arrx_layer", &[n(3.0), n(-1.0), s(kind)]));
        assert_eq!(st(&r[2]), label);
        assert!(num(&r[1]) > 0.0);
    }
}

#[test]
fn synth_words_move_named_parameters_and_never_pitch() {
    let mut d = song();
    let rel = num(&call(&mut d, "daw_inst_get", &[n(3.0), s("release")]));
    let dec = num(&call(&mut d, "daw_inst_get", &[n(3.0), s("decay")]));
    let lfo_p = num(&call(&mut d, "daw_inst_get", &[n(3.0), s("lfo pitch")]));
    let det = num(&call(&mut d, "daw_inst_get", &[n(3.0), s("detune")]));
    let fmr = num(&call(&mut d, "daw_inst_get", &[n(3.0), s("fm ratio")]));
    let r = st(&call(&mut d, "daw_arrx_synth", &[n(3.0), s("longer release and shorter decay, a bit brighter")]));
    assert!(r.contains("release"), "{r}");
    assert!(num(&call(&mut d, "daw_inst_get", &[n(3.0), s("release")])) > rel);
    assert!(num(&call(&mut d, "daw_inst_get", &[n(3.0), s("decay")])) < dec);
    // the pitch guard
    let r = st(&call(&mut d, "daw_arrx_synth", &[n(3.0), s("grittier, wobblier, faster, more movement, wider, dubstep")]));
    assert!(!r.is_empty());
    assert_eq!(num(&call(&mut d, "daw_inst_get", &[n(3.0), s("lfo pitch")])), lfo_p);
    assert_eq!(num(&call(&mut d, "daw_inst_get", &[n(3.0), s("detune")])), det);
    assert_eq!(num(&call(&mut d, "daw_inst_get", &[n(3.0), s("fm ratio")])), fmr);
    assert_eq!(num(&call(&mut d, "daw_arrx_is_pitch", &[s("lfo pitch")])), 1.0);
    assert_eq!(num(&call(&mut d, "daw_arrx_is_pitch", &[s("cutoff")])), 0.0);
}

#[test]
fn automation_by_words_is_drawn_over_the_section_and_not_in_pitch() {
    let mut d = song();
    let r = list(&call(&mut d, "daw_arrx_auto", &[n(3.0), s("filter sweep up over the build"), n(16.0), n(48.0)]));
    assert!(r.iter().any(|x| st(x).contains("filter")), "{r:?}");
    let lanes = list(&call(&mut d, "daw_auto_lanes", &[n(3.0)]));
    let names: Vec<String> = lanes.iter().map(st).collect();
    assert!(names.contains(&"cutoff".to_string()), "{names:?}");
    let i = names.iter().position(|x| x == "cutoff").unwrap();
    let pts: Vec<f64> = list(&call(&mut d, "daw_auto_points", &[n(3.0), n(i as f64)])).chunks(2).map(|c| num(&c[1])).collect();
    assert!(pts.iter().cloned().fold(0.0, f64::max) > 0.9 && pts.iter().cloned().fold(1.0, f64::min) <= 0.25, "{pts:?}");
    // a fade out ends silent, a pump ducks every beat
    call(&mut d, "daw_arrx_auto", &[n(2.0), s("fade out"), n(48.0), n(80.0)]);
    let v = list(&call(&mut d, "daw_auto_points", &[n(2.0), n(0.0)]));
    assert_eq!(num(&v[v.len() - 1]), 0.0);
    call(&mut d, "daw_arrx_auto", &[n(1.0), s("sidechain pump"), n(16.0), n(24.0)]);
    let v = list(&call(&mut d, "daw_auto_points", &[n(1.0), n(0.0)]));
    assert!(v.len() >= 32, "{}", v.len());
    // no pitch lane without a pitch request: the lead has none of bend / vibrato / sweep lanes
    let all: Vec<String> = list(&call(&mut d, "daw_auto_lanes", &[n(3.0)])).iter().map(st).collect();
    assert!(all.iter().all(|x| num(&call(&mut d, "daw_arrx_is_pitch", &[s(x)])) == 0.0), "{all:?}");
}

#[test]
fn words_find_the_section_and_the_agent_runs_them() {
    let mut d = song();
    call(&mut d, "daw_arr_tags_set", &[s("0:16:intro;16:48:verse;48:80:chorus;80:112:verse;112:144:chorus")]);
    let f = list(&call(&mut d, "daw_arrx_find", &[s("the second verse"), n(0.0), n(0.0)]));
    assert_eq!((num(&f[0]), num(&f[1])), (80.0, 112.0));
    let f = list(&call(&mut d, "daw_arrx_find", &[s("the last chorus"), n(0.0), n(0.0)]));
    assert_eq!(num(&f[0]), 112.0);
    let f = list(&call(&mut d, "daw_arrx_find", &[s("bars 9 to 16"), n(0.0), n(0.0)]));
    assert_eq!((num(&f[0]), num(&f[1])), (32.0, 64.0));
    // run: repeat the first chorus (it is the one at 48)
    let r = st(&call(&mut d, "daw_arrx_run", &[s("repeat_section"), s("repeat the first chorus"), n(-1.0), n(-1.0), n(-1.0), n(1.0)]));
    assert!(r.contains("Repeated"), "{r}");
    assert_eq!(tags(&mut d).iter().filter(|t| t.2 == "chorus").count(), 3);
    // swap by words, remove by words
    let r = st(&call(&mut d, "daw_arrx_run", &[s("swap_sections"), s("swap the intro and the first verse"), n(-1.0), n(-1.0), n(-1.0), n(1.0)]));
    assert!(r.contains("Swapped"), "{r}");
    assert_eq!(tags(&mut d)[0].2, "verse");
    let r = st(&call(&mut d, "daw_arrx_run", &[s("remove_section"), s("remove the intro"), n(-1.0), n(-1.0), n(-1.0), n(1.0)]));
    assert!(r.contains("Removed"), "{r}");
    assert!(!tags(&mut d).iter().any(|t| t.2 == "intro"));
    // no section named: a sentence, not a crash
    let r = st(&call(&mut d, "daw_arrx_run", &[s("remove_section"), s("remove the coda"), n(-1.0), n(-1.0), n(-1.0), n(1.0)]));
    assert!(r.contains("Name a section"), "{r}");
}
