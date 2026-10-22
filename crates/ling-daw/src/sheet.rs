//! Sheet music as a playful, technical schematic: no staff, no clefs. Time runs left to right on a bar / beat grid, height is pitch, every
//! note is a tile whose colour is its pitch class (twelve colours, the same ones the karaoke ribbon uses), whose letter is its name in
//! English, Daemon or Chesilin letters, whose width is its length and whose brightness is its loudness. Black-key rows are shaded, chords
//! are stacked and joined by a stem, drums get a symbol per piece, lyrics sit under the notes they are sung on. A sight reader learns the
//! colour wheel on the first page and then reads the rest like tablature.
//!
//! Output: one SVG per page (fonts embedded) and one HTML with every page, ready to print to PDF.
use crate::karaoke::pc_rgb;
use crate::model::*;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum FontMode {
    English,
    Daemon,
    Chesilin,
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Scheme {
    /// twelve pitch-class colours
    Rainbow,
    /// the same, softer, for printing on paper
    Pastel,
    /// white tiles with black outlines: only the letters tell the pitch
    Ink,
    /// the track's own colour, lighter for higher octaves
    Track,
}

#[derive(Clone, Debug)]
pub struct Opts {
    pub font: FontMode,
    pub scheme: Scheme,
    pub bars_per_row: usize,
    pub landscape: bool,
    /// tracks to draw (empty = every track with notes)
    pub tracks: Vec<usize>,
    pub lyrics: bool,
    pub names: bool,
    pub octaves: bool,
    pub dark: bool,
    pub square: bool,
    /// 1-based inclusive bars to draw; 0 = to the end
    pub from_bar: usize,
    pub to_bar: usize,
    pub title: String,
    /// notation on five-line staves (default) instead of the piano-roll style grid
    pub staff: bool,
}

impl Default for Opts {
    fn default() -> Opts {
        Opts { font: FontMode::Daemon, scheme: Scheme::Rainbow, bars_per_row: 4, landscape: true, tracks: vec![], lyrics: true, names: true, octaves: true, dark: false, square: false, from_bar: 1, to_bar: 0, title: String::new(), staff: true }
    }
}

impl Opts {
    /// `key=value;key=value`
    pub fn parse(s: &str) -> Opts {
        let mut o = Opts::default();
        for kv in s.split(';') {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "font" => o.font = match v { "english" => FontMode::English, "chesilin" => FontMode::Chesilin, _ => FontMode::Daemon },
                "scheme" => o.scheme = match v { "pastel" => Scheme::Pastel, "ink" => Scheme::Ink, "track" => Scheme::Track, _ => Scheme::Rainbow },
                "bars" => o.bars_per_row = v.parse().unwrap_or(4usize).clamp(1, 16),
                "landscape" => o.landscape = v != "0",
                "tracks" => o.tracks = v.split(',').filter_map(|x| x.trim().parse().ok()).collect(),
                "lyrics" => o.lyrics = v != "0",
                "names" => o.names = v != "0",
                "octaves" => o.octaves = v != "0",
                "dark" => o.dark = v == "1",
                "square" => o.square = v == "1",
                "from" => o.from_bar = v.parse().unwrap_or(1usize).max(1),
                "to" => o.to_bar = v.parse().unwrap_or(0),
                "title" => o.title = v.to_string(),
                "style" => o.staff = v != "grid",
                _ => {}
            }
        }
        o
    }
}

// ── data ──────────────────────────────────────────────────────────────────────

#[derive(Clone)]
struct N {
    s: f64,
    e: f64,
    key: u8,
    vel: f32,
}

fn track_notes(t: &Track) -> Vec<N> {
    let mut out = vec![];
    for c in &t.clips {
        let ClipData::Midi(notes) = &c.data else { continue };
        let ll = if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 };
        let reps = if ll > 0.0 { ((c.len / ll).ceil() as usize).min(512) } else { 1 };
        for k in 0..reps {
            let base = c.start + k as f64 * ll;
            let limit = if ll > 0.0 { ll.min(c.len - k as f64 * ll) } else { c.len };
            for n in notes.iter().filter(|n| n.start < limit) {
                out.push(N { s: base + n.start, e: base + n.start + n.len.min(limit - n.start).max(0.05), key: n.key, vel: n.vel });
            }
        }
    }
    out.sort_by(|a, b| a.s.total_cmp(&b.s).then(a.key.cmp(&b.key)));
    out
}

const NAMES: [&str; 12] = ["C", "C", "D", "D", "E", "F", "F", "G", "G", "A", "A", "B"];
fn is_sharp(k: u8) -> bool {
    matches!(k % 12, 1 | 3 | 6 | 8 | 10)
}

/// drum groups, top row first: (label letter, colour, keys)
const DRUMS: [(&str, u32, &[u8]); 8] = [
    ("C", 0xE8B23A, &[49, 57, 52, 55]),
    ("R", 0x9AA8B8, &[51, 59, 53, 56]),
    ("O", 0x58C8E8, &[46]),
    ("H", 0x3AA0D8, &[42, 44]),
    ("P", 0xC878E8, &[39, 54, 69, 70, 75, 76, 77, 80, 81]),
    ("S", 0xE8584A, &[37, 38, 40]),
    ("T", 0x5AC878, &[41, 43, 45, 47, 48, 50]),
    ("K", 0xF08A2A, &[35, 36]),
];
fn drum_row(k: u8) -> usize {
    DRUMS.iter().position(|d| d.2.contains(&k)).unwrap_or(4)
}

// ── drawing ───────────────────────────────────────────────────────────────────

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
fn hexc(c: u32) -> String {
    format!("#{c:06x}")
}
fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32| -> u32 {
        let (x, y) = (((a >> s) & 255) as f32, ((b >> s) & 255) as f32);
        (x + (y - x) * t).round().clamp(0.0, 255.0) as u32
    };
    ch(16) << 16 | ch(8) << 8 | ch(0)
}

struct Theme {
    paper: u32,
    ink: u32,
    faint: u32,
    black_row: u32,
}
fn theme(dark: bool) -> Theme {
    if dark {
        Theme { paper: 0x0e1118, ink: 0xe8edf5, faint: 0x2b3340, black_row: 0x161b25 }
    } else {
        Theme { paper: 0xfbfaf6, ink: 0x1a1f2b, faint: 0xd5d8de, black_row: 0xeceef2 }
    }
}

fn tile_fill(o: &Opts, th: &Theme, key: u8, track_col: u32) -> (u32, u32) {
    match o.scheme {
        Scheme::Rainbow => (pc_rgb(key % 12), mix(pc_rgb(key % 12), if o.dark { 0xffffff } else { 0x000000 }, 0.45)),
        Scheme::Pastel => (mix(pc_rgb(key % 12), 0xffffff, 0.45), mix(pc_rgb(key % 12), 0x000000, 0.25)),
        Scheme::Ink => (th.paper, th.ink),
        Scheme::Track => {
            let oct = (key / 12) as f32;
            (mix(track_col, if o.dark { 0x000000 } else { 0xffffff }, ((oct - 2.0) / 7.0).clamp(0.0, 1.0) * 0.6), mix(track_col, 0x000000, 0.4))
        }
    }
}

struct Page {
    body: String,
}

struct Ctx<'a> {
    o: &'a Opts,
    th: Theme,
    w: f64,
    h: f64,
    fam_note: String,
    sys: std::cell::RefCell<Vec<(usize, SysGeo)>>,
    nts: std::cell::RefCell<Vec<(usize, NoteGeo)>>,
    page: std::cell::Cell<usize>,
}

fn font_family(m: FontMode) -> &'static str {
    match m {
        FontMode::English => "'Segoe UI',Arial,sans-serif",
        FontMode::Daemon => "'LSXDaemon','Segoe UI',sans-serif",
        FontMode::Chesilin => "'LSXChesilin','Segoe UI',sans-serif",
    }
}

fn text(x: f64, y: f64, size: f64, fill: &str, anchor: &str, fam: &str, s: &str, extra: &str) -> String {
    format!("<text x=\"{x:.1}\" y=\"{y:.1}\" font-size=\"{size:.1}\" fill=\"{fill}\" text-anchor=\"{anchor}\" font-family=\"{fam}\" {extra}>{}</text>\n", esc(s))
}

/// a small sharp sign drawn with lines (every font has one, and Chesilin has none)
fn sharp_mark(x: f64, y: f64, s: f64, col: &str) -> String {
    format!(
        "<g stroke=\"{col}\" stroke-width=\"{:.2}\" stroke-linecap=\"round\"><path d=\"M{:.1} {:.1}v{:.1}M{:.1} {:.1}v{:.1}M{:.1} {:.1}l{:.1} {:.1}M{:.1} {:.1}l{:.1} {:.1}\"/></g>\n",
        (s * 0.12).max(0.6),
        x - s * 0.15, y - s * 0.5, s,
        x + s * 0.15, y - s * 0.55, s,
        x - s * 0.4, y - s * 0.12, s * 0.8, -s * 0.2,
        x - s * 0.4, y + s * 0.2, s * 0.8, -s * 0.2
    )
}

fn legend(c: &Ctx, x: f64, y: f64) -> String {
    let (o, th) = (c.o, &c.th);
    let mut s = String::new();
    let ink = hexc(th.ink);
    s.push_str(&text(x, y + 8.0, 9.0, &hexc(mix(th.ink, th.paper, 0.45)), "start", "'Segoe UI',Arial,sans-serif", "HOW TO READ", "letter-spacing=\"2\""));
    // the colour wheel: twelve tiles in chromatic order
    let tw = 34.0;
    for k in 0..12u8 {
        let (fill, stroke) = tile_fill(o, th, 60 + k, 0x4a64d8);
        let tx = x + k as f64 * (tw + 3.0);
        let r = if o.square { 0.0 } else { 6.0 };
        s.push_str(&format!("<rect x=\"{tx:.1}\" y=\"{:.1}\" width=\"{tw}\" height=\"28\" rx=\"{r}\" fill=\"{}\" stroke=\"{}\" stroke-width=\"1.2\"/>\n", y + 16.0, hexc(fill), hexc(stroke)));
        let lc = if matches!(o.scheme, Scheme::Ink) { ink.clone() } else { "#10131a".to_string() };
        s.push_str(&text(tx + tw / 2.0 - if is_sharp(k) { 4.0 } else { 0.0 }, y + 37.0, 20.0, &lc, "middle", font_family(o.font), NAMES[k as usize], ""));
        if is_sharp(k) {
            s.push_str(&sharp_mark(tx + tw - 8.0, y + 25.0, 9.0, &lc));
        }
        // the plain letter underneath, so the glyphs can be learned
        s.push_str(&text(tx + tw / 2.0, y + 56.0, 9.0, &hexc(mix(th.ink, th.paper, 0.35)), "middle", "'Segoe UI',Arial,sans-serif", &format!("{}{}", NAMES[k as usize], if is_sharp(k) { "#" } else { "" }), ""));
    }
    let lx = x + 12.0 * (tw + 3.0) + 22.0;
    let line = |i: f64, t: &str| text(lx, y + 18.0 + i * 13.0, 10.0, &hexc(mix(th.ink, th.paper, 0.2)), "start", "'Segoe UI',Arial,sans-serif", t, "");
    if o.staff {
        s.push_str(&line(0.0, "STAFF: higher on the lines = higher pitch (treble / bass clef)"));
        s.push_str(&line(1.0, "COLOUR of each head names the note (see the wheel); # = sharp"));
        s.push_str(&line(2.0, "DARK-CENTRED head = long note; flags / beams = shorter"));
        s.push_str(&line(3.0, "RESTS = silence; a curve ties a note over a bar line"));
    } else {
        s.push_str(&line(0.0, "TIME runs left to right: a bar = 4 beats, the lines mark each beat"));
        s.push_str(&line(1.0, "HEIGHT is pitch: higher tile = higher note; shaded rows are the black keys"));
        s.push_str(&line(2.0, "WIDTH is length; a long tile is a held note; a stem joins a chord"));
        s.push_str(&line(3.0, "BRIGHT is loud, faded is soft; the small number is the octave"));
    }
    s.push_str(&line(4.0, "DRUMS: K kick  S snare  H hat  O open hat  T tom  C crash  R ride  P perc"));
    s
}

fn header(c: &Ctx, p: &Project, first: bool, page: usize, pages: usize, bars: usize, secs: f64) -> String {
    let (o, th) = (c.o, &c.th);
    let mut s = String::new();
    let title = if o.title.is_empty() { p.name.clone() } else { o.title.clone() };
    let ink = hexc(th.ink);
    let dim = hexc(mix(th.ink, th.paper, 0.45));
    if first {
        let fs = if title.chars().count() > 30 { 30.0 } else { 38.0 };
        s.push_str(&text(40.0, 58.0, fs, &ink, "start", font_family(o.font), &title, ""));
        s.push_str(&text(42.0, 84.0, 12.0, &dim, "start", "'Segoe UI',Arial,sans-serif", &format!("{} BPM   4/4   {} bars   {}:{:02}   LingStation", p.bpm.round(), bars, (secs / 60.0) as u32, (secs % 60.0) as u32), "letter-spacing=\"1.5\""));
        s.push_str(&format!("<line x1=\"40\" y1=\"96\" x2=\"{:.1}\" y2=\"96\" stroke=\"{}\" stroke-width=\"1.4\"/>\n", c.w - 40.0, ink));
        s.push_str(&legend(c, 40.0, 106.0));
    } else {
        s.push_str(&text(40.0, 40.0, 18.0, &ink, "start", font_family(o.font), &title, ""));
        s.push_str(&text(c.w - 40.0, 40.0, 11.0, &dim, "end", "'Segoe UI',Arial,sans-serif", &format!("page {} / {}", page, pages), ""));
    }
    s
}

struct Lane {
    track: usize,
    drums: bool,
    min: u8,
    max: u8,
    row_h: f64,
    height: f64,
    lyrics: bool,
}

pub struct Sheet {
    pub pages: Vec<String>,
    pub bars: usize,
    pub tracks: usize,
    /// where things are on each page (staff style only): the staves of every system and every notehead, in page units, so a viewer can draw
    /// a playhead and pick and edit notes
    pub geo: Vec<PageGeo>,
}

/// one track's staff (or grand staff) in one system: beats [b0, b1) run from x0 across `bar_w` per bar
#[derive(Clone, Debug)]
pub struct SysGeo {
    pub track: usize,
    pub b0: f64,
    pub b1: f64,
    pub x0: f64,
    pub bar_w: f64,
    pub y0: f64,
    pub y1: f64,
    /// (clef 0 treble / 1 bass / 2 percussion, y of the bottom line, diatonic step of the bottom line)
    pub staves: Vec<(u8, f64, i32)>,
}

#[derive(Clone, Debug)]
pub struct NoteGeo {
    pub track: usize,
    pub key: u8,
    /// start beat of this drawn piece (a note tied across a bar is several pieces)
    pub start: f64,
    pub x: f64,
    pub y: f64,
}

#[derive(Clone, Debug, Default)]
pub struct PageGeo {
    pub systems: Vec<SysGeo>,
    pub notes: Vec<NoteGeo>,
}

/// the MIDI key on a diatonic step (C0 = 0, D0 = 1 ...), the inverse of `dn` for white keys
pub fn key_of_dn(step: i32) -> u8 {
    const SEMI: [i32; 7] = [0, 2, 4, 5, 7, 9, 11];
    (step.div_euclid(7) * 12 + SEMI[step.rem_euclid(7) as usize]).clamp(0, 127) as u8
}

/// Build the pages. `lyrics(track)` gives (start beat, end beat, text, word start) for a track's syllables.
pub fn build(p: &Project, o: &Opts, lyric_fn: &dyn Fn(usize) -> Vec<(f64, f64, String, bool)>) -> Sheet {
    if o.staff {
        return build_staff(p, o, lyric_fn);
    }
    let th = theme(o.dark);
    let (pw, ph) = if o.landscape { (1400.0, 990.0) } else { (990.0, 1400.0) };
    let c = Ctx { o, th, w: pw, h: ph, fam_note: font_family(o.font).to_string(), sys: Default::default(), nts: Default::default(), page: Default::default() };
    let end_beat = p.end_beat().max(4.0);
    let total_bars = (end_beat / 4.0).ceil() as usize;
    let from = o.from_bar.max(1) - 1;
    let to = if o.to_bar == 0 { total_bars } else { o.to_bar.min(total_bars) };
    let bars = to.saturating_sub(from).max(1);
    let secs = p.beats_to_secs(0.0, end_beat);
    // lanes
    let want: Vec<usize> = if o.tracks.is_empty() { (0..p.tracks.len()).collect() } else { o.tracks.clone() };
    let mut lanes: Vec<(Lane, Vec<N>)> = vec![];
    for ti in want {
        let Some(t) = p.tracks.get(ti) else { continue };
        if !matches!(t.kind, TrackKind::Instrument) {
            continue;
        }
        let notes = track_notes(t);
        let notes: Vec<N> = notes.into_iter().filter(|n| n.e > from as f64 * 4.0 && n.s < to as f64 * 4.0).collect();
        if notes.is_empty() {
            continue;
        }
        let drums = t.inst_kind == "drums";
        let (min, max) = if drums { (0, (DRUMS.len() - 1) as u8) } else { (notes.iter().map(|n| n.key).min().unwrap_or(60), notes.iter().map(|n| n.key).max().unwrap_or(60)) };
        let rows = if drums { DRUMS.len() } else { (max - min) as usize + 3 };
        let row_h = if drums { 11.0 } else { (150.0 / rows as f64).clamp(4.5, 11.0) };
        let ly = o.lyrics && !lyric_fn(ti).is_empty();
        lanes.push((Lane { track: ti, drums, min, max, row_h, height: rows as f64 * row_h + if ly { 20.0 } else { 0.0 }, lyrics: ly }, notes));
    }
    let grid_x = 40.0 + 120.0;
    let grid_w = pw - 40.0 - grid_x;
    let bar_w = grid_w / o.bars_per_row as f64;
    let sys_gap = 26.0;
    let lane_gap = 7.0;
    let mut pages: Vec<Page> = vec![];
    let mut cur = Page { body: String::new() };
    let mut y = 110.0 + 96.0;
    let top_first = y;
    let mut first = true;
    let mut sys_start = from;
    let sys_h = 16.0 + lanes.iter().map(|l| l.0.height + lane_gap).sum::<f64>();
    while sys_start < to {
        if y + sys_h > ph - 40.0 && y > (if first { top_first } else { 70.0 }) + 1.0 {
            pages.push(std::mem::replace(&mut cur, Page { body: String::new() }));
            first = false;
            y = 70.0;
        }
        let sys_bars = o.bars_per_row.min(to - sys_start);
        let b0 = sys_start as f64 * 4.0;
        let b1 = (sys_start + sys_bars) as f64 * 4.0;
        let x_end = grid_x + sys_bars as f64 * bar_w;
        let ink = hexc(c.th.ink);
        // bar numbers and the tempo marks
        for b in 0..=sys_bars {
            let bx = grid_x + b as f64 * bar_w;
            if b < sys_bars {
                cur.body.push_str(&text(bx + 3.0, y + 9.0, 9.0, &hexc(mix(c.th.ink, c.th.paper, 0.4)), "start", "'Segoe UI',Arial,sans-serif", &format!("{}", sys_start + b + 1), ""));
            }
        }
        for (tb, bpm) in &p.tempo_map {
            if *tb >= b0 && *tb < b1 {
                let tx = grid_x + (tb - b0) / 4.0 * bar_w;
                cur.body.push_str(&text(tx + 3.0, y - 2.0, 9.0, "#d66a2a", "start", "'Segoe UI',Arial,sans-serif", &format!("{} BPM", bpm.round()), "font-weight=\"bold\""));
            }
        }
        let mut ly = y + 16.0;
        let mut page_top = if first { top_first } else { 70.0 };
        for (li, (lane, notes)) in lanes.iter().enumerate() {
            // a system taller than the page continues on the next one, with its bar numbers repeated
            if li > 0 && ly + lane.height > ph - 40.0 && ly > page_top + 20.0 {
                pages.push(std::mem::replace(&mut cur, Page { body: String::new() }));
                first = false;
                y = 70.0;
                page_top = 70.0;
                for b in 0..sys_bars {
                    cur.body.push_str(&text(grid_x + b as f64 * bar_w + 3.0, y + 9.0, 9.0, &hexc(mix(c.th.ink, c.th.paper, 0.4)), "start", "'Segoe UI',Arial,sans-serif", &format!("{}", sys_start + b + 1), ""));
                }
                ly = y + 16.0;
            }
            let t = &p.tracks[lane.track];
            let tcol = t.color & 0xFFFFFF;
            let rh = lane.row_h;
            let grid_h = lane.height - if lane.lyrics { 20.0 } else { 0.0 };
            // label
            cur.body.push_str(&format!("<rect x=\"40\" y=\"{:.1}\" width=\"6\" height=\"{:.1}\" fill=\"{}\"/>\n", ly, grid_h, hexc(tcol)));
            cur.body.push_str(&text(54.0, ly + 14.0, 13.0, &ink, "start", &c.fam_note, &t.name.chars().take(14).collect::<String>(), ""));
            cur.body.push_str(&text(54.0, ly + 28.0, 8.5, &hexc(mix(c.th.ink, c.th.paper, 0.45)), "start", "'Segoe UI',Arial,sans-serif", &t.name.chars().take(22).collect::<String>(), ""));
            // background rows
            cur.body.push_str(&format!("<rect x=\"{grid_x:.1}\" y=\"{ly:.1}\" width=\"{:.1}\" height=\"{grid_h:.1}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1\"/>\n", x_end - grid_x, hexc(c.th.faint)));
            if !lane.drums {
                for k in lane.min.saturating_sub(1)..=lane.max + 1 {
                    let ry = ly + (lane.max as f64 + 1.0 - k as f64) * rh + rh * 0.0;
                    let ry = ry.max(ly - 0.0);
                    if is_sharp(k) {
                        cur.body.push_str(&format!("<rect x=\"{grid_x:.1}\" y=\"{ry:.1}\" width=\"{:.1}\" height=\"{rh:.1}\" fill=\"{}\"/>\n", x_end - grid_x, hexc(c.th.black_row)));
                    }
                    if k % 12 == 0 {
                        cur.body.push_str(&format!("<line x1=\"{grid_x:.1}\" y1=\"{:.1}\" x2=\"{x_end:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"0.8\"/>\n", ry + rh, ry + rh, hexc(mix(c.th.faint, c.th.ink, 0.25))));
                        if rh >= 6.0 {
                            cur.body.push_str(&text(grid_x - 4.0, ry + rh - 1.5, 8.0, &hexc(mix(c.th.ink, c.th.paper, 0.4)), "end", "'Segoe UI',Arial,sans-serif", &format!("C{}", k / 12), ""));
                        }
                    }
                }
            } else {
                for (i, d) in DRUMS.iter().enumerate() {
                    let ry = ly + i as f64 * rh;
                    cur.body.push_str(&format!("<line x1=\"{grid_x:.1}\" y1=\"{:.1}\" x2=\"{x_end:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"0.5\"/>\n", ry + rh, ry + rh, hexc(c.th.faint)));
                    cur.body.push_str(&text(grid_x - 5.0, ry + rh - 2.0, 9.0, &hexc(d.1), "end", &c.fam_note, d.0, "font-weight=\"bold\""));
                }
            }
            // beat lines
            for b in 0..sys_bars * 4 + 1 {
                let bx = grid_x + b as f64 * bar_w / 4.0;
                let bar = b % 4 == 0;
                cur.body.push_str(&format!("<line x1=\"{bx:.1}\" y1=\"{ly:.1}\" x2=\"{bx:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"{}\"{}/>\n", ly + grid_h, if bar { ink.clone() } else { hexc(c.th.faint) }, if bar { 1.3 } else { 0.7 }, if bar { "" } else { " stroke-dasharray=\"2 3\"" }));
            }
            // notes
            let mut chord_marks: Vec<(f64, f64, f64)> = vec![];
            for n in notes.iter().filter(|n| n.e > b0 && n.s < b1) {
                let (s, e) = (n.s.max(b0), n.e.min(b1));
                let x0 = grid_x + (s - b0) / 4.0 * bar_w;
                let w = ((e - s) / 4.0 * bar_w - 1.5).max(3.0);
                let op = 0.5 + 0.5 * n.vel.clamp(0.0, 1.0) as f64;
                if lane.drums {
                    let r = drum_row(n.key);
                    let (cx, cy) = (x0 + 5.0, ly + r as f64 * rh + rh * 0.5);
                    let col = hexc(if matches!(o.scheme, Scheme::Ink) { c.th.ink } else { DRUMS[r].1 });
                    let q = rh * 0.38;
                    let shape = match DRUMS[r].0 {
                        "K" => format!("<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{:.1}\" fill=\"{col}\" opacity=\"{op:.2}\"/>", q * 1.2),
                        "S" => format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{col}\" opacity=\"{op:.2}\"/>", cx - q, cy - q, q * 2.0, q * 2.0),
                        "H" => format!("<path d=\"M{:.1} {:.1}l{:.1} {:.1}M{:.1} {:.1}l{:.1} {:.1}\" stroke=\"{col}\" stroke-width=\"1.8\" opacity=\"{op:.2}\"/>", cx - q, cy - q, q * 2.0, q * 2.0, cx + q, cy - q, -q * 2.0, q * 2.0),
                        "O" => format!("<circle cx=\"{cx:.1}\" cy=\"{cy:.1}\" r=\"{:.1}\" fill=\"none\" stroke=\"{col}\" stroke-width=\"1.6\" opacity=\"{op:.2}\"/>", q * 1.1),
                        "T" => format!("<path d=\"M{cx:.1} {:.1}l{:.1} {:.1}h{:.1}z\" fill=\"{col}\" opacity=\"{op:.2}\"/>", cy - q * 1.1, q * 1.1, q * 2.0, -q * 2.2),
                        "C" => format!("<path d=\"M{:.1} {cy:.1}h{:.1}M{cx:.1} {:.1}v{:.1}M{:.1} {:.1}l{:.1} {:.1}M{:.1} {:.1}l{:.1} {:.1}\" stroke=\"{col}\" stroke-width=\"1.5\" opacity=\"{op:.2}\"/>", cx - q * 1.2, q * 2.4, cy - q * 1.2, q * 2.4, cx - q, cy - q, q * 2.0, q * 2.0, cx + q, cy - q, -q * 2.0, q * 2.0),
                        "R" => format!("<path d=\"M{:.1} {:.1}h{:.1}l{:.1} {:.1}z\" fill=\"{col}\" opacity=\"{op:.2}\"/>", cx - q * 1.1, cy - q, q * 2.2, -q * 1.1, q * 2.0),
                        _ => format!("<path d=\"M{cx:.1} {:.1}l{:.1} {:.1}l{:.1} {:.1}l{:.1} {:.1}z\" fill=\"{col}\" opacity=\"{op:.2}\"/>", cy - q * 1.2, q * 1.2, q * 1.2, -q * 1.2, q * 1.2, -q * 1.2, -q * 1.2),
                    };
                    cur.body.push_str(&shape);
                    cur.body.push('\n');
                    continue;
                }
                let y0 = ly + (lane.max as f64 + 1.0 - n.key as f64) * rh;
                let (fill, stroke) = tile_fill(o, &c.th, n.key, tcol);
                let rr = if o.square { 0.0 } else { (rh * 0.3).min(4.0) };
                let cl = n.s < b0;
                let cr = n.e > b1;
                cur.body.push_str(&format!(
                    "<rect x=\"{x0:.1}\" y=\"{:.1}\" width=\"{w:.1}\" height=\"{:.1}\" rx=\"{rr:.1}\" fill=\"{}\" fill-opacity=\"{op:.2}\" stroke=\"{}\" stroke-width=\"{}\"{}/>\n",
                    y0 + 0.6,
                    rh - 1.2,
                    hexc(fill),
                    hexc(stroke),
                    if rh >= 7.0 { 1.0 } else { 0.6 },
                    if cl || cr { " stroke-dasharray=\"0\"" } else { "" }
                ));
                if cr {
                    cur.body.push_str(&format!("<path d=\"M{:.1} {:.1}l4 {:.1}l-4 {:.1}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1.2\"/>\n", x0 + w - 1.0, y0 + 1.0, rh / 2.0 - 1.0, rh / 2.0 - 1.0, hexc(stroke)));
                }
                if cl {
                    cur.body.push_str(&format!("<path d=\"M{:.1} {:.1}v{:.1}\" stroke=\"{}\" stroke-width=\"2\"/>\n", x0 + 0.5, y0 + 1.0, rh - 2.0, hexc(stroke)));
                }
                chord_marks.push((x0, y0, y0 + rh));
                // the letter
                if o.names && !cl && rh >= 8.0 && w >= 11.0 {
                    let lc = if matches!(o.scheme, Scheme::Ink) { ink.clone() } else if o.dark && matches!(o.scheme, Scheme::Pastel) { "#10131a".into() } else { "#10131a".to_string() };
                    let fs = rh * 0.86;
                    let acc = is_sharp(n.key);
                    cur.body.push_str(&text(x0 + 3.0, y0 + rh - 1.9, fs, &lc, "start", &c.fam_note, NAMES[(n.key % 12) as usize], ""));
                    if acc {
                        cur.body.push_str(&sharp_mark(x0 + 3.0 + fs * 0.85, y0 + rh * 0.45, rh * 0.55, &lc));
                    }
                    if o.octaves && w >= 26.0 {
                        cur.body.push_str(&text(x0 + w - 3.0, y0 + rh - 2.2, rh * 0.5, &lc, "end", "'Segoe UI',Arial,sans-serif", &format!("{}", n.key / 12), "opacity=\"0.7\""));
                    }
                }
            }
            // chord stems: where several tiles start together
            chord_marks.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut i = 0;
            while i < chord_marks.len() {
                let mut j = i;
                let (mut top, mut bot) = (chord_marks[i].1, chord_marks[i].2);
                while j + 1 < chord_marks.len() && (chord_marks[j + 1].0 - chord_marks[i].0).abs() < 0.5 {
                    j += 1;
                    top = top.min(chord_marks[j].1);
                    bot = bot.max(chord_marks[j].2);
                }
                if j > i {
                    cur.body.push_str(&format!("<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"2\"/>\n", chord_marks[i].0 - 1.5, top, chord_marks[i].0 - 1.5, bot, ink));
                }
                i = j + 1;
            }
            // lyrics under the lane
            if lane.lyrics {
                let ty = ly + grid_h + 14.0;
                let mut lastx = f64::MIN;
                for (s, _e, tx, ws) in lyric_fn(lane.track).into_iter().filter(|x| x.0 >= b0 && x.0 < b1) {
                    let x0 = grid_x + (s - b0) / 4.0 * bar_w;
                    let shown = if x0 < lastx { continue } else { format!("{}{}", if ws { "" } else { "-" }, tx) };
                    cur.body.push_str(&text(x0 + 1.0, ty, 12.0, &ink, "start", &c.fam_note, &shown, ""));
                    lastx = x0 + 6.0 + 6.5 * shown.chars().count() as f64;
                }
            }
            ly += lane.height + lane_gap;
        }
        y = ly + sys_gap - lane_gap;
        sys_start += sys_bars;
    }
    pages.push(cur);
    let n_pages = pages.len();
    let mut out = vec![];
    for (i, pg) in pages.into_iter().enumerate() {
        let mut s = String::new();
        s.push_str(&format!("<rect width=\"{pw}\" height=\"{ph}\" fill=\"{}\"/>\n", hexc(c.th.paper)));
        s.push_str(&header(&c, p, i == 0, i + 1, n_pages, bars, secs));
        s.push_str(&pg.body);
        // footer rule
        s.push_str(&format!("<line x1=\"40\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"0.8\"/>\n", ph - 28.0, pw - 40.0, ph - 28.0, hexc(c.th.faint)));
        s.push_str(&text(40.0, ph - 12.0, 9.0, &hexc(mix(c.th.ink, c.th.paper, 0.45)), "start", "'Segoe UI',Arial,sans-serif", &format!("{} - page {} / {}", p.name, i + 1, n_pages), ""));
        out.push(s);
    }
    let _ = c.h;
    Sheet { pages: out, bars, tracks: lanes.len(), geo: vec![] }
}

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

/// @font-face rules for the embedded fonts: (family, bytes, mime)
pub fn font_css(fonts: &[(&str, Vec<u8>, &str)]) -> String {
    fonts.iter().map(|(fam, d, mime)| format!("@font-face{{font-family:'{fam}';src:url(data:{mime};base64,{}) format('{}');}}\n", b64(d), if *mime == "font/otf" { "opentype" } else { "truetype" })).collect()
}

/// one page as a standalone SVG file
pub fn svg_page(body: &str, landscape: bool, css: &str) -> String {
    let (w, h) = if landscape { (1400, 990) } else { (990, 1400) };
    format!("<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\" width=\"{w}\" height=\"{h}\">\n<style>{css}</style>\n{body}</svg>\n")
}

/// all pages in one printable HTML document
pub fn html(title: &str, pages: &[String], landscape: bool, css: &str) -> String {
    let (w, h) = if landscape { (1400, 990) } else { (990, 1400) };
    let mut s = format!(
        "<!doctype html><html><head><meta charset=\"utf-8\"><title>{}</title><style>{css}\n@page{{size:{};margin:0}}html,body{{margin:0;background:#8a90a0}}.pg{{width:{w}px;max-width:100%;margin:12px auto;background:#fff;box-shadow:0 2px 12px #0006}}svg{{display:block;width:100%;height:auto}}@media print{{html,body{{background:none}}.pg{{margin:0;width:{pgw};height:{pgh};overflow:hidden;box-shadow:none;break-after:page}}.pg:last-child{{break-after:auto}}svg{{width:{pgw};height:{pgh}}}}}</style></head><body>\n",
        esc(title),
        if landscape { "297mm 210mm" } else { "210mm 297mm" },
        pgw = if landscape { "297mm" } else { "210mm" },
        pgh = if landscape { "210mm" } else { "297mm" },
    );
    for p in pages {
        s.push_str(&format!("<div class=\"pg\"><svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {h}\">{p}</svg></div>\n"));
    }
    s.push_str("</body></html>\n");
    s
}

// ── notation: staves, clefs, noteheads, stems, beams, rests ───────────────────────

const GS: f64 = 9.5; // the space between two staff lines

#[derive(Clone, Copy, PartialEq)]
enum Clef {
    Treble,
    Bass,
    Perc,
}

/// (letter index C=0 .. B=6, sharp, octave) of a MIDI key, spelt with sharps
fn spell(key: u8) -> (i32, bool, i32) {
    let (l, sh) = match key % 12 {
        0 => (0, false),
        1 => (0, true),
        2 => (1, false),
        3 => (1, true),
        4 => (2, false),
        5 => (3, false),
        6 => (3, true),
        7 => (4, false),
        8 => (4, true),
        9 => (5, false),
        10 => (5, true),
        _ => (6, false),
    };
    (l, sh, (key / 12) as i32 - 1)
}
/// diatonic step number: E4 = 30, G2 = 18
fn dn(key: u8) -> i32 {
    let (l, _, o) = spell(key);
    7 * o + l
}

/// rhythmic value of a length in beats: (kind 0 whole .. 5 thirty-second, dots)
fn classify(len: f64) -> (u8, u8) {
    let table: [(f64, u8, u8); 10] = [(4.0, 0, 0), (3.0, 1, 1), (2.0, 1, 0), (1.5, 2, 1), (1.0, 2, 0), (0.75, 3, 1), (0.5, 3, 0), (0.375, 4, 1), (0.25, 4, 0), (0.125, 5, 0)];
    let mut best = table[0];
    for t in table {
        if (len - t.0).abs() < (len - best.0).abs() {
            best = t;
        }
    }
    (best.1, best.2)
}

#[derive(Clone)]
struct Piece {
    s: f64,
    e: f64,
    key: u8,
    vel: f32,
    /// the note carries over from the bar (or system) before / into the next
    tie_in: bool,
    tie_out: bool,
    /// staff position in half-steps above the bottom line (set when placed)
    pos: i32,
}

struct Group {
    x: f64,
    s: f64,
    heads: Vec<(f64, f64, Piece)>, // (x, y, piece)
    up: bool,
    kind: u8,
    stem_x: f64,
    y_from: f64,
    y_to: f64,
}

fn perc_pos(key: u8) -> (i32, &'static str) {
    match drum_row(key) {
        0 => (10, "x"),
        1 => (8, "x"),
        2 => (9, "xo"),
        3 => (9, "x"),
        4 => (7, "d"),
        5 => (5, "n"),
        6 => (4, "n"),
        _ => (1, "n"),
    }
}

fn clef_svg(clef: Clef, cx: f64, yb: f64, col: &str) -> String {
    let g = GS;
    let mut s = String::new();
    match clef {
        Clef::Treble => {
            // a stylised G clef: the spine, the curl round the G line and the hook on top
            let yg = yb - g;
            s.push_str(&format!(
                "<g fill=\"none\" stroke=\"{col}\" stroke-width=\"1.7\" stroke-linecap=\"round\"><path d=\"M{:.1} {:.1} C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}\"/><circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\"/><path d=\"M{:.1} {:.1} C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}\"/></g>\n",
                cx - g * 0.9, yb + g * 1.3, cx - g * 1.4, yb + g * 2.2, cx - g * 0.2, yb + g * 2.4, cx + g * 0.1, yb + g * 1.2,
                cx - g * 0.1, yg - g * 0.05, g * 1.0,
                cx + g * 0.1, yb + g * 1.2, cx + g * 0.0, yb - g * 3.6, cx + g * 0.9, yb - g * 5.0, cx - g * 0.3, yb - g * 5.2
            ));
            s.push_str(&format!("<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"{col}\"/>\n", cx - g * 0.9, yb + g * 1.3, g * 0.38));
        }
        Clef::Bass => {
            let y4 = yb - g * 3.0; // the F line
            s.push_str(&format!("<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"{col}\"/>\n", cx - g * 0.6, y4, g * 0.55));
            s.push_str(&format!(
                "<path d=\"M{:.1} {:.1} C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1} C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}\" fill=\"none\" stroke=\"{col}\" stroke-width=\"1.8\" stroke-linecap=\"round\"/>\n",
                cx - g * 0.6, y4, cx + g * 0.2, y4 - g * 0.9, cx + g * 1.5, y4 - g * 0.3, cx + g * 1.2, y4 + g * 1.0,
                cx + g * 0.9, y4 + g * 2.0, cx - g * 0.1, y4 + g * 2.6, cx - g * 1.2, y4 + g * 3.2
            ));
            s.push_str(&format!("<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"{col}\"/><circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"{col}\"/>\n", cx + g * 2.0, y4 - g * 0.5, g * 0.28, cx + g * 2.0, y4 + g * 0.5, g * 0.28));
        }
        Clef::Perc => {
            s.push_str(&format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{col}\"/><rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{col}\"/>\n", cx - g * 0.6, yb - g * 3.0, g * 0.45, g * 2.0, cx + g * 0.3, yb - g * 3.0, g * 0.45, g * 2.0));
        }
    }
    s
}

fn rest_svg(kind: u8, x: f64, yb: f64, col: &str) -> String {
    let g = GS;
    let mid = yb - g * 2.0;
    match kind {
        0 => format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{col}\"/>\n", x - g * 0.8, yb - g * 3.0, g * 1.6, g * 0.55),
        1 => format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"{:.1}\" fill=\"{col}\"/>\n", x - g * 0.8, mid - g * 0.55, g * 1.6, g * 0.55),
        2 => format!(
            "<path d=\"M{:.1} {:.1} l{:.1} {:.1} l{:.1} {:.1} l{:.1} {:.1} c{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}\" fill=\"none\" stroke=\"{col}\" stroke-width=\"1.8\" stroke-linejoin=\"round\"/>\n",
            x - g * 0.3, mid - g * 1.4, g * 0.7, g * 0.7, -g * 0.8, g * 0.9, g * 0.8, g * 0.8, -g * 0.7, g * 0.1, -g * 0.6, g * 0.6, -g * 0.1, g * 0.9
        ),
        k => {
            let flags = (k as usize).saturating_sub(2).max(1);
            let mut s = String::new();
            for i in 0..flags {
                let y = mid - g * 0.6 + i as f64 * g * 0.9;
                s.push_str(&format!("<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"{:.1}\" fill=\"{col}\"/>\n", x - g * 0.4 + i as f64 * g * 0.15, y, g * 0.3));
            }
            s.push_str(&format!("<path d=\"M{:.1} {:.1} l{:.1} {:.1}\" stroke=\"{col}\" stroke-width=\"1.5\"/>\n", x + g * 0.35 - 0.0, mid - g * 0.4, -g * 0.7 + (flags as f64 - 1.0) * -g * 0.15, g * 0.9 * flags as f64 + g * 0.5));
            s
        }
    }
}

/// The vertical room one track needs in a system, from the notes it actually plays: ledger lines, stems and ties may not touch the
/// neighbouring staves.
struct Lay {
    two: bool,
    single: Clef,
    split: i32,
    /// space above the first staff, between the two staves, below the last staff
    top: f64,
    gap: f64,
    bot: f64,
}

impl Lay {
    fn height(&self) -> f64 {
        self.top + 4.0 * GS + if self.two { self.gap + 4.0 * GS } else { 0.0 } + self.bot
    }
}

fn layout(all: &[N], notes: &[N], drums: bool) -> Lay {
    // how far above the top line / below the bottom line a note with this staff position (half-steps above the bottom line) reaches,
    // counting its head, ledger lines and a stem
    let up_need = |pos: i32| -> f64 { (pos as f64 * GS / 2.0 + 3.5 * GS - 4.0 * GS).max((pos as f64 * GS / 2.0 - 4.0 * GS) + GS).max(0.0) };
    let dn_need = |pos: i32| -> f64 { (-(pos as f64) * GS / 2.0 + 3.5 * GS).max(-(pos as f64) * GS / 2.0 + GS).max(0.0) };
    let (min_top, min_bot) = (2.6 * GS, 3.0 * GS);
    if drums {
        let (mut up, mut dnn) = (0.0f64, 0.0f64);
        for n in notes {
            let pos = perc_pos(n.key).0;
            up = up.max(up_need(pos));
            dnn = dnn.max(dn_need(pos));
        }
        return Lay { two: false, single: Clef::Perc, split: 0, top: min_top.max(up + 1.2 * GS), gap: 0.0, bot: min_bot.max(dnn + 0.8 * GS), };
    }
    let (lo, hi) = (all.iter().map(|n| n.key).min().unwrap_or(60), all.iter().map(|n| n.key).max().unwrap_or(60));
    let reach = |clef_dn: i32, sel: &dyn Fn(u8) -> bool| -> (f64, f64) {
        let (mut up, mut dnn) = (0.0f64, 0.0f64);
        for n in notes.iter().filter(|n| sel(n.key)) {
            let pos = dn(n.key) - clef_dn;
            up = up.max(up_need(pos));
            dnn = dnn.max(dn_need(pos));
        }
        (up, dnn)
    };
    if hi < 62 {
        let (u, d) = reach(dn(43), &|_| true);
        Lay { two: false, single: Clef::Bass, split: 0, top: min_top.max(u + 1.2 * GS), gap: 0.0, bot: min_bot.max(d + 0.8 * GS) }
    } else if lo >= 55 {
        let (u, d) = reach(dn(64), &|_| true);
        Lay { two: false, single: Clef::Treble, split: 0, top: min_top.max(u + 1.2 * GS), gap: 0.0, bot: min_bot.max(d + 0.8 * GS) }
    } else {
        let (ut, dt) = reach(dn(64), &|k| k >= 60);
        let (ub, db) = reach(dn(43), &|k| k < 60);
        Lay { two: true, single: Clef::Treble, split: 60, top: min_top.max(ut + 1.2 * GS), gap: (dt + ub + 1.6 * GS).max(5.0 * GS), bot: min_bot.max(db + 0.8 * GS) }
    }
}

/// One track in one system: its staff (or the two of a grand staff), clefs, notes, rests, lyrics. Returns the drawing and its height.
#[allow(clippy::too_many_arguments)]
fn staff_block(c: &Ctx, p: &Project, ti: usize, notes: &[N], drums: bool, lyrics: &[(f64, f64, String, bool)], b0: f64, sys_bars: usize, gx: f64, bar_w: f64, y0: f64, first_system: bool, last_system: bool) -> (String, f64) {
    let (o, th) = (c.o, &c.th);
    let ink = hexc(th.ink);
    let t = &p.tracks[ti];
    let tcol = t.color & 0xFFFFFF;
    let b1 = b0 + sys_bars as f64 * 4.0;
    let x_end = gx + sys_bars as f64 * bar_w;
    // staves: (clef, bottom y, bottom diatonic step)
    let sys_notes: Vec<N> = notes.iter().filter(|n| n.e > b0 && n.s < b1).cloned().collect();
    let lay = layout(notes, &sys_notes, drums);
    let staff_h = 4.0 * GS;
    let mut staves: Vec<(Clef, f64, i32)> = vec![];
    let split = lay.split;
    let top_y = y0 + lay.top + staff_h;
    if drums {
        staves.push((Clef::Perc, top_y, 0));
    } else if lay.two {
        staves.push((Clef::Treble, top_y, dn(64)));
        staves.push((Clef::Bass, top_y + lay.gap + staff_h, dn(43)));
    } else {
        staves.push((lay.single, top_y, if lay.single == Clef::Bass { dn(43) } else { dn(64) }));
    }
    c.sys.borrow_mut().push((
        c.page.get(),
        SysGeo { track: ti, b0, b1, x0: gx, bar_w, y0: staves[0].1 - staff_h, y1: staves.last().unwrap().1, staves: staves.iter().map(|(cl, y, d)| (match cl { Clef::Treble => 0, Clef::Bass => 1, Clef::Perc => 2 }, *y, *d)).collect() },
    ));
    let mut height = lay.height();
    let lyric_h = if !lyrics.is_empty() { 20.0 } else { 0.0 };
    let mut s = String::new();
    let x_of = |beat: f64| -> f64 { gx + (beat - b0) / 4.0 * bar_w + 14.0 };
    // the label
    let label_y = (staves[0].1 + staves.last().unwrap().1) * 0.5 - staff_h * 0.5;
    s.push_str(&format!("<rect x=\"40\" y=\"{:.1}\" width=\"3\" height=\"{:.1}\" fill=\"{}\"/>\n", staves[0].1 - staff_h - 2.0, staves.last().unwrap().1 - staves[0].1 + staff_h + 4.0, hexc(tcol)));
    // the instrument name on up to two lines, inside the 124 px gutter
    let shown_name = {
        let n = t.name.trim();
        match n.rfind(" (") {
            Some(i) if n.ends_with(')') && n[..i].trim().eq_ignore_ascii_case(n[i + 2..n.len() - 1].trim()) => n[..i].trim().to_string(),
            _ => n.to_string(),
        }
    };
    let mut lines: Vec<String> = vec![];
    let mut cur = String::new();
    for w in shown_name.split_whitespace() {
        if !cur.is_empty() && cur.chars().count() + 1 + w.chars().count() > 17 {
            lines.push(std::mem::take(&mut cur));
        }
        if !cur.is_empty() {
            cur.push(' ');
        }
        cur.push_str(w);
    }
    if !cur.is_empty() {
        lines.push(cur);
    }
    lines.truncate(2);
    for l in lines.iter_mut() {
        if l.chars().count() > 18 {
            *l = format!("{}…", l.chars().take(17).collect::<String>());
        }
    }
    for (i, l) in lines.iter().enumerate() {
        s.push_str(&text(52.0, label_y + staff_h * 0.5 + 4.0 + (i as f64 - (lines.len() as f64 - 1.0) * 0.5) * 14.0, 11.5, &ink, "start", "'Segoe UI',Arial,sans-serif", l, "font-weight=\"600\""));
    }
    let line_col = hexc(mix(th.ink, th.paper, 0.2));
    for (si, (clef, yb, bdn)) in staves.iter().enumerate() {
        // five lines, bar lines, clef, time signature
        for l in 0..5 {
            let ly = yb - l as f64 * GS;
            s.push_str(&format!("<line x1=\"{:.1}\" y1=\"{ly:.1}\" x2=\"{x_end:.1}\" y2=\"{ly:.1}\" stroke=\"{line_col}\" stroke-width=\"0.9\"/>\n", gx - 52.0));
        }
        s.push_str(&clef_svg(*clef, gx - 40.0, *yb, &ink));
        if first_system {
            let f = hexc(th.ink);
            let serif = "'Segoe UI',Arial,sans-serif";
            s.push_str(&text(gx - 6.0, yb - GS * 2.0 - 1.0, 19.0, &f, "middle", serif, "4", "font-weight=\"bold\""));
            s.push_str(&text(gx - 6.0, yb - 1.0, 19.0, &f, "middle", serif, "4", "font-weight=\"bold\""));
        }
        for b in 0..=sys_bars {
            let bx = gx + b as f64 * bar_w;
            let w = if b == sys_bars && last_system { 2.6 } else { 1.0 };
            s.push_str(&format!("<line x1=\"{bx:.1}\" y1=\"{:.1}\" x2=\"{bx:.1}\" y2=\"{yb:.1}\" stroke=\"{ink}\" stroke-width=\"{w}\"/>\n", yb - staff_h));
        }
        s.push_str(&format!("<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{yb:.1}\" stroke=\"{ink}\" stroke-width=\"1.6\"/>\n", gx - 52.0, yb - staff_h, gx - 52.0));
        let _ = (si, bdn);
    }
    if staves.len() == 2 {
        // brace and the system line joining the two staves
        let (ya, yb2) = (staves[0].1 - staff_h, staves[1].1);
        s.push_str(&format!("<line x1=\"{:.1}\" y1=\"{ya:.1}\" x2=\"{:.1}\" y2=\"{yb2:.1}\" stroke=\"{ink}\" stroke-width=\"1.6\"/>\n", gx - 52.0, gx - 52.0));
        s.push_str(&format!("<path d=\"M{:.1} {ya:.1} C{:.1} {:.1} {:.1} {:.1} {:.1} {:.1} C{:.1} {:.1} {:.1} {:.1} {:.1} {yb2:.1}\" fill=\"none\" stroke=\"{ink}\" stroke-width=\"2.2\"/>\n", gx - 58.0, gx - 70.0, ya + 18.0, gx - 54.0, (ya + yb2) * 0.5 - 12.0, gx - 68.0, (ya + yb2) * 0.5, gx - 54.0, (ya + yb2) * 0.5 + 12.0, gx - 70.0, yb2 - 18.0, gx - 58.0));
        for b in 0..=sys_bars {
            let bx = gx + b as f64 * bar_w;
            s.push_str(&format!("<line x1=\"{bx:.1}\" y1=\"{:.1}\" x2=\"{bx:.1}\" y2=\"{:.1}\" stroke=\"{ink}\" stroke-width=\"1\"/>\n", staves[0].1 - staff_h, staves[0].1));
            s.push_str(&format!("<line x1=\"{bx:.1}\" y1=\"{:.1}\" x2=\"{bx:.1}\" y2=\"{:.1}\" stroke=\"{ink}\" stroke-width=\"1\" opacity=\"0.0\"/>\n", staves[0].1, staves[1].1 - staff_h));
        }
    }
    // pieces of every note, cut at the bar lines, per staff
    let mut per_staff: Vec<Vec<Piece>> = vec![vec![]; staves.len()];
    for n in notes.iter().filter(|n| n.e > b0 && n.s < b1) {
        let si = if drums || staves.len() == 1 { 0 } else if n.key as i32 >= split { 0 } else { 1 };
        let (cs, ce) = (n.s.max(b0), n.e.min(b1));
        let mut a = cs;
        while a < ce - 1e-6 {
            let bar_end = ((a / 4.0).floor() + 1.0) * 4.0;
            let z = ce.min(bar_end);
            let pos = if drums {
                perc_pos(n.key).0
            } else {
                dn(n.key) - staves[si].2
            };
            per_staff[si].push(Piece { s: a, e: z, key: n.key, vel: n.vel, tie_in: a > cs + 1e-6 || n.s < b0 && a <= cs + 1e-6, tie_out: z < ce - 1e-6 || n.e > b1 && z >= ce - 1e-6, pos });
            a = z;
        }
    }
    let rx = GS * 0.78;
    let ry = GS * 0.56;
    for (si, (clef, yb, _)) in staves.iter().enumerate() {
        let pieces = &mut per_staff[si];
        pieces.sort_by(|a, b| a.s.total_cmp(&b.s).then(a.key.cmp(&b.key)));
        let ypos = |pos: i32| -> f64 { yb - pos as f64 * GS / 2.0 };
        // rests: the gaps in every bar
        for bar in 0..sys_bars {
            let (ba, bz) = (b0 + bar as f64 * 4.0, b0 + bar as f64 * 4.0 + 4.0);
            let mut iv: Vec<(f64, f64)> = pieces.iter().filter(|q| q.e > ba && q.s < bz).map(|q| (q.s.max(ba), q.e.min(bz))).collect();
            iv.sort_by(|a, b| a.0.total_cmp(&b.0));
            let mut cur = ba;
            let mut gaps: Vec<(f64, f64)> = vec![];
            for (a, z) in iv {
                if a > cur + 0.49 {
                    gaps.push((cur, a));
                }
                cur = cur.max(z);
            }
            if bz > cur + 0.49 {
                gaps.push((cur, bz));
            }
            for (a, z) in gaps {
                let mut at = a;
                let whole_bar = (a - ba).abs() < 1e-6 && (z - bz).abs() < 1e-6;
                while z - at > 0.49 {
                    let rem = z - at;
                    let (len, kind) = if whole_bar { (4.0, 0u8) } else if rem >= 4.0 - 1e-6 && (at - ba).abs() < 1e-6 { (4.0, 0) } else if rem >= 2.0 - 1e-6 && ((at - ba) % 2.0).abs() < 1e-6 { (2.0, 1) } else if rem >= 1.0 - 1e-6 { (1.0, 2) } else { (0.5, 3) };
                    let rx_ = if whole_bar { gx + (bar as f64 + 0.5) * bar_w } else { x_of(at) + 4.0 };
                    s.push_str(&rest_svg(kind, rx_, *yb, &hexc(mix(th.ink, th.paper, 0.15))));
                    at += len;
                }
            }
        }
        // heads and groups
        let mut groups: Vec<Group> = vec![];
        let mut i = 0;
        let mut acc_state: std::collections::HashMap<(i32, i32), bool> = std::collections::HashMap::new();
        while i < pieces.len() {
            let mut j = i;
            while j + 1 < pieces.len() && (pieces[j + 1].s - pieces[i].s).abs() < 0.01 {
                j += 1;
            }
            let grp: Vec<Piece> = pieces[i..=j].to_vec();
            let bar_idx = ((grp[0].s - b0) / 4.0).floor() as i32;
            let x = x_of(grp[0].s);
            let min_len = grp.iter().map(|q| q.e - q.s).fold(f64::MAX, f64::min);
            let kind = classify(min_len).0;
            let avg = grp.iter().map(|q| q.pos as f64).sum::<f64>() / grp.len() as f64;
            let up = avg < 4.0;
            // second intervals swap sides
            let mut heads: Vec<(f64, f64, Piece)> = vec![];
            let mut sorted = grp.clone();
            sorted.sort_by_key(|q| q.pos);
            let mut last_pos = -1000i32;
            let mut last_shift = false;
            for q in sorted {
                let shift = q.pos - last_pos == 1 && !last_shift;
                last_pos = q.pos;
                last_shift = shift;
                let hx = if shift { x + rx * 1.8 * if up { 1.0 } else { -1.0 } } else { x };
                heads.push((hx, ypos(q.pos), q));
            }
            // heads, ledger lines, accidentals, dots
            for (hx, hy, q) in &heads {
                c.nts.borrow_mut().push((c.page.get(), NoteGeo { track: ti, key: q.key, start: q.s, x: *hx, y: *hy }));
                let len = q.e - q.s;
                let (_, dots) = classify(len);
                let open = len >= 2.0 - 1e-6;
                if !drums {
                    // ledger lines
                    for pp in (q.pos..0).filter(|p| p % 2 == 0).chain((10..=q.pos).filter(|p| p % 2 == 0)) {
                        s.push_str(&format!("<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{ink}\" stroke-width=\"1\"/>\n", hx - rx * 1.5, ypos(pp), hx + rx * 1.5, ypos(pp)));
                    }
                    // accidental: shown once per bar for a staff position, like real notation
                    let (_, sharp, _) = spell(q.key);
                    let key_state = (bar_idx, dn(q.key));
                    let prev = acc_state.insert(key_state, sharp).unwrap_or(false);
                    if sharp != prev && !q.tie_in {
                        if sharp {
                            s.push_str(&sharp_mark(hx - rx - 6.0, *hy, GS * 1.5, &ink));
                        } else {
                            s.push_str(&format!("<path d=\"M{:.1} {:.1}v{:.1}m0 {:.1}l{:.1} {:.1}m0 {:.1}v{:.1}m{:.1} {:.1}l{:.1} {:.1}\" stroke=\"{ink}\" stroke-width=\"1\" fill=\"none\"/>\n", hx - rx - 8.0, hy - GS, GS * 1.7, -GS * 0.8, GS * 0.7, -GS * 0.25, GS * 0.5, -GS * 1.0, -GS * 0.0, GS * 0.6, GS * 0.7, -GS * 0.25));
                        }
                    }
                }
                let (fill, stroke) = tile_fill(o, th, q.key, tcol);
                let op = if drums { 0.6 + 0.4 * q.vel.clamp(0.0, 1.0) as f64 } else { 1.0 };
                if drums {
                    let shape = perc_pos(q.key).1;
                    let col = hexc(if matches!(o.scheme, Scheme::Ink) { th.ink } else { DRUMS[drum_row(q.key)].1 });
                    match shape {
                        "x" | "xo" => {
                            s.push_str(&format!("<path d=\"M{:.1} {:.1}l{:.1} {:.1}M{:.1} {:.1}l{:.1} {:.1}\" stroke=\"{col}\" stroke-width=\"1.8\" opacity=\"{op:.2}\"/>\n", hx - rx * 0.8, hy - ry * 1.1, rx * 1.6, ry * 2.2, hx + rx * 0.8, hy - ry * 1.1, -rx * 1.6, ry * 2.2));
                            if shape == "xo" {
                                s.push_str(&format!("<circle cx=\"{hx:.1}\" cy=\"{:.1}\" r=\"2.2\" fill=\"none\" stroke=\"{col}\" stroke-width=\"1\"/>\n", hy - GS * 1.2));
                            }
                        }
                        "d" => s.push_str(&format!("<path d=\"M{:.1} {:.1} l{:.1} {:.1} l{:.1} {:.1} l{:.1} {:.1}z\" fill=\"{col}\" opacity=\"{op:.2}\"/>\n", hx - rx, hy, rx, -ry * 1.3, rx, ry * 1.3, -rx, ry * 1.3)),
                        _ => s.push_str(&format!("<ellipse cx=\"{hx:.1}\" cy=\"{hy:.1}\" rx=\"{rx:.1}\" ry=\"{ry:.1}\" transform=\"rotate(-18 {hx:.1} {hy:.1})\" fill=\"{col}\" opacity=\"{op:.2}\"/>\n")),
                    }
                } else {
                    // solid colour heads only (no outline, no hollow); a long note keeps a darker core so it still reads as open
                    let _ = stroke;
                    s.push_str(&format!(
                        "<ellipse cx=\"{hx:.1}\" cy=\"{hy:.1}\" rx=\"{rx:.1}\" ry=\"{ry:.1}\" transform=\"rotate(-18 {hx:.1} {hy:.1})\" fill=\"{}\" fill-opacity=\"{op:.2}\"/>\n",
                        hexc(fill)
                    ));
                    if open {
                        s.push_str(&format!(
                            "<ellipse cx=\"{hx:.1}\" cy=\"{hy:.1}\" rx=\"{:.1}\" ry=\"{:.1}\" transform=\"rotate(-18 {hx:.1} {hy:.1})\" fill=\"{}\" fill-opacity=\"0.55\"/>\n",
                            rx * 0.5, ry * 0.45, hexc(mix(fill, th.paper, 0.7))
                        ));
                    }
                    if false {
                        let lc = if open || matches!(o.scheme, Scheme::Ink) { ink.clone() } else { "#10131a".to_string() };
                        s.push_str(&text(*hx, hy + GS * 0.3, GS * 0.95, &lc, "middle", &c.fam_note, NAMES[(q.key % 12) as usize], ""));
                    }
                }
                for d in 0..dots {
                    let dy = if q.pos % 2 == 0 { hy - GS * 0.5 } else { *hy };
                    s.push_str(&format!("<circle cx=\"{:.1}\" cy=\"{:.1}\" r=\"1.5\" fill=\"{ink}\"/>\n", hx + rx + 4.0 + d as f64 * 4.0, dy));
                }
                // ties
                if q.tie_out {
                    let bx = gx + (((q.s - b0) / 4.0).floor() + 1.0) * bar_w;
                    let dir = if up { 1.0 } else { -1.0 };
                    s.push_str(&format!("<path d=\"M{:.1} {:.1} Q{:.1} {:.1} {:.1} {:.1}\" fill=\"none\" stroke=\"{ink}\" stroke-width=\"1.2\"/>\n", hx + rx, hy + dir * 3.0, (hx + bx) * 0.5, hy + dir * 9.0, bx - 3.0, hy + dir * 3.0));
                }
                if q.tie_in {
                    let bx = gx + ((q.s - b0) / 4.0).floor() * bar_w;
                    let dir = if up { 1.0 } else { -1.0 };
                    s.push_str(&format!("<path d=\"M{:.1} {:.1} Q{:.1} {:.1} {:.1} {:.1}\" fill=\"none\" stroke=\"{ink}\" stroke-width=\"1.2\"/>\n", bx + 3.0, hy + dir * 3.0, (hx + bx) * 0.5, hy + dir * 9.0, hx - rx, hy + dir * 3.0));
                }
            }
            // the stem
            let ys: Vec<f64> = heads.iter().map(|h| h.1).collect();
            let (ymin, ymax) = (ys.iter().cloned().fold(f64::MAX, f64::min), ys.iter().cloned().fold(f64::MIN, f64::max));
            let main_x = heads.iter().find(|h| (h.0 - x).abs() < 0.01).map(|h| h.0).unwrap_or(x);
            let len = GS * 3.5;
            let mid = yb - GS * 2.0;
            let (stem_x, y_from, mut y_to) = if up { (main_x + rx * 0.92, ymax, ymin - len) } else { (main_x - rx * 0.92, ymin, ymax + len) };
            // a stem reaches at least the middle line
            if up && y_to > mid {
                y_to = mid;
            }
            if !up && y_to < mid {
                y_to = mid;
            }
            groups.push(Group { x, s: grp[0].s, heads, up, kind, stem_x, y_from, y_to });
            i = j + 1;
        }
        // beams: runs of flagged groups inside one beat
        let mut gi = 0;
        let mut beamed = vec![false; groups.len()];
        while gi < groups.len() {
            let mut gj = gi;
            let beat = groups[gi].s.floor();
            while gj + 1 < groups.len() && groups[gj + 1].kind >= 3 && groups[gi].kind >= 3 && groups[gj + 1].s.floor() == beat && groups[gj + 1].s < groups[gj].s + 1.0 {
                gj += 1;
            }
            if gj > gi {
                let ups = (gi..=gj).filter(|k| groups[*k].up).count();
                let up = ups * 2 >= gj - gi + 1;
                let end_y = if up { (gi..=gj).map(|k| groups[k].y_to).fold(f64::MAX, f64::min) } else { (gi..=gj).map(|k| groups[k].y_to).fold(f64::MIN, f64::max) };
                for k in gi..=gj {
                    let g = &mut groups[k];
                    g.up = up;
                    let ys: Vec<f64> = g.heads.iter().map(|h| h.1).collect();
                    let main_x = g.heads.iter().find(|h| (h.0 - g.x).abs() < 0.01).map(|h| h.0).unwrap_or(g.x);
                    g.stem_x = if up { main_x + rx * 0.92 } else { main_x - rx * 0.92 };
                    g.y_from = if up { ys.iter().cloned().fold(f64::MIN, f64::max) } else { ys.iter().cloned().fold(f64::MAX, f64::min) };
                    g.y_to = end_y;
                    beamed[k] = true;
                }
                let (xa, xb) = (groups[gi].stem_x, groups[gj].stem_x);
                let dir = if up { 1.0 } else { -1.0 };
                s.push_str(&format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"3\" fill=\"{ink}\"/>\n", xa - 0.8, if up { end_y } else { end_y - 3.0 }, xb - xa + 1.6));
                // second beam over the runs of sixteenths
                let mut k = gi;
                while k <= gj {
                    if groups[k].kind >= 4 {
                        let mut m = k;
                        while m < gj && groups[m + 1].kind >= 4 {
                            m += 1;
                        }
                        let (xs, xe) = if m > k { (groups[k].stem_x, groups[m].stem_x) } else if k > gi { (groups[k].stem_x - 6.0, groups[k].stem_x) } else { (groups[k].stem_x, groups[k].stem_x + 6.0) };
                        s.push_str(&format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"{:.1}\" height=\"3\" fill=\"{ink}\"/>\n", xs - 0.8, end_y + dir * 4.6 - if up { 0.0 } else { 3.0 }, xe - xs + 1.6));
                        k = m + 1;
                    } else {
                        k += 1;
                    }
                }
            }
            gi = gj + 1;
        }
        for (k, g) in groups.iter().enumerate() {
            if g.kind == 0 {
                continue;
            }
            s.push_str(&format!("<line x1=\"{:.1}\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{ink}\" stroke-width=\"1.6\"/>\n", g.stem_x, g.y_from, g.stem_x, g.y_to));
            if g.kind >= 3 && !beamed[k] {
                let n = (g.kind - 2) as usize;
                for f in 0..n {
                    let dy = f as f64 * 4.0 * if g.up { 1.0 } else { -1.0 };
                    let (y, sgn) = (g.y_to + dy, if g.up { 1.0 } else { -1.0 });
                    s.push_str(&format!("<path d=\"M{:.1} {:.1} c{:.1} {:.1} {:.1} {:.1} {:.1} {:.1}\" fill=\"none\" stroke=\"{ink}\" stroke-width=\"1.5\"/>\n", g.stem_x, y, 1.0, sgn * 2.5, 6.5, sgn * 3.5, 4.5, sgn * 9.0));
                }
            }
        }
        let _ = clef;
    }
    // lyrics under the lowest staff
    if !lyrics.is_empty() {
        let ty = staves.last().unwrap().1 + lay.bot + 10.0;
        let mut lastx = f64::MIN;
        for (s0, _e, tx, ws) in lyrics.iter().filter(|x| x.0 >= b0 && x.0 < b1) {
            let x0 = x_of(*s0) - rx;
            if x0 < lastx {
                continue;
            }
            let shown = format!("{}{}", if *ws { "" } else { "-" }, tx);
            s.push_str(&text(x0, ty, 12.0, &ink, "start", &c.fam_note, &shown, ""));
            lastx = x0 + 6.0 + 6.5 * shown.chars().count() as f64;
        }
        height += lyric_h;
    }
    (s, height)
}

fn build_staff(p: &Project, o: &Opts, lyric_fn: &dyn Fn(usize) -> Vec<(f64, f64, String, bool)>) -> Sheet {
    let th = theme(o.dark);
    let (pw, ph) = if o.landscape { (1400.0, 990.0) } else { (990.0, 1400.0) };
    let c = Ctx { o, th, w: pw, h: ph, fam_note: font_family(o.font).to_string(), sys: Default::default(), nts: Default::default(), page: Default::default() };
    let end_beat = p.end_beat().max(4.0);
    let total_bars = (end_beat / 4.0).ceil() as usize;
    let from = o.from_bar.max(1) - 1;
    let to = if o.to_bar == 0 { total_bars } else { o.to_bar.min(total_bars) };
    let bars = to.saturating_sub(from).max(1);
    let secs = p.beats_to_secs(0.0, end_beat);
    let want: Vec<usize> = if o.tracks.is_empty() { (0..p.tracks.len()).collect() } else { o.tracks.clone() };
    let mut tracks: Vec<(usize, Vec<N>, bool, Vec<(f64, f64, String, bool)>)> = vec![];
    for ti in want {
        let Some(t) = p.tracks.get(ti) else { continue };
        if !matches!(t.kind, TrackKind::Instrument) {
            continue;
        }
        let notes: Vec<N> = track_notes(t).into_iter().filter(|n| n.e > from as f64 * 4.0 && n.s < to as f64 * 4.0).collect();
        if notes.is_empty() {
            continue;
        }
        let ly = if o.lyrics { lyric_fn(ti) } else { vec![] };
        tracks.push((ti, notes, t.inst_kind == "drums", ly));
    }
    let gx = 40.0 + 124.0 + 52.0;
    // bars per row: as asked, but never so many that the closest notes (sixteenths, not grace-note flams) would overlap
    let mut min_gap = 4.0f64;
    for (_, notes, _, _) in &tracks {
        let mut on: Vec<f64> = notes.iter().map(|n| n.s).collect();
        on.dedup_by(|a, b| (*a - *b).abs() < 0.02);
        for w in on.windows(2) {
            let d = w[1] - w[0];
            if d > 0.1 {
                min_gap = min_gap.min(d);
            }
        }
    }
    let per_row_fit = (((pw - 40.0 - gx) / (4.0 / min_gap.max(0.25) * 19.0)).floor() as usize).max(1);
    let per_row = o.bars_per_row.min(6).min(per_row_fit).max(1);
    let bar_w = (pw - 40.0 - gx) / per_row as f64;
    // the tracks that play in bars [b0, b1) (empty staves are left out of a system, as in a printed score) and the height each needs there
    let visible = |b0: f64, b1: f64| -> Vec<(usize, f64)> {
        let mut v: Vec<(usize, f64)> = vec![];
        for (k, (_, notes, drums, ly)) in tracks.iter().enumerate() {
            let sys: Vec<N> = notes.iter().filter(|n| n.e > b0 && n.s < b1).cloned().collect();
            if sys.is_empty() {
                continue;
            }
            let mut h = layout(notes, &sys, *drums).height();
            if ly.iter().any(|x| x.0 >= b0 && x.0 < b1) {
                h += 20.0;
            }
            v.push((k, h));
        }
        if v.is_empty() && !tracks.is_empty() {
            v.push((0, layout(&tracks[0].1, &[], tracks[0].2).height()));
        }
        v
    };
    let mut pages: Vec<Page> = vec![];
    let mut cur = Page { body: String::new() };
    let mut y = 110.0 + 96.0;
    let top_first = y;
    let mut first = true;
    let mut sys_start = from;
    let mut first_system = true;
    // a row of bar numbers (and tempo marks) over a system; repeated when a system continues on the next page
    let bar_marks = |body: &mut String, y: f64, sys_start: usize, sys_bars: usize| {
        let b0 = sys_start as f64 * 4.0;
        for b in 0..sys_bars {
            let (sz, col, st) = if b == 0 { (10.0, hexc(c.th.ink), "font-style=\"italic\" font-weight=\"600\"") } else { (8.5, hexc(mix(c.th.ink, c.th.paper, 0.5)), "") };
            body.push_str(&text(gx + b as f64 * bar_w + 3.0, y + 8.0, sz, &col, "start", "'Segoe UI',Arial,sans-serif", &format!("{}", sys_start + b + 1), st));
        }
        for (tb, bpm) in &p.tempo_map {
            if *tb >= b0 && *tb < b0 + sys_bars as f64 * 4.0 {
                body.push_str(&text(gx + (tb - b0) / 4.0 * bar_w + 14.0, y - 2.0, 9.0, "#d66a2a", "start", "'Segoe UI',Arial,sans-serif", &format!("{} BPM", bpm.round()), "font-weight=\"bold\""));
            }
        }
    };
    let limit = ph - 40.0;
    while sys_start < to {
        let sys_bars = per_row.min(to - sys_start);
        let b0 = sys_start as f64 * 4.0;
        let vis = visible(b0, b0 + sys_bars as f64 * 4.0);
        let first_h = vis.first().map(|v| v.1).unwrap_or(0.0);
        if y + 16.0 + first_h > limit && y > (if first { top_first } else { 70.0 }) + 1.0 {
            pages.push(std::mem::replace(&mut cur, Page { body: String::new() }));
            c.page.set(pages.len());
            first = false;
            y = 70.0;
        }
        bar_marks(&mut cur.body, y, sys_start, sys_bars);
        let mut ly = y + 16.0;
        let mut page_top = if first { top_first } else { 70.0 };
        let last_system = sys_start + sys_bars >= to;
        for (vi, (k, hk)) in vis.iter().enumerate() {
            let (ti, notes, drums, lyr) = &tracks[*k];
            // a system taller than the page continues on the next one: nothing is ever drawn past the margin
            if vi > 0 && ly + hk > limit && ly > page_top + 20.0 {
                pages.push(std::mem::replace(&mut cur, Page { body: String::new() }));
                c.page.set(pages.len());
                first = false;
                y = 70.0;
                page_top = 70.0;
                bar_marks(&mut cur.body, y, sys_start, sys_bars);
                ly = y + 16.0;
            }
            let (body, h) = staff_block(&c, p, *ti, notes, *drums, lyr, b0, sys_bars, gx, bar_w, ly, first_system, last_system);
            cur.body.push_str(&body);
            ly += hk.max(h) + 8.0;
        }
        y = ly + 14.0;
        sys_start += sys_bars;
        first_system = false;
    }
    pages.push(cur);
    let n_pages = pages.len();
    let mut out = vec![];
    for (i, pg) in pages.into_iter().enumerate() {
        let mut s = String::new();
        s.push_str(&format!("<rect width=\"{pw}\" height=\"{ph}\" fill=\"{}\"/>\n", hexc(c.th.paper)));
        s.push_str(&header(&c, p, i == 0, i + 1, n_pages, bars, secs));
        s.push_str(&pg.body);
        s.push_str(&format!("<line x1=\"40\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"0.8\"/>\n", ph - 28.0, pw - 40.0, ph - 28.0, hexc(c.th.faint)));
        s.push_str(&text(40.0, ph - 12.0, 9.0, &hexc(mix(c.th.ink, c.th.paper, 0.45)), "start", "'Segoe UI',Arial,sans-serif", &format!("{} - page {} / {}", p.name, i + 1, n_pages), ""));
        out.push(s);
    }
    let mut geo: Vec<PageGeo> = (0..n_pages).map(|_| PageGeo::default()).collect();
    for (pg, g) in c.sys.take() {
        if let Some(x) = geo.get_mut(pg) {
            x.systems.push(g);
        }
    }
    for (pg, g) in c.nts.take() {
        if let Some(x) = geo.get_mut(pg) {
            x.notes.push(g);
        }
    }
    Sheet { pages: out, bars, tracks: tracks.len(), geo }
}
