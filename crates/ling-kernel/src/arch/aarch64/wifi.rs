//! Raspberry Pi 5 Wi-Fi: the CYW43455 radio on the BCM2712's SDIO2.
//!
//! The radio sits on the second SDHCI controller (`sdio2`, `0x10_0110_0000`,
//! cfg block at +0x400), 4-bit SDIO on GPIO 30-35, powered by WL_ON = GPIO 28
//! (all from the bcm2712 device trees). Linux leaves `sdio2` to its own
//! driver and the firmware doesn't bring it up, so this does everything by
//! hand, printing every step (the HDMI console is the only debug channel on
//! a Pi 5 without a serial adapter):
//!
//! stage 1 (`bus_up`): pin mux (the mux table differs between the BCM2712
//! C0/C1 stepping on rev 1.0 boards and D0 on rev 1.1+), power-cycle WL_ON,
//! reset the host, CMD0/CMD5/CMD3/CMD7, function 1 + ALP clock, ChipCommon
//! chip id (a CYW43455 answers 0x4345), then 4-bit bus at 25 MHz.
//!
//! stage 2 (`radio_up`): the chip's cores from its EROM, put the ARM CR4 in
//! reset, write the brcmfmac43455 firmware into its TCM (`firmware/`), the
//! NVRAM behind it, release the CPU on the image's reset vector, enable
//! function 2 (frames), then speak SDPCM + BCDC over it: ioctls / iovars on
//! the control channel (CLM blob, country, event mask, `up`), events on the
//! event channel (escan results, join progress), Ethernet frames on the data
//! channel -- what `brcmfmac` and the Pico W's cyw43 driver do, trimmed to a
//! station that scans, joins WPA2 with the firmware's own supplicant, and
//! passes Ethernet frames to `netstack` (through `netdev`).
//!
//! Untested on hardware beyond stage 1 at the time of writing (the ALP clock
//! came up; the first backplane read is where the previous attempt stopped),
//! so every stage reports what it saw.

use crate::arch::mmio::{read32, write32};
use crate::arch::timer::{delay_us, now_ms};
use crate::console_write as out;
use alloc::string::String;
use alloc::vec::Vec;

const SDIO2: usize = 0x10_0110_0000;
const SDIO2_CFG: usize = 0x10_0110_0400;
const GIO: usize = 0x10_7d50_8500; // brcmstb GPIO, bank 0 = GPIO 0..31
const PINCTRL: usize = 0x10_7d50_4100;

/// Base clock feeding the SDHCI (the device tree's fixed `clk_emmc2`).
const BASE_CLOCK_HZ: u32 = 200_000_000;

// SDHCI registers.
const BLOCK: usize = 0x04; // block size (lo 16) + block count (hi 16)
const ARG: usize = 0x08;
const XFER_CMD: usize = 0x0C; // transfer mode (lo 16) + command (hi 16)
const RESP0: usize = 0x10;
const BUF_DATA: usize = 0x20;
const PRESENT: usize = 0x24;
const HOST_CTRL: usize = 0x28; // + power 0x29, block gap 0x2A, wakeup 0x2B
const CLOCK_CTRL: usize = 0x2C; // clock (lo 16) + timeout 0x2E + reset 0x2F
const INT_STATUS: usize = 0x30;
const INT_ENABLE: usize = 0x34;
const INT_SIGNAL: usize = 0x38;
const CAPS: usize = 0x40;
const VERSION: usize = 0xFC; // slot int status (lo 16) + host version (hi 16)

// Command flags (low byte of the command register).
const RESP_NONE: u32 = 0x00;
const RESP_48: u32 = 0x02; // R4: no CRC / index check
const RESP_48_CHK: u32 = 0x1A; // R5/R6: CRC + index check
const RESP_48_BUSY: u32 = 0x1B; // R1b

// Function-1 (backplane) registers of Broadcom SDIO chips.
const SB_ADDR_LOW: u32 = 0x1000A;
const SB_ADDR_MID: u32 = 0x1000B;
const SB_ADDR_HIGH: u32 = 0x1000C;
const CHIP_CLK_CSR: u32 = 0x1000E;
const SDIO_PULLUP: u32 = 0x1000F;
const WATERMARK: u32 = 0x10008;
const DEVICE_CTL: u32 = 0x10009;
const MESBUSYCTRL: u32 = 0x1001D;
const SLEEPCSR: u32 = 0x1001F;
const FORCE_HT: u8 = 0x02;
const ALP_AVAIL_REQ: u8 = 0x08;
const HT_AVAIL_REQ: u8 = 0x10;
const FORCE_HW_CLKREQ_OFF: u8 = 0x20;
const ALP_AVAIL: u8 = 0x40;
const HT_AVAIL: u8 = 0x80;
const CHIPCOMMON: u32 = 0x1800_0000;
/// Function-1 address flag for a 32-bit backplane access (SBSDIO_SB_ACCESS_2_4B_FLAG).
const SB_ACCESS_4B: u32 = 0x8000;
/// Function-2 frames: the ChipCommon window offset with the 32-bit flag, fixed address.
const F2_ADDR: u32 = 0x8000;
const F1_BLOCK: usize = 64;
const F2_BLOCK: usize = 512;

// AI (AXI) backplane wrapper registers.
const AI_IOCTL: u32 = 0x408;
const AI_RESET_CTL: u32 = 0x800;
const IOCTL_CLK: u32 = 0x1;
const IOCTL_FGC: u32 = 0x2;
const RESET: u32 = 0x1;
const CR4_CPUHALT: u32 = 0x20;
const D11_PHYCLOCKEN: u32 = 0x4;
const D11_PHYRESET: u32 = 0x8;

// Core ids (BCMA).
const CORE_CHIPCOMMON: u32 = 0x800;
const CORE_80211: u32 = 0x812;
const CORE_SDIO_DEV: u32 = 0x829;
const CORE_ARM_CR4: u32 = 0x83E;

// SDIO device core registers (struct sdpcmd_regs).
const SD_INTSTATUS: u32 = 0x20;
const SD_HOSTINTMASK: u32 = 0x24;
const SD_TOSBMAILBOX: u32 = 0x40;
const SD_TOSBMAILBOXDATA: u32 = 0x48;
const SD_TOHOSTMAILBOXDATA: u32 = 0x4C;
const I_HMB_FRAME_IND: u32 = 1 << 6;
const I_HMB_HOST_INT: u32 = 1 << 7;
const HOSTINTMASK: u32 = 0xF0 | (1 << 13);
const SMB_INT_ACK: u32 = 1 << 1;

// SDPCM channels.
const CHAN_CONTROL: u8 = 0;
const CHAN_EVENT: u8 = 1;
const CHAN_DATA: u8 = 2;

// ioctls.
const WLC_UP: u32 = 2;
const WLC_SET_INFRA: u32 = 20;
const WLC_SET_AUTH: u32 = 22;
const WLC_SET_SSID: u32 = 26;
const WLC_DISASSOC: u32 = 52;
const WLC_SET_PM: u32 = 86;
const WLC_SET_GMODE: u32 = 110;
const WLC_SET_WSEC: u32 = 134;
const WLC_SET_BAND: u32 = 142;
const WLC_GET_VAR: u32 = 262;
const WLC_SET_VAR: u32 = 263;
const WLC_SET_WSEC_PMK: u32 = 268;

// Events.
const EV_SET_SSID: u32 = 0;
const EV_AUTH: u32 = 3;
const EV_DEAUTH: u32 = 5;
const EV_DEAUTH_IND: u32 = 6;
const EV_DISASSOC: u32 = 11;
const EV_DISASSOC_IND: u32 = 12;
const EV_LINK: u32 = 16;
const EV_PSK_SUP: u32 = 46;
const EV_ESCAN_RESULT: u32 = 69;

const FW: &[u8] = include_bytes!("../../../firmware/brcmfmac43455-sdio.bin");
const CLM: &[u8] = include_bytes!("../../../firmware/brcmfmac43455-sdio.clm_blob");
const NVRAM_TXT: &str = include_str!("../../../firmware/brcmfmac43455-sdio.txt");
const RAMBASE: u32 = 0x198000;
const RAMSIZE_DEFAULT: u32 = 0xC8000;

/// One network a scan saw.
#[derive(Clone)]
pub struct Network {
    pub ssid: String,
    pub rssi: i32,
    /// 0 open, 1 WEP, 2 WPA, 3 WPA2.
    pub secure: u8,
    pub channel: u16,
}

struct Cores {
    d11_wrap: u32,
    cr4: u32,
    cr4_wrap: u32,
    sdiod: u32,
}

struct Radio {
    cores: Cores,
    tx_seq: u8,
    tx_max: u8,
    ioctl_id: u16,
    mac: [u8; 6],
    joined: bool,
    link: bool,
    ssid: String,
    scan_done: Option<u32>,
    scan_hits: Vec<Network>,
    join_status: Option<u32>,
    psk_status: Option<u32>,
    /// The kernel's WPA2 supplicant while joining a protected network (and
    /// after, for group-key rekeys).
    sup: Option<super::wpa::Handshake>,
    sup_error: Option<&'static str>,
    /// Handshake message 1 that arrived before the association IEs were read.
    pending_eapol: Option<Vec<u8>>,
    /// The association request's RSN IE has been read back (or given up on).
    ies_known: bool,
    /// Data frames that arrived while waiting for something else.
    rx_queue: Vec<Vec<u8>>,
    frames_in: u32,
    frames_out: u32,
    /// One-line summaries of the last data frame out / in (the connection test).
    last_tx: String,
    last_rx: String,
    rx_data: u32,
}

static mut RADIO: Option<Radio> = None;
static mut BUS_TRIED: bool = false;
static mut LAST_ERROR: &str = "";

fn radio() -> Option<&'static mut Radio> {
    unsafe { (*(&raw mut RADIO)).as_mut() }
}

fn sd_rd(off: usize) -> u32 {
    unsafe { read32(SDIO2 + off) }
}
fn sd_wr(off: usize, v: u32) {
    unsafe { write32(SDIO2 + off, v) }
}

fn hex(label: &[u8], v: u64, nibbles: usize) {
    out(label);
    out(b"0x");
    let mut b = [0u8; 16];
    let n = nibbles.min(16);
    for (i, c) in b.iter_mut().enumerate().take(n) {
        let d = ((v >> ((n - 1 - i) * 4)) & 0xF) as u8;
        *c = if d < 10 { b'0' + d } else { b'a' + d - 10 };
    }
    out(&b[..n]);
    out(b"\n");
}

fn dec(label: &[u8], v: u64) {
    out(label);
    let mut b = [0u8; 20];
    let mut n = 0;
    let mut x = v;
    if x == 0 {
        b[0] = b'0';
        n = 1;
    }
    while x > 0 {
        b[n] = b'0' + (x % 10) as u8;
        x /= 10;
        n += 1;
    }
    b[..n].reverse();
    out(&b[..n]);
}

fn fail(msg: &'static str) -> Option<()> {
    unsafe { LAST_ERROR = msg };
    out(b"  wifi: ");
    out(msg.as_bytes());
    out(b"\n");
    None
}

/// Why the last bring-up step failed ("" when nothing failed yet).
pub fn last_error() -> &'static str {
    unsafe { LAST_ERROR }
}

// ── SDHCI host ───────────────────────────────────────────────────────────────

/// Set pin `pin`'s 4-bit mux field and 2-bit pull field (0 none, 1 down, 2 up).
/// (mux word, mux nibble, pad word, pad pair) per pin from pinctrl-bcm2712.c.
fn pin_set(d0: bool, pin: u32, fsel: u32, pull: u32) {
    let (mr, mb, pr, pb) = match (d0, pin) {
        (false, 28) => (3, 4, 9, 5),
        (false, 30) => (3, 6, 9, 7),
        (false, 31) => (3, 7, 9, 8),
        (false, 32) => (4, 0, 9, 9),
        (false, 33) => (4, 1, 9, 10),
        (false, 34) => (4, 2, 9, 11),
        (false, 35) => (4, 3, 9, 12),
        (true, 28) => (2, 4, 5, 10),
        (true, 30) => (2, 6, 5, 12),
        (true, 31) => (2, 7, 5, 13),
        (true, 32) => (3, 0, 5, 14),
        (true, 33) => (3, 1, 6, 0),
        (true, 34) => (3, 2, 6, 1),
        (true, 35) => (3, 3, 6, 2),
        _ => return,
    };
    unsafe {
        let m = PINCTRL + mr * 4;
        let v = read32(m) & !(0xF << (mb * 4));
        write32(m, v | (fsel << (mb * 4)));
        let p = PINCTRL + pr * 4;
        let v = read32(p) & !(0x3 << (pb * 2));
        write32(p, v | (pull << (pb * 2)));
    }
}

/// Wait until (reg & mask) == want, up to `ms` milliseconds.
fn wait(off: usize, mask: u32, want: u32, ms: u32) -> bool {
    for _ in 0..ms * 10 {
        if sd_rd(off) & mask == want {
            return true;
        }
        delay_us(100);
    }
    false
}

fn reset_cmd_line() {
    sd_wr(CLOCK_CTRL, sd_rd(CLOCK_CTRL) | (0x2 << 24));
    wait(CLOCK_CTRL, 0x2 << 24, 0, 100);
}

/// Reset the DAT line after a data error so the next transfer can go out.
fn reset_dat() {
    sd_wr(CLOCK_CTRL, sd_rd(CLOCK_CTRL) | (0x4 << 24));
    wait(CLOCK_CTRL, 0x4 << 24, 0, 100);
}

/// Issue one command; returns RESP0, or None (after printing the error bits).
fn cmd(idx: u32, arg: u32, flags: u32) -> Option<u32> {
    if !wait(PRESENT, 0x1, 0, 100) {
        out(b"    command line busy\n");
        return None;
    }
    sd_wr(INT_STATUS, 0xFFFF_FFFF);
    sd_wr(ARG, arg);
    sd_wr(XFER_CMD, ((idx << 8) | flags) << 16);
    for _ in 0..2000 {
        let st = sd_rd(INT_STATUS);
        if st & (1 << 15) != 0 {
            sd_wr(INT_STATUS, st);
            reset_cmd_line();
            out(b"    CMD");
            out(&[b'0' + (idx / 10) as u8, b'0' + (idx % 10) as u8]);
            hex(b" error, int status=", st as u64, 8);
            return None;
        }
        if st & 0x1 != 0 {
            sd_wr(INT_STATUS, 0x1);
            if flags == RESP_48_BUSY {
                wait(INT_STATUS, 0x2, 0x2, 100); // transfer complete = busy end
                sd_wr(INT_STATUS, 0x2);
            }
            return Some(sd_rd(RESP0));
        }
        delay_us(50);
    }
    out(b"    CMD");
    out(&[b'0' + (idx / 10) as u8, b'0' + (idx % 10) as u8]);
    out(b" timeout (no response)\n");
    None
}

/// CMD52 (IO_RW_DIRECT): one byte at `addr` of function `func`.
fn cmd52(write: bool, func: u32, addr: u32, data: u8) -> Option<u8> {
    let arg = ((write as u32) << 31) | ((func & 7) << 28) | ((addr & 0x1_FFFF) << 9) | data as u32;
    let r = cmd(52, arg, RESP_48_CHK)?;
    if r & 0xCB00 != 0 {
        hex(b"    CMD52 R5 flags=", ((r >> 8) & 0xFF) as u64, 2);
    }
    Some(r as u8)
}

/// CMD53 (IO_RW_EXTENDED) by PIO through the buffer port: byte mode when the
/// transfer fits one block, else block mode (`buf.len()` a multiple of
/// `block`). `incr`: incrementing address (RAM) or a FIFO (function 2 frames).
fn cmd53(write: bool, func: u32, addr: u32, incr: bool, buf: &mut [u8], block: usize) -> Option<()> {
    let n = buf.len();
    if n == 0 || (n > block && n % block != 0) || (n <= block && n > 512) {
        out(b"    CMD53: bad length\n");
        return None;
    }
    if !wait(PRESENT, 0x3, 0, 100) {
        out(b"    CMD53: command/data line busy\n");
        return None;
    }
    let (blocks, bsize) = if n > block { (n / block, block) } else { (1, n) };
    let count = if n > block { blocks as u32 } else { (n as u32) & 0x1FF };
    let arg = ((write as u32) << 31)
        | ((func & 7) << 28)
        | (((n > block) as u32) << 27)
        | ((incr as u32) << 26)
        | ((addr & 0x1_FFFF) << 9)
        | count;
    sd_wr(INT_STATUS, 0xFFFF_FFFF);
    sd_wr(BLOCK, ((blocks as u32) << 16) | bsize as u32);
    sd_wr(ARG, arg);
    // Transfer mode: data direction (bit 4 = read), multi-block + block count
    // enable for block mode. Command: 53, data present, R5 with checks.
    let mut mode = if write { 0 } else { 1 << 4 };
    if blocks > 1 {
        mode |= (1 << 5) | (1 << 1);
    }
    sd_wr(XFER_CMD, (((53 << 8) | RESP_48_CHK | (1 << 5)) << 16) | mode);
    let mut done = 0usize;
    let words = bsize.div_ceil(4);
    for _ in 0..40_000 {
        let st = sd_rd(INT_STATUS);
        if st & (1 << 15) != 0 {
            sd_wr(INT_STATUS, st);
            reset_cmd_line();
            reset_dat();
            hex(b"    CMD53 error, int status=", st as u64, 8);
            return None;
        }
        if write && st & (1 << 4) != 0 && done < n {
            sd_wr(INT_STATUS, 1 << 4);
            for _ in 0..words {
                let mut w = [0u8; 4];
                for b in w.iter_mut() {
                    if done < n {
                        *b = buf[done];
                        done += 1;
                    }
                }
                sd_wr(BUF_DATA, u32::from_le_bytes(w));
            }
        }
        if !write && st & (1 << 5) != 0 && done < n {
            sd_wr(INT_STATUS, 1 << 5);
            for _ in 0..words {
                let w = sd_rd(BUF_DATA).to_le_bytes();
                for b in w {
                    if done < n {
                        buf[done] = b;
                        done += 1;
                    }
                }
            }
        }
        if st & (1 << 1) != 0 && done >= n {
            sd_wr(INT_STATUS, 0x3);
            return Some(());
        }
        delay_us(20);
    }
    reset_dat();
    hex(b"    CMD53 timeout, int status=", sd_rd(INT_STATUS) as u64, 8);
    None
}

fn cmd53_read(func: u32, addr: u32, incr: bool, buf: &mut [u8], block: usize) -> Option<()> {
    cmd53(false, func, addr, incr, buf, block)
}

fn cmd53_write(func: u32, addr: u32, incr: bool, data: &[u8], block: usize) -> Option<()> {
    let mut tmp: Vec<u8> = data.to_vec();
    cmd53(true, func, addr, incr, &mut tmp, block)
}

// ── backplane (function 1) ───────────────────────────────────────────────────

static mut WINDOW: u32 = 0xFFFF_FFFF;

/// Point the function-1 backplane window at `addr`'s 32 KiB page.
fn set_window(addr: u32) -> Option<()> {
    let win = addr & !0x7FFF;
    if unsafe { WINDOW } == win {
        return Some(());
    }
    cmd52(true, 1, SB_ADDR_LOW, ((win >> 8) & 0x80) as u8)?;
    cmd52(true, 1, SB_ADDR_MID, ((win >> 16) & 0xFF) as u8)?;
    cmd52(true, 1, SB_ADDR_HIGH, ((win >> 24) & 0xFF) as u8)?;
    unsafe { WINDOW = win };
    Some(())
}

fn bp_read32(addr: u32) -> Option<u32> {
    set_window(addr)?;
    let mut b4 = [0u8; 4];
    if cmd53_read(1, (addr & 0x7FFF) | SB_ACCESS_4B, true, &mut b4, F1_BLOCK).is_some() {
        return Some(u32::from_le_bytes(b4));
    }
    // Four byte reads as a fallback (what an older bring-up used).
    let mut v = 0u32;
    for i in 0..4 {
        let b = cmd52(false, 1, (addr & 0x7FFF) + i, 0)?;
        v |= (b as u32) << (8 * i);
    }
    Some(v)
}

fn bp_write32(addr: u32, v: u32) -> Option<()> {
    set_window(addr)?;
    cmd53_write(1, (addr & 0x7FFF) | SB_ACCESS_4B, true, &v.to_le_bytes(), F1_BLOCK)
}

/// Write `data` into the chip's RAM at backplane address `addr`, 64 bytes a
/// command, never crossing a window.
fn ram_write(addr: u32, data: &[u8]) -> Option<()> {
    let mut off = 0usize;
    let mut next_dot = 0usize;
    while off < data.len() {
        let a = addr + off as u32;
        set_window(a)?;
        let room = 0x8000 - (a & 0x7FFF) as usize;
        let n = (data.len() - off).min(F1_BLOCK).min(room);
        cmd53_write(1, (a & 0x7FFF) | SB_ACCESS_4B, true, &data[off..off + n], F1_BLOCK)?;
        off += n;
        if off >= next_dot {
            out(b".");
            next_dot += 65_536;
        }
    }
    Some(())
}

fn ram_read(addr: u32, buf: &mut [u8]) -> Option<()> {
    let mut off = 0usize;
    while off < buf.len() {
        let a = addr + off as u32;
        set_window(a)?;
        let room = 0x8000 - (a & 0x7FFF) as usize;
        let n = (buf.len() - off).min(F1_BLOCK).min(room);
        cmd53_read(1, (a & 0x7FFF) | SB_ACCESS_4B, true, &mut buf[off..off + n], F1_BLOCK)?;
        off += n;
    }
    Some(())
}

fn ai_coredisable(wrap: u32, prereset: u32, reset: u32) -> Option<()> {
    if bp_read32(wrap + AI_RESET_CTL)? & RESET == 0 {
        bp_write32(wrap + AI_IOCTL, prereset | IOCTL_FGC | IOCTL_CLK)?;
        bp_read32(wrap + AI_IOCTL)?;
        bp_write32(wrap + AI_RESET_CTL, RESET)?;
        delay_us(20);
        for _ in 0..30 {
            if bp_read32(wrap + AI_RESET_CTL)? & RESET != 0 {
                break;
            }
            delay_us(10);
        }
    }
    bp_write32(wrap + AI_IOCTL, reset | IOCTL_FGC | IOCTL_CLK)?;
    bp_read32(wrap + AI_IOCTL)?;
    Some(())
}

fn ai_resetcore(wrap: u32, prereset: u32, reset: u32, postreset: u32) -> Option<()> {
    ai_coredisable(wrap, prereset, reset)?;
    for _ in 0..50 {
        if bp_read32(wrap + AI_RESET_CTL)? & RESET == 0 {
            break;
        }
        bp_write32(wrap + AI_RESET_CTL, 0)?;
        delay_us(60);
    }
    bp_write32(wrap + AI_IOCTL, postreset | IOCTL_CLK)?;
    bp_read32(wrap + AI_IOCTL)?;
    Some(())
}

/// The cores this driver needs, from the chip's enumeration ROM.
fn erom_scan() -> Option<Cores> {
    let mut erom = bp_read32(CHIPCOMMON + 0xFC)?;
    hex(b"  EROM at ", erom as u64, 8);
    let mut found = [(0u32, 0u32, 0u32); 4]; // (id, base, wrap) for cc, d11, cr4, sdiod
    let want = [CORE_CHIPCOMMON, CORE_80211, CORE_ARM_CR4, CORE_SDIO_DEV];
    let next = |erom: &mut u32| -> Option<u32> {
        let v = bp_read32(*erom)?;
        *erom += 4;
        Some(v)
    };
    for _ in 0..512 {
        let v = next(&mut erom)?;
        if v & 0xF == 0xF {
            break;
        }
        if v & 0x7 != 0x1 {
            continue; // not a component descriptor
        }
        let id = (v >> 8) & 0xFFF;
        let v2 = next(&mut erom)?;
        let nmw = (v2 >> 22) & 0x1F;
        let nsw = (v2 >> 27) & 0x1F;
        if nmw + nsw == 0 && id != 0x827 && id != 0x840 {
            continue;
        }
        // Address descriptors: the first slave port is the register base, the
        // first wrapper port the wrapper base.
        let (mut base, mut wrap) = (0u32, 0u32);
        loop {
            let d = next(&mut erom)?;
            let t = d & 0x7;
            if t == 0x3 {
                continue; // master port
            }
            if t != 0x5 {
                erom -= 4; // not an address descriptor: belongs to the next component
                break;
            }
            let stype = (d >> 6) & 0x3;
            if d & 0x8 != 0 {
                next(&mut erom)?; // high half of a 64-bit address
            }
            let sz = (d >> 4) & 0x3;
            if sz == 3 {
                let s = next(&mut erom)?;
                if s & 0x8 != 0 {
                    next(&mut erom)?;
                }
                continue;
            }
            if sz > 1 {
                continue; // only 4K / 8K register regions
            }
            if base == 0 && stype == 0 {
                base = d & 0xFFFF_F000;
            }
            if wrap == 0 && (stype == 2 || stype == 3) {
                wrap = d & 0xFFFF_F000;
            }
        }
        for (k, w) in want.iter().enumerate() {
            if id == *w && found[k].1 == 0 {
                found[k] = (id, base, wrap);
            }
        }
    }
    for (k, (id, base, wrap)) in found.iter().enumerate() {
        let name: &[u8] = [&b"chipcommon"[..], b"80211", b"arm-cr4", b"sdiod"][k];
        out(b"  core ");
        out(name);
        if *id == 0 {
            out(b": not found\n");
        } else {
            hex(b" base=", *base as u64, 8);
            hex(b"      wrap=", *wrap as u64, 8);
        }
    }
    if found[1].1 == 0 || found[2].1 == 0 || found[3].1 == 0 {
        fail("EROM scan did not find the 80211 / CR4 / SDIO cores");
        return None;
    }
    Some(Cores { d11_wrap: found[1].2, cr4: found[2].1, cr4_wrap: found[2].2, sdiod: found[3].1 })
}

/// The CR4's tightly coupled memory size from its bank registers.
fn tcm_ramsize(c: &Cores) -> u32 {
    let Some(cap) = bp_read32(c.cr4 + 0x04) else { return RAMSIZE_DEFAULT };
    let banks = (cap & 0xF) + ((cap >> 4) & 0xF);
    let mut size = 0u32;
    for idx in 0..banks {
        if bp_write32(c.cr4 + 0x40, idx).is_none() {
            return RAMSIZE_DEFAULT;
        }
        let Some(info) = bp_read32(c.cr4 + 0x44) else { return RAMSIZE_DEFAULT };
        size += ((info & 0x3F) + 1) * 8192;
    }
    if size == 0 || size > 0x20_0000 {
        RAMSIZE_DEFAULT
    } else {
        size
    }
}

// ── stage 1: the bus ─────────────────────────────────────────────────────────

/// Bring the SDIO bus and the radio chip up (idempotent); the chip id.
fn bus_up() -> Option<u32> {
    if crate::arch::mmio::board() != crate::arch::mmio::RpiBoard::Pi5 {
        out(b"wifi: this driver is for the Raspberry Pi 5 (CYW43455 on BCM2712 SDIO2)\n");
        unsafe { LAST_ERROR = "no Wi-Fi radio on this board (Pi 5 only)" };
        return None;
    }
    out(b"Wi-Fi (Pi 5: CYW43455 on SDIO2)\n");

    // 1. Pin mux for the stepping this board carries.
    let rev = crate::arch::mailbox::board_revision().unwrap_or(0);
    hex(b"  board revision=", rev as u64, 6);
    let d0 = rev & 0xF >= 1;
    out(if d0 { b"  BCM2712 D0 pin table (rev 1.1+)\n" } else { b"  BCM2712 C0/C1 pin table (rev 1.0)\n" });
    let sd2: [u32; 6] = if d0 { [1, 1, 1, 1, 1, 1] } else { [4, 4, 4, 3, 4, 3] };
    for (i, pin) in (30..=35u32).enumerate() {
        pin_set(d0, pin, sd2[i], if pin == 30 { 0 } else { 2 });
    }
    pin_set(d0, 28, 0, 0); // WL_ON as a plain GPIO

    // 2. Power-cycle the radio: WL_ON low, then high (+150 ms startup per DT).
    unsafe {
        write32(GIO + 0x08, read32(GIO + 0x08) & !(1 << 28)); // IODIR: output
        write32(GIO + 0x04, read32(GIO + 0x04) & !(1 << 28));
        delay_us(20_000);
        write32(GIO + 0x04, read32(GIO + 0x04) | (1 << 28));
    }
    delay_us(150_000);
    out(b"  WL_ON (GPIO 28) high: radio powered\n");

    // 3. Host controller.
    hex(b"  SDHCI version=", (sd_rd(VERSION) >> 16) as u64, 4);
    hex(b"  capabilities=", sd_rd(CAPS) as u64, 8);
    unsafe {
        // Non-removable: force card-present; pins in SD (not eMMC) mode.
        let c = read32(SDIO2_CFG) & !(1 << 30);
        write32(SDIO2_CFG, c | (1 << 31));
        let p = read32(SDIO2_CFG + 0x44) & !0x3;
        write32(SDIO2_CFG + 0x44, p | 0x2);
    }
    sd_wr(CLOCK_CTRL, 1 << 24); // software reset (all)
    if !wait(CLOCK_CTRL, 1 << 24, 0, 100) {
        fail("SDHCI reset timeout");
        return None;
    }
    unsafe { WINDOW = 0xFFFF_FFFF };
    // 3.3 V bus power (power control at 0x29), 1-bit bus for identification.
    sd_wr(HOST_CTRL, 0x0F << 8);
    delay_us(10_000);
    set_clock(400_000)?;
    sd_wr(INT_ENABLE, 0xFFFF_FFFF);
    sd_wr(INT_SIGNAL, 0);
    delay_us(10_000);
    hex(b"  present state=", sd_rd(PRESENT) as u64, 8);

    // 4. SDIO card init.
    cmd(0, 0, RESP_NONE);
    delay_us(2_000);
    let Some(ocr) = cmd(5, 0, RESP_48) else {
        fail("no answer to CMD5: radio not reachable on SDIO2");
        return None;
    };
    hex(b"  CMD5 OCR=", ocr as u64, 8);
    let mut ready = ocr;
    for _ in 0..100 {
        match cmd(5, ocr & 0x00FF_FF00, RESP_48) {
            Some(r) if r & (1 << 31) != 0 => {
                ready = r;
                break;
            },
            Some(r) => ready = r,
            None => return None,
        }
        delay_us(10_000);
    }
    if ready & (1 << 31) == 0 {
        fail("card never reported ready");
        return None;
    }
    out(b"  SDIO card ready, functions=");
    out(&[b'0' + ((ready >> 28) & 7) as u8, b'\n']);
    let rca = cmd(3, 0, RESP_48_CHK)? >> 16;
    hex(b"  RCA=", rca as u64, 4);
    cmd(7, rca << 16, RESP_48_BUSY)?;
    out(b"  card selected\n");

    // 4-bit bus at 25 MHz (brcmfmac runs 50 MHz; 25 keeps margin).
    let bic = cmd52(false, 0, 0x07, 0)?;
    cmd52(true, 0, 0x07, (bic & !0x3) | 0x02)?;
    sd_wr(HOST_CTRL, (sd_rd(HOST_CTRL) & !0x4) | 0x2);
    set_clock(25_000_000)?;
    out(b"  4-bit bus at 25 MHz\n");
    // Block sizes: 64 for the backplane function, 512 for frames.
    cmd52(true, 0, 0x110, F1_BLOCK as u8)?;
    cmd52(true, 0, 0x111, 0)?;
    cmd52(true, 0, 0x210, 0)?;
    cmd52(true, 0, 0x211, (F2_BLOCK >> 8) as u8)?;

    // 5. Function 1 (backplane), ALP clock, ChipCommon chip ID.
    let cccr = cmd52(false, 0, 0x00, 0)?;
    hex(b"  CCCR/SDIO revision=", cccr as u64, 2);
    cmd52(true, 0, 0x02, 0x02)?; // IOEx: enable F1
    let mut f1 = false;
    for _ in 0..100 {
        if cmd52(false, 0, 0x03, 0)? & 0x02 != 0 {
            f1 = true;
            break;
        }
        delay_us(10_000);
    }
    out(if f1 { b"  function 1 ready\n" } else { b"  function 1 did not come ready\n" });
    cmd52(true, 1, SDIO_PULLUP, 0)?;
    cmd52(true, 1, CHIP_CLK_CSR, FORCE_HW_CLKREQ_OFF | ALP_AVAIL_REQ)?;
    let mut alp = false;
    for _ in 0..100 {
        if cmd52(false, 1, CHIP_CLK_CSR, 0)? & ALP_AVAIL != 0 {
            alp = true;
            break;
        }
        delay_us(1_000);
    }
    out(if alp { b"  ALP clock available\n" } else { b"  ALP clock not available (continuing)\n" });
    cmd52(true, 1, CHIP_CLK_CSR, 0)?; // leave the chip to manage its clocks
    // Keep SDIO on across the chip's sleep states.
    let kso = cmd52(false, 1, SLEEPCSR, 0)?;
    if kso & 0x1 == 0 {
        cmd52(true, 1, SLEEPCSR, kso | 0x1)?;
    }
    let Some(id) = bp_read32(CHIPCOMMON) else {
        fail("backplane read of the chip id failed");
        return None;
    };
    hex(b"  ChipCommon chipid=", id as u64, 8);
    let chip = id & 0xFFFF;
    if chip == 0x4345 {
        out(b"  -> CYW43455 Wi-Fi radio found (stage 1 OK)\n");
    } else {
        hex(b"  -> unexpected chip id ", chip as u64, 4);
    }
    Some(id)
}

fn set_clock(hz: u32) -> Option<()> {
    // Divided-clock mode, f = base / (2N); stop, reprogram, restart.
    let n = if hz >= BASE_CLOCK_HZ { 0 } else { BASE_CLOCK_HZ.div_ceil(2 * hz) };
    let clk = ((n & 0xFF) << 8) | (((n >> 8) & 0x3) << 6) | 0x1;
    let timeout = (sd_rd(CLOCK_CTRL) & 0x000F_0000) | (0x0E << 16);
    sd_wr(CLOCK_CTRL, timeout | clk);
    if !wait(CLOCK_CTRL, 0x2, 0x2, 100) {
        fail("internal clock never stabilised");
        return None;
    }
    sd_wr(CLOCK_CTRL, sd_rd(CLOCK_CTRL) | 0x4); // SD clock on
    delay_us(2_000);
    Some(())
}

// ── stage 2: firmware ────────────────────────────────────────────────────────

/// The NVRAM image the firmware reads: `key=value` entries NUL-separated,
/// a terminating NUL, 4-byte padding and the length token brcmfmac appends.
fn build_nvram() -> Vec<u8> {
    let mut v: Vec<u8> = Vec::with_capacity(NVRAM_TXT.len() + 16);
    for line in NVRAM_TXT.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') || !l.contains('=') {
            continue;
        }
        v.extend_from_slice(l.as_bytes());
        v.push(0);
    }
    v.push(0);
    while v.len() % 4 != 0 {
        v.push(0);
    }
    let token = (v.len() / 4) as u32;
    let token = (!token << 16) | (token & 0xFFFF);
    v.extend_from_slice(&token.to_le_bytes());
    v
}

fn request_clock(req: u8, avail: u8, ms: u32) -> bool {
    if cmd52(true, 1, CHIP_CLK_CSR, req).is_none() {
        return false;
    }
    for _ in 0..ms {
        if let Some(v) = cmd52(false, 1, CHIP_CLK_CSR, 0) {
            if v & avail != 0 {
                return true;
            }
        }
        delay_us(1_000);
    }
    false
}

fn load_firmware(c: &Cores) -> Option<()> {
    if FW.len() < 64 {
        return fail("firmware image missing from this build");
    }
    out(if request_clock(HT_AVAIL_REQ, HT_AVAIL, 2_000) { b"  HT clock available\n" } else { b"  HT clock not reported (using ALP)\n" });
    // ARM CR4 into reset (keeping only its halt bit), the 802.11 core reset
    // with its PHY clock on: what brcmfmac calls the chip's passive state.
    let ioctl = bp_read32(c.cr4_wrap + AI_IOCTL)? & CR4_CPUHALT;
    ai_resetcore(c.cr4_wrap, ioctl, CR4_CPUHALT, CR4_CPUHALT)?;
    ai_resetcore(c.d11_wrap, D11_PHYRESET | D11_PHYCLOCKEN, D11_PHYCLOCKEN, D11_PHYCLOCKEN)?;
    out(b"  CR4 halted, 80211 core reset\n");
    let ramsize = tcm_ramsize(c);
    dec(b"  TCM: ", ramsize as u64);
    out(b" bytes at 0x198000\n");
    dec(b"  writing firmware (", FW.len() as u64);
    out(b" bytes) ");
    let t0 = now_ms();
    ram_write(RAMBASE, FW)?;
    dec(b" done in ", now_ms() - t0);
    out(b" ms\n");
    let mut check = [0u8; 64];
    ram_read(RAMBASE, &mut check)?;
    if check != FW[..64] {
        return fail("firmware read-back mismatch (RAM window writes are off)");
    }
    out(b"  firmware verified (first 64 bytes)\n");
    let nvram = build_nvram();
    if nvram.len() as u32 + FW.len() as u32 > ramsize {
        return fail("firmware + NVRAM larger than the chip's RAM");
    }
    let vaddr = RAMBASE + ramsize - nvram.len() as u32;
    dec(b"  NVRAM: ", nvram.len() as u64);
    hex(b" bytes at ", vaddr as u64, 8);
    ram_write(vaddr, &nvram)?;
    out(b"\n");
    // Activate: clear the SDIO core's interrupts, the reset vector at 0, run.
    let rstvec = u32::from_le_bytes([FW[0], FW[1], FW[2], FW[3]]);
    bp_write32(c.sdiod + SD_INTSTATUS, 0xFFFF_FFFF)?;
    ram_write(0, &rstvec.to_le_bytes())?;
    ai_resetcore(c.cr4_wrap, CR4_CPUHALT, 0, 0)?;
    hex(b"  CR4 released on reset vector ", rstvec as u64, 8);
    Some(())
}

fn start_f2(c: &Cores) -> Option<()> {
    out(if request_clock(HT_AVAIL_REQ, HT_AVAIL, 3_000) { b"  HT clock up (firmware running)\n" } else { b"  HT clock did not come up after the firmware start\n" });
    let saveclk = cmd52(false, 1, CHIP_CLK_CSR, 0)?;
    cmd52(true, 1, CHIP_CLK_CSR, saveclk | FORCE_HT)?;
    bp_write32(c.sdiod + SD_HOSTINTMASK, HOSTINTMASK)?;
    bp_write32(c.sdiod + SD_TOSBMAILBOXDATA, 4 << 16)?; // SDPCM protocol version 4
    let ioe = cmd52(false, 0, 0x02, 0)?;
    cmd52(true, 0, 0x02, ioe | 0x04)?;
    let mut ready = false;
    for _ in 0..350 {
        if cmd52(false, 0, 0x03, 0)? & 0x04 != 0 {
            ready = true;
            break;
        }
        delay_us(10_000);
    }
    if !ready {
        cmd52(true, 0, 0x02, ioe)?;
        return fail("function 2 never came ready: the firmware did not start");
    }
    out(b"  function 2 ready: firmware is up\n");
    cmd52(true, 1, WATERMARK, 0x60)?;
    let devctl = cmd52(false, 1, DEVICE_CTL, 0)?;
    cmd52(true, 1, DEVICE_CTL, devctl | 0x10)?;
    cmd52(true, 1, MESBUSYCTRL, 0x60 | 0x80)?;
    cmd52(true, 1, CHIP_CLK_CSR, saveclk)?;
    Some(())
}

// ── SDPCM frames ─────────────────────────────────────────────────────────────

fn has_credit(r: &Radio) -> bool {
    r.tx_seq != r.tx_max && (r.tx_max.wrapping_sub(r.tx_seq)) & 0x80 == 0
}

fn send_frame(chan: u8, payload: &[u8]) -> Option<()> {
    let t0 = now_ms();
    while !has_credit(radio()?) {
        poll_rx();
        if now_ms() - t0 > 2_000 {
            return fail("no transmit credit from the firmware");
        }
    }
    let r = radio()?;
    let len = 12 + payload.len();
    let mut total = len.div_ceil(4) * 4;
    if total > F2_BLOCK {
        total = total.div_ceil(F2_BLOCK) * F2_BLOCK;
    }
    let mut f: Vec<u8> = Vec::with_capacity(total);
    f.extend_from_slice(&(len as u16).to_le_bytes());
    f.extend_from_slice(&(!(len as u16)).to_le_bytes());
    f.extend_from_slice(&[r.tx_seq, chan, 0, 12, 0, 0, 0, 0]);
    f.extend_from_slice(payload);
    f.resize(total, 0);
    r.tx_seq = r.tx_seq.wrapping_add(1);
    r.frames_out += 1;
    cmd53_write(2, F2_ADDR, false, &f, F2_BLOCK)
}

/// One frame off function 2: (channel, bytes after the SDPCM headers).
fn read_frame() -> Option<(u8, Vec<u8>)> {
    let mut hdr = [0u8; 32];
    cmd53_read(2, F2_ADDR, false, &mut hdr, F2_BLOCK)?;
    let len = u16::from_le_bytes([hdr[0], hdr[1]]) as usize;
    let chk = u16::from_le_bytes([hdr[2], hdr[3]]);
    if len == 0 {
        return None;
    }
    if (len as u16) ^ chk != 0xFFFF {
        hex(b"  wifi: bad frame header ", u32::from_le_bytes([hdr[0], hdr[1], hdr[2], hdr[3]]) as u64, 8);
        return None;
    }
    let mut frame: Vec<u8> = hdr.to_vec();
    if len > 32 {
        let mut rest = (len - 32).div_ceil(4) * 4;
        if rest > F2_BLOCK {
            rest = rest.div_ceil(F2_BLOCK) * F2_BLOCK;
        }
        let mut more = alloc::vec![0u8; rest];
        cmd53_read(2, F2_ADDR, false, &mut more, F2_BLOCK)?;
        frame.extend_from_slice(&more);
    }
    let r = radio()?;
    r.tx_max = frame[9];
    r.frames_in += 1;
    let chan = frame[5] & 0xF;
    let doff = frame[7] as usize;
    if len < 12 || doff < 12 || doff > len {
        return None; // a credit-only / glom header: nothing to deliver
    }
    frame.truncate(len);
    Some((chan, frame[doff..].to_vec()))
}

/// Drain pending frames: control frames are returned to the ioctl waiter,
/// events handled, data queued for `receive`.
fn poll_rx() -> Option<Vec<u8>> {
    let sdiod = radio()?.cores.sdiod;
    let st = bp_read32(sdiod + SD_INTSTATUS)?;
    if st & (I_HMB_FRAME_IND | I_HMB_HOST_INT) == 0 {
        return None;
    }
    bp_write32(sdiod + SD_INTSTATUS, st & (I_HMB_FRAME_IND | I_HMB_HOST_INT))?;
    if st & I_HMB_HOST_INT != 0 {
        // Mailbox from the firmware (fw-ready / version); acknowledge it.
        let _ = bp_read32(sdiod + SD_TOHOSTMAILBOXDATA);
        let _ = bp_write32(sdiod + SD_TOSBMAILBOX, SMB_INT_ACK);
    }
    let mut control: Option<Vec<u8>> = None;
    for _ in 0..32 {
        let Some((chan, payload)) = read_frame() else { break };
        match chan {
            CHAN_CONTROL => control = Some(payload),
            CHAN_EVENT => handle_event(&payload),
            CHAN_DATA => {
                if payload.len() > 4 {
                    let doff = 4 + (payload[3] as usize) * 4;
                    if payload.len() > doff {
                        let eth = &payload[doff..];
                        if eth.len() >= 14 && eth[12] == 0x88 && eth[13] == 0x8E {
                            supplicant_rx(eth);
                        } else {
                            let r = radio()?;
                            if r.rx_queue.len() < 64 {
                                r.rx_queue.push(eth.to_vec());
                            }
                        }
                    }
                }
            },
            _ => {},
        }
    }
    control
}

fn be32(b: &[u8], i: usize) -> u32 {
    u32::from_be_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn le32(b: &[u8], i: usize) -> u32 {
    u32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]])
}
fn le16(b: &[u8], i: usize) -> u16 {
    u16::from_le_bytes([b[i], b[i + 1]])
}

/// An EAPOL frame from the access point: run it through the kernel's WPA2
/// supplicant and carry out what it asks for (replies, keys to install).
/// After associating: the RSN IE the firmware put in the association request
/// goes to the supplicant (message 2 must repeat it byte for byte), then a
/// message 1 that arrived meanwhile is handled.
fn assoc_ies_learned() {
    let ies = get_iovar("assoc_req_ies", 512).unwrap_or_default();
    let mut i = 0usize;
    let mut found = false;
    while i + 2 <= ies.len() {
        let (id, l) = (ies[i], ies[i + 1] as usize);
        if i + 2 + l > ies.len() {
            break;
        }
        if id == 0x30 {
            if let Some(sup) = radio().and_then(|r| r.sup.as_mut()) {
                sup.set_rsn_ie(&ies[i..i + 2 + l]);
                out(b"  wifi: association RSN IE taken from the firmware (");
                dec(b"", l as u64 + 2);
                out(b" bytes)\n");
                found = true;
            }
            break;
        }
        i += 2 + l;
    }
    if !found {
        out(b"  wifi: no RSN IE in the association request; using the default\n");
    }
    let Some(r) = radio() else { return };
    r.ies_known = true;
    if let Some(f) = r.pending_eapol.take() {
        supplicant_rx(&f);
    }
}

fn supplicant_rx(frame: &[u8]) {
    let Some(r) = radio() else { return };
    if !r.ies_known {
        // Message 1 can beat the SET_SSID event; answer it once the IEs are known.
        r.pending_eapol = Some(frame.to_vec());
        return;
    }
    let Some(sup) = r.sup.as_mut() else { return };
    let actions = match sup.rx(frame) {
        Ok(a) => a,
        Err(e) => {
            out(b"wifi: handshake: ");
            out(e.as_bytes());
            out(b"\n");
            r.sup_error = Some(e);
            r.psk_status = Some(0xFFFF);
            return;
        },
    };
    let ap = sup.ap();
    for a in actions {
        match a {
            super::wpa::Action::Send(f) => {
                if send_eth(&f).is_none() {
                    out(b"wifi: could not send a handshake message\n");
                }
            },
            super::wpa::Action::Pairwise(tk) => {
                // Let message 4 leave unencrypted before the key is in.
                delay_us(20_000);
                if let Err(e) = install_key(0, &tk, 0, &ap, None) {
                    dec(b"wifi: pairwise key refused, code -", e.unsigned_abs() as u64);
                    out(b"\n");
                }
            },
            super::wpa::Action::Group { index, key, tx, rsc } => {
                if let Err(e) = install_key(index as u32, &key, if tx { 2 } else { 0 }, &[0; 6], Some(rsc)) {
                    dec(b"wifi: group key refused, code -", e.unsigned_abs() as u64);
                    out(b"\n");
                }
            },
            super::wpa::Action::Done => {
                out(b"wifi: WPA2 handshake complete\n");
                let Some(r) = radio() else { return };
                r.psk_status = Some(6);
            },
        }
    }
}

/// `wsec_key`: brcmfmac's `brcmf_wsec_key_le` (164 bytes). CCMP; `ea` is the
/// access point for the pairwise key, zero for a group key; a group key's
/// receive sequence counter comes from the handshake message.
fn install_key(index: u32, key: &[u8], flags: u32, ea: &[u8; 6], rsc: Option<[u8; 8]>) -> Result<(), i32> {
    let mut b = [0u8; 164];
    b[0..4].copy_from_slice(&index.to_le_bytes());
    b[4..8].copy_from_slice(&(key.len() as u32).to_le_bytes());
    let n = key.len().min(32);
    b[8..8 + n].copy_from_slice(&key[..n]);
    b[112..116].copy_from_slice(&4u32.to_le_bytes()); // CRYPTO_ALGO_AES_CCM
    b[116..120].copy_from_slice(&flags.to_le_bytes());
    if let Some(rsc) = rsc {
        b[132..136].copy_from_slice(&1u32.to_le_bytes()); // iv_initialized
        b[140..144].copy_from_slice(&rsc[2..6]); // rxiv.hi
        b[144..146].copy_from_slice(&rsc[0..2]); // rxiv.lo
    }
    b[156..162].copy_from_slice(ea);
    set_iovar("wsec_key", &b)
}

fn handle_event(payload: &[u8]) {
    // BDC header, Ethernet header, Broadcom event header, event message.
    if payload.len() < 4 {
        return;
    }
    let p = &payload[4 + (payload[3] as usize) * 4..];
    if p.len() < 14 + 10 + 48 || u16::from_be_bytes([p[12], p[13]]) != 0x886C {
        return;
    }
    let m = &p[24..];
    let flags = u16::from_be_bytes([m[2], m[3]]);
    let ev = be32(m, 4);
    let status = be32(m, 8);
    let datalen = be32(m, 20) as usize;
    let data = &p[72..];
    let data = &data[..datalen.min(data.len())];
    let Some(r) = radio() else { return };
    match ev {
        EV_ESCAN_RESULT => {
            if status == 8 {
                parse_escan(r, data);
            } else {
                r.scan_done = Some(status);
            }
        },
        EV_SET_SSID => r.join_status = Some(status),
        EV_LINK => {
            r.link = flags & 1 != 0;
            if !r.link && r.joined {
                out(b"wifi: link down\n");
                r.joined = false;
            }
        },
        EV_PSK_SUP => r.psk_status = Some(status),
        EV_AUTH => {},
        EV_DEAUTH | EV_DEAUTH_IND | EV_DISASSOC | EV_DISASSOC_IND => {
            if r.joined {
                out(b"wifi: disconnected by the access point\n");
            }
            r.joined = false;
            r.link = false;
        },
        _ => {},
    }
}

fn parse_escan(r: &mut Radio, data: &[u8]) {
    if data.len() < 12 {
        return;
    }
    let count = le16(data, 10) as usize;
    let mut off = 12usize;
    for _ in 0..count {
        if off + 128 > data.len() {
            break;
        }
        let b = &data[off..];
        let blen = le32(b, 4) as usize;
        if blen < 128 || off + blen > data.len() {
            break;
        }
        let ssid_len = (b[18] as usize).min(32);
        let ssid = String::from_utf8_lossy(&b[19..19 + ssid_len]).into_owned();
        let cap = le16(b, 16);
        let chanspec = le16(b, 72);
        let rssi = le16(b, 78) as i16 as i32;
        let ie_off = le16(b, 116) as usize;
        let ie_len = le32(b, 120) as usize;
        let mut secure = if cap & 0x10 != 0 { 1u8 } else { 0u8 };
        if ie_off + ie_len <= blen && ie_len > 0 {
            let ies = &b[ie_off..ie_off + ie_len];
            let mut i = 0usize;
            while i + 2 <= ies.len() {
                let (id, l) = (ies[i], ies[i + 1] as usize);
                if i + 2 + l > ies.len() {
                    break;
                }
                if id == 48 {
                    // RSN: version, group cipher, pairwise suites, AKM suites.
                    // PSK (2 / 6) joins with a passphrase; SAE-only (8) is WPA3.
                    let rsn = &ies[i + 2..i + 2 + l];
                    let (mut psk, mut sae) = (false, false);
                    if rsn.len() >= 8 {
                        let pc = u16::from_le_bytes([rsn[6], rsn[7]]) as usize;
                        let ak = 8 + pc * 4;
                        if rsn.len() >= ak + 2 {
                            let ac = u16::from_le_bytes([rsn[ak], rsn[ak + 1]]) as usize;
                            for k in 0..ac {
                                let o = ak + 2 + k * 4;
                                if rsn.len() >= o + 4 && rsn[o..o + 3] == [0x00, 0x0F, 0xAC] {
                                    match rsn[o + 3] {
                                        2 | 6 => psk = true,
                                        8 | 9 => sae = true,
                                        _ => {},
                                    }
                                }
                            }
                        }
                    }
                    secure = if sae && !psk { 4 } else { 3 };
                } else if id == 221 && l >= 4 && ies[i + 2..i + 6] == [0x00, 0x50, 0xF2, 0x01] && secure < 3 {
                    secure = 2;
                }
                i += 2 + l;
            }
        }
        let channel = chanspec & 0xFF;
        if !ssid.is_empty() {
            match r.scan_hits.iter_mut().find(|n| n.ssid == ssid) {
                Some(n) => {
                    if rssi > n.rssi {
                        n.rssi = rssi;
                        n.channel = channel;
                        n.secure = secure;
                    }
                },
                None => r.scan_hits.push(Network { ssid, rssi, secure, channel }),
            }
        }
        off += blen;
    }
}

// ── BCDC control channel ─────────────────────────────────────────────────────

/// An ioctl: returns the response bytes, or the firmware's error code.
fn ioctl(set: bool, cmd: u32, data: &[u8]) -> Result<Vec<u8>, i32> {
    let r = radio().ok_or(-1)?;
    r.ioctl_id = r.ioctl_id.wrapping_add(1);
    if r.ioctl_id == 0 {
        r.ioctl_id = 1;
    }
    let id = r.ioctl_id;
    let flags = ((id as u32) << 16) | if set { 0x2 } else { 0 };
    let mut p: Vec<u8> = Vec::with_capacity(16 + data.len());
    p.extend_from_slice(&cmd.to_le_bytes());
    p.extend_from_slice(&(data.len() as u32).to_le_bytes());
    p.extend_from_slice(&flags.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes());
    p.extend_from_slice(data);
    if send_frame(CHAN_CONTROL, &p).is_none() {
        return Err(-2);
    }
    let t0 = now_ms();
    loop {
        if let Some(resp) = poll_rx() {
            if resp.len() >= 16 {
                let rflags = le32(&resp, 8);
                let rid = (rflags >> 16) as u16;
                let status = le32(&resp, 12) as i32;
                if rid == id {
                    if rflags & 1 != 0 {
                        return Err(status);
                    }
                    return Ok(resp[16..].to_vec());
                }
            }
        }
        if now_ms() - t0 > 3_000 {
            return Err(-3);
        }
        delay_us(200);
    }
}

fn ioctl_set_u32(cmd: u32, v: u32) -> Result<(), i32> {
    ioctl(true, cmd, &v.to_le_bytes()).map(|_| ())
}

fn set_iovar(name: &str, data: &[u8]) -> Result<(), i32> {
    let mut p: Vec<u8> = Vec::with_capacity(name.len() + 1 + data.len());
    p.extend_from_slice(name.as_bytes());
    p.push(0);
    p.extend_from_slice(data);
    ioctl(true, WLC_SET_VAR, &p).map(|_| ())
}

fn set_iovar_u32(name: &str, v: u32) -> Result<(), i32> {
    set_iovar(name, &v.to_le_bytes())
}

fn set_iovar_u32x2(name: &str, a: u32, b: u32) -> Result<(), i32> {
    let mut d = [0u8; 8];
    d[..4].copy_from_slice(&a.to_le_bytes());
    d[4..].copy_from_slice(&b.to_le_bytes());
    set_iovar(name, &d)
}

fn get_iovar(name: &str, len: usize) -> Result<Vec<u8>, i32> {
    let mut p: Vec<u8> = Vec::with_capacity(name.len() + 1 + len);
    p.extend_from_slice(name.as_bytes());
    p.push(0);
    p.resize(p.len().max(len), 0);
    ioctl(false, WLC_GET_VAR, &p)
}

fn step(name: &[u8], r: Result<(), i32>) -> Option<()> {
    match r {
        Ok(()) => Some(()),
        Err(e) => {
            out(b"  wifi: ");
            out(name);
            out(b" failed, code ");
            dec(b"", (e as i64).unsigned_abs());
            out(b"\n");
            None
        },
    }
}

/// `step` for the join path: the failing iovar / ioctl and the firmware's
/// code end up in the message the picker shows.
fn stepc(name: &str, r: Result<(), i32>) -> Result<(), String> {
    match r {
        Ok(()) => Ok(()),
        Err(e) => {
            out(b"  wifi: ");
            out(name.as_bytes());
            out(b" failed, code ");
            dec(b"", (e as i64).unsigned_abs());
            out(b"\n");
            Err(alloc::format!("{name} refused by the firmware (code {e})"))
        },
    }
}

/// Signal strength of the joined network in dBm (WLC_GET_RSSI).
pub fn rssi() -> Option<i32> {
    if !is_joined() {
        return None;
    }
    let v = ioctl(false, 127, &[0u8; 4]).ok()?;
    if v.len() < 4 {
        return None;
    }
    Some(le32(&v, 0) as i32)
}

fn load_clm() -> Option<()> {
    const CHUNK: usize = 1024;
    let mut off = 0usize;
    while off < CLM.len() {
        let n = (CLM.len() - off).min(CHUNK);
        let mut flag: u16 = 1 << 12; // handler version
        if off == 0 {
            flag |= 0x2; // begin
        }
        if off + n >= CLM.len() {
            flag |= 0x4; // end
        }
        let mut d: Vec<u8> = Vec::with_capacity(12 + n);
        d.extend_from_slice(&flag.to_le_bytes());
        d.extend_from_slice(&2u16.to_le_bytes()); // CLM download type
        d.extend_from_slice(&(n as u32).to_le_bytes());
        d.extend_from_slice(&0u32.to_le_bytes());
        d.extend_from_slice(&CLM[off..off + n]);
        step(b"clmload", set_iovar("clmload", &d))?;
        off += n;
    }
    match get_iovar("clmload_status", 4) {
        Ok(v) if v.len() >= 4 && le32(&v, 0) == 0 => {
            out(b"  CLM blob loaded\n");
        },
        Ok(v) => {
            hex(b"  wifi: clmload_status=", if v.len() >= 4 { le32(&v, 0) as u64 } else { 0 }, 8);
        },
        Err(e) => {
            dec(b"  wifi: clmload_status unreadable (", (e as i64).unsigned_abs());
            out(b")\n");
        },
    }
    Some(())
}

fn radio_up() -> Option<()> {
    if radio().is_some() {
        return Some(());
    }
    let cores = erom_scan()?;
    load_firmware(&cores)?;
    start_f2(&cores)?;
    unsafe {
        *(&raw mut RADIO) = Some(Radio {
            cores,
            tx_seq: 0,
            tx_max: 1,
            ioctl_id: 0,
            mac: [0; 6],
            joined: false,
            link: false,
            ssid: String::new(),
            scan_done: None,
            scan_hits: Vec::new(),
            join_status: None,
            psk_status: None,
            sup: None,
            sup_error: None,
            pending_eapol: None,
            ies_known: true,
            rx_queue: Vec::new(),
            frames_in: 0,
            frames_out: 0,
            last_tx: String::new(),
            last_rx: String::new(),
            rx_data: 0,
        });
    }
    // Give the firmware a moment and take its first mailbox / credit frame.
    let t0 = now_ms();
    while now_ms() - t0 < 300 {
        poll_rx();
        delay_us(10_000);
    }
    // The firmware's version line: proof the control channel works.
    match get_iovar("ver", 128) {
        Ok(v) => {
            let end = v.iter().position(|&b| b == 0).unwrap_or(v.len());
            out(b"  firmware: ");
            out(String::from_utf8_lossy(&v[..end]).trim().as_bytes());
            out(b"\n");
        },
        Err(e) => {
            dec(b"  wifi: the firmware did not answer the first ioctl (", (e as i64).unsigned_abs());
            out(b")\n");
            unsafe { *(&raw mut RADIO) = None };
            unsafe { LAST_ERROR = "firmware control channel did not answer" };
            return None;
        },
    }
    load_clm()?;
    if let Ok(m) = get_iovar("cur_etheraddr", 6) {
        if m.len() >= 6 {
            let r = radio()?;
            r.mac.copy_from_slice(&m[..6]);
            out(b"  MAC ");
            for (i, b) in r.mac.iter().enumerate() {
                let h = |x: u8| if x < 10 { b'0' + x } else { b'a' + x - 10 };
                out(&[h(b >> 4), h(b & 0xF)]);
                if i < 5 {
                    out(b":");
                }
            }
            out(b"\n");
        }
    }
    // Country, events, station mode, up.
    let mut country = [0u8; 12];
    country[..2].copy_from_slice(b"XX");
    country[4..8].copy_from_slice(&(-1i32).to_le_bytes());
    country[8..10].copy_from_slice(b"XX");
    let _ = set_iovar("country", &country);
    let mut mask = [0u8; 16];
    for ev in [EV_SET_SSID, EV_AUTH, EV_DEAUTH, EV_DEAUTH_IND, EV_DISASSOC, EV_DISASSOC_IND, EV_LINK, EV_PSK_SUP, EV_ESCAN_RESULT] {
        mask[(ev / 8) as usize] |= 1 << (ev % 8);
    }
    let _ = set_iovar("event_msgs", &mask);
    let _ = set_iovar_u32("bus:txglom", 0);
    let _ = set_iovar_u32("apsta", 1);
    let _ = set_iovar_u32("ampdu_ba_wsize", 8);
    // 802.11 b/g/n on 2.4 GHz and a/n/ac on 5 GHz: HT and VHT on, the band
    // on auto, g-mode auto (these take effect before `up`).
    let _ = set_iovar_u32("nmode", 1);
    let _ = set_iovar_u32("vhtmode", 1);
    let _ = ioctl_set_u32(WLC_SET_GMODE, 1);
    let _ = ioctl_set_u32(WLC_SET_BAND, 0);
    step(b"up", ioctl(true, WLC_UP, &[]).map(|_| ()))?;
    let _ = ioctl_set_u32(WLC_SET_PM, 0);
    let flag = |name: &str| get_iovar(name, 4).ok().filter(|v| v.len() >= 4).map(|v| le32(&v, 0)).unwrap_or(0);
    out(b"  modes: 802.11b/g");
    if flag("nmode") != 0 {
        out(b"/n");
    }
    if flag("vhtmode") != 0 {
        out(b" + a/n/ac on 5 GHz");
    }
    out(b"\n  radio is up\n");
    Some(())
}

/// `wifi country <CC>`: the regulatory domain (channels 12/13 and the 5 GHz
/// set depend on it; "XX" is the world-safe default).
pub fn set_country(cc: &str) -> Result<(), String> {
    if !ensure_init() {
        return Err(String::from(last_error()));
    }
    let code = cc.trim().to_ascii_uppercase();
    if code.len() != 2 {
        return Err(String::from("a country code is two letters (JP, US, CN, GB ...)"));
    }
    let mut country = [0u8; 12];
    country[..2].copy_from_slice(code.as_bytes());
    country[4..8].copy_from_slice(&(-1i32).to_le_bytes());
    country[8..10].copy_from_slice(code.as_bytes());
    stepc("country", set_iovar("country", &country))
}

// ── public API ───────────────────────────────────────────────────────────────

/// Bus + firmware, once. False (with `last_error`) when the radio is unusable.
pub fn ensure_init() -> bool {
    if radio().is_some() {
        return true;
    }
    if unsafe { BUS_TRIED } && !last_error().is_empty() {
        return false;
    }
    unsafe { BUS_TRIED = true };
    if bus_up().is_none() {
        if last_error().is_empty() {
            unsafe { LAST_ERROR = "the radio did not answer on SDIO2" };
        }
        return false;
    }
    if radio_up().is_none() {
        if last_error().is_empty() {
            unsafe { LAST_ERROR = "firmware start failed" };
        }
        return false;
    }
    true
}

pub fn is_joined() -> bool {
    radio().is_some_and(|r| r.joined)
}

pub fn mac() -> [u8; 6] {
    radio().map(|r| r.mac).unwrap_or([0; 6])
}

pub fn ssid() -> String {
    radio().map(|r| r.ssid.clone()).unwrap_or_default()
}

/// Scan for networks (a few seconds); strongest first.
pub fn scan() -> Result<Vec<Network>, &'static str> {
    if !ensure_init() {
        return Err(last_error());
    }
    let r = radio().ok_or("radio not up")?;
    r.scan_hits.clear();
    r.scan_done = None;
    // escan params: version 1, action start, sync id, then the scan params.
    let mut p: Vec<u8> = Vec::with_capacity(80);
    p.extend_from_slice(&1u32.to_le_bytes());
    p.extend_from_slice(&1u16.to_le_bytes());
    p.extend_from_slice(&0x1234u16.to_le_bytes());
    p.extend_from_slice(&0u32.to_le_bytes()); // ssid len
    p.extend_from_slice(&[0u8; 32]); // ssid
    p.extend_from_slice(&[0xFF; 6]); // bssid: any
    p.push(2); // bss type: any
    p.push(0); // active scan
    p.extend_from_slice(&(-1i32).to_le_bytes()); // nprobes
    p.extend_from_slice(&(-1i32).to_le_bytes()); // active time
    p.extend_from_slice(&(-1i32).to_le_bytes()); // passive time
    p.extend_from_slice(&(-1i32).to_le_bytes()); // home time
    p.extend_from_slice(&0u32.to_le_bytes()); // channel count
    p.extend_from_slice(&0u16.to_le_bytes()); // channel list
    if let Err(e) = set_iovar("escan", &p) {
        dec(b"  wifi: escan refused (", (e as i64).unsigned_abs());
        out(b")\n");
        return Err("the firmware refused to scan");
    }
    let t0 = now_ms();
    loop {
        poll_rx();
        if let Some(st) = radio().and_then(|r| r.scan_done) {
            if st != 0 {
                dec(b"  wifi: scan ended with status ", st as u64);
                out(b"\n");
            }
            break;
        }
        if now_ms() - t0 > 8_000 {
            out(b"  wifi: scan timed out\n");
            break;
        }
        delay_us(5_000);
    }
    let r = radio().ok_or("radio not up")?;
    let mut hits = r.scan_hits.clone();
    hits.sort_by(|a, b| b.rssi.cmp(&a.rssi));
    Ok(hits)
}

/// Join `ssid` (WPA2-PSK with a passphrase, open without); blocks until the
/// firmware reports the association or gives up.
pub fn join(ssid: &str, psk: &str) -> Result<(), String> {
    if !ensure_init() {
        return Err(String::from(last_error()));
    }
    if ssid.is_empty() || ssid.len() > 32 {
        return Err(String::from("network name must be 1-32 bytes"));
    }
    if !psk.is_empty() && (psk.len() < 8 || psk.len() > 63) {
        return Err(String::from("a WPA2 passphrase is 8-63 characters"));
    }
    let r = radio().ok_or_else(|| String::from("radio not up"))?;
    if r.joined {
        let _ = ioctl(true, WLC_DISASSOC, &[]);
        r.joined = false;
    }
    r.join_status = None;
    r.psk_status = None;
    r.link = false;
    let wpa = !psk.is_empty();
    // What the last scan saw for this network: 4 = WPA3-only (SAE).
    let sae = r.scan_hits.iter().any(|n| n.ssid == ssid && n.secure == 4);
    if wpa && sae {
        out(b"  wifi: WPA3-only network, joining with SAE\n");
        stepc("wsec", ioctl_set_u32(WLC_SET_WSEC, 4))?;
        if set_iovar_u32("sup_wpa", 1).is_err() {
            let _ = set_iovar_u32x2("bsscfg:sup_wpa", 0, 1);
        }
        let mut pw = [0u8; 130];
        pw[..2].copy_from_slice(&(psk.len() as u16).to_le_bytes());
        pw[2..2 + psk.len()].copy_from_slice(psk.as_bytes());
        stepc("sae_password", set_iovar("sae_password", &pw)).map_err(|e| alloc::format!("{e}; this firmware may lack WPA3 -- enable WPA2 on the router"))?;
        stepc("infra", ioctl_set_u32(WLC_SET_INFRA, 1))?;
        stepc("auth", ioctl_set_u32(WLC_SET_AUTH, 3))?;
        stepc("wpa_auth", set_iovar_u32("wpa_auth", 0x40000))?;
    } else if wpa {
        stepc("wsec", ioctl_set_u32(WLC_SET_WSEC, 4))?;
        // This firmware has no internal supplicant (`sup_wpa` answers
        // BCME_UNSUPPORTED, -23), so the kernel does the 4-way handshake
        // itself (super::wpa) over the EAPOL frames the firmware passes up,
        // and installs the keys with `wsec_key` -- the way Linux works too.
        let pmk = super::wpa::pmk_from_passphrase(psk.as_bytes(), ssid.as_bytes());
        let mut snonce = [0u8; 32];
        if !crate::crypto::random_bytes(&mut snonce) {
            let t = crate::arch::timer::now_us();
            for (i, b) in snonce.iter_mut().enumerate() {
                *b = (t.wrapping_mul(0x9E37_79B9_7F4A_7C15).rotate_left(i as u32 * 5) >> (i % 8 * 8)) as u8;
            }
        }
        r.sup = Some(super::wpa::Handshake::new(pmk, r.mac, snonce));
        r.sup_error = None;
        // The firmware composes the association IEs itself (its RSN IE matches
        // what the access point offers -- group cipher included); the exact IE
        // it sent is read back after associating, for handshake message 2.
        r.pending_eapol = None;
        r.ies_known = false;
    } else {
        stepc("wsec", ioctl_set_u32(WLC_SET_WSEC, 0))?;
    }
    if !(wpa && sae) {
        stepc("infra", ioctl_set_u32(WLC_SET_INFRA, 1))?;
        stepc("auth", ioctl_set_u32(WLC_SET_AUTH, 0))?;
        stepc("wpa_auth", set_iovar_u32("wpa_auth", if wpa { 0x80 } else { 0 }))?;
    }
    let mut s = [0u8; 36];
    s[..4].copy_from_slice(&(ssid.len() as u32).to_le_bytes());
    s[4..4 + ssid.len()].copy_from_slice(ssid.as_bytes());
    stepc("set_ssid", ioctl(true, WLC_SET_SSID, &s).map(|_| ()))?;
    let t0 = now_ms();
    let mut assoc = false;
    loop {
        poll_rx();
        let r = radio().ok_or_else(|| String::from("radio not up"))?;
        if let Some(st) = r.join_status {
            if st != 0 {
                dec(b"  wifi: join failed, status ", st as u64);
                out(b"\n");
                return Err(match st {
                    3 => String::from("network not found"),
                    1 => String::from("the access point refused the association (status 1): check the security type; a 5 GHz network needs `wifi country <CC>`"),
                    _ => alloc::format!("the access point refused the join (status {st})"),
                });
            }
            if !assoc && wpa {
                assoc_ies_learned();
            }
            assoc = true;
        }
        if assoc && (!wpa || r.psk_status.is_some() || now_ms() - t0 > 12_000) {
            break;
        }
        if now_ms() - t0 > 20_000 {
            return Err(String::from("no answer from the access point (timeout)"));
        }
        delay_us(5_000);
    }
    let r = radio().ok_or_else(|| String::from("radio not up"))?;
    if wpa {
        match (r.psk_status, r.sup_error) {
            (Some(6), _) => {},
            (_, Some(e)) => {
                let _ = ioctl(true, WLC_DISASSOC, &[]);
                return Err(alloc::format!("WPA2 handshake: {e}"));
            },
            (Some(st), None) => {
                dec(b"  wifi: WPA2 handshake status ", st as u64);
                out(b"\n");
                return Err(alloc::format!("wrong passphrase (the WPA2 handshake failed, status {st})"));
            },
            (None, None) => {
                let _ = ioctl(true, WLC_DISASSOC, &[]);
                return Err(String::from("the WPA2 handshake did not complete (wrong passphrase?)"));
            },
        }
    }
    // No firmware power save: a polling host with no interrupt line would
    // otherwise see every frame the access point buffers arrive late. No ARP
    // or TCP offload either: the host's stack does all of it and must see
    // every frame as it crossed the air.
    let _ = ioctl_set_u32(WLC_SET_PM, 0);
    let _ = set_iovar_u32("arpoe", 0);
    let _ = set_iovar_u32("arp_ol", 0);
    let _ = set_iovar_u32("toe_ol", 0);
    r.joined = true;
    r.ssid = String::from(ssid);
    out(b"wifi: joined ");
    out(ssid.as_bytes());
    out(b"\n");
    Ok(())
}

pub fn leave() {
    if let Some(r) = radio() {
        if r.joined {
            let _ = ioctl(true, WLC_DISASSOC, &[]);
        }
        r.joined = false;
        r.link = false;
        r.sup = None;
        r.ssid.clear();
    }
}

/// One Ethernet frame to the firmware's data channel (BDC header first).
fn send_eth(frame: &[u8]) -> Option<()> {
    let mut p: Vec<u8> = Vec::with_capacity(4 + frame.len());
    p.extend_from_slice(&[0x20, 0, 0, 0]); // BDC: protocol version 2, priority 0, if 0
    p.extend_from_slice(frame);
    send_frame(CHAN_DATA, &p)
}

/// One Ethernet frame to the air (after `join`).
pub fn transmit(frame: &[u8]) -> bool {
    if !is_joined() || frame.len() < 14 || frame.len() > 1600 {
        return false;
    }
    let ok = send_eth(frame).is_some();
    if let Some(r) = radio() {
        r.last_tx = frame_summary(frame, ok);
    }
    ok
}

/// "UDP 192.168.0.38:33012 > 1.1.1.1:53 (78 B)" and the like.
fn frame_summary(f: &[u8], ok: bool) -> String {
    if f.len() < 14 {
        return String::from("short frame");
    }
    let et = u16::from_be_bytes([f[12], f[13]]);
    let ip = |o: usize| alloc::format!("{}.{}.{}.{}", f[o], f[o + 1], f[o + 2], f[o + 3]);
    let s = if et == 0x0806 && f.len() >= 42 {
        alloc::format!("ARP {} {} > {}", if f[21] == 1 { "who-has" } else { "reply" }, ip(28), ip(38))
    } else if et == 0x0800 && f.len() >= 34 {
        let ihl = ((f[14] & 0x0F) as usize) * 4;
        let t = 14 + ihl;
        let ports = if f.len() >= t + 4 { alloc::format!(":{} > {}:{}", u16::from_be_bytes([f[t], f[t + 1]]), ip(30), u16::from_be_bytes([f[t + 2], f[t + 3]])) } else { alloc::format!(" > {}", ip(30)) };
        let proto = match f[23] {
            1 => "ICMP",
            6 => "TCP",
            17 => "UDP",
            _ => "IP",
        };
        alloc::format!("{proto} {}{ports}", ip(26))
    } else if et == 0x888E {
        String::from("EAPOL")
    } else {
        alloc::format!("ethertype {et:04x}")
    };
    alloc::format!("{s} ({} B){}", f.len(), if ok { "" } else { " NOT SENT" })
}

/// What the last frames looked like, for the connection test.
pub fn last_frames() -> String {
    match radio() {
        Some(r) => alloc::format!("wifi: {} frames in ({} data), {} out; last out: {}; last in: {}", r.frames_in, r.rx_data, r.frames_out, if r.last_tx.is_empty() { "-" } else { &r.last_tx }, if r.last_rx.is_empty() { "-" } else { &r.last_rx }),
        None => String::from("wifi: radio not up"),
    }
}

/// One received Ethernet frame, if any arrived.
pub fn receive(outbuf: &mut [u8]) -> Option<usize> {
    if radio()?.rx_queue.is_empty() {
        poll_rx();
    }
    let r = radio()?;
    if r.rx_queue.is_empty() {
        return None;
    }
    let f = r.rx_queue.remove(0);
    let n = f.len().min(outbuf.len());
    outbuf[..n].copy_from_slice(&f[..n]);
    r.rx_data += 1;
    r.last_rx = frame_summary(&f, true);
    Some(n)
}

/// A line for the console / tray.
pub fn status() -> String {
    match radio() {
        None => {
            if last_error().is_empty() {
                String::from("radio not started (wifi scan starts it)")
            } else {
                alloc::format!("radio unavailable: {}", last_error())
            }
        },
        Some(r) if r.joined => alloc::format!("joined {} ({} frames in, {} out)", r.ssid, r.frames_in, r.frames_out),
        Some(_) => String::from("radio up, not joined"),
    }
}

/// Console `wifi`: bring everything up and report.
pub fn probe() -> Option<u32> {
    if ensure_init() {
        out(b"wifi: ");
        out(status().as_bytes());
        out(b"\n");
        Some(0x4345)
    } else {
        out(b"wifi: ");
        out(last_error().as_bytes());
        out(b"\n");
        None
    }
}
