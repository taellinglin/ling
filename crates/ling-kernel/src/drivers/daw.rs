//! LingStation for LingOS: the DAW window.
//!
//! A tracker-style song editor on top of the freestanding `ling-osdaw` engine: eight channels, 64 rows, fourteen built-in instruments, a master
//! effect rack (drive / echo / verb / crush), note scripts (arpeggiator, humanize, reverse, euclid, repeat, snap to key, transpose), Mix AI,
//! undo, Standard MIDI File export / import, a sample-accurate sequencer that the mixer pulls once per audio block (`block`), save / load to
//! lingfs, and a WAV export that lands in `music/` where the Media Player finds it. Tools are mouse buttons; `A` cycles the arpeggio style and
//! `F` the euclid density. Keyboard: the two piano rows enter notes (Z S X D C V G B H N J M , / Q 2 W 3 E R 5 T 6 Y 7 U I), arrows
//! move, Space plays, Tab / Shift+Tab change channel, K changes the channel's instrument, L mutes it, O writes a note-off, Backspace clears,
//! `,` `.` change the velocity, `[` `]` the octave, `-` `=` the tempo. The mouse works on every button, cell and channel header.

use crate::drivers::{font8x8, framebuffer, keyboard, mixer, theme};
use crate::fs::lingfs;
use alloc::boxed::Box;
use alloc::format;
use alloc::vec::Vec;
use ling_osdaw::{midi, mix, script, Cell, Engine, Song, CHANNELS, FX_NAMES, INSTRUMENTS, NOTE_OFF};

static mut ENGINE: Option<Box<Engine>> = None;
static mut CUR_ROW: usize = 0;
static mut CUR_CH: usize = 0;
static mut OCTAVE: u8 = 4;
static mut STATUS: [u8; 96] = [0; 96];
static mut STATUS_LEN: usize = 0;
static mut OUT: [i16; 8192] = [0; 8192];
static mut LAST_PLAY_ROW: usize = usize::MAX;

static mut VIS: usize = 16;
static mut UNDO: Vec<Song> = Vec::new();
static mut ARP_I: usize = 0;
static mut EU_I: usize = 0;
const EU_HITS: [usize; 5] = [3, 5, 7, 9, 11];

/// (label, button row): transport, then scripts, then tools, then the effect rack (labels of the last four show their level)
const BUTTONS: [(&[u8], u8); 29] = [
    (b"Play", 0),
    (b"Loop", 0),
    (b"-", 0),
    (b"+", 0),
    (b"Save", 0),
    (b"Load", 0),
    (b"WAV", 0),
    (b"Demo", 0),
    (b"Clear", 0),
    (b"Undo", 1),
    (b"Arp", 1),
    (b"Human", 1),
    (b"Rev", 1),
    (b"Euclid", 1),
    (b"Repeat", 1),
    (b"Snap", 1),
    (b"Xp-", 1),
    (b"Xp+", 1),
    (b"Mix", 2),
    (b"MIDI", 2),
    (b"LoadMid", 2),
    (b"Vol-", 2),
    (b"Vol+", 2),
    (b"Pan<", 2),
    (b"Pan>", 2),
    (b"Drive", 3),
    (b"Echo", 3),
    (b"Verb", 3),
    (b"Crush", 3),
];
const FX_BTN: usize = 25;
const BTN_ROWS: u32 = 4;

fn rows_vis() -> usize {
    unsafe { VIS }
}

/// remember the song before an edit so Undo can bring it back (the last eight)
fn snap() {
    unsafe {
        let u = &mut *&raw mut UNDO;
        if u.len() >= 8 {
            u.remove(0);
        }
        u.push(eng().song.clone());
    }
}

fn pitch_name(root: u8, minor: bool) -> alloc::string::String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{} {}", N[(root % 12) as usize], if minor { "minor" } else { "major" })
}

fn eng() -> &'static mut Engine {
    unsafe {
        let e = &mut *&raw mut ENGINE;
        if e.is_none() {
            let mut en = Box::new(Engine::new(Song::demo()));
            en.master = 0.9;
            *e = Some(en);
        }
        e.as_mut().unwrap()
    }
}

fn set_status(s: &str) {
    unsafe {
        let n = s.len().min(95);
        let st = &mut *&raw mut STATUS;
        st[..n].copy_from_slice(&s.as_bytes()[..n]);
        STATUS_LEN = n;
    }
}

/// The mixer's hook: the next `frames` stereo frames of the song, or None when nothing is sounding (so an idle window costs no CPU).
pub fn block(frames: usize) -> Option<&'static [i16]> {
    unsafe {
        let e = (*&raw mut ENGINE).as_mut()?;
        if !e.playing && e.peak < 0.0008 {
            return None;
        }
        let n = frames.min(4096);
        let out = &mut *&raw mut OUT;
        e.render(&mut out[..n * 2]);
        Some(&out[..n * 2])
    }
}

/// true while the song is playing (the Media Player and the mixer UI can show it)
pub fn playing() -> bool {
    unsafe { (*&raw const ENGINE).as_ref().map(|e| e.playing).unwrap_or(false) }
}

fn wake(e: &mut Engine) {
    e.peak = 1.0;
}

fn toggle_play() {
    let e = eng();
    if e.playing {
        e.stop();
        set_status("stopped");
    } else {
        e.row = unsafe { CUR_ROW };
        e.play();
        wake(e);
        set_status("playing");
    }
}

fn audition(note: u8) {
    let e = eng();
    let ch = unsafe { CUR_CH };
    let vel = e.song.cells[unsafe { CUR_ROW }][ch].vel;
    e.trigger(ch, note, vel);
    wake(e);
}

fn piano_offset(k: u8) -> Option<u8> {
    Some(match k {
        b'z' => 0,
        b's' => 1,
        b'x' => 2,
        b'd' => 3,
        b'c' => 4,
        b'v' => 5,
        b'g' => 6,
        b'b' => 7,
        b'h' => 8,
        b'n' => 9,
        b'j' => 10,
        b'm' => 11,
        b',' => 12,
        b'q' => 12,
        b'2' => 13,
        b'w' => 14,
        b'3' => 15,
        b'e' => 16,
        b'r' => 17,
        b'5' => 18,
        b't' => 19,
        b'6' => 20,
        b'y' => 21,
        b'7' => 22,
        b'u' => 23,
        b'i' => 24,
        _ => return None,
    })
}

fn move_cursor(dr: i32, dc: i32) {
    unsafe {
        let len = eng().song.length as i32;
        CUR_ROW = ((CUR_ROW as i32 + dr).rem_euclid(len)) as usize;
        CUR_CH = ((CUR_CH as i32 + dc).rem_euclid(CHANNELS as i32)) as usize;
    }
}

fn save() {
    let bytes = eng().song.to_bytes();
    match lingfs::write_in_dir("music", "song.lsx", &bytes) {
        Ok(()) => set_status("saved music/song.lsx"),
        Err(()) => set_status("could not save (no writable disk?)"),
    }
}

fn load() {
    let mut buf = [0u8; 2048];
    match lingfs::read_file_all("music/song.lsx", &mut buf) {
        Ok(Some(n)) => match Song::from_bytes(&buf[..n]) {
            Some(s) => {
                let e = eng();
                e.song = s;
                e.all_off();
                set_status("loaded music/song.lsx");
            },
            None => set_status("music/song.lsx is not a LingStation song"),
        },
        _ => set_status("no saved song yet (Save first)"),
    }
}

fn export_wav() {
    let song = eng().song.clone();
    if song.seconds() > 40.0 {
        set_status("song too long to export here (40 s max)");
        return;
    }
    let wav = Engine::render_wav(&song);
    match lingfs::write_in_dir_any("music", "LingStation.wav", &wav) {
        Ok(()) => set_status("wrote music/LingStation.wav (open it in Files)"),
        Err(()) => set_status("could not write the WAV"),
    }
}

fn read_midi() -> Result<Song, alloc::string::String> {
    let mut buf: Vec<u8> = alloc::vec![0u8; 48 * 1024];
    for path in ["music/LingStation.mid", "music/song.mid"] {
        match lingfs::read_file_all(path, &mut buf) {
            Ok(Some(n)) => return midi::from_midi(&buf[..n]).ok_or_else(|| format!("{} is not a usable MIDI file ({} bytes)", path, n)),
            Ok(None) => {},
            Err(()) => return Err(format!("could not read {}", path)),
        }
    }
    Err(alloc::string::String::from("no MIDI file in music/ (export one first)"))
}

fn tool(i: usize) {
    let e = eng();
    let ch = unsafe { CUR_CH };
    match i {
        9 => {
            let prev = unsafe { (*&raw mut UNDO).pop() };
            match prev {
                Some(s) => {
                    e.song = s;
                    e.all_off();
                    set_status("undone");
                },
                None => set_status("nothing to undo"),
            }
        },
        10 => {
            snap();
            let (shape, dir) = unsafe { (ARP_I % 4, (ARP_I / 4) % 3) };
            let n = script::arpeggiate(&mut e.song, ch, shape, dir as u8);
            set_status(&format!("arp {} {}: {} notes (A changes style)", script::ARP_CHORDS[shape], ["up", "down", "up-down"][dir], n));
        },
        11 => {
            snap();
            let n = script::humanize(&mut e.song, ch, 3);
            set_status(&format!("humanize: {} velocities", n));
        },
        12 => {
            snap();
            let n = script::reverse(&mut e.song, ch);
            set_status(&format!("reversed channel {} ({} cells)", ch + 1, n));
        },
        13 => {
            snap();
            let hits = EU_HITS[unsafe { EU_I } % EU_HITS.len()];
            let steps = 16;
            let cur = e.song.cells[unsafe { CUR_ROW }][ch].note;
            let key = if cur > 0 && cur != NOTE_OFF {
                cur
            } else {
                match e.song.chans[ch].inst {
                    5 => 36,
                    6 => 38,
                    7 => 42,
                    12 => 39,
                    13 => 45,
                    _ => 60,
                }
            };
            let n = script::euclid(&mut e.song, ch, hits, steps, key, 12);
            set_status(&format!("euclid {} of {}: {} hits (F changes density)", hits, steps, n));
        },
        14 => {
            snap();
            let n = script::repeat_half(&mut e.song, ch);
            set_status(&format!("repeated the first half ({} notes)", n));
        },
        15 => {
            snap();
            let (root, minor) = script::detect_key(&e.song);
            let n = script::snap_to_scale(&mut e.song, ch, root, minor);
            set_status(&format!("snapped {} notes to {}", n, pitch_name(root, minor)));
        },
        16 | 17 => {
            snap();
            let d = if i == 16 { -1 } else { 1 };
            let n = script::transpose_melodic(&mut e.song, d);
            set_status(&format!("transposed {} notes {}", n, if d < 0 { "down" } else { "up" }));
        },
        18 => {
            snap();
            set_status("mixing...");
            let r = mix::auto_mix(&mut e.song);
            let n = r.roles.iter().filter(|x| **x != mix::Role::Silent).count();
            set_status(&format!("Mix AI: balanced {} channels, peak {}%", n, (r.peak * 100.0) as u32));
        },
        19 => {
            let m = midi::to_midi(&e.song);
            match lingfs::write_in_dir_any("music", "LingStation.mid", &m) {
                Ok(()) => set_status(&format!("wrote music/LingStation.mid ({} bytes)", m.len())),
                Err(()) => set_status("could not write the MIDI file"),
            }
        },
        20 => match read_midi() {
            Ok(s) => {
                snap();
                let l = e.looping;
                e.song = s;
                e.looping = l;
                e.all_off();
                set_status("imported the MIDI file from music/");
            },
            Err(m) => set_status(&m),
        },
        21 | 22 => {
            let c = &mut e.song.chans[ch];
            c.vol = (c.vol as i32 + if i == 21 { -8 } else { 8 }).clamp(0, 100) as u8;
            set_status(&format!("channel {} volume {}", ch + 1, c.vol));
        },
        23 | 24 => {
            let c = &mut e.song.chans[ch];
            c.pan = (c.pan as i32 + if i == 23 { -10 } else { 10 }).clamp(-100, 100) as i8;
            set_status(&format!("channel {} pan {}", ch + 1, c.pan));
        },
        _ => {
            let k = i - FX_BTN;
            let l = &mut e.song.fx[k];
            *l = if *l >= 10 { 0 } else { *l + 2 };
            let (name, lv) = (FX_NAMES[k], *l);
            set_status(&format!("{} {}", name, lv));
            wake(e);
        },
    }
}

fn button(i: usize) {
    let e = eng();
    if i >= 9 {
        return tool(i);
    }
    match i {
        0 => toggle_play(),
        1 => {
            e.looping = !e.looping;
            set_status(if e.looping { "loop on" } else { "loop off" });
        },
        2 => e.song.bpm = e.song.bpm.saturating_sub(2).max(50),
        3 => e.song.bpm = (e.song.bpm + 2).min(220),
        4 => save(),
        5 => {
            snap();
            load()
        },
        6 => export_wav(),
        7 => {
            snap();
            let l = e.looping;
            e.song = Song::demo();
            e.looping = l;
            e.all_off();
            set_status("the demo groove");
        },
        _ => {
            snap();
            e.song = Song::empty();
            e.all_off();
            set_status("empty song");
        },
    }
}

pub fn key(k: u8) {
    let e = eng();
    unsafe {
        match k {
            b' ' => toggle_play(),
            keyboard::UP_ARROW => move_cursor(-1, 0),
            keyboard::DOWN_ARROW => move_cursor(1, 0),
            keyboard::LEFT_ARROW => move_cursor(0, -1),
            keyboard::RIGHT_ARROW => move_cursor(0, 1),
            keyboard::PGUP_KEY => move_cursor(-(rows_vis() as i32), 0),
            keyboard::PGDN_KEY => move_cursor(rows_vis() as i32, 0),
            keyboard::HOME_KEY => CUR_ROW = 0,
            9 => move_cursor(0, if keyboard::shift_down() { -1 } else { 1 }),
            b'1' => tool(9),
            b'4' => tool(10),
            b'8' => tool(11),
            b'9' => tool(12),
            b'0' => tool(13),
            b';' => tool(14),
            39 => tool(15), // '
            b'`' => tool(18),
            92 => tool(19), // backslash
            b'/' => tool(20),
            b'a' | b'A' => {
                ARP_I = (ARP_I + 1) % 12;
                set_status(&format!("arp style: {} {}", script::ARP_CHORDS[ARP_I % 4], ["up", "down", "up-down"][(ARP_I / 4) % 3]));
            },
            b'f' | b'F' => {
                EU_I = (EU_I + 1) % EU_HITS.len();
                set_status(&format!("euclid density: {} of 16", EU_HITS[EU_I]));
            },
            8 | 127 => {
                snap();
                e.song.cells[CUR_ROW][CUR_CH] = Cell::EMPTY;
                move_cursor(1, 0);
            },
            b'o' | b'O' => {
                snap();
                e.song.cells[CUR_ROW][CUR_CH] = Cell { note: NOTE_OFF, vel: 0 };
                move_cursor(1, 0);
            },
            b'k' | b'K' => {
                let c = &mut e.song.chans[CUR_CH];
                c.inst = (c.inst + 1) % INSTRUMENTS.len() as u8;
                let name = INSTRUMENTS[c.inst as usize];
                e.all_off();
                set_status(name);
            },
            b'l' | b'L' => {
                let c = &mut e.song.chans[CUR_CH];
                c.mute = !c.mute;
                set_status(if c.mute { "channel muted" } else { "channel on" });
            },
            b'.' | b'>' => {
                let c = &mut e.song.cells[CUR_ROW][CUR_CH];
                if c.note > 0 && c.note != NOTE_OFF {
                    c.vel = (if c.vel == 0 { 11 } else { c.vel } + 1).min(15);
                }
            },
            b'<' => {
                let c = &mut e.song.cells[CUR_ROW][CUR_CH];
                if c.note > 0 && c.note != NOTE_OFF {
                    c.vel = (if c.vel == 0 { 11 } else { c.vel }).saturating_sub(1).max(1);
                }
            },
            b'[' => OCTAVE = OCTAVE.saturating_sub(1).max(1),
            b']' => OCTAVE = (OCTAVE + 1).min(8),
            b'-' => button(2),
            b'=' | b'+' => button(3),
            b'p' | b'P' => toggle_play(),
            _ => {
                if let Some(off) = piano_offset(k) {
                    let note = (12 * (OCTAVE as u32 + 1) + off as u32).min(127) as u8;
                    let vel = e.song.cells[CUR_ROW][CUR_CH].vel;
                    snap();
                    e.song.cells[CUR_ROW][CUR_CH] = Cell { note, vel: if vel == 0 { 11 } else { vel } };
                    audition(note);
                    move_cursor(1, 0);
                }
            },
        }
    }
}

// ── layout shared by drawing and the mouse ────────────────────────────────────

struct Lay {
    adv: u32,
    rowh: u32,
    /// height of the four button rows
    br: u32,
    grid_y: u32,
    rn_w: u32,
    cell_w: u32,
}

/// text scale: the 8x8 font, doubled on a big screen so the grid is readable
fn ts() -> u32 {
    (framebuffer::height() / 540).max(font8x8::scale()).max(1)
}

fn dtext(x: u32, y: u32, s: &[u8], fg: u32, bg: u32) {
    let sc = ts();
    for (i, c) in s.iter().enumerate() {
        font8x8::draw_char_scaled(x + i as u32 * 8 * sc, y, *c, fg, bg, sc);
    }
}

fn lay(y: u32) -> Lay {
    let adv = 8 * ts();
    let rowh = 8 * ts() + 3 * ts();
    let br = BTN_ROWS * (rowh + 8);
    Lay { adv, rowh, br, grid_y: y + br + 2 * rowh + 12, rn_w: adv * 3 + 6, cell_w: adv * 7 + 4 }
}

/// the text on a button; the effect knobs show their level
fn label(i: usize, buf: &mut [u8; 12]) -> usize {
    let t = BUTTONS[i].0;
    let mut n = t.len().min(8);
    buf[..n].copy_from_slice(&t[..n]);
    if i >= FX_BTN {
        let lv = eng().song.fx[i - FX_BTN];
        buf[n] = b' ';
        n += 1;
        if lv >= 10 {
            buf[n] = b'1';
            buf[n + 1] = b'0';
            n += 2;
        } else {
            buf[n] = b'0' + lv;
            n += 1;
        }
    }
    n
}

fn button_rect(x: u32, y: u32, i: usize) -> (u32, u32, u32, u32) {
    let l = lay(y);
    let row = BUTTONS[i].1;
    let mut bx = x;
    for b in 0..i {
        if BUTTONS[b].1 == row {
            let mut lb = [0u8; 12];
            bx += (label(b, &mut lb) as u32 + 2) * l.adv + 6;
        }
    }
    let mut lb = [0u8; 12];
    let n = label(i, &mut lb);
    (bx, y + row as u32 * (l.rowh + 8), (n as u32 + 2) * l.adv, l.rowh + 4)
}

fn inside(mx: i64, my: i64, r: (u32, u32, u32, u32)) -> bool {
    mx >= r.0 as i64 && mx < (r.0 + r.2) as i64 && my >= r.1 as i64 && my < (r.1 + r.3) as i64
}

/// A click inside the window body; (x, y) is the content origin `draw` was given.
pub fn click(mx: i64, my: i64, x: u32, y: u32) -> bool {
    for i in 0..BUTTONS.len() {
        if inside(mx, my, button_rect(x, y, i)) {
            button(i);
            mixer::jingle(mixer::EVENT_CLICK);
            return true;
        }
    }
    let l = lay(y);
    let page = unsafe { CUR_ROW } / rows_vis();
    // channel headers cycle the instrument (right half) or toggle mute (left half)
    let hy = l.grid_y - l.rowh - 2;
    if my >= hy as i64 && my < (hy + l.rowh) as i64 {
        for c in 0..CHANNELS {
            let cx = x + l.rn_w + c as u32 * l.cell_w;
            if mx >= cx as i64 && mx < (cx + l.cell_w) as i64 {
                let e = eng();
                unsafe { CUR_CH = c };
                if mx < (cx + l.cell_w / 2) as i64 {
                    e.song.chans[c].mute = !e.song.chans[c].mute;
                } else {
                    e.song.chans[c].inst = (e.song.chans[c].inst + 1) % INSTRUMENTS.len() as u8;
                    e.all_off();
                    set_status(INSTRUMENTS[e.song.chans[c].inst as usize]);
                }
                return true;
            }
        }
    }
    // grid cells
    for r in 0..rows_vis() {
        let row = page * rows_vis() + r;
        if row >= eng().song.length as usize {
            break;
        }
        let ry = l.grid_y + r as u32 * l.rowh;
        if my >= ry as i64 && my < (ry + l.rowh) as i64 {
            for c in 0..CHANNELS {
                let cx = x + l.rn_w + c as u32 * l.cell_w;
                if mx >= cx as i64 && mx < (cx + l.cell_w) as i64 {
                    unsafe {
                        CUR_ROW = row;
                        CUR_CH = c;
                    }
                    return true;
                }
            }
            // the row number: move the play position
            if mx < (x + l.rn_w) as i64 {
                unsafe { CUR_ROW = row };
                let e = eng();
                e.seek(row);
                return true;
            }
        }
    }
    false
}

fn put_u32(buf: &mut [u8], mut n: usize, v: u32, width: usize) -> usize {
    let mut digits = [0u8; 10];
    let mut i = 0;
    let mut x = v;
    loop {
        digits[i] = b'0' + (x % 10) as u8;
        i += 1;
        x /= 10;
        if x == 0 {
            break;
        }
    }
    for _ in i..width {
        buf[n] = b' ';
        n += 1;
    }
    while i > 0 {
        i -= 1;
        buf[n] = digits[i];
        n += 1;
    }
    n
}

fn put_str(buf: &mut [u8], n: usize, s: &[u8]) -> usize {
    let k = s.len().min(buf.len() - n);
    buf[n..n + k].copy_from_slice(&s[..k]);
    n + k
}

pub fn draw(x: u32, y: u32, w: u32, h: u32) {
    let e = eng();
    let panel = theme::color(theme::SLOT_PANEL);
    let border = theme::color(theme::SLOT_PANEL_BORDER);
    let text = theme::color(theme::SLOT_TEXT);
    let dim = theme::color(theme::SLOT_DIM);
    let accent = theme::color(theme::SLOT_ACCENT);
    let beat = theme::color(theme::SLOT_DOT_DIM);
    let l = lay(y);
    // as many grid rows as the window has room for (two help lines below when they fit)
    let room = h.saturating_sub(l.grid_y - y) / l.rowh.max(1);
    unsafe { VIS = (room.saturating_sub(2) as usize).clamp(6, 16) };
    // the buttons: transport, scripts, tools, effect rack
    for i in 0..BUTTONS.len() {
        let r = button_rect(x, y, i);
        let on = (i == 0 && e.playing) || (i == 1 && e.looping) || (i >= FX_BTN && e.song.fx[i - FX_BTN] > 0);
        framebuffer::back_fill_rounded_rect(r.0, r.1, r.2, r.3, 4, if on { accent } else { border });
        let mut lb = [0u8; 12];
        let n = label(i, &mut lb);
        dtext(r.0 + l.adv, r.1 + 3, &lb[..n], if on { panel } else { text }, if on { accent } else { border });
    }
    // the state line
    let mut line = [0u8; 120];
    let mut n = 0;
    n = put_str(&mut line, n, b"BPM ");
    n = put_u32(&mut line, n, e.song.bpm as u32, 0);
    n = put_str(&mut line, n, b"  oct ");
    n = put_u32(&mut line, n, unsafe { OCTAVE } as u32, 0);
    n = put_str(&mut line, n, b"  row ");
    n = put_u32(&mut line, n, unsafe { CUR_ROW } as u32, 2);
    n = put_str(&mut line, n, b"/");
    n = put_u32(&mut line, n, e.song.length as u32, 0);
    n = put_str(&mut line, n, b"  ch ");
    n = put_u32(&mut line, n, unsafe { CUR_CH } as u32 + 1, 0);
    n = put_str(&mut line, n, b" ");
    n = put_str(&mut line, n, INSTRUMENTS[e.song.chans[unsafe { CUR_CH }].inst as usize].as_bytes());
    dtext(x, y + l.br, &line[..n], text, panel);
    let st: &[u8] = unsafe {
        let all = &*&raw const STATUS;
        &all[..STATUS_LEN]
    };
    if !st.is_empty() {
        dtext(x + w.saturating_sub((st.len() as u32 + 1) * l.adv), y + l.br, st, accent, panel);
    }
    // meter
    let m = ((e.peak.min(1.0)) * (w as f32 * 0.25)) as u32;
    framebuffer::back_fill_rect(x, y + l.br + l.rowh + 3, w / 4, 3, beat);
    framebuffer::back_fill_rect(x, y + l.br + l.rowh + 3, m, 3, accent);
    // channel headers
    let hy = l.grid_y - l.rowh - 2;
    for c in 0..CHANNELS {
        let cx = x + l.rn_w + c as u32 * l.cell_w;
        let ch = e.song.chans[c];
        let cur = c == unsafe { CUR_CH };
        let mut hb = [0u8; 8];
        let name = INSTRUMENTS[ch.inst as usize].as_bytes();
        let k = name.len().min(7);
        hb[..k].copy_from_slice(&name[..k]);
        let colr = if ch.mute { dim } else if cur { accent } else { text };
        dtext(cx, hy, &hb[..k], colr, panel);
        if ch.mute {
            framebuffer::back_fill_rect(cx, hy + 3 * ts(), (k as u32) * l.adv, ts(), dim);
        }
    }
    // the grid: one page of 16 rows (the page follows the cursor and the playhead while playing)
    let follow = if e.playing { e.row } else { unsafe { CUR_ROW } };
    let page = follow / rows_vis();
    for r in 0..rows_vis() {
        let row = page * rows_vis() + r;
        if row >= e.song.length as usize {
            break;
        }
        let ry = l.grid_y + r as u32 * l.rowh;
        let playing_row = e.playing && row == e.row;
        let rowbg = if playing_row { border } else if row % (e.song.rpb.max(1) as usize) == 0 { beat } else { panel };
        framebuffer::back_fill_rect(x, ry, l.rn_w + l.cell_w * CHANNELS as u32, l.rowh, rowbg);
        let mut rb = [0u8; 3];
        let rn = put_u32(&mut rb, 0, row as u32, 2);
        dtext(x + 2, ry + 1, &rb[..rn], if playing_row { accent } else { dim }, rowbg);
        for c in 0..CHANNELS {
            let cx = x + l.rn_w + c as u32 * l.cell_w;
            let cell = e.song.cells[row][c];
            let (nt, v) = Song::cell_text(cell);
            let is_cur = row == unsafe { CUR_ROW } && c == unsafe { CUR_CH };
            let (fg, bg) = if is_cur { (panel, accent) } else if cell.note > 0 { (text, rowbg) } else { (dim, rowbg) };
            if is_cur {
                framebuffer::back_fill_rect(cx, ry, l.cell_w - 2, l.rowh, accent);
            }
            let mut cb = [b' '; 5];
            cb[..3].copy_from_slice(&nt);
            cb[4] = v;
            dtext(cx + 1, ry + 1, &cb, fg, bg);
        }
    }
    // help
    let hy = l.grid_y + rows_vis() as u32 * l.rowh + 8;
    if h > hy.saturating_sub(y) + 2 * l.rowh {
        dtext(x, hy, b"piano Z S X D C V G B H N J M  [ ] octave  Space play  A arp style  F euclid density  Tab channel", dim, panel);
        dtext(x, hy + l.rowh, b"1 undo  4 arp  8 human  9 rev  0 euclid  ; repeat  ' snap  ` Mix AI  \\ MIDI out  / MIDI in", dim, panel);
    }
    // follow-up: keep the cursor on the playhead page when stopped is the user's own
    unsafe {
        if e.playing && e.row != LAST_PLAY_ROW {
            LAST_PLAY_ROW = e.row;
        }
    }
}
