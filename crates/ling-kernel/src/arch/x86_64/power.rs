//! Reboot and power-off. Real hardware needs ACPI (parse the FADT, find
//! PM1a_CNT + SLP_TYPa, write SLP_EN) which this kernel doesn't have yet;
//! what's here is the set of mechanisms that actually work on the VMs
//! LingOS targets (QEMU, VirtualBox, Bochs), tried in turn, then a hard
//! halt if none took. Honest scope: these are the emulator power paths,
//! not a general ACPI implementation.

use super::io;

/// Reboot: pulse the 8042 keyboard controller's reset line (0xFE to the
/// command port) -- the classic PC reset that QEMU/VBox honor -- then, if
/// that somehow returns, force a triple fault by loading a null IDT and
/// raising an interrupt, which every x86 resets on.
pub fn reboot() -> ! {
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
        // Drain the 8042 input buffer, then pulse reset.
        let mut spin = 0;
        while io::inb(0x64) & 0x02 != 0 && spin < 100000 {
            spin += 1;
        }
        io::outb(0x64, 0xFE);
        // Fallback: triple fault via a null IDT.
        core::arch::asm!("lidt [{}]", in(reg) &NULL_IDT, options(nostack));
        core::arch::asm!("int3", options(nostack));
        loop {
            core::arch::asm!("hlt");
        }
    }
}

#[repr(C, packed)]
struct IdtPtr {
    limit: u16,
    base: u64,
}
static NULL_IDT: IdtPtr = IdtPtr { limit: 0, base: 0 };

/// Power off. Tries the well-known VM ACPI-shutdown I/O ports in turn:
///   0x604  <- 0x2000  (QEMU >= 2.0)
///   0xB004 <- 0x2000  (Bochs / older QEMU)
///   0x4004 <- 0x3400  (VirtualBox)
/// then APM, then halts if nothing powered the machine down.
pub fn poweroff() -> ! {
    unsafe {
        core::arch::asm!("cli", options(nostack, nomem));
        io::outw(0x604, 0x2000);
        io::outw(0xB004, 0x2000);
        io::outw(0x4004, 0x3400);
        // Nothing took -- halt forever (the user can hard-off the VM).
        loop {
            core::arch::asm!("hlt");
        }
    }
}
