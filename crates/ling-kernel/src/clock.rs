//! Wall-clock time (seconds since 1970) for everything that needs a date:
//! Ling's `now()`, the wallet's P2P layer, signed lists with an issue date.
//!
//! Sources, best first: the network (SNTP, asked once DHCP gives us an
//! address), the board's clock (the PC's CMOS RTC; the Pi 5's firmware RTC,
//! which only keeps time with a backup battery and otherwise restarts at
//! 1970), and a floor near this kernel's build -- a clock that reads earlier
//! than the software it runs makes every recently signed thing look like it
//! is from the future (the wallet called its seed list "Unverified").

use crate::arch::timer::now_ms;

/// Never earlier than this: 2026-10-30 00:00 UTC, about when this was written.
const FLOOR: u64 = 1_793_318_400;

/// Unix seconds at boot (now = base + uptime), once a source has set it.
static mut NET_BASE: Option<u64> = None;
static mut BOARD_BASE: Option<u64> = None;
static mut BOARD_TRIED: bool = false;

fn uptime_s() -> u64 {
    now_ms() / 1000
}

/// The board's own clock as unix seconds, if it has a believable one.
fn board_time() -> Option<u64> {
    #[cfg(target_arch = "x86_64")]
    {
        let t = crate::arch::rtc::unix_timestamp();
        return if t > 1_600_000_000 { Some(t as u64) } else { None };
    }
    #[cfg(target_arch = "aarch64")]
    {
        return crate::arch::mailbox::rtc_time().map(|t| t as u64);
    }
    #[allow(unreachable_code)]
    None
}

/// Seconds since 1970, UTC.
pub fn unix_now() -> u64 {
    let up = uptime_s();
    if let Some(b) = unsafe { NET_BASE } {
        return b + up;
    }
    #[cfg(target_arch = "x86_64")]
    {
        // the PC's CMOS clock keeps running and is read fresh
        return board_time().unwrap_or(0).max(FLOOR + up);
    }
    #[allow(unreachable_code)]
    unsafe {
        if !BOARD_TRIED {
            BOARD_TRIED = true;
            BOARD_BASE = board_time().map(|t| t.saturating_sub(up));
        }
        BOARD_BASE.map(|b| b + up).unwrap_or(0).max(FLOOR + up)
    }
}

/// Whether the time came from the network (or a working board clock), not the floor.
pub fn is_set() -> bool {
    let net = unsafe { NET_BASE.is_some() };
    net || board_time().is_some()
}

/// Ask the network for the time (SNTP) and use it from now on. Called when
/// DHCP brings an interface up; true when a server answered.
pub fn sync_from_network() -> bool {
    let Some(t) = crate::drivers::netstack::sntp_time() else {
        crate::console_write(b"clock: no time server answered -- using the board clock\n");
        return false;
    };
    unsafe { NET_BASE = Some(t.saturating_sub(uptime_s())) };
    let mut buf = [0u8; 48];
    let n = fmt_utc(t, &mut buf);
    crate::console_write(b"clock: set from the network: ");
    crate::console_write(&buf[..n]);
    crate::console_write(b" UTC\n");
    true
}

/// While the network has not given us the time: try again, at most once a
/// minute (the first try can come before DNS works, right after joining).
pub fn retry_sync() {
    static mut LAST_MS: u64 = 0;
    static mut TRIES: u32 = 0;
    // a failed try blocks for the DNS timeouts: give up after ten (a network with no internet)
    if unsafe { NET_BASE.is_some() || TRIES >= 10 } {
        return;
    }
    let now = now_ms();
    if unsafe { LAST_MS } != 0 && now.saturating_sub(unsafe { LAST_MS }) < 60_000 {
        return;
    }
    unsafe {
        LAST_MS = now;
        TRIES += 1;
    }
    if crate::drivers::netstack::self_ip() != [0; 4] {
        sync_from_network();
    }
}

/// "YYYY-MM-DD HH:MM:SS" for `t`.
fn fmt_utc(t: u64, out: &mut [u8; 48]) -> usize {
    let days = (t / 86_400) as i64;
    let secs = t % 86_400;
    // civil-from-days (Howard Hinnant)
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = yoe + era * 400 + (m <= 2) as i64;
    let parts = [(y as u64, 4), (m as u64, 2), (d as u64, 2), (secs / 3600, 2), (secs / 60 % 60, 2), (secs % 60, 2)];
    let seps = [b'-', b'-', b' ', b':', b':', 0];
    let mut n = 0;
    for (i, &(v, w)) in parts.iter().enumerate() {
        for k in (0..w).rev() {
            out[n] = b'0' + ((v / 10u64.pow(k)) % 10) as u8;
            n += 1;
        }
        if seps[i] != 0 {
            out[n] = seps[i];
            n += 1;
        }
    }
    n
}
