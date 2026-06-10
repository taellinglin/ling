//! Cryptographic verification of physics state.
//!
//! Two complementary guarantees:
//!
//! 1. **Per-snapshot authenticity** ([`ServerSigner`] / [`ClientVerifier`]).
//!    The host hashes each snapshot (BLAKE3 over a canonical encoding) and signs
//!    that hash with Ed25519. A client checks the signature against the host's
//!    public key, proving the state it renders came from the host unmodified —
//!    safe over lossy UDP because every snapshot is independently verifiable.
//!
//! 2. **Tamper-evident ordering** ([`TickChain`]).
//!    A running hash chain commits the *entire ordered history* of snapshots,
//!    so any later replacement of a past state is detectable. This needs an
//!    in-order, lossless view of the stream (the reliable channel, or replay),
//!    and is used for audit / desync forensics rather than the hot path.
//!
//! Desync detection ([`ClientVerifier::matches_local`]) compares the hash of the
//! host's authoritative snapshot against a client's own re-simulated snapshot —
//! if they differ for the same tick, the client mis-predicted (or someone is
//! cheating).

use ling_crypto::{Blake3, Ed25519Keypair};

use super::codec::ByteWriter;
use super::snapshot::Snapshot;

/// Canonical BLAKE3 hash of a snapshot's gameplay state.
///
/// Deterministic regardless of in-memory ordering: states, removed ids, and
/// stat blocks are all sorted before hashing. Both peers therefore agree on the
/// hash for identical logical state.
pub fn hash_snapshot(snap: &Snapshot) -> [u8; 32] {
    let mut states = snap.states.clone();
    states.sort_by_key(|e| e.id);
    let mut removed = snap.removed.clone();
    removed.sort_unstable();
    let mut stats = snap.stats.clone();
    stats.sort_by_key(|(id, _)| *id);

    let mut w = ByteWriter::with_capacity(64 + states.len() * 56);
    w.raw(b"LNG-SNAP-v1");
    w.u64(snap.tick);
    w.u32(states.len() as u32);
    for e in &states {
        e.encode(&mut w);
    }
    w.u32(removed.len() as u32);
    for id in &removed {
        w.u64(*id);
    }
    w.u32(stats.len() as u32);
    for (id, s) in &stats {
        w.u64(*id);
        s.encode(&mut w);
    }
    Blake3::hash(w.as_slice())
}

/// A snapshot bundled with the host's Ed25519 signature over its hash.
#[derive(Clone, Debug)]
pub struct SignedSnapshot {
    pub snapshot: Snapshot,
    pub signature: [u8; 64],
}

impl SignedSnapshot {
    pub(crate) fn encode(&self, w: &mut ByteWriter) {
        w.fixed(&self.signature);
        self.snapshot.encode(w);
    }

    pub(crate) fn decode(r: &mut super::codec::ByteReader) -> Result<Self, super::codec::CodecError> {
        let signature = r.fixed::<64>()?;
        let snapshot = Snapshot::decode(r)?;
        Ok(Self { snapshot, signature })
    }
}

/// Host-side signer. Holds the long-term Ed25519 identity of the authoritative
/// server and signs every outgoing snapshot.
pub struct ServerSigner {
    keypair: Ed25519Keypair,
}

impl ServerSigner {
    pub fn generate() -> Self {
        Self { keypair: Ed25519Keypair::generate() }
    }

    /// Deterministic identity from a 32-byte seed — useful for tests and for
    /// pinning a server's identity across restarts.
    pub fn from_seed(seed: [u8; 32]) -> Self {
        Self { keypair: Ed25519Keypair::from_seed(seed) }
    }

    pub fn public_key(&self) -> [u8; 32] {
        self.keypair.public_key()
    }

    pub fn sign(&self, snap: &Snapshot) -> [u8; 64] {
        self.keypair.sign(&hash_snapshot(snap))
    }

    pub fn sign_snapshot(&self, snap: Snapshot) -> SignedSnapshot {
        let signature = self.sign(&snap);
        SignedSnapshot { snapshot: snap, signature }
    }
}

impl Default for ServerSigner {
    fn default() -> Self {
        Self::generate()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VerifyError {
    /// Signature did not validate against the pinned host public key.
    BadSignature,
}

impl std::fmt::Display for VerifyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::BadSignature => write!(f, "snapshot signature failed verification"),
        }
    }
}

impl std::error::Error for VerifyError {}

/// Client-side verifier holding the host's pinned public key.
pub struct ClientVerifier {
    server_pub: [u8; 32],
}

impl ClientVerifier {
    pub fn new(server_pub: [u8; 32]) -> Self {
        Self { server_pub }
    }

    pub fn server_public_key(&self) -> [u8; 32] {
        self.server_pub
    }

    /// Verify the host's signature over a snapshot.
    pub fn verify(&self, signed: &SignedSnapshot) -> Result<(), VerifyError> {
        let hash = hash_snapshot(&signed.snapshot);
        Ed25519Keypair::verify(&self.server_pub, &hash, &signed.signature)
            .map_err(|_| VerifyError::BadSignature)
    }

    /// True when a client's locally re-simulated snapshot matches the host's
    /// authoritative one (same hash) — i.e. no desync / no tampering.
    pub fn matches_local(server: &Snapshot, local: &Snapshot) -> bool {
        hash_snapshot(server) == hash_snapshot(local)
    }
}

/// Tamper-evident running hash over an ordered stream of snapshots.
///
/// `head ← BLAKE3("LNG-CHAIN-v1" ‖ head ‖ hash(snapshot) ‖ tick)`. Two parties
/// that processed the same snapshots in the same order converge on the same
/// head; a single altered or reordered past snapshot changes it.
pub struct TickChain {
    head: [u8; 32],
    count: u64,
}

impl Default for TickChain {
    fn default() -> Self {
        Self::new()
    }
}

impl TickChain {
    pub fn new() -> Self {
        Self { head: [0u8; 32], count: 0 }
    }

    /// Start the chain from a shared genesis value (e.g. a session id hash).
    pub fn with_genesis(genesis: [u8; 32]) -> Self {
        Self { head: genesis, count: 0 }
    }

    pub fn head(&self) -> [u8; 32] {
        self.head
    }

    pub fn count(&self) -> u64 {
        self.count
    }

    /// Fold a snapshot into the chain, returning the new head.
    pub fn append(&mut self, snap: &Snapshot) -> [u8; 32] {
        let h = hash_snapshot(snap);
        let mut w = ByteWriter::with_capacity(11 + 32 + 32 + 8);
        w.raw(b"LNG-CHAIN-v1").fixed(&self.head).fixed(&h).u64(snap.tick);
        self.head = Blake3::hash(w.as_slice());
        self.count += 1;
        self.head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mmo::snapshot::EntityState;
    use glam::Vec3;

    fn snap(tick: u64, x: f32) -> Snapshot {
        Snapshot::keyframe(tick, vec![EntityState::at(1, Vec3::new(x, 0.0, 0.0))])
    }

    #[test]
    fn valid_signature_verifies() {
        let signer = ServerSigner::from_seed([7u8; 32]);
        let verifier = ClientVerifier::new(signer.public_key());
        let signed = signer.sign_snapshot(snap(1, 3.0));
        assert!(verifier.verify(&signed).is_ok());
    }

    #[test]
    fn tampered_state_fails_verification() {
        let signer = ServerSigner::from_seed([7u8; 32]);
        let verifier = ClientVerifier::new(signer.public_key());
        let mut signed = signer.sign_snapshot(snap(1, 3.0));
        // Attacker rewrites the position after signing.
        signed.snapshot.states[0].pos.x = 999.0;
        assert_eq!(verifier.verify(&signed), Err(VerifyError::BadSignature));
    }

    #[test]
    fn wrong_key_fails() {
        let signer = ServerSigner::from_seed([1u8; 32]);
        let impostor = ServerSigner::from_seed([2u8; 32]);
        let verifier = ClientVerifier::new(impostor.public_key());
        let signed = signer.sign_snapshot(snap(1, 3.0));
        assert!(verifier.verify(&signed).is_err());
    }

    #[test]
    fn hash_is_order_independent() {
        let a = Snapshot::keyframe(1, vec![
            EntityState::at(1, Vec3::X),
            EntityState::at(2, Vec3::Y),
        ]);
        let b = Snapshot::keyframe(1, vec![
            EntityState::at(2, Vec3::Y),
            EntityState::at(1, Vec3::X),
        ]);
        assert_eq!(hash_snapshot(&a), hash_snapshot(&b));
    }

    #[test]
    fn chain_diverges_on_history_edit() {
        let mut honest = TickChain::new();
        let mut forged = TickChain::new();
        honest.append(&snap(1, 1.0));
        forged.append(&snap(1, 1.0));
        // Same tick 2 → still equal.
        assert_eq!(honest.append(&snap(2, 2.0)), forged.append(&snap(2, 2.0)));
        // Forged history changes tick 3.
        honest.append(&snap(3, 3.0));
        forged.append(&snap(3, 3.5));
        assert_ne!(honest.head(), forged.head());
    }

    #[test]
    fn desync_detection() {
        let server = snap(10, 4.0);
        let good = snap(10, 4.0);
        let bad = snap(10, 4.5);
        assert!(ClientVerifier::matches_local(&server, &good));
        assert!(!ClientVerifier::matches_local(&server, &bad));
    }
}
