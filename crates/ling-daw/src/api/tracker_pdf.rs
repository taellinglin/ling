//! Export Tracker: the tracker pages (rows down, channels across, like OpenMPT) as printable pages. Every pattern (a block of rows) becomes
//! one page, wider songs continue on further pages of the same pattern; each page is an SVG with the fonts embedded by the caller, and
//! `html_to_pdf` turns them into one PDF. Cells read `C-5 01 v32`: note, instrument (the track number, hex) and volume (0..64); `|`
//! holds an earlier note, `===` ends it, the dots are empty rows.
use super::tracker_api::{page_cells, Layout};
use crate::model::*;

#[derive(Clone, Debug)]
pub struct TOpts {
    pub rpb: f64,
    pub rows: usize,
    pub hex: bool,
    /// 0 English (monospace), 1 Daemon, 2 Chesilin
    pub font: u8,
    pub dark: bool,
    pub landscape: bool,
    pub title: String,
    /// 1-based first / last pattern to print (0 = to the end)
    pub from: usize,
    pub to: usize,
    /// keep channels (and patterns) that never play
    pub empty: bool,
    /// tracks to print (empty = all)
    pub tracks: Vec<usize>,
}

impl TOpts {
    pub fn parse(s: &str) -> TOpts {
        let mut o = TOpts { rpb: 4.0, rows: 64, hex: true, font: 0, dark: false, landscape: true, title: String::new(), from: 1, to: 0, empty: false, tracks: vec![] };
        for kv in s.split(';') {
            let Some((k, v)) = kv.split_once('=') else { continue };
            let (k, v) = (k.trim(), v.trim());
            match k {
                "rpb" => o.rpb = v.parse::<f64>().unwrap_or(4.0).clamp(1.0, 16.0),
                "rows" => o.rows = v.parse::<usize>().unwrap_or(64).clamp(8, 256),
                "hex" => o.hex = v != "0",
                "font" => o.font = match v { "daemon" => 1, "chesilin" => 2, _ => 0 },
                "dark" => o.dark = v == "1",
                "landscape" => o.landscape = v != "0",
                "title" => o.title = v.to_string(),
                "from" => o.from = v.parse().unwrap_or(1usize).max(1),
                "to" => o.to = v.parse().unwrap_or(0),
                "empty" => o.empty = v == "1",
                "tracks" => o.tracks = v.split(',').filter_map(|x| x.trim().parse().ok()).collect(),
                _ => {}
            }
        }
        o
    }
}

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
fn note_name(k: i64) -> String {
    const N: [&str; 12] = ["C-", "C#", "D-", "D#", "E-", "F-", "F#", "G-", "G#", "A-", "A#", "B-"];
    format!("{}{}", N[(k % 12) as usize], k / 12)
}
fn num(v: usize, hex: bool, w: usize) -> String {
    if hex { format!("{v:0w$X}") } else { format!("{v:0w$}") }
}

/// every page of the export as SVG drawings (without the outer <svg>)
pub(super) fn render_pages(p: &Project, l: &Layout, o: &TOpts) -> Vec<String> {
    let (pw, ph) = if o.landscape { (1400.0, 990.0) } else { (990.0, 1400.0) };
    let (paper, ink) = if o.dark { (0x0e1118u32, 0xe8edf5u32) } else { (0xfbfaf6, 0x1a1f2b) };
    let dim = mix(ink, paper, 0.55);
    let (c_inst, c_vol, c_off, c_smp, c_bar) = if o.dark { (0xe0b030, 0x58d084, 0xff7b7b, 0xc58cf0, 0x7d88ff) } else { (0x9a7400, 0x1f8a45, 0xc02f33, 0x7a3fb0, 0x4b3fc4) };
    let fam = match o.font {
        1 => "'LSXDaemon','Consolas','Courier New',monospace",
        2 => "'LSXChesilin','Consolas','Courier New',monospace",
        _ => "Consolas,'Cascadia Mono','Courier New',monospace",
    };
    let sans = "'Segoe UI',Arial,sans-serif";
    // channels worth printing, in column order
    let chans: Vec<usize> = (0..l.chans.len())
        .filter(|i| {
            let c = &l.chans[*i];
            (o.tracks.is_empty() || o.tracks.contains(&c.track)) && (o.empty || !c.evs.is_empty())
        })
        .collect();
    if chans.is_empty() {
        return vec![];
    }
    // geometry: the rows fill the page, the font follows the row height
    let top = 108.0;
    let hdr = 40.0;
    let rh = ((ph - top - hdr - 46.0) / o.rows as f64).min(15.0);
    let fs = (rh * 0.8).max(5.5);
    let rnw = fs * 3.6;
    let cw = fs * 7.9;
    let per_page = (((pw - 80.0 - rnw) / cw).floor() as usize).max(1);
    let groups: Vec<&[usize]> = chans.chunks(per_page).collect();
    let rows_total = (l.end * o.rpb).ceil() as usize;
    let n_patterns = (rows_total + o.rows - 1) / o.rows.max(1);
    let n_patterns = n_patterns.max(1);
    let from = o.from.max(1) - 1;
    let to = if o.to == 0 { n_patterns } else { o.to.min(n_patterns) };
    let mut pages: Vec<String> = vec![];
    let tsig = 4.0 * o.rpb; // rows in a bar
    for pat in from..to {
        let beat0 = (pat * o.rows) as f64 / o.rpb;
        // a pattern nothing plays in is left out, unless asked for
        let any = chans.iter().any(|ci| l.chans[*ci].evs.iter().any(|e| e.e > beat0 && e.s < beat0 + o.rows as f64 / o.rpb));
        if !any && !o.empty {
            continue;
        }
        for (gi, grp) in groups.iter().enumerate() {
            let mut s = String::new();
            s.push_str(&format!("<rect width=\"{pw}\" height=\"{ph}\" fill=\"{}\"/>\n", hexc(paper)));
            // header
            let title = if o.title.is_empty() { p.name.clone() } else { o.title.clone() };
            s.push_str(&format!("<text x=\"40\" y=\"56\" font-size=\"30\" fill=\"{}\" font-family=\"{fam}\">{}</text>\n", hexc(ink), esc(&title)));
            let bar0 = beat0 / 4.0 + 1.0;
            let bar1 = (beat0 + o.rows as f64 / o.rpb) / 4.0;
            let mut sub = format!("PATTERN {}   rows {}-{}   bars {:.0}-{:.0}   {} BPM   {} rows / beat", num(pat, o.hex, 2), num(0, o.hex, 2), num(o.rows - 1, o.hex, 2), bar0, bar1.ceil(), p.bpm.round(), o.rpb);
            if groups.len() > 1 {
                sub.push_str(&format!("   channels {}-{} of {}", chans.iter().position(|c| c == &grp[0]).unwrap_or(0) + 1, chans.iter().position(|c| c == grp.last().unwrap()).unwrap_or(0) + 1, chans.len()));
            }
            s.push_str(&format!("<text x=\"42\" y=\"80\" font-size=\"12\" fill=\"{}\" font-family=\"{sans}\" letter-spacing=\"1.2\">{}</text>\n", hexc(dim), esc(&sub)));
            s.push_str(&format!("<line x1=\"40\" y1=\"92\" x2=\"{:.1}\" y2=\"92\" stroke=\"{}\" stroke-width=\"1.4\"/>\n", pw - 40.0, hexc(ink)));
            let gx = 40.0;
            let w_grid = rnw + grp.len() as f64 * cw;
            // channel headers
            for (k, ci) in grp.iter().enumerate() {
                let c = &l.chans[*ci];
                let t = &p.tracks[c.track];
                let hx = gx + rnw + k as f64 * cw;
                s.push_str(&format!("<rect x=\"{:.1}\" y=\"{top:.1}\" width=\"{:.1}\" height=\"4\" fill=\"{}\"/>\n", hx + 1.0, cw - 4.0, hexc(t.color & 0xFFFFFF)));
                let (name, sub) = if c.voice == 0 { (t.name.clone(), format!("ins {}", num(c.track + 1, true, 2))) } else { (String::new(), format!("+ voice {}", c.voice + 1)) };
                let max_chars = (((cw - 6.0) / (fs * 0.62)).floor() as usize).max(4);
                let nm: String = if name.chars().count() > max_chars { format!("{}…", name.chars().take(max_chars - 1).collect::<String>()) } else { name };
                if !nm.is_empty() {
                    s.push_str(&format!("<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"{:.1}\" fill=\"{}\" font-family=\"{sans}\" font-weight=\"600\">{}</text>\n", hx + 3.0, top + 18.0, fs * 0.95, hexc(ink), esc(&nm)));
                }
                s.push_str(&format!("<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"{:.1}\" fill=\"{}\" font-family=\"{sans}\">{}</text>\n", hx + 3.0, top + 31.0, fs * 0.8, hexc(dim), esc(&sub)));
            }
            // the rows
            let cells = page_cells(l, beat0, o.rows, o.rpb, grp);
            let y0 = top + hdr;
            for r in 0..o.rows {
                let ry = y0 + r as f64 * rh;
                let rr = r as f64;
                if rr % tsig < 0.5 {
                    s.push_str(&format!("<rect x=\"{gx}\" y=\"{ry:.1}\" width=\"{w_grid:.1}\" height=\"{rh:.1}\" fill=\"{}\" fill-opacity=\"0.13\"/>\n", hexc(c_bar)));
                } else if rr % o.rpb < 0.5 {
                    s.push_str(&format!("<rect x=\"{gx}\" y=\"{ry:.1}\" width=\"{w_grid:.1}\" height=\"{rh:.1}\" fill=\"{}\" fill-opacity=\"0.07\"/>\n", hexc(ink)));
                }
                let tb = ry + rh * 0.5 + fs * 0.34;
                s.push_str(&format!("<text x=\"{:.1}\" y=\"{tb:.1}\" font-size=\"{fs:.1}\" fill=\"{}\" font-family=\"{fam}\">{}</text>\n", gx + 3.0, hexc(if rr % tsig < 0.5 { c_bar } else if rr % o.rpb < 0.5 { ink } else { dim }), num(r, o.hex, 2)));
                for (k, ci) in grp.iter().enumerate() {
                    let (key, vel, hold) = cells[r * grp.len() + k];
                    let cx = gx + rnw + k as f64 * cw + 3.0;
                    let text = |x: f64, col: u32, t: &str| format!("<text x=\"{x:.1}\" y=\"{tb:.1}\" font-size=\"{fs:.1}\" fill=\"{}\" font-family=\"{fam}\">{}</text>\n", hexc(col), esc(t));
                    if key >= 0 {
                        if key == 128 {
                            s.push_str(&text(cx, c_smp, "Smp"));
                        } else {
                            s.push_str(&text(cx, ink, &note_name(key)));
                            s.push_str(&text(cx + fs * 2.2, c_inst, &num(l.chans[*ci].track + 1, true, 2)));
                            s.push_str(&text(cx + fs * 4.0, c_vol, &format!("v{:02}", (vel * 64.0).round() as i32)));
                        }
                    } else if hold == 2 {
                        s.push_str(&text(cx, c_off, "==="));
                    } else if hold == 1 {
                        s.push_str(&text(cx + fs * 0.7, dim, "|"));
                    } else {
                        for d in 0..3 {
                            s.push_str(&format!("<rect x=\"{:.1}\" y=\"{:.1}\" width=\"1.6\" height=\"1.6\" fill=\"{}\" fill-opacity=\"0.55\"/>\n", cx + 1.0 + d as f64 * fs * 0.72, ry + rh * 0.55, hexc(dim)));
                        }
                    }
                }
            }
            // column rules and the frame
            for k in 0..=grp.len() {
                let x = gx + rnw + k as f64 * cw - 1.5;
                s.push_str(&format!("<line x1=\"{x:.1}\" y1=\"{y0:.1}\" x2=\"{x:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"0.8\" stroke-opacity=\"0.5\"/>\n", y0 + o.rows as f64 * rh, hexc(dim)));
            }
            s.push_str(&format!("<rect x=\"{gx}\" y=\"{y0:.1}\" width=\"{w_grid:.1}\" height=\"{:.1}\" fill=\"none\" stroke=\"{}\" stroke-width=\"1\"/>\n", o.rows as f64 * rh, hexc(ink)));
            // the key
            s.push_str(&format!("<text x=\"{:.1}\" y=\"{:.1}\" font-size=\"10\" fill=\"{}\" font-family=\"{sans}\">NOTE  INSTRUMENT (track, hex)  VOLUME 0-64      |  held      ===  note ends      Smp  audio clip starts</text>\n", gx, y0 + o.rows as f64 * rh + 16.0, hexc(dim)));
            pages.push(s);
        }
    }
    let n = pages.len();
    for (i, pg) in pages.iter_mut().enumerate() {
        pg.push_str(&format!("<line x1=\"40\" y1=\"{:.1}\" x2=\"{:.1}\" y2=\"{:.1}\" stroke=\"{}\" stroke-width=\"0.8\"/>\n", ph - 28.0, pw - 40.0, ph - 28.0, hexc(mix(ink, paper, 0.8))));
        pg.push_str(&format!("<text x=\"40\" y=\"{:.1}\" font-size=\"9\" fill=\"{}\" font-family=\"{sans}\">{} - tracker - page {} / {}</text>\n", ph - 12.0, hexc(dim), esc(&p.name), i + 1, n));
    }
    pages
}
