//! LWP-1 handshake (DESIGN §8.1). Port of ling-net `lwp/handshake.rs`:
//! mutually authenticated, hybrid X25519 ‖ ML-KEM-1024 key agreement,
//! ML-DSA-87 transcript signatures, both identities encrypted. The message
//! layout and key schedule are the host's, byte for byte; only the socket is
//! an [`Io`] and the self-nonce set is single-threaded.
//!
//! ```text
//! msg1 I→R (1641 B) = "LWP1" ‖ magic:4 ‖ network:u8 ‖ nonce_i:32 ‖ kem_pk_i:1600
//! msg2 R→I (8867 B) = nonce_r:32 ‖ kem_ct:1600 ‖ AEAD(k_hs_r, 0^24, aad = th1, id_pk_r ‖ sig_r)
//! msg3 I→R (7235 B) = AEAD(k_hs_i, 0^24, aad = th2, id_pk_i ‖ sig_i)
//! then R→I: first encrypted frame must be ACCEPT.
//!
//! ss    = hybrid KEM shared secret with context "LW1/p2p/kem"
//! th1   = H("LW1/p2p/th1", msg1, nonce_r, kem_ct)
//! k_hs  = XOF("LW1/p2p/hs-keys", ss, th1, 64) = k_hs_r ‖ k_hs_i
//! sig_r = ML-DSA-87 sign, context "p2p-hs-r", msg = th1 ‖ id_pk_r
//! th2   = H("LW1/p2p/th2", th1, id_pk_r, sig_r)
//! sig_i = ML-DSA-87 sign, context "p2p-hs-i", msg = th2 ‖ id_pk_i
//! th3   = H("LW1/p2p/th3", th2, id_pk_i, sig_i)
//! keys  = XOF("LW1/p2p/keys", ss, th3, 128) = k_i2r ‖ k_r2i ‖ dice_i2r ‖ dice_r2i
//! ```

use alloc::collections::BTreeSet;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::lwp::crypto::identity::{self, IdentityKey, IdentityPublic, IdentitySignature};
use crate::lwp::crypto::{aead, hash, kdf, kem, Zeroizing};
use crate::lwp::frame::{CipherState, DirKeys, Io, IoError, DIR_I2R, DIR_R2I};

pub const PROTO: &[u8; 4] = b"LWP1";
pub const MSG1_LEN: usize = 4 + 4 + 1 + 32 + kem::PUBLIC_KEY_LEN;
const ID_BLOB: usize = identity::PUBLIC_KEY_LEN + identity::SIGNATURE_LEN;
pub const ENC_ID_LEN: usize = ID_BLOB + aead::TAG_LEN;
pub const MSG2_LEN: usize = 32 + kem::CIPHERTEXT_LEN + ENC_ID_LEN;
pub const MSG3_LEN: usize = ENC_ID_LEN;
const KEM_CONTEXT: &[u8] = b"LW1/p2p/kem";

#[allow(dead_code)] // fields are for Debug output
#[derive(Debug)]
pub(crate) enum HsError {
    Io(IoError),
    Proto(&'static str),
    SelfConnect,
    PinMismatch,
}

impl From<IoError> for HsError {
    fn from(e: IoError) -> Self {
        HsError::Io(e)
    }
}

/// Nonces of handshakes we are currently running (either role); seeing one
/// come back means we dialed ourselves.
#[derive(Default)]
pub(crate) struct SelfNonces(RefCell<BTreeSet<[u8; 32]>>);

pub(crate) struct NonceGuard<'a>(&'a SelfNonces, pub [u8; 32]);
impl Drop for NonceGuard<'_> {
    fn drop(&mut self) {
        if let Ok(mut s) = (self.0).0.try_borrow_mut() {
            s.remove(&self.1);
        }
    }
}

impl SelfNonces {
    fn register(&self) -> NonceGuard<'_> {
        let mut n = [0u8; 32];
        kdf::random_bytes(&mut n);
        self.0.borrow_mut().insert(n);
        NonceGuard(self, n)
    }
    fn contains(&self, n: &[u8; 32]) -> bool {
        self.0.borrow().contains(n)
    }
}

pub(crate) struct Params<'a> {
    pub magic: [u8; 4],
    pub network: u8,
    /// `sys::now_ms()` deadline for the whole handshake.
    pub deadline: u64,
    pub self_nonces: &'a SelfNonces,
    /// Our long-term identity; a peer presenting it is ourselves.
    pub own_identity: &'a IdentityPublic,
}

pub(crate) struct Session {
    pub send: CipherState,
    pub recv: CipherState,
    pub peer: IdentityPublic,
}

fn split64(b: &[u8; 64]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let mut a = Zeroizing::new([0u8; 32]);
    let mut c = Zeroizing::new([0u8; 32]);
    a.copy_from_slice(&b[..32]);
    c.copy_from_slice(&b[32..]);
    (a, c)
}

fn hs_keys(ss: &[u8; 32], th1: &[u8; 32]) -> (Zeroizing<[u8; 32]>, Zeroizing<[u8; 32]>) {
    let mut o = Zeroizing::new([0u8; 64]);
    hash::xof("LW1/p2p/hs-keys", &[ss, th1], o.as_mut());
    split64(&o)
}

/// (i2r, r2i) direction keys.
fn session_keys(ss: &[u8; 32], th3: &[u8; 32]) -> (DirKeys, DirKeys) {
    let mut o = Zeroizing::new([0u8; 128]);
    hash::xof("LW1/p2p/keys", &[ss, th3], o.as_mut());
    let k = |r: core::ops::Range<usize>| {
        let mut a = Zeroizing::new([0u8; 32]);
        a.copy_from_slice(&o[r]);
        a
    };
    (DirKeys { aead: k(0..32), dice: k(64..96) }, DirKeys { aead: k(32..64), dice: k(96..128) })
}

fn cat(a: &[u8], b: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(a.len() + b.len());
    v.extend_from_slice(a);
    v.extend_from_slice(b);
    v
}

fn seal_identity(k: &[u8; 32], th: &[u8; 32], me: &IdentityKey, ctx: &str) -> (Vec<u8>, IdentityPublic, IdentitySignature) {
    let pk = me.public();
    let sig = me.sign(ctx, &cat(th, &pk.0));
    let blob = Zeroizing::new(cat(&pk.0, &sig.0));
    (aead::seal(k, &[0u8; 24], th, &blob), pk, sig)
}

fn open_identity(k: &[u8; 32], th: &[u8; 32], enc: &[u8], ctx: &str) -> Result<(IdentityPublic, IdentitySignature), HsError> {
    let blob = aead::open(k, &[0u8; 24], th, enc).map_err(|_| HsError::Proto("identity decrypt"))?;
    if blob.len() != ID_BLOB {
        return Err(HsError::Proto("identity size"));
    }
    let pk = IdentityPublic::from_bytes(&blob[..identity::PUBLIC_KEY_LEN]).map_err(|_| HsError::Proto("identity"))?;
    let sig = IdentitySignature::from_bytes(&blob[identity::PUBLIC_KEY_LEN..]).map_err(|_| HsError::Proto("sig"))?;
    if !pk.verify(ctx, &cat(th, &pk.0), &sig) {
        return Err(HsError::Proto("bad signature"));
    }
    Ok((pk, sig))
}

/// Run the initiator side. `expect` pins the responder identity (seeds).
pub(crate) fn initiate<S: Io + ?Sized>(
    s: &mut S,
    p: &Params<'_>,
    me: &IdentityKey,
    expect: Option<&IdentityPublic>,
) -> Result<Session, HsError> {
    let guard = p.self_nonces.register();
    let nonce_i = guard.1;
    let eph = kem::KemSecret::generate();
    let mut msg1 = Vec::with_capacity(MSG1_LEN);
    msg1.extend_from_slice(PROTO);
    msg1.extend_from_slice(&p.magic);
    msg1.push(p.network);
    msg1.extend_from_slice(&nonce_i);
    msg1.extend_from_slice(eph.public().as_bytes());
    debug_assert_eq!(msg1.len(), MSG1_LEN);
    s.write_all_deadline(&msg1, p.deadline)?;

    let mut msg2 = alloc::vec![0u8; MSG2_LEN];
    s.read_exact_deadline(&mut msg2, p.deadline)?;
    let nonce_r: [u8; 32] = msg2[..32].try_into().unwrap();
    if nonce_r == nonce_i || p.self_nonces.contains(&nonce_r) {
        return Err(HsError::SelfConnect);
    }
    let ct_bytes = &msg2[32..32 + kem::CIPHERTEXT_LEN];
    let ct = kem::KemCiphertext::from_bytes(ct_bytes).map_err(|_| HsError::Proto("kem ct"))?;
    let ss = Zeroizing::new(eph.decapsulate(&ct, KEM_CONTEXT).map_err(|_| HsError::Proto("kem"))?);
    let th1 = hash::h256("LW1/p2p/th1", &[&msg1, &nonce_r, ct_bytes]);
    let (k_hs_r, k_hs_i) = hs_keys(&ss, &th1);
    let (pk_r, sig_r) = open_identity(&k_hs_r, &th1, &msg2[32 + kem::CIPHERTEXT_LEN..], "p2p-hs-r")?;
    if &pk_r == p.own_identity {
        return Err(HsError::SelfConnect);
    }
    if let Some(pin) = expect {
        if pin != &pk_r {
            return Err(HsError::PinMismatch);
        }
    }
    let th2 = hash::h256("LW1/p2p/th2", &[&th1, &pk_r.0, &sig_r.0]);
    let (msg3, pk_i, sig_i) = seal_identity(&k_hs_i, &th2, me, "p2p-hs-i");
    debug_assert_eq!(msg3.len(), MSG3_LEN);
    s.write_all_deadline(&msg3, p.deadline)?;
    let th3 = hash::h256("LW1/p2p/th3", &[&th2, &pk_i.0, &sig_i.0]);
    let (i2r, r2i) = session_keys(&ss, &th3);
    drop(guard);
    Ok(Session { send: CipherState::new(i2r, DIR_I2R), recv: CipherState::new(r2i, DIR_R2I), peer: pk_r })
}

/// Run the responder side (msg1 → msg2 → msg3). The caller then decides
/// admission and, if admitted, sends the ACCEPT frame first.
pub(crate) fn respond<S: Io + ?Sized>(s: &mut S, p: &Params<'_>, me: &IdentityKey) -> Result<Session, HsError> {
    let mut msg1 = alloc::vec![0u8; MSG1_LEN];
    s.read_exact_deadline(&mut msg1, p.deadline)?;
    if &msg1[..4] != PROTO || msg1[4..8] != p.magic || msg1[8] != p.network {
        return Err(HsError::Proto("wrong protocol or network"));
    }
    let nonce_i: [u8; 32] = msg1[9..41].try_into().unwrap();
    if p.self_nonces.contains(&nonce_i) {
        return Err(HsError::SelfConnect);
    }
    let pk_i_eph = kem::KemPublic::from_bytes(&msg1[41..]).map_err(|_| HsError::Proto("kem pk"))?;
    let guard = p.self_nonces.register();
    let nonce_r = guard.1;
    let (ct, ss) = kem::try_encapsulate(&pk_i_eph, KEM_CONTEXT).map_err(|_| HsError::Proto("kem pk"))?;
    let ss = Zeroizing::new(ss);
    let th1 = hash::h256("LW1/p2p/th1", &[&msg1, &nonce_r, ct.as_bytes()]);
    let (k_hs_r, k_hs_i) = hs_keys(&ss, &th1);
    let (enc_r, pk_r, sig_r) = seal_identity(&k_hs_r, &th1, me, "p2p-hs-r");
    let mut msg2 = Vec::with_capacity(MSG2_LEN);
    msg2.extend_from_slice(&nonce_r);
    msg2.extend_from_slice(ct.as_bytes());
    msg2.extend_from_slice(&enc_r);
    debug_assert_eq!(msg2.len(), MSG2_LEN);
    s.write_all_deadline(&msg2, p.deadline)?;

    let th2 = hash::h256("LW1/p2p/th2", &[&th1, &pk_r.0, &sig_r.0]);
    let mut msg3 = alloc::vec![0u8; MSG3_LEN];
    s.read_exact_deadline(&mut msg3, p.deadline)?;
    let (pk_i, sig_i) = open_identity(&k_hs_i, &th2, &msg3, "p2p-hs-i")?;
    if &pk_i == p.own_identity {
        return Err(HsError::SelfConnect);
    }
    let th3 = hash::h256("LW1/p2p/th3", &[&th2, &pk_i.0, &sig_i.0]);
    let (i2r, r2i) = session_keys(&ss, &th3);
    drop(guard);
    Ok(Session { send: CipherState::new(r2i, DIR_R2I), recv: CipherState::new(i2r, DIR_I2R), peer: pk_i })
}
