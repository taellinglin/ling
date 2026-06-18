//! Standard MIDI File (`.mid`) loading → timed note events, via `midly`.
//!
//! Builds a tempo map from all tracks (format-1 tracks run in parallel from tick
//! 0, so each track's absolute ticks are global), converts ticks→seconds, and
//! pairs note-on/off into [`MidiNote`]s sorted by start time.

use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};

/// One note extracted from a MIDI file.
#[derive(Clone, Copy, Debug)]
pub struct MidiNote {
    pub time: f32, // seconds
    pub dur: f32,  // seconds
    pub midi: u8,  // 0..127
    pub vel: u8,   // 1..127
    pub channel: u8,
}

/// A parsed MIDI song.
#[derive(Clone, Debug, Default)]
pub struct MidiSong {
    pub notes: Vec<MidiNote>,
    pub duration: f32,
}

/// Load a `.mid` file.
pub fn load(path: &str) -> Result<MidiSong, String> {
    let data = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    from_bytes(&data)
}

/// Parse a Standard MIDI File from bytes.
pub fn from_bytes(data: &[u8]) -> Result<MidiSong, String> {
    let smf = Smf::parse(data).map_err(|e| format!("midi parse: {e}"))?;

    // ticks per quarter-note (metrical) or ticks per second (timecode)
    let (tpqn, timecode_tps): (f64, Option<f64>) = match smf.header.timing {
        Timing::Metrical(t) => (t.as_int() as f64, None),
        Timing::Timecode(fps, sub) => (1.0, Some(fps.as_f32() as f64 * sub as f64)),
    };

    // ── tempo map: (abs_tick, microseconds-per-quarter) sorted ──
    let mut tempo_map: Vec<(u64, u32)> = Vec::new();
    for track in &smf.tracks {
        let mut tick = 0u64;
        for ev in track {
            tick += ev.delta.as_int() as u64;
            if let TrackEventKind::Meta(MetaMessage::Tempo(us)) = ev.kind {
                tempo_map.push((tick, us.as_int()));
            }
        }
    }
    tempo_map.sort_by_key(|e| e.0);
    if tempo_map.is_empty() || tempo_map[0].0 != 0 {
        tempo_map.insert(0, (0, 500_000)); // default 120 BPM
    }

    // tick → seconds, integrating the tempo segments (or fixed for timecode)
    let tick_to_sec = |tick: u64| -> f32 {
        if let Some(tps) = timecode_tps {
            return (tick as f64 / tps) as f32;
        }
        let mut sec = 0.0f64;
        for i in 0..tempo_map.len() {
            let (seg_start, us) = tempo_map[i];
            if seg_start >= tick {
                break;
            }
            let seg_end = tempo_map
                .get(i + 1)
                .map(|e| e.0)
                .unwrap_or(u64::MAX)
                .min(tick);
            let dticks = seg_end.saturating_sub(seg_start) as f64;
            sec += dticks / tpqn * (us as f64 / 1_000_000.0);
        }
        sec as f32
    };

    // ── collect notes ──
    let mut notes: Vec<MidiNote> = Vec::new();
    let mut duration = 0.0f32;
    for track in &smf.tracks {
        let mut tick = 0u64;
        // pending note-ons per (channel, key): stack of (start_tick, vel)
        let mut pending: std::collections::HashMap<(u8, u8), Vec<(u64, u8)>> =
            std::collections::HashMap::new();
        for ev in track {
            tick += ev.delta.as_int() as u64;
            if let TrackEventKind::Midi { channel, message } = ev.kind {
                let ch = channel.as_int();
                match message {
                    MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                        pending
                            .entry((ch, key.as_int()))
                            .or_default()
                            .push((tick, vel.as_int()));
                    },
                    MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                        if let Some(stack) = pending.get_mut(&(ch, key.as_int())) {
                            if let Some((start, vel)) = stack.pop() {
                                let t = tick_to_sec(start);
                                let end = tick_to_sec(tick);
                                duration = duration.max(end);
                                notes.push(MidiNote {
                                    time: t,
                                    dur: (end - t).max(0.0),
                                    midi: key.as_int(),
                                    vel,
                                    channel: ch,
                                });
                            }
                        }
                    },
                    _ => {},
                }
            }
        }
    }
    notes.sort_by(|a, b| {
        a.time
            .partial_cmp(&b.time)
            .unwrap_or(std::cmp::Ordering::Equal)
    });
    Ok(MidiSong { notes, duration })
}

#[cfg(test)]
mod tests {
    use super::*;
    use midly::{
        num::*, Format, Header, MetaMessage, MidiMessage, Timing, Track, TrackEvent, TrackEventKind,
    };

    /// Build a tiny SMF in memory: 120 BPM, two quarter notes (C4 then E4).
    fn synth_smf() -> Vec<u8> {
        let mut track = Track::new();
        track.push(TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::Tempo(500_000.into())),
        });
        // C4 on @0, off @480 (one quarter at 480 tpqn)
        track.push(TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Midi {
                channel: 0.into(),
                message: MidiMessage::NoteOn { key: 60.into(), vel: 100.into() },
            },
        });
        track.push(TrackEvent {
            delta: 480.into(),
            kind: TrackEventKind::Midi {
                channel: 0.into(),
                message: MidiMessage::NoteOff { key: 60.into(), vel: 0.into() },
            },
        });
        // E4 on @480, off @960
        track.push(TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Midi {
                channel: 0.into(),
                message: MidiMessage::NoteOn { key: 64.into(), vel: 100.into() },
            },
        });
        track.push(TrackEvent {
            delta: 480.into(),
            kind: TrackEventKind::Midi {
                channel: 0.into(),
                message: MidiMessage::NoteOff { key: 64.into(), vel: 0.into() },
            },
        });
        track.push(TrackEvent {
            delta: 0.into(),
            kind: TrackEventKind::Meta(MetaMessage::EndOfTrack),
        });
        let smf = Smf {
            header: Header::new(Format::SingleTrack, Timing::Metrical(u15::new(480))),
            tracks: vec![track],
        };
        let mut out = Vec::new();
        smf.write(&mut out).unwrap();
        out
    }

    #[test]
    fn parses_two_notes_with_tempo() {
        let song = from_bytes(&synth_smf()).unwrap();
        assert_eq!(song.notes.len(), 2);
        assert_eq!(song.notes[0].midi, 60);
        assert_eq!(song.notes[1].midi, 64);
        // sorted by time
        assert!(song.notes[0].time <= song.notes[1].time);
        // at 120 BPM a quarter = 0.5 s, so the 2nd note starts ~0.5 s
        assert!(
            (song.notes[1].time - 0.5).abs() < 0.02,
            "got {}",
            song.notes[1].time
        );
        assert!((song.notes[0].dur - 0.5).abs() < 0.02);
        assert!((song.duration - 1.0).abs() < 0.02);
    }
}
