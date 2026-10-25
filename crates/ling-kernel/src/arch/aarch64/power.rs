//! Reset and halt through the Raspberry Pi's power-management block: the
//! watchdog reset every Pi generation shares (`PM_WDOG` arms a 10-tick
//! timeout, `PM_RSTC` requests a full reset), at `peripheral_base + 0x10_0000`
//! on the Pi 3 / 4 and 0x10_7D20_0000 on the Pi 5. "Power off" is the same
//! reset with `PM_RSTS` asking for boot partition 63, which the firmware
//! treats as halt (how Linux powers these boards down).
use crate::arch::mmio::{board, peripheral_base, read32, write32, RpiBoard};

const PM_PASSWORD: u32 = 0x5A00_0000;
const PM_RSTC: usize = 0x1C;
const PM_RSTS: usize = 0x20;
const PM_WDOG: usize = 0x24;
const PM_RSTC_WRCFG_FULL_RESET: u32 = 0x20;
const PM_RSTC_WRCFG_CLR: u32 = 0xFFFF_FFCF;

fn pm_base() -> usize {
    match board() {
        RpiBoard::Pi5 => 0x10_7D20_0000,
        _ => peripheral_base() + 0x10_0000,
    }
}

fn reset_now() -> ! {
    let pm = pm_base();
    unsafe {
        crate::arch::cpu::cli();
        write32(pm + PM_WDOG, PM_PASSWORD | 10);
        let rstc = read32(pm + PM_RSTC) & PM_RSTC_WRCFG_CLR;
        write32(pm + PM_RSTC, PM_PASSWORD | rstc | PM_RSTC_WRCFG_FULL_RESET);
    }
    loop {
        unsafe { core::arch::asm!("wfe", options(nomem, nostack)) };
    }
}

/// Reboot the board.
pub fn reboot() -> ! {
    crate::console_write(b"power: rebooting\n");
    reset_now()
}

/// Halt: ask the firmware for "partition 63" on the next reset, which it
/// honours by not booting (the board stays powered but idle until unplugged).
pub fn poweroff() -> ! {
    crate::console_write(b"power: halting\n");
    let pm = pm_base();
    unsafe {
        let rsts = read32(pm + PM_RSTS) & !0xFFFF_FAAA;
        // partition bits 0,2,4,6,8,10 all set = 63
        write32(pm + PM_RSTS, PM_PASSWORD | rsts | 0x555);
    }
    reset_now()
}
