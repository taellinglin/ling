//! Builtins for the karaoke export (`daw_kar_*`, see `karaoke.rs`).
use super::{Daw, A, Val};
use crate::karaoke::{self, Style};
use std::sync::{Arc, Mutex};

pub type VideoJob = Arc<Mutex<Option<Result<String, String>>>>;

fn tracks_of(opts: &str) -> Vec<usize> {
    for kv in opts.split(';') {
        if let Some(v) = kv.strip_prefix("tracks=") {
            return v.split(',').filter_map(|x| x.trim().parse().ok()).collect();
        }
    }
    vec![]
}
fn opt(opts: &str, key: &str) -> String {
    opts.split(';').find_map(|kv| kv.trim().strip_prefix(&format!("{key}=")).map(|v| v.to_string())).unwrap_or_default()
}

fn mime(path: &str) -> &'static str {
    if path.to_lowercase().ends_with(".otf") {
        "font/otf"
    } else {
        "font/ttf"
    }
}

/// ffmpeg on PATH or in a few usual places
pub fn find_ffmpeg() -> String {
    let exe = if cfg!(windows) { "ffmpeg.exe" } else { "ffmpeg" };
    if let Some(p) = std::env::var_os("PATH") {
        for d in std::env::split_paths(&p) {
            if d.join(exe).is_file() {
                return d.join(exe).to_string_lossy().into_owned();
            }
        }
    }
    for p in ["C:/ffmpeg/bin/ffmpeg.exe", "C:/Program Files/ffmpeg/bin/ffmpeg.exe", "/usr/bin/ffmpeg", "/usr/local/bin/ffmpeg", "/opt/homebrew/bin/ffmpeg"] {
        if std::path::Path::new(p).is_file() {
            return p.to_string();
        }
    }
    String::new()
}

impl Daw {
    pub(super) fn kar_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        let eng = self.out.engine.clone();
        macro_rules! ok {
            ($v:expr) => {
                return Ok(Val::from($v))
            };
        }
        match name {
            "daw_kar_scan" => {
                // -> [syllables, lines, seconds, tracks, (track, name, lyric notes)*]
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let song = karaoke::collect(&en.project, &[]);
                let lt = karaoke::lyric_tracks(&en.project);
                let mut v = vec![Val::Num(song.lines.iter().map(|l| l.syls.len()).sum::<usize>() as f64), Val::Num(song.lines.len() as f64), Val::Num(song.length), Val::Num(lt.len() as f64)];
                for (t, n) in lt {
                    v.extend([Val::Num(t as f64), Val::Str(en.project.tracks[t].name.clone()), Val::Num(n as f64)]);
                }
                ok!(Val::List(v))
            }
            "daw_kar_lines" => {
                // (max lines, tracks "1,3" or "") -> [text, start seconds, syllables]*
                let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                let tr: Vec<usize> = a.s(1).split(',').filter_map(|x| x.trim().parse().ok()).collect();
                let song = karaoke::collect(&en.project, &tr);
                let mut v = vec![];
                for l in song.lines.iter().take(a.u(0).clamp(1, 200)) {
                    v.extend([Val::Str(l.text()), Val::Num(l.start()), Val::Num(l.syls.len() as f64)]);
                }
                ok!(Val::List(v))
            }
            "daw_kar_ffmpeg" => ok!(find_ffmpeg()),
            "daw_kar_font_family" => {
                let Ok(d) = std::fs::read(a.s(0)) else { return ok!(String::new()) };
                ok!(karaoke::font_family(&d).unwrap_or_default())
            }
            "daw_kar_write" => {
                // (folder, base name, options "k=v;...", formats "ass,srt,lrc,html") -> the files written (names), [] on failure (status says why)
                let (dir, base, opts, formats) = (a.s(0), a.s(1), a.s(2), a.s(3));
                let song = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    karaoke::collect(&en.project, &tracks_of(&opts))
                };
                if song.lines.is_empty() {
                    self.status = "Karaoke: no lyrics found - fit lyrics to a clip's notes first (Lyrics > Fit...)".into();
                    return ok!(Val::List(vec![]));
                }
                let mut st = Style::parse(&opts);
                if st.title.is_empty() {
                    st.title = song.title.clone();
                }
                let dirp = std::path::Path::new(&dir);
                std::fs::create_dir_all(dirp).map_err(|e| format!("{dir}: {e}"))?;
                // fonts: the chosen file (for ASS and the player) and any in the fonts folder (the player offers them)
                let mut fam = st.font.clone();
                let mut embed: Vec<(String, Vec<u8>, &'static str)> = vec![];
                if !st.font_file.is_empty() {
                    if let Ok(d) = std::fs::read(&st.font_file) {
                        let family = karaoke::font_family(&d).unwrap_or_else(|| "Custom".into());
                        let fname = std::path::Path::new(&st.font_file).file_name().map(|f| f.to_string_lossy().into_owned()).unwrap_or_else(|| "font.ttf".into());
                        let _ = std::fs::create_dir_all(dirp.join("fonts"));
                        let _ = std::fs::write(dirp.join("fonts").join(&fname), &d);
                        let label = std::path::Path::new(&fname).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| family.clone());
                        embed.push((label.clone(), d, mime(&fname)));
                        fam = family;
                        st.font = label;
                    }
                }
                let fdir = opt(&opts, "fontsdir");
                if !fdir.is_empty() {
                    if let Ok(rd) = std::fs::read_dir(&fdir) {
                        for f in rd.flatten() {
                            let p = f.path();
                            let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
                            if ext == "otf" || ext == "ttf" {
                                let label = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                                if embed.iter().all(|e| e.0 != label) {
                                    if let Ok(d) = std::fs::read(&p) {
                                        embed.push((label, d, mime(&p.to_string_lossy())));
                                    }
                                }
                            }
                        }
                    }
                }
                let mut written: Vec<Val> = vec![];
                let mut put = |file: String, body: String| -> Result<(), String> {
                    std::fs::write(dirp.join(&file), body).map_err(|e| format!("{file}: {e}"))?;
                    written.push(Val::Str(file));
                    Ok(())
                };
                for f in formats.split(',').map(|x| x.trim()) {
                    match f {
                        "ass" => put(format!("{base}.ass"), karaoke::ass(&song, &st, &fam))?,
                        "srt" => {
                            put(format!("{base}.srt"), karaoke::srt_lines(&song, &st))?;
                            put(format!("{base}_karaoke.srt"), karaoke::srt_karaoke(&song, &st))?;
                        }
                        "lrc" => put(format!("{base}.lrc"), karaoke::lrc(&song, &st))?,
                        "html" => put(format!("{base}.html"), karaoke::html(&song, &st, &embed))?,
                        _ => {}
                    }
                }
                self.status = format!("Karaoke: {} lines, {} syllables -> {} file(s) in {dir}", song.lines.len(), song.lines.iter().map(|l| l.syls.len()).sum::<usize>(), written.len());
                ok!(Val::List(written))
            }
            "daw_kar_video" => {
                // (folder, base name, options) -> 1 when ffmpeg started: `<base>.ass` + the audio named in the options become `<base>.mp4`
                let (dir, base, opts) = (a.s(0), a.s(1), a.s(2));
                let ff = find_ffmpeg();
                if ff.is_empty() {
                    self.status = "Karaoke video needs ffmpeg (not found on PATH or in C:/ffmpeg/bin)".into();
                    return ok!(false);
                }
                let st = Style::parse(&opts);
                let secs = {
                    let en = eng.lock().unwrap_or_else(|p| p.into_inner());
                    en.project.beats_to_secs(0.0, en.project.end_beat())
                } + 2.0;
                let job: VideoJob = Arc::new(Mutex::new(None));
                let jc = job.clone();
                let has_audio = !st.audio.is_empty() && std::path::Path::new(&dir).join(&st.audio).is_file();
                let hexc = |c: u32| format!("0x{c:06X}");
                let size = format!("{}x{}", st.w, st.h);
                let bg = if st.bg1 == st.bg2 {
                    format!("color=c={}:s={size}:r=30:d={secs:.2}", hexc(st.bg1))
                } else {
                    format!("gradients=s={size}:c0={}:c1={}:x0=0:y0=0:x1=0:y1={}:speed=0.0001:rate=30:duration={secs:.2}", hexc(st.bg1), hexc(st.bg2), st.h)
                };
                let vf = if std::path::Path::new(&dir).join("fonts").is_dir() { format!("ass={base}.ass:fontsdir=fonts") } else { format!("ass={base}.ass") };
                let out = format!("{base}.mp4");
                std::thread::Builder::new()
                    .name("ling-karaoke-video".into())
                    .spawn(move || {
                        let run = |bg: &str| -> Result<String, String> {
                            let mut c = std::process::Command::new(&ff);
                            c.current_dir(&dir).args(["-y", "-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i", bg]);
                            if has_audio {
                                c.args(["-i", &st.audio]);
                            }
                            c.args(["-vf", &vf, "-c:v", "libx264", "-pix_fmt", "yuv420p", "-preset", "veryfast"]);
                            if has_audio {
                                c.args(["-c:a", "aac", "-b:a", "192k", "-shortest"]);
                            } else {
                                c.args(["-t", &format!("{secs:.2}")]);
                            }
                            c.arg(&out);
                            let r = c.output().map_err(|e| format!("ffmpeg: {e}"))?;
                            if r.status.success() {
                                Ok(format!("{dir}/{out}"))
                            } else {
                                Err(String::from_utf8_lossy(&r.stderr).lines().last().unwrap_or("ffmpeg failed").to_string())
                            }
                        };
                        let mut r = run(&bg);
                        if r.is_err() && bg.starts_with("gradients") {
                            // an older ffmpeg without the gradients source: a flat colour
                            r = run(&format!("color=c={}:s={size}:r=30:d={secs:.2}", hexc(st.bg1)));
                        }
                        *jc.lock().unwrap_or_else(|p| p.into_inner()) = Some(r);
                    })
                    .map_err(|e| e.to_string())?;
                self.kar_job = Some(job);
                ok!(true)
            }
            // 0 none, 1 running, 2 done, 3 failed
            "daw_kar_video_state" => ok!(match &self.kar_job {
                None => 0.0,
                Some(j) => match &*j.lock().unwrap_or_else(|p| p.into_inner()) {
                    None => 1.0,
                    Some(Ok(_)) => 2.0,
                    Some(Err(_)) => 3.0,
                },
            }),
            "daw_kar_video_message" => ok!(match &self.kar_job {
                Some(j) => match &*j.lock().unwrap_or_else(|p| p.into_inner()) {
                    Some(Ok(p)) => format!("Karaoke video: {p}"),
                    Some(Err(m)) => format!("Karaoke video failed: {m}"),
                    None => String::new(),
                },
                None => String::new(),
            }),
            "daw_kar_video_ack" => {
                self.kar_job = None;
                Ok(Val::Unit)
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}
