//! LWP-1 peer-to-peer networking (ling-wallet DESIGN §8), moved here from
//! ling-wallet's `lw-net` crate unchanged (feature `lwp`). Transport only: payloads are
//! opaque bytes tagged with an application `kind`; `lw-node` defines what
//! they mean. Kinds `0x00..=0x0F` are reserved for this crate's own control
//! messages (padding, ping, peer exchange, Dandelion stem).
//!
//! Modules: `handshake` (§8.1), `frame` (§8.2), `addr` / `peerdb` / `seeds` /
//! `socks5` (§8.3), `dandelion` (§8.4), `net` (threads and peer management).
//! Wire formats are documented in each module and in DESIGN §8.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use crate::lwp::crypto::identity::{IdentityKey, IdentityPublic, IdentitySignature};

/// Crypto shim (hash/kem/identity/aead/random); see its module docs.
pub mod crypto;

mod addr;
mod dandelion;
mod frame;
mod handshake;
mod net;
mod peerdb;
mod seeds;
mod socks5;
mod wire;

#[doc(hidden)]
pub use net::Tuning;

pub type PeerId = u64;

/// First application kind; everything below is reserved for LWP-1.
pub const APP_KIND_MIN: u8 = 0x10;

/// Largest application payload [`Network::send`] / [`Network::broadcast`] /
/// [`Network::relay_tx`] accept (4 MiB + 64 KiB - 5 bytes); larger payloads
/// are dropped. A maximum-size 4 MiB block plus a small header fits.
pub const MAX_PAYLOAD: usize = frame::MAX_PAYLOAD;
/// Plaintext frame body sizes: `kind ‖ u32 len ‖ payload ‖ padding` is padded
/// to the smallest bucket that fits.
pub const FRAME_BUCKETS: [usize; 7] = frame::BUCKETS;
/// The u32 length prefix of every frame equals `bucket + FRAME_OVERHEAD`.
pub const FRAME_OVERHEAD: usize = frame::OVERHEAD;
/// Fixed handshake message sizes (I→R, R→I, I→R).
pub const HANDSHAKE_MSG1_LEN: usize = handshake::MSG1_LEN;
pub const HANDSHAKE_MSG2_LEN: usize = handshake::MSG2_LEN;
pub const HANDSHAKE_MSG3_LEN: usize = handshake::MSG3_LEN;
/// Maximum seeds in a [`SignedSeedList`].
pub const MAX_SEEDS: usize = seeds::MAX_SEEDS;

/// The receiving end of a [`Network`]'s event queue.
pub type EventReceiver = crossbeam_channel::Receiver<NetEvent>;

/// Whether `s` is a well-formed peer address (`a.b.c.d:port`, `[v6]:port`,
/// `name:port`, `x.onion:port`; DESIGN §8.3). Used by front ends to validate
/// operator input before it reaches [`SignedSeedList::sign`] (which panics on
/// an unencodable list).
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
    /// Panics if the list is unencodable (more than [`MAX_SEEDS`] seeds, an
    /// address that does not parse or exceeds 255 bytes, or a malformed
    /// identity): this is operator tooling input, never network input.
    pub fn sign(network: u8, issued_at: u64, seeds: Vec<SeedPeer>, authority: &IdentityKey) -> Self {
        Self::sign_impl(network, issued_at, seeds, authority)
    }
    /// Returns the seeds only if the signature verifies under `authority`.
    /// The caller must also check `network` (and may reject a stale `issued_at`).
    pub fn verify(&self, authority: &IdentityPublic) -> Option<&[SeedPeer]> {
        self.verify_impl(authority)
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        self.to_bytes_impl()
    }
    /// Total and bounded: never panics, rejects trailing bytes. Does not check
    /// the signature (use [`Self::verify`]).
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Self::from_bytes_impl(b)
    }
}

pub struct NetConfig {
    pub network_magic: [u8; 4],
    pub network: u8,
    pub listen: Option<SocketAddr>,
    /// Peer DB, anchors, and persisted identity live here.
    pub data_dir: PathBuf,
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
    /// Extra addresses to always try (e.g. other local nodes on this machine).
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

pub struct Network {
    sh: Arc<net::Shared>,
}

impl Network {
    /// Start listener + connection manager threads.
    ///
    /// `listen = None` means no listener at all (outbound only); in particular
    /// with a SOCKS5 proxy we only ever listen where `listen` says. Events
    /// arrive on a bounded channel: the node must keep draining it (a stalled
    /// consumer back-pressures every peer's reader).
    pub fn start(cfg: NetConfig) -> std::io::Result<(Network, crossbeam_channel::Receiver<NetEvent>)> {
        Self::start_tuned(cfg, Tuning::default())
    }
    /// [`Network::start`] with non-default timers (tests).
    #[doc(hidden)]
    pub fn start_tuned(cfg: NetConfig, tuning: Tuning) -> std::io::Result<(Network, crossbeam_channel::Receiver<NetEvent>)> {
        let (sh, rx) = net::start(cfg, tuning)?;
        Ok((Network { sh }, rx))
    }
    /// The bound listen address (useful with port 0).
    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.sh.local_addr
    }
    /// The peer's network group (e.g. `v4:1.2`), for the node's "compare
    /// chain tips across ≥3 groups" rule (DESIGN §8.3).
    pub fn peer_netgroup(&self, peer: PeerId) -> Option<String> {
        self.sh.peer_netgroup(peer)
    }
    /// Queue an application message (`kind >= APP_KIND_MIN`, payload ≤
    /// [`MAX_PAYLOAD`]; anything else is dropped). Never blocks; a peer whose
    /// send queue is full is disconnected.
    pub fn send(&self, peer: PeerId, kind: u8, payload: Vec<u8>) {
        self.sh.send(peer, kind, payload)
    }
    pub fn broadcast(&self, kind: u8, payload: Vec<u8>, except: Option<PeerId>) {
        self.sh.broadcast(kind, payload, except)
    }
    /// Dandelion++ entry point for a *validated* transaction: either stem it
    /// to this epoch's stem peer or fluff it (broadcast as `fluff_kind`).
    /// `from` is the peer it came from (None if created locally). Starts the
    /// embargo timer.
    pub fn relay_tx(&self, tx_id: [u8; 32], payload: Vec<u8>, fluff_kind: u8, from: Option<PeerId>) {
        self.sh.relay_tx(tx_id, payload, fluff_kind, from)
    }
    /// Tell the Dandelion router a tx was seen in fluff phase (cancels embargo).
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
    /// Stop all threads, close all connections, persist the peer DB. Idempotent.
    pub fn shutdown(&self) {
        self.sh.shutdown()
    }
}

impl Drop for Network {
    fn drop(&mut self) {
        self.sh.shutdown();
    }
}
