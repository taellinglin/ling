//! Legacy 8259 PIC — remapped off the CPU exception range, individually
//! maskable, EOI'd per IRQ. This kernel never brings up a second core (see
//! `sched.rs`'s module doc for the same single-core assumption from the
//! scheduler side), so an IOAPIC/LAPIC's only real benefit — routing
//! interrupts across multiple cores — doesn't apply here; the 8259 pair is
//! the whole interrupt controller, not a fallback for a missing APIC.
use super::io;

const PIC1_COMMAND: u16 = 0x20;
const PIC1_DATA: u16 = 0x21;
const PIC2_COMMAND: u16 = 0xA0;
const PIC2_DATA: u16 = 0xA1;

/// Where remapped IRQ0..7 (master) land in the IDT — chosen past the 32 CPU
/// exception vectors, same convention every x86 OS bring-up uses (the 8259's
/// power-on default of vectors 8..15 collides with Double Fault/GP/etc.,
/// which is precisely the "spurious NMI-looking garbage on IRQ0" bug this
/// remap exists to avoid).
pub const IRQ0_VECTOR: u8 = 32;
const PIC2_OFFSET: u8 = IRQ0_VECTOR + 8;

const ICW1_INIT: u8 = 0x10;
const ICW1_ICW4: u8 = 0x01;
const ICW4_8086: u8 = 0x01;

/// Remap both PICs and mask every line. Callers unmask individual IRQs
/// (`unmask`) only once their handler and any device-side init (e.g.
/// `mouse::init`'s synchronous 8042 handshake) are ready — an early IRQ with
/// no handler installed would otherwise fire into whatever vector 32+n
/// currently holds.
pub fn init() {
    unsafe {
        let mask1 = io::inb(PIC1_DATA);
        let mask2 = io::inb(PIC2_DATA);

        io::outb(PIC1_COMMAND, ICW1_INIT | ICW1_ICW4);
        io::outb(PIC2_COMMAND, ICW1_INIT | ICW1_ICW4);
        io::outb(PIC1_DATA, IRQ0_VECTOR);
        io::outb(PIC2_DATA, PIC2_OFFSET);
        io::outb(PIC1_DATA, 0x04); // slave attached on master's IRQ2
        io::outb(PIC2_DATA, 0x02); // slave's cascade identity
        io::outb(PIC1_DATA, ICW4_8086);
        io::outb(PIC2_DATA, ICW4_8086);

        // Preserve whatever was masked before remap, then force IRQ2 (the
        // cascade line) unmasked on the master — required for any slave
        // (IRQ8..15, e.g. IRQ12 mouse) interrupt to ever reach the CPU at
        // all, regardless of which individual lines callers unmask later.
        io::outb(PIC1_DATA, mask1 & !0x04);
        io::outb(PIC2_DATA, mask2);
    }
}

pub fn mask(irq: u8) {
    unsafe {
        if irq < 8 {
            let port = PIC1_DATA;
            io::outb(port, io::inb(port) | (1 << irq));
        } else {
            let port = PIC2_DATA;
            io::outb(port, io::inb(port) | (1 << (irq - 8)));
        }
    }
}

pub fn unmask(irq: u8) {
    unsafe {
        if irq < 8 {
            let port = PIC1_DATA;
            io::outb(port, io::inb(port) & !(1 << irq));
        } else {
            let port = PIC2_DATA;
            io::outb(port, io::inb(port) & !(1 << (irq - 8)));
        }
    }
}

/// Send end-of-interrupt for `irq` (0..15). Must be called at the end of
/// every IRQ handler, slave line or not — a slave IRQ needs EOI sent to
/// *both* controllers, since the cascade makes the master think IRQ2 (not
/// the real source) is what fired.
pub fn eoi(irq: u8) {
    unsafe {
        if irq >= 8 {
            io::outb(PIC2_COMMAND, 0x20);
        }
        io::outb(PIC1_COMMAND, 0x20);
    }
}
