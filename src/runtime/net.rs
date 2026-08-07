// src/runtime/net.rs — minimal line-oriented TCP transport for 2-peer co-op.
//
// One global connection per process. Socket I/O runs on background threads so
// the game loop never blocks: `send` enqueues a line, `recv` returns the most
// recent line received. Intended for LAN-latency state sync (positions etc.).
//
//   net_host(port)        → listen, accept one peer
//   net_join(ip, port)    → connect to a host
//   net_send(str)         → queue a line to the peer
//   net_recv()            → most recent line from the peer ("" if none)
//   net_status()          → 0 = idle/closed, 1 = waiting, 2 = connected

use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicU32, AtomicU8, Ordering};
use std::sync::mpsc::{channel, Receiver, Sender};
use std::sync::{Arc, Mutex};

struct NetInner {
    status: Arc<AtomicU8>,
    latest: Arc<Mutex<String>>,
    out_tx: Sender<String>,
}

static NET: Mutex<Option<NetInner>> = Mutex::new(None);

// Drive an established stream: spawn a reader thread (newest line → `latest`),
// then drain the outgoing channel on this thread (writer).
fn run_stream(
    stream: TcpStream,
    status: Arc<AtomicU8>,
    latest: Arc<Mutex<String>>,
    rx: Receiver<String>,
) {
    let _ = stream.set_nodelay(true);
    status.store(2, Ordering::SeqCst);

    if let Ok(read_stream) = stream.try_clone() {
        let latest = latest.clone();
        let rstat = status.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(read_stream);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) => {
                        rstat.store(0, Ordering::SeqCst);
                        break;
                    },
                    Ok(_) => {
                        if let Ok(mut g) = latest.lock() {
                            *g = line.trim_end_matches(['\r', '\n']).to_string();
                        }
                    },
                    Err(_) => {
                        rstat.store(0, Ordering::SeqCst);
                        break;
                    },
                }
            }
        });
    }

    let mut w = stream;
    for msg in rx {
        if w.write_all(msg.as_bytes()).is_err() || w.write_all(b"\n").is_err() {
            break;
        }
        let _ = w.flush();
    }
    status.store(0, Ordering::SeqCst);
}

fn install() -> (Arc<AtomicU8>, Arc<Mutex<String>>, Receiver<String>) {
    let status = Arc::new(AtomicU8::new(1));
    let latest = Arc::new(Mutex::new(String::new()));
    let (tx, rx) = channel::<String>();
    if let Ok(mut g) = NET.lock() {
        *g = Some(NetInner { status: status.clone(), latest: latest.clone(), out_tx: tx });
    }
    (status, latest, rx)
}

// ── multi-client host (net_host / net_recv_from / net_send_to) ─────────────
// Up to 64 concurrent connections, each with its own id (assigned on accept)
// and its own outbound queue. Inbound lines from every connection land in one
// shared FIFO `inbox`, drained via recv_from() as "<id>|<line>".
struct HostState {
    next_id: AtomicU32,
    clients: Mutex<HashMap<u32, Sender<String>>>,
    inbox: Mutex<VecDeque<(u32, String)>>,
}

static HOST: Mutex<Option<Arc<HostState>>> = Mutex::new(None);

fn run_client_conn(id: u32, stream: TcpStream, host: Arc<HostState>, rx: Receiver<String>) {
    let _ = stream.set_nodelay(true);

    if let Ok(read_stream) = stream.try_clone() {
        let host = host.clone();
        std::thread::spawn(move || {
            let mut reader = BufReader::new(read_stream);
            loop {
                let mut line = String::new();
                match reader.read_line(&mut line) {
                    Ok(0) | Err(_) => break,
                    Ok(_) => {
                        let line = line.trim_end_matches(['\r', '\n']).to_string();
                        if let Ok(mut q) = host.inbox.lock() {
                            q.push_back((id, line));
                        }
                    },
                }
            }
            if let Ok(mut m) = host.clients.lock() {
                m.remove(&id);
            }
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

pub fn host(port: u16) {
    let state = Arc::new(HostState {
        next_id: AtomicU32::new(1),
        clients: Mutex::new(HashMap::new()),
        inbox: Mutex::new(VecDeque::new()),
    });
    if let Ok(mut g) = HOST.lock() {
        *g = Some(state.clone());
    }
    std::thread::spawn(move || {
        let listener = match TcpListener::bind(("0.0.0.0", port)) {
            Ok(l) => l,
            Err(_) => return,
        };
        for incoming in listener.incoming() {
            let stream = match incoming {
                Ok(s) => s,
                Err(_) => continue,
            };
            if let Ok(m) = state.clients.lock() {
                if m.len() >= 64 {
                    continue;
                }
            }
            let id = state.next_id.fetch_add(1, Ordering::SeqCst);
            let (tx, rx) = channel::<String>();
            if let Ok(mut m) = state.clients.lock() {
                m.insert(id, tx);
            }
            let state2 = state.clone();
            std::thread::spawn(move || run_client_conn(id, stream, state2, rx));
        }
    });
}

/// Pop the next queued inbound line from any connected client, formatted
/// "<id>|<line>" ("" if nothing queued).
pub fn recv_from() -> String {
    if let Ok(g) = HOST.lock() {
        if let Some(h) = g.as_ref() {
            if let Ok(mut q) = h.inbox.lock() {
                if let Some((id, line)) = q.pop_front() {
                    return format!("{id}|{line}");
                }
            }
        }
    }
    String::new()
}

/// Send a line to one specific client id (no-op if that client already
/// disconnected).
pub fn send_to(id: u32, s: &str) {
    if let Ok(g) = HOST.lock() {
        if let Some(h) = g.as_ref() {
            if let Ok(m) = h.clients.lock() {
                if let Some(tx) = m.get(&id) {
                    let _ = tx.send(s.to_string());
                }
            }
        }
    }
}

/// Tear down whichever networking role is active: a multi-client host, or a
/// joined (client) connection. Actually drops the socket rather than just
/// forgetting it, so a fresh host/join afterward starts clean.
pub fn close() {
    if let Ok(mut g) = HOST.lock() {
        *g = None;
    }
    if let Ok(mut g) = NET.lock() {
        if let Some(n) = g.take() {
            n.status.store(0, Ordering::SeqCst);
            drop(n.out_tx);
        }
    }
}

pub fn join(ip: &str, port: u16) {
    let (status, latest, rx) = install();
    let addr = format!("{ip}:{port}");
    std::thread::spawn(move || match TcpStream::connect(&addr) {
        Ok(stream) => run_stream(stream, status, latest, rx),
        Err(_) => status.store(0, Ordering::SeqCst),
    });
}

pub fn send(s: &str) {
    if let Ok(g) = NET.lock() {
        if let Some(n) = g.as_ref() {
            let _ = n.out_tx.send(s.to_string());
        }
    }
}

pub fn recv() -> String {
    if let Ok(g) = NET.lock() {
        if let Some(n) = g.as_ref() {
            if let Ok(l) = n.latest.lock() {
                return l.clone();
            }
        }
    }
    String::new()
}

pub fn status() -> u8 {
    if let Ok(g) = NET.lock() {
        if let Some(n) = g.as_ref() {
            return n.status.load(Ordering::SeqCst);
        }
    }
    0
}

// ── LAN lobby discovery (UDP broadcast) ─────────────────────────────────────
// A host periodically broadcasts an "info" line; joiners listen and build a
// live server list. Info is opaque to the transport (the game encodes
// name|stage|private|... into it).

use std::collections::{HashMap, VecDeque};
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
