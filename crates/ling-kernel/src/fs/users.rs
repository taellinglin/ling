//! User accounts: one lingfs blob per user under the synthetic `users/`
//! directory (via `lingfs::write_in_dir`), a simple `key:value`-per-line
//! text record — first/last name, phone, email, group, and a *salted
//! hash*, not a proper password KDF. `hash.rs`'s BLAKE3 is a fast general
//! hash, not a slow memory-hard one (Argon2id) — that's still on the
//! roadmap (see `packages/README.md`'s no_std `ling-crypto` port item), so
//! this is meaningfully better than plaintext but not what a real
//! multi-user system should ship long-term. There's also no
//! process/privilege model yet, so nothing here is *kernel*-enforced —
//! `login` only changes which name new lingfs writes get attributed to
//! (`lingfs::set_current_user`).
//!
//! The shell's `sudo` (see `apps/lsh`) checks `group_of() == "wheel"` via
//! `ling_kernel_user_is_wheel` before running certain commands, with a
//! password re-prompt. Say plainly what that can and can't mean: there is
//! no ring-3, no per-process address space, no MMU-enforced boundary of any
//! kind in this kernel — every line of code already runs with full
//! hardware privilege, sudo gate or not. It's a shell-level social
//! contract (stops an operator from fat-fingering a sensitive command by
//! accident, same category of value as bash/zsh's own `sudo` for a single
//! physical operator), not a security boundary against malicious code.
use crate::fs::lingfs::{self, BLOCK_SIZE};

const USERS_DIR: &str = "users";
const RECORD_MAX: usize = 512;
const SALT_BYTES: usize = 8;

fn hex_encode(bytes: &[u8], out: &mut [u8]) -> usize {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    for (i, &b) in bytes.iter().enumerate() {
        out[i * 2] = HEX[(b >> 4) as usize];
        out[i * 2 + 1] = HEX[(b & 0xF) as usize];
    }
    bytes.len() * 2
}

fn hex_digit(c: u8) -> Option<u8> {
    match c {
        b'0'..=b'9' => Some(c - b'0'),
        b'a'..=b'f' => Some(c - b'a' + 10),
        b'A'..=b'F' => Some(c - b'A' + 10),
        _ => None,
    }
}

fn hex_decode(hex: &[u8], out: &mut [u8]) -> Option<usize> {
    if hex.len() % 2 != 0 {
        return None;
    }
    let n = hex.len() / 2;
    for i in 0..n {
        let hi = hex_digit(hex[i * 2])?;
        let lo = hex_digit(hex[i * 2 + 1])?;
        out[i] = (hi << 4) | lo;
    }
    Some(n)
}

/// Find the value of `key:` in a `key:value`-per-line record. `None` if the
/// key isn't present.
fn field<'a>(record: &'a [u8], key: &str) -> Option<&'a [u8]> {
    for line in record.split(|&b| b == b'\n') {
        let prefix_len = key.len() + 1;
        if line.len() > prefix_len && &line[..key.len()] == key.as_bytes() && line[key.len()] == b':' {
            return Some(&line[prefix_len..]);
        }
    }
    None
}

fn append(buf: &mut [u8; RECORD_MAX], len: &mut usize, s: &[u8]) {
    let n = s.len().min(RECORD_MAX - *len);
    buf[*len..*len + n].copy_from_slice(&s[..n]);
    *len += n;
}

fn hash_password(salt: &[u8; SALT_BYTES], password: &[u8]) -> [u8; 32] {
    let mut buf = [0u8; SALT_BYTES + 256];
    let n = password.len().min(256);
    buf[..SALT_BYTES].copy_from_slice(salt);
    buf[SALT_BYTES..SALT_BYTES + n].copy_from_slice(&password[..n]);
    crate::hash::blake3(&buf[..SALT_BYTES + n])
}

/// Create or update a user account. Takes every field as its own argument
/// (rather than one delimited string parsed here) specifically so the
/// shell can gather them via separate prompts — the password one using
/// `ling_kernel_read_line_masked`, which doesn't echo or record history.
/// Returns `false` on an empty username or a lingfs write failure.
pub fn create(
    username: &str,
    password: &str,
    first: &str,
    last: &str,
    phone: &str,
    email: &str,
    group: &str,
) -> bool {
    if username.is_empty() {
        return false;
    }

    let salt: [u8; SALT_BYTES] = unsafe {
        let a = crate::arch::cpu::rdtsc();
        let b = crate::arch::cpu::rdtsc();
        let mut s = [0u8; SALT_BYTES];
        s[..4].copy_from_slice(&(a as u32).to_le_bytes());
        s[4..].copy_from_slice(&(b as u32).to_le_bytes());
        s
    };
    let hash = hash_password(&salt, password.as_bytes());

    let mut salt_hex = [0u8; SALT_BYTES * 2];
    hex_encode(&salt, &mut salt_hex);
    let mut hash_hex = [0u8; 64];
    hex_encode(&hash, &mut hash_hex);

    let mut buf = [0u8; RECORD_MAX];
    let mut len = 0usize;
    append(&mut buf, &mut len, b"first:");
    append(&mut buf, &mut len, first.as_bytes());
    append(&mut buf, &mut len, b"\nlast:");
    append(&mut buf, &mut len, last.as_bytes());
    append(&mut buf, &mut len, b"\nphone:");
    append(&mut buf, &mut len, phone.as_bytes());
    append(&mut buf, &mut len, b"\nemail:");
    append(&mut buf, &mut len, email.as_bytes());
    append(&mut buf, &mut len, b"\ngroup:");
    append(&mut buf, &mut len, if group.is_empty() { b"users" } else { group.as_bytes() });
    append(&mut buf, &mut len, b"\nsalt:");
    append(&mut buf, &mut len, &salt_hex);
    append(&mut buf, &mut len, b"\nhash:");
    append(&mut buf, &mut len, &hash_hex);
    append(&mut buf, &mut len, b"\n");

    lingfs::write_in_dir(USERS_DIR, username, &buf[..len]).is_ok()
}

fn load_record(username: &str, out: &mut [u8; BLOCK_SIZE]) -> Option<usize> {
    let mut path_buf = [0u8; 64];
    let prefix = b"users/";
    let n = prefix.len() + username.len().min(path_buf.len() - prefix.len());
    path_buf[..prefix.len()].copy_from_slice(prefix);
    path_buf[prefix.len()..n].copy_from_slice(&username.as_bytes()[..n - prefix.len()]);
    let path = core::str::from_utf8(&path_buf[..n]).ok()?;
    lingfs::read_file(path, out).ok().flatten()
}

/// Whether `password` matches the stored (salted) hash for `username`.
/// `false` if the user doesn't exist or the record is malformed.
pub fn verify(username: &str, password: &str) -> bool {
    let mut buf = [0u8; BLOCK_SIZE];
    let Some(len) = load_record(username, &mut buf) else { return false };
    let record = &buf[..len];
    let Some(salt_hex) = field(record, "salt") else { return false };
    let Some(hash_hex) = field(record, "hash") else { return false };

    let mut salt = [0u8; SALT_BYTES];
    if hex_decode(salt_hex, &mut salt) != Some(SALT_BYTES) {
        return false;
    }
    let mut stored_hash = [0u8; 32];
    if hex_decode(hash_hex, &mut stored_hash) != Some(32) {
        return false;
    }

    hash_password(&salt, password.as_bytes()) == stored_hash
}

/// The stored group for `username`, written into `out`, returning its
/// length — `None` if the user doesn't exist.
pub fn group_of(username: &str, out: &mut [u8; 32]) -> Option<usize> {
    let mut buf = [0u8; BLOCK_SIZE];
    let len = load_record(username, &mut buf)?;
    let g = field(&buf[..len], "group")?;
    let n = g.len().min(out.len());
    out[..n].copy_from_slice(&g[..n]);
    Some(n)
}

/// Change `username`'s password, keeping every other field. `false` if the
/// user doesn't exist.
pub fn set_password(username: &str, new_password: &str) -> bool {
    let mut buf = [0u8; BLOCK_SIZE];
    let Some(len) = load_record(username, &mut buf) else { return false };
    let record_copy: [u8; BLOCK_SIZE] = buf;
    let record = &record_copy[..len];

    let get = |key: &str| -> ([u8; 128], usize) {
        let mut out = [0u8; 128];
        match field(record, key) {
            Some(v) => {
                let n = v.len().min(128);
                out[..n].copy_from_slice(&v[..n]);
                (out, n)
            },
            None => (out, 0),
        }
    };
    let (first, first_n) = get("first");
    let (last, last_n) = get("last");
    let (phone, phone_n) = get("phone");
    let (email, email_n) = get("email");
    let (group, group_n) = get("group");

    let (Ok(first), Ok(last), Ok(phone), Ok(email), Ok(group)) = (
        core::str::from_utf8(&first[..first_n]),
        core::str::from_utf8(&last[..last_n]),
        core::str::from_utf8(&phone[..phone_n]),
        core::str::from_utf8(&email[..email_n]),
        core::str::from_utf8(&group[..group_n]),
    ) else {
        return false;
    };
    create(username, new_password, first, last, phone, email, group)
}

/// Change `username`'s group, keeping every other field (including the
/// password hash/salt, unlike `set_password` which regenerates them via
/// `create` — this rewrites the record directly instead).
pub fn set_group(username: &str, new_group: &str) -> bool {
    let mut buf = [0u8; BLOCK_SIZE];
    let Some(len) = load_record(username, &mut buf) else { return false };
    let record_copy: [u8; BLOCK_SIZE] = buf;
    let record = &record_copy[..len];

    let mut out = [0u8; RECORD_MAX];
    let mut out_len = 0usize;
    for line in record.split(|&b| b == b'\n') {
        if line.is_empty() {
            continue;
        }
        if line.starts_with(b"group:") {
            append(&mut out, &mut out_len, b"group:");
            append(&mut out, &mut out_len, new_group.as_bytes());
        } else {
            append(&mut out, &mut out_len, line);
        }
        append(&mut out, &mut out_len, b"\n");
    }
    lingfs::write_in_dir(USERS_DIR, username, &out[..out_len]).is_ok()
}
