//! The kernel side of everything LWP-1 took from `std`: monotonic time,
//! wall-clock time, randomness, cooperative waiting and the data dir.
//! (The host wire test swaps this one file for a std version.)

use alloc::string::String;
use alloc::vec::Vec;

/// Monotonic milliseconds (the host's `Instant`).
pub fn now_ms() -> u64 {
    crate::arch::timer::now_ms()
}

/// Seconds since the Unix epoch (ban expiry, address-book timestamps).
pub fn unix_now() -> u64 {
    #[cfg(target_arch = "x86_64")]
    {
        crate::arch::rtc::unix_timestamp().max(0) as u64
    }
    #[cfg(target_arch = "aarch64")]
    {
        // The Pi 5's firmware RTC, read once and carried forward on the
        // monotonic clock; boards without one count from a fixed 2026 epoch
        // (bans then still expire after the right number of seconds).
        static mut BASE: Option<u64> = None;
        let base = unsafe {
            match *(&raw const BASE) {
                Some(b) => b,
                None => {
                    let rtc = crate::arch::mailbox::rtc_time().map(|t| t as u64).unwrap_or(1_767_225_600);
                    let b = rtc.saturating_sub(now_ms() / 1000);
                    *(&raw mut BASE) = Some(b);
                    b
                },
            }
        };
        base + now_ms() / 1000
    }
}

/// The kernel CSPRNG (RDRAND / the SoC RNG) for ling-crypto's `rng`.
fn entropy(buf: &mut [u8]) {
    if !crate::crypto::random_bytes(buf) {
        // fail closed, like ling-crypto without a source: never weak keys
        panic!("lwp: no hardware random number generator");
    }
}

/// Route ling-crypto's randomness (ML-KEM / ML-DSA keygen, X25519 ephemerals)
/// to the kernel RNG. Idempotent.
pub fn install_entropy() {
    ling_crypto::rng::set_entropy_source(entropy);
}

pub fn random_bytes(buf: &mut [u8]) {
    entropy(buf)
}

pub fn rand_u64() -> u64 {
    let mut b = [0u8; 8];
    random_bytes(&mut b);
    u64::from_le_bytes(b)
}

/// Uniform in `0..n` (`n > 0`).
pub fn gen_index(n: usize) -> usize {
    if n <= 1 {
        return 0;
    }
    // rejection sampling: no modulo bias
    let n = n as u64;
    let zone = u64::MAX - (u64::MAX % n);
    loop {
        let r = rand_u64();
        if r < zone {
            return (r % n) as usize;
        }
    }
}

/// Uniform in `a..=b`.
pub fn gen_range_incl(a: u64, b: u64) -> u64 {
    if b <= a {
        return a;
    }
    let span = b - a;
    if span == u64::MAX {
        return rand_u64();
    }
    a + gen_index((span + 1) as usize) as u64
}

pub fn gen_bool(p: f64) -> bool {
    if p >= 1.0 {
        return true;
    }
    if p <= 0.0 {
        return false;
    }
    ((rand_u64() >> 11) as f64 / (1u64 << 53) as f64) < p
}

pub fn shuffle<T>(v: &mut [T]) {
    let n = v.len();
    if n < 2 {
        return;
    }
    for i in (1..n).rev() {
        let j = gen_index(i + 1);
        v.swap(i, j);
    }
}

/// Give the other coroutines (and the desktop) a turn.
pub fn yield_now() {
    crate::coro::yield_now();
}

/// Sleep cooperatively.
pub fn sleep_ms(ms: u64) {
    crate::coro::sleep_ms(ms);
}

/// Run `f` with the vfs scope set so that `dir/name` resolves to exactly
/// that path, whichever app is in front when a worker saves.
fn in_dir<R>(dir: &str, f: impl FnOnce() -> R) -> R {
    let saved = crate::vfs::take_scope();
    crate::vfs::set_root(dir);
    let r = f();
    crate::vfs::restore_scope(saved);
    r
}

/// The data dir as an absolute vfs path (resolved against the starting
/// program's data folder, like the host's relative `data_dir`), created.
pub fn prepare_dir(dir: &str) -> String {
    let abs = crate::vfs::resolve(dir);
    let _ = crate::vfs::mkdir(&abs);
    abs
}

pub fn read_file(dir: &str, name: &str) -> Option<Vec<u8>> {
    if dir.is_empty() {
        return None;
    }
    let p = alloc::format!("{dir}/{name}");
    in_dir(dir, || crate::vfs::read(&p))
}

pub fn write_file(dir: &str, name: &str, data: &[u8]) -> bool {
    if dir.is_empty() || !crate::vfs::ready() {
        return false;
    }
    let p = alloc::format!("{dir}/{name}");
    in_dir(dir, || crate::vfs::write(&p, data))
}
