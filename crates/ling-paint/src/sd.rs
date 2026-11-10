//! Stable Diffusion through a local AUTOMATIC1111 / Forge / SD.Next server (started with `--api`).
//! Checkpoints are whatever that server has in its models folder; a request can switch checkpoint with
//! `override_settings`. All network work runs on background threads; the studio polls a channel.

use serde_json::{json, Value as J};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::Arc;
use std::time::Duration;

pub enum SdMsg {
    Models(Vec<String>),
    Samplers(Vec<String>),
    Progress(f32, String),
    Images(Vec<Vec<u8>>),
    Error(String),
}

fn split_url(base: &str) -> Result<(String, u16, String), String> {
    let b = base.trim().trim_end_matches('/');
    if b.starts_with("https://") {
        return Err("https is not supported for the local SD server - use http://127.0.0.1:7860".into());
    }
    let rest = b.strip_prefix("http://").unwrap_or(b);
    let (hostport, prefix) = match rest.find('/') {
        Some(i) => (&rest[..i], rest[i..].to_string()),
        None => (rest, String::new()),
    };
    let (host, port) = match hostport.rsplit_once(':') {
        Some((h, p)) => (h.to_string(), p.parse::<u16>().map_err(|_| format!("bad port in {base}"))?),
        None => (hostport.to_string(), 80),
    };
    Ok((host, port, prefix))
}

fn dechunk(body: &[u8]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < body.len() {
        let Some(nl) = body[i..].windows(2).position(|w| w == b"\r\n") else { break };
        let line = String::from_utf8_lossy(&body[i..i + nl]);
        let size = usize::from_str_radix(line.split(';').next().unwrap_or("0").trim(), 16).unwrap_or(0);
        i += nl + 2;
        if size == 0 || i + size > body.len() {
            break;
        }
        out.extend_from_slice(&body[i..i + size]);
        i += size + 2;
    }
    out
}

/// minimal HTTP/1.1 request over a plain TCP socket
pub fn http(base: &str, method: &str, path: &str, body: Option<&str>, timeout_s: u64) -> Result<String, String> {
    let (host, port, prefix) = split_url(base)?;
    let addr = format!("{host}:{port}");
    let mut s = TcpStream::connect(&addr).map_err(|e| format!("cannot reach {addr} ({e}). Start your SD WebUI / Forge with --api"))?;
    s.set_read_timeout(Some(Duration::from_secs(timeout_s))).ok();
    s.set_write_timeout(Some(Duration::from_secs(60))).ok();
    let body = body.unwrap_or("");
    let req = format!(
        "{method} {prefix}{path} HTTP/1.1\r\nHost: {host}:{port}\r\nUser-Agent: LingAnime\r\nAccept: application/json\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    s.write_all(req.as_bytes()).map_err(|e| e.to_string())?;
    s.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    s.read_to_end(&mut buf).map_err(|e| format!("reading the reply: {e}"))?;
    let Some(hend) = buf.windows(4).position(|w| w == b"\r\n\r\n") else { return Err("malformed HTTP reply".into()) };
    let head = String::from_utf8_lossy(&buf[..hend]).to_string();
    let mut payload = buf[hend + 4..].to_vec();
    if head.to_ascii_lowercase().contains("transfer-encoding: chunked") {
        payload = dechunk(&payload);
    }
    let status: u32 = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let text = String::from_utf8_lossy(&payload).to_string();
    if !(200..300).contains(&status) {
        let detail = serde_json::from_str::<J>(&text).ok().and_then(|j| j.get("detail").or(j.get("error")).map(|d| d.to_string())).unwrap_or_else(|| text.chars().take(300).collect());
        return Err(format!("server answered {status}: {detail}"));
    }
    Ok(text)
}

pub fn fetch_lists(base: String) -> Receiver<SdMsg> {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        match http(&base, "GET", "/sdapi/v1/sd-models", None, 20).and_then(|t| serde_json::from_str::<J>(&t).map_err(|e| e.to_string())) {
            Ok(J::Array(a)) => {
                let names = a.iter().filter_map(|m| m.get("title").and_then(|t| t.as_str()).map(String::from)).collect();
                let _ = tx.send(SdMsg::Models(names));
            }
            Ok(_) => {
                let _ = tx.send(SdMsg::Error("unexpected model list".into()));
            }
            Err(e) => {
                let _ = tx.send(SdMsg::Error(e));
                return;
            }
        }
        if let Ok(J::Array(a)) = http(&base, "GET", "/sdapi/v1/samplers", None, 20).and_then(|t| serde_json::from_str::<J>(&t).map_err(|e| e.to_string())) {
            let names = a.iter().filter_map(|m| m.get("name").and_then(|t| t.as_str()).map(String::from)).collect();
            let _ = tx.send(SdMsg::Samplers(names));
        }
    });
    rx
}

pub struct GenReq {
    pub base: String,
    pub prompt: String,
    pub negative: String,
    pub steps: u32,
    pub cfg: f32,
    pub denoise: f32,
    pub seed: i64,
    pub sampler: String,
    pub model: String,
    pub w: u32,
    pub h: u32,
    /// png bytes of the picture under the selection (img2img / inpaint)
    pub init: Option<Vec<u8>>,
    /// png bytes of the inpaint mask (white = repaint)
    pub mask: Option<Vec<u8>>,
}

pub fn generate(r: GenReq) -> Receiver<SdMsg> {
    let (tx, rx) = channel::<SdMsg>();
    let done = Arc::new(AtomicBool::new(false));
    // progress poller
    {
        let tx: Sender<SdMsg> = tx.clone();
        let base = r.base.clone();
        let done = done.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(600));
            while !done.load(Ordering::Relaxed) {
                if let Ok(t) = http(&base, "GET", "/sdapi/v1/progress?skip_current_image=true", None, 5) {
                    if let Ok(j) = serde_json::from_str::<J>(&t) {
                        let p = j.get("progress").and_then(|v| v.as_f64()).unwrap_or(0.0) as f32;
                        let eta = j.get("eta_relative").and_then(|v| v.as_f64()).unwrap_or(0.0);
                        let job = j.pointer("/state/job").and_then(|v| v.as_str()).unwrap_or("");
                        let _ = tx.send(SdMsg::Progress(p, format!("{:.0}%  eta {:.0}s {}", p * 100.0, eta, job)));
                    }
                }
                std::thread::sleep(Duration::from_millis(700));
            }
        });
    }
    std::thread::spawn(move || {
        use base64::Engine;
        let b64 = base64::engine::general_purpose::STANDARD;
        let mut body = json!({
            "prompt": r.prompt,
            "negative_prompt": r.negative,
            "steps": r.steps,
            "cfg_scale": r.cfg,
            "width": r.w,
            "height": r.h,
            "seed": r.seed,
            "batch_size": 1,
            "n_iter": 1,
            "send_images": true,
            "save_images": false,
        });
        if !r.sampler.is_empty() {
            body["sampler_name"] = json!(r.sampler);
        }
        if !r.model.is_empty() {
            body["override_settings"] = json!({ "sd_model_checkpoint": r.model });
            body["override_settings_restore_afterwards"] = json!(false);
        }
        let path = if let Some(init) = &r.init {
            body["init_images"] = json!([b64.encode(init)]);
            body["denoising_strength"] = json!(r.denoise);
            body["resize_mode"] = json!(0);
            if let Some(m) = &r.mask {
                body["mask"] = json!(b64.encode(m));
                body["mask_blur"] = json!(4);
                body["inpainting_fill"] = json!(1);
                body["inpaint_full_res"] = json!(false);
                body["inpainting_mask_invert"] = json!(0);
            }
            "/sdapi/v1/img2img"
        } else {
            "/sdapi/v1/txt2img"
        };
        let res = http(&r.base, "POST", path, Some(&body.to_string()), 1800).and_then(|t| serde_json::from_str::<J>(&t).map_err(|e| format!("bad reply: {e}")));
        done.store(true, Ordering::Relaxed);
        match res {
            Ok(j) => {
                let imgs: Vec<Vec<u8>> = j
                    .get("images")
                    .and_then(|a| a.as_array())
                    .map(|a| a.iter().filter_map(|s| s.as_str()).filter_map(|s| b64.decode(s.split(',').next_back().unwrap_or(s)).ok()).collect())
                    .unwrap_or_default();
                if imgs.is_empty() {
                    let _ = tx.send(SdMsg::Error("the server returned no images".into()));
                } else {
                    let _ = tx.send(SdMsg::Images(imgs));
                }
            }
            Err(e) => {
                let _ = tx.send(SdMsg::Error(e));
            }
        }
    });
    rx
}

pub fn interrupt(base: String) {
    std::thread::spawn(move || {
        let _ = http(&base, "POST", "/sdapi/v1/interrupt", Some("{}"), 10);
    });
}
