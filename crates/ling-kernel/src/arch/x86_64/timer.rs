//! A calibrated, boot-relative clock/delay source built on the TSC, with no
//! timer interrupt involved at all — this kernel has no IDT/PIC/APIC-timer
//! (see `sched.rs`'s module doc for the same constraint from the scheduler
//! side), so calibration itself has to be a one-time *polled* measurement
//! against a known-frequency reference (PIT channel 2, gated through the
//! legacy port-0x61 speaker interface — no interrupt wiring needed there
//! either, this is pure port-I/O polling, the same idiom every existing
//! hardware-wait loop in this kernel already uses).
//!
//! Every hardware-wait loop in `ata.rs`/`ahci.rs` is bounded through
//! [`poll_until`], built on this module's clock — confirmed necessary, not
//! just theoretical: booting with no disk attached at all left the status
//! port floating with BSY permanently set, hanging the shell before it ever
//! reached a prompt, until these were bounded.
//!
//! Assumes an invariant TSC (true for QEMU/VirtualBox's virtualized CPUs;
//! not guaranteed on all real hardware) and a single core (this kernel
//! never brings up a second one, so there's no cross-core sync concern).
use super::{cpu, io};

const PIT_CHANNEL0_DATA: u16 = 0x40;
const PIT_CHANNEL2_DATA: u16 = 0x42;
const PIT_COMMAND: u16 = 0x43;
const PIT_FREQUENCY_HZ: u64 = 1_193_182;
const SPEAKER_PORT: u16 = 0x61;

/// Calibration window: long enough that TSC-tick rounding error is
/// negligible, short enough not to make boot noticeably slower.
const CALIBRATION_MS: u64 = 10;

static mut TSC_PER_US: u64 = 0;
static mut TSC_AT_BOOT: u64 = 0;

/// Measure `TSC_PER_US` against PIT channel 2 and stamp the boot-time TSC
/// reading `now_us`/`now_ms`/`delay_us` are relative to. Call once, early
/// in `init()`, before anything that needs timing (the network stack's
/// clock source, hardware reset-sequence delays).
pub fn calibrate() {
    unsafe {
        crate::drivers::serial::write(b"timer: calibrate start\n");

        // Gate channel 2 off while reprogramming it (bit0 = gate, bit1 =
        // speaker data -- clear both so this stays silent).
        io::outb(SPEAKER_PORT, io::inb(SPEAKER_PORT) & 0xFC);

        // Channel 2, lobyte/hibyte access, mode 0 (interrupt-on-terminal-
        // count -- irrelevant here since IRQ0/2 is never enabled; polled
        // via the status bit below instead), binary mode.
        io::outb(PIT_COMMAND, 0b1011_0000);

        let count = (PIT_FREQUENCY_HZ * CALIBRATION_MS / 1000) as u16;
        io::outb(PIT_CHANNEL2_DATA, (count & 0xFF) as u8);
        io::outb(PIT_CHANNEL2_DATA, (count >> 8) as u8);

        // Set the gate bit to start the count.
        io::outb(SPEAKER_PORT, io::inb(SPEAKER_PORT) | 0x01);

        crate::drivers::serial::write(b"timer: gate set, polling\n");

        let t0 = cpu::rdtsc();
        // Bit 5 (OUT2 status) goes high once the count reaches terminal.
        // Bounded by a fixed spin count rather than trusting the
        // emulated/real hardware to ever actually set it — `poll_until`
        // (used by every wait loop elsewhere in this kernel) isn't
        // available yet here: it needs `TSC_PER_US`, which this
        // calibration run is what computes in the first place.
        let mut spins: u64 = 0;
        const MAX_SPINS: u64 = 100_000_000;
        let mut timed_out = false;
        while io::inb(SPEAKER_PORT) & 0x20 == 0 {
            cpu::pause();
            spins += 1;
            if spins > MAX_SPINS {
                timed_out = true;
                break;
            }
        }
        let t1 = cpu::rdtsc();

        if timed_out {
            crate::drivers::serial::write(b"timer: PIT poll TIMED OUT, using fallback estimate\n");
            // A plausible modern-CPU TSC frequency estimate so `now_us`/
            // `delay_us` are merely imprecise instead of unusable if the
            // PIT-based measurement can't complete in this environment.
            TSC_PER_US = 1000;
            TSC_AT_BOOT = t1;
            crate::drivers::serial::write(b"timer: TSC_PER_US=");
            print_decimal(TSC_PER_US);
            crate::drivers::serial::write(b" (fallback)\n");
            return;
        }
        crate::drivers::serial::write(b"timer: PIT poll done after ");
        print_decimal(spins);
        crate::drivers::serial::write(b" spins\n");

        TSC_PER_US = (t1 - t0) / (CALIBRATION_MS * 1000);
        if TSC_PER_US == 0 {
            // A TSC too slow (or a `t1==t0` freak reading) to measure at
            // this resolution would make every delay/clock call below
            // divide-by-zero or never advance; 1 keeps them merely
            // imprecise instead of broken.
            TSC_PER_US = 1;
        }
        TSC_AT_BOOT = t1;

        crate::drivers::serial::write(b"timer: TSC_PER_US=");
        print_decimal(TSC_PER_US);
        crate::drivers::serial::write(b"\n");
    }
}

/// Microseconds since `calibrate()` was called.
pub fn now_us() -> u64 {
    unsafe { (cpu::rdtsc() - TSC_AT_BOOT) / TSC_PER_US }
}

pub fn now_ms() -> u64 {
    now_us() / 1000
}

/// Busy-wait for at least `us` microseconds. Bounded by construction (a
/// fixed TSC deadline) -- prefer this over an unbounded `loop { pause() }`
/// for any new hardware-wait code.
pub fn delay_us(us: u64) {
    let target = unsafe { cpu::rdtsc() } + us * unsafe { TSC_PER_US };
    while unsafe { cpu::rdtsc() } < target {
        unsafe { cpu::pause() };
    }
}

/// Program PIT channel 0 (IRQ0, remapped by `pic::init` to vector 32) to
/// fire `hz` times per second. This is independent of channel 2 (used only
/// by `calibrate`'s one-shot measurement above) and of the TSC clock this
/// module otherwise runs on — it exists purely to give the CPU a periodic
/// interrupt: proof the IDT/PIC pipeline works end-to-end, a real-time
/// heartbeat `keyboard.rs` uses to drop its idle-font busy-poll, and the
/// tick Phase 3's scheduler rewrite will drive preemption from.
pub fn start_periodic(hz: u32) {
    let divisor = (PIT_FREQUENCY_HZ / hz as u64).clamp(1, u16::MAX as u64) as u16;
    unsafe {
        // Channel 0, lobyte/hibyte access, mode 3 (square wave generator —
        // free-running, unlike channel 2's one-shot mode 0 above), binary.
        io::outb(PIT_COMMAND, 0b0011_0110);
        io::outb(PIT_CHANNEL0_DATA, (divisor & 0xFF) as u8);
        io::outb(PIT_CHANNEL0_DATA, (divisor >> 8) as u8);
    }
}

/// Poll `cond` (true = condition satisfied) until it's true or `timeout_us`
/// microseconds pass, `pause()`-ing between checks. Returns whether `cond`
/// was actually satisfied — `false` means it timed out. The bounded
/// counterpart to a raw `while !ready() { pause() }` hardware-wait loop:
/// with no drive/controller present, `cond` may simply never become true
/// (a floating I/O bus reads back with status bits stuck set), and without
/// a deadline that hangs the caller — and everything after it — forever.
pub fn poll_until(timeout_us: u64, mut cond: impl FnMut() -> bool) -> bool {
    let deadline = now_us() + timeout_us;
    loop {
        if cond() {
            return true;
        }
        if now_us() >= deadline {
            return false;
        }
        unsafe { cpu::pause() };
    }
}

/// Print `n` as decimal ASCII to serial (no leading zeros; "0" for zero) --
/// same shape as `lingfs.rs`'s `print_decimal`, just `u64` and serial-only
/// (this runs at boot, before the active terminal is necessarily useful).
fn print_decimal(n: u64) {
    if n == 0 {
        crate::drivers::serial::write(b"0");
        return;
    }
    let mut digits = [0u8; 20];
    let mut i = digits.len();
    let mut v = n;
    while v > 0 {
        i -= 1;
        digits[i] = b'0' + (v % 10) as u8;
        v /= 10;
    }
    crate::drivers::serial::write(&digits[i..]);
}
