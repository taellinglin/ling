//! CMOS real-time clock (ports 0x70/0x71) — the one source of actual
//! wall-clock date/time anywhere in this kernel. Everything else (`timer.rs`)
//! is boot-relative only; this is what a locale/timezone system needs to
//! mean anything more than a symbolic offset.
//!
//! Real limitations, disclosed rather than hidden: this reads whatever the
//! CMOS chip holds, which on real hardware is whatever the user's BIOS clock
//! is set to (commonly local time, not UTC — there's no standard way to ask
//! the RTC itself which one it is) and on QEMU/VirtualBox defaults to the
//! host clock in UTC unless `-rtc base=localtime` says otherwise. This
//! driver treats the reading as UTC and lets the locale layer apply
//! per-region offsets on top — correct for this project's own QEMU test
//! setup, not a guarantee for arbitrary real hardware. No century register
//! handling beyond a fixed 2000 pivot (register 0x32 (century) is present on
//! ACPI-era hardware but not universally at a fixed offset across vendors —
//! QEMU exposes years as 2-digit BCD under 0x09, so this assumes 20xx).

use super::io;

const CMOS_INDEX: u16 = 0x70;
const CMOS_DATA: u16 = 0x71;

const REG_SECONDS: u8 = 0x00;
const REG_MINUTES: u8 = 0x02;
const REG_HOURS: u8 = 0x04;
const REG_DAY: u8 = 0x07;
const REG_MONTH: u8 = 0x08;
const REG_YEAR: u8 = 0x09;
const REG_STATUS_A: u8 = 0x0A;
const REG_STATUS_B: u8 = 0x0B;

const STATUS_A_UPDATE_IN_PROGRESS: u8 = 0x80;

fn read_reg(reg: u8) -> u8 {
    unsafe {
        io::outb(CMOS_INDEX, reg);
        io::inb(CMOS_DATA)
    }
}

fn bcd_to_bin(v: u8) -> u8 {
    (v & 0x0F) + ((v >> 4) * 10)
}

/// UTC (per this module's doc) wall-clock reading, decoded to plain binary.
#[derive(Clone, Copy, Default)]
pub struct DateTime {
    pub year: u16,
    pub month: u8,
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
}

/// Read the CMOS clock. Bounded-spin on the "update in progress" flag (a
/// live RTC tick can tear a read mid-update) rather than trusting it clears
/// promptly, then reads twice and retries once if the two readings disagree
/// — the standard RTC read idiom, since there's no way to atomically latch
/// all registers together on this hardware.
pub fn read() -> DateTime {
    let mut spins = 0u32;
    while read_reg(REG_STATUS_A) & STATUS_A_UPDATE_IN_PROGRESS != 0 {
        spins += 1;
        if spins > 1_000_000 {
            break;
        }
    }

    let mut dt = read_raw();
    let dt2 = read_raw();
    if !same(&dt, &dt2) {
        dt = dt2;
    }

    let status_b = read_reg(REG_STATUS_B);
    let binary_mode = status_b & 0x04 != 0;
    if !binary_mode {
        dt.second = bcd_to_bin(dt.second);
        dt.minute = bcd_to_bin(dt.minute);
        // Hour's low 7 bits are BCD even in 12-hour mode; bit 7 (PM flag)
        // handled separately below.
        dt.hour = bcd_to_bin(dt.hour & 0x7F) | (dt.hour & 0x80);
        dt.day = bcd_to_bin(dt.day);
        dt.month = bcd_to_bin(dt.month);
        dt.year = bcd_to_bin(dt.year as u8) as u16;
    }
    let is_12hr = status_b & 0x02 == 0;
    if is_12hr && dt.hour & 0x80 != 0 {
        dt.hour = (dt.hour & 0x7F) % 12 + 12;
    } else {
        dt.hour &= 0x7F;
    }
    dt.year += 2000;
    dt
}

fn read_raw() -> DateTime {
    DateTime {
        second: read_reg(REG_SECONDS),
        minute: read_reg(REG_MINUTES),
        hour: read_reg(REG_HOURS),
        day: read_reg(REG_DAY),
        month: read_reg(REG_MONTH),
        year: read_reg(REG_YEAR) as u16,
    }
}

fn same(a: &DateTime, b: &DateTime) -> bool {
    a.second == b.second
        && a.minute == b.minute
        && a.hour == b.hour
        && a.day == b.day
        && a.month == b.month
        && a.year == b.year
}

/// Seconds since the Unix epoch (1970-01-01T00:00:00), via a plain civil
/// calendar calculation (Howard Hinnant's `days_from_civil` algorithm) --
/// no libc/chrono available in this no_std build. Treats `read()`'s output
/// as UTC per this module's doc.
pub fn unix_timestamp() -> i64 {
    let dt = read();
    let y = dt.year as i64 - if dt.month <= 2 { 1 } else { 0 };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = (y - era * 400) as i64;
    let mp = ((dt.month as i64 + 9) % 12) as i64;
    let doy = (153 * mp + 2) / 5 + dt.day as i64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146097 + doe - 719468;
    days * 86400 + dt.hour as i64 * 3600 + dt.minute as i64 * 60 + dt.second as i64
}
