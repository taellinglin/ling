//! Multi-track General MIDI import: every (MIDI track, channel, program) becomes its own DAW track,
//! melodic parts get the Ling Synth X patch tuned for their GM program (with its effects), channel 10
//! gets the physical-modelling drum kit. Channel volume / expression / pan become the track's mixer
//! settings plus automation lanes, and the sustain pedal lengthens notes like a real piano would.
use crate::model::Note;
use midly::{MetaMessage, MidiMessage, Smf, Timing, TrackEventKind};

#[derive(Default, Clone)]
pub struct ImportedTrack {
    pub name: String,
    /// the FL Studio plugin the channel used ("Sytrus", "3x Osc" ...); empty for MIDI files
    pub plugin: String,
    /// the file gave this part a name (otherwise `name` is the GM instrument)
    pub named: bool,
    /// GM program (0..127); `None` for the percussion channel
    pub program: Option<u8>,
    /// bank select (CC0) in force for the part
    pub bank: u16,
    pub channel: u8,
    pub notes: Vec<Note>,
    /// mixer level / pan at the first note (CC7 x CC11, CC10)
    pub volume: f32,
    pub pan: f32,
    /// automation from volume / expression / pan controller changes: (beat, value)
    pub vol_auto: Vec<(f64, f32)>,
    pub pan_auto: Vec<(f64, f32)>,
    /// pitch-wheel moves (beat, 0..1 with 0.5 = centre) and the wheel range in semitones (RPN 0)
    pub bend_auto: Vec<(f64, f32)>,
    pub bend_range: f32,
    /// audio samples to place instead of notes: (beat, file) - used by project imports (FL Studio audio clips)
    pub samples: Vec<(f64, String)>,
    /// a Sampler playing the part's own samples: (config JSON, parameters) - tracker modules
    pub sampler: Option<(String, Vec<(String, f32)>)>,
    /// the part's name sounds like a drum or percussion piece
    pub percussive: bool,
    /// further automation lanes by parameter name: (beat, value)
    pub extra_auto: Vec<(String, Vec<(f64, f32)>)>,
}

#[derive(Default, Clone)]
pub struct ImportedMidi {
    pub tracks: Vec<ImportedTrack>,
    /// first tempo in the file
    pub bpm: Option<f64>,
    /// every tempo change (beat, bpm), the first at beat 0
    pub tempos: Vec<(f64, f64)>,
    pub length_beats: f64,
}

pub const GM_NAMES: [&str; 128] = [
    "Acoustic Grand Piano", "Bright Acoustic Piano", "Electric Grand Piano", "Honky-tonk Piano", "Electric Piano 1", "Electric Piano 2", "Harpsichord", "Clavinet",
    "Celesta", "Glockenspiel", "Music Box", "Vibraphone", "Marimba", "Xylophone", "Tubular Bells", "Dulcimer",
    "Drawbar Organ", "Percussive Organ", "Rock Organ", "Church Organ", "Reed Organ", "Accordion", "Harmonica", "Tango Accordion",
    "Nylon Guitar", "Steel Guitar", "Jazz Guitar", "Clean Guitar", "Muted Guitar", "Overdriven Guitar", "Distortion Guitar", "Guitar Harmonics",
    "Acoustic Bass", "Finger Bass", "Pick Bass", "Fretless Bass", "Slap Bass 1", "Slap Bass 2", "Synth Bass 1", "Synth Bass 2",
    "Violin", "Viola", "Cello", "Contrabass", "Tremolo Strings", "Pizzicato Strings", "Orchestral Harp", "Timpani",
    "String Ensemble 1", "String Ensemble 2", "Synth Strings 1", "Synth Strings 2", "Choir Aahs", "Voice Oohs", "Synth Voice", "Orchestra Hit",
    "Trumpet", "Trombone", "Tuba", "Muted Trumpet", "French Horn", "Brass Section", "Synth Brass 1", "Synth Brass 2",
    "Soprano Sax", "Alto Sax", "Tenor Sax", "Baritone Sax", "Oboe", "English Horn", "Bassoon", "Clarinet",
    "Piccolo", "Flute", "Recorder", "Pan Flute", "Blown Bottle", "Shakuhachi", "Whistle", "Ocarina",
    "Square Lead", "Saw Lead", "Calliope Lead", "Chiff Lead", "Charang Lead", "Voice Lead", "Fifths Lead", "Bass + Lead",
    "New Age Pad", "Warm Pad", "Polysynth Pad", "Choir Pad", "Bowed Pad", "Metallic Pad", "Halo Pad", "Sweep Pad",
    "Rain FX", "Soundtrack FX", "Crystal FX", "Atmosphere FX", "Brightness FX", "Goblins FX", "Echoes FX", "Sci-Fi FX",
    "Sitar", "Banjo", "Shamisen", "Koto", "Kalimba", "Bagpipe", "Fiddle", "Shanai",
    "Tinkle Bell", "Agogo", "Steel Drums", "Woodblock", "Taiko Drum", "Melodic Tom", "Synth Drum", "Reverse Cymbal",
    "Guitar Fret Noise", "Breath Noise", "Seashore", "Bird Tweet", "Telephone Ring", "Helicopter", "Applause", "Gunshot",
];

pub fn gm_name(program: u8) -> &'static str {
    GM_NAMES[(program & 127) as usize]
}

/// The stock patch for a GM program: tuned Ling Synth X parameters plus its effects.
pub fn gm_patch(program: u8) -> crate::patch::Patch {
    use std::sync::OnceLock;
    static ALL: OnceLock<Vec<crate::patch::Patch>> = OnceLock::new();
    let all = ALL.get_or_init(|| {
        let text = include_str!("gm_patches.txt");
        let mut out: Vec<crate::patch::Patch> = Vec::new();
        let mut cur = String::new();
        for line in text.lines() {
            if line.starts_with('[') && line.ends_with(']') {
                if !cur.is_empty() {
                    out.push(crate::patch::parse(&cur));
                }
                cur.clear();
            } else {
                cur.push_str(line);
                cur.push('\n');
            }
        }
        if !cur.is_empty() {
            out.push(crate::patch::parse(&cur));
        }
        out
    });
    all.get((program & 127) as usize).cloned().unwrap_or_default()
}

/// One sounding part: a (MIDI track, channel, program) combination.
struct Seg {
    chan: u8,
    program: Option<u8>,
    bank: u16,
    notes: Vec<Note>,
}

/// controller changes per channel: (beat, value)
#[derive(Default, Clone)]
struct Lanes {
    vol: Vec<(f64, f32)>,
    pan: Vec<(f64, f32)>,
    bend: Vec<(f64, f32)>,
    /// per-note expression sources: polyphonic aftertouch (beat, key, 0..1), channel aftertouch and CC74 (beat, 0..1)
    poly_at: Vec<(f64, u8, f32)>,
    chan_at: Vec<(f64, f32)>,
    cc74: Vec<(f64, f32)>,
}

/// The points of a channel-wide controller inside a note, as a curve over the note: the value in force at its start, then
/// every change while it sounds. Empty when nothing moves inside or before it (or it stays at `rest`).
fn note_curve(pts: &[(f64, f32)], n: &Note, rest: f32) -> crate::model::Curve {
    let (s, e) = (n.start, n.start + n.len.max(1e-6));
    let first = pts.iter().filter(|p| p.0 <= s + 1e-9).last().map(|p| p.1).unwrap_or(rest);
    let mut c = vec![(0.0f32, first)];
    c.extend(pts.iter().filter(|p| p.0 > s + 1e-9 && p.0 < e - 1e-9).map(|p| (((p.0 - s) / (e - s)) as f32, p.1)));
    let mut c = crate::gesture::tidy(&c);
    crate::gesture::prune(&mut c, rest);
    c
}

/// Per-note expression from the channel's controllers: polyphonic aftertouch on the note's key and channel aftertouch become
/// its pressure, CC74 its timbre (MPE slide), and on a monophonic part whose wheel only moves while notes sound, the pitch wheel
/// becomes each note's pitch gesture (returns true: the wheel needs no lane then).
fn note_expression(notes: &mut [Note], ln: &Lanes, bend_range: f32) -> bool {
    for n in notes.iter_mut() {
        let poly: Vec<(f64, f32)> = ln.poly_at.iter().filter(|p| p.1 == n.key).map(|p| (p.0, p.2)).collect();
        // pressure starts at 0 for each note (aftertouch is sent after the note-on)
        let mut pr = note_curve(&poly.iter().copied().filter(|p| p.0 > n.start - 1e-9).collect::<Vec<_>>(), n, 0.0);
        if pr.is_empty() {
            pr = note_curve(&ln.chan_at, n, 0.0);
        }
        n.press = pr;
        n.timbre = note_curve(&ln.cc74, n, 0.5);
    }
    let moving = ln.bend.iter().any(|p| (p.1 - 0.5).abs() > 0.002);
    if !moving || notes.is_empty() {
        return false;
    }
    let mut order: Vec<usize> = (0..notes.len()).collect();
    order.sort_by(|a, b| notes[*a].start.total_cmp(&notes[*b].start));
    let mono = order.windows(2).all(|w| notes[w[1]].start >= notes[w[0]].start + notes[w[0]].len - 0.01);
    // every off-centre wheel move falls inside a note (a move between notes would bend the next one before it starts)
    let inside = ln.bend.iter().filter(|p| (p.1 - 0.5).abs() > 0.002).all(|p| notes.iter().any(|n| p.0 >= n.start - 1e-6 && p.0 < n.start + n.len + 1e-6));
    if !mono || !inside {
        return false;
    }
    let semis: Vec<(f64, f32)> = ln.bend.iter().map(|p| (p.0, (p.1 - 0.5) * 2.0 * bend_range)).collect();
    for n in notes.iter_mut() {
        n.bend = note_curve(&semis, n, 0.0);
    }
    true
}

struct ChanState {
    program: u8,
    bank: u16,
    cc7: f32,
    cc11: f32,
    pedal: bool,
    rpn: (u8, u8),
    bend_range: f32,
    /// key -> (start tick, vel, seg)
    pending: std::collections::HashMap<u8, Vec<(u64, u8, usize)>>,
    /// notes released while the sustain pedal is down: (key, start, vel, seg)
    held: Vec<(u8, u64, u8, usize)>,
}

fn vol_norm(cc7: f32, cc11: f32) -> f32 {
    // GM default volume 100/127 maps to unity
    ((cc7 / 127.0) * (cc11 / 127.0) / (100.0 / 127.0)).clamp(0.0, 1.5)
}

/// Keep automation light: drop repeated values and sub-1/16-beat jitter.
fn thin(mut pts: Vec<(f64, f32)>) -> Vec<(f64, f32)> {
    pts.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut out: Vec<(f64, f32)> = Vec::new();
    for p in pts {
        if let Some(last) = out.last_mut() {
            if (p.0 - last.0).abs() < 1e-6 {
                last.1 = p.1; // same instant: the last one wins
                continue;
            }
            if (p.1 - last.1).abs() < 0.004 || (p.0 - last.0 < 1.0 / 16.0 && (p.1 - last.1).abs() < 0.03) {
                continue;
            }
        }
        out.push(p);
    }
    out
}

/// Read a MIDI file or an FL Studio project, whichever `data` is.
pub fn parse_any(path: &str, data: &[u8]) -> Result<ImportedMidi, String> {
    if crate::tracker::looks_like_module(data) {
        let dir = std::env::temp_dir().join("lingstation-tracker");
        crate::tracker::parse(data, path, &dir).map(|(m, _)| m)
    } else if crate::flp_import::looks_like_flp(data) {
        crate::flp_import::parse(data, Some(std::path::Path::new(path)))
    } else {
        parse(data)
    }
}

pub fn parse(data: &[u8]) -> Result<ImportedMidi, String> {
    let smf = Smf::parse(data).map_err(|e| format!("midi parse: {e}"))?;
    let tpqn = match smf.header.timing {
        Timing::Metrical(t) => t.as_int().max(1) as f64,
        // SMPTE: assume 120 bpm so one second is two beats
        Timing::Timecode(fps, sub) => (fps.as_f32() as f64 * sub as f64 / 2.0).max(1.0),
    };
    let mut tempos: Vec<(f64, f64)> = Vec::new();
    let mut out: Vec<ImportedTrack> = Vec::new();
    for track in smf.tracks.iter() {
        let mut tick = 0u64;
        let mut tname = String::new();
        let mut segs: Vec<Seg> = Vec::new();
        let mut states: Vec<(u8, ChanState, Lanes)> = Vec::new();
        for ev in track {
            tick += ev.delta.as_int() as u64;
            let beat = tick as f64 / tpqn;
            match ev.kind {
                TrackEventKind::Meta(MetaMessage::TrackName(n)) if tname.is_empty() => {
                    tname = String::from_utf8_lossy(n).trim().chars().filter(|c| !c.is_control()).collect();
                }
                TrackEventKind::Meta(MetaMessage::Tempo(us)) if us.as_int() > 0 => {
                    let v = 60_000_000.0 / us.as_int() as f64;
                    tempos.push((beat, v));
                }
                TrackEventKind::Midi { channel, message } => {
                    let c = channel.as_int();
                    let si = match states.iter().position(|(k, _, _)| *k == c) {
                        Some(i) => i,
                        None => {
                            states.push((c, ChanState { program: 0, bank: 0, cc7: 100.0, cc11: 127.0, pedal: false, rpn: (127, 127), bend_range: 2.0, pending: Default::default(), held: vec![] }, Lanes::default()));
                            states.len() - 1
                        }
                    };
                    let (_, st, ln) = &mut states[si];
                    // part index for the current program on this channel
                    let mut seg_of = |program: u8, bank: u16| -> usize {
                        let prog = if c == 9 { None } else { Some(program) };
                        match segs.iter().position(|s| s.chan == c && s.program == prog && s.bank == bank) {
                            Some(i) => i,
                            None => {
                                segs.push(Seg { chan: c, program: prog, bank, notes: vec![] });
                                segs.len() - 1
                            }
                        }
                    };
                    match message {
                        MidiMessage::ProgramChange { program } => st.program = program.as_int(),
                        MidiMessage::PitchBend { bend } => {
                            // -8192..8191 -> 0..1
                            ln.bend.push((beat, (bend.as_int() as f32 / 8192.0 * 0.5 + 0.5).clamp(0.0, 1.0)));
                        }
                        MidiMessage::Aftertouch { key, vel } => ln.poly_at.push((beat, key.as_int(), vel.as_int() as f32 / 127.0)),
                        MidiMessage::ChannelAftertouch { vel } => ln.chan_at.push((beat, vel.as_int() as f32 / 127.0)),
                        MidiMessage::Controller { controller, value } => {
                            let v = value.as_int() as f32;
                            match controller.as_int() {
                                7 => {
                                    st.cc7 = v;
                                    ln.vol.push((beat, vol_norm(st.cc7, st.cc11)));
                                }
                                11 => {
                                    st.cc11 = v;
                                    ln.vol.push((beat, vol_norm(st.cc7, st.cc11)));
                                }
                                10 => ln.pan.push((beat, ((v - 64.0) / 63.0).clamp(-1.0, 1.0))),
                                74 => ln.cc74.push((beat, v / 127.0)),
                                0 => st.bank = value.as_int() as u16,
                                32 => {
                                    // bank LSB only matters when the MSB is unused
                                    if st.bank == 0 {
                                        st.bank = value.as_int() as u16;
                                    }
                                }
                                101 => st.rpn.0 = value.as_int(),
                                100 => st.rpn.1 = value.as_int(),
                                6 => {
                                    // RPN 0,0 = pitch-bend sensitivity in semitones
                                    if st.rpn == (0, 0) {
                                        st.bend_range = v.clamp(1.0, 24.0);
                                    }
                                }
                                64 => {
                                    let down = v >= 64.0;
                                    if st.pedal && !down {
                                        // release everything the pedal was holding
                                        for (key, start, vel, seg) in std::mem::take(&mut st.held) {
                                            let len = (tick.saturating_sub(start) as f64 / tpqn).max(0.05);
                                            segs[seg].notes.push(Note::new(start as f64 / tpqn, len, key, vel as f32 / 127.0));
                                        }
                                    }
                                    st.pedal = down;
                                }
                                _ => {}
                            }
                        }
                        MidiMessage::NoteOn { key, vel } if vel.as_int() > 0 => {
                            let seg = seg_of(st.program, st.bank);
                            st.pending.entry(key.as_int()).or_default().push((tick, vel.as_int(), seg));
                        }
                        MidiMessage::NoteOn { key, .. } | MidiMessage::NoteOff { key, .. } => {
                            let started = st.pending.get_mut(&key.as_int()).and_then(|s| if s.is_empty() { None } else { Some(s.remove(0)) });
                            if let Some((start, vel, seg)) = started {
                                if st.pedal && c != 9 {
                                    st.held.push((key.as_int(), start, vel, seg));
                                } else {
                                    let len = (tick.saturating_sub(start) as f64 / tpqn).max(0.05);
                                    segs[seg].notes.push(Note::new(start as f64 / tpqn, len, key.as_int(), vel as f32 / 127.0));
                                }
                            }
                        }
                        _ => {}
                    }
                }
                _ => {}
            }
        }
        // notes still open at the end of the track (missing note-offs / a pedal never released)
        for (_, st, _) in states.iter_mut() {
            let open: Vec<(u8, u64, u8, usize)> =
                st.pending.iter().flat_map(|(k, v)| v.iter().map(move |(s, vel, seg)| (*k, *s, *vel, *seg))).chain(std::mem::take(&mut st.held)).collect();
            for (key, start, vel, seg) in open {
                let len = (tick.saturating_sub(start) as f64 / tpqn).max(0.05);
                segs[seg].notes.push(Note::new(start as f64 / tpqn, len, key, vel as f32 / 127.0));
            }
        }
        let multi = segs.iter().filter(|s| !s.notes.is_empty()).count() > 1;
        for seg in segs {
            if seg.notes.is_empty() {
                continue;
            }
            let base = match seg.program {
                None => "Drums".to_string(),
                Some(p) => gm_name(p).to_string(),
            };
            // use the file's track name; keep the GM patch visible when a track holds several parts
            let name = if tname.is_empty() {
                base
            } else if multi || tname.eq_ignore_ascii_case(&base) {
                format!("{tname} ({base})")
            } else {
                tname.clone()
            };
            let mut notes = seg.notes;
            notes.sort_by(|a, b| a.start.partial_cmp(&b.start).unwrap_or(std::cmp::Ordering::Equal));
            let first = notes[0].start;
            let ln = states.iter().find(|(c, _, _)| *c == seg.chan).map(|(_, _, l)| l.clone()).unwrap_or_default();
            let at = |pts: &[(f64, f32)], default: f32| pts.iter().filter(|p| p.0 <= first + 1e-6).last().map(|p| p.1).unwrap_or(default);
            let volume = at(&ln.vol, 1.0);
            let pan = at(&ln.pan, 0.0);
            // only build a lane when the controller actually moves
            let vol_auto = thin(ln.vol.clone());
            let pan_auto = thin(ln.pan.clone());
            let bend_range = states.iter().find(|(c, _, _)| *c == seg.chan).map(|(_, st, _)| st.bend_range).unwrap_or(2.0);
            // aftertouch, CC74 and (on a monophonic part) the wheel become per-note expression
            let per_note_bend = seg.program.is_some() && note_expression(&mut notes, &ln, bend_range);
            let bend_auto = if per_note_bend { vec![] } else { thin(ln.bend.clone()) };
            out.push(ImportedTrack {
                named: !tname.is_empty(),
                plugin: String::new(),
                name,
                program: seg.program,
                bank: seg.bank,
                channel: seg.chan,
                notes,
                volume,
                pan,
                vol_auto: if vol_auto.len() > 1 { vol_auto } else { vec![] },
                pan_auto: if pan_auto.len() > 1 { pan_auto } else { vec![] },
                // a wheel that never leaves centre needs no lane
                bend_auto: if bend_auto.iter().any(|p| (p.1 - 0.5).abs() > 0.002) { bend_auto } else { vec![] },
                bend_range,
                samples: vec![],
                sampler: None,
                percussive: false,
                extra_auto: vec![],
            });
        }
    }
    let length_beats = out.iter().flat_map(|t| t.notes.iter()).map(|n| n.start + n.len).fold(0.0, f64::max);
    // tempo map: sorted, consecutive duplicates removed, starting at beat 0
    tempos.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap_or(std::cmp::Ordering::Equal));
    let mut map: Vec<(f64, f64)> = Vec::new();
    for (b, v) in tempos {
        if let Some(last) = map.last_mut() {
            if (last.0 - b).abs() < 1e-6 {
                last.1 = v;
                continue;
            }
            if (last.1 - v).abs() < 0.01 {
                continue;
            }
        }
        map.push((b, v));
    }
    if let Some(f) = map.first().copied() {
        if f.0 > 0.0 {
            map.insert(0, (0.0, f.1));
        }
    }
    let bpm = map.first().map(|f| f.1);
    Ok(ImportedMidi { tracks: out, bpm, tempos: map, length_beats })
}
