//! Horizon's vault: saved logins, addresses, phone numbers and 2FA keys,
//! in one encrypted file per account, `home/<user>/.horizon-vault`.
//!
//! Nothing in it is readable without the account password typed *for that
//! read*: the browser asks for it each time it fills or saves a form, and the
//! kernel checks it with [`crate::fs::users::verify`] before deriving any key.
//! A signed-in session alone (an open keyring, a page that got code running)
//! is not enough, and neither is the password alone (a copied disk): the
//! file key needs both.
//!
//! The keys, from one password entry:
//!
//! - `pw`   = Argon2id(salt, "horizon-vault:" + password), the same 16 MiB,
//!   t=3 cost as the login hash, with its own salt and prefix;
//! - `ring` = the keyring's purpose key `horizon-vault` (HKDF of the random
//!   master key that only opens at sign-in);
//! - `root` = HKDF-SHA3-256(pw || ring, salt, "horizon-vault v1"), and from it
//!   by label: the XChaCha20-Poly1305 key, the DICE-42 reactor seed and the
//!   ML-DSA-87 signing seed.
//!
//! The file, `HZV1 || salt(32) || pk_hash(32) || sig_len(4, LE) || sig ||
//! body`, where `body` = DICE-42 seal (the Möbius-Helix cascade,
//! [`ling_crypto::MobiusHelixReactor`]) of the XChaCha20-Poly1305 ciphertext
//! of the vault text (encrypt-then-encrypt under independent keys), and
//! `sig` is an ML-DSA-87 signature over everything before it and the body.
//! Opening checks, in order: the password, the public-key hash (a wrong key
//! stops here), the signature, the DICE-42 tag, the AEAD tag.
//!
//! Wrong passwords are counted: after three in a row each further try waits
//! 30 s, doubling to 15 minutes, until the right one.

use alloc::string::String;
use alloc::vec::Vec;

use ling_crypto::{Blake3, MlDsa87Keypair, MobiusHelixReactor, XChaCha20};

const MAGIC: &[u8; 4] = b"HZV1";
const PREFIX: &[u8] = b"horizon-vault:";

static mut FAILS: u32 = 0;
static mut WAIT_UNTIL: u64 = 0;
static mut ERR: &str = "";

fn path_for(user: &str) -> String {
    alloc::format!("home/{user}/.horizon-vault")
}

fn now() -> u64 {
    crate::clock::unix_now()
}

pub fn error() -> &'static str {
    unsafe { ERR }
}

fn fail(msg: &'static str) -> Option<String> {
    unsafe { ERR = msg };
    None
}

/// For the browser: "locked" (nobody signed in), "wait:<s>" (too many wrong
/// passwords), "new" (no vault yet) or "ready".
pub fn status() -> String {
    let Some(user) = crate::keyring::user() else { return String::from("locked") };
    let w = unsafe { WAIT_UNTIL };
    let t = now();
    if w > t {
        return alloc::format!("wait:{}", w - t);
    }
    let mut probe = alloc::vec![0u8; 8];
    match crate::fs::lingfs::read_file_all(&path_for(&user), &mut probe) {
        Ok(Some(_)) | Err(_) => String::from("ready"),
        Ok(None) => String::from("new"),
    }
}

/// The password gate: the signed-in user's own password, rate-limited.
fn check_password(password: &str) -> Option<String> {
    let Some(user) = crate::keyring::user() else { return fail("sign in to LingOS first") };
    let t = now();
    if unsafe { WAIT_UNTIL } > t {
        return fail("too many wrong passwords: wait, then try again");
    }
    if !crate::fs::users::verify(&user, password) {
        unsafe {
            FAILS += 1;
            if FAILS >= 3 {
                let shift = (FAILS - 3).min(5);
                WAIT_UNTIL = t + (30u64 << shift).min(900);
            }
            ERR = "wrong password";
        }
        return None;
    }
    unsafe {
        FAILS = 0;
        WAIT_UNTIL = 0;
    }
    Some(user)
}

struct Keys {
    aead: [u8; 32],
    dice: [u8; 32],
    sig: [u8; 32],
}

impl Drop for Keys {
    fn drop(&mut self) {
        for b in self.aead.iter_mut().chain(self.dice.iter_mut()).chain(self.sig.iter_mut()) {
            unsafe { core::ptr::write_volatile(b, 0) };
        }
    }
}

fn derive(password: &str, salt: &[u8]) -> Option<Keys> {
    let mut ikm = Vec::with_capacity(PREFIX.len() + password.len());
    ikm.extend_from_slice(PREFIX);
    ikm.extend_from_slice(password.as_bytes());
    let pw = crate::fs::users::argon2id(salt, &ikm);
    for b in ikm.iter_mut() {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    let Some(mut pw) = pw else { return None };
    let Some(mut ring) = crate::keyring::key("horizon-vault") else { return None };
    let mut both = [0u8; 64];
    both[..32].copy_from_slice(&pw);
    both[32..].copy_from_slice(&ring);
    let root = ling_crypto::hkdf_sha3(&both, salt, b"horizon-vault v1", 32).ok();
    for b in pw.iter_mut().chain(ring.iter_mut()).chain(both.iter_mut()) {
        unsafe { core::ptr::write_volatile(b, 0) };
    }
    let root = root?;
    let sub = |label: &[u8]| -> Option<[u8; 32]> {
        let k = ling_crypto::hkdf_sha3(&root, salt, label, 32).ok()?;
        let mut out = [0u8; 32];
        out.copy_from_slice(&k);
        Some(out)
    };
    Some(Keys { aead: sub(b"xchacha20-poly1305")?, dice: sub(b"dice-42")?, sig: sub(b"ml-dsa-87")? })
}

/// Random bytes from the board, or (with no hardware source) bytes unique to
/// this key, time and content -- a nonce must never repeat, and does not.
fn nonce_bytes(out: &mut [u8], key: &[u8], extra: &[u8]) {
    if crate::crypto::random_bytes(out) {
        return;
    }
    static mut COUNTER: u64 = 0;
    let n = unsafe {
        COUNTER = COUNTER.wrapping_add(1);
        COUNTER
    };
    let mut m = Vec::new();
    m.extend_from_slice(b"horizon-vault-nonce");
    m.extend_from_slice(key);
    m.extend_from_slice(&n.to_le_bytes());
    m.extend_from_slice(&crate::arch::timer::now_us().to_le_bytes());
    m.extend_from_slice(&Blake3::hash(extra));
    let mut block = 0u32;
    let mut at = 0;
    while at < out.len() {
        let mut b = m.clone();
        b.extend_from_slice(&block.to_le_bytes());
        let h = Blake3::hash(&b);
        let k = (out.len() - at).min(32);
        out[at..at + k].copy_from_slice(&h[..k]);
        at += k;
        block += 1;
    }
}

fn signed_part(salt: &[u8], pk_hash: &[u8], body: &[u8]) -> Vec<u8> {
    let mut m = Vec::with_capacity(4 + 64 + body.len());
    m.extend_from_slice(MAGIC);
    m.extend_from_slice(salt);
    m.extend_from_slice(pk_hash);
    m.extend_from_slice(body);
    m
}

/// The vault text after the password, or None (see [`error`]). A missing
/// vault opens as "".
pub fn open(password: &str) -> Option<String> {
    let user = check_password(password)?;
    let path = path_for(&user);
    let mut size = alloc::vec![0u8; 1 << 20];
    let n = match crate::fs::lingfs::read_file_all(&path, &mut size) {
        Ok(Some(n)) => n,
        Ok(None) => return Some(String::new()),
        Err(_) => return fail("could not read the vault file"),
    };
    let file = &size[..n];
    if n < 4 + 32 + 32 + 4 || &file[..4] != MAGIC {
        return fail("the vault file is damaged");
    }
    let salt = &file[4..36];
    let pk_hash = &file[36..68];
    let sig_len = u32::from_le_bytes([file[68], file[69], file[70], file[71]]) as usize;
    if 72 + sig_len > n {
        return fail("the vault file is damaged");
    }
    let sig = &file[72..72 + sig_len];
    let body = &file[72 + sig_len..];
    let Some(keys) = derive(password, salt) else { return fail("could not derive the vault key (out of memory?)") };
    let kp = MlDsa87Keypair::from_seed(keys.sig);
    let pk = kp.public_key();
    if Blake3::hash(&pk)[..] != pk_hash[..] {
        return fail("this vault was sealed under another key (a different account or keyring)");
    }
    if MlDsa87Keypair::verify(&pk, &signed_part(salt, pk_hash, body), sig).is_err() {
        return fail("the vault file was changed outside Horizon (ML-DSA-87 signature does not match)");
    }
    let Ok(inner) = MobiusHelixReactor::derive(&keys.dice).open(body) else {
        return fail("the vault file is damaged (DICE-42 tag)");
    };
    let Ok(plain) = XChaCha20::new(keys.aead).decrypt(&inner) else {
        return fail("the vault file is damaged (XChaCha20-Poly1305 tag)");
    };
    match String::from_utf8(plain) {
        Ok(s) => Some(s),
        Err(_) => fail("the vault file is damaged"),
    }
}

/// Seal `text` as the vault, after the password. A fresh salt (so fresh
/// keys) every save.
pub fn save(password: &str, text: &str) -> bool {
    let Some(user) = check_password(password) else { return false };
    let mut salt = [0u8; 32];
    nonce_bytes(&mut salt, user.as_bytes(), text.as_bytes());
    let Some(keys) = derive(password, &salt) else {
        unsafe { ERR = "could not derive the vault key (out of memory?)" };
        return false;
    };
    let mut n24 = [0u8; 24];
    nonce_bytes(&mut n24, &keys.aead, text.as_bytes());
    let inner = XChaCha20::new(keys.aead).encrypt_with_nonce(&n24, text.as_bytes());
    let Ok(inner) = inner else {
        unsafe { ERR = "encryption failed" };
        return false;
    };
    let mut d24 = [0u8; 24];
    nonce_bytes(&mut d24, &keys.dice, &inner);
    let body = MobiusHelixReactor::derive(&keys.dice).seal(&d24, &inner);
    let kp = MlDsa87Keypair::from_seed(keys.sig);
    let pk_hash = Blake3::hash(&kp.public_key());
    let sig = kp.sign(&signed_part(&salt, &pk_hash, &body));
    let mut file = Vec::with_capacity(72 + sig.len() + body.len());
    file.extend_from_slice(MAGIC);
    file.extend_from_slice(&salt);
    file.extend_from_slice(&pk_hash);
    file.extend_from_slice(&(sig.len() as u32).to_le_bytes());
    file.extend_from_slice(&sig);
    file.extend_from_slice(&body);
    if crate::fs::lingfs::write_path_any(&path_for(&user), &file).is_err() {
        unsafe { ERR = "could not write the vault file" };
        return false;
    }
    unsafe { ERR = "" };
    true
}

/// RFC 6238 TOTP: the 6-digit code for a base32 secret at unix time `t`
/// (30 s steps, HMAC-SHA1). "" for a secret that is not base32.
pub fn totp(secret_b32: &str, t: u64) -> String {
    let Some(key) = base32(secret_b32) else { return String::new() };
    let mac = hmac_sha1(&key, &(t / 30).to_be_bytes());
    let off = (mac[19] & 15) as usize;
    let bin = ((mac[off] as u32 & 0x7f) << 24) | ((mac[off + 1] as u32) << 16) | ((mac[off + 2] as u32) << 8) | mac[off + 3] as u32;
    alloc::format!("{:06}", bin % 1_000_000)
}

fn base32(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let mut buf = 0u64;
    let mut bits = 0u32;
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a',
            b'2'..=b'7' => c - b'2' + 26,
            b' ' | b'-' | b'=' => continue,
            _ => return None,
        };
        buf = (buf << 5) | v as u64;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((buf >> bits) as u8);
        }
    }
    (!out.is_empty()).then_some(out)
}

fn sha1(msg: &[u8]) -> [u8; 20] {
    let mut h: [u32; 5] = [0x67452301, 0xEFCDAB89, 0x98BADCFE, 0x10325476, 0xC3D2E1F0];
    let mut m = msg.to_vec();
    let bitlen = (msg.len() as u64) * 8;
    m.push(0x80);
    while m.len() % 64 != 56 {
        m.push(0);
    }
    m.extend_from_slice(&bitlen.to_be_bytes());
    for chunk in m.chunks(64) {
        let mut w = [0u32; 80];
        for i in 0..16 {
            w[i] = u32::from_be_bytes([chunk[4 * i], chunk[4 * i + 1], chunk[4 * i + 2], chunk[4 * i + 3]]);
        }
        for i in 16..80 {
            w[i] = (w[i - 3] ^ w[i - 8] ^ w[i - 14] ^ w[i - 16]).rotate_left(1);
        }
        let (mut a, mut b, mut c, mut d, mut e) = (h[0], h[1], h[2], h[3], h[4]);
        for (i, wi) in w.iter().enumerate() {
            let (f, k) = match i {
                0..=19 => ((b & c) | (!b & d), 0x5A827999),
                20..=39 => (b ^ c ^ d, 0x6ED9EBA1),
                40..=59 => ((b & c) | (b & d) | (c & d), 0x8F1BBCDC),
                _ => (b ^ c ^ d, 0xCA62C1D6),
            };
            let t = a.rotate_left(5).wrapping_add(f).wrapping_add(e).wrapping_add(k).wrapping_add(*wi);
            e = d;
            d = c;
            c = b.rotate_left(30);
            b = a;
            a = t;
        }
        h[0] = h[0].wrapping_add(a);
        h[1] = h[1].wrapping_add(b);
        h[2] = h[2].wrapping_add(c);
        h[3] = h[3].wrapping_add(d);
        h[4] = h[4].wrapping_add(e);
    }
    let mut out = [0u8; 20];
    for i in 0..5 {
        out[4 * i..4 * i + 4].copy_from_slice(&h[i].to_be_bytes());
    }
    out
}

fn hmac_sha1(key: &[u8], msg: &[u8]) -> [u8; 20] {
    let mut k = [0u8; 64];
    if key.len() > 64 {
        k[..20].copy_from_slice(&sha1(key));
    } else {
        k[..key.len()].copy_from_slice(key);
    }
    let mut inner = Vec::with_capacity(64 + msg.len());
    inner.extend(k.iter().map(|b| b ^ 0x36));
    inner.extend_from_slice(msg);
    let ih = sha1(&inner);
    let mut outer = Vec::with_capacity(84);
    outer.extend(k.iter().map(|b| b ^ 0x5c));
    outer.extend_from_slice(&ih);
    sha1(&outer)
}

#[cfg(test)]
mod tests {
    #[test]
    fn totp_rfc6238() {
        // RFC 6238 appendix B, SHA1, secret "12345678901234567890"
        let b32 = "GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ";
        assert_eq!(super::totp(b32, 59), "287082");
        assert_eq!(super::totp(b32, 1111111109), "081804");
    }
}
