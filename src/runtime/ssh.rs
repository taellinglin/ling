// src/runtime/ssh.rs — real SSH client + server (via the `russh` crate),
// mirroring net.rs's global-singleton / polling-builtin style so `.ling`
// scripts drive it the same way they drive net_host/net_join.
//
// Client (connecting OUT to another computer's real sshd, or another ling
// process running ssh_serve):
//   ssh_connect(host, port, user, password) → 1 = connected+authenticated
//                                              0 = wrong user/password
//                                             -1 = connect/handshake error
//                                             -2 = the remote host key does not
//                                                  match what we saw last time
//                                                  (refused — possible MITM)
//   ssh_exec(cmd)        → remote stdout, blocking until the command exits
//   ssh_exit_code()      → exit code of the last ssh_exec (-1 if none yet)
//   ssh_client_status()  → 0 idle/disconnected, 2 connected
//   ssh_close()          → disconnect
//
// Server (accepting connections FROM other computers):
//   ssh_config(port, user, password, host_key_path) → stage server config;
//     host_key_path defaults to .ling-ssh/host_ed25519 (generated once and
//     reused on every later ssh_serve(), so returning clients get a stable,
//     verifiable host identity instead of a fresh unverifiable key each run)
//   ssh_serve()               → start listening (false if unconfigured or the
//                                configured password is blank — a server is
//                                never started with no password)
//   ssh_stop()                → stop listening, drop all clients
//   ssh_serve_send(id, text)  → write one line to a connected client's channel
//   ssh_serve_recv()          → oldest queued "<id>|<text>" from any client,
//                                message only ("" if none) — fed by both
//                                interactive shell input and exec commands
//   ssh_serve_exit(id, code)  → send exit-status + EOF + close for a client's
//                                channel (call this once you're done replying
//                                to an exec request)
//   ssh_serve_clients()       → connected client ids, comma-separated
//   ssh_serve_client_count()
//   ssh_serve_events()        → drains "connect:id" / "disconnect:id" /
//                                "authfail:user" log, newline-joined
//   ssh_serve_status()        → 0 idle, 1 listening, 2 >=1 client connected
//
// Security posture:
//   - The server accepts password authentication ONLY. publickey / none /
//     keyboard-interactive are not in the advertised method set at all, so
//     russh never even asks the Handler about them (see `ssh_serve`'s
//     MethodSet below) — there is no path to an unauthenticated session.
//   - ssh_serve() refuses to start with a blank configured password rather
//     than silently listening with none.
//   - Password comparison is constant-time (`ct_eq`); russh additionally
//     enforces a constant `auth_rejection_time` regardless of what the
//     Handler does, so failed attempts don't leak timing either way.
//   - The host key is generated once (ed25519, via a CSPRNG) and persisted,
//     not regenerated per launch — that's what makes the client's host-key
//     check below meaningful instead of training users to click through
//     "key changed" warnings.
//   - The client checks every server host key against a local known_hosts
//     file (russh's own `known_hosts` module — real OpenSSH-compatible
//     format and semantics, not a hand-rolled trust store) and refuses to
//     proceed on a mismatch (`ssh_connect` returns -2) rather than trusting
//     blindly. First contact with a given host is trust-on-first-use, same
//     as OpenSSH's own default behavior.
//   - This module is intentionally NOT available for ling-kernel: that
//     no_std bare-metal target has no TCP/IP stack yet (its e1000 driver
//     does raw frames only), no CSPRNG (only rdtsc, explicitly unsuitable
//     for key material), and only verify-only ed25519 — shipping "SSH"
//     there without those would be exploitable-by-construction, not a real
//     secure channel.

use std::collections::{HashMap, VecDeque};
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicU8, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use russh::keys::{Algorithm, PrivateKey, PublicKeyOrCertificate};
use russh::server::{Auth, Msg, Server as ServerTrait, Session};
use russh::{client, server, Channel, ChannelId, ChannelMsg, Disconnect, MethodKind, MethodSet};

const MAX_QUEUE: usize = 4096;
const MAX_EVENTS: usize = 256;

fn rt() -> &'static tokio::runtime::Runtime {
    static RT: OnceLock<tokio::runtime::Runtime> = OnceLock::new();
    RT.get_or_init(|| tokio::runtime::Runtime::new().expect("tokio runtime for ssh_*"))
}

fn push_bounded<T>(q: &mut VecDeque<T>, cap: usize, item: T) {
    if q.len() >= cap {
        q.pop_front();
    }
    q.push_back(item);
}

fn ct_eq(a: &str, b: &str) -> bool {
    let (ab, bb) = (a.as_bytes(), b.as_bytes());
    if ab.len() != bb.len() {
        return false;
    }
    let mut diff = 0u8;
    for i in 0..ab.len() {
        diff |= ab[i] ^ bb[i];
    }
    diff == 0
}

fn known_hosts_file() -> PathBuf {
    PathBuf::from(".ling-ssh/known_hosts")
}

// ── server: accept connections FROM other computers ────────────────────────

#[derive(Clone)]
struct PendingConfig {
    port: u16,
    username: String,
    password: String,
    host_key_path: PathBuf,
}

static PENDING: Mutex<Option<PendingConfig>> = Mutex::new(None);

pub fn configure(port: u16, username: &str, password: &str, host_key_path: &str) {
    let path = if host_key_path.is_empty() {
        PathBuf::from(".ling-ssh/host_ed25519")
    } else {
        PathBuf::from(host_key_path)
    };
    if let Ok(mut g) = PENDING.lock() {
        *g = Some(PendingConfig {
            port,
            username: username.to_string(),
            password: password.to_string(),
            host_key_path: path,
        });
    }
}

fn load_or_create_host_key(path: &std::path::Path) -> std::io::Result<PrivateKey> {
    if let Ok(key) = PrivateKey::read_openssh_file(path) {
        return Ok(key);
    }
    let key = PrivateKey::random(&mut rand10::rng(), Algorithm::Ed25519)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    key.write_openssh_file(path, russh::keys::ssh_key::LineEnding::LF)
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    Ok(key)
}

struct ServeShared {
    username: String,
    password: String,
    status: AtomicU8,
    inbound: Mutex<VecDeque<(u64, String)>>,
    events: Mutex<VecDeque<String>>,
    clients: Mutex<HashMap<u64, (server::Handle, ChannelId)>>,
    next_id: AtomicU64,
}

fn push_event(shared: &ServeShared, line: String) {
    if let Ok(mut g) = shared.events.lock() {
        push_bounded(&mut g, MAX_EVENTS, line);
    }
}

static SERVE: Mutex<Option<Arc<ServeShared>>> = Mutex::new(None);
static SERVE_STOP: Mutex<Option<server::RunningServerHandle>> = Mutex::new(None);

fn serve_shared() -> Option<Arc<ServeShared>> {
    SERVE.lock().ok().and_then(|g| g.clone())
}

#[derive(Clone)]
struct ServeFactory {
    shared: Arc<ServeShared>,
}

impl server::Server for ServeFactory {
    type Handler = SshServerHandler;
    fn new_client(&mut self, _peer: Option<std::net::SocketAddr>) -> Self::Handler {
        let id = self.shared.next_id.fetch_add(1, Ordering::SeqCst);
        SshServerHandler { shared: self.shared.clone(), id }
    }
}

struct SshServerHandler {
    shared: Arc<ServeShared>,
    id: u64,
}

impl server::Handler for SshServerHandler {
    type Error = russh::Error;

    async fn auth_password(&mut self, user: &str, password: &str) -> Result<Auth, Self::Error> {
        if ct_eq(user, &self.shared.username) && ct_eq(password, &self.shared.password) {
            Ok(Auth::Accept)
        } else {
            push_event(&self.shared, format!("authfail:{user}"));
            Ok(Auth::reject())
        }
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<Msg>,
        reply: server::ChannelOpenHandle,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        if let Ok(mut m) = self.shared.clients.lock() {
            m.insert(self.id, (session.handle(), channel.id()));
        }
        self.shared.status.store(2, Ordering::SeqCst);
        push_event(&self.shared, format!("connect:{}", self.id));
        reply.accept().await;
        Ok(())
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut Session,
    ) -> Result<(), Self::Error> {
        let cmd = String::from_utf8_lossy(data).into_owned();
        if let Ok(mut q) = self.shared.inbound.lock() {
            push_bounded(&mut q, MAX_QUEUE, (self.id, cmd));
        }
        session.channel_success(channel)?;
        Ok(())
    }

    async fn data(
        &mut self,
        _channel: ChannelId,
        data: &[u8],
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let line = String::from_utf8_lossy(data)
            .trim_end_matches(['\r', '\n'])
            .to_string();
        if !line.is_empty() {
            if let Ok(mut q) = self.shared.inbound.lock() {
                push_bounded(&mut q, MAX_QUEUE, (self.id, line));
            }
        }
        Ok(())
    }
}

impl Drop for SshServerHandler {
    fn drop(&mut self) {
        if let Ok(mut m) = self.shared.clients.lock() {
            m.remove(&self.id);
            if m.is_empty() {
                self.shared.status.store(1, Ordering::SeqCst);
            }
        }
        push_event(&self.shared, format!("disconnect:{}", self.id));
    }
}

/// Start listening with the config staged by `configure()`. Returns false if
/// unconfigured, if the password is blank, or if the host key/socket bind
/// fails — never starts a passwordless or keyless listener.
pub fn serve() -> bool {
    let Some(cfg) = PENDING.lock().ok().and_then(|g| g.clone()) else { return false };
    if cfg.password.is_empty() {
        return false;
    }
    let Ok(host_key) = load_or_create_host_key(&cfg.host_key_path) else { return false };

    let shared = Arc::new(ServeShared {
        username: cfg.username,
        password: cfg.password,
        status: AtomicU8::new(1),
        inbound: Mutex::new(VecDeque::new()),
        events: Mutex::new(VecDeque::new()),
        clients: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
    });
    if let Ok(mut g) = SERVE.lock() {
        *g = Some(shared.clone());
    }

    let mut methods = MethodSet::empty();
    methods.push(MethodKind::Password);
    let config = Arc::new(server::Config {
        keys: vec![host_key],
        methods,
        inactivity_timeout: Some(std::time::Duration::from_secs(3600)),
        nodelay: true,
        ..Default::default()
    });

    let port = cfg.port;
    let shared_for_task = shared.clone();
    rt().spawn(async move {
        let socket = match tokio::net::TcpListener::bind(("0.0.0.0", port)).await {
            Ok(s) => s,
            Err(_) => {
                shared_for_task.status.store(0, Ordering::SeqCst);
                return;
            },
        };
        let mut factory = ServeFactory { shared: shared_for_task.clone() };
        let running = factory.run_on_socket(config, &socket);
        let stop_handle = running.handle();
        if let Ok(mut g) = SERVE_STOP.lock() {
            *g = Some(stop_handle);
        }
        let _ = running.await;
        shared_for_task.status.store(0, Ordering::SeqCst);
    });

    true
}

pub fn stop() {
    if let Ok(mut g) = SERVE_STOP.lock() {
        if let Some(h) = g.take() {
            h.shutdown("stopped".to_string());
        }
    }
    if let Ok(mut g) = SERVE.lock() {
        if let Some(shared) = g.take() {
            shared.status.store(0, Ordering::SeqCst);
        }
    }
}

pub fn serve_send(id: u64, s: &str) {
    let Some(shared) = serve_shared() else { return };
    let target = shared.clients.lock().ok().and_then(|m| m.get(&id).cloned());
    if let Some((handle, channel)) = target {
        let data = format!("{s}\n").into_bytes();
        rt().block_on(async move {
            let _ = handle.data(channel, data).await;
        });
    }
}

pub fn serve_recv() -> String {
    let Some(shared) = serve_shared() else { return String::new() };
    if let Ok(mut q) = shared.inbound.lock() {
        if let Some((id, msg)) = q.pop_front() {
            return format!("{id}|{msg}");
        }
    }
    String::new()
}

pub fn serve_exit(id: u64, code: u32) {
    let Some(shared) = serve_shared() else { return };
    let target = shared.clients.lock().ok().and_then(|m| m.get(&id).cloned());
    if let Some((handle, channel)) = target {
        rt().block_on(async move {
            let _ = handle.exit_status_request(channel, code).await;
            let _ = handle.eof(channel).await;
            let _ = handle.close(channel).await;
        });
    }
}

pub fn serve_clients() -> String {
    let Some(shared) = serve_shared() else { return String::new() };
    let Ok(m) = shared.clients.lock() else { return String::new() };
    let mut ids: Vec<u64> = m.keys().copied().collect();
    ids.sort_unstable();
    ids.iter().map(|i| i.to_string()).collect::<Vec<_>>().join(",")
}

pub fn serve_client_count() -> usize {
    serve_shared()
        .and_then(|shared| shared.clients.lock().ok().map(|m| m.len()))
        .unwrap_or(0)
}

pub fn serve_events() -> String {
    let Some(shared) = serve_shared() else { return String::new() };
    if let Ok(mut q) = shared.events.lock() {
        let out: Vec<String> = q.drain(..).collect();
        return out.join("\n");
    }
    String::new()
}

pub fn serve_status() -> u8 {
    serve_shared().map(|s| s.status.load(Ordering::SeqCst)).unwrap_or(0)
}

// ── client: connect OUT to another computer ─────────────────────────────────

struct ClientHandler {
    host: String,
    port: u16,
}

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        // Certificates aren't supported by this MVP's host-key trust store —
        // fail closed rather than accept an unverifiable identity.
        let PublicKeyOrCertificate::PublicKey { key: pubkey, .. } = key else {
            return Ok(false);
        };
        let path = known_hosts_file();
        match russh::keys::known_hosts::check_known_hosts_path(&self.host, self.port, pubkey, &path)
        {
            Ok(true) => Ok(true),
            Ok(false) => {
                // Not seen before at this algorithm — trust-on-first-use,
                // same default OpenSSH itself uses for a fresh host.
                let _ = russh::keys::known_hosts::learn_known_hosts_path(
                    &self.host, self.port, pubkey, &path,
                );
                Ok(true)
            },
            // Includes the case where a *different* key was previously
            // recorded for this host/algorithm — refuse rather than warn.
            Err(_) => Ok(false),
        }
    }
}

struct ClientSession {
    handle: client::Handle<ClientHandler>,
    last_exit: i64,
}

static CLIENT: Mutex<Option<ClientSession>> = Mutex::new(None);

/// Connect and authenticate. 1 = ok, 0 = wrong credentials, -1 = connect
/// error, -2 = the remote host key changed since we last saw it (refused).
pub fn connect(host: &str, port: u16, user: &str, password: &str) -> f64 {
    let handler = ClientHandler { host: host.to_string(), port };
    let config = Arc::new(client::Config::default());

    let outcome = rt().block_on(async {
        let mut session = client::connect(config, (host, port), handler).await?;
        let auth = session.authenticate_password(user, password).await?;
        Ok::<_, russh::Error>((session, auth))
    });

    match outcome {
        Ok((session, auth)) if auth.success() => {
            if let Ok(mut g) = CLIENT.lock() {
                *g = Some(ClientSession { handle: session, last_exit: -1 });
            }
            1.0
        },
        Ok(_) => 0.0,
        Err(russh::Error::UnknownKey) => -2.0,
        Err(_) => -1.0,
    }
}

/// Run one command on the already-connected host, blocking until it exits.
/// Returns its stdout ("" if not connected or the command never produced
/// output); use `exit_code()` for its exit status.
pub fn exec(cmd: &str) -> String {
    let Some(mut sess) = CLIENT.lock().ok().and_then(|mut g| g.take()) else {
        return String::new();
    };
    let (out, code) = rt().block_on(async {
        let mut out: Vec<u8> = Vec::new();
        let mut code: i64 = -1;
        if let Ok(mut channel) = sess.handle.channel_open_session().await {
            if channel.exec(true, cmd).await.is_ok() {
                while let Some(msg) = channel.wait().await {
                    match msg {
                        ChannelMsg::Data { data } => out.extend_from_slice(&data),
                        ChannelMsg::ExitStatus { exit_status } => code = exit_status as i64,
                        _ => {},
                    }
                }
            }
        }
        (out, code)
    });
    sess.last_exit = code;
    if let Ok(mut g) = CLIENT.lock() {
        *g = Some(sess);
    }
    String::from_utf8_lossy(&out).into_owned()
}

pub fn exit_code() -> f64 {
    CLIENT.lock().ok().and_then(|g| g.as_ref().map(|s| s.last_exit as f64)).unwrap_or(-1.0)
}

pub fn client_status() -> u8 {
    if CLIENT.lock().ok().map(|g| g.is_some()).unwrap_or(false) { 2 } else { 0 }
}

pub fn close() {
    if let Ok(mut g) = CLIENT.lock() {
        if let Some(sess) = g.take() {
            rt().block_on(async move {
                let _ = sess.handle.disconnect(Disconnect::ByApplication, "", "en").await;
            });
        }
    }
}

// `SERVE`/`CLIENT`/`PENDING` are one global slot per process, so these tests
// share them and must run serially — take `TEST_LOCK` first in every test.
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

    // Defensive reset in case a prior test in this file panicked mid-way and
    // left global state dangling.
    fn cleanup() {
        stop();
        close();
        if let Ok(mut g) = PENDING.lock() {
            *g = None;
        }
    }

    #[test]
    fn wrong_password_is_rejected_not_silently_accepted() {
        let _g = TEST_LOCK.lock().unwrap();
        cleanup();
        let port = 19202;
        configure(port, "tester", "s3cret", ".ling-ssh-test/host_19202");
        assert!(serve(), "server must start with a configured, non-blank password");
        assert!(deadline_poll(|| serve_status() >= 1));

        assert_eq!(
            connect("127.0.0.1", port, "tester", "WRONG"),
            0.0,
            "a wrong password must be rejected, never accepted"
        );
        assert_eq!(
            client_status(),
            0,
            "a rejected auth attempt must not leave the client marked connected"
        );

        stop();
        let _ = std::fs::remove_dir_all(".ling-ssh-test");
    }

    #[test]
    fn serve_refuses_to_start_with_a_blank_password() {
        let _g = TEST_LOCK.lock().unwrap();
        cleanup();
        configure(19203, "tester", "", ".ling-ssh-test/host_19203");
        assert!(
            !serve(),
            "a blank configured password must never start a listening server"
        );
        assert_eq!(serve_status(), 0);
    }

    #[test]
    fn exec_round_trip_delivers_command_reply_and_exit_code() {
        let _g = TEST_LOCK.lock().unwrap();
        cleanup();
        let port = 19204;
        configure(port, "tester", "s3cret", ".ling-ssh-test/host_19204");
        assert!(serve());
        assert!(deadline_poll(|| serve_status() >= 1));

        assert_eq!(
            connect("127.0.0.1", port, "tester", "s3cret"),
            1.0,
            "correct credentials must be accepted"
        );
        // Authenticating alone doesn't open a channel yet (that happens
        // inside exec(), spawned below) — so client_count is still 0 here.

        // Server-side responder: wait for the exec command to arrive, echo it
        // back over the channel, then end the exec with exit code 7.
        let responder = std::thread::spawn(|| {
            let start = Instant::now();
            loop {
                let line = serve_recv();
                if let Some((id, cmd)) = line.split_once('|') {
                    let id: u64 = id.parse().unwrap();
                    serve_send(id, &format!("echo:{cmd}"));
                    serve_exit(id, 7);
                    return;
                }
                assert!(start.elapsed() < Duration::from_secs(5), "command never arrived");
                std::thread::sleep(Duration::from_millis(20));
            }
        });

        let (tx, rx) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let out = exec("do-a-thing");
            let _ = tx.send(out);
        });
        let out = rx
            .recv_timeout(Duration::from_secs(5))
            .expect("ssh_exec must not hang waiting for the server to finish the channel");
        responder.join().unwrap();

        assert!(out.contains("echo:do-a-thing"), "got {out:?}");
        assert_eq!(exit_code(), 7.0);

        close();
        stop();
        let _ = std::fs::remove_dir_all(".ling-ssh-test");
    }

    #[test]
    fn known_hosts_rejects_a_changed_host_key() {
        let _g = TEST_LOCK.lock().unwrap();
        cleanup();
        let port = 19205;
        // check_server_key() always consults the production `.ling-ssh/known_hosts`
        // path (it's not configurable per-connection), so this test must clean
        // that specific file, not just its own `.ling-ssh-test/` scratch dir.
        let _ = std::fs::remove_file(known_hosts_file());
        let _ = std::fs::remove_dir_all(".ling-ssh-test");

        // First server instance: client connects, learns (TOFU) its host key.
        configure(port, "tester", "s3cret", ".ling-ssh-test/host_a");
        assert!(serve());
        assert!(deadline_poll(|| serve_status() >= 1));
        let handler = ClientHandler { host: "127.0.0.1".to_string(), port };
        let outcome: Result<bool, russh::Error> = rt().block_on(async {
            let config = Arc::new(client::Config::default());
            let mut session = client::connect(config, ("127.0.0.1", port), handler).await?;
            Ok(session.authenticate_password("tester", "s3cret").await?.success())
        });
        assert_eq!(outcome.ok(), Some(true));
        stop();
        assert!(deadline_poll(|| serve_status() == 0));

        // Second server instance, same port, DIFFERENT host key: a client
        // trusting the first key must refuse to proceed.
        configure(port, "tester", "s3cret", ".ling-ssh-test/host_b");
        assert!(serve());
        assert!(deadline_poll(|| serve_status() >= 1));
        let handler2 = ClientHandler { host: "127.0.0.1".to_string(), port };
        let outcome2: Result<bool, russh::Error> = rt().block_on(async {
            let config = Arc::new(client::Config::default());
            let mut session = client::connect(config, ("127.0.0.1", port), handler2).await?;
            Ok(session.authenticate_password("tester", "s3cret").await?.success())
        });
        assert!(
            matches!(outcome2, Err(russh::Error::UnknownKey)),
            "a changed host key must be refused, not silently trusted: {outcome2:?}"
        );

        stop();
        let _ = std::fs::remove_file(known_hosts_file());
        let _ = std::fs::remove_dir_all(".ling-ssh-test");
    }
}
