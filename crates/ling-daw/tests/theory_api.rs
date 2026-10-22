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
fn nums(v: &Val) -> Vec<f64> {
    match v {
        Val::List(l) => l.iter().map(|x| if let Val::Num(x) = x { *x } else { f64::NAN }).collect(),
        _ => vec![],
    }
}
fn list(v: &Val) -> Vec<Val> {
    match v {
        Val::List(l) => l.clone(),
        _ => vec![],
    }
}

/// track 0: C / G / Am / F pad, track 1: a melody with one wrong note (C#), track 2: an empty clip to compose into
fn song() -> Daw {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    for _ in 0..3 {
        call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    }
    for t in 0..3 {
        call(&mut d, "daw_clip_add_midi", &[n(t as f64), n(0.0), n(16.0)]);
    }
    for (bar, chord) in [[48.0, 52.0, 55.0], [43.0, 47.0, 50.0], [45.0, 48.0, 52.0], [41.0, 45.0, 48.0]].iter().enumerate() {
        for k in chord {
            call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(bar as f64 * 4.0), n(4.0), n(*k), n(0.8)]);
        }
    }
    for (i, k) in [72.0, 74.0, 76.0, 77.0, 79.0, 77.0, 76.0, 74.0, 72.0, 71.0, 72.0, 74.0, 76.0, 72.0, 69.0, 72.0].iter().enumerate() {
        call(&mut d, "daw_note_add", &[n(1.0), n(0.0), n(i as f64), n(0.9), n(*k), n(0.8)]);
    }
    call(&mut d, "daw_note_add", &[n(1.0), n(0.0), n(2.0), n(1.0), n(73.0), n(0.8)]); // C# over C
    d
}

#[test]
fn analysis_flags_the_wrong_note_and_autocorrect_fixes_it_with_one_undo() {
    let mut d = song();
    let sum = call(&mut d, "daw_th_summary", &[n(1.0), n(0.0)]);
    let sum = list(&sum);
    assert_eq!(sum[0], s("C major"), "{sum:?}");
    assert_eq!(nums(&Val::List(sum[5..6].to_vec()))[0], 1.0, "one out-of-key note: {sum:?}");
    // the chords come from the pad track, in Roman numerals
    let chords = list(&call(&mut d, "daw_th_chords", &[n(1.0), n(0.0)]));
    let romans: Vec<String> = chords.chunks(8).map(|c| if let Val::Str(x) = &c[5] { x.clone() } else { String::new() }).collect();
    assert_eq!(romans, ["I", "V", "vi", "IV"]);
    // preview, then apply
    let prev = nums(&call(&mut d, "daw_th_autocorrect", &[n(1.0), n(0.0), n(3.0), n(0.0)]));
    assert_eq!(prev[0], 1.0, "{prev:?}");
    call(&mut d, "daw_undo_push", &[]);
    call(&mut d, "daw_th_autocorrect", &[n(1.0), n(0.0), n(3.0), n(1.0)]);
    let sum2 = list(&call(&mut d, "daw_th_summary", &[n(1.0), n(0.0)]));
    assert_eq!(nums(&Val::List(sum2[5..6].to_vec()))[0], 0.0, "{sum2:?}");
    call(&mut d, "daw_undo", &[]);
    let sum3 = list(&call(&mut d, "daw_th_summary", &[n(1.0), n(0.0)]));
    assert_eq!(nums(&Val::List(sum3[5..6].to_vec()))[0], 1.0, "undo brings the wrong note back");
}

#[test]
fn next_chord_suggestions_and_inserting_a_chord() {
    let mut d = song();
    let next = list(&call(&mut d, "daw_th_next", &[n(0.0), n(0.0), n(16.0)]));
    // after F (IV) the dominant is the top suggestion
    assert_eq!(next[0], s("V"), "{next:?}");
    let before = nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)])).len();
    let added = nums(&Val::List(vec![call(&mut d, "daw_th_insert_chord", &[n(0.0), n(0.0), n(16.0), n(4.0), n(7.0), n(0.0)])]))[0];
    assert_eq!(added, 3.0);
    assert_eq!(nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)])).len(), before + 3 * 7);
}

#[test]
fn a_prompt_composes_over_the_projects_own_chords() {
    let mut d = song();
    let spec = match call(&mut d, "daw_th_spec", &[s("4 bars of a sparse arch melody"), s(""), s(""), n(2.0), n(0.0)]) {
        Val::Str(x) => x,
        v => panic!("{v:?}"),
    };
    assert!(spec.contains("harmony=project") && spec.contains("bars=4") && spec.contains("kind=melody") && spec.contains("C major"), "{spec}");
    let added = nums(&Val::List(vec![call(&mut d, "daw_th_generate", &[n(2.0), n(0.0), s(&spec), n(0.0), n(0.0)])]))[0];
    assert!(added >= 4.0, "{added}");
    // what it wrote is judged clean against those chords
    let sum = list(&call(&mut d, "daw_th_summary", &[n(2.0), n(0.0)]));
    assert_eq!(nums(&Val::List(sum[5..6].to_vec()))[0], 0.0, "{sum:?}");
}

#[test]
fn the_project_becomes_a_story_stored_in_its_metadata() {
    let mut d = song();
    let story = match call(&mut d, "daw_style_thumbprint", &[]) {
        Val::Str(x) => x,
        v => panic!("{v:?}"),
    };
    assert!(story.contains("C major") && story.contains("I V vi IV"), "{story}");
    assert_eq!(call(&mut d, "daw_meta_get", &[s("style_story")]), Val::Str(story.clone()));
    // a style made from it applied to a bass: same feel, this project's key
    let fields = match call(&mut d, "daw_style_fields", &[]) {
        Val::Str(x) => x,
        v => panic!("{v:?}"),
    };
    let spec = match call(&mut d, "daw_th_spec", &[s("8 bars bass"), s(""), s(&fields), n(2.0), n(0.0)]) {
        Val::Str(x) => x,
        v => panic!("{v:?}"),
    };
    assert!(spec.contains("kind=bass") && spec.contains("bars=8"), "{spec}");
}

#[test]
fn retuning_moves_the_melody_to_a_new_key() {
    let mut d = song();
    let moved = nums(&Val::List(vec![call(&mut d, "daw_th_retune", &[n(1.0), n(0.0), n(2.0), n(1.0)])]))[0];
    assert!(moved > 10.0, "{moved}");
}

#[test]
fn drum_tracks_have_no_theory_and_library_scan_survives_a_missing_folder() {
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_track_add", &[s("drums"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(0.0), n(0.0), n(4.0)]);
    assert!(list(&call(&mut d, "daw_th_summary", &[n(0.0), n(0.0)])).is_empty());
    assert!(list(&call(&mut d, "daw_lib_scan", &[s("no/such/folder")])).is_empty());
}

#[test]
fn every_kind_of_request_composes_without_panicking() {
    let mut d = song();
    let styles = ["", "trance", "house", "deep house", "disco", "hyperpop", "witch house", "lofi", "ambient", "pop", "rock", "techno", "dub", "jazz", "cinematic", "synthwave", "dnb"];
    let kinds = ["melody", "chords", "bass", "arp", "pad"];
    let contours = ["arch", "rise", "fall", "wave", "flat", "zigzag"];
    let feels = ["straight", "syncopated", "offbeat", "swing", "driving", "staccato", "sustained", "weird"];
    let dens = ["sparse", "medium", "busy", "0.0", "1.0", "9", "-3"];
    let mut n_runs = 0;
    for (i, style) in styles.iter().enumerate() {
        for kind in kinds {
            for (j, c) in contours.iter().enumerate() {
                let feel = feels[(i + j) % feels.len()];
                let den = dens[(i * 3 + j) % dens.len()];
                let bars = [1, 2, 3, 4, 8, 16, 64, 200][(i + j) % 8];
                let reply = format!(r#"{{"kind": "{kind}", "style": "{style}", "contour": "{c}", "feel": "{feel}", "density": "{den}", "bars": {bars}}}"#);
                let spec = match call(&mut d, "daw_th_spec", &[s("4 bars"), s(&reply), s(""), n(2.0), n(0.0)]) {
                    Val::Str(x) => x,
                    v => panic!("{v:?}"),
                };
                for mode in [0.0, 1.0] {
                    call(&mut d, "daw_th_generate", &[n(2.0), n(0.0), s(&spec), n(2.0), n(mode)]);
                }
                // analysis of whatever was written must also work
                call(&mut d, "daw_th_summary", &[n(2.0), n(0.0)]);
                call(&mut d, "daw_th_chords", &[n(2.0), n(0.0)]);
                call(&mut d, "daw_th_phrases", &[n(2.0), n(0.0)]);
                call(&mut d, "daw_th_autocorrect", &[n(2.0), n(0.0), n(31.0), n(0.0)]);
                n_runs += 1;
            }
        }
    }
    assert!(n_runs > 400);
}

#[test]
fn drums_arpeggios_regions_and_derive() {
    let mut d = song();
    // an empty drum track and clip
    call(&mut d, "daw_track_add", &[s("drums"), s("")]);
    call(&mut d, "daw_clip_add_midi", &[n(3.0), n(0.0), n(16.0)]);
    let spec = match call(&mut d, "daw_th_spec", &[s("4 bars house drums"), s(""), s(""), n(3.0), n(0.0)]) {
        Val::Str(x) => x,
        v => panic!("{v:?}"),
    };
    assert!(spec.contains("kind=drums"), "{spec}");
    let made = nums(&Val::List(vec![call(&mut d, "daw_th_generate", &[n(3.0), n(0.0), s(&spec), n(0.0), n(0.0)])]))[0];
    assert!(made > 30.0, "{made}");
    // writing into a region only replaces that region
    let before = nums(&call(&mut d, "daw_clip_notes", &[n(3.0), n(0.0)])).len() / 7;
    call(&mut d, "daw_th_generate", &[n(3.0), n(0.0), s(&spec.replace("bars=4", "bars=1")), n(4.0), n(2.0)]);
    let after = nums(&call(&mut d, "daw_clip_notes", &[n(3.0), n(0.0)])).len() / 7;
    assert!(after > before / 2 && after <= before + 8, "{before} -> {after}");
    // chords -> arpeggio
    let pad_before = nums(&call(&mut d, "daw_clip_notes", &[n(0.0), n(0.0)])).len() / 7;
    let made = nums(&Val::List(vec![call(&mut d, "daw_roll_arp", &[n(0.0), n(0.0), Val::List(vec![]), s("1/8"), s("updown"), n(2.0), n(0.9), s(""), n(1.0)])]))[0];
    assert!(made > pad_before as f64, "{made} vs {pad_before}");
    // derive: describe, then apply a model's reply
    let prompt = match call(&mut d, "daw_derive_prompt", &[n(0.0), n(1.0)]) {
        Val::Str(x) => x,
        v => panic!("{v:?}"),
    };
    assert!(prompt.contains("parameters") && prompt.contains("{\"") && prompt.contains(": 0."), "{prompt}");
    let pname = prompt.split('"').nth(1).unwrap().to_string();
    let changed = nums(&Val::List(vec![call(&mut d, "daw_derive_apply", &[n(0.0), s(&format!("{{\"{pname}\": 0.9, \"not_a_param\": 0.5}}")), n(1.0)])]))[0];
    assert!(changed <= 1.0);
    let local = nums(&Val::List(vec![call(&mut d, "daw_derive_local", &[n(0.0), n(3.0), n(0.8)])]))[0];
    assert!(local >= 1.0, "{local}");
}

#[test]
fn fix_all_repeats_until_clean_and_the_arrangement_is_lined_up() {
    let mut d = song();
    // a second wrong note and a doubled one, and a clip that starts a little late
    call(&mut d, "daw_note_add", &[n(1.0), n(0.0), n(6.0), n(1.0), n(70.0), n(0.8)]); // A# over F
    call(&mut d, "daw_note_add", &[n(1.0), n(0.0), n(8.0), n(1.0), n(72.0), n(0.8)]);
    call(&mut d, "daw_note_add", &[n(1.0), n(0.0), n(8.0), n(1.0), n(72.0), n(0.8)]);
    let scan = list(&call(&mut d, "daw_fix_scan", &[n(31.0)]));
    assert!(!scan.is_empty());
    let total_before: f64 = scan.chunks(9).map(|c| if let Val::Num(x) = c[3] { x } else { 0.0 }).sum();
    assert!(total_before >= 3.0, "{total_before}");
    let run = nums(&call(&mut d, "daw_fix_run", &[n(31.0), n(8.0)]));
    assert_eq!(run[0], 1.0, "clean: {run:?}");
    assert!(run[1] >= 2.0 && run[2] >= 3.0, "at least one pass of work and a final check pass: {run:?}");
    assert_eq!(*run.last().unwrap(), 0.0);
    let rep = list(&call(&mut d, "daw_fix_report", &[]));
    assert!(!rep.is_empty() && rep.len() % 9 == 0);
    // a second run has nothing left
    let again = nums(&call(&mut d, "daw_fix_run", &[n(31.0), n(8.0)]));
    assert_eq!(again[2], 0.0, "{again:?}");
    // the arrangement: move a clip off the grid, then fix it
    call(&mut d, "daw_clip_set", &[n(2.0), n(0.0), s("start"), n(16.2)]);
    let plan = list(&call(&mut d, "daw_arr_scan", &[n(15.0)]));
    assert!(!plan.is_empty(), "the late clip is found");
    let fixed = nums(&Val::List(vec![call(&mut d, "daw_arr_fix", &[n(15.0)])]))[0];
    assert!(fixed >= 1.0);
    assert!(list(&call(&mut d, "daw_arr_scan", &[n(15.0)])).is_empty());
    // sections and tags
    let secs = list(&call(&mut d, "daw_arr_sections", &[]));
    assert!(secs.len() >= 4 && secs.len() % 4 == 0, "{secs:?}");
    call(&mut d, "daw_arr_tags_set", &[s("0:16:intro;16:32:chorus")]);
    let tags = list(&call(&mut d, "daw_arr_tags", &[]));
    assert_eq!(tags.len(), 6);
    assert_eq!(tags[2], s("intro"));
}

#[test]
fn a_vocal_recording_is_tuned_to_the_notes_under_it() {
    let dir = std::env::temp_dir().join(format!("ling_vox_test_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    // a voice-like tone at 214 Hz (about 45 cents flat of A3) from 0.1 s to 1.5 s
    let wav = dir.join("vox.wav");
    {
        let spec = hound::WavSpec { channels: 2, sample_rate: 44100, bits_per_sample: 32, sample_format: hound::SampleFormat::Float };
        let mut w = hound::WavWriter::create(&wav, spec).unwrap();
        let mut ph = 0.0f64;
        for i in 0..(44100.0 * 1.8) as usize {
            let t = i as f64 / 44100.0;
            let mut v = 0.0;
            if (0.1..1.5).contains(&t) {
                ph += 214.0 / 44100.0;
                let env = ((t - 0.1) / 0.02).min(1.0).min((1.5 - t) / 0.03).max(0.0);
                for h in 1..=8 {
                    v += (ph * h as f64 * std::f64::consts::TAU).sin() / h as f64;
                }
                v *= 0.25 * env;
            }
            w.write_sample(v as f32).unwrap();
            w.write_sample(v as f32).unwrap();
        }
        w.finalize().unwrap();
    }
    let mut d = Daw::new();
    call(&mut d, "daw_new", &[]);
    call(&mut d, "daw_set_project_path", &[s(dir.join("p.json").to_str().unwrap())]);
    call(&mut d, "daw_set_bpm", &[n(120.0)]);
    call(&mut d, "daw_track_add", &[s("synth"), s("")]); // 0: the score
    call(&mut d, "daw_clip_add_midi", &[n(0.0), n(0.0), n(4.0)]);
    call(&mut d, "daw_note_add", &[n(0.0), n(0.0), n(0.2), n(2.8), n(57.0), n(0.8)]); // A3 from 0.1 s to 1.5 s
    call(&mut d, "daw_track_add", &[s("audio"), s("")]); // 1: the voice
    let ci = nums(&Val::List(vec![call(&mut d, "daw_clip_add_audio", &[n(1.0), s(wav.to_str().unwrap()), n(0.0)])]))[0];
    assert!(ci >= 0.0, "the wav loads");
    let src = list(&call(&mut d, "daw_vox_sources", &[n(1.0), n(ci)]));
    assert_eq!(src[0], n(0.0), "{src:?}");
    let r = list(&call(&mut d, "daw_vox_tune", &[n(1.0), n(ci), n(0.0), n(1.0), n(25.0), n(1.0), n(120.0), n(0.0), n(25.0), n(5.5)]));
    assert_eq!(r[0], n(1.0), "{r:?}");
    let (before, after) = (nums(&Val::List(vec![r[4].clone()]))[0], nums(&Val::List(vec![r[5].clone()]))[0]);
    assert!(before > 30.0 && after < 10.0, "{before} -> {after}");
    // the tuned audio is a file in the project's audio folder
    assert!(std::fs::read_dir(dir.join("audio")).unwrap().count() >= 1);
    // and a track with no notes is refused
    call(&mut d, "daw_track_add", &[s("synth"), s("")]);
    let bad = list(&call(&mut d, "daw_vox_tune", &[n(1.0), n(ci), n(2.0), n(1.0), n(25.0), n(1.0), n(120.0), n(0.0), n(25.0), n(5.5)]));
    assert_eq!(bad[0], n(0.0));
    let _ = std::fs::remove_dir_all(&dir);
}
