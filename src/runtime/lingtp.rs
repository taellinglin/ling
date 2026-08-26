// src/runtime/lingtp.rs — "lingtp://": a from-scratch, post-quantum secure
// transport (client + server), mirroring how https:// relates to http:// —
// see src/runtime/web.rs for the plain/TLS HTTP side this is the PQ analog
// of. Fetches a page over an encrypted, authenticated channel the way
// http_get/http_serve do over TLS, except the handshake uses ML-KEM (via the
// X25519+ML-KEM-768 hybrid in ling-crypto) for confidentiality and ML-DSA-87
// for server authentication — both NIST-standardized (FIPS 203 / FIPS 204)
// post-quantum primitives, not experimental ones.
//
// ── Handshake (mirrors TLS 1.3's shape, adapted for a KEM instead of DH) ──
//
//   Client -> Server   ClientHello { client_random[32] }
//   Server -> Client   ServerHello { server_random[32], ephemeral hybrid
//                                    pubkey[1216], ML-DSA-87 pubkey,
//                                    signature over the transcript so far }
//   [client verifies the signature, then checks the ML-DSA-87 pubkey
//    against a local known_hosts-style store — trust-on-first-use, same
//    model as this codebase's ssh_connect/known_hosts]
//   Client -> Server   ClientKeyExchange { KEM ciphertext[1120],
//                                          client_finished[32] }
//   [server decapsulates, derives session keys, and checks client_finished
//    — this check is NOT optional: ML-KEM's decapsulate has an "implicit
//    rejection" failure mode where a tampered/wrong ciphertext silently
//    yields a *different* shared secret instead of an error, so without a
//    keyed confirmation MAC over the transcript, a corrupted handshake
//    would go undetected]
//   Server -> Client   ServerFinished { server_finished[32] }
//   [client checks it — confirms the server actually derived the same key]
//
// Both sides then hold four independent keys, each an HKDF-SHA3 output of
// the raw KEM shared secret under a distinct label: two AEAD data keys (one
// per direction: client-writes/server-writes) and the two Finished keys
// already used above. Ephemeral KEM keypair per connection => forward
// secrecy; every AEAD frame carries its own fresh 24-byte nonce (see
// ling_crypto::symmetric::XChaCha20) => no nonce-reuse bookkeeping needed.
//
// ── Application layer ──────────────────────────────────────────────────
//
// One request/response pair per frame exchange, GET-only for v1 (matching
// http_get/http_route's shape): client sends one encrypted frame holding
// the requested path, server sends one encrypted frame back holding the
// response body, and the connection either closes or the client sends
// another request frame (keep-alive, like HTTP/1.1) — a script chooses via
// lingtp_get (opens, requests once, closes) or lingtp_serve (loops per
// connection until the client stops asking).
//
// ── What this is NOT ────────────────────────────────────────────────────
//
// A PKI/certificate-authority system. There is no chain of trust to a root
// CA the way https:// has — a fresh lingtp:// server presents a
// self-attested identity, exactly like a fresh SSH host does. Trust is
// earned the same way SSH's is: trust-on-first-use, with any later change
// refused rather than silently accepted (see `check_and_learn`, below).

use std::collections::{HashMap, VecDeque};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};

use ling_crypto::hybrid::{self, HybridKeypair};
use ling_crypto::{Blake3, MlDsa87Keypair, Sha3_256};

const MAX_QUEUE: usize = 4096;
const MAX_EVENTS: usize = 256;
/// A generous but bounded cap on any single frame (handshake field or data
/// payload) read off the wire — without this, a malicious/broken peer
/// claiming a huge length prefix could force an unbounded allocation.
const MAX_FRAME_LEN: u32 = 64 * 1024 * 1024;

// ── wire framing ─────────────────────────────────────────────────────────

fn write_frame(w: &mut impl Write, payload: &[u8]) -> std::io::Result<()> {
    w.write_all(&(payload.len() as u32).to_be_bytes())?;
    w.write_all(payload)?;
    w.flush()
}

fn read_frame(r: &mut impl Read) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME_LEN {
        return Err(std::io::Error::other("lingtp: frame exceeds max length"));
    }
    let mut buf = vec![0u8; len as usize];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

fn write_u16_prefixed(out: &mut Vec<u8>, data: &[u8]) {
    out.extend_from_slice(&(data.len() as u16).to_be_bytes());
    out.extend_from_slice(data);
}

fn take_u16_prefixed<'a>(buf: &'a [u8], pos: &mut usize) -> Result<&'a [u8], &'static str> {
    if buf.len() < *pos + 2 {
        return Err("truncated length prefix");
    }
    let len = u16::from_be_bytes([buf[*pos], buf[*pos + 1]]) as usize;
    *pos += 2;
    if buf.len() < *pos + len {
        return Err("truncated field");
    }
    let out = &buf[*pos..*pos + len];
    *pos += len;
    Ok(out)
}

fn take_fixed<'a, const N: usize>(buf: &'a [u8], pos: &mut usize) -> Result<&'a [u8; N], &'static str> {
    if buf.len() < *pos + N {
        return Err("truncated field");
    }
    let out: &[u8; N] = buf[*pos..*pos + N].try_into().unwrap();
    *pos += N;
    Ok(out)
}

// ── session keys ─────────────────────────────────────────────────────────

struct SessionKeys {
    client_finished_key: [u8; 32],
    server_finished_key: [u8; 32],
    c2s_key: [u8; 32],
    s2c_key: [u8; 32],
}

fn derive_session_keys(shared_secret: &[u8; 32], client_random: &[u8; 32], server_random: &[u8; 32]) -> SessionKeys {
    let salt = [client_random.as_slice(), server_random.as_slice()].concat();
    let derive = |label: &[u8]| -> [u8; 32] {
        let out = ling_crypto::hkdf_sha3(shared_secret, &salt, label, 32)
            .expect("hkdf_sha3: fixed 32-byte output never fails");
        let mut key = [0u8; 32];
        key.copy_from_slice(&out);
        key
    };
    SessionKeys {
        client_finished_key: derive(b"lingtp-v1 client-finished"),
        server_finished_key: derive(b"lingtp-v1 server-finished"),
        c2s_key: derive(b"lingtp-v1 client-to-server"),
        s2c_key: derive(b"lingtp-v1 server-to-client"),
    }
}

fn encrypt_frame(key: &[u8; 32], plaintext: &[u8]) -> Vec<u8> {
    ling_crypto::XChaCha20::new(*key)
        .encrypt(plaintext)
        .expect("XChaCha20 encrypt with a freshly generated nonce never fails")
}

fn decrypt_frame(key: &[u8; 32], ciphertext: &[u8]) -> Result<Vec<u8>, &'static str> {
    ling_crypto::XChaCha20::new(*key).decrypt(ciphertext)
}

// ── known_hosts (TOFU) ───────────────────────────────────────────────────

fn known_hosts_path() -> PathBuf {
    PathBuf::from(".lingtp/known_hosts")
}

/// `Ok(true)` = matches a previously-learned key, `Ok(false)` = never seen
/// this host before (caller should learn it), `Err` = a *different* key was
/// learned before — refuse, don't silently trust.
fn check_known_host(host_port: &str, pubkey_hex: &str) -> Result<bool, ()> {
    let Ok(text) = std::fs::read_to_string(known_hosts_path()) else { return Ok(false) };
    for line in text.lines() {
        if let Some((h, k)) = line.split_once(' ') {
            if h == host_port {
                return if k == pubkey_hex { Ok(true) } else { Err(()) };
            }
        }
    }
    Ok(false)
}

fn learn_known_host(host_port: &str, pubkey_hex: &str) {
    let path = known_hosts_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(&path) {
        let _ = writeln!(f, "{host_port} {pubkey_hex}");
    }
}

fn to_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

fn random_32() -> [u8; 32] {
    use rand::RngCore;
    let mut buf = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut buf);
    buf
}

// ── client: connect out and fetch one resource ──────────────────────────

/// A completed, verified handshake, ready for request/response frames.
struct ClientSession {
    stream: TcpStream,
    keys: SessionKeys,
}

fn client_handshake(host: &str, port: u16) -> Result<ClientSession, String> {
    let mut stream = TcpStream::connect((host, port)).map_err(|e| e.to_string())?;
    let _ = stream.set_nodelay(true);

    let client_random = random_32();
    write_frame(&mut stream, &client_random).map_err(|e| e.to_string())?;

    let server_hello = read_frame(&mut stream).map_err(|e| e.to_string())?;
    let mut pos = 0;
    let server_random: [u8; 32] =
        *take_fixed::<32>(&server_hello, &mut pos).map_err(|e| e.to_string())?;
    let ephemeral_pubkey: [u8; hybrid::PUBLIC_KEY_LEN] =
        take_fixed::<{ hybrid::PUBLIC_KEY_LEN }>(&server_hello, &mut pos)
            .map_err(|e| e.to_string())?
            .to_owned()
            .try_into()
            .unwrap();
    let ml_dsa_pubkey = take_u16_prefixed(&server_hello, &mut pos).map_err(|e| e.to_string())?;
    let sig = take_u16_prefixed(&server_hello, &mut pos).map_err(|e| e.to_string())?;

    let transcript_1 = Sha3_256::hash_many(&[&client_random, &server_random, &ephemeral_pubkey, ml_dsa_pubkey]);
    MlDsa87Keypair::verify(ml_dsa_pubkey, &transcript_1, sig)
        .map_err(|_| "SIGNATURE_INVALID".to_string())?;

    let host_port = format!("{host}:{port}");
    let pubkey_hex = to_hex(ml_dsa_pubkey);
    match check_known_host(&host_port, &pubkey_hex) {
        Ok(true) => {},
        Ok(false) => learn_known_host(&host_port, &pubkey_hex),
        Err(()) => return Err("HOST_KEY_CHANGED".to_string()),
    }

    let (kem_ct, shared_secret) = hybrid::encapsulate(&ephemeral_pubkey).map_err(|e| e.to_string())?;
    let keys = derive_session_keys(&shared_secret, &client_random, &server_random);

    let transcript_2 = Sha3_256::hash_many(&[&transcript_1, &kem_ct]);
    let client_finished = Blake3::keyed_hash(&keys.client_finished_key, &transcript_2);

    let mut cke = Vec::with_capacity(hybrid::CIPHERTEXT_LEN + 32);
    cke.extend_from_slice(&kem_ct);
    cke.extend_from_slice(&client_finished);
    write_frame(&mut stream, &cke).map_err(|e| e.to_string())?;

    let server_finished_msg = read_frame(&mut stream).map_err(|e| e.to_string())?;
    if server_finished_msg.len() != 32 {
        return Err("malformed ServerFinished".to_string());
    }
    let transcript_3 = Sha3_256::hash_many(&[&transcript_2, &client_finished]);
    let expected_server_finished = Blake3::keyed_hash(&keys.server_finished_key, &transcript_3);
    if expected_server_finished.as_slice() != server_finished_msg.as_slice() {
        return Err("SERVER_FINISHED_MISMATCH".to_string());
    }

    Ok(ClientSession { stream, keys })
}

/// Connect, request `path`, return the response body. One request per
/// connection (like HTTP/1.0) — simplest correct thing for a "fetch this
/// page" call; `lingtp_serve` on the other side loops per connection so a
/// future keep-alive client isn't precluded.
///
/// Returns `(status, body)`. status: 1 = ok, 0 = connect/handshake error,
/// -1 = signature invalid, -2 = host key changed (refused, possible MITM).
fn fetch(host: &str, port: u16, path: &str) -> (i32, String) {
    let mut session = match client_handshake(host, port) {
        Ok(s) => s,
        Err(e) => {
            return match e.as_str() {
                "SIGNATURE_INVALID" => (-1, String::new()),
                "HOST_KEY_CHANGED" => (-2, String::new()),
                _ => (0, String::new()),
            }
        },
    };

    let req = encrypt_frame(&session.keys.c2s_key, path.as_bytes());
    if write_frame(&mut session.stream, &req).is_err() {
        return (0, String::new());
    }
    let resp = match read_frame(&mut session.stream) {
        Ok(r) => r,
        Err(_) => return (0, String::new()),
    };
    match decrypt_frame(&session.keys.s2c_key, &resp) {
        Ok(body) => (1, String::from_utf8_lossy(&body).into_owned()),
        Err(_) => (0, String::new()),
    }
}

pub fn get(host: &str, port: u16, path: &str) -> (i32, String) {
    fetch(host, port, path)
}

// ── server: accept connections and answer requests ──────────────────────

#[derive(Clone)]
struct PendingConfig {
    port: u16,
    host_key_path: PathBuf,
}

static PENDING: Mutex<Option<PendingConfig>> = Mutex::new(None);

pub fn configure(port: u16, host_key_path: &str) {
    let path = if host_key_path.is_empty() {
        PathBuf::from(".lingtp/host_mldsa87")
    } else {
        PathBuf::from(host_key_path)
    };
    if let Ok(mut g) = PENDING.lock() {
        *g = Some(PendingConfig { port, host_key_path: path });
    }
}

fn load_or_create_host_key(path: &std::path::Path) -> std::io::Result<MlDsa87Keypair> {
    if let Ok(seed_hex) = std::fs::read_to_string(path) {
        if let Ok(seed_bytes) = hex_decode(seed_hex.trim()) {
            if let Ok(seed) = <[u8; 32]>::try_from(seed_bytes.as_slice()) {
                return Ok(MlDsa87Keypair::from_seed(seed));
            }
        }
    }
    let kp = MlDsa87Keypair::generate();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    std::fs::write(path, to_hex(&*kp.to_bytes()))?;
    Ok(kp)
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if s.len() % 2 != 0 {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

pub struct PendingRequest {
    pub path: String,
    pub respond_to: std::sync::mpsc::Sender<String>,
}

struct ServeShared {
    identity: MlDsa87Keypair,
    identity_pubkey: Vec<u8>,
    status: AtomicU8,
    inbound: Mutex<VecDeque<(u64, PendingRequest)>>,
    events: Mutex<VecDeque<String>>,
    clients: Mutex<HashMap<u64, ()>>,
    next_id: AtomicU64,
    accept_stop: Arc<std::sync::atomic::AtomicBool>,
}

fn push_event(shared: &ServeShared, line: String) {
    if let Ok(mut g) = shared.events.lock() {
        if g.len() >= MAX_EVENTS {
            g.pop_front();
        }
        g.push_back(line);
    }
}

static SERVE: Mutex<Option<Arc<ServeShared>>> = Mutex::new(None);

fn serve_shared() -> Option<Arc<ServeShared>> {
    SERVE.lock().ok().and_then(|g| g.clone())
}

/// `Err` carries a short reason (`connect:read_client_hello`-style tag,
/// no sensitive detail) so `handle_connection` can log *why* a handshake
/// failed to the events queue — the same auditability `ssh_serve_events()`
/// already gives its `authfail:` entries.
fn server_handshake(
    stream: &mut TcpStream,
    identity: &MlDsa87Keypair,
    identity_pubkey: &[u8],
) -> Result<SessionKeys, &'static str> {
    let client_hello = read_frame(stream).map_err(|_| "read_client_hello")?;
    if client_hello.len() != 32 {
        return Err("bad_client_hello_len");
    }
    let mut client_random = [0u8; 32];
    client_random.copy_from_slice(&client_hello);

    let ephemeral = HybridKeypair::generate();
    let ephemeral_pubkey = ephemeral.public_key();
    let server_random = random_32();

    let transcript_1 = Sha3_256::hash_many(&[&client_random, &server_random, &ephemeral_pubkey, identity_pubkey]);
    let sig = identity.sign(&transcript_1);

    let mut hello = Vec::with_capacity(32 + hybrid::PUBLIC_KEY_LEN + 4 + identity_pubkey.len() + sig.len());
    hello.extend_from_slice(&server_random);
    hello.extend_from_slice(&ephemeral_pubkey);
    write_u16_prefixed(&mut hello, identity_pubkey);
    write_u16_prefixed(&mut hello, &sig);
    write_frame(stream, &hello).map_err(|_| "write_server_hello")?;

    let cke = read_frame(stream).map_err(|_| "read_client_key_exchange")?;
    if cke.len() != hybrid::CIPHERTEXT_LEN + 32 {
        return Err("bad_client_key_exchange_len");
    }
    let (kem_ct, client_finished) = cke.split_at(hybrid::CIPHERTEXT_LEN);
    let shared_secret = ephemeral.decapsulate(kem_ct).map_err(|_| "decapsulate")?;
    let keys = derive_session_keys(&shared_secret, &client_random, &server_random);

    let transcript_2 = Sha3_256::hash_many(&[&transcript_1, kem_ct]);
    let expected_client_finished = Blake3::keyed_hash(&keys.client_finished_key, &transcript_2);
    // Constant-time-ish compare isn't critical here (the attacker already
    // has to have broken the KEM or the signature to reach this point with
    // a value worth timing against), but there's no reason not to use it.
    use subtle::ConstantTimeEq;
    if expected_client_finished.ct_eq(client_finished).unwrap_u8() != 1 {
        return Err("client_finished_mismatch");
    }

    let transcript_3 = Sha3_256::hash_many(&[&transcript_2, client_finished]);
    let server_finished = Blake3::keyed_hash(&keys.server_finished_key, &transcript_3);
    write_frame(stream, &server_finished).map_err(|_| "write_server_finished")?;

    Ok(keys)
}

fn handle_connection(mut stream: TcpStream, id: u64, shared: Arc<ServeShared>) {
    let keys = match server_handshake(&mut stream, &shared.identity, &shared.identity_pubkey) {
        Ok(k) => k,
        Err(reason) => {
            push_event(&shared, format!("handshake_failed:{id}:{reason}"));
            return;
        },
    };
    if let Ok(mut m) = shared.clients.lock() {
        m.insert(id, ());
    }
    shared.status.store(2, Ordering::SeqCst);
    push_event(&shared, format!("connect:{id}"));

    loop {
        let req_frame = match read_frame(&mut stream) {
            Ok(f) => f,
            Err(_) => break,
        };
        let path = match decrypt_frame(&keys.c2s_key, &req_frame) {
            Ok(p) => String::from_utf8_lossy(&p).into_owned(),
            Err(_) => break,
        };

        let (resp_tx, resp_rx) = std::sync::mpsc::channel::<String>();
        if let Ok(mut q) = shared.inbound.lock() {
            if q.len() >= MAX_QUEUE {
                q.pop_front();
            }
            q.push_back((id, PendingRequest { path, respond_to: resp_tx }));
        }
        let body = resp_rx.recv().unwrap_or_default();
        let resp_frame = encrypt_frame(&keys.s2c_key, body.as_bytes());
        if write_frame(&mut stream, &resp_frame).is_err() {
            break;
        }
    }

    if let Ok(mut m) = shared.clients.lock() {
        m.remove(&id);
        if m.is_empty() {
            shared.status.store(1, Ordering::SeqCst);
        }
    }
    push_event(&shared, format!("disconnect:{id}"));
}

/// Starts listening with the config staged by `configure()`. Returns false
/// if unconfigured or the host identity key / socket bind fails.
pub fn serve() -> bool {
    let Some(cfg) = PENDING.lock().ok().and_then(|g| g.clone()) else { return false };
    let Ok(identity) = load_or_create_host_key(&cfg.host_key_path) else { return false };
    let identity_pubkey = identity.public_key();

    let accept_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let shared = Arc::new(ServeShared {
        identity,
        identity_pubkey,
        status: AtomicU8::new(1),
        inbound: Mutex::new(VecDeque::new()),
        events: Mutex::new(VecDeque::new()),
        clients: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        accept_stop: accept_stop.clone(),
    });
    if let Ok(mut g) = SERVE.lock() {
        *g = Some(shared.clone());
    }

    let listener = match TcpListener::bind(("0.0.0.0", cfg.port)) {
        Ok(l) => l,
        Err(_) => {
            shared.status.store(0, Ordering::SeqCst);
            return false;
        },
    };
    let _ = listener.set_nonblocking(true);

    std::thread::spawn(move || {
        loop {
            if accept_stop.load(Ordering::SeqCst) {
                break;
            }
            let stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    continue;
                },
                Err(_) => continue,
            };
            // On Windows, a socket accepted off a non-blocking listener can
            // inherit the non-blocking flag — the rest of this module (the
            // handshake and request loop) assumes blocking I/O throughout,
            // so this must be reset explicitly or read_exact fails
            // immediately with WouldBlock instead of actually waiting.
            let _ = stream.set_nonblocking(false);
            let _ = stream.set_nodelay(true);
            let id = shared.next_id.fetch_add(1, Ordering::SeqCst);
            let shared2 = shared.clone();
            std::thread::spawn(move || handle_connection(stream, id, shared2));
        }
    });

    true
}

pub fn stop() {
    if let Ok(mut g) = SERVE.lock() {
        if let Some(shared) = g.take() {
            shared.accept_stop.store(true, Ordering::SeqCst);
            shared.status.store(0, Ordering::SeqCst);
        }
    }
}

/// Blocking receive of the next pending request across all connected
/// clients — mirrors `http_serve`'s dispatch loop shape (drained on the
/// interpreter's own thread), but since this is a plain `mpsc` (not a
/// `Receiver<PendingRequest>` returned once like `web::spawn_server`), the
/// caller polls this in a loop instead of `for pending in rx`.
pub fn recv_request() -> Option<PendingRequest> {
    let shared = serve_shared()?;
    let mut q = shared.inbound.lock().ok()?;
    q.pop_front().map(|(_, req)| req)
}

pub fn status() -> u8 {
    serve_shared().map(|s| s.status.load(Ordering::SeqCst)).unwrap_or(0)
}

pub fn client_count() -> usize {
    serve_shared().and_then(|s| s.clients.lock().ok().map(|m| m.len())).unwrap_or(0)
}

pub fn events() -> String {
    let Some(shared) = serve_shared() else { return String::new() };
    let Ok(mut q) = shared.events.lock() else { return String::new() };
    let out: Vec<String> = q.drain(..).collect();
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn deadline_poll(mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(5) {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        false
    }

    fn cleanup() {
        stop();
        if let Ok(mut g) = PENDING.lock() {
            *g = None;
        }
        let _ = std::fs::remove_dir_all(".lingtp-test");
        let _ = std::fs::remove_dir_all(".lingtp");
    }

    /// Drains one request off the server's inbound queue and answers it —
    /// shared by every test that needs a live responder without spinning up
    /// a full `lingtp_serve` builtin loop.
    fn respond_once(body: impl Into<String> + Send + 'static) -> std::thread::JoinHandle<String> {
        std::thread::spawn(move || {
            let start = Instant::now();
            loop {
                if let Some(req) = recv_request() {
                    let path = req.path.clone();
                    let _ = req.respond_to.send(body.into());
                    return path;
                }
                assert!(start.elapsed() < Duration::from_secs(5), "request never arrived");
                std::thread::sleep(Duration::from_millis(10));
            }
        })
    }

    #[test]
    fn request_response_round_trip_over_two_separate_connections() {
        let _g = TEST_LOCK.lock().unwrap();
        cleanup();
        configure(19301, ".lingtp-test/host_19301");
        assert!(serve(), "serve() must succeed");
        assert!(deadline_poll(|| status() >= 1), "status must reach 1 (listening)");

        let responder = respond_once("echo-response");
        let (status, body) = fetch("127.0.0.1", 19301, "/hello");
        let seen_path = responder.join().unwrap();
        assert_eq!(status, 1, "fetch must succeed, got body={body:?}");
        assert_eq!(body, "echo-response");
        assert_eq!(seen_path, "/hello");

        // A second, independent connection must work identically — proves
        // the server's per-connection handshake/cleanup doesn't leak state
        // that would only break a *subsequent* connection.
        let responder2 = respond_once("second-response");
        let (status2, body2) = fetch("127.0.0.1", 19301, "/again");
        let seen_path2 = responder2.join().unwrap();
        assert_eq!(status2, 1);
        assert_eq!(body2, "second-response");
        assert_eq!(seen_path2, "/again");

        cleanup();
    }

    #[test]
    fn known_hosts_rejects_a_changed_host_key() {
        let _g = TEST_LOCK.lock().unwrap();
        cleanup();
        let _ = std::fs::remove_file(known_hosts_path());

        // First server instance: client connects, TOFU-learns its identity key.
        configure(19302, ".lingtp-test/host_a");
        assert!(serve());
        assert!(deadline_poll(|| status() >= 1));
        let responder = respond_once("first");
        let (status1, _) = fetch("127.0.0.1", 19302, "/");
        responder.join().unwrap();
        assert_eq!(status1, 1, "first connection must succeed and learn the key");
        stop();
        assert!(deadline_poll(|| status() == 0));

        // Second server instance, same port, a DIFFERENT identity key: a
        // client trusting the first key must refuse, not silently re-learn.
        // stop() flips `status` to 0 immediately but the accept thread only
        // notices accept_stop on its next ~20ms poll tick, so the listening
        // socket isn't necessarily released the instant status() reads 0 —
        // retry the bind rather than asserting it succeeds on the first try.
        configure(19302, ".lingtp-test/host_b");
        assert!(
            deadline_poll(|| serve()),
            "port must become bindable again shortly after stop()"
        );
        assert!(deadline_poll(|| status() >= 1));
        let (status2, body2) = fetch("127.0.0.1", 19302, "/");
        assert_eq!(
            status2, -2,
            "a changed host identity key must be refused (-2), not silently trusted; body={body2:?}"
        );

        stop();
        let _ = std::fs::remove_file(known_hosts_path());
        cleanup();
    }

    /// Simulates a malicious/broken peer sending a `ClientKeyExchange` whose
    /// KEM ciphertext doesn't match what it originally encapsulated. ML-KEM's
    /// decapsulate has "implicit rejection": it returns a *different* shared
    /// secret instead of an error, so without the Finished-MAC check this
    /// would silently proceed with mismatched keys instead of the server
    /// refusing the connection outright.
    #[test]
    fn tampered_kem_ciphertext_is_rejected_not_silently_accepted() {
        let _g = TEST_LOCK.lock().unwrap();
        cleanup();
        configure(19303, ".lingtp-test/host_19303");
        assert!(serve());
        assert!(deadline_poll(|| status() >= 1));

        let mut stream = TcpStream::connect("127.0.0.1:19303").unwrap();
        let client_random = random_32();
        write_frame(&mut stream, &client_random).unwrap();

        let server_hello = read_frame(&mut stream).unwrap();
        let mut pos = 0;
        let server_random: [u8; 32] = *take_fixed::<32>(&server_hello, &mut pos).unwrap();
        let ephemeral_pubkey: [u8; hybrid::PUBLIC_KEY_LEN] =
            take_fixed::<{ hybrid::PUBLIC_KEY_LEN }>(&server_hello, &mut pos).unwrap().to_owned().try_into().unwrap();
        let ml_dsa_pubkey = take_u16_prefixed(&server_hello, &mut pos).unwrap();
        let sig = take_u16_prefixed(&server_hello, &mut pos).unwrap();
        let transcript_1 = Sha3_256::hash_many(&[&client_random, &server_random, &ephemeral_pubkey, ml_dsa_pubkey]);
        MlDsa87Keypair::verify(ml_dsa_pubkey, &transcript_1, sig).expect("server's own signature must verify");

        let (mut kem_ct, shared_secret) = hybrid::encapsulate(&ephemeral_pubkey).unwrap();
        let keys = derive_session_keys(&shared_secret, &client_random, &server_random);
        // Tamper with the ciphertext AFTER computing the finished-MAC over
        // the correct one, so the MAC no longer matches what the (tampered)
        // ciphertext would actually decapsulate to on the server side.
        let last = kem_ct.len() - 1;
        kem_ct[last] ^= 0xFF;
        let transcript_2 = Sha3_256::hash_many(&[&transcript_1, &kem_ct]);
        let client_finished = Blake3::keyed_hash(&keys.client_finished_key, &transcript_2);

        let mut cke = Vec::new();
        cke.extend_from_slice(&kem_ct);
        cke.extend_from_slice(&client_finished);
        write_frame(&mut stream, &cke).unwrap();

        // The server must NOT send a valid ServerFinished back — either it
        // closes the connection outright, or (if it did reply) the reply
        // must not be the 32-byte confirmation a genuine handshake gets.
        let mut len_buf = [0u8; 4];
        let read_result = stream.read_exact(&mut len_buf);
        if read_result.is_ok() {
            let len = u32::from_be_bytes(len_buf);
            // A genuine ServerFinished is exactly 32 bytes; refuse to treat
            // an incidental 32-byte-length read as proof of anything else,
            // but a *matching* one here would be the real bug.
            assert_ne!(len, 32, "server must not complete the handshake for a tampered ciphertext");
        }
        // No request should ever have been queued for this connection.
        assert!(recv_request().is_none(), "a rejected handshake must never reach the request queue");

        stop();
        cleanup();
    }

    /// A signature that doesn't match the claimed public key/transcript
    /// must be rejected client-side — simulates a malicious server (or a
    /// MITM without the real private key) presenting a ServerHello it can't
    /// actually back up.
    #[test]
    fn forged_signature_is_rejected_by_the_client() {
        let _g = TEST_LOCK.lock().unwrap();
        cleanup();

        let listener = TcpListener::bind("127.0.0.1:19304").unwrap();
        let handle = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let client_hello = read_frame(&mut stream).unwrap();
            let mut client_random = [0u8; 32];
            client_random.copy_from_slice(&client_hello);

            let ephemeral = HybridKeypair::generate();
            let ephemeral_pubkey = ephemeral.public_key();
            let server_random = random_32();

            // Sign with a DIFFERENT identity than the one advertised in the
            // ServerHello — a client checking the signature against the
            // advertised key must reject this.
            let real_identity = MlDsa87Keypair::generate();
            let advertised_pubkey = real_identity.public_key();
            let attacker_identity = MlDsa87Keypair::generate();
            let transcript_1 = Sha3_256::hash_many(&[&client_random, &server_random, &ephemeral_pubkey, &advertised_pubkey]);
            let forged_sig = attacker_identity.sign(&transcript_1);

            let mut hello = Vec::new();
            hello.extend_from_slice(&server_random);
            hello.extend_from_slice(&ephemeral_pubkey);
            write_u16_prefixed(&mut hello, &advertised_pubkey);
            write_u16_prefixed(&mut hello, &forged_sig);
            write_frame(&mut stream, &hello).unwrap();
        });

        let (status, body) = fetch("127.0.0.1", 19304, "/");
        handle.join().unwrap();
        assert_eq!(status, -1, "a forged signature must be rejected, body={body:?}");

        cleanup();
    }
}
