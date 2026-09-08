//! An in-kernel SSH **server** -- the "ssh into LingOS from another machine"
//! half -- interoperable with real OpenSSH. Built on the pieces that now
//! exist: `netstack`'s passive open (`tcp_listen_accept`), the classical
//! crypto suite (`crypto`: X25519, SHA-256), Ed25519 host-key signing
//! (`ed25519`), and raw ChaCha20 + Poly1305 (for `chacha20-poly1305@openssh.com`).
//!
//! Negotiated algorithms (the modern, pure-software OpenSSH default set, so no
//! AES-NI dependency): key exchange `curve25519-sha256`, host key `ssh-ed25519`,
//! cipher+MAC `chacha20-poly1305@openssh.com` (AEAD). One session channel, a
//! filesystem-backed built-in shell. Runs on its own netstack socket (see the
//! multi-socket TCP section), so an SSH session interleaves with other network
//! users -- a browser fetch, Messenger -- instead of monopolizing the wire.
//!
//! Authentication is **password**, verified against the user DB
//! (`users::verify`): `none` and wrong passwords are rejected, six failures
//! disconnect, no passwordless login. So a real account must exist -- the Live
//! desktop's `live`/`live`, or the root/user the installer created; a bare
//! rescue image with no accounts is intentionally unreachable. Started at boot
//! as a background task when SSH is enabled (`services::ssh_enabled`, set by the
//! installer or the first-boot prompt) -- see `start`; also runnable manually
//! via the rescue `sshd` command.
//!
//! Honest scope (v0.1), disclosed not hidden:
//! - **One connection at a time** (the shared `netstack` CONN): an active SSH
//!   session holds the single network connection, so the desktop's own net
//!   (browser/messenger) is blocked while someone is logged in. The desktop
//!   still renders (the session yields during reads); it just can't also use
//!   the network. Multi-socket support is future work.
//! - The shell is a small built-in command set (help/echo/whoami/uname/uptime/
//!   exit), not yet the full `lsh` (whose console I/O would need redirecting
//!   onto the channel). The point reached is a *real* encrypted, authenticated,
//!   interactive session against an unmodified `ssh` client.

use crate::drivers::netstack;
use crate::fs::{lingfs, users};
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use chacha20::cipher::{KeyIvInit, StreamCipher};
use chacha20::ChaCha20Legacy;
use poly1305::universal_hash::{KeyInit, UniversalHash};
use poly1305::Poly1305;

pub const SSH_PORT: u16 = 22;
const IDENT: &[u8] = b"SSH-2.0-LingOS_0.1";

// SSH message numbers (RFC 4253/4252/4254).
const MSG_DISCONNECT: u8 = 1;
const MSG_IGNORE: u8 = 2;
const MSG_UNIMPLEMENTED: u8 = 3;
const MSG_DEBUG: u8 = 4;
const MSG_SERVICE_REQUEST: u8 = 5;
const MSG_SERVICE_ACCEPT: u8 = 6;
const MSG_KEXINIT: u8 = 20;
const MSG_NEWKEYS: u8 = 21;
const MSG_KEX_ECDH_INIT: u8 = 30;
const MSG_KEX_ECDH_REPLY: u8 = 31;
const MSG_USERAUTH_REQUEST: u8 = 50;
const MSG_USERAUTH_FAILURE: u8 = 51;
const MSG_USERAUTH_SUCCESS: u8 = 52;
const MSG_GLOBAL_REQUEST: u8 = 80;
const MSG_REQUEST_FAILURE: u8 = 82;
const MSG_CHANNEL_OPEN: u8 = 90;
const MSG_CHANNEL_OPEN_CONFIRMATION: u8 = 91;
const MSG_CHANNEL_WINDOW_ADJUST: u8 = 93;
const MSG_CHANNEL_DATA: u8 = 94;
const MSG_CHANNEL_EOF: u8 = 96;
const MSG_CHANNEL_CLOSE: u8 = 97;
const MSG_CHANNEL_REQUEST: u8 = 98;
const MSG_CHANNEL_SUCCESS: u8 = 99;

// ── SSH wire encoding helpers ────────────────────────────────────────────────

fn put_u32(out: &mut Vec<u8>, v: u32) {
    out.extend_from_slice(&v.to_be_bytes());
}
fn put_string(out: &mut Vec<u8>, s: &[u8]) {
    put_u32(out, s.len() as u32);
    out.extend_from_slice(s);
}
/// `mpint`: a big-endian integer, minimal, with a leading 0x00 if the top bit
/// of the first byte is set (so it reads as non-negative). `val` is the raw
/// fixed-width big-endian bytes (e.g. the 32-byte X25519 shared secret).
fn put_mpint(out: &mut Vec<u8>, val: &[u8]) {
    let mut i = 0;
    while i < val.len() && val[i] == 0 {
        i += 1;
    }
    let sig = &val[i..];
    if sig.is_empty() {
        put_u32(out, 0);
    } else if sig[0] & 0x80 != 0 {
        put_u32(out, (sig.len() + 1) as u32);
        out.push(0);
        out.extend_from_slice(sig);
    } else {
        put_u32(out, sig.len() as u32);
        out.extend_from_slice(sig);
    }
}

/// Read a `uint32`-length-prefixed string from `buf` at `*pos`, advancing it.
fn get_string<'a>(buf: &'a [u8], pos: &mut usize) -> Option<&'a [u8]> {
    if *pos + 4 > buf.len() {
        return None;
    }
    let len = u32::from_be_bytes([buf[*pos], buf[*pos + 1], buf[*pos + 2], buf[*pos + 3]]) as usize;
    *pos += 4;
    if *pos + len > buf.len() {
        return None;
    }
    let s = &buf[*pos..*pos + len];
    *pos += len;
    Some(s)
}

/// The `ssh-ed25519` public-key blob: `string("ssh-ed25519") ‖ string(pk32)`.
fn host_key_blob(pk: &[u8; 32]) -> Vec<u8> {
    let mut v = Vec::new();
    put_string(&mut v, b"ssh-ed25519");
    put_string(&mut v, pk);
    v
}

/// Persisted 32-byte Ed25519 host-key seed (stable across boots so the client
/// fingerprint doesn't change). Generated from RDRAND on first use.
fn host_seed() -> [u8; 32] {
    let mut buf = [0u8; 64];
    if let Ok(Some(n)) = lingfs::read_file_all("ssh_host_seed", &mut buf) {
        if n >= 32 {
            let mut s = [0u8; 32];
            s.copy_from_slice(&buf[..32]);
            return s;
        }
    }
    let mut s = [0u8; 32];
    crate::crypto::random_bytes(&mut s);
    let _ = lingfs::write_file("ssh_host_seed", &s);
    s
}

// ── chacha20-poly1305@openssh.com ────────────────────────────────────────────
// Two 256-bit keys: K_2 (main, key[0..32]) encrypts the packet body, K_1
// (header, key[32..64]) encrypts the 4-byte length separately. The nonce is the
// 64-bit packet sequence number, big-endian. Poly1305's key is the first 32
// bytes of K_2's keystream at block 0; the body is encrypted from block 1.

fn chacha_apply(key: &[u8], seq: u32, skip_blocks: bool, data: &mut [u8]) {
    let nonce = (seq as u64).to_be_bytes();
    let mut c = ChaCha20Legacy::new_from_slices(key, &nonce).expect("32-byte key, 8-byte nonce");
    if skip_blocks {
        // Advance past block 0 (used only for the poly1305 key) so the body is
        // encrypted starting at block 1, matching OpenSSH.
        let mut skip = [0u8; 64];
        c.apply_keystream(&mut skip);
    }
    c.apply_keystream(data);
}

fn poly_key(k_main: &[u8], seq: u32) -> [u8; 32] {
    let nonce = (seq as u64).to_be_bytes();
    let mut c = ChaCha20Legacy::new_from_slices(k_main, &nonce).expect("key/nonce");
    let mut pk = [0u8; 32];
    c.apply_keystream(&mut pk);
    pk
}

fn poly_tag(k_main: &[u8], seq: u32, ciphertext: &[u8]) -> [u8; 16] {
    let pk = poly_key(k_main, seq);
    let mac = Poly1305::new_from_slice(&pk).expect("32-byte poly key");
    let tag = mac.compute_unpadded(ciphertext);
    let mut out = [0u8; 16];
    out.copy_from_slice(&tag);
    out
}

// ── Session ──────────────────────────────────────────────────────────────────

struct Session {
    handle: usize,     // netstack socket handle for this connection
    rx: Vec<u8>,       // received bytes not yet consumed
    send_seq: u32,
    recv_seq: u32,
    enc_out: bool,     // encrypt outgoing (after we send NEWKEYS)
    enc_in: bool,      // decrypt incoming (after we receive NEWKEYS)
    k_s2c: [u8; 64],   // server->client key (main[0..32] || header[32..64])
    k_c2s: [u8; 64],   // client->server key
}

impl Session {
    fn new(handle: usize) -> Session {
        Session {
            handle,
            rx: Vec::new(),
            send_seq: 0,
            recv_seq: 0,
            enc_out: false,
            enc_in: false,
            k_s2c: [0; 64],
            k_c2s: [0; 64],
        }
    }

    /// Ensure at least `n` bytes are buffered, reading from TCP with a bounded
    /// idle tolerance. Returns false if the peer went silent.
    fn ensure(&mut self, n: usize) -> bool {
        let mut idle = 0;
        while self.rx.len() < n {
            let mut tmp = [0u8; 4096];
            let got = netstack::tcp_read_h(self.handle, &mut tmp, 2_000_000);
            if got == 0 {
                idle += 1;
                if idle > 8 {
                    return false;
                }
                continue;
            }
            idle = 0;
            self.rx.extend_from_slice(&tmp[..got]);
        }
        true
    }

    /// Receive one packet, returning its payload (the message body, i.e. the
    /// byte after padding_length onward). Handles cleartext and encrypted.
    fn recv(&mut self) -> Option<Vec<u8>> {
        if self.enc_in {
            // Decrypt the 4-byte length with the header key (nonce = recv_seq).
            if !self.ensure(4) {
                return None;
            }
            let mut len4 = [self.rx[0], self.rx[1], self.rx[2], self.rx[3]];
            chacha_apply(&self.k_c2s[32..64], self.recv_seq, false, &mut len4);
            let pkt_len = u32::from_be_bytes(len4) as usize;
            if pkt_len < 8 || pkt_len > 65536 {
                return None;
            }
            if !self.ensure(4 + pkt_len + 16) {
                return None;
            }
            // Verify Poly1305 over the encrypted (length || body).
            let tag = poly_tag(&self.k_c2s[0..32], self.recv_seq, &self.rx[0..4 + pkt_len]);
            if tag[..] != self.rx[4 + pkt_len..4 + pkt_len + 16] {
                return None; // bad MAC
            }
            // Decrypt the body (block 1 onward).
            let mut body = self.rx[4..4 + pkt_len].to_vec();
            chacha_apply(&self.k_c2s[0..32], self.recv_seq, true, &mut body);
            let padlen = body[0] as usize;
            if 1 + padlen > body.len() {
                return None;
            }
            let payload = body[1..body.len() - padlen].to_vec();
            self.rx.drain(0..4 + pkt_len + 16);
            self.recv_seq = self.recv_seq.wrapping_add(1);
            Some(payload)
        } else {
            if !self.ensure(4) {
                return None;
            }
            let pkt_len =
                u32::from_be_bytes([self.rx[0], self.rx[1], self.rx[2], self.rx[3]]) as usize;
            if pkt_len < 8 || pkt_len > 65536 {
                return None;
            }
            if !self.ensure(4 + pkt_len) {
                return None;
            }
            let padlen = self.rx[4] as usize;
            if 5 + (pkt_len - 1 - padlen) > 4 + pkt_len {
                return None;
            }
            let payload = self.rx[5..4 + pkt_len - padlen].to_vec();
            self.rx.drain(0..4 + pkt_len);
            self.recv_seq = self.recv_seq.wrapping_add(1);
            Some(payload)
        }
    }

    /// Frame and send one payload as an SSH packet (cleartext or encrypted).
    fn send(&mut self, payload: &[u8]) -> bool {
        let ok = if self.enc_out {
            // padding aligns (padlen_byte + payload + padding) to 8, min 4; the
            // separately-encrypted length field is excluded from the modulus.
            let mut padlen = 8 - ((1 + payload.len()) % 8);
            if padlen < 4 {
                padlen += 8;
            }
            let pkt_len = 1 + payload.len() + padlen;
            let mut body = Vec::with_capacity(pkt_len);
            body.push(padlen as u8);
            body.extend_from_slice(payload);
            let mut pad = alloc::vec![0u8; padlen];
            let _ = crate::crypto::random_bytes(&mut pad);
            body.extend_from_slice(&pad);
            // Encrypt length (header key) and body (main key, block 1+).
            let mut len4 = (pkt_len as u32).to_be_bytes();
            chacha_apply(&self.k_s2c[32..64], self.send_seq, false, &mut len4);
            chacha_apply(&self.k_s2c[0..32], self.send_seq, true, &mut body);
            let mut wire = Vec::with_capacity(4 + pkt_len + 16);
            wire.extend_from_slice(&len4);
            wire.extend_from_slice(&body);
            let tag = poly_tag(&self.k_s2c[0..32], self.send_seq, &wire);
            wire.extend_from_slice(&tag);
            netstack::tcp_write_h(self.handle, &wire)
        } else {
            // include the 4-byte length field in the block-8 alignment.
            let mut padlen = 8 - ((4 + 1 + payload.len()) % 8);
            if padlen < 4 {
                padlen += 8;
            }
            let pkt_len = 1 + payload.len() + padlen;
            let mut wire = Vec::with_capacity(4 + pkt_len);
            put_u32(&mut wire, pkt_len as u32);
            wire.push(padlen as u8);
            wire.extend_from_slice(payload);
            let mut pad = alloc::vec![0u8; padlen];
            let _ = crate::crypto::random_bytes(&mut pad);
            wire.extend_from_slice(&pad);
            netstack::tcp_write_h(self.handle, &wire)
        };
        if ok {
            self.send_seq = self.send_seq.wrapping_add(1);
        }
        ok
    }
}

/// Build our SSH_MSG_KEXINIT payload (the algorithm-negotiation message body).
fn build_kexinit() -> Vec<u8> {
    let mut p = Vec::new();
    p.push(MSG_KEXINIT);
    let mut cookie = [0u8; 16];
    let _ = crate::crypto::random_bytes(&mut cookie);
    p.extend_from_slice(&cookie);
    put_string(&mut p, b"curve25519-sha256,curve25519-sha256@libssh.org");
    put_string(&mut p, b"ssh-ed25519");
    put_string(&mut p, b"chacha20-poly1305@openssh.com");
    put_string(&mut p, b"chacha20-poly1305@openssh.com");
    put_string(&mut p, b"hmac-sha2-256"); // ignored: chachapoly is AEAD
    put_string(&mut p, b"hmac-sha2-256");
    put_string(&mut p, b"none");
    put_string(&mut p, b"none");
    put_string(&mut p, b""); // languages c2s
    put_string(&mut p, b""); // languages s2c
    p.push(0); // first_kex_packet_follows
    put_u32(&mut p, 0); // reserved
    p
}

fn kdf(k_mpint: &[u8], h: &[u8; 32], letter: u8, sid: &[u8; 32], out_len: usize) -> [u8; 64] {
    let mut input = Vec::new();
    input.extend_from_slice(k_mpint);
    input.extend_from_slice(h);
    input.push(letter);
    input.extend_from_slice(sid);
    let mut key: Vec<u8> = crate::crypto::sha256(&input).to_vec();
    while key.len() < out_len {
        let mut inp2 = Vec::new();
        inp2.extend_from_slice(k_mpint);
        inp2.extend_from_slice(h);
        inp2.extend_from_slice(&key);
        key.extend_from_slice(&crate::crypto::sha256(&inp2));
    }
    let _ = out_len;
    let mut out = [0u8; 64];
    out.copy_from_slice(&key[..64]);
    out
}

fn log(msg: &[u8]) {
    crate::console_write(msg);
}

/// Read the client's SSH version line, keeping any bytes after the CR/LF (the
/// client's first packet) in the session RX buffer. Returns V_C (no CR/LF).
fn read_version(sess: &mut Session) -> Option<Vec<u8>> {
    loop {
        // Find a LF in whatever's buffered.
        if let Some(nl) = sess.rx.iter().position(|&b| b == b'\n') {
            let mut line = sess.rx[..nl].to_vec();
            if line.last() == Some(&b'\r') {
                line.pop();
            }
            sess.rx.drain(0..nl + 1);
            return Some(line);
        }
        if !sess.ensure(sess.rx.len() + 1) {
            return None;
        }
    }
}

/// Serve one inbound SSH connection to completion (up to `budget_us` for the
/// accept). Returns Ok(()) on a clean session, or an error string.
pub fn serve(budget_us: u64) -> Result<(), &'static str> {
    let handle = netstack::tcp_listen_accept(SSH_PORT, budget_us).ok_or("no client connected")?;
    log(b"sshd: client connected -- starting SSH transport\n");

    let mut sess = Session::new(handle);

    // 1. Version exchange.
    let mut v_s = Vec::new();
    v_s.extend_from_slice(IDENT);
    let mut ident_line = v_s.clone();
    ident_line.extend_from_slice(b"\r\n");
    if !netstack::tcp_write_h(handle, &ident_line) {
        netstack::tcp_close_h(handle);
        return Err("write version failed");
    }
    let Some(v_c) = read_version(&mut sess) else {
        netstack::tcp_close_h(handle);
        return Err("no client version");
    };
    log(b"sshd: client version: ");
    log(&v_c);
    log(b"\n");

    // 2. KEXINIT exchange.
    let i_s = build_kexinit();
    if !sess.send(&i_s) {
        netstack::tcp_close_h(handle);
        return Err("send KEXINIT failed");
    }
    let Some(i_c) = sess.recv() else {
        netstack::tcp_close_h(handle);
        return Err("no client KEXINIT");
    };
    if i_c.first() != Some(&MSG_KEXINIT) {
        netstack::tcp_close_h(handle);
        return Err("expected KEXINIT");
    }
    log(b"sshd: KEXINIT exchanged\n");

    // 3. curve25519-sha256 key exchange.
    let Some(ecdh) = sess.recv() else {
        netstack::tcp_close_h(handle);
        return Err("no KEX_ECDH_INIT");
    };
    if ecdh.first() != Some(&MSG_KEX_ECDH_INIT) {
        netstack::tcp_close_h(handle);
        return Err("expected KEX_ECDH_INIT");
    }
    let mut pos = 1usize;
    let Some(q_c) = get_string(&ecdh, &mut pos) else {
        netstack::tcp_close_h(handle);
        return Err("bad ECDH_INIT");
    };
    if q_c.len() != 32 {
        netstack::tcp_close_h(handle);
        return Err("bad client eph key");
    }
    let mut q_c_arr = [0u8; 32];
    q_c_arr.copy_from_slice(q_c);

    // Our ephemeral X25519 keypair + shared secret.
    let mut eph_secret = [0u8; 32];
    if !crate::crypto::random_bytes(&mut eph_secret) {
        netstack::tcp_close_h(handle);
        return Err("no RDRAND");
    }
    let q_s = crate::crypto::x25519_public(eph_secret);
    let k = crate::crypto::x25519(eph_secret, q_c_arr);

    // Host key + exchange hash H.
    let seed = host_seed();
    let host_pub = crate::ed25519::public_from_seed(&seed);
    let k_s = host_key_blob(&host_pub);
    let mut hbuf = Vec::new();
    put_string(&mut hbuf, &v_c);
    put_string(&mut hbuf, &v_s);
    put_string(&mut hbuf, &i_c);
    put_string(&mut hbuf, &i_s);
    put_string(&mut hbuf, &k_s);
    put_string(&mut hbuf, &q_c_arr);
    put_string(&mut hbuf, &q_s);
    put_mpint(&mut hbuf, &k);
    let h = crate::crypto::sha256(&hbuf);
    let sig = crate::ed25519::sign(&seed, &h);
    let mut sig_blob = Vec::new();
    put_string(&mut sig_blob, b"ssh-ed25519");
    put_string(&mut sig_blob, &sig);

    // KEX_ECDH_REPLY.
    let mut reply = Vec::new();
    reply.push(MSG_KEX_ECDH_REPLY);
    put_string(&mut reply, &k_s);
    put_string(&mut reply, &q_s);
    put_string(&mut reply, &sig_blob);
    if !sess.send(&reply) {
        netstack::tcp_close_h(handle);
        return Err("send ECDH_REPLY failed");
    }

    // 4. NEWKEYS. Derive directional keys; session_id = H (first kex).
    let mut k_mpint = Vec::new();
    put_mpint(&mut k_mpint, &k);
    let k_c2s = kdf(&k_mpint, &h, b'C', &h, 64);
    let k_s2c = kdf(&k_mpint, &h, b'D', &h, 64);

    if !sess.send(&[MSG_NEWKEYS]) {
        netstack::tcp_close_h(handle);
        return Err("send NEWKEYS failed");
    }
    sess.enc_out = true;
    sess.k_s2c = k_s2c;
    sess.k_c2s = k_c2s;
    let Some(nk) = sess.recv() else {
        netstack::tcp_close_h(handle);
        return Err("no client NEWKEYS");
    };
    if nk.first() != Some(&MSG_NEWKEYS) {
        netstack::tcp_close_h(handle);
        return Err("expected NEWKEYS");
    }
    sess.enc_in = true;
    log(b"sshd: key exchange complete -- encrypted channel up\n");

    // 5. Password userauth, verified against the user DB (`users::verify`). A
    // `none` attempt or a wrong password gets USERAUTH_FAILURE offering
    // `password`; six failures disconnect. So an account with a real password
    // must exist (the Live desktop's `live`/`live`, or the root/user the
    // installer created) -- a passwordless login is not offered.
    let mut username = String::new();
    let mut attempts = 0u32;
    loop {
        let Some(msg) = sess.recv() else {
            netstack::tcp_close_h(handle);
            return Err("disconnected during auth");
        };
        match msg.first().copied() {
            Some(MSG_SERVICE_REQUEST) => {
                let mut p = 1;
                let name = get_string(&msg, &mut p).unwrap_or(b"");
                let mut r = Vec::new();
                r.push(MSG_SERVICE_ACCEPT);
                put_string(&mut r, name);
                sess.send(&r);
            },
            Some(MSG_USERAUTH_REQUEST) => {
                let mut p = 1;
                let user = get_string(&msg, &mut p).unwrap_or(b"").to_vec();
                let _service = get_string(&msg, &mut p);
                let method = get_string(&msg, &mut p).unwrap_or(b"").to_vec();
                let uname = core::str::from_utf8(&user).unwrap_or("");
                if method == b"password" {
                    let mut pp = p + 1; // skip the boolean FALSE
                    let pw = get_string(&msg, &mut pp).unwrap_or(b"");
                    let pws = core::str::from_utf8(pw).unwrap_or("");
                    if !pws.is_empty() && users::verify(uname, pws) {
                        username = uname.to_string();
                        sess.send(&[MSG_USERAUTH_SUCCESS]);
                        log(b"sshd: password auth OK for ");
                        log(&user);
                        log(b"\n");
                        break;
                    }
                }
                attempts += 1;
                let mut r = Vec::new();
                r.push(MSG_USERAUTH_FAILURE);
                put_string(&mut r, b"password");
                r.push(0); // partial success = false
                sess.send(&r);
                if attempts >= 6 {
                    netstack::tcp_close_h(handle);
                    return Err("too many auth failures");
                }
            },
            Some(MSG_IGNORE) | Some(MSG_DEBUG) => {},
            Some(MSG_GLOBAL_REQUEST) => {
                sess.send(&[MSG_REQUEST_FAILURE]);
            },
            Some(MSG_DISCONNECT) => {
                netstack::tcp_close_h(handle);
                return Ok(());
            },
            _ => {},
        }
    }

    // 6. Session channel + interactive shell.
    run_channel(&mut sess, &username);
    netstack::tcp_close_h(handle);
    log(b"sshd: session closed\n");
    Ok(())
}

/// Handle the connection layer: accept a `session` channel, its pty-req/shell
/// requests, then run a small interactive shell over CHANNEL_DATA.
fn run_channel(sess: &mut Session, username: &str) {
    let mut peer_chan: u32 = 0;
    let my_chan: u32 = 0;
    let mut have_channel = false;
    let mut line: Vec<u8> = Vec::new();
    let mut banner_sent = false;
    // Per-session working directory (lingfs path, no leading slash; "" = root).
    // Kept local so an SSH session doesn't move the console shell's CWD.
    let mut cwd = String::new();

    loop {
        let Some(msg) = sess.recv() else { return };
        match msg.first().copied() {
            Some(MSG_CHANNEL_OPEN) => {
                let mut p = 1;
                let ctype = get_string(&msg, &mut p).unwrap_or(b"");
                if p + 4 <= msg.len() {
                    peer_chan =
                        u32::from_be_bytes([msg[p], msg[p + 1], msg[p + 2], msg[p + 3]]);
                }
                if ctype == b"session" {
                    let mut r = Vec::new();
                    r.push(MSG_CHANNEL_OPEN_CONFIRMATION);
                    put_u32(&mut r, peer_chan);
                    put_u32(&mut r, my_chan);
                    put_u32(&mut r, 0x0010_0000); // window
                    put_u32(&mut r, 0x0000_8000); // max packet
                    sess.send(&r);
                    have_channel = true;
                }
            },
            Some(MSG_CHANNEL_REQUEST) => {
                // uint32 chan, string type, bool want_reply, ...
                let mut p = 1 + 4;
                let rtype = get_string(&msg, &mut p).unwrap_or(b"").to_vec();
                let want_reply = msg.get(p).copied().unwrap_or(0) != 0;
                if want_reply {
                    let mut r = Vec::new();
                    r.push(MSG_CHANNEL_SUCCESS);
                    put_u32(&mut r, peer_chan);
                    sess.send(&r);
                }
                if (rtype == b"shell" || rtype == b"exec") && have_channel && !banner_sent {
                    banner_sent = true;
                    channel_write(sess, peer_chan, b"\r\n  LingOS SSH -- welcome, ");
                    channel_write(sess, peer_chan, username.as_bytes());
                    channel_write(sess, peer_chan, b"\r\n  type 'help' for commands\r\n");
                    send_prompt(sess, peer_chan, &cwd);
                }
            },
            Some(MSG_CHANNEL_DATA) => {
                let mut p = 1 + 4;
                let data = get_string(&msg, &mut p).unwrap_or(b"").to_vec();
                for &b in &data {
                    if b == b'\r' || b == b'\n' {
                        channel_write(sess, peer_chan, b"\r\n");
                        let cmd = core::str::from_utf8(&line).unwrap_or("").trim().to_string();
                        line.clear();
                        if run_cmd(sess, peer_chan, &cmd, username, &mut cwd) {
                            // exit requested
                            let mut eof = Vec::new();
                            eof.push(MSG_CHANNEL_EOF);
                            put_u32(&mut eof, peer_chan);
                            sess.send(&eof);
                            let mut cl = Vec::new();
                            cl.push(MSG_CHANNEL_CLOSE);
                            put_u32(&mut cl, peer_chan);
                            sess.send(&cl);
                            return;
                        }
                        send_prompt(sess, peer_chan, &cwd);
                    } else if b == 0x7f || b == 0x08 {
                        if line.pop().is_some() {
                            channel_write(sess, peer_chan, b"\x08 \x08");
                        }
                    } else if b >= 0x20 && b < 0x7f {
                        line.push(b);
                        channel_write(sess, peer_chan, &[b]); // echo
                    }
                }
            },
            Some(MSG_CHANNEL_EOF) => {},
            Some(MSG_CHANNEL_CLOSE) => {
                let mut cl = Vec::new();
                cl.push(MSG_CHANNEL_CLOSE);
                put_u32(&mut cl, peer_chan);
                sess.send(&cl);
                return;
            },
            Some(MSG_CHANNEL_WINDOW_ADJUST) => {},
            Some(MSG_GLOBAL_REQUEST) => {
                sess.send(&[MSG_REQUEST_FAILURE]);
            },
            Some(MSG_DISCONNECT) => return,
            _ => {},
        }
    }
}

/// Write channel data, split into <=1024-byte pieces so each SSH packet stays
/// under one TCP segment (`netstack::tcp_write` is single-segment).
fn channel_write(sess: &mut Session, chan: u32, data: &[u8]) {
    let mut off = 0usize;
    while off < data.len() {
        let end = (off + 1024).min(data.len());
        let mut r = Vec::new();
        r.push(MSG_CHANNEL_DATA);
        put_u32(&mut r, chan);
        put_string(&mut r, &data[off..end]);
        sess.send(&r);
        off = end;
    }
}

/// Like `channel_write` but converts bare `\n` to `\r\n` so file contents (which
/// use Unix line endings) render correctly on the client's pty.
fn channel_write_text(sess: &mut Session, chan: u32, data: &[u8]) {
    let mut out = Vec::with_capacity(data.len() + 16);
    for &b in data {
        match b {
            b'\r' => {},
            b'\n' => {
                out.push(b'\r');
                out.push(b'\n');
            },
            _ => out.push(b),
        }
    }
    channel_write(sess, chan, &out);
}

fn send_prompt(sess: &mut Session, chan: u32, cwd: &str) {
    let mut p = String::from("lingos:/");
    p.push_str(cwd);
    p.push_str("$ ");
    channel_write(sess, chan, p.as_bytes());
}

/// Join a path argument against the session CWD into a lingfs path (no leading
/// slash; "" is root). An absolute `/x` argument resets to root-relative.
fn join_path(cwd: &str, arg: &str) -> String {
    if let Some(abs) = arg.strip_prefix('/') {
        return abs.to_string();
    }
    if arg.is_empty() {
        return cwd.to_string();
    }
    if cwd.is_empty() {
        arg.to_string()
    } else {
        let mut s = String::from(cwd);
        s.push('/');
        s.push_str(arg);
        s
    }
}

/// Run one shell command over the channel against the real lingfs. A genuine
/// (Rust-side) shell sharing the kernel's filesystem and user DB -- not the
/// `.ling` lsh binary (whose VGA/keyboard console I/O would need redirecting
/// onto the channel first), but real navigation and inspection. Returns true if
/// the session should end (`exit`).
fn run_cmd(sess: &mut Session, chan: u32, cmd: &str, username: &str, cwd: &mut String) -> bool {
    if cmd.is_empty() {
        return false;
    }
    let (name, arg) = match cmd.find(' ') {
        Some(i) => (&cmd[..i], cmd[i + 1..].trim()),
        None => (cmd, ""),
    };
    match name {
        "exit" | "logout" | "quit" => {
            channel_write(sess, chan, b"bye\r\n");
            return true;
        },
        "help" => {
            channel_write(sess, chan, b"commands:\r\n");
            channel_write(sess, chan, b"  ls [dir]   cat <file>   cd <dir>   pwd\r\n");
            channel_write(sess, chan, b"  echo <t>   whoami   hostname   uname   uptime   free\r\n");
            channel_write(sess, chan, b"  clear   net <host>   help   exit\r\n");
        },
        "ls" => {
            let path = join_path(cwd, arg);
            let mut i = 0usize;
            let mut any = false;
            loop {
                let mut nb = [0u8; 64];
                let Some((n, is_dir)) = lingfs::list_entry(&path, i, &mut nb) else { break };
                i += 1;
                any = true;
                let mut lineb = Vec::with_capacity(n + 3);
                lineb.extend_from_slice(&nb[..n]);
                if is_dir {
                    lineb.push(b'/');
                }
                lineb.extend_from_slice(b"\r\n");
                channel_write(sess, chan, &lineb);
                if i > 512 {
                    break;
                }
            }
            if !any {
                channel_write(sess, chan, b"(empty)\r\n");
            }
        },
        "cat" => {
            if arg.is_empty() {
                channel_write(sess, chan, b"usage: cat <file>\r\n");
            } else {
                let path = join_path(cwd, arg);
                let mut buf = alloc::vec![0u8; 256 * 1024];
                match lingfs::read_file_all(&path, &mut buf) {
                    Ok(Some(n)) => channel_write_text(sess, chan, &buf[..n]),
                    _ => {
                        let mut s = String::from("cat: ");
                        s.push_str(arg);
                        s.push_str(": no such file\r\n");
                        channel_write(sess, chan, s.as_bytes());
                    },
                }
            }
        },
        "cd" => {
            let new = if arg.is_empty() || arg == "/" || arg == "~" {
                String::new()
            } else if arg == ".." {
                match cwd.rfind('/') {
                    Some(i) => cwd[..i].to_string(),
                    None => String::new(),
                }
            } else {
                join_path(cwd, arg)
            };
            if new.is_empty() {
                *cwd = new;
            } else {
                let mut nb = [0u8; 64];
                if lingfs::list_entry(&new, 0, &mut nb).is_some() {
                    *cwd = new;
                } else {
                    let mut s = String::from("cd: ");
                    s.push_str(arg);
                    s.push_str(": no such directory\r\n");
                    channel_write(sess, chan, s.as_bytes());
                }
            }
        },
        "pwd" => {
            let mut s = String::from("/");
            s.push_str(cwd);
            s.push_str("\r\n");
            channel_write(sess, chan, s.as_bytes());
        },
        "echo" => {
            channel_write(sess, chan, arg.as_bytes());
            channel_write(sess, chan, b"\r\n");
        },
        "whoami" => {
            channel_write(sess, chan, username.as_bytes());
            channel_write(sess, chan, b"\r\n");
        },
        "hostname" => {
            let mut b = [0u8; lingfs::BLOCK_SIZE];
            match lingfs::read_file("/hostname", &mut b) {
                Ok(Some(n)) => {
                    channel_write(sess, chan, &b[..n]);
                    channel_write(sess, chan, b"\r\n");
                },
                _ => channel_write(sess, chan, b"lingos\r\n"),
            }
        },
        "uname" => {
            channel_write(sess, chan, b"LingOS x86_64 (a from-scratch OS written in Ling)\r\n");
        },
        "uptime" => {
            let secs = crate::arch::timer::now_ms() / 1000;
            let mut s = String::from("up ");
            push_u64(&mut s, secs);
            s.push_str("s\r\n");
            channel_write(sess, chan, s.as_bytes());
        },
        "free" => {
            let kib = crate::mm::frame::free_frame_count() * 4;
            let mut s = String::from("free: ");
            push_u64(&mut s, kib as u64);
            s.push_str(" KiB\r\n");
            channel_write(sess, chan, s.as_bytes());
        },
        "clear" => {
            channel_write(sess, chan, b"\x1b[2J\x1b[H");
        },
        "net" => {
            // Concurrency probe: open a CLIENT TCP connection (a separate
            // netstack socket) while this SSH server session is live. With the
            // old single-connection stack this would have torn the session
            // down; with multi-socket it coexists, and the shell keeps working.
            if arg.is_empty() {
                channel_write(sess, chan, b"usage: net <host>  (opens a client TCP conn to <host>:443)\r\n");
            } else {
                match netstack::dns_resolve(arg) {
                    Some(ip) => {
                        let ok = netstack::tcp_connect(ip, 443);
                        netstack::tcp_close();
                        if ok {
                            channel_write(sess, chan, b"net: client socket connected to :443 -- this SSH session is still alive\r\n");
                        } else {
                            channel_write(sess, chan, b"net: client connect failed, but this SSH session survived it\r\n");
                        }
                    },
                    None => channel_write(sess, chan, b"net: could not resolve host\r\n"),
                }
            }
        },
        _ => {
            let mut s = String::from(name);
            s.push_str(": command not found (try 'help')\r\n");
            channel_write(sess, chan, s.as_bytes());
        },
    }
    false
}

// ── Boot-time background listener ────────────────────────────────────────────

static mut SSHD_STARTED: bool = false;

/// Cooperative background task (same model as `pkgman::icon_task`): loop
/// accepting SSH connections. It does NOT hold the net lock across the session
/// -- the netstack now takes the lock only briefly per read/write/pump (see the
/// multi-socket TCP section), so an SSH session interleaves with other network
/// users (a browser fetch, Messenger) on their own sockets instead of blocking
/// them. Each iteration listens with a short accept window, runs the session if
/// a client arrives (yielding during reads so the desktop keeps rendering),
/// then yields.
extern "C" fn sshd_task() {
    loop {
        // 400ms accept window per idle iteration; a real SYN completes the
        // handshake + session inside this call. Errors (incl. "no client") are
        // silent here -- only a real connection logs (see `serve`).
        let _ = serve(400_000);
        crate::proc::sched::yield_now();
    }
}

/// Start the SSH server as a background task, once. Call at boot when SSH is
/// enabled (`services::ssh_enabled`), after the NIC is up and the scheduler's
/// main task exists.
pub fn start() {
    unsafe {
        if SSHD_STARTED {
            return;
        }
        SSHD_STARTED = true;
    }
    crate::console_write(b"sshd: SSH server enabled -- listening on port 22\n");
    crate::proc::sched::spawn(sshd_task as *const () as usize as u64);
}

fn push_u64(s: &mut String, mut v: u64) {
    if v == 0 {
        s.push('0');
        return;
    }
    let mut buf = [0u8; 20];
    let mut i = 20;
    while v > 0 {
        i -= 1;
        buf[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    s.push_str(core::str::from_utf8(&buf[i..]).unwrap_or(""));
}
