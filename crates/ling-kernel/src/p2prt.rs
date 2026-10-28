//! The `p2p_*` builtins (ling-wallet LING_API.md §4) for compiled Ling in the
//! kernel, over [`crate::lwp`] (the LWP-1 transport). A port of the host's
//! `src/runtime/p2p.rs`: same argument parsing, same return values, same
//! event shapes —
//!
//! `["connected", peer_id, addr, inbound, fingerprint, is_seed]`,
//! `["disconnected", peer_id]`, `["message", peer_id, kind, payload_hex]`,
//! `["stem", peer_id, payload_hex]` (plain Ling lists, read with `list_get`).
//!
//! A network is identified by an unguessable 128-bit random hex handle valid
//! only in the isolate that started it. After `p2p_shutdown` the handle stays
//! known as a tombstone, so late calls return `false` / `[]`. Only poll mode
//! exists (the host's channel mode is not wired there either): pass `""` as
//! `p2p_start`'s second argument and drain with `p2p_poll`.
//!
//! Differences forced by the kernel: the host raises a Ling runtime error for
//! an unknown handle or bad `p2p_seeds_sign` input; compiled kernel Ling has
//! no runtime errors, so these print a console line and return the call's
//! "nothing" value (`false`, `[]`, `""`, `()`). The `"p2p"` capability check
//! does not exist in the kernel. When the program that started a network
//! quits, its network is aborted (sockets freed, peer DB saved).

use alloc::format;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::lingrec::{mk_bad_str, mk_ok};
use crate::lingrt::{boolean, is_list, is_num, is_str, list_from, list_items, mk_str, num, str_of, FALSE, TRUE, UNIT};
use crate::lwp::crypto::identity::{IdentityKey, IdentityPublic};
use crate::lwp::{self, NetConfig, NetEvent, Network, SeedPeer, SignedSeedList, Tuning};

struct Slot {
    handle: String,
    iso: u64,
    /// `None` once shut down (tombstone).
    net: Option<Rc<Network>>,
}

static mut NETS: Vec<Slot> = Vec::new();

fn nets() -> &'static mut Vec<Slot> {
    unsafe { &mut *(&raw mut NETS) }
}

/// Coroutines ended (an app quit): abort the networks whose isolate is gone.
fn on_kill() {
    let v = nets();
    let mut i = 0;
    while i < v.len() {
        let gone = match &v[i].net {
            Some(n) => {
                let iso = n.isolate();
                if iso != 0 { !crate::coro::alive(iso) } else { !n.workers_alive() }
            },
            None => v[i].iso != 0 && !crate::coro::alive(v[i].iso),
        };
        if gone {
            let s = v.remove(i);
            if let Some(n) = s.net {
                n.abort();
            }
        } else {
            i += 1;
        }
    }
}

fn hex_encode(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for x in b {
        s.push(D[(x >> 4) as usize] as char);
        s.push(D[(x & 15) as usize] as char);
    }
    s
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

/// An exact non-negative integer ≤ `max` (Ling numbers are f64, so ≤ 2^53).
fn uint(v: u64, max: u64) -> Option<u64> {
    if !is_num(v) {
        return None;
    }
    let n = f64::from_bits(v);
    if n.is_finite() && libm::trunc(n) == n && n >= 0.0 && n <= max as f64 && n <= 9_007_199_254_740_992.0 {
        Some(n as u64)
    } else {
        None
    }
}

/// A peer id, or `None` for `-1` / `()`. `Err` = invalid.
fn opt_peer(v: u64) -> Result<Option<u64>, ()> {
    if v == UNIT || (is_num(v) && f64::from_bits(v) == -1.0) {
        return Ok(None);
    }
    uint(v, u64::MAX).filter(|&p| p >= 1).map(Some).ok_or(())
}

fn as_str(v: u64) -> Option<&'static str> {
    is_str(v).then(|| str_of(v))
}

fn as_bool(v: u64) -> Option<bool> {
    if v == TRUE {
        Some(true)
    } else if v == FALSE {
        Some(false)
    } else {
        None
    }
}

/// An application kind (`0x10..=0xFF`).
fn app_kind(v: u64) -> Option<u8> {
    uint(v, 255).filter(|&k| k >= lwp::APP_KIND_MIN as u64).map(|k| k as u8)
}

/// A hex payload within the transport's size limit.
fn payload(v: u64) -> Option<Vec<u8>> {
    as_str(v).and_then(unhex).filter(|p| p.len() <= lwp::MAX_PAYLOAD)
}

fn new_handle() -> String {
    let mut b = [0u8; 16];
    lwp::sys::random_bytes(&mut b);
    hex_encode(&b)
}

fn display(v: u64) -> String {
    crate::lingrt::to_string(v)
}

struct StartCfg {
    cfg: NetConfig,
    tuning: Tuning,
}

/// Parse and validate the `[key, value]` config list (LING_API.md §4).
fn parse_config(v: u64) -> Result<StartCfg, String> {
    if !is_list(v) {
        return Err("config must be a list of [key, value] pairs".into());
    }
    let mut seen: Vec<String> = Vec::new();
    let mut magic: Option<[u8; 4]> = None;
    let mut network: Option<u8> = None;
    let mut listen: Option<core::net::SocketAddr> = None;
    let mut data_dir: Option<String> = None;
    let mut seeds_hex: Option<String> = None;
    let mut authority: Option<IdentityPublic> = None;
    let mut proxy: Option<core::net::SocketAddr> = None;
    let mut max_outbound = 8usize;
    let mut max_inbound = 32usize;
    let mut identity: Option<IdentityKey> = None;
    let mut ephemeral = true;
    let mut peers: Vec<String> = Vec::new();
    let mut fast = false;

    for &item in list_items(v) {
        let pair = list_items(item);
        if !is_list(item) || pair.len() != 2 {
            return Err("config entries must be [key, value] pairs".into());
        }
        let key = as_str(pair[0]).ok_or("config key must be a string")?;
        let val = pair[1];
        if seen.iter().any(|k| k == key) {
            return Err(format!("duplicate config key: {key}"));
        }
        seen.push(key.to_string());
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
                data_dir = Some(s.to_string());
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
                if !is_list(val) {
                    return Err(bad_val("expected a list of address strings"));
                }
                for &p in list_items(val) {
                    let s = as_str(p).filter(|s| lwp::is_valid_addr(s)).ok_or_else(|| bad_val(&format!("invalid address {}", display(p))))?;
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

/// An event as its Ling list (the host's `P2pEvent::fields`).
fn event_value(ev: NetEvent) -> u64 {
    match ev {
        NetEvent::Connected(p) => {
            let how = if p.inbound { ", inbound" } else if p.is_seed { ", seed" } else { "" };
            crate::console_write(format!("p2p: peer {} connected ({}{how})\n", p.id, p.addr).as_bytes());
            list_from(alloc::vec![
                mk_str("connected"),
                num(p.id as f64),
                mk_str(&p.addr),
                boolean(p.inbound),
                mk_str(&p.identity_fingerprint),
                boolean(p.is_seed),
            ])
        },
        NetEvent::Disconnected(peer) => {
            crate::console_write(format!("p2p: peer {peer} gone\n").as_bytes());
            list_from(alloc::vec![mk_str("disconnected"), num(peer as f64)])
        },
        NetEvent::Message { peer, kind, payload } => {
            list_from(alloc::vec![mk_str("message"), num(peer as f64), num(kind as f64), mk_str(&hex_encode(&payload))])
        },
        NetEvent::StemTx { peer, payload } => list_from(alloc::vec![mk_str("stem"), num(peer as f64), mk_str(&hex_encode(&payload))]),
    }
}

enum Lookup {
    Live(Rc<Network>),
    Shut,
    Unknown,
}

/// Look up a handle of the calling isolate.
fn lookup(h: u64) -> Lookup {
    let Some(key) = as_str(h) else { return Lookup::Unknown };
    let iso = crate::coro::current_isolate();
    match nets().iter().find(|s| s.handle == key && s.iso == iso) {
        Some(Slot { net: Some(n), .. }) => Lookup::Live(n.clone()),
        Some(_) => Lookup::Shut,
        None => Lookup::Unknown,
    }
}

fn unknown(name: &str, h: u64) {
    let msg = if is_str(h) {
        format!("{name}: unknown p2p network handle: {}\n", str_of(h))
    } else {
        format!("{name}: not a p2p network handle: {}\n", display(h))
    };
    crate::console_write(msg.as_bytes());
}

/// `Some(net)`, or the call's "nothing" value already decided.
macro_rules! net_or {
    ($name:expr, $h:expr, $none:expr) => {
        match lookup($h) {
            Lookup::Live(n) => n,
            Lookup::Shut => return Some($none),
            Lookup::Unknown => {
                unknown($name, $h);
                return Some($none);
            },
        }
    };
}

pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    if !name.starts_with(b"p2p_") {
        return None;
    }
    let arg = |i: usize| args.get(i).copied().unwrap_or(UNIT);
    let no = FALSE;
    Some(match name {
        b"p2p_start" => {
            lwp::sys::install_entropy();
            let sc = match parse_config(arg(0)) {
                Ok(c) => c,
                Err(e) => return Some(mk_bad_str(&e)),
            };
            let ev = arg(1);
            if !(ev == UNIT || (is_str(ev) && str_of(ev).is_empty())) {
                return Some(mk_bad_str(&format!(
                    "p2p_start: {} is not a channel send handle (channel delivery is not wired yet: pass \"\" and use p2p_poll)",
                    display(ev)
                )));
            }
            let mut cfg = sc.cfg;
            cfg.data_dir = lwp::sys::prepare_dir(&cfg.data_dir);
            let net = match Network::start_tuned(cfg, sc.tuning) {
                Ok(n) => n,
                Err(e) => return Some(mk_bad_str(&format!("start: {e}"))),
            };
            crate::coro::on_kill(on_kill);
            crate::console_write(b"p2p: network started\n");
            let h = new_handle();
            nets().push(Slot { handle: h.clone(), iso: crate::coro::current_isolate(), net: Some(Rc::new(net)) });
            mk_ok(mk_str(&h))
        },
        b"p2p_poll" => {
            let net = net_or!("p2p_poll", arg(0), list_from(Vec::new()));
            let max = uint(arg(1), 1 << 20).unwrap_or(64).max(1) as usize;
            let timeout_ms = uint(arg(2), 3_600_000).unwrap_or(0);
            let evs = net.poll(max, timeout_ms);
            list_from(evs.into_iter().map(event_value).collect())
        },
        b"p2p_send" => {
            let net = net_or!("p2p_send", arg(0), no);
            let (Some(peer), Some(kind), Some(payload)) = (uint(arg(1), u64::MAX).filter(|&p| p >= 1), app_kind(arg(2)), payload(arg(3))) else {
                return Some(no);
            };
            if !net.peers().iter().any(|p| p.id == peer) {
                return Some(no);
            }
            net.send(peer, kind, payload);
            TRUE
        },
        b"p2p_broadcast" => {
            let net = net_or!("p2p_broadcast", arg(0), no);
            let (Some(kind), Some(payload), Ok(except)) = (app_kind(arg(1)), payload(arg(2)), opt_peer(arg(3))) else {
                return Some(no);
            };
            net.broadcast(kind, payload, except);
            TRUE
        },
        b"p2p_relay" => {
            let net = net_or!("p2p_relay", arg(0), no);
            let (Some(id), Some(payload), Some(kind), Ok(from)) =
                (as_str(arg(1)).and_then(unhex_n::<32>), payload(arg(2)), app_kind(arg(3)), opt_peer(arg(4)))
            else {
                return Some(no);
            };
            net.relay_tx(id, payload, kind, from);
            TRUE
        },
        b"p2p_mark_fluffed" => {
            let net = net_or!("p2p_mark_fluffed", arg(0), no);
            let Some(id) = as_str(arg(1)).and_then(unhex_n::<32>) else { return Some(no) };
            net.mark_fluffed(id);
            TRUE
        },
        b"p2p_peers" => {
            let net = net_or!("p2p_peers", arg(0), list_from(Vec::new()));
            let mut peers = net.peers();
            peers.sort_by_key(|p| p.id);
            list_from(
                peers
                    .into_iter()
                    .map(|p| {
                        list_from(alloc::vec![
                            num(p.id as f64),
                            mk_str(&p.addr),
                            boolean(p.inbound),
                            mk_str(&p.identity_fingerprint),
                            boolean(p.is_seed),
                            num(p.connected_secs as f64),
                        ])
                    })
                    .collect(),
            )
        },
        b"p2p_misbehaving" => {
            let net = net_or!("p2p_misbehaving", arg(0), no);
            let (Some(peer), Some(score)) = (uint(arg(1), u64::MAX).filter(|&p| p >= 1), uint(arg(2), u32::MAX as u64)) else {
                return Some(no);
            };
            let reason = if arg(3) == UNIT { String::new() } else { display(arg(3)) };
            net.misbehaving(peer, score as u32, &reason);
            TRUE
        },
        b"p2p_local_addr" => {
            let net = net_or!("p2p_local_addr", arg(0), mk_str(""));
            match net.local_addr() {
                Some(a) => mk_str(&a.to_string()),
                None => mk_str(""),
            }
        },
        b"p2p_netgroup" => {
            let net = net_or!("p2p_netgroup", arg(0), mk_str(""));
            let g = uint(arg(1), u64::MAX).and_then(|p| net.peer_netgroup(p));
            mk_str(&g.unwrap_or_default())
        },
        b"p2p_seeds_sign" => {
            lwp::sys::install_entropy();
            match seeds_sign(args) {
                Ok(s) => mk_str(&s),
                Err(m) => {
                    crate::console_write(format!("p2p_seeds_sign: {m}\n").as_bytes());
                    UNIT
                },
            }
        },
        b"p2p_seeds_verify" => {
            let Some(list_bytes) = as_str(arg(0)).and_then(unhex) else { return Some(mk_bad_str("malformed seed list")) };
            let Some(auth) = as_str(arg(1)).and_then(unhex).and_then(|b| IdentityPublic::from_bytes(&b).ok()) else {
                return Some(mk_bad_str("malformed authority public key"));
            };
            let Some(sl) = SignedSeedList::from_bytes(&list_bytes) else { return Some(mk_bad_str("malformed seed list")) };
            let Some(seeds) = sl.verify(&auth) else { return Some(mk_bad_str("bad signature")) };
            let entries = seeds.iter().map(|s| list_from(alloc::vec![mk_str(&s.addr), mk_str(&hex_encode(&s.identity.0))])).collect();
            mk_ok(list_from(alloc::vec![num(sl.network as f64), num(sl.issued_at as f64), list_from(entries)]))
        },
        b"p2p_shutdown" => {
            let h = arg(0);
            let Some(key) = as_str(h) else {
                unknown("p2p_shutdown", h);
                return Some(no);
            };
            let iso = crate::coro::current_isolate();
            let Some(i) = nets().iter().position(|s| s.handle == key && s.iso == iso) else {
                unknown("p2p_shutdown", h);
                return Some(no);
            };
            // take it out of the slot first: the shutdown yields while the
            // coroutines finish, and other calls must see the tombstone
            match nets()[i].net.take() {
                Some(net) => {
                    net.shutdown();
                    drop(net);
                    TRUE
                },
                None => FALSE,
            }
        },
        _ => return None,
    })
}

fn seeds_sign(args: &[u64]) -> Result<String, &'static str> {
    let arg = |i: usize| args.get(i).copied().unwrap_or(UNIT);
    let network = uint(arg(0), 255).ok_or("network must be an integer 0..255")? as u8;
    let issued_at = uint(arg(1), u64::MAX).ok_or("issued_at must be a non-negative integer")?;
    if !is_list(arg(2)) {
        return Err("entries must be a list of [addr, identity_pk_hex]");
    }
    let entries = list_items(arg(2));
    if entries.len() > lwp::MAX_SEEDS {
        return Err("too many seeds");
    }
    let mut seeds = Vec::with_capacity(entries.len());
    for &e in entries {
        let p = list_items(e);
        if !is_list(e) || p.len() != 2 {
            return Err("each entry must be [addr, identity_pk_hex]");
        }
        let addr = as_str(p[0]).filter(|a| lwp::is_valid_addr(a)).ok_or("invalid seed address")?;
        let identity = as_str(p[1])
            .and_then(unhex)
            .and_then(|b| IdentityPublic::from_bytes(&b).ok())
            .ok_or("invalid identity public key")?;
        seeds.push(SeedPeer { addr: addr.to_string(), identity });
    }
    let seed = as_str(arg(3)).and_then(unhex_n::<32>).ok_or("authority_seed_hex must be 64 hex chars")?;
    let authority = IdentityKey::from_seed(&seed);
    let list = SignedSeedList::sign(network, issued_at, seeds, &authority).ok_or("unencodable seed list")?;
    Ok(hex_encode(&list.to_bytes()))
}
