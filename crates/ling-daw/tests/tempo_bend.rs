use ling_daw::midi_import::parse;
use ling_daw::*;
use midly::{num::*, Format, Header, MetaMessage, MidiMessage, Smf, Timing, TrackEvent, TrackEventKind};

fn ev(delta: u32, kind: TrackEventKind<'static>) -> TrackEvent<'static> {
    TrackEvent { delta: u28::new(delta), kind }
}
fn midi(m: MidiMessage) -> TrackEventKind<'static> {
    TrackEventKind::Midi { channel: u4::new(0), message: m }
}

#[test]
fn tempo_changes_and_pitch_bend_are_imported() {
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(480))));
    smf.tracks.push(vec![
        ev(0, TrackEventKind::Meta(MetaMessage::Tempo(u24::new(500_000)))), // 120
        ev(480 * 4, TrackEventKind::Meta(MetaMessage::Tempo(u24::new(1_000_000)))), // 60 from beat 4
        ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
    ]);
    smf.tracks.push(vec![
        ev(0, midi(MidiMessage::Controller { controller: u7::new(101), value: u7::new(0) })),
        ev(0, midi(MidiMessage::Controller { controller: u7::new(100), value: u7::new(0) })),
        ev(0, midi(MidiMessage::Controller { controller: u7::new(6), value: u7::new(12) })),
        ev(0, midi(MidiMessage::NoteOn { key: u7::new(60), vel: u7::new(100) })),
        ev(240, midi(MidiMessage::PitchBend { bend: midly::PitchBend::from_int(8191) })),
        ev(240, midi(MidiMessage::PitchBend { bend: midly::PitchBend::from_int(-8192) })),
        ev(240, midi(MidiMessage::PitchBend { bend: midly::PitchBend::from_int(0) })),
        ev(240, midi(MidiMessage::NoteOff { key: u7::new(60), vel: u7::new(0) })),
        ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
    ]);
    let mut bytes = Vec::new();
    smf.write(&mut bytes).unwrap();
    let m = parse(&bytes).unwrap();
    assert_eq!(m.tempos.len(), 2);
    assert!((m.tempos[0].1 - 120.0).abs() < 1e-6 && (m.tempos[1].0 - 4.0).abs() < 1e-9 && (m.tempos[1].1 - 60.0).abs() < 1e-6);
    let t = &m.tracks[0];
    assert_eq!(t.bend_range, 12.0);
    // a monophonic part whose wheel moves only while its note sounds: the wheel becomes the note's own pitch gesture (no lane)
    assert!(t.bend_auto.is_empty(), "{:?}", t.bend_auto);
    let b = &t.notes[0].bend;
    assert!(b.iter().any(|p| p.1 > 11.9) && b.iter().any(|p| p.1 < -11.9), "{b:?}");
    // the same wheel under two overlapping notes stays the track's bend lane
    let mut smf = Smf::new(Header::new(Format::Parallel, Timing::Metrical(u15::new(480))));
    smf.tracks.push(vec![
        ev(0, midi(MidiMessage::NoteOn { key: u7::new(60), vel: u7::new(100) })),
        ev(0, midi(MidiMessage::NoteOn { key: u7::new(64), vel: u7::new(100) })),
        ev(240, midi(MidiMessage::PitchBend { bend: midly::PitchBend::from_int(8191) })),
        ev(240, midi(MidiMessage::PitchBend { bend: midly::PitchBend::from_int(0) })),
        ev(240, midi(MidiMessage::NoteOff { key: u7::new(60), vel: u7::new(0) })),
        ev(0, midi(MidiMessage::NoteOff { key: u7::new(64), vel: u7::new(0) })),
        ev(0, TrackEventKind::Meta(MetaMessage::EndOfTrack)),
    ]);
    let mut bytes = Vec::new();
    smf.write(&mut bytes).unwrap();
    let m = parse(&bytes).unwrap();
    assert_eq!(m.tracks[0].bend_auto.len(), 2, "{:?}", m.tracks[0].bend_auto);
    assert!(m.tracks[0].notes.iter().all(|n| n.bend.is_empty()));
}

#[test]
fn tempo_map_drives_time_and_bounce_length() {
    let sr = 44100.0;
    let mut e = Engine::new(sr);
    e.project.bpm = 120.0;
    e.project.tempo_map = vec![(4.0, 60.0)];
    // 4 beats at 120 (2 s) + 4 beats at 60 (4 s)
    assert!((e.project.beats_to_secs(0.0, 8.0) - 6.0).abs() < 1e-9);
    assert!((e.project.bpm_at(3.9) - 120.0).abs() < 1e-9 && (e.project.bpm_at(4.0) - 60.0).abs() < 1e-9);
    let out = e.bounce_range(0.0, 8.0, 0.0);
    let secs = out.len() as f64 / 2.0 / sr as f64;
    assert!((secs - 6.0).abs() < 0.01, "bounce length {secs}");
}

#[test]
fn pitch_bend_param_shifts_pitch() {
    use ling_daw::dsp::synth::LingSynth;
    use ling_daw::dsp::Instrument;
    fn freq(bend: f32) -> f32 {
        let mut s = LingSynth::new(48000.0);
        s.set_param("sub", 1.0);
        s.set_param("wave", 0.0);
        s.set_param("cutoff", 1.0);
        s.set_param("bend range", (12.0 - 1.0) / 23.0);
        s.set_param("bend", bend);
        s.note_on(57, 1.0); // 220 Hz
        let mut b = vec![0.0; 48000 * 2];
        s.render(&mut b);
        let mut cross = 0;
        for w in b[20000..].windows(4).step_by(2) {
            if w[0] <= 0.0 && w[2] > 0.0 {
                cross += 1;
            }
        }
        cross as f32 / ((48000 * 2 - 20000) as f32 / 2.0 / 48000.0) // Hz
    }
    let base = freq(0.5);
    let up = freq(1.0);
    assert!((up / base - 2.0).abs() < 0.05, "full bend up = one octave (12 st), got {}", up / base);
}
