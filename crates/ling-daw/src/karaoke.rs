//! Karaoke export: the words of the song with exact timings, as subtitles and as a player.
//!
//! The syllables come from the lyric clips (the text typed or fitted in the lyrics panel, spread over the notes by `lyricfit`), so a held
//! note is one long syllable and a chord is one syllable, with the timings read from the notes through the tempo map. They are grouped into
//! lines at the rests and written as:
//!
//! * `.ass` - Advanced SubStation Alpha with per-syllable karaoke tags (sweep / pop / outline fill), a styled font, colours, outline thickness
//!   and opacity, a count-in of dots before the first line and after long rests, and lines laid out in alternating slots;
//! * `.srt` (one entry per line) and `_karaoke.srt` (one entry per syllable, the sung part coloured with `<font color>`) - for any player;
//! * `.lrc` - enhanced LRC with a time tag in front of every syllable;
//! * `.html` - a self-contained player: the words with a cursor (sweep, pop, ball, underline, block, arrow), a scrolling pitch ribbon of the
//!   melody (singing-game style), the same options live in a panel, Daemon / Chesilin / any font embedded;
//! * an `.mp4` (ffmpeg, if installed) with the ASS burned onto a background over the audio.
use crate::lyricfit::{self, NoteIn, Opts};
use crate::model::*;

#[derive(Clone, Debug)]
pub struct Syl {
    pub s: f64,
    pub e: f64,
    pub text: String,
    pub ws: bool,
    pub key: u8,
    pub track: usize,
    /// the lyrics start a new line here
    pub brk: bool,
    /// the same span in beats (for the sheet music)
    pub b0: f64,
    pub b1: f64,
}

#[derive(Clone, Debug, Default)]
pub struct Line {
    pub syls: Vec<Syl>,
}
impl Line {
    pub fn start(&self) -> f64 {
        self.syls.first().map(|s| s.s).unwrap_or(0.0)
    }
    pub fn end(&self) -> f64 {
        self.syls.last().map(|s| s.e).unwrap_or(0.0)
    }
    pub fn text(&self) -> String {
        let mut t = String::new();
        for (i, s) in self.syls.iter().enumerate() {
            if s.ws && i > 0 {
                t.push(' ');
            }
            t.push_str(&s.text);
        }
        t
    }
}

pub struct Song {
    pub lines: Vec<Line>,
    /// the melody for the ribbon: (start s, end s, key, track)
    pub notes: Vec<(f64, f64, u8, usize)>,
    pub title: String,
    pub bpm: f64,
    pub length: f64,
}

// ── reading the project ───────────────────────────────────────────────────────

struct Ev {
    s: f64,
    e: f64,
    key: u8,
    text: String,
}

/// tracks that carry lyrics, with the number of lyric notes
pub fn lyric_tracks(p: &Project) -> Vec<(usize, usize)> {
    let mut out = vec![];
    for (i, t) in p.tracks.iter().enumerate() {
        let mut n = 0;
        for c in &t.clips {
            if let ClipData::Midi(notes) = &c.data {
                if !c.lyric_text.is_empty() {
                    n += notes.len();
                } else {
                    n += notes.iter().filter(|x| !x.lyric.is_empty() && x.lyric != "~").count();
                }
            }
        }
        if n > 0 {
            out.push((i, n));
        }
    }
    out
}

fn clip_events(c: &Clip) -> Vec<Ev> {
    let ClipData::Midi(notes) = &c.data else { return vec![] };
    let texts: Vec<String> = if !c.lyric_text.is_empty() {
        let opts = Opts::parse(&c.lyric_lang).unwrap_or_default();
        let input: Vec<NoteIn> = notes.iter().map(|n| NoteIn { start: n.start, len: n.len, key: n.key }).collect();
        lyricfit::fit_text(&input, &c.lyric_text, &opts)
    } else {
        notes.iter().map(|n| if n.lyric.is_empty() || n.lyric == "~" { n.lyric.clone() } else { format!(" {}", n.lyric.trim_start_matches('~')) }).collect()
    };
    let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
    let reps = if ll > 0.0 { ((c.len / ll).ceil() as usize).min(512) } else { 1 };
    let mut out = vec![];
    for k in 0..reps {
        let base = c.start + k as f64 * ll;
        let limit = if ll > 0.0 { ll.min(c.len - k as f64 * ll) } else { c.len };
        for (n, t) in notes.iter().zip(texts.iter()) {
            if n.start >= limit {
                continue;
            }
            out.push(Ev { s: base + n.start, e: base + n.start + n.len.min(limit - n.start).max(0.05), key: n.key, text: t.clone() });
        }
    }
    out
}

/// Gather the syllables of `tracks` (all lyric tracks when empty) into lines.
pub fn collect(p: &Project, tracks: &[usize]) -> Song {
    let use_tracks: Vec<usize> = if tracks.is_empty() { lyric_tracks(p).into_iter().map(|x| x.0).collect() } else { tracks.to_vec() };
    let mut syls: Vec<Syl> = vec![];
    let mut ribbon: Vec<(f64, f64, u8, usize)> = vec![];
    for &ti in &use_tracks {
        let Some(t) = p.tracks.get(ti) else { continue };
        let mut evs: Vec<Ev> = t.clips.iter().flat_map(clip_events).collect();
        evs.sort_by(|a, b| a.s.total_cmp(&b.s).then(b.key.cmp(&a.key)));
        // a chord is one thing: the top note carries it, text from whichever note has some
        let mut merged: Vec<Ev> = vec![];
        for ev in evs {
            match merged.last_mut() {
                Some(m) if (m.s - ev.s).abs() < 1e-6 => {
                    if m.text.trim().is_empty() && !ev.text.trim().is_empty() {
                        m.text = ev.text.clone();
                    }
                    m.e = m.e.max(ev.e);
                }
                _ => merged.push(ev),
            }
        }
        let mut last_syl: Option<usize> = None;
        let mut last_end = f64::MIN;
        for ev in &merged {
            let (sb, eb) = (p.beats_to_secs(0.0, ev.s), p.beats_to_secs(0.0, ev.e));
            let tie = ev.text.starts_with('~');
            let body = ev.text.trim_start_matches('~');
            // a sustain lengthens the syllable before it
            if (tie || ev.text.is_empty() && false) && last_syl.is_some() && sb - last_end < 0.6 {
                if let Some(i) = last_syl {
                    syls[i].e = syls[i].e.max(eb);
                }
                ribbon.push((sb, eb, ev.key, ti));
                last_end = last_end.max(eb);
                if body.trim().is_empty() {
                    continue;
                }
            }
            if body.trim().is_empty() {
                ribbon.push((sb, eb, ev.key, ti));
                continue;
            }
            let units: Vec<&str> = body.split('-').filter(|u| !u.trim().is_empty()).collect();
            let n = units.len().max(1) as f64;
            for (k, u) in units.iter().enumerate() {
                let s = sb + (eb - sb) * k as f64 / n;
                let e = sb + (eb - sb) * (k + 1) as f64 / n;
                let (bs, be) = (ev.s + (ev.e - ev.s) * k as f64 / n, ev.s + (ev.e - ev.s) * (k + 1) as f64 / n);
                last_syl = Some(syls.len());
                let brk = u.contains('\n');
                let u = u.replace('\n', "");
                syls.push(Syl { s, e, text: u.trim().to_string(), ws: u.starts_with(' ') || (k == 0 && syls.is_empty()), key: ev.key, track: ti, brk, b0: bs, b1: be });
            }
            ribbon.push((sb, eb, ev.key, ti));
            last_end = eb;
        }
    }
    syls.sort_by(|a, b| a.s.total_cmp(&b.s));
    ribbon.sort_by(|a, b| a.0.total_cmp(&b.0));
    // lines: a rest of more than 1.3 s, or a long line at a word boundary
    let mut lines: Vec<Line> = vec![];
    let mut cur = Line::default();
    let mut chars = 0usize;
    for s in syls {
        let gap = cur.syls.last().map(|l| s.s - l.e).unwrap_or(0.0);
        let too_long = chars > 40 && s.ws;
        if !cur.syls.is_empty() && (gap > 1.3 || too_long || s.brk) {
            lines.push(std::mem::take(&mut cur));
            chars = 0;
        }
        chars += s.text.chars().count() + 1;
        cur.syls.push(s);
    }
    if !cur.syls.is_empty() {
        lines.push(cur);
    }
    let length = p.beats_to_secs(0.0, p.end_beat());
    Song { lines, notes: ribbon, title: p.name.clone(), bpm: p.bpm, length }
}

/// the syllables of one track as (start beat, end beat, text, word start), for the sheet music
pub fn track_syllables(p: &Project, ti: usize) -> Vec<(f64, f64, String, bool)> {
    collect(p, &[ti]).lines.iter().flat_map(|l| l.syls.iter().map(|s| (s.b0, s.b1, s.text.clone(), s.ws))).collect()
}

// ── options ───────────────────────────────────────────────────────────────────

#[derive(Clone, Debug, PartialEq)]
pub enum Cursor {
    Sweep,
    Pop,
    Fill,
    Ball,
    Underline,
    Block,
    Arrow,
    None,
}

#[derive(Clone, Debug)]
pub struct Style {
    /// a family name, or empty for the default
    pub font: String,
    /// a font file to embed / use (Daemon, Chesilin or any .ttf / .otf)
    pub font_file: String,
    pub size: f32,
    pub w: u32,
    pub h: u32,
    pub unsung: u32,
    pub sung: u32,
    pub outline: u32,
    pub outline_w: f32,
    /// 0..1
    pub outline_a: f32,
    pub text_a: f32,
    pub shadow: f32,
    pub bg1: u32,
    pub bg2: u32,
    pub lines: u32,
    pub cursor: Cursor,
    pub cursor_c: u32,
    pub pitch_color: bool,
    pub ribbon: bool,
    pub countdown: bool,
    pub bold: bool,
    pub upper: bool,
    pub lead: f64,
    pub audio: String,
    pub title: String,
}

impl Default for Style {
    fn default() -> Style {
        Style {
            font: String::new(),
            font_file: String::new(),
            size: 64.0,
            w: 1920,
            h: 1080,
            unsung: 0xFFFFFF,
            sung: 0xFFC83C,
            outline: 0x000000,
            outline_w: 4.0,
            outline_a: 1.0,
            text_a: 1.0,
            shadow: 2.0,
            bg1: 0x10182A,
            bg2: 0x2A1640,
            lines: 2,
            cursor: Cursor::Sweep,
            cursor_c: 0xFFFFFF,
            pitch_color: false,
            ribbon: true,
            countdown: true,
            bold: true,
            upper: false,
            lead: 2.0,
            audio: String::new(),
            title: String::new(),
        }
    }
}

fn hex(s: &str) -> Option<u32> {
    u32::from_str_radix(s.trim().trim_start_matches('#'), 16).ok().map(|v| v & 0xFFFFFF)
}

impl Style {
    /// `key=value;key=value` (colours as RRGGBB hex)
    pub fn parse(s: &str) -> Style {
        let mut st = Style::default();
        for kv in s.split(';') {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            let f = || v.parse::<f32>().ok();
            match k {
                "font" => st.font = v.to_string(),
                "fontfile" => st.font_file = v.to_string(),
                "size" => st.size = f().unwrap_or(st.size).clamp(16.0, 240.0),
                "w" => st.w = v.parse().unwrap_or(st.w).clamp(320, 7680),
                "h" => st.h = v.parse().unwrap_or(st.h).clamp(240, 4320),
                "unsung" => st.unsung = hex(v).unwrap_or(st.unsung),
                "sung" => st.sung = hex(v).unwrap_or(st.sung),
                "outline" => st.outline = hex(v).unwrap_or(st.outline),
                "outw" => st.outline_w = f().unwrap_or(st.outline_w).clamp(0.0, 30.0),
                "outa" => st.outline_a = f().unwrap_or(st.outline_a).clamp(0.0, 1.0),
                "texta" => st.text_a = f().unwrap_or(st.text_a).clamp(0.05, 1.0),
                "shadow" => st.shadow = f().unwrap_or(st.shadow).clamp(0.0, 20.0),
                "bg1" => st.bg1 = hex(v).unwrap_or(st.bg1),
                "bg2" => st.bg2 = hex(v).unwrap_or(st.bg2),
                "lines" => st.lines = v.parse().unwrap_or(st.lines).clamp(1, 4),
                "cursor" => {
                    st.cursor = match v {
                        "pop" => Cursor::Pop,
                        "fill" => Cursor::Fill,
                        "ball" => Cursor::Ball,
                        "underline" => Cursor::Underline,
                        "block" => Cursor::Block,
                        "arrow" => Cursor::Arrow,
                        "none" => Cursor::None,
                        _ => Cursor::Sweep,
                    }
                }
                "cursorc" => st.cursor_c = hex(v).unwrap_or(st.cursor_c),
                "pitchcolor" => st.pitch_color = v == "1",
                "ribbon" => st.ribbon = v == "1",
                "countdown" => st.countdown = v == "1",
                "bold" => st.bold = v == "1",
                "upper" => st.upper = v == "1",
                "lead" => st.lead = f().map(|x| x as f64).unwrap_or(st.lead).clamp(0.3, 8.0),
                "audio" => st.audio = v.to_string(),
                "title" => st.title = v.to_string(),
                _ => {}
            }
        }
        st
    }
    fn cursor_name(&self) -> &'static str {
        match self.cursor {
            Cursor::Sweep => "sweep",
            Cursor::Pop => "pop",
            Cursor::Fill => "fill",
            Cursor::Ball => "ball",
            Cursor::Underline => "underline",
            Cursor::Block => "block",
            Cursor::Arrow => "arrow",
            Cursor::None => "none",
        }
    }
}

/// the colour of a pitch class (the same twelve colours the sheet music uses)
pub fn pc_rgb(pc: u8) -> u32 {
    // hue walks the circle of fifths so neighbouring keys have neighbouring colours
    let order = [0u8, 7, 2, 9, 4, 11, 6, 1, 8, 3, 10, 5];
    let pos = order.iter().position(|x| *x == pc % 12).unwrap_or(0) as f32;
    let h = pos * 30.0;
    let (s, v) = (0.72f32, 0.98f32);
    let c = v * s;
    let x = c * (1.0 - ((h / 60.0) % 2.0 - 1.0).abs());
    let m = v - c;
    let (r, g, b) = match (h / 60.0) as u32 {
        0 => (c, x, 0.0),
        1 => (x, c, 0.0),
        2 => (0.0, c, x),
        3 => (0.0, x, c),
        4 => (x, 0.0, c),
        _ => (c, 0.0, x),
    };
    let q = |f: f32| ((f + m) * 255.0).round().clamp(0.0, 255.0) as u32;
    q(r) << 16 | q(g) << 8 | q(b)
}

// ── timing helpers ────────────────────────────────────────────────────────────

/// when each line appears and disappears: the lines alternate between `slots` places on screen and a line shows from when the line before it
/// in the same place has finished (at the latest `lead` before it is sung)
fn windows(lines: &[Line], slots: usize, lead: f64, hold: f64) -> Vec<(f64, f64)> {
    let mut out: Vec<(f64, f64)> = vec![];
    for (i, l) in lines.iter().enumerate() {
        let start = l.start();
        let mut appear = (start - lead).max(0.0);
        if slots > 1 && i >= slots {
            appear = appear.max(out[i - slots].1 + 0.15).min((start - 0.4).max(0.0));
        } else if slots == 1 && i > 0 {
            appear = appear.max(out[i - 1].1 + 0.05).min((start - 0.2).max(0.0));
        }
        let mut gone = l.end() + hold;
        if let Some(next) = lines.get(i + slots.max(1)) {
            gone = gone.min((next.start() - lead).max(l.end() + 0.1));
        }
        out.push((appear, gone));
    }
    out
}

fn ts_srt(t: f64) -> String {
    let ms = (t.max(0.0) * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}
fn ts_ass(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{}:{:02}:{:02}.{:02}", cs / 360_000, cs / 6000 % 60, cs / 100 % 60, cs % 100)
}
fn ts_lrc(t: f64) -> String {
    let cs = (t.max(0.0) * 100.0).round() as u64;
    format!("{:02}:{:02}.{:02}", cs / 6000, cs / 100 % 60, cs % 100)
}
fn ass_color(rgb: u32, opacity: f32) -> String {
    let (r, g, b) = ((rgb >> 16) & 255, (rgb >> 8) & 255, rgb & 255);
    let a = ((1.0 - opacity.clamp(0.0, 1.0)) * 255.0).round() as u32;
    format!("&H{a:02X}{b:02X}{g:02X}{r:02X}")
}
fn disp(s: &str, st: &Style) -> String {
    if st.upper {
        s.to_uppercase()
    } else {
        s.to_string()
    }
}

// ── .ass ──────────────────────────────────────────────────────────────────────

/// family name of a font file (name id 16, else 1), read from its `name` table
pub fn font_family(data: &[u8]) -> Option<String> {
    let u16be = |o: usize| -> Option<usize> { Some((*data.get(o)? as usize) << 8 | *data.get(o + 1)? as usize) };
    let u32be = |o: usize| -> Option<usize> { Some(u16be(o)? << 16 | u16be(o + 2)?) };
    // a collection starts with 'ttcf': use its first font
    let base = if data.starts_with(b"ttcf") { u32be(12)? } else { 0 };
    let n = u16be(base + 4)?;
    let mut name_off = None;
    for i in 0..n {
        let rec = base + 12 + i * 16;
        if data.get(rec..rec + 4)? == b"name" {
            name_off = Some(u32be(rec + 8)?);
        }
    }
    let t = name_off?;
    let count = u16be(t + 2)?;
    let str_off = t + u16be(t + 4)?;
    let mut best: Option<(u8, String)> = None;
    for i in 0..count {
        let r = t + 6 + i * 12;
        let (plat, id, len, off) = (u16be(r)?, u16be(r + 6)?, u16be(r + 8)?, u16be(r + 10)?);
        if id != 16 && id != 1 {
            continue;
        }
        let raw = data.get(str_off + off..str_off + off + len)?;
        let s = if plat == 3 || plat == 0 { String::from_utf16_lossy(&raw.chunks_exact(2).map(|c| (c[0] as u16) << 8 | c[1] as u16).collect::<Vec<_>>()) } else { String::from_utf8_lossy(raw).into_owned() };
        let rank = if id == 16 { 2 } else { 1 };
        if best.as_ref().map(|b| rank > b.0).unwrap_or(true) && !s.trim().is_empty() {
            best = Some((rank, s));
        }
    }
    best.map(|b| b.1)
}

pub fn ass(song: &Song, st: &Style, family: &str) -> String {
    let (w, h) = (st.w, st.h);
    let size = st.size * h as f32 / 1080.0;
    let slots = st.lines.max(1) as usize;
    let mut o = String::new();
    o.push_str(&format!("[Script Info]\nTitle: {}\nScriptType: v4.00+\nPlayResX: {w}\nPlayResY: {h}\nWrapStyle: 2\nScaledBorderAndShadow: yes\n\n", song.title));
    let fam = if family.is_empty() { "Arial" } else { family };
    // ASS karaoke: PrimaryColour is the sung part, SecondaryColour the part still to come
    let unsung = if st.cursor == Cursor::None { st.sung } else { st.unsung };
    o.push_str("[V4+ Styles]\nFormat: Name, Fontname, Fontsize, PrimaryColour, SecondaryColour, OutlineColour, BackColour, Bold, Italic, Underline, StrikeOut, ScaleX, ScaleY, Spacing, Angle, BorderStyle, Outline, Shadow, Alignment, MarginL, MarginR, MarginV, Encoding\n");
    o.push_str(&format!(
        "Style: Karaoke,{fam},{size:.0},{},{},{},{},{},0,0,0,100,100,0,0,1,{:.1},{:.1},5,20,20,20,1\n",
        ass_color(st.sung, st.text_a),
        ass_color(unsung, st.text_a),
        ass_color(st.outline, st.outline_a),
        ass_color(0x000000, 0.6 * st.outline_a),
        if st.bold { -1 } else { 0 },
        st.outline_w * h as f32 / 1080.0,
        st.shadow * h as f32 / 1080.0
    ));
    o.push_str(&format!(
        "Style: Count,{fam},{:.0},{},{},{},&H80000000,-1,0,0,0,100,100,0,0,1,{:.1},0,5,20,20,20,1\n\n",
        size * 0.8,
        ass_color(st.sung, 1.0),
        ass_color(st.sung, 1.0),
        ass_color(st.outline, st.outline_a),
        st.outline_w * h as f32 / 1080.0
    ));
    o.push_str("[Events]\nFormat: Layer, Start, End, Style, Name, MarginL, MarginR, MarginV, Effect, Text\n");
    let ys: Vec<f32> = match slots {
        1 => vec![0.78],
        2 => vec![0.62, 0.82],
        3 => vec![0.46, 0.64, 0.82],
        _ => vec![0.40, 0.55, 0.70, 0.85],
    };
    let win = windows(&song.lines, slots, st.lead, 0.5);
    let tag = match st.cursor {
        Cursor::Pop => "k",
        Cursor::Fill => "ko",
        Cursor::None => "k",
        _ => "kf",
    };
    for (i, l) in song.lines.iter().enumerate() {
        let (appear, gone) = win[i];
        let y = (ys[i % slots] * h as f32).round() as u32;
        let mut text = format!("{{\\an5\\pos({},{y})}}", w / 2);
        let lead_cs = ((l.start() - appear) * 100.0).round() as i64;
        if lead_cs > 0 {
            text.push_str(&format!("{{\\k{lead_cs}}}"));
        }
        let mut t = l.start();
        for (j, s) in l.syls.iter().enumerate() {
            let gap = ((s.s - t) * 100.0).round() as i64;
            if gap > 0 {
                text.push_str(&format!("{{\\k{gap}}}"));
            }
            let dur = (((s.e - s.s) * 100.0).round() as i64).max(1);
            let pre = if s.ws && j > 0 { " " } else { "" };
            let mut tg = format!("{{\\{tag}{dur}");
            if st.pitch_color {
                tg.push_str(&format!("\\1c&H{:02X}{:02X}{:02X}&", pc_rgb(s.key % 12) & 255, (pc_rgb(s.key % 12) >> 8) & 255, (pc_rgb(s.key % 12) >> 16) & 255));
            }
            tg.push('}');
            text.push_str(&format!("{pre}{tg}{}", disp(&s.text, st)));
            t = s.e.max(t);
        }
        o.push_str(&format!("Dialogue: 0,{},{},Karaoke,,0,0,0,,{text}\n", ts_ass(appear), ts_ass(gone)));
    }
    // count-in: three dots before the first line and after long rests
    if st.countdown {
        let mut marks: Vec<f64> = vec![];
        for (i, l) in song.lines.iter().enumerate() {
            let prev_end = if i == 0 { f64::MIN } else { song.lines[i - 1].end() };
            if i == 0 || l.start() - prev_end >= 6.0 {
                marks.push(l.start());
            }
        }
        for m in marks {
            for k in 0..3 {
                let t0 = m - 3.0 + k as f64;
                if t0 < 0.0 {
                    continue;
                }
                let dots: String = (0..(3 - k)).map(|_| "●").collect::<Vec<_>>().join(" ");
                o.push_str(&format!("Dialogue: 1,{},{},Count,,0,0,0,,{{\\an5\\pos({},{})\\fad(80,80)}}{dots}\n", ts_ass(t0), ts_ass(t0 + 1.0), w / 2, (h as f32 * ys[0] - size * 1.6).round() as u32));
            }
        }
    }
    o
}

// ── .srt / .lrc ───────────────────────────────────────────────────────────────

pub fn srt_lines(song: &Song, st: &Style) -> String {
    let win = windows(&song.lines, 1, st.lead.min(1.0), 0.4);
    let mut o = String::new();
    for (i, l) in song.lines.iter().enumerate() {
        o.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, ts_srt(win[i].0), ts_srt(win[i].1), disp(&l.text(), st)));
    }
    o
}

/// one entry per syllable: the line with what has been sung so far coloured
pub fn srt_karaoke(song: &Song, st: &Style) -> String {
    let win = windows(&song.lines, 1, st.lead.min(1.0), 0.4);
    let col = format!("#{:06X}", st.sung);
    let mut o = String::new();
    let mut n = 0;
    for (i, l) in song.lines.iter().enumerate() {
        let (appear, gone) = win[i];
        let mut starts: Vec<f64> = vec![appear];
        starts.extend(l.syls.iter().map(|s| s.s));
        for k in 0..=l.syls.len() {
            let t0 = starts[k];
            let t1 = if k < l.syls.len() { starts.get(k + 1).copied().unwrap_or(gone) } else { gone };
            // the entry for "k syllables sung" runs from when the k-th began
            let (from, to) = if k == 0 { (appear, l.syls.first().map(|s| s.s).unwrap_or(gone)) } else { (t0, if k < l.syls.len() { l.syls[k].s } else { gone }) };
            let _ = t1;
            if to <= from + 0.001 {
                continue;
            }
            let mut text = String::new();
            let mut open = false;
            for (j, s) in l.syls.iter().enumerate() {
                if s.ws && j > 0 {
                    text.push(' ');
                }
                if j == 0 && k > 0 {
                    text.push_str(&format!("<font color=\"{col}\">"));
                    open = true;
                }
                if j == k && open {
                    text.push_str("</font>");
                    open = false;
                }
                text.push_str(&disp(&s.text, st));
            }
            if open {
                text.push_str("</font>");
            }
            n += 1;
            o.push_str(&format!("{n}\n{} --> {}\n{text}\n\n", ts_srt(from), ts_srt(to)));
        }
    }
    o
}

pub fn lrc(song: &Song, st: &Style) -> String {
    let mut o = format!("[ti:{}]\n[la:song]\n[length:{}]\n", song.title, ts_lrc(song.length).split('.').next().unwrap_or("00:00"));
    for l in &song.lines {
        o.push_str(&format!("[{}]", ts_lrc(l.start())));
        for (j, s) in l.syls.iter().enumerate() {
            if s.ws && j > 0 {
                o.push(' ');
            }
            o.push_str(&format!("<{}>{}", ts_lrc(s.s), disp(&s.text, st)));
        }
        o.push_str(&format!("<{}>\n", ts_lrc(l.end())));
    }
    o
}

// ── .html player ──────────────────────────────────────────────────────────────

fn b64(data: &[u8]) -> String {
    const T: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut o = String::with_capacity(data.len() * 4 / 3 + 4);
    for c in data.chunks(3) {
        let n = (c[0] as u32) << 16 | (*c.get(1).unwrap_or(&0) as u32) << 8 | *c.get(2).unwrap_or(&0) as u32;
        o.push(T[(n >> 18) as usize & 63] as char);
        o.push(T[(n >> 12) as usize & 63] as char);
        o.push(if c.len() > 1 { T[(n >> 6) as usize & 63] as char } else { '=' });
        o.push(if c.len() > 2 { T[n as usize & 63] as char } else { '=' });
    }
    o
}

fn js_str(s: &str) -> String {
    let mut o = String::from("\"");
    for c in s.chars() {
        match c {
            '"' => o.push_str("\\\""),
            '\\' => o.push_str("\\\\"),
            '\n' => o.push_str("\\n"),
            '<' => o.push_str("\\u003c"),
            c => o.push(c),
        }
    }
    o.push('"');
    o
}

/// `fonts`: (family name, file bytes, mime) embedded with @font-face
pub fn html(song: &Song, st: &Style, fonts: &[(String, Vec<u8>, &str)]) -> String {
    let mut data = String::from("{\"lines\":[");
    for (i, l) in song.lines.iter().enumerate() {
        if i > 0 {
            data.push(',');
        }
        data.push('[');
        for (j, s) in l.syls.iter().enumerate() {
            if j > 0 {
                data.push(',');
            }
            data.push_str(&format!("[{:.3},{:.3},{},{},{}]", s.s, s.e, js_str(&s.text), if s.ws { 1 } else { 0 }, s.key));
        }
        data.push(']');
    }
    data.push_str("],\"notes\":[");
    for (i, n) in song.notes.iter().enumerate() {
        if i > 0 {
            data.push(',');
        }
        data.push_str(&format!("[{:.3},{:.3},{}]", n.0, n.1, n.2));
    }
    data.push_str(&format!("],\"length\":{:.3},\"title\":{},\"bpm\":{:.2}}}", song.length, js_str(&song.title), song.bpm));
    let mut faces = String::new();
    let mut choices = String::from("[[\"System sans\",\"'Segoe UI',Arial,sans-serif\"],[\"Serif\",\"Georgia,serif\"],[\"Monospace\",\"Consolas,monospace\"]");
    for (fam, bytes, mime) in fonts {
        let css = format!("lsx_{}", fam.chars().filter(|c| c.is_alphanumeric()).collect::<String>());
        faces.push_str(&format!("@font-face{{font-family:'{css}';src:url(data:{mime};base64,{}) format('{}');}}\n", b64(bytes), if *mime == "font/otf" { "opentype" } else { "truetype" }));
        choices.push_str(&format!(",[{},{}]", js_str(fam), js_str(&format!("'{css}','Segoe UI',sans-serif"))));
    }
    choices.push(']');
    let hexs = |c: u32| format!("#{c:06x}");
    let settings = format!(
        "{{\"font\":{},\"size\":{},\"unsung\":\"{}\",\"sung\":\"{}\",\"outline\":\"{}\",\"outw\":{},\"outa\":{},\"texta\":{},\"bg1\":\"{}\",\"bg2\":\"{}\",\"lines\":{},\"cursor\":\"{}\",\"cursorc\":\"{}\",\"pitchcolor\":{},\"ribbon\":{},\"bold\":{},\"upper\":{}}}",
        js_str(&st.font),
        st.size,
        hexs(st.unsung),
        hexs(st.sung),
        hexs(st.outline),
        st.outline_w,
        st.outline_a,
        st.text_a,
        hexs(st.bg1),
        hexs(st.bg2),
        st.lines,
        st.cursor_name(),
        hexs(st.cursor_c),
        st.pitch_color,
        st.ribbon,
        st.bold,
        st.upper
    );
    let pcs: String = (0..12).map(|i| format!("\"#{:06x}\"", pc_rgb(i))).collect::<Vec<_>>().join(",");
    include_str!("karaoke_player.html")
        .replace("/*FONTS*/", &faces)
        .replace("/*DATA*/", &data)
        .replace("/*SETTINGS*/", &settings)
        .replace("/*FONTCHOICES*/", &choices)
        .replace("/*PCS*/", &pcs)
        .replace("/*AUDIO*/", &js_str(&st.audio))
        .replace("<!--TITLE-->", &song.title.replace('<', "&lt;"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Song {
        let mk = |s: f64, e: f64, t: &str, ws: bool, key: u8| Syl { s, e, text: t.into(), ws, key, track: 0, brk: false, b0: s, b1: e };
        Song {
            lines: vec![Line { syls: vec![mk(1.0, 1.5, "Hel", true, 60), mk(1.5, 2.0, "lo", false, 62), mk(2.0, 3.0, "world", true, 64)] }, Line { syls: vec![mk(10.0, 10.5, "Bye", true, 67)] }],
            notes: vec![(1.0, 1.5, 60, 0), (1.5, 2.0, 62, 0), (2.0, 3.0, 64, 0), (10.0, 10.5, 67, 0)],
            title: "T".into(),
            bpm: 120.0,
            length: 12.0,
        }
    }

    #[test]
    fn ass_has_karaoke_tags_and_colours() {
        let st = Style::parse("sung=FFC83C;unsung=FFFFFF;outline=000000;outw=5;outa=0.5;cursor=sweep");
        let a = ass(&song(), &st, "Arial");
        assert!(a.contains("{\\kf50}Hel") && a.contains("{\\kf50}lo") && a.contains("{\\kf100} {\\kf") == false);
        assert!(a.contains("&H003CC8FF"), "sung colour in BGR: {a}");
        assert!(a.contains("&H80000000"), "outline at 50% opacity");
        assert!(a.contains("Dialogue: 1,"), "count-in dots");
        // a pop cursor uses \k, outline fill \ko
        assert!(ass(&song(), &Style::parse("cursor=pop"), "A").contains("{\\k50}Hel"));
        assert!(ass(&song(), &Style::parse("cursor=fill"), "A").contains("{\\ko50}Hel"));
    }

    #[test]
    fn srt_lrc_forms() {
        let st = Style::default();
        let s = srt_lines(&song(), &st);
        assert!(s.contains("Hello world") && s.contains("-->"));
        let k = srt_karaoke(&song(), &st);
        assert!(k.contains("<font color=\"#FFC83C\">Hel</font>lo world") || k.contains("<font color=\"#FFC83C\">Hel"), "{k}");
        let l = lrc(&song(), &st);
        assert!(l.contains("[00:01.00]<00:01.00>Hel<00:01.50>lo <00:02.00>world"), "{l}");
    }

    #[test]
    fn lines_alternate_and_count_in() {
        let w = windows(&song().lines, 2, 2.0, 0.5);
        assert_eq!(w.len(), 2);
        assert!(w[0].0 <= 0.0 + 1e-9 + 0.0 || (w[0].0 - 0.0).abs() < 1e-9, "first line appears 2 s ahead, clamped at 0: {:?}", w);
        assert!(w[1].0 >= 8.0 - 1e-9 && w[1].0 < 10.0);
    }

    #[test]
    fn font_family_reads_the_assets() {
        let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../LingStationX/assets/fonts/Daemon.otf");
        if let Ok(d) = std::fs::read(p) {
            assert!(font_family(&d).is_some());
        }
    }
}
