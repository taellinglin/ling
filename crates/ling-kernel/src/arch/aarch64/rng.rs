//! Hardware random numbers (TLS client randoms and key shares need real
//! entropy): the "rng200" block of the Pi 4 / Pi 5 (BCM2711 / BCM2712), or
//! the BCM2835 RNG of the Pi 3 -- which QEMU's raspi3b emulates too.

use crate::arch::mmio::{board, read32, write32, RpiBoard};
use crate::arch::timer::delay_us;

static mut STARTED: bool = false;
/// The block answered a data abort when first touched: no RNG here.
static mut ABSENT: bool = false;

/// First touch: is the block on the bus at all?
fn present(base: usize) -> bool {
    unsafe {
        if ABSENT {
            return false;
        }
        if STARTED {
            return true;
        }
        if crate::arch::vectors::probe_read32(base).is_none() {
            ABSENT = true;
            crate::console_write(b"rng: no hardware random-number block on this board/emulator\n");
            return false;
        }
        true
    }
}

/// One random 32-bit word, or None when this board has no usable RNG.
pub fn random_u32() -> Option<u32> {
    match board() {
        RpiBoard::Pi5 => rng200(0x10_7d20_8000),
        RpiBoard::Pi4 => rng200(0xFE10_4000),
        RpiBoard::Pi3 => bcm2835(0x3F10_4000),
        RpiBoard::Unknown => None,
    }
}

pub fn random_u64() -> Option<u64> {
    Some(((random_u32()? as u64) << 32) | random_u32()? as u64)
}

/// iproc-rng200: CTRL 0x00 (RBGEN in bits 12:0), RNG_SOFT_RESET 0x04,
/// RBG_SOFT_RESET 0x08, FIFO_DATA 0x20, FIFO_COUNT 0x24 (words in bits 7:0).
fn rng200(base: usize) -> Option<u32> {
    if !present(base) {
        return None;
    }
    unsafe {
        if !STARTED {
            write32(base + 0x00, 0);
            write32(base + 0x04, 1);
            write32(base + 0x04, 0);
            write32(base + 0x08, 1);
            write32(base + 0x08, 0);
            write32(base + 0x00, 1);
            STARTED = true;
            delay_us(200);
        }
        for _ in 0..2000 {
            if read32(base + 0x24) & 0xFF > 0 {
                return Some(read32(base + 0x20));
            }
            delay_us(10);
        }
    }
    None
}

/// BCM2835 RNG: CTRL 0x0 (bit 0 enable), STATUS 0x4 (warm-up count on write,
/// words available in bits 31:24 on read), DATA 0x8.
fn bcm2835(base: usize) -> Option<u32> {
    if !present(base) {
        return None;
    }
    unsafe {
        if !STARTED {
            write32(base + 0x4, 0x40000);
            write32(base + 0x0, 1);
            STARTED = true;
        }
        for _ in 0..4000 {
            if read32(base + 0x4) >> 24 > 0 {
                return Some(read32(base + 0x8));
            }
            delay_us(10);
        }
    }
    None
}
