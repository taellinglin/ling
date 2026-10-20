//! Raspberry Pi 5 Wi-Fi, stage 1: reach the CYW43455 radio and read its chip ID.
//!
//! The radio sits on the BCM2712's second SDHCI controller (`sdio2`, at
//! `0x10_0110_0000`, cfg block at +0x400), 4-bit SDIO on GPIO 30-35, powered
//! by WL_ON = GPIO 28 (all from the Raspberry Pi bcm2712 device trees). Linux
//! leaves `sdio2` to its own driver, and the firmware doesn't bring it up, so
//! this does everything by hand:
//!
//! 1. pin-mux GPIO 30-35 to SD2 (the mux table differs between the BCM2712
//!    C0/C1 stepping on rev 1.0 boards and D0 on rev 1.1+), GPIO 28 to GPIO;
//! 2. power-cycle the radio through WL_ON;
//! 3. reset the SDHCI host, force card-present, 3.3 V, 400 kHz identification clock;
//! 4. SDIO card init: CMD0, CMD5 (until the card's OCR says ready), CMD3, CMD7;
//! 5. enable function 1 (the chip's backplane), request its ALP clock, point
//!    the backplane window at the ChipCommon core (0x1800_0000) and read the
//!    chip-ID register — a CYW43455 answers 0x4345.
//!
//! Every step prints (the HDMI console is the only debug channel on a Pi 5
//! without a serial adapter). Stage 2 is firmware download (the brcmfmac43455
//! firmware + NVRAM + CLM blobs), then the firmware's control channel, scan,
//! join with WPA2 (done in-firmware), and an aarch64 network stack.

use crate::arch::mmio::{read32, write32};
use crate::arch::timer::delay_us;
use crate::console_write as out;

const SDIO2: usize = 0x10_0110_0000;
const SDIO2_CFG: usize = 0x10_0110_0400;
const GIO: usize = 0x10_7d50_8500; // brcmstb GPIO, bank 0 = GPIO 0..31
const PINCTRL: usize = 0x10_7d50_4100;

/// Base clock feeding the SDHCI (the device tree's fixed `clk_emmc2`).
const BASE_CLOCK_HZ: u32 = 200_000_000;

// SDHCI registers.
const ARG: usize = 0x08;
const XFER_CMD: usize = 0x0C; // transfer mode (lo 16) + command (hi 16)
const RESP0: usize = 0x10;
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
            // Reset the CMD line so the next command can go out.
            sd_wr(CLOCK_CTRL, sd_rd(CLOCK_CTRL) | (0x2 << 24));
            wait(CLOCK_CTRL, 0x2 << 24, 0, 100);
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

// Function-1 (backplane) registers of Broadcom SDIO chips.
const SB_ADDR_LOW: u32 = 0x1000A;
const SB_ADDR_MID: u32 = 0x1000B;
const SB_ADDR_HIGH: u32 = 0x1000C;
const CHIP_CLK_CSR: u32 = 0x1000E;
const ALP_AVAIL_REQ: u8 = 0x08;
const FORCE_HW_CLKREQ_OFF: u8 = 0x20;
const ALP_AVAIL: u8 = 0x40;
const CHIPCOMMON: u32 = 0x1800_0000;

/// Read a 32-bit backplane register with four CMD52 byte reads through the
/// function-1 window.
fn backplane_read32(addr: u32) -> Option<u32> {
    let win = addr & !0x7FFF;
    cmd52(true, 1, SB_ADDR_LOW, ((win >> 8) & 0x80) as u8)?;
    cmd52(true, 1, SB_ADDR_MID, ((win >> 16) & 0xFF) as u8)?;
    cmd52(true, 1, SB_ADDR_HIGH, ((win >> 24) & 0xFF) as u8)?;
    let mut v = 0u32;
    for i in 0..4 {
        let b = cmd52(false, 1, (addr & 0x7FFF) + i, 0)?;
        v |= (b as u32) << (8 * i);
    }
    Some(v)
}

/// Stage-1 probe, printing as it goes. Returns the chip ID if the radio
/// answered.
pub fn probe() -> Option<u32> {
    if crate::arch::mmio::board() != crate::arch::mmio::RpiBoard::Pi5 {
        out(b"wifi: this probe is for the Raspberry Pi 5 (CYW43455 on BCM2712 SDIO2)\n");
        return None;
    }
    out(b"Wi-Fi probe (Pi 5: CYW43455 on SDIO2)\n");

    // 1. Pin mux for the stepping this board carries.
    let rev = crate::arch::mailbox::board_revision().unwrap_or(0);
    hex(b"  board revision=", rev as u64, 6);
    let d0 = rev & 0xF >= 1;
    out(if d0 { b"  BCM2712 D0 pin table (rev 1.1+)\n" } else { b"  BCM2712 C0/C1 pin table (rev 1.0)\n" });
    let sd2: [u32; 6] = if d0 { [1, 1, 1, 1, 1, 1] } else { [4, 4, 4, 3, 4, 3] };
    for (i, pin) in (30..=35u32).enumerate() {
        // CLK no pull; CMD and DAT0-3 pulled up as SDIO requires.
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
        out(b"  SDHCI reset timeout\n");
        return None;
    }
    // 3.3 V bus power (power control at 0x29), 1-bit bus for identification.
    sd_wr(HOST_CTRL, 0x0F << 8);
    delay_us(10_000);
    // 400 kHz: divided-clock mode, f = base / (2N).
    let n = BASE_CLOCK_HZ.div_ceil(2 * 400_000);
    let clk = ((n & 0xFF) << 8) | (((n >> 8) & 0x3) << 6) | 0x1;
    sd_wr(CLOCK_CTRL, (0x0E << 16) | clk); // + data timeout 2^27
    if !wait(CLOCK_CTRL, 0x2, 0x2, 100) {
        out(b"  internal clock never stabilised\n");
        return None;
    }
    sd_wr(CLOCK_CTRL, sd_rd(CLOCK_CTRL) | 0x4); // SD clock on
    sd_wr(INT_ENABLE, 0xFFFF_FFFF);
    sd_wr(INT_SIGNAL, 0);
    delay_us(10_000);
    hex(b"  present state=", sd_rd(PRESENT) as u64, 8);

    // 4. SDIO card init.
    cmd(0, 0, RESP_NONE);
    delay_us(2_000);
    let Some(ocr) = cmd(5, 0, RESP_48) else {
        out(b"  no answer to CMD5: radio not reachable on SDIO2\n");
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
        out(b"  card never reported ready\n");
        return None;
    }
    out(b"  SDIO card ready, functions=");
    out(&[b'0' + ((ready >> 28) & 7) as u8, b'\n']);
    let rca = cmd(3, 0, RESP_48_CHK)? >> 16;
    hex(b"  RCA=", rca as u64, 4);
    cmd(7, rca << 16, RESP_48_BUSY)?;
    out(b"  card selected\n");

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
    let id = backplane_read32(CHIPCOMMON)?;
    hex(b"  ChipCommon chipid=", id as u64, 8);
    let chip = id & 0xFFFF;
    if chip == 0x4345 {
        out(b"  -> CYW43455 Wi-Fi radio FOUND (stage 1 OK)\n");
    } else {
        hex(b"  -> unexpected chip id ", chip as u64, 4);
    }
    Some(id)
}
