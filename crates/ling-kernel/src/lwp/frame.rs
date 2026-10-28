//! LWP-1 frames (DESIGN §8.2). Port of ling-net `lwp/frame.rs`: the cipher
//! state is byte-for-byte the host's; only the socket I/O is abstracted
//! behind [`Io`] (a cooperative kernel socket, or a host `TcpStream` in the
//! wire-compatibility test).
//!
//! ```text
//! frame  = u32_le(L) ‖ outer                                  L = |outer|
//! outer  = DICE42(dice_dir).seal(dnonce, inner)               = dnonce(24) ‖ ct ‖ tag(32)
//! inner  = XChaCha20-Poly1305(k_dir, nonce = dir ‖ u64_le(ctr) ‖ 0^15,
//!                             aad = "LWP1/frame" ‖ dir ‖ u64_le(ctr) ‖ u32_le(L), body)
//! body   = kind:u8 ‖ u32_le(plen) ‖ payload ‖ random padding   (|body| ∈ BUCKETS)
//! dnonce = XOF("LW1/p2p/dice-nonce", nkey_dir, dir, u64_le(ctr), 24)
//! ```

use alloc::vec::Vec;

use crate::lwp::crypto::{aead, hash, kdf, Zeroizing};
use crate::lwp::sys;
use ling_crypto::MobiusHelixReactor;

pub const BUCKETS: [usize; 7] = [
    1 << 10,
    4 << 10,
    16 << 10,
    64 << 10,
    256 << 10,
    1 << 20,
    (4 << 20) + (64 << 10),
];
/// AEAD tag (16) + DICE-42 nonce (24) + DICE-42 tag (32).
pub const OVERHEAD: usize = 16 + 24 + 32;
/// kind:u8 ‖ u32 payload length.
pub const BODY_HEADER: usize = 5;
pub const MAX_PAYLOAD: usize = BUCKETS[BUCKETS.len() - 1] - BODY_HEADER;

pub const DIR_I2R: u8 = 0x01;
pub const DIR_R2I: u8 = 0x02;

pub fn bucket_for(body_len: usize) -> Option<usize> {
    BUCKETS.iter().copied().find(|&b| b >= body_len)
}

pub fn valid_wire_len(l: usize) -> bool {
    BUCKETS.iter().any(|&b| b + OVERHEAD == l)
}

/// Socket errors (the host's `io::Error`, reduced to what callers check).
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IoError {
    TimedOut,
    Eof,
    Closed,
    Other(&'static str),
}

/// A byte stream the handshake and frame reader run over. Implementations
/// must wait cooperatively (never spin without yielding).
pub trait Io {
    /// `read_exact` that fails once `deadline_ms` (see [`sys::now_ms`])
    /// passes, however slowly the peer trickles bytes (slowloris-safe).
    fn read_exact_deadline(&mut self, buf: &mut [u8], deadline_ms: u64) -> Result<(), IoError>;
    /// Write everything (gives up after `deadline_ms`).
    fn write_all_deadline(&mut self, b: &[u8], deadline_ms: u64) -> Result<(), IoError>;
}

/// One direction's key material. Zeroized on drop.
pub(crate) struct DirKeys {
    pub aead: Zeroizing<[u8; 32]>,
    pub dice: Zeroizing<[u8; 32]>,
}

pub(crate) struct CipherState {
    keys: DirKeys,
    reactor: MobiusHelixReactor,
    nkey: Zeroizing<[u8; 32]>,
    dir: u8,
    counter: u64,
    pub since_rekey: u64,
    /// `sys::now_ms()` of the last (re)key.
    pub rekeyed_at: u64,
}

#[allow(dead_code)] // fields are for Debug output
#[derive(Debug)]
pub(crate) enum FrameError {
    Io(IoError),
    Bad(&'static str),
}

impl From<IoError> for FrameError {
    fn from(e: IoError) -> Self {
        FrameError::Io(e)
    }
}

fn nkey_for(dice: &[u8; 32]) -> Zeroizing<[u8; 32]> {
    Zeroizing::new(hash::h256("LW1/p2p/dice-nonce-key", &[dice]))
}

impl CipherState {
    pub fn new(keys: DirKeys, dir: u8) -> Self {
        let reactor = MobiusHelixReactor::derive(&keys.dice);
        let nkey = nkey_for(&keys.dice);
        CipherState { keys, reactor, nkey, dir, counter: 0, since_rekey: 0, rekeyed_at: sys::now_ms() }
    }

    /// Forward-secure ratchet: new keys from old, old keys zeroized.
    pub fn ratchet(&mut self) {
        let mut out = Zeroizing::new([0u8; 64]);
        hash::xof("LW1/p2p/rekey", &[&[self.dir], &self.keys.aead[..], &self.keys.dice[..]], out.as_mut());
        let mut a = Zeroizing::new([0u8; 32]);
        let mut d = Zeroizing::new([0u8; 32]);
        a.copy_from_slice(&out[..32]);
        d.copy_from_slice(&out[32..]);
        self.reactor = MobiusHelixReactor::derive(&d);
        self.nkey = nkey_for(&d);
        self.keys = DirKeys { aead: a, dice: d };
        self.since_rekey = 0;
        self.rekeyed_at = sys::now_ms();
    }

    /// Milliseconds since the last (re)key.
    pub fn key_age_ms(&self) -> u64 {
        sys::now_ms().saturating_sub(self.rekeyed_at)
    }

    fn aead_nonce(&self) -> [u8; 24] {
        let mut n = [0u8; 24];
        n[0] = self.dir;
        n[1..9].copy_from_slice(&self.counter.to_le_bytes());
        n
    }

    fn aad(&self, l: u32) -> [u8; 23] {
        let mut a = [0u8; 23];
        a[..10].copy_from_slice(b"LWP1/frame");
        a[10] = self.dir;
        a[11..19].copy_from_slice(&self.counter.to_le_bytes());
        a[19..23].copy_from_slice(&l.to_le_bytes());
        a
    }

    fn dice_nonce(&self) -> [u8; 24] {
        let mut n = [0u8; 24];
        hash::xof("LW1/p2p/dice-nonce", &[&self.nkey[..], &[self.dir], &self.counter.to_le_bytes()], &mut n);
        n
    }

    /// Encrypt one frame (including the 4-byte length prefix).
    pub fn seal(&mut self, kind: u8, payload: &[u8]) -> Vec<u8> {
        assert!(payload.len() <= MAX_PAYLOAD, "payload size checked by caller");
        let body_len = bucket_for(BODY_HEADER + payload.len()).expect("bounded above");
        let mut body = Zeroizing::new(Vec::with_capacity(body_len));
        body.push(kind);
        body.extend_from_slice(&(payload.len() as u32).to_le_bytes());
        body.extend_from_slice(payload);
        let pad_start = body.len();
        body.resize(body_len, 0);
        kdf::random_bytes(&mut body[pad_start..]);

        let l = (body_len + OVERHEAD) as u32;
        let inner = aead::seal(&self.keys.aead, &self.aead_nonce(), &self.aad(l), &body);
        let outer = self.reactor.seal(&self.dice_nonce(), &inner);
        debug_assert_eq!(outer.len(), l as usize);
        let mut frame = Vec::with_capacity(4 + outer.len());
        frame.extend_from_slice(&l.to_le_bytes());
        frame.extend_from_slice(&outer);
        self.counter += 1;
        self.since_rekey += 1;
        frame
    }

    /// Decrypt one frame body (`outer`, already length-checked). Any failure
    /// is fatal for the connection; the counter does not advance on error.
    pub fn open(&mut self, outer: &[u8]) -> Result<(u8, Vec<u8>), FrameError> {
        if !valid_wire_len(outer.len()) {
            return Err(FrameError::Bad("frame length"));
        }
        if outer[..24] != self.dice_nonce() {
            return Err(FrameError::Bad("frame order"));
        }
        let inner = self.reactor.open(outer).map_err(|_| FrameError::Bad("outer auth"))?;
        let l = outer.len() as u32;
        let body = Zeroizing::new(
            aead::open(&self.keys.aead, &self.aead_nonce(), &self.aad(l), &inner).map_err(|_| FrameError::Bad("inner auth"))?,
        );
        if body.len() < BODY_HEADER {
            return Err(FrameError::Bad("short body"));
        }
        let kind = body[0];
        let plen = u32::from_le_bytes([body[1], body[2], body[3], body[4]]) as usize;
        if plen > body.len() - BODY_HEADER {
            return Err(FrameError::Bad("payload length"));
        }
        // Must be the smallest bucket that fits, so a peer cannot use oversized
        // frames as a side channel or amplification.
        if bucket_for(BODY_HEADER + plen) != Some(body.len()) {
            return Err(FrameError::Bad("non-minimal bucket"));
        }
        self.counter += 1;
        self.since_rekey += 1;
        Ok((kind, body[BODY_HEADER..BODY_HEADER + plen].to_vec()))
    }
}

/// Read `len` bytes, buffering incrementally so a claimed length costs
/// memory only as bytes actually arrive.
pub(crate) fn read_vec_deadline<S: Io + ?Sized>(s: &mut S, len: usize, deadline_ms: u64) -> Result<Vec<u8>, IoError> {
    let mut v = Vec::with_capacity(len.min(64 << 10));
    let mut chunk = alloc::vec![0u8; (64 << 10).min(len.max(1))];
    while v.len() < len {
        let n = (len - v.len()).min(chunk.len());
        s.read_exact_deadline(&mut chunk[..n], deadline_ms)?;
        v.extend_from_slice(&chunk[..n]);
    }
    Ok(v)
}

/// Read one raw frame: waits up to `idle_ms` for the length prefix, then up
/// to `frame_timeout_ms` for the rest. Returns the `outer` bytes.
pub(crate) fn read_frame<S: Io + ?Sized>(s: &mut S, idle_ms: u64, frame_timeout_ms: u64) -> Result<Vec<u8>, FrameError> {
    let mut hdr = [0u8; 4];
    s.read_exact_deadline(&mut hdr, sys::now_ms() + idle_ms)?;
    let l = u32::from_le_bytes(hdr) as usize;
    if !valid_wire_len(l) {
        return Err(FrameError::Bad("frame length"));
    }
    Ok(read_vec_deadline(s, l, sys::now_ms() + frame_timeout_ms)?)
}
