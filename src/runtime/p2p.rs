//! ling-wallet runtime additions: the `p2p_*` builtins (LING_API.md §4),
//! backed by `ling_net::lwp` (the LWP-1 transport, ling-wallet DESIGN §8).
//!
//! Every builtin needs the `"p2p"` capability. A network is identified by an
//! unguessable 128-bit random hex handle that is valid only in the isolate
//! that started it (it lives in this isolate's [`P2pState`]); any other
//! string is a runtime error. After `p2p_shutdown` the handle stays known as
//! a tombstone, so late calls return `false` / `[]` instead of erroring.
//!
//! ## Event delivery
//!
//! LWP-1 produces events on a bounded crossbeam queue ([`NetEvent`]). They
//! reach Ling in one of two ways, chosen at `p2p_start`:
//!
//! * **poll mode** (`events_send_handle` is `""` or `()`): events stay in the
//!   queue and `p2p_poll(net, max[, timeout_ms])` drains them. The program
//!   must keep polling — a full queue back-pressures every peer's reader.
//! * **channel mode** (`events_send_handle` is a Ling channel send handle): a
//!   forwarder thread moves every event into that channel as soon as it
//!   arrives. The ONLY place that knows about channels is
//!   [`Interpreter::p2p_event_sink`] — see the integration note there.
//!
//! Either way the event shapes are those of [`P2pEvent::fields`].

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use ling_net::lwp::crypto::identity::{IdentityKey, IdentityPublic};
use ling_net::lwp::{self, EventReceiver, NetConfig, NetEvent, Network, SeedPeer, SignedSeedList, Tuning};

use super::{hex_encode, EvalErr, EvalResult, Interpreter, Value};

/// Per-isolate state for this builtin family.
#[derive(Default)]
pub(crate) struct P2pState {
    nets: HashMap<String, NetSlot>,
}

struct NetSlot {
    /// `None` once shut down (tombstone).
    net: Option<Network>,
    /// The event queue in poll mode; `None` in channel mode (the forwarder
    /// thread owns the receiver) and after shutdown.
    rx: Option<EventReceiver>,
}

/// A transport event in thread-safe, plain-data form (Send; no `Rc`), so it
/// can cross to a forwarder thread and into a channel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum P2pEvent {
    Connected { peer: u64, addr: String, inbound: bool, fingerprint: String, is_seed: bool },
    Disconnected { peer: u64 },
    Message { peer: u64, kind: u8, payload_hex: String },
    Stem { peer: u64, payload_hex: String },
}

/// One field of an event: everything an event carries is a string, number
/// or bool, so this maps 1:1 onto any frozen/thread-safe value form.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum EvField {
    Str(String),
    Num(f64),
    Bool(bool),
}

impl P2pEvent {
    fn from_net(ev: NetEvent) -> Self {
        match ev {
            NetEvent::Connected(p) => P2pEvent::Connected {
                peer: p.id,
                addr: p.addr,
                inbound: p.inbound,
                fingerprint: p.identity_fingerprint,
                is_seed: p.is_seed,
            },
            NetEvent::Disconnected(peer) => P2pEvent::Disconnected { peer },
            NetEvent::Message { peer, kind, payload } => P2pEvent::Message { peer, kind, payload_hex: hex_encode(&payload) },
            NetEvent::StemTx { peer, payload } => P2pEvent::Stem { peer, payload_hex: hex_encode(&payload) },
        }
    }

    /// The normative event shapes (LING_API.md §4):
    /// `["connected", peer_id, addr, inbound, fingerprint, is_seed]`,
    /// `["disconnected", peer_id]`, `["message", peer_id, kind, payload_hex]`,
    /// `["stem", peer_id, payload_hex]`.
    pub(crate) fn fields(&self) -> Vec<EvField> {
        use EvField::*;
        let s = |x: &str| Str(x.to_string());
        match self {
            P2pEvent::Connected { peer, addr, inbound, fingerprint, is_seed } => {
                vec![s("connected"), Num(*peer as f64), s(addr), Bool(*inbound), s(fingerprint), Bool(*is_seed)]
            },
            P2pEvent::Disconnected { peer } => vec![s("disconnected"), Num(*peer as f64)],
            P2pEvent::Message { peer, kind, payload_hex } => {
                vec![s("message"), Num(*peer as f64), Num(*kind as f64), s(payload_hex)]
            },
            P2pEvent::Stem { peer, payload_hex } => vec![s("stem"), Num(*peer as f64), s(payload_hex)],
        }
    }

    /// The event as a Ling list value (isolate thread only: `Value` uses `Rc`).
    pub(crate) fn to_value(&self) -> Value {
        Value::List(Rc::new(
            self.fields()
                .into_iter()
                .map(|f| match f {
                    EvField::Str(s) => Value::Str(s),
                    EvField::Num(n) => Value::Number(n),
                    EvField::Bool(b) => Value::Bool(b),
                })
                .collect(),
        ))
    }
}

/// Delivers one event into a Ling channel; returns `false` once the channel
/// is closed (the forwarder then stops). Runs on the forwarder thread.
pub(crate) type EventSink = Box<dyn FnMut(P2pEvent) -> bool + Send + 'static>;

// ─── small helpers ───────────────────────────────────────────────────────────

fn ok(v: Value) -> Value {
    Value::Ok(Box::new(v))
}
fn bad(msg: impl Into<String>) -> Value {
    Value::Err(Box::new(Value::Str(msg.into())))
}
fn list(v: Vec<Value>) -> Value {
    Value::List(Rc::new(v))
}

/// Strict hex decode: even length, hex digits only.
fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let nib = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    b.chunks(2).map(|p| Some(nib(p[0])? << 4 | nib(p[1])?)).collect()
}

fn unhex_n<const N: usize>(s: &str) -> Option<[u8; N]> {
    unhex(s)?.try_into().ok()
}

/// An exact non-negative integer ≤ `max` (Ling numbers are f64, so also ≤ 2^53).
fn uint(v: &Value, max: u64) -> Option<u64> {
    match v {
        Value::Number(n) if n.is_finite() && n.fract() == 0.0 && *n >= 0.0 && *n <= max as f64 && *n <= 9_007_199_254_740_992.0 => {
            Some(*n as u64)
        },
        _ => None,
    }
}

/// A peer id, or `None` for `-1` / `()` (the "no peer" sentinel). `Err` = invalid.
fn opt_peer(v: &Value) -> Result<Option<u64>, ()> {
    match v {
        Value::Number(n) if *n == -1.0 => Ok(None),
        Value::Unit => Ok(None),
        _ => uint(v, u64::MAX).filter(|&p| p >= 1).map(Some).ok_or(()),
    }
}

fn as_str(v: &Value) -> Option<&str> {
    match v {
        Value::Str(s) => Some(s),
        _ => None,
    }
}

fn as_bool(v: &Value) -> Option<bool> {
    match v {
        Value::Bool(b) => Some(*b),
        _ => None,
    }
}

/// An application kind (`0x10..=0xFF`; lower kinds are LWP-1 control).
fn app_kind(v: &Value) -> Option<u8> {
    uint(v, 255).filter(|&k| k >= lwp::APP_KIND_MIN as u64).map(|k| k as u8)
}

/// A hex payload within the transport's size limit.
fn payload(v: &Value) -> Option<Vec<u8>> {
    as_str(v).and_then(unhex).filter(|p| p.len() <= lwp::MAX_PAYLOAD)
}

fn new_handle() -> String {
    use rand::RngCore;
    let mut b = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut b);
    hex_encode(&b)
}

/// Parsed `p2p_start` configuration.
struct StartCfg {
    cfg: NetConfig,
    tuning: Tuning,
}

/// Parse and validate the `[key, value]` config list (LING_API.md §4).
fn parse_config(v: &Value) -> Result<StartCfg, String> {
    let items = match v {
        Value::List(l) => l.clone(),
        _ => return Err("config must be a list of [key, value] pairs".into()),
    };
    let mut seen = std::collections::HashSet::new();
    let mut magic: Option<[u8; 4]> = None;
    let mut network: Option<u8> = None;
    let mut listen: Option<SocketAddr> = None;
    let mut data_dir: Option<PathBuf> = None;
    let mut seeds_hex: Option<String> = None;
    let mut authority: Option<IdentityPublic> = None;
    let mut proxy: Option<SocketAddr> = None;
    let mut max_outbound = 8usize;
    let mut max_inbound = 32usize;
    let mut identity: Option<IdentityKey> = None;
    let mut ephemeral = true;
    let mut peers: Vec<String> = Vec::new();
    let mut fast = false;

    for item in items.iter() {
        let pair = match item {
            Value::List(p) if p.len() == 2 => p,
            _ => return Err("config entries must be [key, value] pairs".into()),
        };
        let key = as_str(&pair[0]).ok_or("config key must be a string")?;
        let val = &pair[1];
        if !seen.insert(key.to_string()) {
            return Err(format!("duplicate config key: {key}"));
        }
        let bad_val = |what: &str| format!("config {key}: {what}");
        match key {
            "magic_hex" => {
                magic = Some(as_str(val).and_then(unhex_n::<4>).ok_or_else(|| bad_val("expected 8 hex chars"))?);
            },
            "network" => network = Some(uint(val, 255).ok_or_else(|| bad_val("expected an integer 0..255"))? as u8),
            "listen" => {
                let s = as_str(val).ok_or_else(|| bad_val("expected \"ip:port\" or \"\""))?;
                if !s.is_empty() {
                    listen = Some(s.parse().map_err(|_| bad_val("expected \"ip:port\" (e.g. 0.0.0.0:4747)"))?);
                }
            },
            "data_dir" => {
                let s = as_str(val).filter(|s| !s.is_empty()).ok_or_else(|| bad_val("expected a non-empty path"))?;
                data_dir = Some(PathBuf::from(s));
            },
            "seeds_hex" => seeds_hex = Some(as_str(val).ok_or_else(|| bad_val("expected a hex string"))?.to_string()),
            "authority_pk_hex" => {
                let s = as_str(val).ok_or_else(|| bad_val("expected hex"))?;
                if !s.is_empty() {
                    let b = unhex(s).ok_or_else(|| bad_val("expected hex"))?;
                    authority = Some(IdentityPublic::from_bytes(&b).map_err(|_| bad_val("expected a 2592-byte ML-DSA-87 public key"))?);
                }
            },
            "proxy" => {
                let s = as_str(val).ok_or_else(|| bad_val("expected \"ip:port\" or \"\""))?;
                if !s.is_empty() {
                    proxy = Some(s.parse().map_err(|_| bad_val("expected \"ip:port\" (e.g. 127.0.0.1:9050)"))?);
                }
            },
            "max_outbound" => max_outbound = uint(val, 64).ok_or_else(|| bad_val("expected an integer 0..64"))? as usize,
            "max_inbound" => max_inbound = uint(val, 1024).ok_or_else(|| bad_val("expected an integer 0..1024"))? as usize,
            "identity_seed_hex" => {
                let s = as_str(val).ok_or_else(|| bad_val("expected 64 hex chars"))?;
                if !s.is_empty() {
                    let seed = unhex_n::<32>(s).ok_or_else(|| bad_val("expected 64 hex chars"))?;
                    identity = Some(IdentityKey::from_seed(&seed));
                }
            },
            "ephemeral_outbound" => ephemeral = as_bool(val).ok_or_else(|| bad_val("expected a bool"))?,
            "peers" => {
                let l = match val {
                    Value::List(l) => l,
                    _ => return Err(bad_val("expected a list of address strings")),
                };
                for p in l.iter() {
                    let s = as_str(p).filter(|s| lwp::is_valid_addr(s)).ok_or_else(|| bad_val(&format!("invalid address {p}")))?;
                    peers.push(s.to_string());
                }
            },
            "fast_timers" => fast = as_bool(val).ok_or_else(|| bad_val("expected a bool"))?,
            _ => return Err(format!("unknown config key: {key}")),
        }
    }

    let network_magic = magic.ok_or("config: magic_hex is required")?;
    let network = network.ok_or("config: network is required")?;
    let data_dir = data_dir.ok_or("config: data_dir is required")?;
    let seeds = match (seeds_hex.as_deref(), &authority) {
        (None | Some(""), _) => Vec::new(),
        (Some(_), None) => return Err("config: seeds_hex needs authority_pk_hex".into()),
        (Some(h), Some(auth)) => {
            let list = unhex(h).and_then(|b| SignedSeedList::from_bytes(&b)).ok_or("config seeds_hex: malformed seed list")?;
            let seeds = list.verify(auth).ok_or("config seeds_hex: bad signature")?.to_vec();
            if list.network != network {
                return Err("config seeds_hex: seed list is for another network".into());
            }
            seeds
        },
    };
    if peers.iter().any(|p| p.to_ascii_lowercase().contains(".onion:")) && proxy.is_none() {
        return Err("config peers: onion addresses need a proxy".into());
    }
    Ok(StartCfg {
        cfg: NetConfig {
            network_magic,
            network,
            listen,
            data_dir,
            seeds,
            socks5_proxy: proxy,
            max_outbound,
            max_inbound,
            identity: identity.unwrap_or_else(IdentityKey::generate),
            ephemeral_outbound_identity: ephemeral,
            manual_peers: peers,
        },
        tuning: if fast { Tuning::fast_for_tests() } else { Tuning::default() },
    })
}

impl Interpreter {
    /// **Channel integration point** (the only code that knows about Ling
    /// channels). Resolve `events_send_handle` to an [`EventSink`];
    /// `Ok(None)` selects poll mode.
    ///
    /// TODO(threads integration): once `threads.rs` lands, look the handle up
    /// in `self.threads_state` (it must be a *send* handle of this isolate,
    /// else a runtime error as for any bad channel handle), clone its
    /// thread-safe sender, and return
    /// `Some(Box::new(move |ev| sender.send(frozen_list(ev.fields())).is_ok()))`
    /// where `frozen_list` builds the channel's frozen list of
    /// strings/numbers/bools from [`P2pEvent::fields`]. A blocking send is
    /// intended: a full channel back-pressures the network exactly like an
    /// undrained poll queue. Nothing else in this file needs to change.
    fn p2p_event_sink(&mut self, events_handle: &Value) -> Result<Option<EventSink>, EvalErr> {
        match events_handle {
            Value::Unit => Ok(None),
            Value::Str(s) if s.is_empty() => Ok(None),
            other => Err(EvalErr::Runtime(format!(
                "p2p_start: {other} is not a channel send handle (channel delivery is not wired yet: pass \"\" and use p2p_poll)"
            ))),
        }
    }

    /// Look up a network handle. Unknown handle ⇒ runtime error (handles are
    /// capabilities); `Ok(None)` ⇒ it was shut down.
    fn p2p_net(&self, h: &Value) -> Result<Option<&Network>, EvalErr> {
        let key = as_str(h).ok_or_else(|| EvalErr::Runtime(format!("not a p2p network handle: {h}")))?;
        match self.p2p_state.nets.get(key) {
            Some(slot) => Ok(slot.net.as_ref()),
            None => Err(EvalErr::Runtime(format!("unknown p2p network handle: {key}"))),
        }
    }

    pub(super) fn p2p_builtin(&mut self, name: &str, args: &[Value]) -> Option<EvalResult> {
        match name {
            "p2p_start" | "p2p_poll" | "p2p_send" | "p2p_broadcast" | "p2p_relay" | "p2p_mark_fluffed" | "p2p_peers"
            | "p2p_misbehaving" | "p2p_seeds_sign" | "p2p_seeds_verify" | "p2p_shutdown" | "p2p_local_addr"
            | "p2p_netgroup" | "p2p_fingerprint" => Some(self.require_cap("p2p").and_then(|_| self.p2p_call(name, args))),
            _ => None,
        }
    }

    fn p2p_call(&mut self, name: &str, args: &[Value]) -> EvalResult {
        let unit = Value::Unit;
        let arg = |i: usize| args.get(i).unwrap_or(&unit);
        let no = Value::Bool(false);
        Ok(match name {
            "p2p_start" => {
                let sc = match parse_config(arg(0)) {
                    Ok(c) => c,
                    Err(e) => return Ok(bad(e)),
                };
                let sink = self.p2p_event_sink(arg(1))?;
                if let Err(e) = std::fs::create_dir_all(&sc.cfg.data_dir) {
                    return Ok(bad(format!("data_dir: {e}")));
                }
                let (net, rx) = match Network::start_tuned(sc.cfg, sc.tuning) {
                    Ok(x) => x,
                    Err(e) => return Ok(bad(format!("start: {e}"))),
                };
                let rx = match sink {
                    None => Some(rx),
                    Some(mut sink) => {
                        // Forwarder: ends when the network shuts down (the
                        // queue closes) or the channel is closed.
                        let spawned = std::thread::Builder::new().name("p2p-events".into()).spawn(move || {
                            for ev in rx.iter() {
                                if !sink(P2pEvent::from_net(ev)) {
                                    break;
                                }
                            }
                        });
                        if let Err(e) = spawned {
                            net.shutdown();
                            return Ok(bad(format!("start: {e}")));
                        }
                        None
                    },
                };
                let h = new_handle();
                self.p2p_state.nets.insert(h.clone(), NetSlot { net: Some(net), rx });
                ok(Value::Str(h))
            },
            "p2p_poll" => {
                self.p2p_net(arg(0))?;
                let max = uint(arg(1), 1 << 20).unwrap_or(64).max(1) as usize;
                let timeout_ms = uint(arg(2), 3_600_000).unwrap_or(0);
                let mut out = Vec::new();
                let slot = as_str(arg(0)).and_then(|k| self.p2p_state.nets.get(k));
                if let Some(rx) = slot.and_then(|s| s.rx.as_ref()) {
                    if timeout_ms > 0 {
                        if let Ok(ev) = rx.recv_timeout(Duration::from_millis(timeout_ms)) {
                            out.push(P2pEvent::from_net(ev).to_value());
                        }
                    }
                    while out.len() < max {
                        match rx.try_recv() {
                            Ok(ev) => out.push(P2pEvent::from_net(ev).to_value()),
                            Err(_) => break,
                        }
                    }
                }
                list(out)
            },
            "p2p_send" => {
                let Some(net) = self.p2p_net(arg(0))? else { return Ok(no) };
                let (Some(peer), Some(kind), Some(payload)) =
                    (uint(arg(1), u64::MAX).filter(|&p| p >= 1), app_kind(arg(2)), payload(arg(3)))
                else {
                    return Ok(no);
                };
                if !net.peers().iter().any(|p| p.id == peer) {
                    return Ok(no);
                }
                net.send(peer, kind, payload);
                Value::Bool(true)
            },
            "p2p_broadcast" => {
                let Some(net) = self.p2p_net(arg(0))? else { return Ok(no) };
                let (Some(kind), Some(payload), Ok(except)) = (app_kind(arg(1)), payload(arg(2)), opt_peer(arg(3))) else {
                    return Ok(no);
                };
                net.broadcast(kind, payload, except);
                Value::Bool(true)
            },
            "p2p_relay" => {
                let Some(net) = self.p2p_net(arg(0))? else { return Ok(no) };
                let (Some(id), Some(payload), Some(kind), Ok(from)) =
                    (as_str(arg(1)).and_then(unhex_n::<32>), payload(arg(2)), app_kind(arg(3)), opt_peer(arg(4)))
                else {
                    return Ok(no);
                };
                net.relay_tx(id, payload, kind, from);
                Value::Bool(true)
            },
            "p2p_mark_fluffed" => {
                let Some(net) = self.p2p_net(arg(0))? else { return Ok(no) };
                let Some(id) = as_str(arg(1)).and_then(unhex_n::<32>) else { return Ok(no) };
                net.mark_fluffed(id);
                Value::Bool(true)
            },
            "p2p_peers" => {
                let Some(net) = self.p2p_net(arg(0))? else { return Ok(list(Vec::new())) };
                let mut peers = net.peers();
                peers.sort_by_key(|p| p.id);
                list(
                    peers
                        .into_iter()
                        .map(|p| {
                            list(vec![
                                Value::Number(p.id as f64),
                                Value::Str(p.addr),
                                Value::Bool(p.inbound),
                                Value::Str(p.identity_fingerprint),
                                Value::Bool(p.is_seed),
                                Value::Number(p.connected_secs as f64),
                            ])
                        })
                        .collect(),
                )
            },
            "p2p_misbehaving" => {
                let Some(net) = self.p2p_net(arg(0))? else { return Ok(no) };
                let (Some(peer), Some(score)) = (uint(arg(1), u64::MAX).filter(|&p| p >= 1), uint(arg(2), u32::MAX as u64)) else {
                    return Ok(no);
                };
                let reason = match arg(3) {
                    Value::Unit => String::new(),
                    v => v.to_string(),
                };
                net.misbehaving(peer, score as u32, &reason);
                Value::Bool(true)
            },
            "p2p_local_addr" => match self.p2p_net(arg(0))?.and_then(|n| n.local_addr()) {
                Some(a) => Value::Str(a.to_string()),
                None => Value::Str(String::new()),
            },
            // p2p_fingerprint(identity_seed_hex) -> the fingerprint peers will see for that identity
            "p2p_fingerprint" => match as_str(arg(0)).and_then(unhex_n::<32>) {
                Some(seed) => Value::Str(IdentityKey::from_seed(&seed).public().fingerprint()),
                None => Value::Str(String::new()),
            },
            "p2p_netgroup" => {
                let Some(net) = self.p2p_net(arg(0))? else { return Ok(Value::Str(String::new())) };
                let g = uint(arg(1), u64::MAX).and_then(|p| net.peer_netgroup(p));
                Value::Str(g.unwrap_or_default())
            },
            "p2p_seeds_sign" => {
                let fail = |m: &str| EvalErr::Runtime(format!("p2p_seeds_sign: {m}"));
                let network = uint(arg(0), 255).ok_or_else(|| fail("network must be an integer 0..255"))? as u8;
                let issued_at = uint(arg(1), u64::MAX).ok_or_else(|| fail("issued_at must be a non-negative integer"))?;
                let entries = match arg(2) {
                    Value::List(l) => l.clone(),
                    _ => return Err(fail("entries must be a list of [addr, identity_pk_hex]")),
                };
                if entries.len() > lwp::MAX_SEEDS {
                    return Err(fail("too many seeds"));
                }
                let mut seeds = Vec::with_capacity(entries.len());
                for e in entries.iter() {
                    let (addr, pk) = match e {
                        Value::List(p) if p.len() == 2 => (as_str(&p[0]), as_str(&p[1])),
                        _ => return Err(fail("each entry must be [addr, identity_pk_hex]")),
                    };
                    let addr = addr.filter(|a| lwp::is_valid_addr(a)).ok_or_else(|| fail("invalid seed address"))?;
                    let identity = pk
                        .and_then(unhex)
                        .and_then(|b| IdentityPublic::from_bytes(&b).ok())
                        .ok_or_else(|| fail("invalid identity public key"))?;
                    seeds.push(SeedPeer { addr: addr.to_string(), identity });
                }
                let seed = as_str(arg(3)).and_then(unhex_n::<32>).ok_or_else(|| fail("authority_seed_hex must be 64 hex chars"))?;
                let authority = IdentityKey::from_seed(&seed);
                Value::Str(hex_encode(&SignedSeedList::sign(network, issued_at, seeds, &authority).to_bytes()))
            },
            "p2p_seeds_verify" => {
                let Some(list_bytes) = as_str(arg(0)).and_then(unhex) else { return Ok(bad("malformed seed list")) };
                let Some(auth) = as_str(arg(1)).and_then(unhex).and_then(|b| IdentityPublic::from_bytes(&b).ok()) else {
                    return Ok(bad("malformed authority public key"));
                };
                let Some(sl) = SignedSeedList::from_bytes(&list_bytes) else { return Ok(bad("malformed seed list")) };
                let Some(seeds) = sl.verify(&auth) else { return Ok(bad("bad signature")) };
                let entries = seeds
                    .iter()
                    .map(|s| list(vec![Value::Str(s.addr.clone()), Value::Str(hex_encode(&s.identity.0))]))
                    .collect();
                ok(list(vec![Value::Number(sl.network as f64), Value::Number(sl.issued_at as f64), list(entries)]))
            },
            "p2p_shutdown" => {
                self.p2p_net(arg(0))?;
                let slot = self.p2p_state.nets.get_mut(as_str(arg(0)).unwrap_or("")).expect("handle checked above");
                slot.rx = None;
                match slot.net.take() {
                    Some(net) => {
                        net.shutdown();
                        Value::Bool(true)
                    },
                    None => Value::Bool(false),
                }
            },
            _ => unreachable!("p2p_builtin dispatches only known names"),
        })
    }
}
