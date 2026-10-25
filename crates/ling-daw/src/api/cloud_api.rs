//! File > Save to Cloud (`daw_cloud_*`): signing in to studio.linglin.art and uploading the project folder with the song's details.
//!
//! * `daw_cloud_login(base, login, password)` - POST /api/login; the reply carries the account's API token (the app keeps the token,
//!   never the password);
//! * `daw_cloud_me(base, token)` - GET /api/me, to show who is signed in and whether the account can upload (a Studio subscription);
//! * `daw_cloud_upload(base, token, project file, [field, value, ...])` - zips the folder of the saved project (its JSON, audio, ...,
//!   but not renders/, cache/ or license files) and POSTs it to /api/upload as multipart/form-data with the fields (title, artist,
//!   genre, description, lyrics, tags, visibility, note, project = an existing project's slug to add a version to it), streamed from a
//!   temporary file with upload progress;
//! * `daw_cloud_poll()` -> [state (0 idle, 1 working, 2 done, -1 failed), message, percent, reply JSON];
//! * `daw_cloud_lyrics()` -> the lyrics typed on the project's clips, in timeline order, one block per distinct text.
//!
//! One request at a time runs on a background thread, so the UI never waits on the network.
use super::{Daw, A, Val};
use crate::model::*;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

struct Job {
    handle: Option<std::thread::JoinHandle<Result<String, String>>>,
    progress: Arc<AtomicU32>,
    what: String,
    result: Option<Result<String, String>>,
}

fn job() -> &'static Mutex<Option<Job>> {
    static J: OnceLock<Mutex<Option<Job>>> = OnceLock::new();
    J.get_or_init(|| Mutex::new(None))
}

fn busy() -> bool {
    job().lock().unwrap_or_else(|p| p.into_inner()).as_ref().map(|j| j.result.is_none() && j.handle.is_some()).unwrap_or(false)
}

fn start(what: &str, f: impl FnOnce(Arc<AtomicU32>) -> Result<String, String> + Send + 'static) -> bool {
    if busy() {
        return false;
    }
    let progress = Arc::new(AtomicU32::new(0));
    let p2 = progress.clone();
    let handle = std::thread::spawn(move || f(p2));
    *job().lock().unwrap_or_else(|p| p.into_inner()) = Some(Job { handle: Some(handle), progress, what: what.to_string(), result: None });
    true
}

fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(std::time::Duration::from_secs(10)).timeout_read(std::time::Duration::from_secs(600)).build()
}

/// a reply body, also for 4xx / 5xx (the site answers errors with {"error": "..."})
fn reply(r: Result<ureq::Response, ureq::Error>) -> Result<String, String> {
    match r {
        Ok(r) => r.into_string().map_err(|e| e.to_string()),
        Err(ureq::Error::Status(code, r)) => {
            let body = r.into_string().unwrap_or_default();
            let msg = serde_json::from_str::<serde_json::Value>(&body).ok().and_then(|v| v.get("error").and_then(|e| e.as_str()).map(str::to_string)).unwrap_or_else(|| body.chars().take(200).collect());
            Err(format!("{code}: {msg}"))
        }
        Err(e) => Err(format!("could not reach the site: {e}")),
    }
}

fn form_encode(fields: &[(&str, &str)]) -> String {
    fields
        .iter()
        .map(|(k, v)| {
            let enc = |s: &str| {
                s.bytes()
                    .map(|b| match b {
                        b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => (b as char).to_string(),
                        b' ' => "+".to_string(),
                        _ => format!("%{b:02X}"),
                    })
                    .collect::<String>()
            };
            format!("{}={}", enc(k), enc(v))
        })
        .collect::<Vec<_>>()
        .join("&")
}

/// the project folder as a zip in a temporary file (renders, caches and license files stay home)
fn zip_folder(dir: &Path, progress: &AtomicU32) -> Result<(PathBuf, u64), String> {
    let skip_dir = |n: &str| matches!(n, "renders" | "cache" | ".ling-build" | "screenshots" | "backups");
    let mut files: Vec<PathBuf> = vec![];
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let rd = std::fs::read_dir(&d).map_err(|e| format!("{}: {e}", d.display()))?;
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().to_string();
            if p.is_dir() {
                if !skip_dir(&name) {
                    stack.push(p);
                }
            } else if !name.ends_with(".lsxlicense") && !name.ends_with(".tmp") {
                files.push(p);
            }
        }
    }
    if files.is_empty() {
        return Err("the project folder is empty".into());
    }
    let out = std::env::temp_dir().join(format!("lsx-cloud-{}.zip", std::process::id()));
    let f = std::fs::File::create(&out).map_err(|e| e.to_string())?;
    let mut z = zip::ZipWriter::new(f);
    let opts = zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Deflated).large_file(true);
    let total = files.len().max(1);
    for (i, p) in files.iter().enumerate() {
        let rel = p.strip_prefix(dir).unwrap_or(p).to_string_lossy().replace('\\', "/");
        z.start_file(rel, opts).map_err(|e| e.to_string())?;
        let mut src = std::fs::File::open(p).map_err(|e| format!("{}: {e}", p.display()))?;
        std::io::copy(&mut src, &mut z).map_err(|e| e.to_string())?;
        progress.store((i * 30 / total) as u32, Ordering::Relaxed);
    }
    z.finish().map_err(|e| e.to_string())?;
    let size = std::fs::metadata(&out).map(|m| m.len()).unwrap_or(0);
    Ok((out, size))
}

/// a reader that counts what it hands out into the progress (30..99 %)
struct Counted<R> {
    inner: R,
    done: u64,
    total: u64,
    progress: Arc<AtomicU32>,
}
impl<R: Read> Read for Counted<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        let n = self.inner.read(buf)?;
        self.done += n as u64;
        self.progress.store(30 + (self.done * 69 / self.total.max(1)) as u32, Ordering::Relaxed);
        Ok(n)
    }
}

fn upload(base: String, token: String, project: PathBuf, fields: Vec<(String, String)>, progress: Arc<AtomicU32>) -> Result<String, String> {
    let dir = project.parent().map(|d| d.to_path_buf()).ok_or("save the project first")?;
    let (zip_path, size) = zip_folder(&dir, &progress)?;
    let boundary = format!("LingStationXCloud{:x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0));
    let mut pre = Vec::new();
    let mut all: Vec<(String, String)> = vec![("format".into(), "zip".into())];
    all.extend(fields);
    for (k, v) in &all {
        let _ = write!(pre, "--{boundary}\r\nContent-Disposition: form-data; name=\"{k}\"\r\n\r\n{v}\r\n");
    }
    let file_name = format!("{}.zip", project.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "project".into()));
    let _ = write!(pre, "--{boundary}\r\nContent-Disposition: form-data; name=\"artifact\"; filename=\"{file_name}\"\r\nContent-Type: application/zip\r\n\r\n");
    let post = format!("\r\n--{boundary}--\r\n").into_bytes();
    let total = pre.len() as u64 + size + post.len() as u64;
    let file = std::fs::File::open(&zip_path).map_err(|e| e.to_string())?;
    let body = Counted { inner: std::io::Cursor::new(pre).chain(file).chain(std::io::Cursor::new(post)), done: 0, total, progress: progress.clone() };
    let r = agent()
        .post(&format!("{}/api/upload", base.trim_end_matches('/')))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Content-Type", &format!("multipart/form-data; boundary={boundary}"))
        .set("Content-Length", &total.to_string())
        .send(body);
    let _ = std::fs::remove_file(&zip_path);
    let out = reply(r)?;
    progress.store(100, Ordering::Relaxed);
    Ok(out)
}

impl Daw {
    pub(super) fn cloud_dispatch(&mut self, name: &str, a: &A) -> Result<Val, String> {
        match name {
            "daw_cloud_login" => {
                let (base, login, pw) = (a.s(0), a.s(1), a.s(2));
                Ok(Val::from(start("Signing in", move |_| {
                    let body = form_encode(&[("login", &login), ("password", &pw)]);
                    reply(agent().post(&format!("{}/api/login", base.trim_end_matches('/'))).set("Content-Type", "application/x-www-form-urlencoded").send_string(&body))
                })))
            }
            "daw_cloud_me" => {
                let (base, token) = (a.s(0), a.s(1));
                Ok(Val::from(start("Checking the account", move |_| reply(agent().get(&format!("{}/api/me", base.trim_end_matches('/'))).set("Authorization", &format!("Bearer {token}")).call()))))
            }
            "daw_cloud_upload" => {
                // (base, token, project file, [field, value, ...])
                let (base, token, project) = (a.s(0), a.s(1), PathBuf::from(a.s(2)));
                let mut fields = vec![];
                if let Some(Val::List(l)) = a.0.get(3) {
                    for kv in l.chunks(2) {
                        if let [Val::Str(k), v] = kv {
                            let v = match v {
                                Val::Str(s) => s.clone(),
                                Val::Num(n) => format!("{n}"),
                                _ => String::new(),
                            };
                            fields.push((k.clone(), v));
                        }
                    }
                }
                if !project.is_file() {
                    self.status = "Save to Cloud: save the project first".into();
                    return Ok(Val::from(false));
                }
                Ok(Val::from(start("Uploading", move |p| upload(base, token, project, fields, p))))
            }
            "daw_cloud_poll" => {
                let mut g = job().lock().unwrap_or_else(|p| p.into_inner());
                let Some(j) = g.as_mut() else { return Ok(Val::List(vec![Val::Num(0.0), Val::Str(String::new()), Val::Num(0.0), Val::Str(String::new())])) };
                if j.result.is_none() && j.handle.as_ref().map(|h| h.is_finished()).unwrap_or(false) {
                    j.result = Some(j.handle.take().map(|h| h.join().unwrap_or_else(|_| Err("the request crashed".into()))).unwrap_or_else(|| Err("lost".into())));
                }
                let pct = j.progress.load(Ordering::Relaxed) as f64;
                Ok(Val::List(match &j.result {
                    None => vec![Val::Num(1.0), Val::Str(format!("{} ...", j.what)), Val::Num(pct), Val::Str(String::new())],
                    Some(Ok(body)) => vec![Val::Num(2.0), Val::Str("done".into()), Val::Num(100.0), Val::Str(body.clone())],
                    Some(Err(e)) => vec![Val::Num(-1.0), Val::Str(e.clone()), Val::Num(pct), Val::Str(String::new())],
                }))
            }
            "daw_cloud_lyrics" => {
                let en = self.out.engine.lock().unwrap_or_else(|p| p.into_inner());
                let mut clips: Vec<(f64, String)> = en.project.tracks.iter().flat_map(|t| t.clips.iter().filter(|c| !c.lyric_text.trim().is_empty()).map(|c| (c.start, c.lyric_text.trim().to_string()))).collect();
                clips.sort_by(|a, b| a.0.total_cmp(&b.0));
                let mut seen: Vec<String> = vec![];
                for (_, t) in clips {
                    if !seen.contains(&t) {
                        seen.push(t);
                    }
                }
                Ok(Val::Str(seen.join("\n\n")))
            }
            _ => Err(format!("unknown builtin {name}")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forms_are_encoded() {
        assert_eq!(form_encode(&[("login", "a b"), ("password", "p&q=1")]), "login=a+b&password=p%26q%3D1");
    }

    #[test]
    fn a_project_folder_zips_without_its_renders() {
        let d = std::env::temp_dir().join(format!("lsx-zip-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("audio")).unwrap();
        std::fs::create_dir_all(d.join("renders")).unwrap();
        std::fs::write(d.join("Song.json"), "{}").unwrap();
        std::fs::write(d.join("audio/a.wav"), "RIFF").unwrap();
        std::fs::write(d.join("renders/big.wav"), "x").unwrap();
        let p = AtomicU32::new(0);
        let (z, size) = zip_folder(&d, &p).unwrap();
        assert!(size > 0);
        let names: Vec<String> = {
            let mut a = zip::ZipArchive::new(std::fs::File::open(&z).unwrap()).unwrap();
            (0..a.len()).map(|i| a.by_index(i).unwrap().name().to_string()).collect()
        };
        assert!(names.contains(&"Song.json".to_string()) && names.contains(&"audio/a.wav".to_string()), "{names:?}");
        assert!(!names.iter().any(|n| n.starts_with("renders")), "{names:?}");
        let _ = std::fs::remove_file(z);
        let _ = std::fs::remove_dir_all(d);
    }
}
