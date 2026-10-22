//! Standard MIDI Files in and out, so a LingStation song moves to and from every other DAW (LingStationX's MIDI import / export at kernel scale).
//! Export writes a format-1 file, one track per channel that has notes. Import reads format 0 / 1, quantizes to sixteenths and maps General MIDI
//! programs and the drum channel onto the built-in instruments.
use crate::{Cell, Song, CHANNELS, MAX_ROWS, NOTE_OFF};
use alloc::vec::Vec;

const DIV: u32 = 480;
/// the General MIDI program each instrument stands for (drums use channel 10 and ignore it)
const PROG: [u8; 14] = [81, 38, 45, 89, 14, 0, 0, 0, 80, 87, 54, 19, 0, 0];

fn is_drum(inst: u8) -> bool {
    matches!(inst, 5 | 6 | 7 | 12 | 13)
}

fn vlq(out: &mut Vec<u8>, mut v: u32) {
    let mut st = [0u8; 5];
    let mut n = 0;
    loop {
        st[n] = (v & 0x7F) as u8;
        n += 1;
        v >>= 7;
        if v == 0 {
            break;
        }
    }
    while n > 0 {
        n -= 1;
        out.push(st[n] | if n > 0 { 0x80 } else { 0 });
    }
}

fn track(body: Vec<u8>, out: &mut Vec<u8>) {
    out.extend_from_slice(b"MTrk");
    out.extend_from_slice(&((body.len() + 4) as u32).to_be_bytes());
    out.extend_from_slice(&body);
    out.extend_from_slice(&[0, 0xFF, 0x2F, 0]);
}

pub fn to_midi(s: &Song) -> Vec<u8> {
    let tpr = DIV / s.rpb.max(1) as u32;
    let len = s.length as usize;
    let mut tracks: Vec<Vec<u8>> = Vec::new();
    let mut t0 = Vec::new();
    vlq(&mut t0, 0);
    t0.extend_from_slice(&[0xFF, 0x51, 3]);
    let us = 60_000_000u32 / s.bpm.max(20) as u32;
    t0.extend_from_slice(&us.to_be_bytes()[1..]);
    tracks.push(t0);
    let mut next_melodic = 0u8;
    for c in 0..CHANNELS {
        if !(0..len).any(|r| s.cells[r][c].note > 0 && s.cells[r][c].note != NOTE_OFF) {
            continue;
        }
        let inst = s.chans[c].inst;
        let mch = if is_drum(inst) {
            9
        } else {
            let m = next_melodic;
            next_melodic = (next_melodic + 1) % 15;
            if next_melodic == 9 {
                next_melodic = 10;
            }
            m
        };
        // (tick, off-first order, status/data bytes)
        let mut ev: Vec<(u32, u8, [u8; 3])> = Vec::new();
        let mut held: Option<u8> = None;
        for r in 0..len {
            let cell = s.cells[r][c];
            let tick = r as u32 * tpr;
            if cell.note == 0 {
                continue;
            }
            if let Some(k) = held.take() {
                ev.push((tick, 0, [0x80 | mch, k, 0]));
            }
            if cell.note != NOTE_OFF {
                let v = (if cell.vel == 0 { 11 } else { cell.vel } as u32 * 8).min(127) as u8;
                ev.push((tick, 1, [0x90 | mch, cell.note.min(127), v]));
                // drums are one-shots: close them a row later
                held = Some(cell.note.min(127));
            }
        }
        if let Some(k) = held {
            ev.push((len as u32 * tpr, 0, [0x80 | mch, k, 0]));
        }
        ev.sort_by_key(|e| (e.0, e.1));
        let mut body = Vec::new();
        vlq(&mut body, 0);
        body.push(0xC0 | mch);
        body.push(PROG[(inst as usize).min(PROG.len() - 1)]);
        let mut last = 0u32;
        for (t, _, d) in ev {
            vlq(&mut body, t - last);
            last = t;
            body.extend_from_slice(&d);
        }
        tracks.push(body);
    }
    let mut out = Vec::new();
    out.extend_from_slice(b"MThd");
    out.extend_from_slice(&6u32.to_be_bytes());
    out.extend_from_slice(&1u16.to_be_bytes());
    out.extend_from_slice(&(tracks.len() as u16).to_be_bytes());
    out.extend_from_slice(&(DIV as u16).to_be_bytes());
    for t in tracks {
        track(t, &mut out);
    }
    out
}

struct Ev {
    tick: u64,
    ch: u8,
    key: u8,
    vel: u8,
    on: bool,
}

fn rd_vlq(b: &[u8], p: &mut usize) -> Option<u32> {
    let mut v = 0u32;
    for _ in 0..4 {
        let x = *b.get(*p)?;
        *p += 1;
        v = (v << 7) | (x & 0x7F) as u32;
        if x & 0x80 == 0 {
            return Some(v);
        }
    }
    None
}

/// Read a Standard MIDI File into a song. None if it is not one. Never panics on hostile bytes.
pub fn from_midi(b: &[u8]) -> Option<Song> {
    if b.len() < 14 || &b[0..4] != b"MThd" {
        return None;
    }
    let hlen = u32::from_be_bytes([b[4], b[5], b[6], b[7]]) as usize;
    let ntr = u16::from_be_bytes([b[10], b[11]]) as usize;
    let div = u16::from_be_bytes([b[12], b[13]]) as u32;
    if hlen < 6 || div == 0 || div & 0x8000 != 0 {
        return None;
    }
    let mut p = 8 + hlen;
    let mut evs: Vec<Ev> = Vec::new();
    let mut prog = [255u8; 16];
    let mut tempo_us = 0u32;
    for _ in 0..ntr.min(64) {
        if p + 8 > b.len() || &b[p..p + 4] != b"MTrk" {
            break;
        }
        let tl = u32::from_be_bytes([b[p + 4], b[p + 5], b[p + 6], b[p + 7]]) as usize;
        let end = (p + 8).saturating_add(tl).min(b.len());
        let mut q = p + 8;
        p = end;
        let mut tick = 0u64;
        let mut run = 0u8;
        while q < end && evs.len() < 40_000 {
            let Some(d) = rd_vlq(b, &mut q) else { break };
            tick += d as u64;
            let Some(&first) = b.get(q) else { break };
            let st = if first & 0x80 != 0 {
                q += 1;
                if first < 0xF0 {
                    run = first;
                }
                first
            } else {
                run
            };
            if st == 0xFF {
                let Some(&ty) = b.get(q) else { break };
                q += 1;
                let Some(l) = rd_vlq(b, &mut q) else { break };
                if ty == 0x51 && l == 3 && tempo_us == 0 && q + 3 <= b.len() {
                    tempo_us = ((b[q] as u32) << 16) | ((b[q + 1] as u32) << 8) | b[q + 2] as u32;
                }
                q = q.saturating_add(l as usize);
            } else if st == 0xF0 || st == 0xF7 {
                let Some(l) = rd_vlq(b, &mut q) else { break };
                q = q.saturating_add(l as usize);
            } else if st >= 0x80 {
                let two = !matches!(st & 0xF0, 0xC0 | 0xD0);
                let need = if two { 2 } else { 1 };
                if q + need > b.len() {
                    break;
                }
                let (d1, d2) = (b[q], if two { b[q + 1] } else { 0 });
                q += need;
                let ch = st & 0x0F;
                match st & 0xF0 {
                    0x90 if d2 > 0 => evs.push(Ev { tick, ch, key: d1 & 0x7F, vel: d2, on: true }),
                    0x90 | 0x80 => evs.push(Ev { tick, ch, key: d1 & 0x7F, vel: 0, on: false }),
                    0xC0 if prog[ch as usize] == 255 => prog[ch as usize] = d1 & 0x7F,
                    _ => {},
                }
            } else {
                break;
            }
        }
    }
    if !evs.iter().any(|e| e.on) {
        return None;
    }
    evs.sort_by_key(|e| e.tick);
    let mut s = Song::empty();
    for c in s.chans.iter_mut() {
        c.vol = 80;
        c.pan = 0;
        c.mute = false;
    }
    s.bpm = if tempo_us > 0 { (60_000_000 / tempo_us).clamp(40, 240) as u16 } else { 120 };
    s.rpb = 4;
    let tpr = (div / 4).max(1) as u64;
    let row_of = |t: u64| ((t + tpr / 2) / tpr) as usize;
    let last_on = evs.iter().filter(|e| e.on).map(|e| row_of(e.tick)).max().unwrap_or(0);
    let length = (last_on + 8).div_ceil(8) * 8;
    s.length = length.clamp(8, MAX_ROWS) as u8;
    let len = s.length as usize;

    // lane assignment: drums first (kick / snare / hat), then melodic MIDI channels in order of appearance
    let mut lane_of_mch = [255u8; 16];
    let mut drum_lane = [255u8; 3];
    let mut free = 0usize;
    let mut take = |free: &mut usize| -> Option<usize> {
        if *free < CHANNELS {
            *free += 1;
            Some(*free - 1)
        } else {
            None
        }
    };
    for e in evs.iter().filter(|e| e.on) {
        let m = e.ch as usize;
        if m == 9 {
            let k = match e.key {
                35 | 36 => 0,
                37 | 38 | 39 | 40 => 1,
                _ => 2,
            };
            if drum_lane[k] == 255 {
                if let Some(l) = take(&mut free) {
                    drum_lane[k] = l as u8;
                    s.chans[l].inst = [5, 6, 7][k];
                }
            }
        } else if lane_of_mch[m] == 255 {
            if let Some(l) = take(&mut free) {
                lane_of_mch[m] = l as u8;
                let pr = prog[m];
                s.chans[l].inst = match pr {
                    32..=39 => 1,
                    16..=23 => 11,
                    40..=55 | 88..=95 => 3,
                    80..=87 | 56..=79 => 0,
                    8..=15 => 4,
                    24..=31 | 0..=7 => 2,
                    _ => {
                        if e.key < 48 {
                            1
                        } else {
                            0
                        }
                    },
                };
            }
        }
    }
    for e in &evs {
        let r = row_of(e.tick);
        if r >= len {
            continue;
        }
        let lane = if e.ch == 9 {
            if !e.on {
                continue;
            }
            let k = match e.key {
                35 | 36 => 0,
                37 | 38 | 39 | 40 => 1,
                _ => 2,
            };
            drum_lane[k]
        } else {
            lane_of_mch[e.ch as usize]
        };
        if lane == 255 {
            continue;
        }
        let cell = &mut s.cells[r][lane as usize];
        if e.on {
            let key = if e.ch == 9 {
                match e.key {
                    35 | 36 => 36,
                    37..=40 => 38,
                    46 | 49 | 51 => 46, // open hat: the engine opens on the D and A# keys
                    _ => 42,
                }
            } else {
                e.key
            };
            if cell.note == 0 || cell.note == NOTE_OFF {
                *cell = Cell { note: key.max(1), vel: (e.vel / 8).clamp(1, 15) };
            }
        } else if cell.note == 0 {
            *cell = Cell { note: NOTE_OFF, vel: 0 };
        }
    }
    Some(s)
}
