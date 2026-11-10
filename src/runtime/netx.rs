//! Network and data builtins for app clients (Horizon's apps: Discord, YouTube ...).
//!
//! - `http_request_async(method, url, headers, body)` -> job id; `http_req_poll(id)` -> "" while it
//!   runs, then "<status>\n<body>" (status 0 and "ERROR: ..." when the request could not be made).
//!   `headers` is "Name: value" lines. Nothing is kept between requests: no cookie jar, no cache.
//! - `ws_open(url, headers)` -> id; `ws_state(id)` "connecting" | "open" | "closed: <code> <why>" |
//!   "error: <why>"; `ws_send(id, text)`, `ws_send_b64(id, b64)` (a binary frame); `ws_recv(id)` the
//!   next message ("" none; a binary frame comes as "\u{1}" + its base64); `ws_close(id)`.
//! - `json_open(text)` -> handle (-1 when it is not JSON); `json_get(h, path)` a scalar as text (an
//!   object / array as compact JSON, "" when missing or null), `json_num`, `json_len` (-1 missing),
//!   `json_type` ("" "null" "bool" "num" "str" "arr" "obj"), `json_keys`, `json_close(h)`.
//!   A path is dot separated keys and array indexes: "d.guilds.0.name" ("" is the root).
//! - `rsa_oaep_new()` -> key handle (RSA-2048), `rsa_public_b64(h)` (SPKI DER, base64),
//!   `rsa_oaep_decrypt(h, b64)` (OAEP SHA-256; base64 of the plain bytes, "" when it fails),
//!   `rsa_free(h)`; `base64_decode_text(b64)`, `sha256_b64url(b64)` (unpadded base64url of the
//!   SHA-256 of the bytes `b64` holds).
//!
//! Everything lives in memory only and is dropped with the process.

use super::Value;
use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};

pub const NAMES: &[&str] = &[
    "http_request_async",
    "http_req_poll",
    "ws_open",
    "ws_state",
    "ws_send",
    "ws_send_b64",
    "ws_recv",
    "ws_close",
    "json_open",
    "json_get",
    "json_num",
    "json_len",
    "json_type",
    "json_keys",
    "json_close",
    "rsa_oaep_new",
    "rsa_public_b64",
    "rsa_oaep_decrypt",
    "rsa_free",
    "base64_decode_text",
    "sha256_b64url",
];

pub fn handles(name: &str) -> bool {
    NAMES.contains(&name)
}

fn s(args: &[Value], i: usize) -> String {
    args.get(i).map(|v| v.to_string()).unwrap_or_default()
}
fn n(args: &[Value], i: usize) -> i64 {
    match args.get(i) {
        Some(Value::Number(x)) => *x as i64,
        Some(v) => v.to_string().trim().parse::<f64>().map(|x| x as i64).unwrap_or(-1),
        None => -1,
    }
}

fn b64() -> base64::engine::GeneralPurpose {
    base64::engine::general_purpose::STANDARD
}

// ── HTTP with a method and headers ──

type Jobs = Arc<Mutex<HashMap<String, Option<String>>>>;
fn jobs() -> &'static Jobs {
    static J: std::sync::OnceLock<Jobs> = std::sync::OnceLock::new();
    J.get_or_init(Default::default)
}

fn client() -> &'static reqwest::Client {
    static C: std::sync::OnceLock<reqwest::Client> = std::sync::OnceLock::new();
    C.get_or_init(|| {
        reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::limited(5))
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| reqwest::Client::new())
    })
}

fn parse_headers(text: &str) -> Vec<(String, String)> {
    text.lines()
        .filter_map(|l| {
            let (k, v) = l.split_once(':')?;
            let k = k.trim();
            if k.is_empty() { None } else { Some((k.to_string(), v.trim().to_string())) }
        })
        .collect()
}

fn http_start(method: String, url: String, headers: String, body: String) -> String {
    let id = {
        use rand::RngCore;
        let mut buf = [0u8; 12];
        rand::rngs::OsRng.fill_bytes(&mut buf);
        buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
    };
    jobs().lock().unwrap().insert(id.clone(), None);
    let job = id.clone();
    super::web::async_runtime_handle().spawn(async move {
        let m = reqwest::Method::from_bytes(method.to_uppercase().as_bytes()).unwrap_or(reqwest::Method::GET);
        let mut rq = client().request(m, &url);
        for (k, v) in parse_headers(&headers) {
            rq = rq.header(k, v);
        }
        if !body.is_empty() {
            rq = rq.body(body);
        }
        let out = match rq.send().await {
            Ok(resp) => {
                let st = resp.status().as_u16();
                match resp.text().await {
                    Ok(t) => format!("{st}\n{t}"),
                    Err(e) => format!("0\nERROR: body read failed: {e}"),
                }
            }
            Err(e) => format!("0\nERROR: {e}"),
        };
        jobs().lock().unwrap().insert(job, Some(out));
    });
    id
}

fn http_poll(id: &str) -> String {
    let mut j = jobs().lock().unwrap();
    match j.get(id) {
        Some(Some(_)) => j.remove(id).flatten().unwrap_or_default(),
        _ => String::new(),
    }
}

// ── WebSocket client ──

enum WsOut {
    Text(String),
    Bin(Vec<u8>),
    Close,
}

struct Ws {
    tx: tokio::sync::mpsc::UnboundedSender<WsOut>,
    rx: std::sync::mpsc::Receiver<String>,
    state: Arc<Mutex<String>>,
}

thread_local! {
    static WS: RefCell<HashMap<i64, Ws>> = RefCell::new(HashMap::new());
    static WS_N: RefCell<i64> = const { RefCell::new(0) };
    static JDOCS: RefCell<HashMap<i64, ling_http::serde_json::Value>> = RefCell::new(HashMap::new());
    static JD_N: RefCell<i64> = const { RefCell::new(0) };
    static RSA: RefCell<HashMap<i64, rsa::RsaPrivateKey>> = RefCell::new(HashMap::new());
    static RSA_N: RefCell<i64> = const { RefCell::new(0) };
}

fn next(c: &'static std::thread::LocalKey<RefCell<i64>>) -> i64 {
    c.with(|n| {
        let mut n = n.borrow_mut();
        *n += 1;
        *n
    })
}

fn tls_connector() -> tokio_tungstenite::Connector {
    let mut roots = rustls::RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()
        .expect("rustls protocol versions")
        .with_root_certificates(roots)
        .with_no_client_auth();
    tokio_tungstenite::Connector::Rustls(Arc::new(cfg))
}

fn ws_open(url: String, headers: String) -> i64 {
    use futures_util::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    use tokio_tungstenite::tungstenite::Message;

    let id = next(&WS_N);
    let (tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<WsOut>();
    let (in_tx, rx) = std::sync::mpsc::channel::<String>();
    let state = Arc::new(Mutex::new("connecting".to_string()));
    let st = state.clone();
    super::web::async_runtime_handle().spawn(async move {
        let set = |v: String| *st.lock().unwrap() = v;
        let mut req = match url.as_str().into_client_request() {
            Ok(r) => r,
            Err(e) => return set(format!("error: {e}")),
        };
        for (k, v) in parse_headers(&headers) {
            if let (Ok(k), Ok(v)) = (
                tokio_tungstenite::tungstenite::http::HeaderName::from_bytes(k.as_bytes()),
                tokio_tungstenite::tungstenite::http::HeaderValue::from_str(&v),
            ) {
                req.headers_mut().insert(k, v);
            }
        }
        let cfg = tokio_tungstenite::tungstenite::protocol::WebSocketConfig {
            max_message_size: Some(256 << 20),
            max_frame_size: Some(64 << 20),
            ..Default::default()
        };
        let conn = tokio_tungstenite::connect_async_tls_with_config(req, Some(cfg), false, Some(tls_connector())).await;
        let (ws, _) = match conn {
            Ok(c) => c,
            Err(e) => return set(format!("error: {e}")),
        };
        set("open".into());
        let (mut sink, mut stream) = ws.split();
        loop {
            tokio::select! {
                m = stream.next() => match m {
                    Some(Ok(Message::Text(t))) => { let _ = in_tx.send(t.to_string()); }
                    Some(Ok(Message::Binary(b))) => {
                        use base64::Engine as _;
                        let _ = in_tx.send(format!("\u{1}{}", b64().encode(&b)));
                    }
                    Some(Ok(Message::Close(f))) => {
                        let why = f.map(|f| format!("{} {}", u16::from(f.code), f.reason)).unwrap_or_else(|| "1005".into());
                        return set(format!("closed: {why}"));
                    }
                    Some(Ok(_)) => {}
                    Some(Err(e)) => return set(format!("error: {e}")),
                    None => return set("closed: 1006 connection lost".into()),
                },
                o = out_rx.recv() => match o {
                    Some(WsOut::Text(t)) => { if let Err(e) = sink.send(Message::Text(t.into())).await { return set(format!("error: {e}")); } }
                    Some(WsOut::Bin(b)) => { if let Err(e) = sink.send(Message::Binary(b.into())).await { return set(format!("error: {e}")); } }
                    Some(WsOut::Close) | None => {
                        let _ = sink.send(Message::Close(None)).await;
                        let _ = sink.close().await;
                        return set("closed: 1000 closed by us".into());
                    }
                },
            }
        }
    });
    WS.with(|w| w.borrow_mut().insert(id, Ws { tx, rx, state }));
    id
}

fn ws_with<R>(id: i64, f: impl FnOnce(&Ws) -> R, d: R) -> R {
    WS.with(|w| w.borrow().get(&id).map(f).unwrap_or(d))
}

// ── JSON documents ──

fn jpath<'a>(v: &'a ling_http::serde_json::Value, path: &str) -> Option<&'a ling_http::serde_json::Value> {
    use ling_http::serde_json::Value as J;
    let mut cur = v;
    if path.is_empty() {
        return Some(cur);
    }
    for seg in path.split('.') {
        cur = match cur {
            J::Object(m) => m.get(seg)?,
            J::Array(a) => a.get(seg.parse::<usize>().ok()?)?,
            _ => return None,
        };
    }
    Some(cur)
}

fn jdoc<R>(h: i64, path: &str, f: impl FnOnce(Option<&ling_http::serde_json::Value>) -> R) -> R {
    JDOCS.with(|d| {
        let d = d.borrow();
        f(d.get(&h).and_then(|v| jpath(v, path)))
    })
}

fn jtext(v: Option<&ling_http::serde_json::Value>) -> String {
    use ling_http::serde_json::Value as J;
    match v {
        None | Some(J::Null) => String::new(),
        Some(J::String(s)) => s.clone(),
        Some(J::Bool(b)) => b.to_string(),
        Some(J::Number(n)) => n.to_string(),
        Some(o) => o.to_string(),
    }
}

// ── the builtins ──

pub fn call(name: &str, args: &[Value]) -> Value {
    use base64::Engine as _;
    use ling_http::serde_json::Value as J;
    match name {
        "http_request_async" => Value::Str(http_start(s(args, 0), s(args, 1), s(args, 2), s(args, 3))),
        "http_req_poll" => Value::Str(http_poll(&s(args, 0))),

        "ws_open" => Value::Number(ws_open(s(args, 0), s(args, 1)) as f64),
        "ws_state" => Value::Str(ws_with(n(args, 0), |w| w.state.lock().unwrap().clone(), "closed: no such socket".into())),
        "ws_send" => {
            let t = s(args, 1);
            Value::Bool(ws_with(n(args, 0), |w| w.tx.send(WsOut::Text(t)).is_ok(), false))
        }
        "ws_send_b64" => {
            let b = b64().decode(s(args, 1).trim()).unwrap_or_default();
            Value::Bool(ws_with(n(args, 0), |w| w.tx.send(WsOut::Bin(b)).is_ok(), false))
        }
        "ws_recv" => Value::Str(ws_with(n(args, 0), |w| w.rx.try_recv().unwrap_or_default(), String::new())),
        "ws_close" => {
            let id = n(args, 0);
            if let Some(w) = WS.with(|m| m.borrow_mut().remove(&id)) {
                let _ = w.tx.send(WsOut::Close);
            }
            Value::Bool(true)
        }

        "json_open" => match ling_http::serde_json::from_str::<J>(&s(args, 0)) {
            Ok(v) => {
                let h = next(&JD_N);
                JDOCS.with(|d| d.borrow_mut().insert(h, v));
                Value::Number(h as f64)
            }
            Err(_) => Value::Number(-1.0),
        },
        "json_get" => Value::Str(jdoc(n(args, 0), &s(args, 1), jtext)),
        "json_num" => Value::Number(jdoc(n(args, 0), &s(args, 1), |v| match v {
            Some(J::Number(x)) => x.as_f64().unwrap_or(0.0),
            Some(J::String(t)) => t.parse::<f64>().unwrap_or(0.0),
            Some(J::Bool(b)) => if *b { 1.0 } else { 0.0 },
            _ => 0.0,
        })),
        "json_len" => Value::Number(jdoc(n(args, 0), &s(args, 1), |v| match v {
            Some(J::Array(a)) => a.len() as f64,
            Some(J::Object(m)) => m.len() as f64,
            Some(J::String(t)) => t.chars().count() as f64,
            Some(_) => 0.0,
            None => -1.0,
        })),
        "json_type" => Value::Str(jdoc(n(args, 0), &s(args, 1), |v| match v {
            None => "",
            Some(J::Null) => "null",
            Some(J::Bool(_)) => "bool",
            Some(J::Number(_)) => "num",
            Some(J::String(_)) => "str",
            Some(J::Array(_)) => "arr",
            Some(J::Object(_)) => "obj",
        }
        .to_string())),
        "json_keys" => Value::List(std::rc::Rc::new(jdoc(n(args, 0), &s(args, 1), |v| match v {
            Some(J::Object(m)) => m.keys().map(|k| Value::Str(k.clone())).collect(),
            Some(J::Array(a)) => (0..a.len()).map(|i| Value::Str(i.to_string())).collect(),
            _ => Vec::new(),
        }))),
        "json_close" => {
            let h = n(args, 0);
            JDOCS.with(|d| d.borrow_mut().remove(&h));
            Value::Bool(true)
        }

        "rsa_oaep_new" => {
            let mut rng = rand::rngs::OsRng;
            match rsa::RsaPrivateKey::new(&mut rng, 2048) {
                Ok(k) => {
                    let h = next(&RSA_N);
                    RSA.with(|r| r.borrow_mut().insert(h, k));
                    Value::Number(h as f64)
                }
                Err(_) => Value::Number(-1.0),
            }
        }
        "rsa_public_b64" => {
            use rsa::pkcs8::EncodePublicKey;
            let h = n(args, 0);
            Value::Str(RSA.with(|r| {
                r.borrow()
                    .get(&h)
                    .and_then(|k| k.to_public_key().to_public_key_der().ok())
                    .map(|d| b64().encode(d.as_bytes()))
                    .unwrap_or_default()
            }))
        }
        "rsa_oaep_decrypt" => {
            let h = n(args, 0);
            let data = b64().decode(s(args, 1).trim()).unwrap_or_default();
            Value::Str(RSA.with(|r| {
                r.borrow()
                    .get(&h)
                    .and_then(|k| k.decrypt(rsa::Oaep::new::<sha2::Sha256>(), &data).ok())
                    .map(|p| b64().encode(p))
                    .unwrap_or_default()
            }))
        }
        "rsa_free" => {
            let h = n(args, 0);
            RSA.with(|r| r.borrow_mut().remove(&h));
            Value::Bool(true)
        }
        "base64_decode_text" => {
            let b = b64().decode(s(args, 0).trim()).unwrap_or_default();
            Value::Str(String::from_utf8_lossy(&b).into_owned())
        }
        "sha256_b64url" => {
            use sha2::Digest;
            let b = b64().decode(s(args, 0).trim()).unwrap_or_default();
            let d = sha2::Sha256::digest(&b);
            Value::Str(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(d))
        }
        _ => Value::Unit,
    }
}
