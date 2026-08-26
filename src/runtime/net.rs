// src/runtime/net.rs — line-oriented TCP transport, N-client star topology.
//
// One process is either a host (accepts many clients) or a client (connects
// to exactly one host). Socket I/O runs on background threads so the game
// loop never blocks: `send`/`send_to` enqueue lines, `recv`/`recv_from`
// dequeue them in arrival order. Intended for LAN-latency state sync.
//
//   net_host(port)          → listen, accept up to MAX_CLIENTS peers
//   net_join(ip, port)      → connect to a host
//   net_send(str)           → host: broadcast to all clients. client: send to host.
//   net_send_to(id, str)    → host only: unicast to one client
//   net_recv()              → oldest queued line, message only ("" if none)
//   net_recv_from()         → oldest queued line as "senderId|line" ("" if none)
//   net_clients()           → host: connected client ids, comma-separated
//   net_client_count()      → number of connected peers
//   net_events()            → drains "connect:id"/"disconnect:id" log, newline-joined
//   net_status()            → 0 = idle/closed, 1 = waiting, 2 = >=1 peer connected
//   net_close()             → tear down the current host/client role and reset to idle
//
// `net_recv` and `net_recv_from` drain the same inbound queue — pick one
// style per program. Mixing them splits messages between the two arbitrarily.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

const MAX_CLIENTS: usize = 64;
const MAX_QUEUE: usize = 4096;
const MAX_EVENTS: usize = 256;

type Inbound = Arc<Mutex<VecDeque<(u64, String)>>>;
type Events = Arc<Mutex<VecDeque<String>>>;

struct ClientHandle {
    out_tx: Sender<String>,
    // clone of the peer socket, kept only so `close()` can force it shut;
    // ordinary I/O goes through `out_tx` / the reader thread, never this.
    stream: Option<TcpStream>,
}

enum Role {
    Host {
        next_id: AtomicU64,
        clients: Mutex<HashMap<u64, ClientHandle>>,
        accept_stop: Arc<AtomicBool>,
    },
    Client {
        out_tx: Sender<String>,
        stream: Arc<Mutex<Option<TcpStream>>>,
    },
}

struct Hub {
    status: Arc<AtomicU8>,
    inbound: Inbound,
    events: Events,
    role: Role,
}

static NET: Mutex<Option<Hub>> = Mutex::new(None);

fn push_bounded<T>(q: &mut VecDeque<T>, cap: usize, item: T) {
    if q.len() >= cap {
        q.pop_front();
    }
    q.push_back(item);
}

fn push_event(events: &Events, line: String) {
    if let Ok(mut g) = events.lock() {
        push_bounded(&mut g, MAX_EVENTS, line);
    }
}

// Drive one established connection: a reader thread pushes every line it
// receives into `inbound` tagged with `id`, while this thread drains `rx`
// and writes each queued outgoing line to the socket.
//
// `on_disconnect` runs from the reader thread the moment it sees EOF/error —
// it is the only reliable disconnect signal, since the writer thread may be
// parked on an empty `rx` indefinitely otherwise. It is expected to drop the
// registry's `Sender<String>` for this peer, which closes `rx` and ends the
// writer loop below.
fn run_peer(
    stream: TcpStream,
    id: u64,
    inbound: Inbound,
    rx: Receiver<String>,
    on_disconnect: impl FnOnce() + Send + 'static,
) {
    let _ = stream.set_nodelay(true);

    if let Ok(read_stream) = stream.try_clone() {
        let inbound = inbound.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(read_stream);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if let Ok(mut g) = inbound.lock() {
                            push_bounded(
                                &mut g,
                                MAX_QUEUE,
                                (id, line.trim_end_matches(['\r', '\n']).to_string()),
                            );
                        }
                    },
                    Err(_) => break,
                }
            }
            on_disconnect();
        });
    }

    let mut w = stream;
    for msg in rx {
        if w.write_all(msg.as_bytes()).is_err() || w.write_all(b"\n").is_err() {
            break;
        }
        let _ = w.flush();
    }
}

fn install(role: Role) -> (Arc<AtomicU8>, Inbound, Events) {
    let status = Arc::new(AtomicU8::new(1));
    let inbound = Arc::new(Mutex::new(VecDeque::new()));
    let events = Arc::new(Mutex::new(VecDeque::new()));
    if let Ok(mut g) = NET.lock() {
        *g = Some(Hub {
            status: status.clone(),
            inbound: inbound.clone(),
            events: events.clone(),
            role,
        });
    }
    (status, inbound, events)
}

// ── multi-client host (net_host / net_recv_from / net_send_to) ─────────────
pub fn host(port: u16) {
    let accept_stop = Arc::new(AtomicBool::new(false));
    let (status, inbound, events) = install(Role::Host {
        next_id: AtomicU64::new(1),
        clients: Mutex::new(HashMap::new()),
        accept_stop: accept_stop.clone(),
    });
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(_) => {
                status.store(0, Ordering::SeqCst);
                return;
            },
        };
        let _ = listener.set_nonblocking(true);
        loop {
            if accept_stop.load(Ordering::SeqCst) {
                break;
            }
            let stream = match listener.accept() {
                Ok((s, _)) => s,
                Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(20));
                    continue;
                },
                Err(_) => continue,
            };
            // On Windows a stream accepted from a non-blocking listener inherits
            // the non-blocking flag, breaking run_peer's blocking reads below.
            let _ = stream.set_nonblocking(false);
            let g = NET.lock().ok();
            let Some(guard) = g else { break };
            let Some(hub) = guard.as_ref() else { break };
            let Role::Host { next_id, clients, .. } = &hub.role else { break };
            let count = clients.lock().map(|m| m.len()).unwrap_or(0);
            if count >= MAX_CLIENTS {
                drop(guard);
                continue;
            }
            let id = next_id.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = channel::<String>();
            let shutdown_handle = stream.try_clone().ok();
            if let Ok(mut m) = clients.lock() {
                m.insert(id, ClientHandle { out_tx: tx, stream: shutdown_handle });
            }
            drop(guard);

            status.store(2, Ordering::SeqCst);
            push_event(&events, format!("connect:{id}"));

            let inbound2 = inbound.clone();
            let events2 = events.clone();
            std::thread::spawn(move || {
                let events3 = events2.clone();
                run_peer(stream, id, inbound2, rx, move || {
                    if let Ok(g) = NET.lock() {
                        if let Some(hub) = g.as_ref() {
                            if let Role::Host { clients, .. } = &hub.role {
                                if let Ok(mut m) = clients.lock() {
                                    m.remove(&id);
                                    if m.is_empty() {
                                        hub.status.store(1, Ordering::SeqCst);
                                    }
                                }
                            }
                        }
                    }
                    push_event(&events3, format!("disconnect:{id}"));
                });
            });
        }
    });
}

pub fn join(ip: &str, port: u16) {
    let (tx, rx) = channel::<String>();
    let shutdown_slot: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
    let (status, inbound, _events) =
        install(Role::Client { out_tx: tx, stream: shutdown_slot.clone() });
    let ip = ip.to_string();
    std::thread::spawn(move || {
        let stream = match TcpStream::connect((ip.as_str(), port)) {
            Ok(s) => s,
            Err(_) => {
                status.store(0, Ordering::SeqCst);
                return;
            },
        };
        if let Ok(mut g) = shutdown_slot.lock() {
            *g = stream.try_clone().ok();
        }
        status.store(2, Ordering::SeqCst);
        run_peer(stream, 0, inbound, rx, move || {
            if let Ok(g) = NET.lock() {
                if let Some(hub) = g.as_ref() {
                    hub.status.store(0, Ordering::SeqCst);
                }
            }
        });
    });
}
// Up to 64 concurrent connections, each with its own id (assigned on accept)
// and its own outbound queue. Inbound lines from every connection land in one
// shared FIFO `inbox`, drained via recv_from() as "<id>|<line>".

/// Tear down the current role (host or client) and reset to idle. Sockets are
/// shut down so the reader/writer threads they own unblock and exit; the
/// registry itself is dropped by returning the `Hub` out of `NET` and letting
/// it fall out of scope. A later `host`/`join` reinstalls a fresh one.
pub fn close() {
    let hub = match NET.lock() {
        Ok(mut g) => g.take(),
        Err(_) => None,
    };
    let Some(hub) = hub else { return };
    hub.status.store(0, Ordering::SeqCst);
    match &hub.role {
        Role::Host { accept_stop, clients, .. } => {
            accept_stop.store(true, Ordering::SeqCst);
            if let Ok(m) = clients.lock() {
                for c in m.values() {
                    if let Some(s) = &c.stream {
                        let _ = s.shutdown(Shutdown::Both);
                    }
                }
            }
        },
        Role::Client { stream, .. } => {
            if let Ok(s) = stream.lock() {
                if let Some(s) = s.as_ref() {
                    let _ = s.shutdown(Shutdown::Both);
                }
            }
        },
    }
}

pub fn send(s: &str) {
    if let Ok(g) = NET.lock() {
        if let Some(hub) = g.as_ref() {
            match &hub.role {
                Role::Host { clients, .. } => {
                    if let Ok(m) = clients.lock() {
                        for c in m.values() {
                            let _ = c.out_tx.send(s.to_string());
                        }
                    }
                },
                Role::Client { out_tx, .. } => {
                    let _ = out_tx.send(s.to_string());
                },
            }
        }
    }
}

pub fn send_to(id: u64, s: &str) {
    if let Ok(g) = NET.lock() {
        if let Some(hub) = g.as_ref() {
            if let Role::Host { clients, .. } = &hub.role {
                if let Ok(m) = clients.lock() {
                    if let Some(c) = m.get(&id) {
                        let _ = c.out_tx.send(s.to_string());
                    }
                }
            }
        }
    }
}

pub fn recv() -> String {
    if let Ok(g) = NET.lock() {
        if let Some(hub) = g.as_ref() {
            if let Ok(mut q) = hub.inbound.lock() {
                if let Some((_, msg)) = q.pop_front() {
                    return msg;
                }
            }
        }
    }
    String::new()
}

pub fn recv_from() -> String {
    if let Ok(g) = NET.lock() {
        if let Some(hub) = g.as_ref() {
            if let Ok(mut q) = hub.inbound.lock() {
                if let Some((id, msg)) = q.pop_front() {
                    return format!("{id}|{msg}");
                }
            }
        }
    }
    String::new()
}

pub fn clients() -> String {
    if let Ok(g) = NET.lock() {
        if let Some(hub) = g.as_ref() {
            if let Role::Host { clients, .. } = &hub.role {
                if let Ok(m) = clients.lock() {
                    let mut ids: Vec<u64> = m.keys().copied().collect();
                    ids.sort_unstable();
                    return ids
                        .iter()
                        .map(|i| i.to_string())
                        .collect::<Vec<_>>()
                        .join(",");
                }
            }
        }
    }
    String::new()
}

pub fn client_count() -> usize {
    if let Ok(g) = NET.lock() {
        if let Some(hub) = g.as_ref() {
            return match &hub.role {
                Role::Host { clients, .. } => clients.lock().map(|m| m.len()).unwrap_or(0),
                Role::Client { .. } => usize::from(hub.status.load(Ordering::SeqCst) == 2),
            };
        }
    }
    0
}

pub fn events() -> String {
    if let Ok(g) = NET.lock() {
        if let Some(hub) = g.as_ref() {
            if let Ok(mut q) = hub.events.lock() {
                let out: Vec<String> = q.drain(..).collect();
                return out.join("\n");
            }
        }
    }
    String::new()
}

pub fn status() -> u8 {
    if let Ok(g) = NET.lock() {
        if let Some(hub) = g.as_ref() {
            return hub.status.load(Ordering::SeqCst);
        }
    }
    0
}

// ── LAN lobby discovery (UDP broadcast) ─────────────────────────────────────
// A host periodically broadcasts an "info" line; joiners listen and build a
// live server list. Info is opaque to the transport (the game encodes
// name|stage|private|... into it).

use std::net::UdpSocket;
use std::time::{Duration, Instant};

static ANNOUNCE: Mutex<Option<String>> = Mutex::new(None); // Some(info) while announcing
static ANNOUNCE_RUN: AtomicU8 = AtomicU8::new(0);
static DISCOVER: Mutex<Option<HashMap<String, (String, Instant)>>> = Mutex::new(None);
static DISCOVER_RUN: AtomicU8 = AtomicU8::new(0);

/// Start (or update) broadcasting `info` on the LAN discovery `port` (~1 Hz).
pub fn announce(port: u16, info: &str) {
    if let Ok(mut g) = ANNOUNCE.lock() {
        *g = Some(info.to_string());
    }
    if ANNOUNCE_RUN.swap(1, Ordering::SeqCst) == 1 {
        return;
    }
    std::thread::spawn(move || {
        let sock = match UdpSocket::bind("0.0.0.0:0") {
            Ok(s) => s,
            Err(_) => {
                ANNOUNCE_RUN.store(0, Ordering::SeqCst);
                return;
            },
        };
        let _ = sock.set_broadcast(true);
        let addr = format!("255.255.255.255:{port}");
        loop {
            let info = ANNOUNCE.lock().ok().and_then(|g| g.clone());
            match info {
                Some(s) => {
                    let _ = sock.send_to(s.as_bytes(), &addr);
                },
                None => break,
            }
            std::thread::sleep(Duration::from_millis(1000));
        }
        ANNOUNCE_RUN.store(0, Ordering::SeqCst);
    });
}

/// Stop broadcasting.
pub fn announce_stop() {
    if let Ok(mut g) = ANNOUNCE.lock() {
        *g = None;
    }
}

/// Host self-test: can we actually bind the TCP host port, and what's our LAN IP?
/// (External reachability still depends on firewall + router port-forward.)
pub fn test_bind(port: u16) -> String {
    let mut out = String::new();
    match TcpListener::bind(("0.0.0.0", port)) {
        Ok(l) => {
            let _ = l;
            out.push_str(&format!("TCP bind 0.0.0.0:{port}: OK (host can listen)\n"));
        },
        Err(e) => {
            out.push_str(&format!("TCP bind 0.0.0.0:{port}: FAILED -- {e}\n"));
        },
    }
    if let Ok(s) = UdpSocket::bind("0.0.0.0:0") {
        if s.connect("8.8.8.8:80").is_ok() {
            if let Ok(a) = s.local_addr() {
                out.push_str(&format!(
                    "LAN IP (give to same-network joiners): {}\n",
                    a.ip()
                ));
            }
        }
    }
    out
}

/// Return discovered servers seen in the last 5 s as "ip|info\n…" (starts the
/// listener on first call).
pub fn discover(port: u16) -> String {
    if DISCOVER_RUN.swap(1, Ordering::SeqCst) == 0 {
        if let Ok(mut g) = DISCOVER.lock() {
            *g = Some(HashMap::new());
        }
        std::thread::spawn(move || {
            let sock = match UdpSocket::bind(("0.0.0.0", port)) {
                Ok(s) => s,
                Err(_) => {
                    DISCOVER_RUN.store(0, Ordering::SeqCst);
                    return;
                },
            };
            let _ = sock.set_read_timeout(Some(Duration::from_millis(700)));
            let mut buf = [0u8; 512];
            loop {
                if let Ok((n, src)) = sock.recv_from(&mut buf) {
                    let info = String::from_utf8_lossy(&buf[..n]).replace(['\n', '\r'], " ");
                    if let Ok(mut g) = DISCOVER.lock() {
                        if let Some(m) = g.as_mut() {
                            m.insert(src.ip().to_string(), (info, Instant::now()));
                        }
                    }
                }
            }
        });
    }
    let mut out = String::new();
    if let Ok(g) = DISCOVER.lock() {
        if let Some(m) = g.as_ref() {
            for (ip, (info, t)) in m.iter() {
                if t.elapsed() < Duration::from_secs(5) {
                    out.push_str(ip);
                    out.push('|');
                    out.push_str(info);
                    out.push('\n');
                }
            }
        }
    }
    out
}

// `NET` is one global slot per process, so these tests share it and must run
// serially — take `TEST_LOCK` first in every test.
#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Read as _;
    use std::time::Duration;

    static TEST_LOCK: Mutex<()> = Mutex::new(());

    fn deadline_poll(mut cond: impl FnMut() -> bool) -> bool {
        let start = Instant::now();
        while start.elapsed() < Duration::from_secs(3) {
            if cond() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        false
    }

    fn wait_connect(addr: &str) -> TcpStream {
        let start = Instant::now();
        loop {
            if let Ok(s) = TcpStream::connect(addr) {
                return s;
            }
            assert!(
                start.elapsed() < Duration::from_secs(3),
                "could not connect to {addr}"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn host_accepts_multiple_clients_and_broadcasts_to_all() {
        let _g = TEST_LOCK.lock().unwrap();
        let port = 18901;
        host(port);
        let addr = format!("127.0.0.1:{port}");
        let mut peers: Vec<TcpStream> = (0..3).map(|_| wait_connect(&addr)).collect();

        assert!(
            deadline_poll(|| client_count() == 3),
            "expected 3 clients connected"
        );
        let ids: Vec<u64> = clients().split(',').map(|s| s.parse().unwrap()).collect();
        assert_eq!(ids.len(), 3);

        let ev = events();
        assert_eq!(ev.matches("connect:").count(), 3);

        send("hello");
        for p in peers.iter_mut() {
            let mut reader = BufReader::new(p.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            assert_eq!(line.trim_end(), "hello");
        }
    }

    #[test]
    fn recv_from_preserves_order_and_does_not_duplicate() {
        let _g = TEST_LOCK.lock().unwrap();
        let port = 18902;
        host(port);
        let addr = format!("127.0.0.1:{port}");
        let mut peer = wait_connect(&addr);
        assert!(deadline_poll(|| client_count() == 1));

        for i in 0..5 {
            peer.write_all(format!("m{i}\n").as_bytes()).unwrap();
        }
        peer.flush().unwrap();

        let mut got = Vec::new();
        assert!(deadline_poll(|| {
            loop {
                let s = recv_from();
                if s.is_empty() {
                    break;
                }
                got.push(s);
            }
            got.len() == 5
        }));

        let expected_id = got[0].split('|').next().unwrap().to_string();
        for (i, line) in got.iter().enumerate() {
            assert_eq!(*line, format!("{expected_id}|m{i}"));
        }
        assert_eq!(recv_from(), "", "queue must be drained, not re-delivered");
    }

    #[test]
    fn disconnect_is_observed_and_client_is_dropped_from_the_registry() {
        let _g = TEST_LOCK.lock().unwrap();
        let port = 18903;
        host(port);
        let addr = format!("127.0.0.1:{port}");
        let peer = wait_connect(&addr);
        assert!(deadline_poll(|| client_count() == 1));

        drop(peer);
        assert!(
            deadline_poll(|| client_count() == 0),
            "disconnect must drop the client"
        );
        assert!(deadline_poll(|| events().contains("disconnect:")));
    }

    #[test]
    fn join_reports_connected_status_and_exchanges_lines_with_a_bare_host() {
        let _g = TEST_LOCK.lock().unwrap();
        let port = 18904;
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        let handle = std::thread::spawn(move || {
            let (mut s, _) = listener.accept().unwrap();
            s.write_all(b"from-host\n").unwrap();
            let mut reader = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            reader.read_line(&mut line).unwrap();
            line
        });

        join("127.0.0.1", port);
        assert!(
            deadline_poll(|| status() == 2),
            "client must report connected"
        );

        let mut got = String::new();
        assert!(deadline_poll(|| {
            got = recv();
            !got.is_empty()
        }));
        assert_eq!(got, "from-host");

        send("ping");
        let echoed = handle.join().unwrap();
        assert_eq!(echoed.trim_end(), "ping");
    }

    #[test]
    fn client_cap_rejects_connections_past_the_limit() {
        let _g = TEST_LOCK.lock().unwrap();
        let port = 18905;
        host(port);
        let addr = format!("127.0.0.1:{port}");
        let mut peers: Vec<TcpStream> = (0..MAX_CLIENTS + 2).map(|_| wait_connect(&addr)).collect();

        assert!(deadline_poll(|| client_count() == MAX_CLIENTS));

        // The rejected sockets get dropped by the server without ever joining
        // the registry, so a read on them observes EOF.
        let rejected = peers.last_mut().unwrap();
        let mut buf = [0u8; 8];
        assert!(deadline_poll(|| matches!(rejected.read(&mut buf), Ok(0))));
    }

    #[test]
    fn close_on_client_role_drops_the_socket_and_resets_status() {
        let _g = TEST_LOCK.lock().unwrap();
        let port = 18906;
        let listener = TcpListener::bind(("127.0.0.1", port)).unwrap();
        let handle = std::thread::spawn(move || {
            let (s, _) = listener.accept().unwrap();
            // hold the socket open; only our shutdown() should end this read
            let mut reader = BufReader::new(s);
            let mut line = String::new();
            let n = reader.read_line(&mut line).unwrap_or(0);
            n == 0
        });

        join("127.0.0.1", port);
        assert!(deadline_poll(|| status() == 2));

        close();
        assert_eq!(status(), 0);
        assert_eq!(recv(), "", "closed hub must not serve stale state");

        let host_saw_eof = handle.join().unwrap();
        assert!(
            host_saw_eof,
            "close() must shut the socket down, not just forget it"
        );
    }

    #[test]
    fn close_on_host_role_stops_accepting_and_drops_all_clients() {
        let _g = TEST_LOCK.lock().unwrap();
        let port = 18907;
        host(port);
        let addr = format!("127.0.0.1:{port}");
        let mut peers: Vec<TcpStream> = (0..3).map(|_| wait_connect(&addr)).collect();
        assert!(deadline_poll(|| client_count() == 3));

        close();
        assert_eq!(status(), 0);
        assert_eq!(client_count(), 0);

        for p in peers.iter_mut() {
            let mut buf = [0u8; 8];
            assert!(deadline_poll(|| matches!(p.read(&mut buf), Ok(0))));
        }

        // the accept loop must actually have stopped, not just forgotten its
        // registry: once the listener itself drops, new connects are refused
        assert!(
            deadline_poll(|| TcpStream::connect(&addr).is_err()),
            "listener must be gone after close(), not still accepting"
        );
    }
}
