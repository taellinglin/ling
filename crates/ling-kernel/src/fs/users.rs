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
        if line.len() > prefix_len
            && &line[..key.len()] == key.as_bytes()
            && line[key.len()] == b':'
        {
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

/// Argon2id (16 MiB, 3 passes, 1 lane) -- what new and changed passwords
/// are stored with (`kdf:argon2id`); the memory comes from the frame
/// allocator for the duration of one hash. None only if that memory is
/// not there.
pub(crate) fn argon2id(salt: &[u8], password: &[u8]) -> Option<[u8; 32]> {
    use argon2::{Algorithm, Argon2, Params, Version};
    let params = Params::new(16 * 1024, 3, 1, Some(32)).ok()?;
    let a = Argon2::new(Algorithm::Argon2id, Version::V0x13, params);
    let mut out = [0u8; 32];
    // The Pi: a permanent 16 MiB workspace. A heap block for it would need
    // 32 MiB of contiguous memory (the allocator's header pushes 16 MiB to
    // the next power of two), which a busy desktop may not have -- and a
    // failed allocation here reads as "wrong password". Hashing never runs
    // twice at once (it does not yield), so one workspace serves everyone.
    #[cfg(target_arch = "aarch64")]
    {
        static mut WORK: [argon2::Block; 16 * 1024] = [argon2::Block::new(); 16 * 1024];
        let work = unsafe { &mut *(&raw mut WORK) };
        a.hash_password_into_with_memory(password, salt, &mut out, &mut work[..]).ok()?;
        return Some(out);
    }
    #[allow(unreachable_code)]
    {
        a.hash_password_into(password, salt, &mut out).ok()?;
        Some(out)
    }
}

/// Why the last `verify` said no ("" after a success).
static mut LAST_VERIFY_ERR: &str = "";

pub fn last_verify_error() -> &'static str {
    unsafe { LAST_VERIFY_ERR }
}

/// A salt from the hardware random source (RDRAND / the Pi's RNG), the
/// timer-derived ticks only when there is none.
pub(crate) fn fresh_salt(out: &mut [u8]) {
    if crate::crypto::random_bytes(out) {
        return;
    }
    for (i, b) in out.iter_mut().enumerate() {
        *b = (cpu_ticks() >> ((i % 8) * 8)) as u8;
    }
}

/// The original salted-BLAKE3 scheme, kept to verify accounts made before
/// Argon2id (`verify` re-hashes nothing on its own; `set_password` upgrades).
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

    let mut salt = [0u8; 16];
    fresh_salt(&mut salt);
    // Argon2id; the old scheme only if its memory could not be had.
    let (hash, kdf): ([u8; 32], &[u8]) = match argon2id(&salt, password.as_bytes()) {
        Some(h) => (h, b"argon2id"),
        None => {
            let mut s = [0u8; SALT_BYTES];
            s.copy_from_slice(&salt[..SALT_BYTES]);
            (hash_password(&s, password.as_bytes()), b"blake3")
        },
    };
    let salt_n = if kdf == b"argon2id" { 16 } else { SALT_BYTES };
    let mut salt_hex = [0u8; 32];
    hex_encode(&salt[..salt_n], &mut salt_hex);
    let salt_hex = &salt_hex[..salt_n * 2];
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
    append(
        &mut buf,
        &mut len,
        if group.is_empty() {
            b"users"
        } else {
            group.as_bytes()
        },
    );
    append(&mut buf, &mut len, b"\nkdf:");
    append(&mut buf, &mut len, kdf);
    append(&mut buf, &mut len, b"\nsalt:");
    append(&mut buf, &mut len, salt_hex);
    append(&mut buf, &mut len, b"\nhash:");
    append(&mut buf, &mut len, &hash_hex);
    append(&mut buf, &mut len, b"\n");

    let ok = lingfs::write_in_dir(USERS_DIR, username, &buf[..len]).is_ok();
    if ok {
        ensure_home(username);
    }
    ok
}

/// The folders every home has, in the order the Files window shows them.
pub const HOME_FOLDERS: [&str; 9] =
    ["Documents", "Photos", "Music", "Videos", "Graphics", "3D Models", "Projects", "Programs", "Scripts"];

/// Create `home/<user>` and its standard folders if missing (existing folders
/// and their files are kept). Called on account creation and at every login,
/// so accounts made before homes existed get one too, plus a starter file in
/// Scripts, Graphics and Documents so there is something to open.
pub fn ensure_home(username: &str) {
    if username.is_empty() {
        return;
    }
    let mut path = [0u8; 96];
    for folder in HOME_FOLDERS {
        let p = join4(&mut path, "home/", username, "/", folder);
        if lingfs::path_kind(p) != Some(true) {
            let _ = lingfs::mkdir_p(p);
        }
    }
    let starters: [(&str, &[u8]); 3] = [
        ("Scripts/hello.ling", b"// A Ling script -- open it from Files, or `ling run` it from the console.\nbind main = do {\n    println(\"Hello from LingOS!\")\n    println(\"2 + 2 = \", 2 + 2)\n}\n"),
        ("Graphics/ling-badge.svg", b"<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 200 200\"><rect width=\"200\" height=\"200\" fill=\"#241B33\"/><circle cx=\"100\" cy=\"100\" r=\"70\" fill=\"#4A8FE0\"/><polygon points=\"100,48 146,140 54,140\" fill=\"#FFFFFF\"/></svg>\n"),
        ("Documents/Welcome.txt", b"Welcome to LingOS!\n\nYour home folders:\n  Documents  - text and notes\n  Photos     - BMP / PNG / SVG pictures\n  Music      - WAV audio\n  Videos     - video files\n  Graphics   - SVG drawings and .ling graphics programs\n  3D Models  - 3D model files\n  Projects   - your LingStationX, LingDev, LingModeler projects (one folder each)\n  Programs   - lingfu programs and downloaded source\n  Scripts    - .ling scripts (open one to run it)\n"),
    ];
    for (rel, data) in starters {
        let p = join4(&mut path, "home/", username, "/", rel);
        if lingfs::path_kind(p).is_none() {
            let _ = lingfs::write_path_any(p, data);
        }
    }
    for (rel, src) in crate::lingc::EXAMPLES {
        let p = join4(&mut path, "home/", username, "/", rel);
        if lingfs::path_kind(p).is_none() {
            let _ = lingfs::write_path_any(p, src.as_bytes());
        }
    }
}

fn join4<'a>(buf: &'a mut [u8; 96], a: &str, b: &str, c: &str, d: &str) -> &'a str {
    let mut n = 0usize;
    for s in [a, b, c, d] {
        for &ch in s.as_bytes() {
            if n < buf.len() {
                buf[n] = ch;
                n += 1;
            }
        }
    }
    core::str::from_utf8(&buf[..n]).unwrap_or("")
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
    unsafe { LAST_VERIFY_ERR = "wrong password" };
    let mut buf = [0u8; BLOCK_SIZE];
    let Some(len) = load_record(username, &mut buf) else {
        unsafe { LAST_VERIFY_ERR = "no such account" };
        return false;
    };
    let record = &buf[..len];
    let Some(salt_hex) = field(record, "salt") else { return false };
    let Some(hash_hex) = field(record, "hash") else { return false };
    let mut salt = [0u8; 16];
    let Some(salt_len) = hex_decode(salt_hex, &mut salt) else { return false };
    let mut stored = [0u8; 32];
    if hex_decode(hash_hex, &mut stored) != Some(32) {
        return false;
    }
    let computed = match field(record, "kdf") {
        Some(k) if k == b"argon2id" => match argon2id(&salt[..salt_len], password.as_bytes()) {
            Some(h) => h,
            None => {
                unsafe { LAST_VERIFY_ERR = "the password check could not get its memory" };
                return false;
            },
        },
        _ => {
            if salt_len != SALT_BYTES {
                return false;
            }
            let mut s = [0u8; SALT_BYTES];
            s.copy_from_slice(&salt[..SALT_BYTES]);
            hash_password(&s, password.as_bytes())
        },
    };
    // Constant-time compare: no early exit on the first differing byte.
    let mut diff = 0u8;
    for (a, b) in computed.iter().zip(stored.iter()) {
        diff |= a ^ b;
    }
    if diff == 0 {
        unsafe { LAST_VERIFY_ERR = "" };
    }
    diff == 0
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
// ── Password reset ─────────────────────────────────────────────────────────
// `password_reset` in lingfs lists the accounts waiting for a new password
// ("*" = all). Set from the boot partition's config.txt (setup.rs); the
// sign-in screen asks for the new password twice; typing the OLD password
// instead just signs in and cancels the reset (nothing is lost).

const RESET_FILE: &str = "password_reset";

fn reset_list() -> alloc::vec::Vec<alloc::string::String> {
    let mut buf = [0u8; 1024];
    match lingfs::read_file_all(RESET_FILE, &mut buf) {
        Ok(Some(n)) => core::str::from_utf8(&buf[..n])
            .unwrap_or("")
            .split_whitespace()
            .map(alloc::string::String::from)
            .collect(),
        _ => alloc::vec::Vec::new(),
    }
}

fn reset_store(list: &[alloc::string::String]) {
    let text = list.join("\n");
    let _ = lingfs::write_file_any(RESET_FILE, text.as_bytes());
}

/// Is `username` waiting to choose a new password?
pub fn reset_pending(username: &str) -> bool {
    if username.is_empty() {
        return false;
    }
    reset_list().iter().any(|u| u == "*" || u == username)
}

/// Put `who` ("*" for every account) into reset mode.
pub fn mark_reset(who: &str) {
    let mut l = reset_list();
    if !l.iter().any(|u| u == who) {
        l.push(alloc::string::String::from(who));
    }
    reset_store(&l);
}

/// The reset is done for `username` (a "*" entry becomes the list of the
/// other accounts still waiting).
pub fn clear_reset(username: &str, all_users: &[alloc::string::String]) {
    let mut out: alloc::vec::Vec<alloc::string::String> = alloc::vec::Vec::new();
    for u in reset_list() {
        if u == "*" {
            for o in all_users {
                if o != username && !out.contains(o) {
                    out.push(o.clone());
                }
            }
        } else if u != username && !out.contains(&u) {
            out.push(u);
        }
    }
    reset_store(&out);
}

pub fn set_password(username: &str, new_password: &str) -> bool {
    let ok = set_password_record(username, new_password);
    if ok && !crate::keyring::rewrap(username, new_password) && crate::keyring::user().is_none() {
        crate::console_write(b"passwd: the keyring was not open, so it was not re-wrapped -- sign in with the old password first to keep private files readable\n");
    }
    ok
}

fn set_password_record(username: &str, new_password: &str) -> bool {
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

/// A tick counter for salts: the TSC on x86, the Pi's timer mixed with a
/// call counter (the generic timer is only millisecond-grained here).
fn cpu_ticks() -> u64 {
    #[cfg(target_arch = "x86_64")]
    return unsafe { crate::arch::cpu::rdtsc() };
    #[allow(unreachable_code)]
    {
        // the hardware random source first (the Pi 5's rng200), the clock as a fallback
        let mut r = [0u8; 8];
        if crate::crypto::random_bytes(&mut r) {
            return u64::from_le_bytes(r);
        }
        static mut N: u64 = 0;
        let n = unsafe {
            N = N.wrapping_add(1);
            N
        };
        (crate::arch::timer::now_ms() as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15) ^ n.wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
    }
}
