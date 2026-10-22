//! Builtins for the sheet-music export (`daw_sheet_*`, see `sheet.rs`).
use super::{Daw, A, Val};
use crate::sheet::{self, Opts};

impl Daw {
    pub(super) fn sheet_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_sheet_scan" => {
                // -> [bars, then (track, name, notes, drums 0/1, has lyrics 0/1)*] for every instrument track that has notes
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let p = &en.project;
                let mut v = vec![Val::Num((p.end_beat().max(4.0) / 4.0).ceil())];
                let lyr: Vec<usize> = crate::karaoke::lyric_tracks(p).into_iter().map(|x| x.0).collect();
                for (i, t) in p.tracks.iter().enumerate() {
                    if !matches!(t.kind, crate::model::TrackKind::Instrument) {
                        continue;
                    }
                    let n: usize = t.clips.iter().map(|c| if let crate::model::ClipData::Midi(m) = &c.data { m.len() } else { 0 }).sum();
                    if n == 0 {
                        continue;
                    }
                    v.extend([Val::Num(i as f64), Val::Str(t.name.clone()), Val::Num(n as f64), Val::Num((t.inst_kind == "drums") as u8 as f64), Val::Num(lyr.contains(&i) as u8 as f64)]);
                }
                ok!(Val::List(v))
            }
            "daw_sheet_write" => {
                // (folder, base name, options "k=v;...", fonts folder) -> the files written; [] when there is nothing to draw
                let (dir, base, opts, fdir) = (a.s(0), a.s(1), a.s(2), a.s(3));
                let o = Opts::parse(&opts);
                let sheet = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let p = &en.project;
                    sheet::build(p, &o, &|t| crate::karaoke::track_syllables(p, t))
                };
                if sheet.tracks == 0 {
                    self.status = "Sheet music: no tracks with notes in that range".into();
                    return ok!(Val::List(vec![]));
                }
                let dirp = std::path::Path::new(&dir);
                std::fs::create_dir_all(dirp).map_err(|e| format!("{dir}: {e}"))?;
                let mut fonts: Vec<(&str, Vec<u8>, &str)> = vec![];
                if let Ok(d) = std::fs::read(std::path::Path::new(&fdir).join("Daemon.otf")) {
                    fonts.push(("LSXDaemon", d, "font/otf"));
                }
                if let Ok(d) = std::fs::read(std::path::Path::new(&fdir).join("Chesilin.ttf")) {
                    fonts.push(("LSXChesilin", d, "font/ttf"));
                }
                let css = sheet::font_css(&fonts);
                let mut out: Vec<Val> = vec![];
                let title = if o.title.is_empty() { base.clone() } else { o.title.clone() };
                let n = sheet.pages.len();
                for (i, pg) in sheet.pages.iter().enumerate() {
                    let file = if n == 1 { format!("{base}.svg") } else { format!("{base}_p{}.svg", i + 1) };
                    std::fs::write(dirp.join(&file), sheet::svg_page(pg, o.landscape, &css)).map_err(|e| format!("{file}: {e}"))?;
                    out.push(Val::Str(file));
                }
                let file = format!("{base}.html");
                std::fs::write(dirp.join(&file), sheet::html(&title, &sheet.pages, o.landscape, &css)).map_err(|e| format!("{file}: {e}"))?;
                out.push(Val::Str(file.clone()));
                let mode = a.s(4);
                let mut pdf_err = String::new();
                if mode.contains("xml") {
                    // the same notes as a MusicXML score for MuseScore, Sibelius, Dorico ...
                    let (xml, n) = {
                        let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                        let p = &en.project;
                        crate::musicxml::musicxml(p, &crate::musicxml::MxOpts { tracks: o.tracks.clone(), from_bar: o.from_bar, to_bar: o.to_bar, title: o.title.clone() }, &|t| crate::karaoke::track_syllables(p, t))
                    };
                    if n > 0 {
                        let name = format!("{base}.musicxml");
                        std::fs::write(dirp.join(&name), xml).map_err(|e| format!("{name}: {e}"))?;
                        out.push(Val::Str(name));
                    }
                }
                if mode.contains("pdf") || mode.contains("parts") {
                    // the whole score as one PDF, and / or one PDF per part (each part is its own score of a single track)
                    if mode.contains("pdf") {
                        let name = format!("{base}.pdf");
                        match html_to_pdf(&dirp.join(&file), &dirp.join(&name)) {
                            Ok(()) => out.push(Val::Str(name)),
                            Err(e) => pdf_err = e,
                        }
                    }
                    if mode.contains("parts") && pdf_err.is_empty() {
                        let ids: Vec<usize> = if o.tracks.is_empty() {
                            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                            (0..en.project.tracks.len()).collect()
                        } else {
                            o.tracks.clone()
                        };
                        for ti in ids {
                            let mut po = o.clone();
                            po.tracks = vec![ti];
                            let (part, tname) = {
                                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                                let p = &en.project;
                                let Some(t) = p.tracks.get(ti) else { continue };
                                po.title = format!("{} - {}", title, t.name);
                                (sheet::build(p, &po, &|t| crate::karaoke::track_syllables(p, t)), t.name.clone())
                            };
                            if part.tracks == 0 {
                                continue;
                            }
                            let safe: String = tname.chars().map(|c| if c.is_alphanumeric() || c == ' ' || c == '-' || c == '_' { c } else { '_' }).collect();
                            let tmp = dirp.join(format!(".{base}_part{ti}.html"));
                            std::fs::write(&tmp, sheet::html(&po.title, &part.pages, po.landscape, &css)).map_err(|e| format!("{}: {e}", tmp.display()))?;
                            let name = format!("{base} - {}.pdf", safe.trim());
                            let r = html_to_pdf(&tmp, &dirp.join(&name));
                            let _ = std::fs::remove_file(&tmp);
                            match r {
                                Ok(()) => out.push(Val::Str(name)),
                                Err(e) => {
                                    pdf_err = e;
                                    break;
                                }
                            }
                        }
                    }
                }
                if !pdf_err.is_empty() {
                    self.status = format!("Sheet music written, but the PDF failed: {pdf_err}");
                    return ok!(Val::List(out));
                }
                self.status = format!("Sheet music: {} bar(s), {} track(s), {} page(s) in {dir}", sheet.bars, sheet.tracks, n);
                ok!(Val::List(out))
            }
            "daw_sheet_png" => {
                // (options, fonts folder, page, pixel width) -> [pages, bars, tracks, width, height, generation, png file]; [] when there is nothing to draw.
                // The page is drawn again only when the project, the options, the page or the width change.
                let (opts, fdir, page, width) = (a.s(0), a.s(1), a.u(2), a.n(3).clamp(200.0, 3200.0) as u32);
                let key = format!("{}|{}|{}|{}|{}", self.rev, opts, fdir, page, width);
                if let Some((_, v)) = self.sheet_png.iter().find(|(k, _)| *k == key) {
                    return ok!(Val::List(v.clone()));
                }
                let o = Opts::parse(&opts);
                let sheet = self.sheet_cached(&opts);
                if sheet.tracks == 0 || sheet.pages.is_empty() {
                    self.sheet_png.push((key, vec![]));
                    return ok!(Val::List(vec![]));
                }
                let pg = page.min(sheet.pages.len() - 1);
                let svg = sheet::svg_page(&sheet.pages[pg], o.landscape, "");
                self.sheet_gen += 1;
                // one file per page, rewritten whenever that page is drawn again
                let file = std::env::temp_dir().join(format!("lsx_sheet_{}_p{}.png", std::process::id(), pg));
                let (w, h) = match render_png(&svg, width, &fdir, &file) {
                    Ok(x) => x,
                    Err(e) => {
                        self.status = format!("Sheet music: {e}");
                        return ok!(Val::List(vec![]));
                    }
                };
                let v = vec![Val::Num(sheet.pages.len() as f64), Val::Num(sheet.bars as f64), Val::Num(sheet.tracks as f64), Val::Num(w as f64), Val::Num(h as f64), Val::Num(self.sheet_gen as f64), Val::Str(file.to_string_lossy().into_owned())];
                self.sheet_png.retain(|(_, old)| old.get(6).map(|p| !matches!((p, v.get(6)), (Val::Str(a), Some(Val::Str(b))) if a == b)).unwrap_or(true));
                self.sheet_png.push((key, v.clone()));
                if self.sheet_png.len() > 8 {
                    self.sheet_png.remove(0);
                }
                ok!(Val::List(v))
            }
            "daw_sheet_geo" => {
                // (options, page) -> [page width, page height, systems, (track, b0, b1, x0, bar width, y top, y bottom, staves, (clef, y bottom line, diatonic step)*)*,
                // notes, (track, key, start beat, x, y)*]: where the staves and noteheads of a page are, in page units; [] for the grid style / no such page
                let (opts, page) = (a.s(0), a.u(1));
                let o = Opts::parse(&opts);
                let sheet = self.sheet_cached(&opts);
                let Some(g) = sheet.geo.get(page) else { return ok!(Val::List(vec![])) };
                let (pw, ph) = if o.landscape { (1400.0, 990.0) } else { (990.0, 1400.0) };
                let n = |x: f64| Val::Num(x);
                let mut v = vec![n(pw), n(ph), n(g.systems.len() as f64)];
                for s in &g.systems {
                    v.extend([n(s.track as f64), n(s.b0), n(s.b1), n(s.x0), n(s.bar_w), n(s.y0), n(s.y1), n(s.staves.len() as f64)]);
                    for (c, y, d) in &s.staves {
                        v.extend([n(*c as f64), n(*y), n(*d as f64)]);
                    }
                }
                v.push(n(g.notes.len() as f64));
                for q in &g.notes {
                    v.extend([n(q.track as f64), n(q.key as f64), n(q.start), n(q.x), n(q.y)]);
                }
                ok!(Val::List(v))
            }
            "daw_sheet_edit" => {
                // (track, op, key, start beat, value) -> 1 when something changed. The note is found by its key and a beat inside it, in any clip of the track
                // (a looping clip edits its source note). op: 0 delete, 1 transpose by `value` semitones, 2 move by `value` beats,
                // 3 set the length to `value` beats, 4 add a note of length `value` at that beat (a clip is made when none covers it)
                let (ti, op, key, start, val) = (a.u(0), a.n(1) as i64, a.n(2).clamp(0.0, 127.0) as u8, a.n(3).max(0.0), a.n(4));
                if !start.is_finite() || !val.is_finite() {
                    return ok!(Val::Num(0.0));
                }
                let mut changed = false;
                {
                    let mut en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let Some(t) = en.project.tracks.get_mut(ti) else { return ok!(Val::Num(0.0)) };
                    if op == 4 {
                        let ci = t.clips.iter().position(|c| matches!(c.data, crate::model::ClipData::Midi(_)) && start >= c.start && start < c.start + c.len);
                        let ci = match ci {
                            Some(i) => i,
                            None => {
                                let s0 = (start / 4.0).floor() * 4.0;
                                t.clips.push(crate::model::Clip::midi(s0, 4.0, "Pattern"));
                                t.clips.len() - 1
                            }
                        };
                        let c = &mut t.clips[ci];
                        if let crate::model::ClipData::Midi(notes) = &mut c.data {
                            let rel = start - c.start;
                            if rel + val.max(0.0625) > c.len {
                                c.len = ((rel + val.max(0.0625)) / 4.0).ceil() * 4.0;
                            }
                            notes.push(crate::model::Note::new(rel, val.max(0.0625), key, 0.8));
                            changed = true;
                        }
                    } else {
                        'find: for c in t.clips.iter_mut() {
                            let (cs, clen, ll) = (c.start, c.len, if c.loop_len >= 0.25 && c.loop_len < c.len { c.loop_len } else { 0.0 });
                            let crate::model::ClipData::Midi(notes) = &mut c.data else { continue };
                            let reps = if ll > 0.0 { ((clen / ll).ceil() as usize).min(512) } else { 1 };
                            for k in 0..reps {
                                let base = cs + k as f64 * ll;
                                let rel = start - base;
                                for i in 0..notes.len() {
                                    let n = &notes[i];
                                    if n.key == key && rel >= n.start - 0.01 && rel < n.start + n.len.max(0.05) + 0.01 {
                                        match op {
                                            0 => {
                                                notes.remove(i);
                                            }
                                            1 => notes[i].key = (n.key as f64 + val.round()).clamp(0.0, 127.0) as u8,
                                            2 => notes[i].start = (n.start + val).max(0.0),
                                            3 => notes[i].len = val.max(0.0625),
                                            _ => break 'find,
                                        }
                                        changed = true;
                                        break 'find;
                                    }
                                }
                            }
                        }
                    }
                }
                if changed {
                    self.touch();
                }
                ok!(Val::Num(changed as u8 as f64))
            }
            "daw_sheet_xml" => {
                // (folder, base name, options) -> the files written: the score as MusicXML (.musicxml) and compressed MusicXML (.mxl) for MuseScore,
                // Sibelius, Dorico, Finale ...; [] when there are no notes in that range
                let (dir, base, opts) = (a.s(0), a.s(1), a.s(2));
                let o = Opts::parse(&opts);
                let (xml, n) = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    let p = &en.project;
                    crate::musicxml::musicxml(p, &crate::musicxml::MxOpts { tracks: o.tracks.clone(), from_bar: o.from_bar, to_bar: o.to_bar, title: o.title.clone() }, &|t| crate::karaoke::track_syllables(p, t))
                };
                if n == 0 {
                    self.status = "MusicXML: no notes in that range".into();
                    return ok!(Val::List(vec![]));
                }
                let dirp = std::path::Path::new(&dir);
                std::fs::create_dir_all(dirp).map_err(|e| format!("{dir}: {e}"))?;
                let (f1, f2) = (format!("{base}.musicxml"), format!("{base}.mxl"));
                std::fs::write(dirp.join(&f1), &xml).map_err(|e| format!("{f1}: {e}"))?;
                std::fs::write(dirp.join(&f2), mxl(&xml)).map_err(|e| format!("{f2}: {e}"))?;
                self.status = format!("MusicXML written: {f1} and {f2} (open either in MuseScore)");
                ok!(Val::List(vec![Val::Str(f1), Val::Str(f2)]))
            }
            "daw_sheet_musescore_find" => ok!(Val::Str(find_musescore().map(|p| p.to_string_lossy().into_owned()).unwrap_or_default())),
            "daw_sheet_musescore_open" => {
                // open a score file in MuseScore (1 when it was started, 0 when MuseScore is not installed)
                let file = a.s(0);
                // only a score that exists is handed over: nothing to open, nothing started
                if !std::path::Path::new(&file).is_file() {
                    self.status = format!("MuseScore: {file} does not exist");
                    return ok!(Val::Num(0.0));
                }
                let Some(exe) = find_musescore() else {
                    self.status = "MuseScore was not found on this computer; open the .mxl in it yourself (musescore.org)".into();
                    return ok!(Val::Num(0.0));
                };
                let mut cmd = std::process::Command::new(&exe);
                cmd.arg(&file).stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).stdin(std::process::Stdio::null());
                #[cfg(windows)]
                {
                    use std::os::windows::process::CommandExt;
                    cmd.creation_flags(0x00000008 | 0x00000200);
                }
                match cmd.spawn() {
                    Ok(_) => {
                        self.status = "Opened in MuseScore".into();
                        ok!(Val::Num(1.0))
                    }
                    Err(e) => {
                        self.status = format!("{}: {e}", exe.display());
                        ok!(Val::Num(0.0))
                    }
                }
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }

    /// the sheet for these options, built once per project revision
    fn sheet_cached(&mut self, opts: &str) -> std::sync::Arc<sheet::Sheet> {
        let key = format!("{}|{}", self.rev, opts);
        if let Some((k, s)) = &self.sheet_cache {
            if *k == key {
                return s.clone();
            }
        }
        let o = Opts::parse(opts);
        let eng = self.out.engine.clone();
        let built = {
            let en = eng.lock().unwrap_or_else(|p| p.into_inner());
            let p = &en.project;
            sheet::build(p, &o, &|t| crate::karaoke::track_syllables(p, t))
        };
        let a = std::sync::Arc::new(built);
        self.sheet_cache = Some((key, a.clone()));
        a
    }
}

/// MuseScore (4, Studio, 3) in the usual places, or on PATH
pub fn find_musescore() -> Option<std::path::PathBuf> {
    let mut c: Vec<std::path::PathBuf> = vec![];
    for var in ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"] {
        let Some(r) = std::env::var_os(var) else { continue };
        let mut roots = vec![std::path::PathBuf::from(&r)];
        roots.push(std::path::PathBuf::from(&r).join("Programs"));
        for root in roots {
            let Ok(rd) = std::fs::read_dir(&root) else { continue };
            let mut dirs: Vec<std::path::PathBuf> = rd.flatten().map(|e| e.path()).filter(|p| p.file_name().map(|n| n.to_string_lossy().starts_with("MuseScore")).unwrap_or(false)).collect();
            dirs.sort();
            dirs.reverse(); // the newest version first
            for d in dirs {
                let Ok(bin) = std::fs::read_dir(d.join("bin")) else { continue };
                for f in bin.flatten() {
                    let n = f.file_name().to_string_lossy().to_string();
                    if n.starts_with("MuseScore") && n.ends_with(".exe") {
                        c.push(f.path());
                    }
                }
            }
        }
    }
    for p in ["/Applications/MuseScore 4.app/Contents/MacOS/mscore", "/Applications/MuseScore 3.app/Contents/MacOS/mscore", "/usr/bin/musescore", "/usr/bin/mscore"] {
        c.push(p.into());
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            for n in ["mscore4portable", "musescore4", "mscore", "musescore", "MuseScore4.exe"] {
                c.push(d.join(n));
            }
        }
    }
    c.into_iter().find(|p| p.is_file())
}

fn crc32(d: &[u8]) -> u32 {
    let mut c = 0xFFFF_FFFFu32;
    for b in d {
        c ^= *b as u32;
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

/// compressed MusicXML (.mxl): a zip holding META-INF/container.xml and the score (entries are stored, which every reader accepts)
pub fn mxl(xml: &str) -> Vec<u8> {
    let container = "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<container><rootfiles><rootfile full-path=\"score.xml\" media-type=\"application/vnd.recordare.musicxml+xml\"/></rootfiles></container>\n";
    let files: [(&str, &[u8]); 2] = [("META-INF/container.xml", container.as_bytes()), ("score.xml", xml.as_bytes())];
    let mut out: Vec<u8> = vec![];
    let mut central: Vec<u8> = vec![];
    for (name, data) in files {
        let off = out.len() as u32;
        let crc = crc32(data);
        let mut h = vec![];
        h.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        h.extend_from_slice(&[20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]); // version, flags, stored, time 0, date 1980-01-01
        h.extend_from_slice(&crc.to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(data.len() as u32).to_le_bytes());
        h.extend_from_slice(&(name.len() as u16).to_le_bytes());
        h.extend_from_slice(&0u16.to_le_bytes());
        h.extend_from_slice(name.as_bytes());
        out.extend_from_slice(&h);
        out.extend_from_slice(data);
        central.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        central.extend_from_slice(&[20, 0, 20, 0, 0, 0, 0, 0, 0, 0, 0x21, 0]);
        central.extend_from_slice(&crc.to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(data.len() as u32).to_le_bytes());
        central.extend_from_slice(&(name.len() as u16).to_le_bytes());
        central.extend_from_slice(&[0u8; 8]); // extra, comment, disk, internal attrs
        central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        central.extend_from_slice(&off.to_le_bytes());
        central.extend_from_slice(name.as_bytes());
    }
    let cd_off = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
    out.extend_from_slice(&[0, 0, 0, 0]);
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_off.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

type FontCache = Option<(String, std::sync::Arc<resvg::usvg::fontdb::Database>, Vec<(String, String)>)>;
static FONTS: std::sync::Mutex<FontCache> = std::sync::Mutex::new(None);

/// the system fonts plus the Daemon and Chesilin fonts of `fdir`, loaded once; the pairs map our CSS names to the fonts' real family names
fn font_db(fdir: &str) -> (std::sync::Arc<resvg::usvg::fontdb::Database>, Vec<(String, String)>) {
    let mut g = FONTS.lock().unwrap_or_else(|p| p.into_inner());
    if let Some((d, db, alias)) = g.as_ref() {
        if d == fdir {
            return (db.clone(), alias.clone());
        }
    }
    let mut db = resvg::usvg::fontdb::Database::new();
    db.load_system_fonts();
    let mut alias = vec![];
    for (name, file) in [("LSXDaemon", "Daemon.otf"), ("LSXChesilin", "Chesilin.ttf")] {
        if let Ok(d) = std::fs::read(std::path::Path::new(fdir).join(file)) {
            let before = db.len();
            db.load_font_data(d);
            if let Some(f) = db.faces().nth(before) {
                if let Some((fam, _)) = f.families.first() {
                    alias.push((name.to_string(), fam.clone()));
                }
            }
        }
    }
    let db = std::sync::Arc::new(db);
    *g = Some((fdir.to_string(), db.clone(), alias.clone()));
    (db, alias)
}

/// draw an SVG page to a PNG `width` pixels wide on white; returns its size
fn render_png(svg: &str, width: u32, fdir: &str, out: &std::path::Path) -> Result<(u32, u32), String> {
    use resvg::{tiny_skia, usvg};
    let (db, alias) = font_db(fdir);
    let mut svg = svg.to_string();
    for (a, fam) in alias {
        svg = svg.replace(&a, &fam);
    }
    let mut opt = usvg::Options::default();
    opt.fontdb = db;
    let tree = usvg::Tree::from_str(&svg, &opt).map_err(|e| format!("the page could not be drawn: {e}"))?;
    let sz = tree.size();
    let scale = width as f32 / sz.width().max(1.0);
    let h = ((sz.height() * scale).ceil() as u32).max(1);
    let mut pm = tiny_skia::Pixmap::new(width, h).ok_or("the page is too large to draw")?;
    pm.fill(tiny_skia::Color::WHITE);
    resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pm.as_mut());
    pm.save_png(out).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok((width, h))
}

/// Chrome, Edge or Chromium: they print a page to PDF headlessly, vector text and fonts included.
pub fn find_browser() -> Option<std::path::PathBuf> {
    let mut c: Vec<std::path::PathBuf> = vec![];
    for var in ["ProgramFiles(x86)", "ProgramFiles", "LOCALAPPDATA"] {
        if let Some(r) = std::env::var_os(var) {
            let r = std::path::PathBuf::from(r);
            c.push(r.join("Microsoft/Edge/Application/msedge.exe"));
            c.push(r.join("Google/Chrome/Application/chrome.exe"));
            c.push(r.join("Chromium/Application/chrome.exe"));
            c.push(r.join("BraveSoftware/Brave-Browser/Application/brave.exe"));
        }
    }
    for p in ["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge", "/Applications/Chromium.app/Contents/MacOS/Chromium"] {
        c.push(p.into());
    }
    if let Some(path) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&path) {
            for n in ["google-chrome", "google-chrome-stable", "chromium", "chromium-browser", "microsoft-edge", "chrome", "msedge"] {
                c.push(d.join(n));
            }
        }
    }
    c.into_iter().find(|p| p.is_file())
}

/// print an HTML file to a PDF (the page size comes from the file's @page rule)
pub fn html_to_pdf(html: &std::path::Path, pdf: &std::path::Path) -> Result<(), String> {
    let exe = find_browser().ok_or("no Chrome / Edge / Chromium found to make the PDF; open the .html in a browser and print to PDF")?;
    let abs = std::fs::canonicalize(html).map_err(|e| format!("{}: {e}", html.display()))?;
    let mut p = abs.to_string_lossy().replace(char::from(92u8), "/");
    if let Some(r) = p.strip_prefix("//?/") {
        p = r.to_string();
    }
    let url = format!("file:///{}", p.trim_start_matches('/').replace(' ', "%20").replace('#', "%23"));
    let prof = std::env::temp_dir().join(format!("lsx_pdf_{}", std::process::id()));
    let _ = std::fs::remove_file(pdf);
    let mut cmd = std::process::Command::new(&exe);
    cmd.arg("--headless=new").arg("--disable-gpu").arg("--no-first-run").arg("--no-pdf-header-footer").arg("--run-all-compositor-stages-before-draw").arg("--virtual-time-budget=4000").arg(format!("--user-data-dir={}", prof.display())).arg(format!("--print-to-pdf={}", pdf.display())).arg(&url);
    cmd.stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000);
    }
    let mut child = cmd.spawn().map_err(|e| format!("{}: {e}", exe.display()))?;
    let t0 = std::time::Instant::now();
    let mut done = false;
    while t0.elapsed().as_secs() < 90 {
        if let Ok(Some(_)) = child.try_wait() {
            done = true;
            break;
        }
        // the PDF is written before the browser winds down
        if pdf.is_file() && std::fs::metadata(pdf).map(|m| m.len() > 0).unwrap_or(false) && t0.elapsed().as_secs() >= 2 {
            std::thread::sleep(std::time::Duration::from_millis(400));
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    if !done {
        let _ = child.kill();
    }
    let _ = child.wait();
    let _ = std::fs::remove_dir_all(&prof);
    if pdf.is_file() && std::fs::metadata(pdf).map(|m| m.len() > 0).unwrap_or(false) {
        Ok(())
    } else {
        Err(format!("{} did not produce {}", exe.display(), pdf.display()))
    }
}

/// @font-face rules for the Daemon and Chesilin fonts found in `fdir`
pub fn fonts_css(fdir: &str) -> String {
    let mut fonts: Vec<(&str, Vec<u8>, &str)> = vec![];
    if let Ok(d) = std::fs::read(std::path::Path::new(fdir).join("Daemon.otf")) {
        fonts.push(("LSXDaemon", d, "font/otf"));
    }
    if let Ok(d) = std::fs::read(std::path::Path::new(fdir).join("Chesilin.ttf")) {
        fonts.push(("LSXChesilin", d, "font/ttf"));
    }
    sheet::font_css(&fonts)
}
