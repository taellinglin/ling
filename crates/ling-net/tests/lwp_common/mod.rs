#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use crossbeam_channel::Receiver;
use ling_net::lwp::crypto::identity::IdentityKey;
use ling_net::lwp::{NetConfig, NetEvent, Network, PeerInfo, SeedPeer, Tuning};

pub const MAGIC: [u8; 4] = *b"LWT1";

pub fn tmpdir() -> PathBuf {
    let d = std::env::temp_dir().join(format!("lwnet-it-{}", rand::random::<u64>()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

pub struct Opts {
    pub listen: bool,
    pub manual: Vec<String>,
    pub seeds: Vec<SeedPeer>,
    pub proxy: Option<SocketAddr>,
    pub identity: Option<IdentityKey>,
    pub ephemeral: bool,
    pub tuning: Tuning,
}

impl Default for Opts {
    fn default() -> Self {
        Opts { listen: true, manual: vec![], seeds: vec![], proxy: None, identity: None, ephemeral: true, tuning: Tuning::fast_for_tests() }
    }
}

pub fn node(o: Opts) -> (Network, Receiver<NetEvent>) {
    let cfg = NetConfig {
        network_magic: MAGIC,
        network: 9,
        listen: o.listen.then(|| "127.0.0.1:0".parse().unwrap()),
        data_dir: tmpdir(),
        seeds: o.seeds,
        socks5_proxy: o.proxy,
        max_outbound: 8,
        max_inbound: 32,
        identity: o.identity.unwrap_or_else(IdentityKey::generate),
        ephemeral_outbound_identity: o.ephemeral,
        manual_peers: o.manual,
    };
    Network::start_tuned(cfg, o.tuning).unwrap()
}

pub fn addr(n: &Network) -> String {
    n.local_addr().unwrap().to_string()
}

/// Wait for the first event matching `f`, discarding others.
pub fn wait_for<T>(rx: &Receiver<NetEvent>, d: Duration, mut f: impl FnMut(&NetEvent) -> Option<T>) -> Option<T> {
    let until = Instant::now() + d;
    loop {
        let left = until.checked_duration_since(Instant::now())?;
        match rx.recv_timeout(left) {
            Ok(ev) => {
                if let Some(t) = f(&ev) {
                    return Some(t);
                }
            }
            Err(_) => return None,
        }
    }
}

pub fn connected(rx: &Receiver<NetEvent>, d: Duration) -> Option<PeerInfo> {
    wait_for(rx, d, |e| match e {
        NetEvent::Connected(p) => Some(p.clone()),
        _ => None,
    })
}

/// A transparent TCP relay that records both directions and can flip one
/// byte of the client→server stream at a given offset.
pub struct Tap {
    pub addr: SocketAddr,
    pub c2s: Arc<Mutex<Vec<u8>>>,
    pub s2c: Arc<Mutex<Vec<u8>>>,
}

fn pump(mut from: TcpStream, mut to: TcpStream, rec: Arc<Mutex<Vec<u8>>>, flip_at: Option<usize>) {
    let mut buf = vec![0u8; 64 << 10];
    let mut off = 0usize;
    loop {
        let n = match from.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        if let Some(f) = flip_at {
            if f >= off && f < off + n {
                buf[f - off] ^= 0x01;
            }
        }
        off += n;
        rec.lock().unwrap().extend_from_slice(&buf[..n]);
        if to.write_all(&buf[..n]).is_err() {
            break;
        }
    }
    let _ = to.shutdown(std::net::Shutdown::Both);
    let _ = from.shutdown(std::net::Shutdown::Both);
}

pub fn tap(target: SocketAddr, flip_at: Option<usize>) -> Tap {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    let c2s = Arc::new(Mutex::new(Vec::new()));
    let s2c = Arc::new(Mutex::new(Vec::new()));
    let (a, b) = (c2s.clone(), s2c.clone());
    thread::spawn(move || {
        // Only the first connection is tapped/tampered; later ones (retries)
        // are relayed untouched.
        let mut first = true;
        for c in l.incoming() {
            let Ok(c) = c else { break };
            let Ok(s) = TcpStream::connect(target) else { continue };
            let (rec_a, rec_b) = if first { (a.clone(), b.clone()) } else { (Arc::new(Mutex::new(vec![])), Arc::new(Mutex::new(vec![]))) };
            let flip = if first { flip_at } else { None };
            first = false;
            let (c2, s2) = (c.try_clone().unwrap(), s.try_clone().unwrap());
            thread::spawn(move || pump(c, s, rec_a, flip));
            thread::spawn(move || pump(s2, c2, rec_b, None));
        }
    });
    Tap { addr, c2s, s2c }
}

/// Split a recorded post-handshake stream into frame lengths.
pub fn frame_lengths(stream: &[u8]) -> Vec<usize> {
    let mut v = Vec::new();
    let mut p = 0;
    while p + 4 <= stream.len() {
        let l = u32::from_le_bytes(stream[p..p + 4].try_into().unwrap()) as usize;
        if p + 4 + l > stream.len() {
            break;
        }
        v.push(l);
        p += 4 + l;
    }
    v
}
