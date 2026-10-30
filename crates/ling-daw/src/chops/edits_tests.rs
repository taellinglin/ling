//! The production edits: the planner's placement rules (where in the song, how many, which genre, what the vocal offers) and
//! each edit's own rules on a section of chops.
use super::*;
use crate::chops::{Chop, Tune, Vocal};
use crate::model::Note;

const RATE: u32 = 48000;
const BPM: f64 = 120.0;

/// eight sung syllables (A3 .. E4 in A minor, 0.3 s each, the third one held 0.6 s) and an unpitched hit, 0.1 s apart: a vocal
/// of chops (one word each, one line)
fn vocal() -> Vocal {
    let mut ch = vec![];
    let mut at = 0u32;
    for (k, m) in [57.0f32, 60.0, 64.0, 62.0, 60.0, 57.0, 59.0, 64.0].iter().enumerate() {
        let len = if k == 2 { 0.6 } else { 0.3 };
        let e = at + (len * RATE as f32) as u32;
        ch.push(Chop { start: at, end: e, midi: Some(*m), level: 0.5 + 0.05 * k as f32 });
        at = e + RATE / 10;
    }
    ch.push(Chop { start: at, end: at + RATE / 10, midi: None, level: 0.6 });
    Vocal::from_chops(&ch, RATE, BPM)
}

/// a section's chops: per bar the hook on the downbeat, a gated pair on the off-beats, a note on 3 and one on the "and" of 4,
/// all on the scale of A minor
fn chops_of(v: &Vocal, beats: f64) -> Vec<Note> {
    let t = Tune::default();
    let mut out = vec![];
    let bars = (beats / 4.0) as usize;
    for b in 0..bars {
        let t0 = b as f64 * 4.0;
        for (k, (off, len)) in [(0.0, 0.9), (1.5, 0.4), (2.0, 0.9), (2.5, 0.4), (3.0, 0.4)].iter().enumerate() {
            let p = (b + k) % 8;
            let c = &v.pieces[p].chop;
            out.push(crate::chops::tuned_note(c, t0 + off, *len, c.midi.unwrap().round() as i32, 0.8, &t, 0).0);
        }
    }
    out
}

fn at<'a>(v: &'a Vocal, beats: f64, genre: &'a str, nth: usize) -> At<'a> {
    At { beats, key: (9, true), energy: 0.8, drums: &[], harmony: &[], vocal: v, tune: Tune::default(), genre, bpm: BPM, seed: 3, silence: 0.0, slicer: true, next_hot: false, nth }
}

fn spot(tag: &str, beats: f64, energy: f64) -> Spot {
    Spot { tag: tag.into(), beats, energy, chopped: true, ..Spot::default() }
}

fn content() -> Content {
    Content { sustain: true, pitched: true, words: 6, hits: true, voices: 1, slicer: true, reverse: true }
}

/// an EDM song: intro, verse, build, drop, break, build, drop, outro
fn edm_song() -> Vec<Spot> {
    let mut v = vec![spot("intro", 16.0, 0.25), spot("verse", 32.0, 0.5), spot("build", 16.0, 0.7), spot("drop", 32.0, 0.95), spot("break", 16.0, 0.35), spot("build", 16.0, 0.7), spot("drop", 32.0, 0.95), spot("outro", 16.0, 0.3)];
    v[2].build = true;
    v[5].build = true;
    v
}

fn bend_end(n: &Note) -> f32 {
    n.bend.last().map(|p| p.1).unwrap_or(0.0)
}

// ── the option ──

#[test]
fn the_option_reads_auto_off_light_more_and_lists() {
    assert_eq!(Which::parse(""), Which::default());
    assert_eq!(Which::parse("auto"), Which::default());
    assert!(!Which::parse("off").on);
    assert_eq!(Which::parse("light").scale, 0.5);
    assert_eq!(Which::parse("more").scale, 1.5);
    let w = Which::parse("swell, tape stop,echo|trance gate");
    assert_eq!(w.only, vec![Swell, TapeStop, Gate, Throw]);
    assert!(w.allows(Swell) && !w.allows(Pad));
    // nothing known: off
    assert!(!Which::parse("sparkles").on);
    for k in KINDS {
        assert_eq!(Kind::parse(k.name()), Some(k), "{k:?}");
    }
}

#[test]
fn the_report_counts_each_edit() {
    assert_eq!(tally(&[Swell, TapeStop, Swell, Throw]), "2 reverse swells, 1 tape stop, 1 echo throw");
    assert_eq!(tally(&[]), "");
}

// ── the planner ──

#[test]
fn run_ups_to_drops_get_a_riser_over_the_build_and_the_second_drop_an_air_pocket() {
    for seed in 0..12 {
        let p = plan(&edm_song(), "house", &Which::default(), &content(), seed);
        // a build into a drop: a riser (club music always), never a swell or a pocket (the build has its breath)
        assert!(p[2].contains(&Riser) && p[5].contains(&Riser), "{seed}: {p:?}");
        assert!(!p[2].contains(&Swell) && !p[2].contains(&Pocket));
        // the verse into the build is no run-up to a drop
        assert!(!p[1].contains(&Riser) && !p[1].contains(&Sweep));
    }
    // a drop reached without a build: a swell (and a sweep) every time, a pocket only on the second
    let song = vec![spot("verse", 32.0, 0.5), spot("drop", 32.0, 0.95), spot("break", 32.0, 0.35), spot("drop", 32.0, 0.95)];
    let mut pockets = 0;
    for seed in 0..12 {
        let p = plan(&song, "dubstep", &Which::default(), &content(), seed);
        assert!(p[0].contains(&Swell) && p[2].contains(&Swell), "{p:?}");
        assert!(!p[0].contains(&Pocket), "the first drop lands straight");
        pockets += p[2].contains(&Pocket) as usize;
    }
    assert!(pockets >= 8, "{pockets}");
}

#[test]
fn a_section_the_energy_falls_out_of_ends_in_a_tape_stop_or_a_throw_and_the_song_dives() {
    let mut stops = 0;
    for seed in 0..16 {
        let p = plan(&edm_song(), "trap", &Which::default(), &content(), seed);
        // drop -> break: the energy falls
        let end: Vec<Kind> = p[3].iter().copied().filter(|k| matches!(k, TapeStop | Throw)).collect();
        assert_eq!(end.len(), 1, "{seed}: {:?}", p[3]);
        stops += (end[0] == TapeStop) as usize;
        // never two tape stops in a row
        let ts: Vec<usize> = (0..p.len()).filter(|i| p[*i].contains(&TapeStop)).collect();
        assert!(ts.len() <= 2);
        // the last section ends the song
        assert!(p[7].iter().any(|k| matches!(k, Dive | Throw | TapeStop)), "{:?}", p[7]);
    }
    assert!(stops >= 4, "trap likes tape stops: {stops}");
    // a medley's change of song is a transition too
    let mut m = vec![spot("drop", 32.0, 0.9), spot("drop", 32.0, 0.9)];
    m[1].song = 1;
    let p = plan(&m, "house", &Which::only_for_test(&[TapeStop, Throw, Dive]), &content(), 1);
    assert!(p[0].iter().any(|k| matches!(k, TapeStop | Throw)), "{p:?}");
}

#[test]
fn the_first_chorus_gets_one_subtle_edit_and_the_repeats_vary() {
    let song: Vec<Spot> = (0..5).flat_map(|_| [spot("verse", 16.0, 0.5), spot("chorus", 32.0, 0.85)]).collect();
    for seed in 0..10 {
        let p = plan(&song, "future bass", &Which::default(), &content(), seed);
        let own = |i: usize| -> Vec<Kind> { p[i].iter().copied().filter(|k| !matches!(k, Swell | Sweep | Pocket | TapeStop | Throw | Dive | Riser)).collect() };
        let first = own(1);
        assert!(first.len() <= 1 && first.iter().all(|k| matches!(k, Candy | Choke | Stutter | Octave)), "{first:?}");
        // the later choruses: two or three, never the ones the chorus before had
        for c in [3usize, 5, 7] {
            let (a, b) = (own(c - 2), own(c));
            assert!(b.len() <= 3, "{b:?}");
            assert!(b.iter().all(|k| !a.contains(k)) || b.is_empty(), "{seed}: {a:?} then {b:?}");
            assert!(!(b.contains(&HalfTime) && (b.contains(&DoubleTime) || b.contains(&Flip))));
        }
        // the song-wide caps
        for k in KINDS {
            assert!(p.iter().filter(|s| s.contains(&k)).count() <= cap(k), "{k:?} {p:?}");
        }
        // verses: one at most of their own
        assert!(p.iter().step_by(2).all(|s| s.iter().filter(|k| matches!(k, Candy | Glide)).count() <= 1));
    }
}

#[test]
fn the_genre_decides_what_fits() {
    let song = edm_song();
    for seed in 0..10 {
        let all: Vec<Kind> = plan(&song, "ballad", &Which::default(), &content(), seed).concat();
        assert!(all.iter().all(|k| matches!(k, Swell | Dive | Sweep | Glide | Pad | Candy | Throw | Response)), "{all:?}");
        let trap: Vec<Kind> = plan(&song, "trap", &Which::default(), &content(), seed).concat();
        assert!(!trap.contains(&Gate), "no trance gate in trap");
        // asked for by name, it plays where it fits
        let asked = plan(&song, "trap", &Which::parse("gate,pad"), &content(), seed).concat();
        assert!(asked.iter().all(|k| matches!(k, Gate | Pad)), "{asked:?}");
    }
    let gates: usize = (0..20).map(|s| plan(&song, "trance", &Which::default(), &content(), s).concat().iter().filter(|k| **k == Gate).count()).sum();
    assert!(gates >= 5, "trance gates in trance: {gates}");
}

#[test]
fn the_vocal_and_the_option_limit_the_edits() {
    let song = edm_song();
    let thin = Content { sustain: false, words: 1, hits: false, slicer: false, reverse: false, ..content() };
    for seed in 0..10 {
        let p = plan(&song, "house", &Which::default(), &thin, seed).concat();
        assert!(!p.iter().any(|k| matches!(k, Pad | Gate | Riser | Flip | Candy | Swell)), "{p:?}");
        assert!(plan(&song, "house", &Which::off(), &content(), seed).concat().is_empty());
        let light = plan(&song, "house", &Which::parse("light"), &content(), seed).concat().len();
        let more = plan(&song, "house", &Which::parse("more"), &content(), seed).concat().len();
        let auto = plan(&song, "house", &Which::default(), &content(), seed).concat().len();
        assert!(light <= auto && auto <= more, "{light} {auto} {more}");
    }
    // call and response needs two voices; every other section sung alone
    let mut song: Vec<Spot> = (0..6).map(|_| Spot { chopped: false, sings: true, ..spot("verse", 32.0, 0.5) }).collect();
    song[5].tag = "outro".into();
    let two = Content { voices: 2, ..content() };
    let p = plan(&song, "pop", &Which::default(), &two, 1);
    assert_eq!(p.iter().filter(|s| s.contains(&Response)).count(), 3, "{p:?}");
    assert!(plan(&song, "pop", &Which::default(), &content(), 1).concat().iter().all(|k| *k != Response));
}

// ── the edits on a section ──

#[test]
fn a_reverse_swell_ends_on_the_downbeat() {
    let v = vocal();
    let mut main = chops_of(&v, 16.0);
    let mut layer = vec![];
    let o = apply(&at(&v, 16.0, "house", 0), &[Swell], &mut main, &mut layer);
    assert_eq!(o.used, vec![Swell]);
    let sw: Vec<&Note> = main.iter().filter(|n| n.lyric.ends_with(":r")).collect();
    assert_eq!(sw.len(), 1);
    assert!((sw[0].start + sw[0].len - 16.0).abs() < 1e-6, "{:?}", sw[0]);
    // nothing else starts under it
    assert!(main.iter().filter(|n| n.start >= sw[0].start - 1e-6).count() == 1);
    // on a Sampler without reversed copies: none
    let mut main = chops_of(&v, 16.0);
    let o = apply(&At { slicer: false, ..at(&v, 16.0, "house", 0) }, &[Swell], &mut main, &mut layer);
    assert!(o.used.is_empty());
}

#[test]
fn a_tape_stop_slows_the_last_beats_to_a_halt() {
    let v = vocal();
    let mut main = chops_of(&v, 16.0);
    let mut layer = main.clone();
    let o = apply(&at(&v, 16.0, "trap", 0), &[TapeStop], &mut main, &mut layer);
    assert_eq!(o.used, vec![TapeStop]);
    // 0.8 s at 120 bpm: a beat and a half
    let w0 = 16.0 - 1.5;
    let inside: Vec<&Note> = main.iter().filter(|n| n.start + n.len > w0 + 1e-6).collect();
    assert!(!inside.is_empty());
    for n in &inside {
        assert!(!n.bend.is_empty());
        // the curve only falls
        assert!(n.bend.windows(2).all(|w| w[1].1 <= w[0].1 + 1e-4), "{:?}", n.bend);
    }
    let last = inside.iter().max_by(|a, b| a.start.total_cmp(&b.start)).unwrap();
    assert!((last.start + last.len - 16.0).abs() < 1e-6 && bend_end(last) < -20.0, "{last:?}");
    // nothing starts in the last 40 % (the tape is stopping), the layer is out of the way
    assert!(!main.iter().any(|n| n.start >= w0 + 0.6 * 1.5 && n.start < 16.0));
    assert!(!layer.iter().any(|n| n.start >= w0 - 1e-6));
    assert!((tape_curve(0.0)).abs() < 1e-6 && tape_curve(0.5) < -10.0 && tape_curve(1.0) <= -36.0 + 1e-3);
}

#[test]
fn the_last_syllable_dives_an_octave_held_on_a_loop() {
    let v = vocal();
    let mut main = chops_of(&v, 16.0);
    let o = apply(&at(&v, 16.0, "dubstep", 0), &[Dive], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Dive]);
    let tail: Vec<&Note> = main.iter().filter(|n| n.start >= 12.0 - 1e-6).collect();
    assert!(tail.len() <= 2, "the attack and the held vowel: {tail:?}");
    let held = tail.last().unwrap();
    assert!(held.lyric.ends_with(":l"), "{held:?}");
    assert!((bend_end(held) + 12.0).abs() < 0.01);
    assert!(held.start + held.len <= 16.0 + 1e-6);
}

#[test]
fn a_riser_bends_up_an_octave_into_the_breath_before_the_drop() {
    let v = vocal();
    let mut main = chops_of(&v, 16.0);
    let o = apply(&At { silence: 0.5, ..at(&v, 16.0, "house", 0) }, &[Riser], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Riser]);
    assert!(!o.fx.is_empty());
    let first = o.fx.first().unwrap();
    let last = o.fx.last().unwrap();
    assert!((first.start - 7.5).abs() < 1e-6, "eight beats before the silence: {first:?}");
    assert!((last.start + last.len - 15.5).abs() < 1e-6);
    assert!((bend_end(last) - 12.0).abs() < 0.01);
    assert_eq!(main, chops_of(&v, 16.0), "the chops are left as they were");
}

#[test]
fn the_second_phrase_call_jumps_an_octave() {
    let v = vocal();
    let before = chops_of(&v, 32.0);
    let mut main = before.clone();
    let o = apply(&at(&v, 32.0, "house", 0), &[Octave], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Octave]);
    // bar 7 (index 6: the third bar of the second phrase), its downbeat
    let up: Vec<usize> = (0..main.len()).filter(|i| main[*i].key != before[*i].key).collect();
    assert!(!up.is_empty());
    for i in up {
        assert_eq!(main[i].key, before[i].key + 12);
        assert!((24.0..25.0).contains(&main[i].start), "{}", main[i].start);
    }
}

#[test]
fn chipmunk_doubles_follow_the_later_calls_an_octave_up_and_quieter() {
    let v = vocal();
    let mut main = chops_of(&v, 32.0);
    let o = apply(&at(&v, 32.0, "trap", 1), &[Chipmunk], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Chipmunk]);
    for d in &o.fx {
        let src = main.iter().find(|n| (n.start - d.start).abs() < 1e-9 && n.lyric == d.lyric).expect("a call it doubles");
        assert_eq!(d.key, src.key + 12);
        assert!(d.vel < src.vel);
        assert!((d.start % 16.0) < 2.0 && d.start >= 16.0, "the second phrase's calls: {}", d.start);
    }
}

#[test]
fn a_stutter_decays_on_the_last_beat_of_a_phrase() {
    let v = vocal();
    let mut main = chops_of(&v, 32.0);
    let o = apply(&at(&v, 32.0, "house", 0), &[Stutter], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Stutter]);
    // bar 8 (the second phrase's last), beat 4, on 16ths
    let st: Vec<&Note> = main.iter().filter(|n| n.start >= 31.0 - 1e-6).collect();
    assert_eq!(st.len(), 4, "{st:?}");
    assert!(st.windows(2).all(|w| w[1].vel < w[0].vel && w[1].len <= w[0].len + 1e-9));
    // into a drop the last bar's roll is left alone: the first phrase's instead
    let mut main = chops_of(&v, 32.0);
    let o = apply(&At { next_hot: true, ..at(&v, 32.0, "house", 0) }, &[Stutter], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Stutter]);
    assert_eq!(main.iter().filter(|n| n.start >= 15.0 - 1e-6 && n.start < 16.0).count(), 4);
}

#[test]
fn half_time_stretches_a_bar_over_two_and_double_time_squeezes_one_twice() {
    let v = vocal();
    let before = chops_of(&v, 32.0);
    let mut main = before.clone();
    let o = apply(&at(&v, 32.0, "dubstep", 0), &[HalfTime], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![HalfTime]);
    let src: Vec<&Note> = before.iter().filter(|n| (16.0..20.0).contains(&n.start)).collect();
    let got: Vec<&Note> = main.iter().filter(|n| (16.0..24.0).contains(&n.start)).collect();
    assert_eq!(src.len(), got.len());
    for (a, b) in src.iter().zip(&got) {
        assert!((b.start - (16.0 + 2.0 * (a.start - 16.0))).abs() < 1e-9 && b.lyric == a.lyric);
    }
    let mut main = before.clone();
    let o = apply(&at(&v, 32.0, "drum and bass", 0), &[DoubleTime], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![DoubleTime]);
    // the next-to-last bar (24..28): twice the notes, each phrase in two beats
    let src = before.iter().filter(|n| (24.0..28.0).contains(&n.start)).count();
    let got: Vec<&Note> = main.iter().filter(|n| (24.0..28.0).contains(&n.start)).collect();
    assert_eq!(got.len(), 2 * src);
    assert!(got.iter().filter(|n| n.start < 26.0).count() == src);
}

#[test]
fn a_trance_gate_retriggers_the_held_vowel_on_sixteenths() {
    let v = vocal();
    let mut main = chops_of(&v, 32.0);
    let o = apply(&at(&v, 32.0, "trance", 0), &[Gate], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Gate]);
    assert!(o.fx.len() >= 16 && o.fx.len() < 32, "a pattern with holes: {}", o.fx.len());
    for n in &o.fx {
        assert!(n.lyric.ends_with(":l"));
        assert!(((n.start * 4.0) - (n.start * 4.0).round()).abs() < 1e-9 && n.len < 0.25);
        assert!((16.0..24.0).contains(&n.start), "the second half: {}", n.start);
    }
    // off the Slicer (no loops): none
    let o = apply(&At { slicer: false, ..at(&v, 32.0, "trance", 0) }, &[Gate], &mut main, &mut vec![]);
    assert!(o.fx.iter().all(|n| !n.lyric.ends_with(":l")));
}

#[test]
fn a_sweep_opens_across_the_run_up() {
    let v = vocal();
    let mut main = chops_of(&v, 32.0);
    let o = apply(&at(&v, 32.0, "house", 0), &[Sweep], &mut main, &mut vec![]);
    assert_eq!(o.sweeps, vec![(16.0, 32.0)]);
    let lane = cutoff_lane(&[(80.0, 96.0)]);
    let at_ = |b: f64| {
        let i = lane.iter().rposition(|p| p.0 <= b).unwrap();
        lane[i].1
    };
    assert_eq!(at_(40.0), 1.0);
    assert!(at_(80.0) < 0.5 && at_(97.0) == 1.0);
}

#[test]
fn glides_slide_into_every_other_long_note() {
    let v = vocal();
    let t = Tune::default();
    // long notes a step or two apart, a beat each
    let mut main: Vec<Note> = (0..8).map(|k| {
        let c = &v.pieces[k].chop;
        crate::chops::tuned_note(c, k as f64 * 2.0, 1.5, c.midi.unwrap().round() as i32, 0.7, &t, 0).0
    }).collect();
    let o = apply(&at(&v, 16.0, "lofi", 0), &[Glide], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Glide]);
    let glided: Vec<&Note> = main.iter().filter(|n| !n.bend.is_empty()).collect();
    assert!(!glided.is_empty() && glided.len() <= 4);
    for n in glided {
        assert!(n.bend[0].1.abs() >= 0.9 && n.bend[0].1.abs() <= 7.1 && n.bend.last().unwrap().1 == 0.0, "{:?}", n.bend);
    }
}

#[test]
fn a_flip_resequences_other_words_into_a_new_hook_on_the_chord_tones() {
    let v = vocal();
    let mut main = chops_of(&v, 32.0);
    let o = apply(&at(&v, 32.0, "pop", 0), &[Flip], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Flip]);
    // bar 6 (the second bar of the second phrase)
    let f: Vec<&Note> = main.iter().filter(|n| (20.0..24.0).contains(&n.start)).collect();
    assert_eq!(f.len(), 6, "{f:?}");
    let ps: std::collections::HashSet<String> = f.iter().map(|n| n.lyric.split(':').next().unwrap().to_string()).collect();
    assert!(ps.len() >= 3, "syllables of three words at least");
    let hook = v.hook_word.unwrap();
    let hs = format!("@{}", v.pieces[v.word_pieces(hook).start].chop.start);
    assert!(!ps.contains(&hs), "not the hook");
    // what they sound: A minor
    for n in f {
        let (_, _, cents) = crate::dsp::sampler::parse_slice(&n.lyric).unwrap();
        let pi = v.pieces.iter().position(|p| n.lyric.starts_with(&format!("@{}:", p.chop.start))).unwrap();
        let heard = v.pieces[pi].chop.midi.unwrap() + n.key as f32 - 60.0 + cents / 100.0;
        let pc = (heard.round() as i32).rem_euclid(12) as u8;
        assert!(Key::new(9, true).scale().contains(&pc), "{heard}");
    }
}

#[test]
fn a_pad_holds_a_vowel_under_the_break() {
    let v = vocal();
    let mut main = chops_of(&v, 16.0);
    let o = apply(&at(&v, 16.0, "lofi", 0), &[Pad], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Pad]);
    let held: Vec<&Note> = o.fx.iter().filter(|n| n.lyric.ends_with(":l")).collect();
    assert_eq!(held.len(), 2, "one per two bars: {:?}", o.fx);
    assert!(held.iter().all(|n| n.len >= 7.0 && n.vel < 0.5));
}

#[test]
fn ear_candy_rings_out_at_the_ends_of_phrases_where_there_is_room() {
    let v = vocal();
    // the chops leave the last beat of every bar free
    let mut main: Vec<Note> = chops_of(&v, 32.0).into_iter().filter(|n| n.start % 4.0 < 3.0).collect();
    let o = apply(&at(&v, 32.0, "dubstep", 0), &[Candy], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Candy]);
    assert!(!o.fx.is_empty() && o.fx.len() <= 2);
    for n in &o.fx {
        assert!(n.lyric.contains(":") && n.lyric.ends_with('o'), "a one-shot: {}", n.lyric);
        assert!(n.start % 16.0 >= 12.0 + 3.25 - 1e-9, "the end of a phrase: {}", n.start);
    }
    if o.fx.len() == 2 {
        assert!(o.fx[0].pan * o.fx[1].pan < 0.0, "left and right in turn");
    }
    // a busy last beat everywhere: no room
    let mut busy = chops_of(&v, 32.0);
    for b in 0..8 {
        busy.push(Note::new(b as f64 * 4.0 + 3.5, 0.25, 60, 0.5));
    }
    assert!(apply(&at(&v, 32.0, "dubstep", 0), &[Candy], &mut busy, &mut vec![]).used.is_empty());
}

#[test]
fn a_choke_cut_lets_each_hit_ring_into_the_next() {
    let v = vocal();
    let mut main = chops_of(&v, 16.0);
    let o = apply(&at(&v, 16.0, "trap", 0), &[Choke], &mut main, &mut vec![]);
    assert_eq!(o.used, vec![Choke]);
    let ch: Vec<&Note> = main.iter().filter(|n| n.lyric.ends_with(":x")).collect();
    assert_eq!(ch.len(), 5);
    assert!((4.0..8.0).contains(&ch[0].start), "the second bar");
    for w in ch.windows(2) {
        assert!(w[0].start + w[0].len > w[1].start || (w[0].len - 1.0).abs() < 1e-9, "it rings until the next cuts it (a beat at most)");
    }
}

#[test]
fn an_air_pocket_clears_the_last_half_beat() {
    let v = vocal();
    let mut main = chops_of(&v, 16.0);
    main.push(Note::new(15.6, 0.4, 60, 0.5));
    let mut layer = main.clone();
    let o = apply(&at(&v, 16.0, "house", 0), &[Pocket], &mut main, &mut layer);
    assert_eq!(o.pockets, vec![(15.5, 16.0)]);
    assert!(main.iter().chain(layer.iter()).all(|n| n.start + n.len <= 15.5 + 1e-6 || n.start + n.len <= 15.0), "nothing sounds in the pocket");
    // a build's own breath: none
    let o = apply(&At { silence: 0.5, ..at(&v, 16.0, "house", 0) }, &[Pocket], &mut chops_of(&v, 16.0), &mut vec![]);
    assert!(o.pockets.is_empty());
    let mut ns = vec![Note::new(0.0, 4.0, 60, 0.5), Note::new(15.6, 0.2, 60, 0.5)];
    assert_eq!(cut_pockets(&mut ns, 64.0, &[(79.5, 80.0)]), 1);
    assert_eq!(ns[0].len, 4.0);
}

#[test]
fn an_echo_throw_opens_on_the_last_word_and_closes_after_the_echoes() {
    let v = vocal();
    let mut main = chops_of(&v, 16.0);
    let o = apply(&at(&v, 16.0, "pop", 0), &[Throw], &mut main, &mut vec![]);
    assert_eq!(o.throws.len(), 1);
    let (a, b) = o.throws[0];
    assert!((a - 15.0).abs() < 1e-9 && (b - (15.4 + 3.0)).abs() < 1e-9, "{a} {b}");
    // the lane: dry, open one echo after the word (the word's echo, not the one before), dry once it has rung
    let lane = throw_lane(&[(10.0, 14.0), (12.0, 15.0)], 0.75, 0.4);
    assert_eq!(lane.len(), 5, "overlapping throws merge: {lane:?}");
    assert!((lane[1].0 - 10.74).abs() < 1e-9 && lane[1].1 == 0.0);
    assert!((lane[2].0 - 10.75).abs() < 1e-9 && lane[2].1 == 0.4);
    assert!((lane[4].0 - 15.0).abs() < 1e-9 && lane[4].1 == 0.0);
    assert_eq!(echo_beats("house"), 0.75);
    assert_eq!(echo_beats("trap"), 1.0);
}

#[test]
fn the_answers_fall_between_the_singers_lines() {
    // half a beat after a line ends, where the rest holds the answer (1.5 beats): not after the first line (one beat to the next)
    let sung = [(0.0, 6.5), (8.0, 13.0), (16.0, 22.0)];
    assert_eq!(response_at(&sung, 24.0, 1.5, 3), vec![13.5, 22.5]);
    assert_eq!(response_at(&sung, 24.0, 1.5, 1), vec![13.5], "at most `max`");
    assert!(response_at(&sung, 23.5, 1.5, 3) == vec![13.5], "nor past the section's end");
    let v = vocal();
    let ns = answer_notes(&v, 5.5, (9, true), &[], &Tune::default(), 0.7);
    assert!(!ns.is_empty() && ns.iter().all(|n| n.start >= 5.5 - 1e-9 && n.vel <= 0.7));
}

#[test]
fn a_whole_section_with_its_budget_stays_tasteful() {
    let v = vocal();
    let song = edm_song();
    for seed in 0..8 {
        let p = plan(&song, "dubstep", &Which::default(), &content(), seed);
        for (i, s) in song.iter().enumerate() {
            let before = chops_of(&v, s.beats);
            let mut main = before.clone();
            let o = apply(&At { seed, nth: i / 3, ..at(&v, s.beats, "dubstep", 0) }, &p[i], &mut main, &mut vec![]);
            assert!(o.used.len() <= 5, "{:?}", o.used);
            // most bars of the section are as composed: the edits touch a few bars, never all of them
            let bars = (s.beats / 4.0) as usize;
            let touched = (0..bars).filter(|b| {
                let t0 = *b as f64 * 4.0;
                let a: Vec<&Note> = before.iter().filter(|n| n.start >= t0 && n.start < t0 + 4.0).collect();
                let c: Vec<&Note> = main.iter().filter(|n| n.start >= t0 && n.start < t0 + 4.0).collect();
                a != c
            }).count();
            assert!(touched < bars.max(2) && touched * 8 <= bars * 5 + 7, "section {i} {:?}: {touched} of {bars} bars edited", o.used);
        }
    }
}

#[test]
fn flags_go_in_the_fourth_field_of_the_slice_lyric() {
    let mut n = Note::new(0.0, 1.0, 60, 0.8);
    n.lyric = "@100:200:-30".into();
    flag(&mut n, "r");
    flag(&mut n, "o");
    assert_eq!(n.lyric, "@100:200:-30:ro");
    assert_eq!(crate::dsp::sampler::parse_slice(&n.lyric), Some((100, 200, -30.0)), "the Sampler still reads it");
    assert_eq!(crate::chops::kit::flags_of(&n.lyric), "ro");
}

impl Which {
    fn only_for_test(ks: &[Kind]) -> Which {
        Which { on: true, only: ks.to_vec(), scale: 1.0 }
    }
}
