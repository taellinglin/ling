//! Optional two-factor sign-in: TOTP (RFC 6238 -- HMAC-SHA1, 30-second steps,
//! 6 digits, what Google Authenticator / Authy / 1Password / Aegis use) plus
//! one-time recovery codes.
//!
//! Per account, `twofa/<user>` in lingfs:
//!   state:pending|on        pending = shown as a QR code, not confirmed yet
//!   secret:<base32>         the shared TOTP secret (20 random bytes)
//!   rc:<hex blake3>         one line per unused recovery code
//! Once `state:on`, every sign-in path requires a code after the password
//! (`lingapp::user_login`, x86 `ling_kernel_user_login`, the lock screens,
//! SSH and the terminal's `su`). A code is accepted for the step before and
//! after the current one (clock drift) and only once.

use crate::fs::lingfs::{self, BLOCK_SIZE};
use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

const DIR: &str = "twofa";
const STEP: u64 = 30;
const RECOVERY_CODES: usize = 8;

struct Rec {
    on: bool,
    secret: Vec<u8>,
    recovery: Vec<String>,
}

fn load(user: &str) -> Option<Rec> {
    if user.is_empty() || user.contains('/') {
        return None;
    }
    let mut buf = [0u8; BLOCK_SIZE];
    let n = lingfs::read_file(&format!("{DIR}/{user}"), &mut buf).ok().flatten()?;
    let text = core::str::from_utf8(&buf[..n]).ok()?;
    let mut rec = Rec { on: false, secret: Vec::new(), recovery: Vec::new() };
    for line in text.lines() {
        if let Some(v) = line.strip_prefix("state:") {
            rec.on = v.trim() == "on";
        } else if let Some(v) = line.strip_prefix("secret:") {
            rec.secret = base32_decode(v.trim())?;
        } else if let Some(v) = line.strip_prefix("rc:") {
            rec.recovery.push(String::from(v.trim()));
        }
    }
    if rec.secret.is_empty() {
        return None;
    }
    Some(rec)
}

fn save(user: &str, rec: &Rec) -> bool {
    let mut s = format!("state:{}\nsecret:{}\n", if rec.on { "on" } else { "pending" }, base32_encode(&rec.secret));
    for h in &rec.recovery {
        s.push_str("rc:");
        s.push_str(h);
        s.push('\n');
    }
    lingfs::write_in_dir(DIR, user, s.as_bytes()).is_ok()
}

/// Does `user` sign in with a code as well as the password?
pub fn enabled(user: &str) -> bool {
    load(user).map(|r| r.on).unwrap_or(false)
}

/// Unused recovery codes left for `user`.
pub fn recovery_left(user: &str) -> usize {
    load(user).filter(|r| r.on).map(|r| r.recovery.len()).unwrap_or(0)
}

/// Start setting up: a fresh secret, kept as pending until a code from the
/// app confirms it. Returns (secret in base32, the `otpauth://` URI for the
/// QR code). None when 2FA is already on (turn it off first) or storage failed.
pub fn begin(user: &str) -> Option<(String, String)> {
    if enabled(user) {
        return None;
    }
    let mut secret = [0u8; 20];
    if !crate::crypto::random_bytes(&mut secret) {
        return None;
    }
    let rec = Rec { on: false, secret: secret.to_vec(), recovery: Vec::new() };
    if !save(user, &rec) {
        return None;
    }
    let b32 = base32_encode(&secret);
    #[cfg(target_arch = "aarch64")]
    let host = crate::setup::hostname();
    #[cfg(not(target_arch = "aarch64"))]
    let host = String::new();
    let label = if host.is_empty() { format!("LingOS:{user}") } else { format!("LingOS:{user}@{host}") };
    let uri = format!("otpauth://totp/{}?secret={b32}&issuer=LingOS&algorithm=SHA1&digits=6&period=30", url_escape(&label));
    Some((b32, uri))
}

/// Finish setting up with a code from the app: 2FA is on from now on.
/// Returns the recovery codes to show once (None: wrong code / not pending).
pub fn confirm(user: &str, code: &str) -> Option<Vec<String>> {
    let mut rec = load(user)?;
    if rec.on || !totp_ok(user, &rec.secret, code) {
        return None;
    }
    let mut codes = Vec::new();
    rec.recovery.clear();
    for _ in 0..RECOVERY_CODES {
        let mut raw = [0u8; 5];
        if !crate::crypto::random_bytes(&mut raw) {
            return None;
        }
        let c = base32_encode(&raw); // 8 characters
        rec.recovery.push(hash_code(&c));
        codes.push(format!("{}-{}", &c[..4], &c[4..]));
    }
    rec.on = true;
    if !save(user, &rec) {
        return None;
    }
    crate::console_write(b"twofa: two-factor sign-in turned on for ");
    crate::console_write(user.as_bytes());
    crate::console_write(b"\n");
    Some(codes)
}

/// Turn 2FA off (needs a current code or a recovery code).
pub fn disable(user: &str, code: &str) -> bool {
    if !check(user, code) {
        return false;
    }
    let ok = lingfs::remove_path(&format!("{DIR}/{user}")).unwrap_or(false);
    if ok {
        crate::console_write(b"twofa: two-factor sign-in turned off for ");
        crate::console_write(user.as_bytes());
        crate::console_write(b"\n");
    }
    ok
}

/// Drop an unconfirmed setup (the QR code was shown but never confirmed).
pub fn cancel(user: &str) {
    if load(user).map(|r| !r.on).unwrap_or(false) {
        let _ = lingfs::remove_path(&format!("{DIR}/{user}"));
    }
}

/// Is `code` good for `user` right now: a TOTP code, or an unused recovery
/// code (used up by this call)? Wrong codes are throttled (see `Throttle`):
/// a sign-in prompt reachable over the network must not allow unlimited guesses.
/// The last `check` failed only because its (right) code had been used already.
static mut REPLAYED: bool = false;

/// Whether the last failed `check` saw a code that was right but already used
/// this 30-second step (a login over SSH, say) -- the user should wait for the
/// next code rather than look for a typo.
pub fn last_was_replay() -> bool {
    unsafe { REPLAYED }
}

pub fn check(user: &str, code: &str) -> bool {
    unsafe { REPLAYED = false };
    if locked_for_ms(user) > 0 {
        return false;
    }
    let ok = check_inner(user, code);
    note_attempt(user, ok);
    ok
}

fn check_inner(user: &str, code: &str) -> bool {
    let Some(mut rec) = load(user) else { return false };
    if !rec.on {
        return false;
    }
    if totp_ok(user, &rec.secret, code) {
        return true;
    }
    let norm: String = code.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.to_ascii_uppercase()).collect();
    if norm.len() == 8 {
        let h = hash_code(&norm);
        if let Some(i) = rec.recovery.iter().position(|x| *x == h) {
            rec.recovery.remove(i);
            save(user, &rec);
            crate::console_write(b"twofa: a recovery code was used\n");
            return true;
        }
    }
    false
}

/// Wrong-code throttle, per account: after `FREE_TRIES` wrong codes in a row the account refuses
/// every code (even a right one) for a while, longer each time (30 s, doubling, up to 15 min). A
/// right code clears it. Six digits are guessable in a few million tries; this makes that take years.
struct Throttle {
    user: String,
    fails: u32,
    until_ms: u64,
    penalty_s: u64,
}

static mut THROTTLE: Vec<Throttle> = Vec::new();
const FREE_TRIES: u32 = 5;

fn throttle() -> &'static mut Vec<Throttle> {
    unsafe { &mut *(&raw mut THROTTLE) }
}

/// Milliseconds `user` must still wait before a code is tried again (0: not locked).
pub fn locked_for_ms(user: &str) -> u64 {
    let now = crate::arch::timer::now_ms();
    throttle().iter().find(|t| t.user == user).map(|t| t.until_ms.saturating_sub(now)).unwrap_or(0)
}

fn note_attempt(user: &str, ok: bool) {
    let now = crate::arch::timer::now_ms();
    let v = throttle();
    if ok {
        v.retain(|t| t.user != user);
        return;
    }
    let i = match v.iter().position(|t| t.user == user) {
        Some(i) => i,
        None => {
            if v.len() >= 16 {
                v.remove(0);
            }
            v.push(Throttle { user: String::from(user), fails: 0, until_ms: 0, penalty_s: 15 });
            v.len() - 1
        },
    };
    let t = &mut v[i];
    t.fails += 1;
    if t.fails >= FREE_TRIES {
        t.penalty_s = (t.penalty_s * 2).min(900);
        t.until_ms = now + t.penalty_s * 1000;
        t.fails = 0;
        crate::console_write(b"twofa: too many wrong codes for ");
        crate::console_write(user.as_bytes());
        crate::console_write(b" -- codes refused for a while\n");
    }
}

/// The last step a code was accepted for (no code works twice).
static mut LAST_USED: Option<(String, u64)> = None;

fn totp_ok(user: &str, secret: &[u8], code: &str) -> bool {
    let digits: String = code.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() != 6 || code.chars().any(|c| c.is_ascii_alphabetic()) {
        return false;
    }
    let Ok(want) = digits.parse::<u32>() else { return false };
    let now = crate::clock::unix_now() / STEP;
    for step in [now, now.wrapping_sub(1), now + 1] {
        if totp(secret, step) == want {
            let last = unsafe { &mut *(&raw mut LAST_USED) };
            if matches!(last, Some((u, s)) if u == user && *s >= step) {
                unsafe { REPLAYED = true };
                return false; // replayed
            }
            *last = Some((String::from(user), step));
            return true;
        }
    }
    false
}

/// RFC 4226 HOTP over `counter` (RFC 6238 passes the time step).
fn totp(secret: &[u8], counter: u64) -> u32 {
    use hmac::{Hmac, Mac};
    let Ok(mut mac) = <Hmac<sha1::Sha1> as Mac>::new_from_slice(secret) else { return u32::MAX };
    mac.update(&counter.to_be_bytes());
    let h = mac.finalize().into_bytes();
    let off = (h[19] & 0x0F) as usize;
    let v = u32::from_be_bytes([h[off] & 0x7F, h[off + 1], h[off + 2], h[off + 3]]);
    v % 1_000_000
}

fn hash_code(code: &str) -> String {
    let h = crate::hash::blake3(code.as_bytes());
    let mut s = String::with_capacity(64);
    for b in h {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

const B32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

fn base32_encode(data: &[u8]) -> String {
    let mut out = String::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for &b in data {
        acc = (acc << 8) | b as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(B32[((acc >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(B32[((acc << (5 - bits)) & 31) as usize] as char);
    }
    out
}

fn base32_decode(s: &str) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes().filter(|c| *c != b'=' && *c != b' ') {
        let v = B32.iter().position(|&x| x == c.to_ascii_uppercase())? as u32;
        acc = (acc << 5) | v;
        bits += 5;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
        }
    }
    Some(out)
}

fn url_escape(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        if b.is_ascii_alphanumeric() || b"-_.~:".contains(&b) {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    #[test]
    fn rfc6238_sha1_vectors() {
        // RFC 6238 appendix B, SHA-1 secret "12345678901234567890", 8 digits there;
        // the low 6 digits are what a 6-digit app shows.
        let k = b"12345678901234567890";
        assert_eq!(super::totp(k, 59 / 30), 287082);
        assert_eq!(super::totp(k, 1111111109 / 30), 81804);
        assert_eq!(super::totp(k, 1234567890 / 30), 5924);
    }
}
