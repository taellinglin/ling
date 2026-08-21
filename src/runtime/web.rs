// src/runtime/web.rs — bridges the (synchronous, single-threaded) Ling
// interpreter to ling-http's async axum server.
//
// `Value` (closures especially) holds `Rc` — it is not `Send`. So the
// interpreter, and every `Value`, must stay on exactly one thread: whichever
// thread is running the `.ling` program. The HTTP server, though, wants real
// async I/O to hold many connections at once. The split:
//
//   - `spawn_server` starts axum on a *background* OS thread with its own
//     tokio runtime. Every request it receives is turned into a
//     `PendingRequest` (plain owned Strings — Send) and posted on an
//     `std::sync::mpsc` channel, then that background task `.await`s a
//     `tokio::sync::oneshot` for the response.
//   - The `http_serve` builtin (see runtime/mod.rs) drains that mpsc
//     receiver in a blocking loop *on the interpreter's own thread*,
//     dispatching each request to the matching registered Ling closure via
//     `Interpreter::call_value`, and sends the result back through the
//     oneshot. That's the only place `Value` is ever touched.
//
// Route matching (method + exact path) happens in the interpreter loop, not
// in axum: the axum side is one dumb catch-all so routes can be registered
// dynamically by `.ling` source before `http_serve` is called.

use ling_http::axum;
use ling_http::tokio;

use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Router;

#[derive(Debug, Clone)]
pub struct HttpResponse {
    pub status: u16,
    pub content_type: String,
    pub body: String,
    /// Raw `Set-Cookie` header value (e.g. `"ling_session=abc; HttpOnly; Path=/"`),
    /// set by a `.ling` handler returning a `set_cookie` field on its response struct.
    pub set_cookie: Option<String>,
    /// `Location` header for a redirect (`.ling` sets `status: 302` alongside this).
    pub location: Option<String>,
}

impl Default for HttpResponse {
    fn default() -> Self {
        Self {
            status: 200,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: String::new(),
            set_cookie: None,
            location: None,
        }
    }
}

pub struct PendingRequest {
    pub method: String,
    pub path: String,
    pub query: String,
    pub body: String,
    /// Raw `Cookie` request header value (empty string if absent) — sessions are
    /// built on top of this in `.ling` code, not in the Rust bridge.
    pub cookie: String,
    /// Raw `Authorization` request header (empty if absent) — API-key auth for
    /// `lingfu publish` reads its bearer token from here.
    pub authorization: String,
    /// Best-effort client IP for rate limiting: the first `X-Forwarded-For`
    /// hop (set by a reverse proxy) or `X-Real-IP`, else "". Behind a proxy
    /// the socket peer is the proxy, so these headers are the real source;
    /// `.ling` code falls back to a username/key when this is empty.
    pub client_ip: String,
    pub respond_to: tokio::sync::oneshot::Sender<HttpResponse>,
}

#[derive(Clone)]
struct ServerState {
    tx: std::sync::mpsc::Sender<PendingRequest>,
}

async fn catch_all(State(state): State<ServerState>, req: Request) -> Response {
    let method = req.method().to_string();
    let path = req.uri().path().to_string();
    let query = req.uri().query().unwrap_or("").to_string();
    let cookie = req
        .headers()
        .get(axum::http::header::COOKIE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let authorization = req
        .headers()
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    let client_ip = req
        .headers()
        .get("x-forwarded-for")
        .and_then(|v| v.to_str().ok())
        .and_then(|s| s.split(',').next())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| {
            req.headers()
                .get("x-real-ip")
                .and_then(|v| v.to_str().ok())
                .map(|s| s.trim().to_string())
        })
        .unwrap_or_default();

    let body_bytes = match axum::body::to_bytes(req.into_body(), 32 * 1024 * 1024).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::BAD_REQUEST, "body too large or unreadable").into_response(),
    };
    let body = String::from_utf8_lossy(&body_bytes).into_owned();

    let (resp_tx, resp_rx) = tokio::sync::oneshot::channel();
    let sent = state.tx.send(PendingRequest {
        method,
        path,
        query,
        body,
        cookie,
        authorization,
        client_ip,
        respond_to: resp_tx,
    });
    if sent.is_err() {
        return (StatusCode::SERVICE_UNAVAILABLE, "no Ling http_serve loop running").into_response();
    }

    match resp_rx.await {
        Ok(resp) => {
            let status = StatusCode::from_u16(resp.status).unwrap_or(StatusCode::OK);
            let mut r = (status, resp.body).into_response();
            if let Ok(value) = resp.content_type.parse() {
                r.headers_mut().insert(axum::http::header::CONTENT_TYPE, value);
            }
            if let Some(sc) = &resp.set_cookie {
                if let Ok(value) = sc.parse() {
                    r.headers_mut().insert(axum::http::header::SET_COOKIE, value);
                }
            }
            if let Some(loc) = &resp.location {
                if let Ok(value) = loc.parse() {
                    r.headers_mut().insert(axum::http::header::LOCATION, value);
                }
            }
            // Anti-clickjacking: forbid this page from being embedded in any
            // <iframe>/<frame>/<embed>. `X-Frame-Options: DENY` covers legacy
            // browsers; CSP `frame-ancestors 'none'` is the modern equivalent
            // (and the only one that reliably applies to `<embed>`/`<object>`).
            r.headers_mut().insert(
                axum::http::header::X_FRAME_OPTIONS,
                axum::http::HeaderValue::from_static("DENY"),
            );
            r.headers_mut().insert(
                axum::http::header::CONTENT_SECURITY_POLICY,
                axum::http::HeaderValue::from_static("frame-ancestors 'none'"),
            );
            r
        },
        Err(_) => (StatusCode::INTERNAL_SERVER_ERROR, "handler dropped the response").into_response(),
    }
}

/// Starts the async HTTP server on a background OS thread. Returns a
/// receiver the caller drains — blocking, on its own thread — to actually
/// answer requests. Non-blocking: returns as soon as the thread is spawned,
/// not once the server is actually bound (bind failures are logged to
/// stderr on the background thread instead of surfaced here, since binding
/// happens after this function has already returned).
///
/// `static_dirs` is `(url_prefix, disk_dir)` pairs registered by the `.ling`
/// program via `http_static` — served directly off disk as raw bytes through
/// `tower_http::services::ServeDir`, so binary files (fonts, images, zips)
/// never have to round-trip through the `String`-typed `Value`/`PendingRequest`
/// bridge that `.ling` route handlers use.
pub fn spawn_server(
    host: String,
    port: u16,
    static_dirs: Vec<(String, String)>,
) -> std::sync::mpsc::Receiver<PendingRequest> {
    let (tx, rx) = std::sync::mpsc::channel::<PendingRequest>();
    let state = ServerState { tx };

    std::thread::spawn(move || {
        let rt = match tokio::runtime::Runtime::new() {
            Ok(rt) => rt,
            Err(e) => {
                eprintln!("http_serve: failed to start async runtime: {e}");
                return;
            },
        };
        rt.block_on(async move {
            let addr = format!("{host}:{port}");
            let addr: std::net::SocketAddr = match addr.parse() {
                Ok(a) => a,
                Err(e) => {
                    eprintln!("http_serve: bad address '{addr}': {e}");
                    return;
                },
            };
            let mut router: Router<ServerState> = Router::new();
            for (prefix, dir) in &static_dirs {
                router = router.nest_service(prefix, tower_http::services::ServeDir::new(dir));
            }
            let router: Router = router.fallback(catch_all).with_state(state);
            if let Err(e) = ling_http::serve_http(router, addr).await {
                eprintln!("http_serve: {e}");
            }
        });
    });

    rx
}

/// Converts a Ling value returned from a route handler into an HTTP
/// response. A plain string is the response body (200, HTML). A `form`
/// struct with `status`/`body`/`content_type` fields gives full control;
/// any field it omits keeps the default.
pub fn value_to_response(v: &crate::runtime::Value) -> HttpResponse {
    use crate::runtime::Value;
    match v {
        Value::Str(s) => HttpResponse {
            status: 200,
            content_type: "text/html; charset=utf-8".to_string(),
            body: s.clone(),
            set_cookie: None,
            location: None,
        },
        Value::Struct { fields, .. } => {
            let mut resp = HttpResponse::default();
            resp.content_type = "text/html; charset=utf-8".to_string();
            for (k, val) in fields {
                match (k.as_str(), val) {
                    ("status", Value::Number(n)) => resp.status = *n as u16,
                    ("body", Value::Str(s)) => resp.body = s.clone(),
                    ("content_type", Value::Str(s)) => resp.content_type = s.clone(),
                    ("set_cookie", Value::Str(s)) if !s.is_empty() => resp.set_cookie = Some(s.clone()),
                    ("location", Value::Str(s)) if !s.is_empty() => resp.location = Some(s.clone()),
                    _ => {},
                }
            }
            resp
        },
        other => HttpResponse {
            status: 200,
            content_type: "text/plain; charset=utf-8".to_string(),
            body: other.to_string(),
            set_cookie: None,
            location: None,
        },
    }
}

// ═══════════════════════════════════════════════════════════════════════════
// Async HTTP jobs — `http_post_async`/`http_job_poll` builtins.
//
// The interpreter loop in `http_serve` (runtime/mod.rs) dispatches requests
// strictly one at a time on its own thread — deliberately not made concurrent,
// since `Value` (closures especially) holds `Rc` and isn't `Send`. That's fine
// for fast handlers (rendering HTML, checking a proof-of-work nonce, reading a
// small file), but a slow external call — a local Stable Diffusion generation
// can take 10-30s — would otherwise block every other visitor for that whole
// time. So instead of making the interpreter concurrent, only the slow call
// itself runs off-thread: `http_post_async` fires the request on a background
// tokio runtime and returns immediately with a job id; `http_job_poll` is a
// fast, non-blocking lookup a `.ling` handler calls on a later, separate
// request to check whether the result has arrived yet.
// ═══════════════════════════════════════════════════════════════════════════

type JobMap = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Option<String>>>>;

#[derive(Clone, Default)]
pub struct AsyncJobs(JobMap);

impl AsyncJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Starts a POST request in the background; returns a job id immediately.
    /// The job's `Some(body)` (or an `{"error": "..."}` JSON string on failure)
    /// becomes visible to `poll` once the request completes.
    pub fn start_post(&self, url: String, content_type: String, body: String) -> String {
        let id = {
            use rand::RngCore;
            let mut buf = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut buf);
            buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        self.0.lock().unwrap().insert(id.clone(), None);

        let jobs = self.0.clone();
        let job_id = id.clone();
        async_runtime_handle().spawn(async move {
            let client = reqwest::Client::new();
            let result = client
                .post(&url)
                .header(reqwest::header::CONTENT_TYPE, content_type)
                .body(body)
                .send()
                .await;
            let text = match result {
                Ok(resp) => resp
                    .text()
                    .await
                    .unwrap_or_else(|e| format!("{{\"error\":\"body read failed: {e}\"}}")),
                Err(e) => format!("{{\"error\":\"{}\"}}", e.to_string().replace('"', "'")),
            };
            jobs.lock().unwrap().insert(job_id, Some(text));
        });

        id
    }

    /// Non-blocking: `None` while the job is still running, `Some(body)` once done.
    /// A never-registered id also reads as still-running (`None`) rather than erroring.
    pub fn poll(&self, id: &str) -> Option<String> {
        self.0.lock().unwrap().get(id).cloned().flatten()
    }

    /// Starts an AUTOMATIC1111-compatible `sdapi/v1/txt2img` request in the
    /// background. Unlike `start_post`, the JSON *response* is also parsed
    /// here (via `serde_json`, already pulled in through `ling_http`) so the
    /// job's eventual result is the plain base64 PNG string from
    /// `images[0]` — `.ling` has no JSON parser of its own, so pushing this
    /// one SDAI-specific bit of JSON handling into Rust avoids needing one.
    /// On failure the result starts with `"ERROR:"`, which `.ling` code can
    /// check with a plain string-prefix comparison instead of parsing JSON.
    pub fn start_sdai_txt2img(&self, base_url: String, prompt: String, width: u32, height: u32) -> String {
        let id = {
            use rand::RngCore;
            let mut buf = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut buf);
            buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        self.0.lock().unwrap().insert(id.clone(), None);

        let jobs = self.0.clone();
        let job_id = id.clone();
        async_runtime_handle().spawn(async move {
            let url = format!("{}/sdapi/v1/txt2img", base_url.trim_end_matches('/'));
            let payload = ling_http::serde_json::json!({
                "prompt": prompt,
                "negative_prompt": "blurry, lowres, watermark, text, signature",
                "steps": 24,
                "cfg_scale": 7,
                "width": width,
                "height": height,
                "sampler_name": "Euler a",
            });
            let client = reqwest::Client::new();
            let result = client
                .post(&url)
                .timeout(std::time::Duration::from_secs(180))
                .json(&payload)
                .send()
                .await;
            let outcome = match result {
                Ok(resp) => {
                    if !resp.status().is_success() {
                        format!("ERROR: SDAI returned HTTP {}", resp.status())
                    } else {
                        match resp.json::<ling_http::serde_json::Value>().await {
                            Ok(v) => match v.get("images").and_then(|im| im.get(0)).and_then(|s| s.as_str()) {
                                Some(b64) => b64.to_string(),
                                None => "ERROR: no images[0] in SDAI response".to_string(),
                            },
                            Err(e) => format!("ERROR: bad JSON from SDAI: {e}"),
                        }
                    }
                },
                Err(e) => format!("ERROR: {}", e.to_string().replace('"', "'")),
            };
            jobs.lock().unwrap().insert(job_id, Some(outcome));
        });

        id
    }
}

/// One Google OAuth sign-in outcome: the three profile fields chat.ling-lang.org
/// needs, or an error. Kept as its own small struct (rather than reusing
/// `AsyncJobs`' single-`String` `JobMap`) because a login result has three
/// fields, not one — same "keep the JSON-specific bit in Rust" reasoning as
/// `start_sdai_txt2img`, just with more fields to hand back.
#[derive(Clone, Default)]
struct OAuthResult {
    error: String,
    sub: String,
    email: String,
    name: String,
}

type OAuthMap = std::sync::Arc<std::sync::Mutex<std::collections::HashMap<String, Option<OAuthResult>>>>;

#[derive(Clone, Default)]
pub struct OAuthJobs(OAuthMap);

impl OAuthJobs {
    pub fn new() -> Self {
        Self::default()
    }

    /// Exchanges an authorization `code` for Google's token, then fetches the
    /// signed-in user's profile — both are outbound HTTPS calls, so this runs
    /// in the background exactly like `AsyncJobs::start_post` and is polled
    /// the same non-blocking way. `client_secret` never leaves this function;
    /// it's read from a `.env` file by `.ling` code and passed in as a plain
    /// argument purely to make one server-to-server token-exchange call.
    pub fn start_google_login(
        &self,
        code: String,
        client_id: String,
        client_secret: String,
        redirect_uri: String,
    ) -> String {
        let id = {
            use rand::RngCore;
            let mut buf = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut buf);
            buf.iter().map(|b| format!("{b:02x}")).collect::<String>()
        };
        self.0.lock().unwrap().insert(id.clone(), None);

        let jobs = self.0.clone();
        let job_id = id.clone();
        async_runtime_handle().spawn(async move {
            let client = reqwest::Client::new();
            let outcome = async {
                let token_resp = client
                    .post("https://oauth2.googleapis.com/token")
                    .form(&[
                        ("code", code.as_str()),
                        ("client_id", client_id.as_str()),
                        ("client_secret", client_secret.as_str()),
                        ("redirect_uri", redirect_uri.as_str()),
                        ("grant_type", "authorization_code"),
                    ])
                    .send()
                    .await
                    .map_err(|e| format!("token request failed: {e}"))?;
                if !token_resp.status().is_success() {
                    let status = token_resp.status();
                    let body = token_resp.text().await.unwrap_or_default();
                    return Err(format!("Google returned HTTP {status} exchanging code: {body}"));
                }
                let token_json: ling_http::serde_json::Value = token_resp
                    .json()
                    .await
                    .map_err(|e| format!("bad JSON from Google token endpoint: {e}"))?;
                let access_token = token_json
                    .get("access_token")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| "no access_token in Google's response".to_string())?;

                let userinfo_resp = client
                    .get("https://www.googleapis.com/oauth2/v3/userinfo")
                    .bearer_auth(access_token)
                    .send()
                    .await
                    .map_err(|e| format!("userinfo request failed: {e}"))?;
                if !userinfo_resp.status().is_success() {
                    return Err(format!("Google returned HTTP {} fetching userinfo", userinfo_resp.status()));
                }
                let profile: ling_http::serde_json::Value = userinfo_resp
                    .json()
                    .await
                    .map_err(|e| format!("bad JSON from Google userinfo endpoint: {e}"))?;
                let sub = profile.get("sub").and_then(|v| v.as_str()).unwrap_or_default();
                if sub.is_empty() {
                    return Err("no sub in Google's userinfo response".to_string());
                }
                Ok(OAuthResult {
                    error: String::new(),
                    sub: sub.to_string(),
                    email: profile.get("email").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                    name: profile.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_string(),
                })
            }
            .await
            .unwrap_or_else(|e| OAuthResult { error: e, ..Default::default() });

            jobs.lock().unwrap().insert(job_id, Some(outcome));
        });

        id
    }

    fn get(&self, id: &str) -> Option<OAuthResult> {
        self.0.lock().unwrap().get(id).cloned().flatten()
    }

    pub fn done(&self, id: &str) -> bool {
        matches!(self.0.lock().unwrap().get(id), Some(Some(_)))
    }

    pub fn error(&self, id: &str) -> String {
        self.get(id).map(|r| r.error).unwrap_or_default()
    }

    pub fn sub(&self, id: &str) -> String {
        self.get(id).map(|r| r.sub).unwrap_or_default()
    }

    pub fn email(&self, id: &str) -> String {
        self.get(id).map(|r| r.email).unwrap_or_default()
    }

    pub fn name(&self, id: &str) -> String {
        self.get(id).map(|r| r.name).unwrap_or_default()
    }
}

/// A background tokio runtime dedicated to `AsyncJobs`, independent of whether
/// `http_serve`/`spawn_server`'s own server runtime is running — so
/// `http_post_async` also works from a plain script, not just inside a route
/// handler. Started lazily on first use and kept alive for the process lifetime.
fn async_runtime_handle() -> tokio::runtime::Handle {
    static HANDLE: std::sync::OnceLock<tokio::runtime::Handle> = std::sync::OnceLock::new();
    HANDLE
        .get_or_init(|| {
            let (tx, rx) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let rt = tokio::runtime::Runtime::new().expect("ling: failed to start async-job tokio runtime");
                tx.send(rt.handle().clone()).expect("ling: async-job runtime handle send failed");
                rt.block_on(std::future::pending::<()>());
            });
            rx.recv().expect("ling: async-job runtime handle recv failed")
        })
        .clone()
}
