//! Runtime: listener coroutine, connection-manager coroutine, and one
//! coroutine per connection. Port of ling-net `lwp/net.rs`: the host's
//! listener / manager / per-connection reader + writer threads become
//! cooperative coroutines (the per-connection reader and writer share one
//! coroutine and one socket), `Arc<Mutex<_>>` becomes `Rc<RefCell<_>>`
//! (never borrowed across a yield), crossbeam channels become `VecDeque`s.
//! Everything on the wire — control kinds, ACCEPT-first, ping/pong, cover
//! traffic, rekeying, address gossip, Dandelion stem — is the host's.

use alloc::boxed::Box;
use alloc::collections::{BTreeMap, BTreeSet, VecDeque};
use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};
use core::net::{IpAddr, Ipv4Addr, SocketAddr};

use crate::drivers::netstack as ns;
use crate::lwp::addr::{canon_ip, ip_group, ip_is_routable, Host, NetAddr};
use crate::lwp::conn::{connect_ip, Conn, Ctx};
use crate::lwp::crypto::identity::{IdentityKey, IdentityPublic};
use crate::lwp::crypto::{hash, kdf};
use crate::lwp::dandelion::{Dandelion, Decision, Embargoed};
use crate::lwp::frame::{self, CipherState, Io, IoError, MAX_PAYLOAD};
use crate::lwp::handshake::{self, HsError, Params, SelfNonces, Session};
use crate::lwp::peerdb::{PeerDb, MAX_ANCHORS};
use crate::lwp::sys;
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
/// Frames a connection sends per turn before it lets others run.
const FRAMES_PER_TURN: usize = 16;

/// Timing and limit knobs, in milliseconds. Defaults are the production
/// values from DESIGN §8 (the host's `Tuning`).
#[derive(Clone, Debug)]
pub struct Tuning {
    pub handshake_timeout: u64,
    pub connect_timeout: u64,
    pub tick: u64,
    /// No frame at all for this long ⇒ peer is dead.
    pub dead_timeout: u64,
    pub ping_interval: u64,
    pub cover_min: u64,
    pub cover_max: u64,
    /// A whole frame must arrive within this long once its length is read.
    pub frame_timeout: u64,
    pub write_timeout: u64,
    pub rekey_frames: u64,
    pub rekey_interval: u64,
    pub embargo_min: u64,
    pub embargo_max: u64,
    pub epoch: u64,
    pub stem_probability: f64,
    pub ban_duration: u64,
    pub backoff_base: u64,
    pub backoff_max: u64,
    pub save_interval: u64,
    pub anchor_min_age: u64,
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
            handshake_timeout: 10_000,
            connect_timeout: 10_000,
            tick: 500,
            dead_timeout: 120_000,
            ping_interval: 45_000,
            cover_min: 5_000,
            cover_max: 30_000,
            frame_timeout: 120_000,
            write_timeout: 60_000,
            rekey_frames: 1 << 20,
            rekey_interval: 3_600_000,
            embargo_min: 30_000,
            embargo_max: 60_000,
            epoch: 600_000,
            stem_probability: 0.9,
            ban_duration: 24 * 3_600_000,
            backoff_base: 5_000,
            backoff_max: 3_600_000,
            save_interval: 300_000,
            anchor_min_age: 120_000,
            max_inbound_handshakes: 16,
            per_ip_inbound: 2,
            max_queue_bytes: 32 << 20,
            allow_local_gossip: false,
        }
    }
}

impl Tuning {
    /// Short timers for single-machine tests (the host's `fast_for_tests`).
    pub fn fast_for_tests() -> Self {
        Tuning {
            handshake_timeout: 10_000,
            connect_timeout: 5_000,
            tick: 100,
            backoff_base: 300,
            backoff_max: 2_000,
            embargo_min: 1_500,
            embargo_max: 2_500,
            save_interval: 1_000,
            anchor_min_age: 200,
            allow_local_gossip: true,
            ..Tuning::default()
        }
    }
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
    /// `sys::now_ms()` at registration.
    connected: u64,
    bytes_in: Cell<u64>,
    bytes_out: Cell<u64>,
    tx: RefCell<VecDeque<(u8, Rc<Vec<u8>>)>>,
    queued: Cell<usize>,
    /// The host's `close_tx` + `stream.shutdown`: the connection coroutine
    /// sees it on its next turn and tears the connection down.
    closed: Cell<bool>,
    score: Cell<u32>,
    getaddr_answered: Cell<bool>,
    addr_pending: Cell<bool>,
    announced: Cell<bool>,
    stem_bucket: Cell<(f64, u64)>,
}

impl PeerHandle {
    fn close(&self) {
        self.closed.set(true);
    }

    fn enqueue(&self, kind: u8, payload: Rc<Vec<u8>>, max_bytes: usize) -> bool {
        let n = payload.len();
        let q = self.queued.get() + n;
        self.queued.set(q);
        let full = self.tx.borrow().len() >= SEND_QUEUE_MSGS;
        if q > max_bytes || full || self.closed.get() {
            // Slow (or dead) peer: drop it rather than buffer without bound.
            self.queued.set(self.queued.get() - n);
            self.close();
            return false;
        }
        self.tx.borrow_mut().push_back((kind, payload));
        true
    }

    fn info(&self) -> PeerInfo {
        PeerInfo {
            id: self.id,
            addr: self.addr.clone(),
            inbound: self.inbound,
            identity_fingerprint: self.identity.fingerprint(),
            is_seed: self.is_seed,
            connected_secs: sys::now_ms().saturating_sub(self.connected) / 1000,
            bytes_in: self.bytes_in.get(),
            bytes_out: self.bytes_out.get(),
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
    listen_port: Option<u16>,
    pub local_addr: Option<SocketAddr>,
    pub t: Tuning,
    /// Shutdown flag + every socket this network holds.
    ctx: Ctx,
    events: RefCell<VecDeque<NetEvent>>,
    events_open: Cell<bool>,
    peers: RefCell<BTreeMap<PeerId, Rc<PeerHandle>>>,
    next_id: Cell<u64>,
    /// addr key → (group, counts toward outbound slots)
    pending: RefCell<BTreeMap<String, (String, bool)>>,
    inbound_hs: RefCell<BTreeMap<IpAddr, usize>>,
    self_nonces: SelfNonces,
    self_addrs: RefCell<BTreeSet<String>>,
    db: RefCell<PeerDb>,
    dl: RefCell<Dandelion>,
    live_threads: Cell<usize>,
    /// The isolate that started the network (0 = main).
    pub iso: u64,
    /// Coroutine ids of the listener and the manager.
    workers: RefCell<Vec<u64>>,
}

struct ThreadGuard(Rc<Shared>);
impl Drop for ThreadGuard {
    fn drop(&mut self) {
        self.0.live_threads.set(self.0.live_threads.get().saturating_sub(1));
    }
}

/// Run `f` on a new worker coroutine of the current isolate; its guard keeps
/// the live count right even when the coroutine cannot start.
fn spawn_tracked(sh: &Rc<Shared>, name: &str, f: impl FnOnce() + 'static) -> u64 {
    sh.live_threads.set(sh.live_threads.get() + 1);
    let g = ThreadGuard(sh.clone());
    crate::coro::spawn(
        name,
        false,
        Box::new(move || {
            let _g = g;
            f()
        }),
    )
}

fn rand_between(a: u64, b: u64) -> u64 {
    if b <= a {
        return a;
    }
    a + sys::gen_range_incl(0, b - a)
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

pub(crate) fn start(cfg: NetConfig, t: Tuning) -> Result<Rc<Shared>, String> {
    sys::install_entropy();
    let ctx = Ctx::new();
    // The listener: the netstack serves IPv4 on all our addresses; only the
    // port of `listen` matters (0 = pick a free one, like binding port 0).
    let (listen_port, local_addr) = match cfg.listen {
        Some(a) => {
            if a.is_ipv6() {
                return Err("listen: IPv6 is not supported by the LingOS network stack".into());
            }
            let mut port = a.port();
            if port == 0 {
                let mut tries = 0;
                loop {
                    let p = 49152 + (sys::gen_index(16000) as u16);
                    if !ns::tcp_listening(p) {
                        port = p;
                        break;
                    }
                    tries += 1;
                    if tries > 64 {
                        return Err("listen: no free port".into());
                    }
                }
            } else if ns::tcp_listening(port) {
                return Err(format!("listen {a}: address in use (port {port} is already listening)"));
            }
            let _ = ns::tcp_listen_accept(port, 0);
            if !ns::tcp_listening(port) {
                return Err("listen: no free socket (the network stack's socket table is full)".into());
            }
            (Some(port), Some(SocketAddr::new(a.ip(), port)))
        },
        None => (None, None),
    };
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
    let own_pub = cfg.identity.public();
    let sh = Rc::new(Shared {
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
        listen_port,
        local_addr,
        t,
        ctx,
        events: RefCell::new(VecDeque::new()),
        events_open: Cell::new(true),
        peers: RefCell::new(BTreeMap::new()),
        next_id: Cell::new(1),
        pending: RefCell::new(BTreeMap::new()),
        inbound_hs: RefCell::new(BTreeMap::new()),
        self_nonces: SelfNonces::default(),
        self_addrs: RefCell::new(BTreeSet::new()),
        db: RefCell::new(PeerDb::load(&cfg.data_dir)),
        dl: RefCell::new(Dandelion::new()),
        live_threads: Cell::new(0),
        iso: crate::coro::current_isolate(),
        workers: RefCell::new(Vec::new()),
    });
    let mut workers = Vec::new();
    if let Some(port) = listen_port {
        let s = sh.clone();
        let id = spawn_tracked(&sh, "lwp-listen", move || listen_loop(s, port));
        if id == 0 {
            ns::tcp_unlisten(port);
            return Err("start: no memory for the listener".into());
        }
        workers.push(id);
    }
    let s = sh.clone();
    let id = spawn_tracked(&sh, "lwp-manager", move || manager_loop(s));
    if id == 0 {
        sh.abort();
        return Err("start: no memory for the connection manager".into());
    }
    workers.push(id);
    *sh.workers.borrow_mut() = workers;
    Ok(sh)
}

impl Shared {
    fn emit(&self, ev: NetEvent) {
        loop {
            if !self.events_open.get() {
                return;
            }
            {
                let mut q = self.events.borrow_mut();
                if q.len() < EVENT_QUEUE {
                    q.push_back(ev);
                    return;
                }
            }
            // a full queue back-pressures this peer until the node polls
            if self.down() {
                return;
            }
            sys::sleep_ms(20);
        }
    }

    pub fn poll(&self, max: usize, timeout_ms: u64) -> Vec<NetEvent> {
        let mut out = Vec::new();
        if timeout_ms > 0 {
            let until = sys::now_ms() + timeout_ms;
            while self.events.borrow().is_empty() && self.events_open.get() && sys::now_ms() < until {
                sys::sleep_ms(5);
            }
        }
        let mut q = self.events.borrow_mut();
        while out.len() < max.max(1) {
            match q.pop_front() {
                Some(ev) => out.push(ev),
                None => break,
            }
        }
        out
    }

    fn down(&self) -> bool {
        self.ctx.stop.get()
    }

    pub fn workers_alive(&self) -> bool {
        self.workers.borrow().iter().any(|&id| crate::coro::alive(id))
    }

    fn handle(&self, id: PeerId) -> Option<Rc<PeerHandle>> {
        self.peers.borrow().get(&id).cloned()
    }

    pub fn send(&self, peer: PeerId, kind: u8, payload: Vec<u8>) {
        if kind < APP_KIND_MIN || payload.len() > MAX_PAYLOAD {
            return;
        }
        if let Some(h) = self.handle(peer) {
            h.enqueue(kind, Rc::new(payload), self.t.max_queue_bytes);
        }
    }

    fn broadcast_rc(&self, kind: u8, payload: Rc<Vec<u8>>, except: Option<PeerId>) {
        let hs: Vec<Rc<PeerHandle>> = self.peers.borrow().values().cloned().collect();
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
        self.broadcast_rc(kind, Rc::new(payload), except);
    }

    pub fn relay_tx(&self, tx_id: [u8; 32], payload: Vec<u8>, fluff_kind: u8, from: Option<PeerId>) {
        if fluff_kind < APP_KIND_MIN || payload.is_empty() || payload.len() > MAX_PAYLOAD {
            return;
        }
        let outbound: Vec<PeerId> = self.peers.borrow().values().filter(|h| !h.inbound).map(|h| h.id).collect();
        let payload = Rc::new(payload);
        let decision = self.dl.borrow_mut().decide(tx_id, from, &outbound, self.t.epoch, self.t.stem_probability);
        match decision {
            Decision::Drop => {},
            Decision::Fluff => self.broadcast_rc(fluff_kind, payload, from),
            Decision::Stem(p) => {
                let sent = self.handle(p).map(|h| h.enqueue(K_STEM, payload.clone(), self.t.max_queue_bytes)).unwrap_or(false);
                let e = Embargoed {
                    deadline: sys::now_ms() + rand_between(self.t.embargo_min, self.t.embargo_max),
                    payload: payload.clone(),
                    kind: fluff_kind,
                };
                if !sent || !self.dl.borrow_mut().start_embargo(tx_id, e) {
                    self.broadcast_rc(fluff_kind, payload, from);
                }
            },
        }
    }

    pub fn mark_fluffed(&self, tx_id: [u8; 32]) {
        self.dl.borrow_mut().mark_fluffed(tx_id);
    }

    pub fn peers(&self) -> Vec<PeerInfo> {
        let mut v: Vec<PeerInfo> = self.peers.borrow().values().map(|h| h.info()).collect();
        v.sort_by_key(|p| p.id);
        v
    }

    pub fn peer_netgroup(&self, id: PeerId) -> Option<String> {
        self.handle(id).map(|h| h.group.clone())
    }

    pub fn misbehaving(&self, peer: PeerId, score: u32, _reason: &str) {
        let Some(h) = self.handle(peer) else { return };
        let total = h.score.get().saturating_add(score);
        h.score.set(total);
        if total >= 100 {
            {
                let mut db = self.db.borrow_mut();
                let d = self.t.ban_duration / 1000;
                db.ban(h.id_key.clone(), d);
                if let Some(ip) = h.remote_ip {
                    // Only routable IPs are banned by address (Tor inbound
                    // arrives from 127.0.0.1).
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

    fn close_all_peers(&self) {
        let hs: Vec<Rc<PeerHandle>> = self.peers.borrow().values().cloned().collect();
        for h in hs {
            h.close();
        }
    }

    pub fn shutdown(&self) {
        if self.ctx.stop.replace(true) {
            return;
        }
        self.close_all_peers();
        // Every wait checks the stop flag, so the coroutines finish on their
        // next turn; bounded like the host's join.
        let until = sys::now_ms() + self.t.handshake_timeout + 1000;
        while self.live_threads.get() > 0 && sys::now_ms() < until {
            self.close_all_peers();
            sys::sleep_ms(20);
        }
        self.release();
    }

    /// Final teardown without waiting: free sockets, stop listening, save.
    fn release(&self) {
        self.events_open.set(false);
        if let Ok(mut q) = self.events.try_borrow_mut() {
            q.clear();
        }
        self.ctx.free_all();
        if let Some(p) = self.listen_port {
            ns::tcp_unlisten(p);
        }
        if let Ok(db) = self.db.try_borrow() {
            db.save();
        }
        if let Ok(mut p) = self.peers.try_borrow_mut() {
            p.clear();
        }
    }

    pub fn abort(&self) {
        let first = !self.ctx.stop.replace(true);
        if first || self.events_open.get() {
            self.close_all_peers_quiet();
            self.release();
        }
    }

    fn close_all_peers_quiet(&self) {
        if let Ok(p) = self.peers.try_borrow() {
            for h in p.values() {
                h.close();
            }
        }
    }

    fn is_banned(&self, keys: &[String]) -> bool {
        let db = self.db.borrow();
        keys.iter().any(|k| db.is_banned(k))
    }
}

// ───────────────────────── listener / inbound ─────────────────────────

fn listen_loop(sh: Rc<Shared>, port: u16) {
    while !sh.down() {
        match ns::tcp_listen_accept(port, 0) {
            Some(h) => accept_one(&sh, h),
            None => sys::sleep_ms(25),
        }
    }
    ns::tcp_unlisten(port);
}

fn accept_one(sh: &Rc<Shared>, h: usize) {
    let conn = Conn::new(h, &sh.ctx);
    let Some((rip, rport)) = ns::tcp_remote_h(h) else {
        conn.abort();
        return;
    };
    let ip = canon_ip(IpAddr::V4(Ipv4Addr::from(rip)));
    let peer = SocketAddr::new(ip, rport);
    if ip_is_routable(ip) && sh.is_banned(&[format!("ip:{ip}")]) {
        conn.abort();
        return;
    }
    {
        let peers = sh.peers.borrow();
        let mut hs = sh.inbound_hs.borrow_mut();
        let hs_total: usize = hs.values().sum();
        let inbound = peers.values().filter(|h| h.inbound).count();
        if inbound + hs_total >= sh.max_in || hs_total >= sh.t.max_inbound_handshakes {
            drop(hs);
            drop(peers);
            conn.abort();
            return;
        }
        // Per-IP limit. Loopback is exempt (Tor, single-machine tests).
        if !ip.is_loopback() {
            let same = peers.values().filter(|h| h.inbound && h.remote_ip == Some(ip)).count() + hs.get(&ip).copied().unwrap_or(0);
            if same >= sh.t.per_ip_inbound {
                drop(hs);
                drop(peers);
                conn.abort();
                return;
            }
        }
        *hs.entry(ip).or_insert(0) += 1;
    }
    let sh2 = sh.clone();
    let id = spawn_tracked(sh, "lwp-inbound", move || {
        let mut conn = conn;
        let res = inbound_handshake(&sh2, &mut conn);
        {
            let mut hs = sh2.inbound_hs.borrow_mut();
            if let Some(c) = hs.get_mut(&ip) {
                *c -= 1;
                if *c == 0 {
                    hs.remove(&ip);
                }
            }
        }
        match res {
            Some(sess) => {
                let meta = Meta {
                    addr: peer.to_string(),
                    inbound: true,
                    is_seed: false,
                    is_manual: false,
                    group: ip_group(ip, peer.port()),
                    remote_ip: Some(ip),
                    dial_addr: None,
                };
                register(&sh2, conn, sess, meta);
            },
            None => conn.close(),
        }
    });
    if id == 0 {
        // no coroutine (the closure, and the connection with it, is gone):
        // give the handshake slot back
        let mut hs = sh.inbound_hs.borrow_mut();
        if let Some(c) = hs.get_mut(&ip) {
            *c -= 1;
            if *c == 0 {
                hs.remove(&ip);
            }
        }
    }
}

fn inbound_handshake(sh: &Rc<Shared>, s: &mut Conn) -> Option<Session> {
    let p = Params {
        magic: sh.magic,
        network: sh.network,
        deadline: sys::now_ms() + sh.t.handshake_timeout,
        self_nonces: &sh.self_nonces,
        own_identity: &sh.own_pub,
    };
    let sess = handshake::respond(s, &p, &sh.identity).ok()?;
    if sh.down() || sh.is_banned(&[id_key(&sess.peer)]) {
        return None;
    }
    let inbound = sh.peers.borrow().values().filter(|h| h.inbound).count();
    if inbound >= sh.max_in {
        return None;
    }
    Some(sess)
}

// ───────────────────────────── outbound ──────────────────────────────

fn connect_direct(a: &NetAddr, timeout_ms: u64, ctx: &Ctx) -> Result<Conn, IoError> {
    let ip: [u8; 4] = match &a.host {
        Host::Ip(IpAddr::V4(v4)) => v4.octets(),
        Host::Ip(IpAddr::V6(_)) => return Err(IoError::Other("IPv6 is not supported by the LingOS network stack")),
        // Direct mode only: resolve here (with a proxy we never get here).
        Host::Name(n) => ns::dns_resolve(n).ok_or(IoError::Other("name not resolved"))?,
        Host::Onion(_) => return Err(IoError::Other("onion requires a proxy")),
    };
    connect_ip(ip, a.port, timeout_ms, ctx)
}

fn start_dial(sh: &Rc<Shared>, key: String, addr: NetAddr, kind: DialKind) {
    let counts = !kind.manual;
    sh.pending.borrow_mut().insert(key.clone(), (addr.group(), counts));
    let sh2 = sh.clone();
    let key2 = key.clone();
    let id = spawn_tracked(sh, "lwp-dial", move || {
        crate::console_write(alloc::format!("p2p: dialing {key}\n").as_bytes());
        let res = outbound_handshake(&sh2, &addr, &kind);
        sh2.pending.borrow_mut().remove(&key);
        match res {
            Err(HsError::SelfConnect) => {
                sh2.self_addrs.borrow_mut().insert(key);
            },
            Err(e) => {
                crate::console_write(alloc::format!("p2p: {key}: {e:?}\n").as_bytes());
                let (b, m) = (sh2.t.backoff_base, sh2.t.backoff_max);
                sh2.db.borrow_mut().on_failure(&key, b, m);
            },
            Ok((s, sess)) => {
                let remote_ip = if sh2.proxy.is_none() {
                    ns::tcp_remote_h(s.h).map(|(ip, _)| canon_ip(IpAddr::V4(Ipv4Addr::from(ip))))
                } else {
                    None
                };
                let group = match remote_ip {
                    Some(ip) => ip_group(ip, addr.port),
                    None => addr.group(),
                };
                let ok = admit_outbound(&sh2, &sess, &kind, &group, remote_ip, &key);
                let (b, m) = (sh2.t.backoff_base, sh2.t.backoff_max);
                if !ok {
                    sh2.db.borrow_mut().on_failure(&key, b, m);
                    s.close();
                    return;
                }
                sh2.db.borrow_mut().on_success(&key);
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
            },
        }
    });
    if id == 0 {
        sh.pending.borrow_mut().remove(&key2);
    }
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
    let peers = sh.peers.borrow();
    // Never two outbound connections to the same node.
    if peers.values().any(|h| !h.inbound && h.identity == sess.peer) {
        return false;
    }
    if !kind.manual {
        let out: Vec<&Rc<PeerHandle>> = peers.values().filter(|h| !h.inbound && !h.is_manual).collect();
        if out.len() >= sh.max_out || out.iter().any(|h| h.group == group) {
            return false;
        }
    }
    true
}

fn outbound_handshake(sh: &Rc<Shared>, addr: &NetAddr, kind: &DialKind) -> Result<(Conn, Session), HsError> {
    let mut s = match sh.proxy {
        Some(p) => crate::lwp::socks5::connect(p, addr, sh.t.connect_timeout, &sh.ctx)?,
        None => connect_direct(addr, sh.t.connect_timeout, &sh.ctx)?,
    };
    crate::console_write(alloc::format!("p2p: {}: connected, handshake\n", addr.to_string()).as_bytes());
    let res = (|| {
        let eph;
        let me = if sh.ephemeral {
            eph = IdentityKey::generate();
            &eph
        } else {
            &sh.identity
        };
        let deadline = sys::now_ms() + sh.t.handshake_timeout;
        let p = Params { magic: sh.magic, network: sh.network, deadline, self_nonces: &sh.self_nonces, own_identity: &sh.own_pub };
        let mut sess = handshake::initiate(&mut s, &p, me, kind.pin.as_ref())?;
        // Key confirmation + admission: the responder's first frame must be ACCEPT.
        let left = deadline.saturating_sub(sys::now_ms()).max(1);
        let outer = frame::read_frame(&mut s, left, left).map_err(|_| HsError::Proto("no accept"))?;
        match sess.recv.open(&outer) {
            Ok((K_ACCEPT, p)) if p.is_empty() => Ok(sess),
            _ => Err(HsError::Proto("bad accept")),
        }
    })();
    match res {
        Ok(sess) => Ok((s, sess)),
        Err(e) => {
            s.close();
            Err(e)
        },
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

/// Admit a handshaken connection and run it on this coroutine until it ends.
fn register(sh: &Rc<Shared>, s: Conn, sess: Session, m: Meta) {
    let id = sh.next_id.get();
    sh.next_id.set(id + 1);
    let Session { send, recv, peer } = sess;
    let h = Rc::new(PeerHandle {
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
        connected: sys::now_ms(),
        bytes_in: Cell::new(0),
        bytes_out: Cell::new(0),
        tx: RefCell::new(VecDeque::new()),
        queued: Cell::new(0),
        closed: Cell::new(false),
        score: Cell::new(0),
        getaddr_answered: Cell::new(false),
        addr_pending: Cell::new(false),
        announced: Cell::new(false),
        stem_bucket: Cell::new((STEM_BURST, sys::now_ms())),
    });
    if sh.down() {
        s.abort();
        return;
    }
    sh.peers.borrow_mut().insert(id, h.clone());
    sh.emit(NetEvent::Connected(h.info()));
    if !h.inbound {
        h.addr_pending.set(true);
        h.enqueue(K_GETADDR, Rc::new(Vec::new()), sh.t.max_queue_bytes);
        if let Some(port) = sh.announce_port {
            h.enqueue(K_ANNOUNCE, Rc::new(port.to_le_bytes().to_vec()), sh.t.max_queue_bytes);
        }
    }
    connection_loop(sh, &h, s, send, recv);
}

fn send_frame(sh: &Shared, h: &PeerHandle, s: &mut Conn, cs: &mut CipherState, kind: u8, payload: &[u8]) -> Result<(), IoError> {
    let limit = sh.t.rekey_frames.max(2);
    let deadline = sys::now_ms() + sh.t.write_timeout;
    if cs.since_rekey + 2 > limit || cs.key_age_ms() >= sh.t.rekey_interval {
        let f = cs.seal(K_REKEY, &[]);
        s.write_all_deadline(&f, deadline)?;
        h.bytes_out.set(h.bytes_out.get() + f.len() as u64);
        cs.ratchet();
    }
    let f = cs.seal(kind, payload);
    s.write_all_deadline(&f, deadline)?;
    h.bytes_out.set(h.bytes_out.get() + f.len() as u64);
    Ok(())
}

const STEM_RATE: f64 = 10.0;
const STEM_BURST: f64 = 50.0;

enum Flow {
    Continue,
    Stop,
}

/// One received frame: the host's reader-thread body.
fn on_frame(sh: &Rc<Shared>, h: &Rc<PeerHandle>, cs: &mut CipherState, outer: &[u8]) -> Flow {
    let limit = sh.t.rekey_frames.max(2);
    let max_key_age = sh.t.rekey_interval * 2 + sh.t.dead_timeout;
    // Any authentication/order failure is fatal (no ban: an on-path
    // attacker could otherwise get honest peers banned).
    let Ok((kind, payload)) = cs.open(outer) else { return Flow::Stop };
    h.bytes_in.set(h.bytes_in.get() + outer.len() as u64 + 4);
    if kind == K_REKEY {
        if !payload.is_empty() {
            return Flow::Stop;
        }
        cs.ratchet();
        return Flow::Continue;
    }
    if cs.since_rekey >= limit || cs.key_age_ms() > max_key_age {
        return Flow::Stop; // peer failed to rekey on schedule
    }
    match kind {
        K_PADDING | K_PONG => {},
        K_PING => {
            if payload.len() == 8 {
                h.enqueue(K_PONG, Rc::new(payload), sh.t.max_queue_bytes);
            } else {
                sh.misbehaving(h.id, 10, "bad ping");
            }
        },
        K_GETADDR => {
            // Answer only inbound peers, once per connection.
            if h.inbound && payload.is_empty() && !h.getaddr_answered.replace(true) {
                let sample = sh.db.borrow().gossip_sample(MAX_ADDR_PER_MSG, sh.t.allow_local_gossip);
                h.enqueue(K_ADDR, Rc::new(encode_addr(&sample)), sh.t.max_queue_bytes);
            }
        },
        K_ADDR => {
            if !h.addr_pending.replace(false) {
                sh.misbehaving(h.id, 5, "unsolicited addr");
            } else {
                match decode_addr(&payload) {
                    Some(list) => {
                        let mut db = sh.db.borrow_mut();
                        for a in list {
                            if gossip_acceptable(sh, &a) {
                                db.add(a);
                            }
                        }
                    },
                    None => sh.misbehaving(h.id, 20, "bad addr"),
                }
            }
        },
        K_ANNOUNCE => {
            if h.inbound && payload.len() == 2 && !h.announced.replace(true) {
                let port = u16::from_le_bytes([payload[0], payload[1]]);
                if let (Some(ip), true) = (h.remote_ip, port != 0) {
                    let a = NetAddr { host: Host::Ip(ip), port };
                    if gossip_acceptable(sh, &a) {
                        sh.db.borrow_mut().add(a);
                    }
                }
            }
        },
        K_STEM => {
            let allowed = {
                let mut b = h.stem_bucket.get();
                let now = sys::now_ms();
                b.0 = (b.0 + now.saturating_sub(b.1) as f64 / 1000.0 * STEM_RATE).min(STEM_BURST);
                b.1 = now;
                let ok = if b.0 >= 1.0 {
                    b.0 -= 1.0;
                    true
                } else {
                    false
                };
                h.stem_bucket.set(b);
                ok
            };
            if allowed && !payload.is_empty() {
                sh.emit(NetEvent::StemTx { peer: h.id, payload });
            }
        },
        K_ACCEPT => return Flow::Stop, // only valid as the very first frame
        k if k < APP_KIND_MIN => sh.misbehaving(h.id, 10, "unknown control kind"),
        k => sh.emit(NetEvent::Message { peer: h.id, kind: k, payload }),
    }
    Flow::Continue
}

/// The host's writer + reader threads for one connection, interleaved on
/// one coroutine: send queued frames, timers (ping, cover traffic), then
/// whatever complete frames have arrived; yield when idle.
fn connection_loop(sh: &Rc<Shared>, h: &Rc<PeerHandle>, mut s: Conn, mut send: CipherState, mut recv: CipherState) {
    let mut alive = true;
    if h.inbound && send_frame(sh, h, &mut s, &mut send, K_ACCEPT, &[]).is_err() {
        alive = false;
    }
    let mut next_cover = sys::now_ms() + rand_between(sh.t.cover_min, sh.t.cover_max);
    let mut next_ping = sys::now_ms() + sh.t.ping_interval;
    // when the last frame finished (the host's `idle` wait for the next
    // length prefix), and when the current frame's prefix arrived
    let mut last_frame = sys::now_ms();
    let mut frame_since: Option<u64> = None;
    'conn: while alive {
        if sh.down() || h.closed.get() {
            break;
        }
        let mut busy = false;
        // ── writer ──
        for _ in 0..FRAMES_PER_TURN {
            let next = h.tx.borrow_mut().pop_front();
            let Some((k, p)) = next else { break };
            h.queued.set(h.queued.get().saturating_sub(p.len()));
            if send_frame(sh, h, &mut s, &mut send, k, &p).is_err() {
                break 'conn;
            }
            next_cover = sys::now_ms() + rand_between(sh.t.cover_min, sh.t.cover_max);
            busy = true;
        }
        let now = sys::now_ms();
        if now >= next_ping || now >= next_cover {
            if now >= next_ping {
                let mut n = [0u8; 8];
                kdf::random_bytes(&mut n);
                if send_frame(sh, h, &mut s, &mut send, K_PING, &n).is_err() {
                    break;
                }
                next_ping = now + sh.t.ping_interval;
            } else {
                // Cover traffic: a padding frame in the 1 KiB or 4 KiB bucket.
                let n = sys::gen_index(3000);
                let mut pad = vec![0u8; n];
                kdf::random_bytes(&mut pad);
                if send_frame(sh, h, &mut s, &mut send, K_PADDING, &pad).is_err() {
                    break;
                }
            }
            next_cover = sys::now_ms() + rand_between(sh.t.cover_min, sh.t.cover_max);
            busy = true;
        }
        // ── reader ──
        if s.pull() > 0 {
            busy = true;
        }
        loop {
            if h.closed.get() || sh.down() {
                break 'conn;
            }
            match s.try_frame() {
                Ok(Some(outer)) => {
                    last_frame = sys::now_ms();
                    frame_since = None;
                    busy = true;
                    if let Flow::Stop = on_frame(sh, h, &mut recv, &outer) {
                        break 'conn;
                    }
                },
                Ok(None) => break,
                Err(()) => break 'conn, // a length no frame can have
            }
        }
        let now = sys::now_ms();
        if s.frame_started() {
            let since = *frame_since.get_or_insert(now);
            if now.saturating_sub(since) > sh.t.frame_timeout {
                break;
            }
        } else if now.saturating_sub(last_frame) > sh.t.dead_timeout {
            break;
        }
        // every complete frame is handled above: whatever is left after the
        // peer closed can never complete (the host's read_frame EOF)
        if s.peer_closed() {
            break;
        }
        if busy {
            sys::yield_now();
        } else {
            sys::sleep_ms(5);
        }
    }
    sh.peers.borrow_mut().remove(&h.id);
    h.close();
    if sh.down() {
        s.abort();
    } else {
        s.close();
    }
    sh.emit(NetEvent::Disconnected(h.id));
}

fn gossip_acceptable(sh: &Shared, a: &NetAddr) -> bool {
    match a.host {
        Host::Ip(ip) => sh.t.allow_local_gossip || ip_is_routable(ip),
        Host::Onion(_) => false, // never learned or relayed via gossip
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
        out.push(NetAddr::parse(core::str::from_utf8(r.take(l)?).ok()?)?);
    }
    r.done().then_some(out)
}

// ───────────────────────────── manager ──────────────────────────────

fn manager_loop(sh: Rc<Shared>) {
    let mut last_save = sys::now_ms();
    while !sh.down() {
        let until = sys::now_ms() + sh.t.tick;
        while sys::now_ms() < until && !sh.down() {
            sys::sleep_ms(20.min(sh.t.tick));
        }
        if sh.down() {
            break;
        }
        let expired = sh.dl.borrow_mut().expired(sys::now_ms());
        for e in expired {
            sh.broadcast_rc(e.kind, e.payload, None);
        }
        fill_outbound(&sh);
        if sys::now_ms().saturating_sub(last_save) >= sh.t.save_interval {
            update_anchors(&sh);
            sh.db.borrow().save();
            last_save = sys::now_ms();
        }
    }
    update_anchors(&sh);
    sh.db.borrow().save();
}

fn update_anchors(sh: &Shared) {
    let now = sys::now_ms();
    let mut out: Vec<(u64, String)> = sh
        .peers
        .borrow()
        .values()
        .filter(|h| !h.inbound && !h.is_manual && now.saturating_sub(h.connected) >= sh.t.anchor_min_age)
        .filter_map(|h| h.dial_addr.clone().map(|a| (now.saturating_sub(h.connected), a)))
        .collect();
    if out.is_empty() {
        return;
    }
    out.sort_by_key(|x| core::cmp::Reverse(x.0));
    sh.db.borrow_mut().anchors = out.into_iter().take(MAX_ANCHORS).map(|x| x.1).collect();
}

fn fill_outbound(sh: &Rc<Shared>) {
    let now = sys::now_ms();
    let (connected_addrs, mut used_groups, mut n_out) = {
        let peers = sh.peers.borrow();
        let addrs: BTreeSet<String> = peers.values().filter_map(|h| h.dial_addr.clone()).collect();
        let out: Vec<&Rc<PeerHandle>> = peers.values().filter(|h| !h.inbound && !h.is_manual).collect();
        (addrs, out.iter().map(|h| h.group.clone()).collect::<BTreeSet<String>>(), out.len())
    };
    let pending = sh.pending.borrow().clone();
    for (g, counts) in pending.values() {
        if *counts {
            used_groups.insert(g.clone());
            n_out += 1;
        }
    }
    let self_addrs = sh.self_addrs.borrow().clone();
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
        let db = sh.db.borrow();
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
        let db = sh.db.borrow();
        let mut c: Vec<(String, NetAddr, DialKind)> = Vec::new();
        let kind_for = |k: &str| DialKind { pin: sh.seeds.iter().find(|s| s.key == k).map(|s| s.identity.clone()), manual: false };
        for a in &db.anchors {
            if let Some(na) = NetAddr::parse(a) {
                c.push((a.clone(), na, kind_for(a)));
            }
        }
        if db.entries.len() < THIN_DB || n_out == 0 {
            let mut seeds: Vec<&SeedEntry> = sh.seeds.iter().collect();
            sys::shuffle(&mut seeds);
            for s in seeds {
                c.push((s.key.clone(), s.addr.clone(), DialKind { pin: Some(s.identity.clone()), manual: false }));
            }
        }
        let mut entries: Vec<_> = db.entries.values().collect();
        sys::shuffle(&mut entries);
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
    let mut tried: BTreeSet<String> = BTreeSet::new();
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
