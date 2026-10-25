//! Signed seed list encoding (DESIGN §8.3).
//!
//! ```text
//! body  = "LWSD" ‖ version:u8 (=1) ‖ network:u8 ‖ issued_at:u64_le ‖ count:u16_le
//!         ‖ count × ( addr_len:u8 ‖ addr (UTF-8, valid peer address) ‖ identity_pk:2592 )
//! file  = body ‖ signature:4627          (ML-DSA-87, context "seed-list", msg = body)
//! ```
//! `count ≤ MAX_SEEDS`; `from_bytes` requires exact consumption.

use crate::lwp::crypto::identity::{self, IdentityKey, IdentityPublic, IdentitySignature};

use crate::lwp::addr::NetAddr;
use crate::lwp::wire::Rd;
use crate::lwp::{SeedPeer, SignedSeedList};

pub const MAX_SEEDS: usize = 256;
const MAGIC: &[u8; 4] = b"LWSD";
const VERSION: u8 = 1;
const ENTRY_MAX: usize = 1 + 255 + identity::PUBLIC_KEY_LEN;
const MAX_LEN: usize = 4 + 1 + 1 + 8 + 2 + MAX_SEEDS * ENTRY_MAX + identity::SIGNATURE_LEN;

/// `None` if the list cannot be encoded (too many seeds, bad address/key).
pub(crate) fn body(network: u8, issued_at: u64, seeds: &[SeedPeer]) -> Option<Vec<u8>> {
    if seeds.len() > MAX_SEEDS {
        return None;
    }
    let mut v = Vec::new();
    v.extend_from_slice(MAGIC);
    v.push(VERSION);
    v.push(network);
    v.extend_from_slice(&issued_at.to_le_bytes());
    v.extend_from_slice(&(seeds.len() as u16).to_le_bytes());
    for s in seeds {
        if NetAddr::parse(&s.addr).is_none() || s.identity.0.len() != identity::PUBLIC_KEY_LEN {
            return None;
        }
        v.push(s.addr.len() as u8);
        v.extend_from_slice(s.addr.as_bytes());
        v.extend_from_slice(&s.identity.0);
    }
    Some(v)
}

impl SignedSeedList {
    pub(crate) fn sign_impl(network: u8, issued_at: u64, seeds: Vec<SeedPeer>, authority: &IdentityKey) -> Self {
        // Operator input, not attacker input: refuse loudly rather than sign
        // something that can never be decoded.
        let b = body(network, issued_at, &seeds).expect("seed list: >256 seeds, invalid address, or invalid identity");
        let signature = authority.sign("seed-list", &b);
        SignedSeedList { network, issued_at, seeds, signature }
    }

    pub(crate) fn verify_impl(&self, authority: &IdentityPublic) -> Option<&[SeedPeer]> {
        let b = body(self.network, self.issued_at, &self.seeds)?;
        authority.verify("seed-list", &b, &self.signature).then_some(&self.seeds[..])
    }

    pub(crate) fn to_bytes_impl(&self) -> Vec<u8> {
        // An unencodable list (only constructible by hand) serializes to an
        // empty buffer, which `from_bytes` rejects.
        match body(self.network, self.issued_at, &self.seeds) {
            Some(mut v) => {
                v.extend_from_slice(&self.signature.0);
                v
            }
            None => Vec::new(),
        }
    }

    pub(crate) fn from_bytes_impl(b: &[u8]) -> Option<Self> {
        if b.len() > MAX_LEN {
            return None;
        }
        let mut r = Rd::new(b);
        if r.take(4)? != MAGIC || r.u8()? != VERSION {
            return None;
        }
        let network = r.u8()?;
        let issued_at = r.u64()?;
        let count = r.u16()? as usize;
        if count > MAX_SEEDS {
            return None;
        }
        let mut seeds = Vec::with_capacity(count);
        for _ in 0..count {
            let alen = r.u8()? as usize;
            let addr = std::str::from_utf8(r.take(alen)?).ok()?.to_string();
            NetAddr::parse(&addr)?;
            let identity = IdentityPublic::from_bytes(r.take(identity::PUBLIC_KEY_LEN)?).ok()?;
            seeds.push(SeedPeer { addr, identity });
        }
        let signature = IdentitySignature::from_bytes(r.take(identity::SIGNATURE_LEN)?).ok()?;
        if !r.done() {
            return None;
        }
        Some(SignedSeedList { network, issued_at, seeds, signature })
    }
}
