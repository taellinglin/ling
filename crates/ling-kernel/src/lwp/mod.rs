//! LWP-1 peer-to-peer networking (ling-wallet DESIGN §8) in the kernel: a
//! port of ling-net's `lwp` (std threads + crossbeam + blocking sockets) to
//! `no_std` + `alloc`, cooperative coroutines (`crate::coro`) and the
//! netstack's TCP handles. The wire protocol — handshake, framing,
//! encryption, control kinds, address gossip, seed-list signatures — is the
//! host's byte for byte, so a LingOS node talks to desktop nodes.
//!
//! Modules: `handshake` (§8.1), `frame` (§8.2), `addr` / `peerdb` / `seeds` /
//! `socks5` (§8.3), `dandelion` (§8.4), `net` (coroutines and peer
//! management), `conn` (kernel TCP adapter), `sys` (time, randomness, files).
//! Kinds `0x00..=0x0F` are reserved for LWP-1's own control messages.

use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::net::SocketAddr;

use crate::lwp::crypto::identity::{IdentityKey, IdentityPublic, IdentitySignature};

/// Crypto shim: ling-crypto's LW-1 primitives (the very code the host shim
/// was copied from) plus the kernel RNG.
pub mod crypto {
    pub use ling_crypto::lw::kem1024 as kem;
    pub use ling_crypto::lw::{aead, hash, identity, Zeroizing};
    pub mod kdf {
        /// Fill `buf` from the kernel CSPRNG.
        pub fn random_bytes(buf: &mut [u8]) {
            crate::lwp::sys::random_bytes(buf)
        }
    }
}

mod addr;
mod conn;
mod dandelion;
mod frame;
mod handshake;
mod net;
mod peerdb;
/// `p2ptest`: a handshake over an in-memory pipe (stack use, crypto on this CPU).
pub mod selftest;
mod seeds;
mod socks5;
pub mod sys;
mod wire;

pub use net::Tuning;

pub type PeerId = u64;

/// First application kind; everything below is reserved for LWP-1.
pub const APP_KIND_MIN: u8 = 0x10;
/// Largest application payload accepted (4 MiB + 64 KiB - 5 bytes).
pub const MAX_PAYLOAD: usize = frame::MAX_PAYLOAD;
pub const FRAME_BUCKETS: [usize; 7] = frame::BUCKETS;
pub const FRAME_OVERHEAD: usize = frame::OVERHEAD;
pub const HANDSHAKE_MSG1_LEN: usize = handshake::MSG1_LEN;
pub const HANDSHAKE_MSG2_LEN: usize = handshake::MSG2_LEN;
pub const HANDSHAKE_MSG3_LEN: usize = handshake::MSG3_LEN;
/// Maximum seeds in a [`SignedSeedList`].
pub const MAX_SEEDS: usize = seeds::MAX_SEEDS;

/// Whether `s` is a well-formed peer address (`a.b.c.d:port`, `[v6]:port`,
/// `name:port`, `x.onion:port`).
pub fn is_valid_addr(s: &str) -> bool {
    addr::NetAddr::parse(s).is_some()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SeedPeer {
    /// `host:port`, `[v6]:port`, or `xyz.onion:port` (onion requires a proxy).
    pub addr: String,
    /// Pinned identity; a seed presenting any other key is dropped.
    pub identity: IdentityPublic,
}

/// A seed list signed by the seed authority (DESIGN §8.3).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SignedSeedList {
    pub network: u8,
    pub issued_at: u64,
    pub seeds: Vec<SeedPeer>,
    pub signature: IdentitySignature,
}

impl SignedSeedList {
    /// `None` if the list is unencodable (more than [`MAX_SEEDS`] seeds, an
    /// address that does not parse, or a malformed identity) — where the host
    /// panics.
    pub fn sign(network: u8, issued_at: u64, seeds: Vec<SeedPeer>, authority: &IdentityKey) -> Option<Self> {
        Self::sign_impl(network, issued_at, seeds, authority)
    }
    /// Returns the seeds only if the signature verifies under `authority`.
    pub fn verify(&self, authority: &IdentityPublic) -> Option<&[SeedPeer]> {
        self.verify_impl(authority)
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_impl()
    }
    /// Total and bounded: never panics, rejects trailing bytes.
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Self::from_bytes_impl(b)
    }
}

pub struct NetConfig {
    pub network_magic: [u8; 4],
    pub network: u8,
    /// Listen address; only IPv4 is served (the kernel netstack), and only
    /// the port matters (we accept on every local address).
    pub listen: Option<SocketAddr>,
    /// Peer DB and anchors live here (a vfs path).
    pub data_dir: String,
    /// Already-verified seeds (caller checks the SignedSeedList).
    pub seeds: Vec<SeedPeer>,
    /// SOCKS5 proxy for all outbound connections (e.g. Tor 127.0.0.1:9050).
    pub socks5_proxy: Option<SocketAddr>,
    pub max_outbound: usize,
    pub max_inbound: usize,
    /// Long-term identity (used for inbound, and outbound unless ephemeral).
    pub identity: IdentityKey,
    /// Use a fresh identity per outbound connection (privacy default).
    pub ephemeral_outbound_identity: bool,
    /// Extra addresses to always try.
    pub manual_peers: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct PeerInfo {
    pub id: PeerId,
    pub addr: String,
    pub inbound: bool,
    pub identity_fingerprint: String,
    pub is_seed: bool,
    pub connected_secs: u64,
    pub bytes_in: u64,
    pub bytes_out: u64,
}

#[derive(Debug)]
pub enum NetEvent {
    Connected(PeerInfo),
    Disconnected(PeerId),
    /// An application message (`kind >= APP_KIND_MIN`).
    Message { peer: PeerId, kind: u8, payload: Vec<u8> },
    /// A Dandelion stem-phase transaction for the node to validate; the node
    /// must answer with [`Network::relay_tx`] (valid) or drop it (invalid).
    StemTx { peer: PeerId, payload: Vec<u8> },
}

/// A running network: a manager coroutine, a listener coroutine (with a
/// listen address) and one coroutine per connection, all workers of the
/// isolate that started it. Dropping it aborts everything at once.
pub struct Network {
    sh: Rc<net::Shared>,
}

impl Network {
    /// Start listener + connection manager coroutines. `Err` carries a
    /// message (e.g. "address in use").
    pub fn start(cfg: NetConfig) -> Result<Network, String> {
        Self::start_tuned(cfg, Tuning::default())
    }
    pub fn start_tuned(cfg: NetConfig, tuning: Tuning) -> Result<Network, String> {
        Ok(Network { sh: net::start(cfg, tuning)? })
    }
    /// The isolate that started this network (0 = main).
    pub fn isolate(&self) -> u64 {
        self.sh.iso
    }
    /// Are this network's service coroutines (manager, listener) running?
    pub fn workers_alive(&self) -> bool {
        self.sh.workers_alive()
    }
    /// The listen address (with the real port when 0 was asked for).
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.sh.local_addr
    }
    /// The peer's network group (e.g. `v4:1.2`).
    pub fn peer_netgroup(&self, peer: PeerId) -> Option<String> {
        self.sh.peer_netgroup(peer)
    }
    /// Queue an application message. Never blocks; a peer whose send queue
    /// is full is disconnected.
    pub fn send(&self, peer: PeerId, kind: u8, payload: Vec<u8>) {
        self.sh.send(peer, kind, payload)
    }
    pub fn broadcast(&self, kind: u8, payload: Vec<u8>, except: Option<PeerId>) {
        self.sh.broadcast(kind, payload, except)
    }
    /// Dandelion++ entry point for a *validated* transaction.
    pub fn relay_tx(&self, tx_id: [u8; 32], payload: Vec<u8>, fluff_kind: u8, from: Option<PeerId>) {
        self.sh.relay_tx(tx_id, payload, fluff_kind, from)
    }
    pub fn mark_fluffed(&self, tx_id: [u8; 32]) {
        self.sh.mark_fluffed(tx_id)
    }
    pub fn peers(&self) -> Vec<PeerInfo> {
        self.sh.peers()
    }
    /// Increase a peer's ban score; at 100 it is disconnected and banned 24 h.
    pub fn misbehaving(&self, peer: PeerId, score: u32, reason: &str) {
        self.sh.misbehaving(peer, score, reason)
    }
    /// Up to `max` queued events. With `timeout_ms > 0`, waits (yielding)
    /// that long for the first one — the host's `recv_timeout` + `try_recv`.
    pub fn poll(&self, max: usize, timeout_ms: u64) -> Vec<NetEvent> {
        self.sh.poll(max, timeout_ms)
    }
    /// Stop everything, close all connections (waiting cooperatively for the
    /// coroutines to finish), persist the peer DB. Idempotent.
    pub fn shutdown(&self) {
        self.sh.shutdown()
    }
    /// Stop everything now without waiting (the program that owned the
    /// network is gone): free every socket, stop listening, save the DB.
    pub fn abort(&self) {
        self.sh.abort()
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.sh.abort();
    }
}
