//! Runtime: listener thread, connection-manager thread, and a reader +
//! writer thread per connection. All shared state lives in [`Shared`].

use std::collections::{HashMap, HashSet};
use std::io;
use std::net::{IpAddr, Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crossbeam_channel::{bounded, select, Receiver, SendTimeoutError, Sender};
use crate::lwp::crypto::identity::{IdentityKey, IdentityPublic};
use crate::lwp::crypto::{hash, kdf};
use rand::seq::SliceRandom;
use rand::Rng;

use crate::lwp::addr::{canon_ip, ip_group, ip_is_routable, Host, NetAddr};
use crate::lwp::dandelion::{Dandelion, Decision, Embargoed};
use crate::lwp::frame::{self, CipherState, MAX_PAYLOAD};
use crate::lwp::handshake::{self, HsError, Params, Session, SelfNonces};
use crate::lwp::peerdb::{PeerDb, MAX_ANCHORS};
use crate::lwp::wire::{hex, Rd};
use crate::lwp::{NetConfig, NetEvent, PeerId, PeerInfo, APP_KIND_MIN};

// Reserved control kinds (0x00..=0x0F).
pub const K_PADDING: u8 = 0x00;
pub const K_PING: u8 = 0x01;
pub const K_PONG: u8 = 0x02;
pub const K_GETADDR: u8 = 0x03;
pub const K_ADDR: u8 = 0x04;
pub const K_STEM: u8 = 0x05;
pub const K_REKEY: u8 = 0x06;
pub const K_ACCEPT: u8 = 0x07;
pub const K_ANNOUNCE: u8 = 0x08;

pub const MAX_ADDR_PER_MSG: usize = 64;
const EVENT_QUEUE: usize = 256;
const SEND_QUEUE_MSGS: usize = 1024;
const MAX_PENDING_DIALS: usize = 8;
const THIN_DB: usize = 8;

/// Timing and limit knobs. Defaults are the production values from DESIGN
/// §8; tests shrink them via [`crate::lwp::Network::start_tuned`].
#[derive(Clone, Debug)]
pub struct Tuning {
    pub handshake_timeout: Duration,
    pub connect_timeout: Duration,
    pub tick: Duration,
    /// No frame at all for this long ⇒ peer is dead.
    pub dead_timeout: Duration,
    pub ping_interval: Duration,
    pub cover_min: Duration,
    pub cover_max: Duration,
    /// A whole frame must arrive within this long once its length is read.
    pub frame_timeout: Duration,
    pub write_timeout: Duration,
    pub rekey_frames: u64,
    pub rekey_interval: Duration,
    pub embargo_min: Duration,
    pub embargo_max: Duration,
    pub epoch: Duration,
    pub stem_probability: f64,
    pub ban_duration: Duration,
    pub backoff_base: Duration,
    pub backoff_max: Duration,
    pub save_interval: Duration,
    pub anchor_min_age: Duration,
    pub max_inbound_handshakes: usize,
    pub per_ip_inbound: usize,
    /// Per-peer cap on bytes queued for sending; exceeding it drops the peer.
    pub max_queue_bytes: usize,
    /// Accept/gossip loopback & private addresses (single-machine tests only).
    pub allow_local_gossip: bool,
}

impl Default for Tuning {
    fn default() -> Self {
        Tuning {
            handshake_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(10),
            tick: Duration::from_millis(500),
            dead_timeout: Duration::from_secs(120),
            ping_interval: Duration::from_secs(45),
            cover_min: Duration::from_secs(5),
            cover_max: Duration::from_secs(30),
            frame_timeout: Duration::from_secs(120),
            write_timeout: Duration::from_secs(60),
            rekey_frames: 1 << 20,
            rekey_interval: Duration::from_secs(3600),
            embargo_min: Duration::from_secs(30),
            embargo_max: Duration::from_secs(60),
            epoch: Duration::from_secs(600),
            stem_probability: 0.9,
            ban_duration: Duration::from_secs(24 * 3600),
            backoff_base: Duration::from_secs(5),
            backoff_max: Duration::from_secs(3600),
            save_interval: Duration::from_secs(300),
            anchor_min_age: Duration::from_secs(120),
            max_inbound_handshakes: 16,
            per_ip_inbound: 2,
            max_queue_bytes: 32 << 20,
            allow_local_gossip: false,
        }
    }
}

impl Tuning {
    /// Short timers for single-machine tests.
    pub fn fast_for_tests() -> Self {
        Tuning {
            handshake_timeout: Duration::from_secs(10),
            connect_timeout: Duration::from_secs(5),
            tick: Duration::from_millis(100),
            backoff_base: Duration::from_millis(300),
            backoff_max: Duration::from_secs(2),
            embargo_min: Duration::from_millis(1500),
            embargo_max: Duration::from_millis(2500),
            save_interval: Duration::from_secs(1),
            anchor_min_age: Duration::from_millis(200),
            allow_local_gossip: true,
            ..Tuning::default()
        }
    }
}

enum Out {
    Frame(u8, Arc<Vec<u8>>),
}

pub(crate) struct PeerHandle {
    id: PeerId,
    addr: String,
    inbound: bool,
    is_seed: bool,
    is_manual: bool,
    group: String,
    identity: IdentityPublic,
    id_key: String,
    remote_ip: Option<IpAddr>,
    dial_addr: Option<String>,
    connected: Instant,
    bytes_in: AtomicU64,
    bytes_out: AtomicU64,
    tx: Sender<Out>,
    queued: AtomicUsize,
    close_tx: Mutex<Option<Sender<()>>>,
    stream: TcpStream,
    score: AtomicU32,
    getaddr_answered: AtomicBool,
    addr_pending: AtomicBool,
    announced: AtomicBool,
    stem_bucket: Mutex<(f64, Instant)>,
}

impl PeerHandle {
    fn close(&self) {
        self.close_tx.lock().unwrap().take();
        let _ = self.stream.shutdown(Shutdown::Both);
    }

    fn enqueue(&self, kind: u8, payload: Arc<Vec<u8>>, max_bytes: usize) -> bool {
        let n = payload.len();
        let q = self.queued.fetch_add(n, Ordering::SeqCst) + n;
        if q > max_bytes || self.tx.try_send(Out::Frame(kind, payload)).is_err() {
            // Slow (or dead) peer: drop it rather than buffer without bound.
            self.queued.fetch_sub(n, Ordering::SeqCst);
            self.close();
            return false;
        }
        true
    }

    fn info(&self) -> PeerInfo {
        PeerInfo {
            id: self.id,
            addr: self.addr.clone(),
            inbound: self.inbound,
            identity_fingerprint: self.identity.fingerprint(),
            is_seed: self.is_seed,
            connected_secs: self.connected.elapsed().as_secs(),
            bytes_in: self.bytes_in.load(Ordering::Relaxed),
            bytes_out: self.bytes_out.load(Ordering::Relaxed),
        }
    }
}

struct SeedEntry {
    key: String,
    addr: NetAddr,
    identity: IdentityPublic,
}

pub(crate) struct Shared {
    magic: [u8; 4],
    network: u8,
    seeds: Vec<SeedEntry>,
    proxy: Option<SocketAddr>,
    max_out: usize,
    max_in: usize,
    identity: IdentityKey,
    own_pub: IdentityPublic,
    ephemeral: bool,
    manual: Vec<(String, NetAddr)>,
    announce_port: Option<u16>,
    pub local_addr: Option<SocketAddr>,
    pub t: Tuning,
    pub shutdown: AtomicBool,
    /// Taken (dropped) at the end of shutdown so the node's receiver
    /// disconnects once drained.
    events: Mutex<Option<Sender<NetEvent>>>,
    peers: Mutex<HashMap<PeerId, Arc<PeerHandle>>>,
    next_id: AtomicU64,
    /// addr key → (group, counts toward outbound slots)
    pending: Mutex<HashMap<String, (String, bool)>>,
    inbound_hs: Mutex<HashMap<IpAddr, usize>>,
    self_nonces: SelfNonces,
    self_addrs: Mutex<HashSet<String>>,
    db: Mutex<PeerDb>,
    dl: Mutex<Dandelion>,
    live_threads: AtomicUsize,
    workers: Mutex<Vec<JoinHandle<()>>>,
}

struct ThreadGuard(Arc<Shared>);
impl Drop for ThreadGuard {
    fn drop(&mut self) {
        self.0.live_threads.fetch_sub(1, Ordering::SeqCst);
    }
}

fn spawn_tracked(sh: &Arc<Shared>, name: &str, f: impl FnOnce() + Send + 'static) {
    sh.live_threads.fetch_add(1, Ordering::SeqCst);
    let g = ThreadGuard(sh.clone());
    let r = thread::Builder::new().name(name.into()).spawn(move || {
        let _g = g;
        f()
    });
    // On spawn failure the closure (and its guard) is dropped, which keeps
    // the live-thread counter balanced.
    drop(r);
}

fn rand_between(a: Duration, b: Duration) -> Duration {
    if b <= a {
        return a;
    }
    a + Duration::from_millis(rand::thread_rng().gen_range(0..=(b - a).as_millis() as u64))
}

fn id_key(pk: &IdentityPublic) -> String {
    format!("id:{}", hex(&hash::h256("LW1/p2p/ban-id", &[&pk.0])[..16]))
}

/// How an outbound dial is treated: `pin` = seed identity that must be
/// presented; `manual` = operator-configured (outside slot/group rules).
#[derive(Clone)]
struct DialKind {
    pin: Option<IdentityPublic>,
    manual: bool,
}

pub(crate) fn start(cfg: NetConfig, t: Tuning) -> io::Result<(Arc<Shared>, Receiver<NetEvent>)> {
    let listener = match cfg.listen {
        Some(a) => {
            let l = TcpListener::bind(a)?;
            l.set_nonblocking(true)?;
            Some(l)
        }
        None => None,
    };
    let local_addr = listener.as_ref().and_then(|l| l.local_addr().ok());
    let announce_port = if cfg.socks5_proxy.is_none() { local_addr.map(|a| a.port()) } else { None };
    let seeds = cfg
        .seeds
        .iter()
        .filter_map(|s| NetAddr::parse(&s.addr).map(|a| SeedEntry { key: a.to_string(), addr: a, identity: s.identity.clone() }))
        .filter(|s| cfg.socks5_proxy.is_some() || !matches!(s.addr.host, Host::Onion(_)))
        .collect();
    let manual = cfg
        .manual_peers
        .iter()
        .filter_map(|m| NetAddr::parse(m))
        .filter(|a| cfg.socks5_proxy.is_some() || !matches!(a.host, Host::Onion(_)))
        .map(|a| (a.to_string(), a))
        .collect();
    let (etx, erx) = bounded(EVENT_QUEUE);
    let own_pub = cfg.identity.public();
    let data_dir: PathBuf = cfg.data_dir.clone();
    let sh = Arc::new(Shared {
        magic: cfg.network_magic,
        network: cfg.network,
        seeds,
        proxy: cfg.socks5_proxy,
        max_out: cfg.max_outbound,
        max_in: cfg.max_inbound,
        identity: cfg.identity,
        own_pub,
        ephemeral: cfg.ephemeral_outbound_identity,
        manual,
        announce_port,
        local_addr,
        t,
        shutdown: AtomicBool::new(false),
        events: Mutex::new(Some(etx)),
        peers: Mutex::new(HashMap::new()),
        next_id: AtomicU64::new(1),
        pending: Mutex::new(HashMap::new()),
        inbound_hs: Mutex::new(HashMap::new()),
        self_nonces: SelfNonces::default(),
        self_addrs: Mutex::new(HashSet::new()),
        db: Mutex::new(PeerDb::load(&data_dir)),
        dl: Mutex::new(Dandelion::new()),
        live_threads: AtomicUsize::new(0),
        workers: Mutex::new(Vec::new()),
    });
    let mut workers = Vec::new();
    if let Some(l) = listener {
        let s = sh.clone();
        workers.push(thread::Builder::new().name("lw-net-listen".into()).spawn(move || listen_loop(s, l))?);
    }
    let s = sh.clone();
    workers.push(thread::Builder::new().name("lw-net-manager".into()).spawn(move || manager_loop(s))?);
    *sh.workers.lock().unwrap() = workers;
    Ok((sh, erx))
}

impl Shared {
    fn emit(&self, mut ev: NetEvent) {
        let Some(tx) = self.events.lock().unwrap().clone() else { return };
        loop {
            match tx.send_timeout(ev, Duration::from_millis(200)) {
                Ok(()) => return,
                Err(SendTimeoutError::Timeout(e)) => {
                    if self.shutdown.load(Ordering::SeqCst) {
                        return;
                    }
                    ev = e;
                }
                Err(SendTimeoutError::Disconnected(_)) => return,
            }
        }
    }

    fn down(&self) -> bool {
        self.shutdown.load(Ordering::SeqCst)
    }

    fn handle(&self, id: PeerId) -> Option<Arc<PeerHandle>> {
        self.peers.lock().unwrap().get(&id).cloned()
    }

    pub fn send(&self, peer: PeerId, kind: u8, payload: Vec<u8>) {
        if kind < APP_KIND_MIN || payload.len() > MAX_PAYLOAD {
            return;
        }
        if let Some(h) = self.handle(peer) {
            h.enqueue(kind, Arc::new(payload), self.t.max_queue_bytes);
        }
    }

    fn broadcast_arc(&self, kind: u8, payload: Arc<Vec<u8>>, except: Option<PeerId>) {
        let hs: Vec<Arc<PeerHandle>> = self.peers.lock().unwrap().values().cloned().collect();
        for h in hs {
            if Some(h.id) != except {
                h.enqueue(kind, payload.clone(), self.t.max_queue_bytes);
            }
        }
    }

    pub fn broadcast(&self, kind: u8, payload: Vec<u8>, except: Option<PeerId>) {
        if kind < APP_KIND_MIN || payload.len() > MAX_PAYLOAD {
            return;
        }
        self.broadcast_arc(kind, Arc::new(payload), except);
    }

    pub fn relay_tx(&self, tx_id: [u8; 32], payload: Vec<u8>, fluff_kind: u8, from: Option<PeerId>) {
        if fluff_kind < APP_KIND_MIN || payload.is_empty() || payload.len() > MAX_PAYLOAD {
            return;
        }
        let outbound: Vec<PeerId> = self.peers.lock().unwrap().values().filter(|h| !h.inbound).map(|h| h.id).collect();
        let payload = Arc::new(payload);
        let decision = self.dl.lock().unwrap().decide(tx_id, from, &outbound, self.t.epoch, self.t.stem_probability);
        match decision {
            Decision::Drop => {}
            Decision::Fluff => self.broadcast_arc(fluff_kind, payload, from),
            Decision::Stem(p) => {
                let sent = self.handle(p).map(|h| h.enqueue(K_STEM, payload.clone(), self.t.max_queue_bytes)).unwrap_or(false);
                let e = Embargoed { deadline: Instant::now() + rand_between(self.t.embargo_min, self.t.embargo_max), payload: payload.clone(), kind: fluff_kind };
                if !sent || !self.dl.lock().unwrap().start_embargo(tx_id, e) {
                    self.broadcast_arc(fluff_kind, payload, from);
                }
            }
        }
    }

    pub fn mark_fluffed(&self, tx_id: [u8; 32]) {
        self.dl.lock().unwrap().mark_fluffed(tx_id);
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        let mut v: Vec<PeerInfo> = self.peers.lock().unwrap().values().map(|h| h.info()).collect();
        v.sort_by_key(|p| p.id);
        v
    }

    pub fn peer_netgroup(&self, id: PeerId) -> Option<String> {
        self.handle(id).map(|h| h.group.clone())
    }

    pub fn misbehaving(&self, peer: PeerId, score: u32, _reason: &str) {
        let Some(h) = self.handle(peer) else { return };
        let total = h.score.fetch_add(score, Ordering::SeqCst).saturating_add(score);
        if total >= 100 {
            {
                let mut db = self.db.lock().unwrap();
                let d = self.t.ban_duration;
                db.ban(h.id_key.clone(), d);
                if let Some(ip) = h.remote_ip {
                    // Only routable IPs are banned by address: loopback/private
                    // sources (Tor inbound arrives from 127.0.0.1!) would
                    // otherwise let one bad peer deafen every local/Tor peer.
                    if ip_is_routable(ip) {
                        db.ban(format!("ip:{ip}"), d);
                    }
                }
                if let Some(a) = &h.dial_addr {
                    db.ban(format!("addr:{a}"), d);
                }
                db.save();
            }
            h.close();
        }
    }

    pub fn shutdown(&self) {
        if self.shutdown.swap(true, Ordering::SeqCst) {
            return;
        }
        let hs: Vec<Arc<PeerHandle>> = self.peers.lock().unwrap().values().cloned().collect();
        for h in hs {
            h.close();
        }
        let workers: Vec<JoinHandle<()>> = std::mem::take(&mut *self.workers.lock().unwrap());
        for w in workers {
            let _ = w.join();
        }
        // Connection threads exit within one socket timeout of the shutdown;
        // dial/handshake threads within their deadline.
        let until = Instant::now() + self.t.handshake_timeout + Duration::from_secs(1);
        while self.live_threads.load(Ordering::SeqCst) > 0 && Instant::now() < until {
            let hs: Vec<Arc<PeerHandle>> = self.peers.lock().unwrap().values().cloned().collect();
            for h in hs {
                h.close();
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.events.lock().unwrap().take();
    }

    fn is_banned(&self, keys: &[String]) -> bool {
        let db = self.db.lock().unwrap();
        keys.iter().any(|k| db.is_banned(k))
    }
}

// ───────────────────────── listener / inbound ─────────────────────────

fn listen_loop(sh: Arc<Shared>, l: TcpListener) {
    while !sh.down() {
        match l.accept() {
            Ok((s, peer)) => accept_one(&sh, s, peer),
            Err(e) if e.kind() == io::ErrorKind::WouldBlock => thread::sleep(Duration::from_millis(25)),
            Err(_) => thread::sleep(Duration::from_millis(100)),
        }
    }
}

fn accept_one(sh: &Arc<Shared>, s: TcpStream, peer: SocketAddr) {
    let ip = canon_ip(peer.ip());
    if s.set_nonblocking(false).is_err() || s.set_nodelay(true).is_err() {
        return;
    }
    if ip_is_routable(ip) && sh.is_banned(&[format!("ip:{ip}")]) {
        return;
    }
    {
        let peers = sh.peers.lock().unwrap();
        let mut hs = sh.inbound_hs.lock().unwrap();
        let hs_total: usize = hs.values().sum();
        let inbound = peers.values().filter(|h| h.inbound).count();
        if inbound + hs_total >= sh.max_in || hs_total >= sh.t.max_inbound_handshakes {
            return;
        }
        // Per-IP limit. Loopback is exempt: with Tor every inbound peer comes
        // from 127.0.0.1, and single-machine test networks need it too.
        if !ip.is_loopback() {
            let same = peers.values().filter(|h| h.inbound && h.remote_ip == Some(ip)).count() + hs.get(&ip).copied().unwrap_or(0);
            if same >= sh.t.per_ip_inbound {
                return;
            }
        }
        *hs.entry(ip).or_insert(0) += 1;
    }
    let sh2 = sh.clone();
    spawn_tracked(sh, "lw-net-inbound-hs", move || {
        let res = inbound_handshake(&sh2, s, peer);
        {
            let mut hs = sh2.inbound_hs.lock().unwrap();
            if let Some(c) = hs.get_mut(&ip) {
                *c -= 1;
                if *c == 0 {
                    hs.remove(&ip);
                }
            }
        }
        if let Some((s, sess)) = res {
            let meta = Meta {
                addr: peer.to_string(),
                inbound: true,
                is_seed: false,
                is_manual: false,
                group: ip_group(ip, peer.port()),
                remote_ip: Some(ip),
                dial_addr: None,
            };
            register(&sh2, s, sess, meta);
        }
    });
}

fn inbound_handshake(sh: &Arc<Shared>, mut s: TcpStream, _peer: SocketAddr) -> Option<(TcpStream, Session)> {
    let p = Params {
        magic: sh.magic,
        network: sh.network,
        deadline: Instant::now() + sh.t.handshake_timeout,
        self_nonces: &sh.self_nonces,
        own_identity: &sh.own_pub,
    };
    let sess = handshake::respond(&mut s, &p, &sh.identity).ok()?;
    if sh.down() || sh.is_banned(&[id_key(&sess.peer)]) {
        return None;
    }
    let inbound = sh.peers.lock().unwrap().values().filter(|h| h.inbound).count();
    if inbound >= sh.max_in {
        return None;
    }
    Some((s, sess))
}

// ───────────────────────────── outbound ──────────────────────────────

fn connect_direct(a: &NetAddr, timeout: Duration) -> io::Result<TcpStream> {
    let addrs: Vec<SocketAddr> = match &a.host {
        Host::Ip(ip) => vec![SocketAddr::new(*ip, a.port)],
        // Direct mode only: resolve via the OS (with a proxy we never get here).
        Host::Name(n) => (n.as_str(), a.port).to_socket_addrs()?.take(4).collect(),
        Host::Onion(_) => return Err(io::Error::other("onion requires a proxy")),
    };
    let mut last = io::Error::other("no address");
    for sa in addrs {
        match TcpStream::connect_timeout(&sa, timeout) {
            Ok(s) => return Ok(s),
            Err(e) => last = e,
        }
    }
    Err(last)
}

fn start_dial(sh: &Arc<Shared>, key: String, addr: NetAddr, kind: DialKind) {
    let counts = !kind.manual;
    sh.pending.lock().unwrap().insert(key.clone(), (addr.group(), counts));
    let sh2 = sh.clone();
    spawn_tracked(sh, "lw-net-dial", move || {
        let res = outbound_handshake(&sh2, &addr, &kind);
        sh2.pending.lock().unwrap().remove(&key);
        match res {
            Err(HsError::SelfConnect) => {
                sh2.self_addrs.lock().unwrap().insert(key);
            }
            Err(_) => {
                let (b, m) = (sh2.t.backoff_base, sh2.t.backoff_max);
                sh2.db.lock().unwrap().on_failure(&key, b, m);
            }
            Ok((s, sess)) => {
                let remote_ip = if sh2.proxy.is_none() { s.peer_addr().ok().map(|a| canon_ip(a.ip())) } else { None };
                let group = match remote_ip {
                    Some(ip) => ip_group(ip, addr.port),
                    None => addr.group(),
                };
                let ok = admit_outbound(&sh2, &sess, &kind, &group, remote_ip, &key);
                let (b, m) = (sh2.t.backoff_base, sh2.t.backoff_max);
                if !ok {
                    sh2.db.lock().unwrap().on_failure(&key, b, m);
                    return;
                }
                sh2.db.lock().unwrap().on_success(&key);
                let meta = Meta {
                    addr: key.clone(),
                    inbound: false,
                    is_seed: kind.pin.is_some(),
                    is_manual: kind.manual,
                    group,
                    remote_ip,
                    dial_addr: Some(key),
                };
                register(&sh2, s, sess, meta);
            }
        }
    });
}

fn admit_outbound(sh: &Shared, sess: &Session, kind: &DialKind, group: &str, remote_ip: Option<IpAddr>, key: &str) -> bool {
    if sh.down() {
        return false;
    }
    let mut keys = vec![id_key(&sess.peer), format!("addr:{key}")];
    if let Some(ip) = remote_ip {
        if ip_is_routable(ip) {
            keys.push(format!("ip:{ip}"));
        }
    }
    if sh.is_banned(&keys) {
        return false;
    }
    let peers = sh.peers.lock().unwrap();
    // Never two outbound connections to the same node.
    if peers.values().any(|h| !h.inbound && h.identity == sess.peer) {
        return false;
    }
    if !kind.manual {
        let out: Vec<&Arc<PeerHandle>> = peers.values().filter(|h| !h.inbound && !h.is_manual).collect();
        if out.len() >= sh.max_out || out.iter().any(|h| h.group == group) {
            return false;
        }
    }
    true
}

fn outbound_handshake(sh: &Arc<Shared>, addr: &NetAddr, kind: &DialKind) -> Result<(TcpStream, Session), HsError> {
    let mut s = match sh.proxy {
        Some(p) => crate::lwp::socks5::connect(p, addr, sh.t.connect_timeout)?,
        None => connect_direct(addr, sh.t.connect_timeout)?,
    };
    s.set_nodelay(true)?;
    let eph;
    let me = if sh.ephemeral {
        eph = IdentityKey::generate();
        &eph
    } else {
        &sh.identity
    };
    let deadline = Instant::now() + sh.t.handshake_timeout;
    let p = Params { magic: sh.magic, network: sh.network, deadline, self_nonces: &sh.self_nonces, own_identity: &sh.own_pub };
    let mut sess = handshake::initiate(&mut s, &p, me, kind.pin.as_ref())?;
    // Key confirmation + admission: the responder's first frame must be ACCEPT.
    let left = deadline.saturating_duration_since(Instant::now()).max(Duration::from_millis(1));
    let outer = frame::read_frame(&mut s, left, left).map_err(|_| HsError::Proto("no accept"))?;
    match sess.recv.open(&outer) {
        Ok((K_ACCEPT, p)) if p.is_empty() => Ok((s, sess)),
        _ => Err(HsError::Proto("bad accept")),
    }
}

// ───────────────────────────── connections ──────────────────────────────

struct Meta {
    addr: String,
    inbound: bool,
    is_seed: bool,
    is_manual: bool,
    group: String,
    remote_ip: Option<IpAddr>,
    dial_addr: Option<String>,
}

fn register(sh: &Arc<Shared>, s: TcpStream, sess: Session, m: Meta) {
    let (Ok(s_w), Ok(s_h)) = (s.try_clone(), s.try_clone()) else { return };
    let (tx, rx) = bounded(SEND_QUEUE_MSGS);
    let (ctx, crx) = bounded::<()>(1);
    let id = sh.next_id.fetch_add(1, Ordering::SeqCst);
    let Session { send, recv, peer } = sess;
    let h = Arc::new(PeerHandle {
        id,
        addr: m.addr,
        inbound: m.inbound,
        is_seed: m.is_seed,
        is_manual: m.is_manual,
        group: m.group,
        id_key: id_key(&peer),
        identity: peer,
        remote_ip: m.remote_ip,
        dial_addr: m.dial_addr,
        connected: Instant::now(),
        bytes_in: AtomicU64::new(0),
        bytes_out: AtomicU64::new(0),
        tx,
        queued: AtomicUsize::new(0),
        close_tx: Mutex::new(Some(ctx)),
        stream: s_h,
        score: AtomicU32::new(0),
        getaddr_answered: AtomicBool::new(false),
        addr_pending: AtomicBool::new(false),
        announced: AtomicBool::new(false),
        stem_bucket: Mutex::new((STEM_BURST, Instant::now())),
    });
    if sh.down() {
        return;
    }
    sh.peers.lock().unwrap().insert(id, h.clone());
    sh.emit(NetEvent::Connected(h.info()));
    let (sh_w, h_w) = (sh.clone(), h.clone());
    spawn_tracked(sh, "lw-net-writer", move || writer_loop(sh_w, h_w, s_w, send, rx, crx));
    let (sh_r, h_r) = (sh.clone(), h.clone());
    spawn_tracked(sh, "lw-net-reader", move || reader_loop(sh_r, h_r, s, recv));
    if !h.inbound {
        h.addr_pending.store(true, Ordering::SeqCst);
        h.enqueue(K_GETADDR, Arc::new(Vec::new()), sh.t.max_queue_bytes);
        if let Some(port) = sh.announce_port {
            h.enqueue(K_ANNOUNCE, Arc::new(port.to_le_bytes().to_vec()), sh.t.max_queue_bytes);
        }
    }
}

fn send_frame(sh: &Shared, h: &PeerHandle, s: &mut TcpStream, cs: &mut CipherState, kind: u8, payload: &[u8]) -> io::Result<()> {
    let limit = sh.t.rekey_frames.max(2);
    if cs.since_rekey + 2 > limit || cs.rekeyed_at.elapsed() >= sh.t.rekey_interval {
        let f = cs.seal(K_REKEY, &[]);
        frame::write_all(s, &f)?;
        h.bytes_out.fetch_add(f.len() as u64, Ordering::Relaxed);
        cs.ratchet();
    }
    let f = cs.seal(kind, payload);
    frame::write_all(s, &f)?;
    h.bytes_out.fetch_add(f.len() as u64, Ordering::Relaxed);
    Ok(())
}

fn writer_loop(sh: Arc<Shared>, h: Arc<PeerHandle>, mut s: TcpStream, mut cs: CipherState, rx: Receiver<Out>, crx: Receiver<()>) {
    let _ = s.set_write_timeout(Some(sh.t.write_timeout));
    let res = (|| -> io::Result<()> {
        if h.inbound {
            send_frame(&sh, &h, &mut s, &mut cs, K_ACCEPT, &[])?;
        }
        let mut next_cover = Instant::now() + rand_between(sh.t.cover_min, sh.t.cover_max);
        let mut next_ping = Instant::now() + sh.t.ping_interval;
        loop {
            if sh.down() {
                return Ok(());
            }
            let wait = next_cover.min(next_ping).saturating_duration_since(Instant::now());
            select! {
                recv(rx) -> m => match m {
                    Ok(Out::Frame(k, p)) => {
                        h.queued.fetch_sub(p.len(), Ordering::SeqCst);
                        send_frame(&sh, &h, &mut s, &mut cs, k, &p)?;
                        next_cover = Instant::now() + rand_between(sh.t.cover_min, sh.t.cover_max);
                    }
                    Err(_) => return Ok(()),
                },
                recv(crx) -> _ => return Ok(()),
                default(wait) => {
                    let now = Instant::now();
                    if now >= next_ping {
                        let mut n = [0u8; 8];
                        kdf::random_bytes(&mut n);
                        send_frame(&sh, &h, &mut s, &mut cs, K_PING, &n)?;
                        next_ping = now + sh.t.ping_interval;
                    } else if now >= next_cover {
                        // Cover traffic: a padding frame in the 1 KiB or 4 KiB bucket.
                        let n = rand::thread_rng().gen_range(0..3000usize);
                        let mut pad = vec![0u8; n];
                        kdf::random_bytes(&mut pad);
                        send_frame(&sh, &h, &mut s, &mut cs, K_PADDING, &pad)?;
                    }
                    next_cover = Instant::now() + rand_between(sh.t.cover_min, sh.t.cover_max);
                }
            }
        }
    })();
    let _ = res;
    h.close();
}

const STEM_RATE: f64 = 10.0;
const STEM_BURST: f64 = 50.0;

fn reader_loop(sh: Arc<Shared>, h: Arc<PeerHandle>, mut s: TcpStream, mut cs: CipherState) {
    let limit = sh.t.rekey_frames.max(2);
    let max_key_age = sh.t.rekey_interval * 2 + sh.t.dead_timeout;
    loop {
        if sh.down() {
            break;
        }
        let Ok(outer) = frame::read_frame(&mut s, sh.t.dead_timeout, sh.t.frame_timeout) else { break };
        // Any authentication/order failure is fatal (no ban: an on-path
        // attacker could otherwise get honest peers banned).
        let Ok((kind, payload)) = cs.open(&outer) else { break };
        h.bytes_in.fetch_add(outer.len() as u64 + 4, Ordering::Relaxed);
        if kind == K_REKEY {
            if !payload.is_empty() {
                break;
            }
            cs.ratchet();
            continue;
        }
        if cs.since_rekey >= limit || cs.rekeyed_at.elapsed() > max_key_age {
            break; // peer failed to rekey on schedule
        }
        match kind {
            K_PADDING | K_PONG => {}
            K_PING => {
                if payload.len() == 8 {
                    h.enqueue(K_PONG, Arc::new(payload), sh.t.max_queue_bytes);
                } else {
                    sh.misbehaving(h.id, 10, "bad ping");
                }
            }
            K_GETADDR => {
                // Answer only inbound peers, once per connection (anti-scraping
                // and anti-fingerprinting, as in Bitcoin Core).
                if h.inbound && payload.is_empty() && !h.getaddr_answered.swap(true, Ordering::SeqCst) {
                    let sample = sh.db.lock().unwrap().gossip_sample(MAX_ADDR_PER_MSG, sh.t.allow_local_gossip);
                    h.enqueue(K_ADDR, Arc::new(encode_addr(&sample)), sh.t.max_queue_bytes);
                }
            }
            K_ADDR => {
                if !h.addr_pending.swap(false, Ordering::SeqCst) {
                    sh.misbehaving(h.id, 5, "unsolicited addr");
                } else {
                    match decode_addr(&payload) {
                        Some(list) => {
                            let mut db = sh.db.lock().unwrap();
                            for a in list {
                                if gossip_acceptable(&sh, &a) {
                                    db.add(a);
                                }
                            }
                        }
                        None => sh.misbehaving(h.id, 20, "bad addr"),
                    }
                }
            }
            K_ANNOUNCE => {
                if h.inbound && payload.len() == 2 && !h.announced.swap(true, Ordering::SeqCst) {
                    let port = u16::from_le_bytes([payload[0], payload[1]]);
                    if let (Some(ip), true) = (h.remote_ip, port != 0) {
                        let a = NetAddr { host: Host::Ip(ip), port };
                        if gossip_acceptable(&sh, &a) {
                            sh.db.lock().unwrap().add(a);
                        }
                    }
                }
            }
            K_STEM => {
                let allowed = {
                    let mut b = h.stem_bucket.lock().unwrap();
                    let now = Instant::now();
                    b.0 = (b.0 + now.duration_since(b.1).as_secs_f64() * STEM_RATE).min(STEM_BURST);
                    b.1 = now;
                    if b.0 >= 1.0 {
                        b.0 -= 1.0;
                        true
                    } else {
                        false
                    }
                };
                if allowed && !payload.is_empty() {
                    sh.emit(NetEvent::StemTx { peer: h.id, payload });
                }
            }
            K_ACCEPT => break, // only valid as the very first frame
            k if k < APP_KIND_MIN => sh.misbehaving(h.id, 10, "unknown control kind"),
            k => sh.emit(NetEvent::Message { peer: h.id, kind: k, payload }),
        }
    }
    sh.peers.lock().unwrap().remove(&h.id);
    h.close();
    sh.emit(NetEvent::Disconnected(h.id));
}

fn gossip_acceptable(sh: &Shared, a: &NetAddr) -> bool {
    match a.host {
        Host::Ip(ip) => sh.t.allow_local_gossip || ip_is_routable(ip),
        Host::Onion(_) => false, // never learned or relayed via gossip (see DESIGN §8.3)
        Host::Name(_) => false,  // would force DNS lookups chosen by a peer
    }
}

/// `count:u16_le ‖ count × (len:u8 ‖ addr)`, count ≤ 64.
pub(crate) fn encode_addr(list: &[String]) -> Vec<u8> {
    let list: Vec<&String> = list.iter().filter(|a| a.len() <= 255).take(MAX_ADDR_PER_MSG).collect();
    let mut v = (list.len() as u16).to_le_bytes().to_vec();
    for a in list {
        v.push(a.len() as u8);
        v.extend_from_slice(a.as_bytes());
    }
    v
}

pub(crate) fn decode_addr(b: &[u8]) -> Option<Vec<NetAddr>> {
    let mut r = Rd::new(b);
    let n = r.u16()? as usize;
    if n > MAX_ADDR_PER_MSG {
        return None;
    }
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        let l = r.u8()? as usize;
        out.push(NetAddr::parse(std::str::from_utf8(r.take(l)?).ok()?)?);
    }
    r.done().then_some(out)
}

// ───────────────────────────── manager ──────────────────────────────

fn manager_loop(sh: Arc<Shared>) {
    let mut last_save = Instant::now();
    while !sh.down() {
        let until = Instant::now() + sh.t.tick;
        while Instant::now() < until && !sh.down() {
            thread::sleep(Duration::from_millis(20).min(sh.t.tick));
        }
        if sh.down() {
            break;
        }
        let expired = sh.dl.lock().unwrap().expired(Instant::now());
        for e in expired {
            sh.broadcast_arc(e.kind, e.payload, None);
        }
        fill_outbound(&sh);
        if last_save.elapsed() >= sh.t.save_interval {
            update_anchors(&sh);
            sh.db.lock().unwrap().save();
            last_save = Instant::now();
        }
    }
    update_anchors(&sh);
    sh.db.lock().unwrap().save();
}

fn update_anchors(sh: &Shared) {
    let mut out: Vec<(Duration, String)> = sh
        .peers
        .lock()
        .unwrap()
        .values()
        .filter(|h| !h.inbound && !h.is_manual && h.connected.elapsed() >= sh.t.anchor_min_age)
        .filter_map(|h| h.dial_addr.clone().map(|a| (h.connected.elapsed(), a)))
        .collect();
    if out.is_empty() {
        return;
    }
    out.sort_by_key(|x| std::cmp::Reverse(x.0));
    sh.db.lock().unwrap().anchors = out.into_iter().take(MAX_ANCHORS).map(|x| x.1).collect();
}

fn fill_outbound(sh: &Arc<Shared>) {
    let now = Instant::now();
    let (connected_addrs, mut used_groups, mut n_out) = {
        let peers = sh.peers.lock().unwrap();
        let addrs: HashSet<String> = peers.values().filter_map(|h| h.dial_addr.clone()).collect();
        let out: Vec<&Arc<PeerHandle>> = peers.values().filter(|h| !h.inbound && !h.is_manual).collect();
        (addrs, out.iter().map(|h| h.group.clone()).collect::<HashSet<String>>(), out.len())
    };
    let pending = sh.pending.lock().unwrap().clone();
    for (g, counts) in pending.values() {
        if *counts {
            used_groups.insert(g.clone());
            n_out += 1;
        }
    }
    let self_addrs = sh.self_addrs.lock().unwrap().clone();
    let mut n_pending = pending.len();
    let usable = |key: &str, a: &NetAddr, db: &PeerDb| -> bool {
        let mut banned = db.is_banned(&format!("addr:{key}"));
        if let Some(ip) = a.ip() {
            banned |= ip_is_routable(ip) && db.is_banned(&format!("ip:{ip}"));
        }
        !banned && !self_addrs.contains(key) && !connected_addrs.contains(key) && !pending.contains_key(key) && db.can_try(key, now)
    };

    // 1. Manual peers: always retried, outside the slot/group rules.
    let manual: Vec<(String, NetAddr)> = {
        let db = sh.db.lock().unwrap();
        sh.manual.iter().filter(|(k, a)| usable(k, a, &db)).cloned().collect()
    };
    for (k, a) in manual {
        if n_pending >= MAX_PENDING_DIALS {
            return;
        }
        let pin = sh.seeds.iter().find(|s| s.key == k).map(|s| s.identity.clone());
        start_dial(sh, k, a, DialKind { pin, manual: true });
        n_pending += 1;
    }

    // 2. Outbound slots: anchors, then seeds (when the address book is thin
    //    or we have no outbound peers), then the address book.
    let mut started = 0;
    if n_out >= sh.max_out {
        return;
    }
    let candidates: Vec<(String, NetAddr, DialKind)> = {
        let db = sh.db.lock().unwrap();
        let mut c: Vec<(String, NetAddr, DialKind)> = Vec::new();
        let kind_for = |k: &str| DialKind { pin: sh.seeds.iter().find(|s| s.key == k).map(|s| s.identity.clone()), manual: false };
        for a in &db.anchors {
            if let Some(na) = NetAddr::parse(a) {
                c.push((a.clone(), na, kind_for(a)));
            }
        }
        if db.entries.len() < THIN_DB || n_out == 0 {
            let mut seeds: Vec<&SeedEntry> = sh.seeds.iter().collect();
            seeds.shuffle(&mut rand::thread_rng());
            for s in seeds {
                c.push((s.key.clone(), s.addr.clone(), DialKind { pin: Some(s.identity.clone()), manual: false }));
            }
        }
        let mut entries: Vec<_> = db.entries.values().collect();
        entries.shuffle(&mut rand::thread_rng());
        entries.sort_by_key(|e| e.last_success == 0); // previously-good first
        for e in entries {
            let k = e.addr.to_string();
            let kind = kind_for(&k);
            c.push((k, e.addr.clone(), kind));
        }
        c.into_iter()
            .filter(|(k, a, _)| usable(k, a, &db) && (sh.proxy.is_some() || !matches!(a.host, Host::Onion(_))))
            .collect()
    };
    let mut tried: HashSet<String> = HashSet::new();
    for (k, a, kind) in candidates {
        if n_out >= sh.max_out || started >= 2 || n_pending >= MAX_PENDING_DIALS {
            break;
        }
        let g = a.group();
        if used_groups.contains(&g) || !tried.insert(k.clone()) || sh.manual.iter().any(|(mk, _)| mk == &k) {
            continue;
        }
        used_groups.insert(g);
        start_dial(sh, k, a, kind);
        n_out += 1;
        n_pending += 1;
        started += 1;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addr_codec_bounded() {
        let l = vec!["1.2.3.4:5".to_string(), "[::1]:7".to_string()];
        let d = decode_addr(&encode_addr(&l)).unwrap();
        assert_eq!(d.iter().map(|a| a.to_string()).collect::<Vec<_>>(), l);
        assert!(decode_addr(&[]).is_none());
        assert!(decode_addr(&[65, 0]).is_none());
        assert!(decode_addr(&[1, 0, 200, b'a']).is_none());
        let mut trailing = encode_addr(&l);
        trailing.push(0);
        assert!(decode_addr(&trailing).is_none());
        for _ in 0..2000 {
            let n = rand::thread_rng().gen_range(0..64);
            let v: Vec<u8> = (0..n).map(|_| rand::random()).collect();
            let _ = decode_addr(&v);
        }
    }
}
