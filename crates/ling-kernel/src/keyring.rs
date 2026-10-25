//! The user's keyring: one random 256-bit master key per account, kept on
//! disk only wrapped under a key derived from the login password, and held
//! in memory only while that user is signed in.
//!
//! Signing in (`lingapp::user_login`, the x86 `ling_kernel_user_login`)
//! verifies the password and then calls [`unlock`], which derives the
//! wrapping key with Argon2id (16 MiB, t=3 -- the same cost as the login
//! hash, with its own salt and a domain prefix, so the stored login hash is
//! useless for decrypting anything) and opens the wrapped master key with
//! ChaCha20-Poly1305. Everything the session needs is then derived from the
//! master key with HKDF-SHA256 per purpose ([`key`]): the Messenger history
//! key, the key sealing files under `home/<user>/Private/` and the Horizon
//! profile folder ([`seal`] / [`open`], applied transparently in `vfs`).
//!
//! Locking the screen or logging out calls [`lock`], which zeroes the master
//! key; nothing of the user's decrypts again until the password is typed.
//! Changing the password re-wraps the same master key ([`rewrap`]), so the
//! data stays readable. The file is `home/<user>/.keyring` (a dotfile, hidden
//! from Files); it holds no secret that the password does not protect.
//!
//! On a board without a hardware random source (QEMU's Pi 4 model) the
//! master key is derived from the password and a salt instead of drawn at
//! random -- noted in the file as `mode:derived` and in [`status`] -- which
//! keeps the data as strong as the password itself but loses the "re-wrap on
//! password change" independence.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

static mut USER: Option<String> = None;
static mut MASTER: [u8; 32] = [0; 32];
static mut UNLOCKED: bool = false;
static mut MODE: &str = "";
static mut KDF: &str = "";

const HEADER: &str = "lingos-keyring v1";
const FILE_MAGIC: &[u8; 5] = b"LPRV1";

fn path_for(user: &str) -> String {
    format!("home/{user}/.keyring")
}

/// The signed-in user whose keyring is open.
pub fn user() -> Option<String> {
    if unsafe { UNLOCKED } {
        unsafe { (*(&raw const USER)).clone() }
    } else {
        None
    }
}

pub fn is_unlocked() -> bool {
    unsafe { UNLOCKED }
}

/// For Settings: what protects the session right now.
pub fn status() -> String {
    match user() {
        Some(u) => {
            let (kdf, mode) = unsafe { (KDF, MODE) };
            format!(
                "unlocked for {u}: master key {} (wrapped with {kdf}, ChaCha20-Poly1305)",
                if mode == "random" { "random" } else { "derived from the password (no hardware RNG)" }
            )
        },
        None => String::from("locked (sign in to unlock private files and chat history)"),
    }
}

/// Forget the master key. The screen lock and log out call this.
pub fn lock() {
    unsafe {
        for b in (*(&raw mut MASTER)).iter_mut() {
            core::ptr::write_volatile(b, 0);
        }
        UNLOCKED = false;
        USER = None;
        MODE = "";
        KDF = "";
    }
}

/// The wrapping key: Argon2id over the password with the keyring's own salt
/// and a domain prefix; BLAKE3 only when the 16 MiB Argon2 workspace cannot
/// be had, recorded in the file so the same path opens it later.
fn kek(kdf: &str, salt: &[u8], password: &[u8]) -> Option<[u8; 32]> {
    let mut ikm = Vec::with_capacity(16 + password.len());
    ikm.extend_from_slice(b"lingos-keyring:");
    ikm.extend_from_slice(password);
    match kdf {
        "argon2id" => crate::fs::users::argon2id(salt, &ikm),
        "blake3" => {
            let mut b = Vec::with_capacity(salt.len() + ikm.len());
            b.extend_from_slice(salt);
            b.extend_from_slice(&ikm);
            Some(crate::hash::blake3(&b))
        },
        _ => None,
    }
}

fn pick_kdf(salt: &[u8], password: &[u8]) -> Option<(&'static str, [u8; 32])> {
    if let Some(k) = kek("argon2id", salt, password) {
        return Some(("argon2id", k));
    }
    kek("blake3", salt, password).map(|k| ("blake3", k))
}

fn hex(b: &[u8]) -> String {
    const D: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(b.len() * 2);
    for &x in b {
        s.push(D[(x >> 4) as usize] as char);
        s.push(D[(x & 15) as usize] as char);
    }
    s
}

fn unhex(s: &str) -> Option<Vec<u8>> {
    let b = s.as_bytes();
    if b.len() % 2 != 0 {
        return None;
    }
    let v = |c: u8| -> Option<u8> {
        Some(match c {
            b'0'..=b'9' => c - b'0',
            b'a'..=b'f' => c - b'a' + 10,
            b'A'..=b'F' => c - b'A' + 10,
            _ => return None,
        })
    };
    let mut out = Vec::with_capacity(b.len() / 2);
    for i in (0..b.len()).step_by(2) {
        out.push((v(b[i])? << 4) | v(b[i + 1])?);
    }
    Some(out)
}

fn aad_for(user: &str) -> Vec<u8> {
    let mut a = Vec::new();
    a.extend_from_slice(HEADER.as_bytes());
    a.push(b':');
    a.extend_from_slice(user.as_bytes());
    a
}

/// Random bytes, or -- with no hardware source -- bytes that are at least
/// unique per call (clock, counter, a secret mixed in when there is one).
fn fill_unique(out: &mut [u8], extra: &[u8]) -> bool {
    if crate::crypto::random_bytes(out) {
        return true;
    }
    static mut COUNTER: u64 = 0;
    let n = unsafe {
        COUNTER = COUNTER.wrapping_add(1);
        COUNTER
    };
    let mut ikm = Vec::new();
    ikm.extend_from_slice(b"lingos-keyring-unique");
    ikm.extend_from_slice(&n.to_le_bytes());
    ikm.extend_from_slice(&crate::arch::timer::now_us().to_le_bytes());
    ikm.extend_from_slice(extra);
    ikm.extend_from_slice(unsafe { &*(&raw const MASTER) });
    let mut cursor = 0;
    let mut block = 0u32;
    while cursor < out.len() {
        let mut m = ikm.clone();
        m.extend_from_slice(&block.to_le_bytes());
        let h = crate::hash::blake3(&m);
        let n = (out.len() - cursor).min(32);
        out[cursor..cursor + n].copy_from_slice(&h[..n]);
        cursor += n;
        block += 1;
    }
    false
}

fn write_file(user: &str, kdf: &str, mode: &str, salt: &[u8], nonce: &[u8; 12], tag: &[u8; 16], wrapped: &[u8]) -> bool {
    let text = format!(
        "{HEADER}\nkdf:{kdf}\nmode:{mode}\nsalt:{}\nnonce:{}\ntag:{}\nwrapped:{}\n",
        hex(salt),
        hex(nonce),
        hex(tag),
        hex(wrapped)
    );
    crate::fs::lingfs::write_path_any(&path_for(user), text.as_bytes()).is_ok()
}

fn seal_master(user: &str, password: &[u8], master: &[u8; 32], mode: &str) -> bool {
    let mut salt = [0u8; 16];
    fill_unique(&mut salt, user.as_bytes());
    let Some((kdf, k)) = pick_kdf(&salt, password) else { return false };
    let mut nonce = [0u8; 12];
    fill_unique(&mut nonce, &salt);
    let mut buf = *master;
    let Some(tag) = crate::crypto::chachapoly_seal(&k, &nonce, &aad_for(user), &mut buf) else { return false };
    let ok = write_file(user, kdf, mode, &salt, &nonce, &tag, &buf);
    if ok {
        unsafe {
            KDF = kdf;
        }
    }
    ok
}

fn set_session(user: &str, master: [u8; 32], mode: &'static str) {
    unsafe {
        *(&raw mut MASTER) = master;
        USER = Some(String::from(user));
        UNLOCKED = true;
        MODE = mode;
    }
}

/// A first keyring for `user`: a random master key where the board has a
/// random source, else one derived from the password and the salt.
fn create(user: &str, password: &[u8]) -> bool {
    let mut master = [0u8; 32];
    let random = fill_unique(&mut master, password);
    let mode: &'static str = if random { "random" } else { "derived" };
    if !seal_master(user, password, &master, mode) {
        return false;
    }
    set_session(user, master, mode);
    crate::console_write(b"keyring: created for ");
    crate::console_write(user.as_bytes());
    crate::console_write(if random { b" (random master key)\n" } else { b" (master key derived from the password: no hardware RNG)\n" });
    true
}

/// Open `user`'s keyring with their password (right after `users::verify`
/// accepted it). Creates the keyring on first sign-in. Idempotent for the
/// user already open; another user's session is locked first.
pub fn unlock(user: &str, password: &str) -> bool {
    if let Some(u) = self::user() {
        if u == user {
            return true;
        }
        lock();
    } else {
        lock();
    }
    let pw = password.as_bytes();
    let mut raw = alloc::vec![0u8; 4096];
    let text = match crate::fs::lingfs::read_file_all(&path_for(user), &mut raw) {
        Ok(Some(n)) => String::from_utf8_lossy(&raw[..n]).into_owned(),
        _ => return create(user, pw),
    };
    let field = |k: &str| -> Option<String> {
        text.lines().find_map(|l| l.strip_prefix(k).map(|v| String::from(v.trim())))
    };
    let parsed = (|| -> Option<([u8; 32], &'static str, &'static str)> {
        if !text.starts_with(HEADER) {
            return None;
        }
        let kdf: &'static str = match field("kdf:")?.as_str() {
            "argon2id" => "argon2id",
            "blake3" => "blake3",
            _ => return None,
        };
        let mode: &'static str = if field("mode:")?.as_str() == "random" { "random" } else { "derived" };
        let salt = unhex(&field("salt:")?)?;
        let nonce: [u8; 12] = unhex(&field("nonce:")?)?.try_into().ok()?;
        let tag: [u8; 16] = unhex(&field("tag:")?)?.try_into().ok()?;
        let mut wrapped = unhex(&field("wrapped:")?)?;
        if wrapped.len() != 32 {
            return None;
        }
        let k = kek(kdf, &salt, pw)?;
        if !crate::crypto::chachapoly_open(&k, &nonce, &aad_for(user), &mut wrapped, &tag) {
            return None;
        }
        let master: [u8; 32] = wrapped.try_into().ok()?;
        Some((master, kdf, mode))
    })();
    match parsed {
        Some((master, kdf, mode)) => {
            set_session(user, master, mode);
            unsafe {
                KDF = kdf;
            }
            crate::console_write(b"keyring: unlocked for ");
            crate::console_write(user.as_bytes());
            crate::console_write(b"\n");
            true
        },
        None => {
            // the password was just verified, so this is a damaged file: start
            // over (earlier private files stay sealed under the lost key)
            crate::console_write(b"keyring: could not open the keyring file for ");
            crate::console_write(user.as_bytes());
            crate::console_write(b" -- creating a new one (earlier private files stay sealed)\n");
            create(user, pw)
        },
    }
}

/// Re-wrap the open master key under a new password (the data stays
/// readable). False when `user`'s keyring is not the one open.
pub fn rewrap(user: &str, new_password: &str) -> bool {
    if self::user().as_deref() != Some(user) {
        return false;
    }
    let master = unsafe { *(&raw const MASTER) };
    let mode = unsafe { MODE };
    seal_master(user, new_password.as_bytes(), &master, mode)
}

/// A 256-bit key for one purpose ("messenger-chat", "vfs-private", ...),
/// derived from the master key; None while locked.
pub fn key(purpose: &str) -> Option<[u8; 32]> {
    if !is_unlocked() {
        return None;
    }
    let mut out = [0u8; 32];
    let ok = crate::crypto::hkdf_sha256(b"lingos-keyring", unsafe { &*(&raw const MASTER) }, purpose.as_bytes(), &mut out);
    ok.then_some(out)
}

/// Seal bytes for storage under the purpose key: `LPRV1` + nonce + tag +
/// ciphertext, bound to `aad` (the file's path). None while locked.
pub fn seal(purpose: &str, aad: &[u8], plaintext: &[u8]) -> Option<Vec<u8>> {
    let k = key(purpose)?;
    let mut nonce = [0u8; 12];
    if !crate::crypto::random_bytes(&mut nonce) {
        // no RNG: a nonce unique to this key, path and content (the same
        // content re-sealed repeats its ciphertext; different content never
        // repeats a nonce)
        let mut m = Vec::with_capacity(64 + aad.len() + plaintext.len());
        m.extend_from_slice(&k);
        m.extend_from_slice(aad);
        m.extend_from_slice(plaintext);
        let h = crate::hash::blake3(&m);
        nonce.copy_from_slice(&h[..12]);
    }
    let mut buf = plaintext.to_vec();
    let tag = crate::crypto::chachapoly_seal(&k, &nonce, aad, &mut buf)?;
    let mut out = Vec::with_capacity(5 + 12 + 16 + buf.len());
    out.extend_from_slice(FILE_MAGIC);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&tag);
    out.extend_from_slice(&buf);
    Some(out)
}

/// Whether bytes carry the sealed-file header.
pub fn is_sealed(blob: &[u8]) -> bool {
    blob.len() >= 33 && &blob[..5] == FILE_MAGIC
}

/// Open a blob from [`seal`]; None while locked, on a wrong key or tampering.
pub fn open(purpose: &str, aad: &[u8], blob: &[u8]) -> Option<Vec<u8>> {
    if !is_sealed(blob) {
        return None;
    }
    let k = key(purpose)?;
    let nonce: [u8; 12] = blob[5..17].try_into().ok()?;
    let tag: [u8; 16] = blob[17..33].try_into().ok()?;
    let mut buf = blob[33..].to_vec();
    crate::crypto::chachapoly_open(&k, &nonce, aad, &mut buf, &tag).then_some(buf)
}

/// Is this lingfs path one the keyring seals? Private files and the browser
/// profile of the signed-in user (reads and writes there go through
/// [`open`] / [`seal`] in `vfs`).
pub fn is_private_path(resolved: &str) -> bool {
    let user = crate::fs::lingfs::current_user();
    if user.is_empty() {
        return false;
    }
    let base = format!("home/{user}/");
    let Some(rest) = resolved.strip_prefix(base.as_str()) else { return false };
    rest.starts_with("Private/") || rest.starts_with("Programs/Horizon/") || rest.starts_with(".messenger/")
}
