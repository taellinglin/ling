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
use std::sync::atomic::{AtomicU8, Ordering};
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
fn run_stream(stream: TcpStream, status: Arc<AtomicU8>, latest: Arc<Mutex<String>>, rx: Receiver<String>) {
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
                    Ok(0) => { rstat.store(0, Ordering::SeqCst); break; }
                    Ok(_) => {
                        if let Ok(mut g) = latest.lock() {
                            *g = line.trim_end_matches(['\r', '\n']).to_string();
                        }
                    }
                    Err(_) => { rstat.store(0, Ordering::SeqCst); break; }
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

pub fn host(port: u16) {
    let (status, latest, rx) = install();
    std::thread::spawn(move || {
        match TcpListener::bind(("0.0.0.0", port)) {
            Ok(listener) => match listener.accept() {
                Ok((stream, _)) => run_stream(stream, status, latest, rx),
                Err(_) => status.store(0, Ordering::SeqCst),
            },
            Err(_) => status.store(0, Ordering::SeqCst),
        }
    });
}

pub fn join(ip: &str, port: u16) {
    let (status, latest, rx) = install();
    let addr = format!("{ip}:{port}");
    std::thread::spawn(move || {
        match TcpStream::connect(&addr) {
            Ok(stream) => run_stream(stream, status, latest, rx),
            Err(_) => status.store(0, Ordering::SeqCst),
        }
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
