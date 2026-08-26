#![no_std]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

// The kernel now has a real global allocator (a `GlobalAlloc` adapter over the
// slab in `mm::heap`), so `alloc` collections are available crate-wide -- the
// foundation for the in-kernel `ling` tree-walking interpreter.
extern crate alloc;

/// Route every `alloc`/`dealloc` through the slab heap. Safe because
/// `mm::init_physical_memory()` seeds the frame allocator early in `init()`,
/// before anything allocates.
#[global_allocator]
static KERNEL_HEAP: crate::mm::heap::KernelHeap = crate::mm::heap::KernelHeap;

// ─── Per-architecture backend (CPU intrinsics, boot stub, port/MMIO I/O) ────
pub mod arch;

// ─── Device drivers (console, input, storage, framebuffer) ─────────────────
pub mod drivers;

// ─── lingfs and everything built on it (block device, users, packages) ─────
pub mod fs;

// ─── Task scheduling ─────────────────────────────────────────────────────
pub mod proc;

// ─── Syscall ABI ring-3 processes use ────────────────────────────────────
pub mod abi;

// ─── Physical memory management (frame allocator; paging/heap follow) ──────
pub mod mm;

// ─── Arch-neutral Ling runtime support: hashing, allocation, strings, sigs ──
pub mod ed25519;
pub mod hash;
// In-kernel tree-walking interpreter for a subset of Ling (`ling run` in the
// Terminal); built on the global allocator above.
pub mod ling;
pub mod runtime;
pub mod strings;

#[cfg(target_arch = "x86_64")]
pub mod history;
#[cfg(target_arch = "x86_64")]
pub mod life;

use core::ptr;

// Bring the moved-under-`arch`/`drivers`/`fs`/`proc` modules back into scope
// under their original bare names — this file is the crate root, so before
// the Phase 1 restructure every one of these was a direct child module,
// nameable without qualification; now that they're nested, that only still
// holds with an explicit `use`.
use crate::arch::cpu;
#[cfg(target_arch = "x86_64")]
use crate::arch::{bootmodule, io, timer};
#[cfg(target_arch = "aarch64")]
use crate::drivers::uart;
#[cfg(target_arch = "x86_64")]
use crate::drivers::{
    browser, display, flags, font8x8, font_unicode, framebuffer, kbdlayout, keyboard, lingfu,
    locale, mixer, mouse, net_e1000, netstack, serial, term, theme, ui_scale, vga, wm, wm_liquid,
};
#[cfg(target_arch = "x86_64")]
use crate::fs::{blockdev, lingfs, packages, users};
#[cfg(target_arch = "x86_64")]
use crate::proc::sched;

/// Initialize the kernel: bring up the console (VGA+serial on x86_64, PL011
/// UART on aarch64/Raspberry Pi) and print a ready banner.
pub fn init() {
    #[cfg(target_arch = "x86_64")]
    {
        serial::init();
        framebuffer::init();
        // Reprograms the VGA DAC palette entries (not just which of the 16
        // slots a cell picks) — swap `THEME_LINGOS` for another `vga::Theme`
        // to retheme the whole console; see its doc comment.
        vga::apply_theme(&vga::THEME_LINGOS);
        unsafe { vga::disable_hardware_cursor() };
        term::clear_active();
        term::set_color_active(vga::color_byte(
            vga::Color::LightGreen as u8,
            vga::Color::Black as u8,
        ));
        serial::write(b"ling-kernel initialized\n");
        timer::calibrate();

        // Real interrupts from here on: GDT+TSS (IST1 double-fault stack)
        // before the IDT (whose gates reference the GDT's kernel code
        // selector), the IDT before the PIC remap (a stray IRQ landing on an
        // unmapped vector before this would triple-fault instead of hitting
        // `isr_spurious`), and the mouse's synchronous 8042 handshake before
        // IRQ12 is unmasked (see `mouse::init`'s doc comment for why the
        // order matters there specifically).
        arch::gdt::init(arch::boot::boot_stack_top());
        arch::idt::init();
        arch::pic::init();
        mouse::init();
        arch::pic::unmask(0); // PIT heartbeat
        arch::pic::unmask(1); // keyboard
        arch::pic::unmask(12); // mouse
        timer::start_periodic(100);
        unsafe { cpu::sti() };
        serial::write(b"interrupts enabled (IDT+PIC, 100Hz heartbeat)\n");

        mm::init_physical_memory();
        console_write(b"mm: ");
        print_decimal((mm::frame::free_frame_count() * 4) as u64);
        console_write(b" KiB free (frame allocator)\n");

        // Real page tables (NX + W^X on the kernel image) replacing
        // boot.rs's temporary flat RWX map — must come after the frame
        // allocator (page tables are themselves frame-allocated) and before
        // anything that depends on the kernel's .text/.rodata actually
        // being protected.
        arch::paging::init();
        serial::write(b"paging enabled (NX, W^X kernel image)\n");

        // Ring-3 processes: must come after paging (per-process address
        // spaces clone the kernel PML4) and after the GDT (STAR encodes
        // its kernel code selector) — before anything ever executes
        // `syscall`, which is only true once a process exists at all.
        arch::trap::init();
        serial::write(b"syscall/sysret enabled (ring-3 processes ready)\n");

        // Establish the shell as slot 0 of the cooperative scheduler before
        // anything (a shell command, `ling_kernel_spawn`) can depend on it.
        sched::init_main_task();
    }
    #[cfg(target_arch = "aarch64")]
    {
        uart::init();
        uart::write(b"ling-kernel initialized\n");

        // Same ordering discipline as the x86_64 side: install the vector
        // table before anything can be routed to it (`arch::intc::init`),
        // and finish every one-time setup before the first `cpu::sti`
        // (`daifclr`) actually lets an IRQ land.
        arch::vectors::init();
        arch::intc::init();
        arch::timer::calibrate();
        arch::timer::start_periodic(100);
        unsafe { cpu::sti() };
        uart::write(b"interrupts enabled (VBAR_EL1+intc, 100Hz heartbeat)\n");

        mm::init_physical_memory();
        console_write(b"mm: ");
        print_decimal((mm::frame::free_frame_count() * 4) as u64);
        console_write(b" KiB free (frame allocator)\n");
    }
}

/// Write a byte string to the kernel's primary console (the active terminal
/// + serial on x86_64, UART on aarch64). Shared by the panic path and the
/// `ling_kernel_*` intrinsics below, so both architectures stay behind one
/// set of names.
pub(crate) fn console_write(bytes: &[u8]) {
    #[cfg(target_arch = "x86_64")]
    {
        for &b in bytes {
            term::write_char_active(b);
        }
        serial::write(bytes);
    }
    #[cfg(target_arch = "aarch64")]
    {
        uart::write(bytes);
    }
}

/// Print `n` as decimal ASCII to the console — same shape as the
/// x86_64-only `timer.rs`'s private `print_decimal`, kept separate since
/// this one needs to work on both architectures.
fn print_decimal(n: u64) {
    if n == 0 {
        console_write(b"0");
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
    console_write(&digits[i..]);
}

/// Print `n` as two lowercase hex digits (zero-padded) -- for MAC address
/// bytes, where the conventional hex notation reads far more recognizably
/// than `print_decimal`'s output (confirmed the hard way: misreading a
/// decimal-formatted MAC as hex cost real debugging time chasing a bug that
/// turned out not to exist).
fn print_hex_byte(n: u8) {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    console_write(&[DIGITS[(n >> 4) as usize], DIGITS[(n & 0xF) as usize]]);
}

fn console_clear() {
    // aarch64: no general "clear screen" concept over a plain UART, so this
    // is a no-op there.
    #[cfg(target_arch = "x86_64")]
    term::clear_active();
}

/// Panic on unreachable kernel path.
pub fn kernel_panic(msg: &str) -> ! {
    console_write(b"KERNEL PANIC: ");
    console_write(msg.as_bytes());
    console_write(b"\n");
    loop {
        unsafe {
            cpu::halt();
        }
    }
}

// ─── Extern "C" ABI for the AOT compiler's kernel intrinsics ────────────
// Same symbol names on every architecture, so `.ling` kernel source is
// portable between the x86_64 and aarch64 (Raspberry Pi) targets — only the
// HAL underneath differs.

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_vga_write_str(s: u64) -> u64 {
    // s is a NaN-boxed string, but in kernel mode we pass raw ptr+len
    // For now, treat s as a pointer to a null-terminated string
    if s != 0 {
        let ptr = s as *const u8;
        let mut i = 0;
        loop {
            let c = ptr::read_volatile(ptr.add(i));
            if c == 0 {
                break;
            }
            i += 1;
        }
        console_write(core::slice::from_raw_parts(ptr, i));
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_vga_write_char(c: u64) -> u64 {
    console_write(&[c as u8]);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_vga_clear() -> u64 {
    console_clear();
    0
}

/// Set the console's current foreground/background color (0..15, standard
/// VGA palette order — see `vga::Color`). No-op on aarch64: a plain UART has
/// no color concept (that's the connecting terminal emulator's job).
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_vga_set_color(fg: u64, bg: u64) -> u64 {
    #[cfg(target_arch = "x86_64")]
    term::set_color_active(vga::color_byte(fg as u8, bg as u8));
    #[cfg(target_arch = "aarch64")]
    let _ = (fg, bg);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_halt() -> u64 {
    loop {
        cpu::halt();
    }
}

/// Reboot the machine (8042 pulse -> triple-fault fallback). Never
/// returns. See `arch::power`'s doc for the VM-vs-real-hardware scope.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_reboot() -> u64 {
    arch::power::reboot();
}

/// Power off (VM ACPI-shutdown ports). Never returns on a VM; halts if no
/// port took.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_poweroff() -> u64 {
    arch::power::poweroff();
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_cli() -> u64 {
    cpu::cli();
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_sti() -> u64 {
    cpu::sti();
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_inb(port: u64) -> u64 {
    io::inb(port as u16) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_outb(port: u64, val: u64) -> u64 {
    io::outb(port as u16, val as u8);
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_serial_write(ptr: u64, len: u64) -> u64 {
    let slice = core::slice::from_raw_parts(ptr as *const u8, len as usize);
    console_write(slice);
    0
}

/// Spawn `entry` (a function pointer already linked into this same kernel
/// image — see `sched`'s module doc) as a new cooperative task. Returns its
/// slot index, or `u64::MAX` if every slot is taken.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_spawn(entry: u64) -> u64 {
    sched::spawn(entry)
}

/// Voluntarily give up the CPU to the next `Ready` task, if any. The 100Hz
/// timer heartbeat isn't wired to preemption, so nothing else ever runs
/// unless something calls this (or `ling_kernel_exit`) — see `sched`'s
/// module doc.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_yield() -> u64 {
    sched::yield_now();
    0
}

/// End the calling task and hand off to whatever's `Ready` next. Never
/// returns.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_exit() -> u64 {
    sched::exit()
}

/// The calling task's own slot index.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_getpid() -> u64 {
    sched::getpid()
}

/// Milliseconds since boot (calibrated TSC, see `arch::timer`) -- for `.ling`
/// code that needs to time something (an animation, a timeout) without any
/// mutable-state workaround, since it can read a fresh value each call.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_uptime_ms() -> u64 {
    timer::now_ms()
}

/// Seconds since the Unix epoch, per the CMOS RTC (see `arch::rtc`'s doc for
/// what "UTC" means here in practice). The one source of real wall-clock
/// time anywhere in this kernel -- everything else is boot-relative only.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_rtc_unix_ts() -> u64 {
    arch::rtc::unix_timestamp().max(0) as u64
}

/// Individual UTC calendar fields from the CMOS RTC, for callers that want
/// to render a date/time rather than just diff two timestamps.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_rtc_year() -> u64 {
    arch::rtc::read().year as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_rtc_month() -> u64 {
    arch::rtc::read().month as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_rtc_day() -> u64 {
    arch::rtc::read().day as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_rtc_hour() -> u64 {
    arch::rtc::read().hour as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_rtc_minute() -> u64 {
    arch::rtc::read().minute as u64
}

/// Locale/timezone table — see `drivers::locale`'s module doc for what's
/// real (Earth UTC offsets, the Moon's actual synodic-month day cycle) and
/// what's disclosed as symbolic (the invented celestial locales' offsets).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_count() -> u64 {
    locale::count() as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_id(i: u64) -> u64 {
    let s = locale::get(i as usize).map(|l| l.id).unwrap_or("");
    strings::ling_str_new(s.as_ptr(), s.len())
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_native_name(i: u64) -> u64 {
    let s = locale::get(i as usize).map(|l| l.native_name).unwrap_or("");
    strings::ling_str_new(s.as_ptr(), s.len())
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_latin_name(i: u64) -> u64 {
    let s = locale::get(i as usize).map(|l| l.latin_name).unwrap_or("");
    strings::ling_str_new(s.as_ptr(), s.len())
}
/// Signed minutes offset from UTC, bit-cast into a `u64` ((`as i64 as u64`) --
/// `.ling` has no signed-integer FFI type, so callers that need the sign
/// back must reinterpret the low 32 bits as `i32` themselves (offsets here
/// never exceed +-2048 minutes, well within range).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_utc_offset_min(i: u64) -> u64 {
    locale::get(i as usize).map(|l| l.utc_offset_min).unwrap_or(0) as i64 as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_is_celestial(i: u64) -> u64 {
    match locale::get(i as usize).map(|l| l.kind) {
        Some(locale::Kind::Celestial) => 1,
        _ => 0,
    }
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_uses_daemon(i: u64) -> u64 {
    locale::get(i as usize).map(|l| l.uses_daemon_script).unwrap_or(false) as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_flag_id(i: u64) -> u64 {
    locale::get(i as usize).map(|l| l.flag_id as u64).unwrap_or(u64::MAX)
}
/// Permille (0..1000) through the Moon's real day/night cycle right now --
/// see `drivers::locale::moon_day_progress_permille`'s doc for the actual
/// astronomy behind this number.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_moon_day_progress() -> u64 {
    locale::moon_day_progress_permille() as u64
}

/// The user's picked locale index, or `locale::count()` (one past the last
/// valid index — small and exact, unlike `u64::MAX`, which `.ling`'s
/// number type can't represent losslessly) if none yet. See
/// `drivers::locale::select`'s doc for why this is kernel-side state.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_selected() -> u64 {
    locale::selected().unwrap_or(locale::count()) as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_index_of_id(id: u64) -> u64 {
    locale::index_of_id(arg_str(id)) as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_select(i: u64) -> u64 {
    locale::select(i as usize);
    0
}
/// See `drivers::locale::reset`'s doc -- lets a multi-step wizard reuse the
/// same picker screen for two independent choices (language, then locale/
/// timezone) over the same table.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_reset() -> u64 {
    locale::reset();
    0
}

/// See `drivers::kbdlayout`'s module doc for what's actually covered
/// (5 real ASCII-position remaps) and, just as importantly, what isn't
/// (any layout needing non-Latin character output or a real IME).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_count() -> u64 {
    kbdlayout::count() as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_name(i: u64) -> u64 {
    let s = kbdlayout::name(i as usize);
    strings::ling_str_new(s.as_ptr(), s.len())
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_current() -> u64 {
    kbdlayout::current() as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_set(i: u64) -> u64 {
    kbdlayout::set_current(i as usize);
    0
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_cursor() -> u64 {
    kbdlayout::cursor() as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_cursor_up() -> u64 {
    kbdlayout::cursor_up();
    0
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_cursor_down() -> u64 {
    kbdlayout::cursor_down();
    0
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_confirm_cursor() -> u64 {
    kbdlayout::confirm_cursor();
    0
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_reset() -> u64 {
    kbdlayout::reset();
    0
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_layout_selected() -> u64 {
    kbdlayout::selected() as u64
}

/// The picker's currently-highlighted row -- see `drivers::locale::cursor`'s
/// doc for how this differs from `ling_kernel_locale_selected`.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_cursor() -> u64 {
    locale::cursor() as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_cursor_up() -> u64 {
    locale::cursor_up();
    0
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_cursor_down() -> u64 {
    locale::cursor_down();
    0
}
/// Select whatever row the cursor currently highlights -- the Enter-key
/// action.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_locale_confirm_cursor() -> u64 {
    locale::confirm_cursor();
    0
}

/// Draw the flag matching `locale::Locale::flag_id` into the box
/// `(x, y, w, h)` — see `drivers::flags`'s module doc for what "flag" means
/// here (hand-encoded proportional layouts, not a decoded image file).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_draw_flag(
    id: u64,
    x: u64,
    y: u64,
    w: u64,
    h: u64,
) -> u64 {
    flags::draw(id as u32, x as u32, y as u32, w as u32, h as u32);
    0
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_rtc_second() -> u64 {
    arch::rtc::read().second as u64
}

/// Run `lingfs::self_test()` against the real disk (mount/format, put,
/// get, dedup, tree, commit, root round-trip). Returns 1 on success, 0 on
/// failure. This *mutates* the filesystem (adds a throwaway commit each
/// run) — fine as an opt-in diagnostic (a shell `selftest` command), but
/// not something to run automatically at every boot once the filesystem
/// holds anything real, since it'd bury the actual root under a fresh
/// test commit every time. x86_64 only (needs the ATA driver).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fs_selftest() -> u64 {
    lingfs::self_test() as u64
}

/// Load and run `testbins/ring3_selftest.elf` — a hand-assembled ring-3
/// program (source alongside it, `testbins/ring3_selftest.asm`) that writes
/// a string via `SYS_WRITE` and exits via `SYS_EXIT(42)` — end to end
/// through real paging, the real GDT/TSS ring-3 descriptors, real
/// `syscall`/`iretq` transitions, and `abi::syscalls::dispatch`'s pointer
/// validation. Returns 1 if the process ran and exited with code 42
/// (proving the whole mechanism actually works, not just that it compiled),
/// 0 otherwise.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_proc_selftest() -> u64 {
    static TEST_ELF: &[u8] = include_bytes!("../testbins/ring3_selftest.elf");
    let pid = match proc::uproc::spawn(TEST_ELF) {
        Ok(p) => p,
        Err(e) => {
            console_write(b"proctest: spawn failed: ");
            console_write(e.as_bytes());
            console_write(b"\n");
            return 0;
        }
    };
    let code = proc::uproc::run_to_completion(pid);
    console_write(b"proctest: exit code = ");
    print_decimal(code as u64);
    console_write(b"\n");
    (code == 42) as u64
}

/// Spawn two independent copies of `testbins/loop_selftest.elf` — each
/// prints its own pid-derived letter six times with a busy-spin between
/// prints, *never* calling `yield` or `sleep_ms` — and run both to
/// completion. Both are `Ready` before either runs, so the timer's
/// preemptive scheduler (`uproc::on_timer_preempt`) is the only thing that
/// interleaves them; genuine concurrent progress shows up as an interleaved
/// mix of both processes' letters in the serial log rather than one
/// process's six lines followed by the other's. Returns 1 if both exited
/// with the expected code 7, 0 otherwise.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_proc_pairtest() -> u64 {
    static TEST_ELF: &[u8] = include_bytes!("../testbins/loop_selftest.elf");
    let pid_a = match proc::uproc::spawn(TEST_ELF) {
        Ok(p) => p,
        Err(e) => {
            console_write(b"pairtest: spawn a failed: ");
            console_write(e.as_bytes());
            console_write(b"\n");
            return 0;
        }
    };
    let pid_b = match proc::uproc::spawn(TEST_ELF) {
        Ok(p) => p,
        Err(e) => {
            console_write(b"pairtest: spawn b failed: ");
            console_write(e.as_bytes());
            console_write(b"\n");
            return 0;
        }
    };
    console_write(b"pairtest: running two processes under the preemptive timer\n");
    // A single `run_to_completion` call blocks until every `Ready` process
    // in the table is done, which by construction includes `pid_b` too
    // (spawned before this call) — calling it a second time here would
    // re-launch `pid_b`'s already-exited frame straight into a fault.
    let code_a = proc::uproc::run_to_completion(pid_a);
    let code_b = proc::uproc::exit_code(pid_b).unwrap_or(-1);
    console_write(b"pairtest: exit codes = ");
    print_decimal(code_a as u64);
    console_write(b", ");
    print_decimal(code_b as u64);
    console_write(b"\n");
    (code_a == 7 && code_b == 7) as u64
}

/// Print pid/state/exit-code for every live process slot — the `ps` shell
/// command's backing intrinsic.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_proc_ps() -> u64 {
    let mut rows = [(0usize, 0u8, 0i32); proc::uproc::MAX_PROCESSES];
    let n = proc::uproc::snapshot(&mut rows);
    console_write(b"PID STATE EXIT\n");
    for &(pid, tag, code) in &rows[..n] {
        print_decimal(pid as u64);
        console_write(b"   ");
        console_write(&[tag]);
        console_write(b"     ");
        if tag == b'Z' {
            print_decimal(code as u64);
        }
        console_write(b"\n");
    }
    n as u64
}

/// Mount (or format, if blank) the filesystem without running the
/// mutating self-test — what actually runs at every boot. Returns 1 on
/// success, 0 on failure.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fs_mount() -> u64 {
    let Ok(()) = lingfs::mount() else { return 0 };
    // Best-effort: an empty/full-root failure here shouldn't fail the mount
    // itself — `dev`/`bin` just won't show up in `ls` this boot.
    let _ = lingfs::seed_defaults();
    1
}

/// Print a directory's entries — backs the shell's `ls`. `arg` is
/// everything typed after `ls`: any whitespace-separated token starting
/// with `-` is parsed as flags (`a` = show hidden/dot-prefixed entries,
/// `l` = long/`-l`-style listing; combinable, e.g. `-la`), in any order
/// relative to the one non-flag token, which names a one-level directory
/// (`""`/root if omitted — the synthetic `dev`/`bin` directories included).
/// Parsed here (not in `.ling` shell source) since Ling's string handling
/// doesn't have a general tokenizer to build this on.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fs_ls(arg: u64) -> u64 {
    let arg = if arg == 0 {
        ""
    } else {
        core::str::from_utf8(strings::bytes_of_ptr(arg as *mut u8)).unwrap_or("")
    };

    let mut show_all = false;
    let mut long = false;
    let mut path = "";
    for token in arg.split_whitespace() {
        if let Some(flags) = token.strip_prefix('-') {
            for c in flags.chars() {
                match c {
                    'a' => show_all = true,
                    'l' => long = true,
                    _ => {},
                }
            }
        } else {
            path = token;
        }
    }
    lingfs::print_ls(path, show_all, long);
    0
}

/// Create or overwrite a top-level file named `name` with `content` (both
/// real Ling strings — see `strings.rs`). Returns 1 on success, 0 on
/// failure (e.g. content too large for lingfs's one-block-per-file v1
/// format, or the root tree is full).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fs_write(name: u64, content: u64) -> u64 {
    let name = core::str::from_utf8(strings::bytes_of_ptr(name as *mut u8)).unwrap_or("");
    let content = strings::bytes_of_ptr(content as *mut u8);
    lingfs::write_file(name, content).is_ok() as u64
}

/// Read a top-level file's content as a new Ling string. Returns an empty
/// string if the file doesn't exist — there's no `Option` return across
/// this ABI, so "empty" and "missing" look the same to the caller for now.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fs_read(name: u64) -> u64 {
    let name = core::str::from_utf8(strings::bytes_of_ptr(name as *mut u8)).unwrap_or("");
    let mut buf = [0u8; lingfs::BLOCK_SIZE];
    let len = lingfs::read_file(name, &mut buf)
        .ok()
        .flatten()
        .unwrap_or(0);
    strings::ling_str_new(buf.as_ptr(), len)
}

/// The part of a line before its first space (or the whole line, if none)
/// — the shell's command-name extractor.
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_str_cmd(line: u64) -> u64 {
    let bytes = strings::bytes_of_ptr(line as *mut u8);
    let end = bytes.iter().position(|&b| b == b' ').unwrap_or(bytes.len());
    strings::ling_str_new(bytes.as_ptr(), end)
}

/// The part of a line after its first space, with that space stripped
/// (empty string if there's no space) — the shell's argument extractor.
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_str_arg(line: u64) -> u64 {
    let bytes = strings::bytes_of_ptr(line as *mut u8);
    match bytes.iter().position(|&b| b == b' ') {
        Some(sp) => strings::ling_str_new(bytes[sp + 1..].as_ptr(), bytes.len() - sp - 1),
        None => strings::ling_str_new(bytes.as_ptr(), 0),
    }
}

/// Switch to `vga::THEME_LINGOS` (the default dark theme).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_theme_dark() -> u64 {
    vga::apply_theme(&vga::THEME_LINGOS);
    0
}

/// Switch to `vga::THEME_LIGHT`.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_theme_light() -> u64 {
    vga::apply_theme(&vga::THEME_LIGHT);
    0
}

/// Whether GRUB actually handed back a linear framebuffer (Multiboot2 tag
/// type 8) rather than leaving us in VGA text mode. Check this before
/// calling any other `ling_kernel_fb_*` function — they're all silent
/// no-ops without one.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_available() -> u64 {
    framebuffer::available() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_width() -> u64 {
    framebuffer::width() as u64
}

/// Map a virtual-canvas coordinate/length (see `drivers::ui_scale`'s module
/// doc -- a fixed 960x600 design canvas, uniformly scaled and letterboxed
/// to whatever real resolution GRUB actually negotiated) to real
/// framebuffer pixels. `ui_x`/`ui_y` include the letterbox offset (use for
/// a rect's position); `ui_len` doesn't (use for a rect's width/height).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_ui_x(vx: u64) -> u64 {
    ui_scale::x(vx) as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_ui_y(vy: u64) -> u64 {
    ui_scale::y(vy) as u64
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_ui_len(v: u64) -> u64 {
    ui_scale::len(v) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_height() -> u64 {
    framebuffer::height() as u64
}

/// `color` is packed `0x00RRGGBB`.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_set_pixel(x: u64, y: u64, color: u64) -> u64 {
    framebuffer::set_pixel(x as u32, y as u32, color as u32);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_fill_rect(
    x: u64,
    y: u64,
    w: u64,
    h: u64,
    color: u64,
) -> u64 {
    framebuffer::fill_rect(x as u32, y as u32, w as u32, h as u32, color as u32);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_clear(color: u64) -> u64 {
    framebuffer::clear(color as u32);
    0
}

/// Back-buffer drawing (see `framebuffer.rs`'s doc comment): build a whole
/// frame with these, then call `ling_kernel_fb_present` once to blit it to
/// the real framebuffer in one shot — avoids the visible tearing that
/// drawing shapes straight into the live framebuffer causes once anything
/// moves frame to frame.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_back_clear(color: u64) -> u64 {
    framebuffer::back_clear(color as u32);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_back_fill_rect(
    x: u64,
    y: u64,
    w: u64,
    h: u64,
    color: u64,
) -> u64 {
    framebuffer::back_fill_rect(x as u32, y as u32, w as u32, h as u32, color as u32);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_present() -> u64 {
    framebuffer::present();
    0
}

/// Draw an 8x8-per-character bitmap string into the framebuffer's back
/// buffer at `(x, y)` -- see `drivers::font8x8`'s module doc for why this
/// exists (nothing rendered text into pixel-graphics mode before it; the
/// VGA hardware font is a different, text-mode-only consumer). `fg`/`bg`
/// are 0x00RRGGBB, same as every other `ling_kernel_fb_*` color argument.
/// Call `ling_kernel_fb_present` once after a batch of draws, same as the
/// fill/clear primitives it's built on.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_draw_str(x: u64, y: u64, s: u64, fg: u64, bg: u64) -> u64 {
    font8x8::draw_str(
        x as u32,
        y as u32,
        arg_str(s).as_bytes(),
        fg as u32,
        bg as u32,
    );
    0
}

/// Draw a UTF-8 string that may contain non-ASCII text -- see
/// `drivers::font_unicode`'s module doc for what's actually covered (a
/// curated character set, not full Unicode) and its real limitations (no
/// text shaping). `daemon` non-zero renders ASCII bytes through the
/// constructed-script glyph reskin instead of plain Latin. Returns the
/// pixel width drawn.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_draw_utf8_str(
    x: u64,
    y: u64,
    s: u64,
    fg: u64,
    bg: u64,
    daemon: u64,
) -> u64 {
    font_unicode::draw_utf8_str(
        x as u32,
        y as u32,
        arg_str(s).as_bytes(),
        fg as u32,
        bg as u32,
        daemon != 0,
    ) as u64
}

/// Advance the "liquid window" spring simulation one frame toward
/// `(target_x, target_y)` -- see `drivers::wm_liquid`'s module doc for why
/// this state lives here and not in `.ling`: the kernel-build path has no
/// mutable-reassignment construct (`bind` only ever introduces a *new*
/// name) and the compiler does no tail-call optimization, so neither real
/// mutation nor safe unbounded recursion exists on the `.ling` side to hold
/// frame-persistent physics state in a loop that runs indefinitely. Call
/// once per frame, before the position/scale getters below. All the actual
/// window/event-loop orchestration stays in `.ling` (`kernel/x86_64-wm`) --
/// this is only the numeric integration step, same division of labor as
/// every other piece of kernel state (`ling_kernel_mouse_x`, `_getpid`, …).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_liquid_step(target_x: u64, target_y: u64) -> u64 {
    wm_liquid::step(target_x as f64, target_y as f64);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_liquid_x() -> u64 {
    wm_liquid::x().max(0.0) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_liquid_y() -> u64 {
    wm_liquid::y().max(0.0) as u64
}

/// Width/height scale, as a percentage (100 = unscaled) -- an integer
/// rather than a raw float since `.ling` only has integer arithmetic over
/// this flat-u64 FFI boundary; multiply/divide by 100 on the caller side.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_liquid_scale_w_pct() -> u64 {
    wm_liquid::scale_w_pct()
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_liquid_scale_h_pct() -> u64 {
    wm_liquid::scale_h_pct()
}

// -- UI themes (drivers::theme) -- one palette table for every graphics
// surface; see the module doc for the slot contract. ---------------------

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_theme_count() -> u64 {
    theme::count() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_theme_current() -> u64 {
    theme::current() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_theme_select(idx: u64) -> u64 {
    theme::set(idx as usize);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_theme_name(idx: u64) -> u64 {
    let n = theme::name(idx as usize);
    strings::ling_str_new(n.as_ptr(), n.len())
}

/// Palette lookup by slot index (`theme::SLOT_*`) -- the one call every
/// themed `.ling` screen routes its colors through.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_theme_color(slot: u64) -> u64 {
    theme::color(slot as usize) as u64
}

// -- Rounded/blended framebuffer primitives (drivers::framebuffer) -- what
// makes windows/cards/dots genuinely rounded and shadowed instead of the
// rect-only approximations gui-common used to disclose. ------------------

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_back_fill_rrect(
    x: u64,
    y: u64,
    w: u64,
    h: u64,
    r: u64,
    color: u64,
) -> u64 {
    framebuffer::back_fill_rounded_rect(x as u32, y as u32, w as u32, h as u32, r as u32, color as u32);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_back_fill_circle(cx: u64, cy: u64, r: u64, color: u64) -> u64 {
    framebuffer::back_fill_circle(cx as u32, cy as u32, r as u32, color as u32);
    0
}

/// `alpha` 0..=255. Per-pixel read-modify-write -- use for shadows and
/// glass, not for large opaque fills.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_back_blend_rect(
    x: u64,
    y: u64,
    w: u64,
    h: u64,
    color: u64,
    alpha: u64,
) -> u64 {
    framebuffer::back_blend_rect(x as u32, y as u32, w as u32, h as u32, color as u32, alpha as u32);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_fb_back_blend_rrect(
    x: u64,
    y: u64,
    w: u64,
    h: u64,
    r: u64,
    color: u64,
    alpha: u64,
) -> u64 {
    framebuffer::back_blend_rounded_rect(
        x as u32,
        y as u32,
        w as u32,
        h as u32,
        r as u32,
        color as u32,
        alpha as u32,
    );
    0
}

// -- Multi-window manager (drivers::wm) -- state/hit-testing kernel-side,
// chrome drawing in .ling; see the module doc for the honest scope
// (kernel-side windows, not an X-style client/server protocol). ----------

/// One frame of window-system logic (click routing, drag, springs). Call
/// once per frame before any `ling_kernel_wm_slot_*` getter.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_step(mx: u64, my: u64, buttons: u64) -> u64 {
    wm::step(mx as i64, my as i64, buttons as u8);
    0
}

/// Route one key (the keyboard driver's byte encoding, same as the
/// pickers) to the focused window.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_key(k: u64) -> u64 {
    wm::key(k as u8);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_open(kind: u64) -> u64 {
    wm::open(kind as u8);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_count() -> u64 {
    wm::slot_count() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_titlebar_h() -> u64 {
    wm::TITLEBAR_H as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_slot_x(slot: u64) -> u64 {
    wm::slot_x(slot as usize).max(0) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_slot_y(slot: u64) -> u64 {
    wm::slot_y(slot as usize).max(0) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_slot_w(slot: u64) -> u64 {
    wm::slot_w(slot as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_slot_h(slot: u64) -> u64 {
    wm::slot_h(slot as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_slot_kind(slot: u64) -> u64 {
    wm::slot_kind(slot as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_slot_focused(slot: u64) -> u64 {
    wm::slot_focused(slot as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_slot_title(slot: u64) -> u64 {
    let t = wm::slot_title(slot as usize);
    strings::ling_str_new(t.as_ptr(), t.len())
}

/// Render the focused-content interior of the window at z `slot` (settings
/// rows, file listings -- runtime-sized loops `.ling` kernel code cannot
/// express). The `.ling` caller draws the chrome first, then calls this.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_draw_content(slot: u64) -> u64 {
    wm::draw_content(slot as usize);
    0
}

/// Render a whole window at z `slot` kernel-side -- soft shadow, the
/// soap-film-bending rounded body, titlebar + buttons, and its content.
/// Handles the dissolve-on-close animation too. Replaces the per-slot
/// rect drawing the `.ling` desktop used to do (rects can't shear or
/// sublimate); `main.ling` now just calls this once per z slot.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_draw_window(slot: u64) -> u64 {
    wm::draw_window(slot as usize);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_dock_count() -> u64 {
    wm::dock_count() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_dock_x(i: u64) -> u64 {
    wm::dock_x(i as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_dock_y() -> u64 {
    wm::dock_y() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_dock_size() -> u64 {
    wm::dock_icon_size() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_dock_letter(i: u64) -> u64 {
    let s = wm::dock_letter(i as usize);
    strings::ling_str_new(s.as_ptr(), s.len())
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_dock_running(i: u64) -> u64 {
    wm::dock_running(i as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_dock_hover(i: u64) -> u64 {
    wm::dock_hover(i as usize) as u64
}

/// "HH:MM" (or 12-hour, per Settings) in the selected locale's UTC offset
/// -- real CMOS clock through the real locale table.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_clock_str() -> u64 {
    let s = wm::clock_str();
    strings::ling_str_new(s.as_ptr(), s.len())
}

/// Wallpaper from the active theme + Settings' wallpaper mode (gradient /
/// ROYGBIV / solid / loaded image) -- the desktop's flat-clear
/// replacement.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_draw_wallpaper() -> u64 {
    wm::draw_wallpaper();
    0
}

/// Load a BMP from lingfs as the desktop wallpaper (the Files app's "open
/// a .bmp"); 1 if it decoded and is now showing.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_set_wallpaper_image(name: u64) -> u64 {
    wm::set_wallpaper_image(arg_str(name)) as u64
}

/// Draw the MATE-style applications menu (top-left dropdown). Called last
/// in the frame so it overlays windows and the dock; a no-op when closed.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_draw_menu() -> u64 {
    wm::draw_menu();
    0
}

/// 1 when the user picked Log Out from the Power menu -- the desktop loop
/// tears down the session and returns to the greeter. `clear` resets it.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_logout_requested() -> u64 {
    wm::logout_requested() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_clear_logout() -> u64 {
    wm::clear_logout();
    0
}

// -- The browser (drivers::browser + the bring-browser engine) -----------

/// Fetch + render `url` at `cols` columns; 1 on success, 0 with the reason
/// via `ling_kernel_browser_status`.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_browser_go(url: u64, cols: u64) -> u64 {
    browser::go(arg_str(url), cols as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_browser_status() -> u64 {
    let s = browser::status();
    strings::ling_str_new(s.as_ptr(), s.len())
}

/// Dump the loaded page to the console, lynx-style -- `bring --browse`.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_browser_dump() -> u64 {
    browser::dump_to_console();
    0
}

// -- Display modes (drivers::display) -------------------------------------

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_display_count() -> u64 {
    display::mode_count() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_display_preferred() -> u64 {
    display::preferred() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_display_set(idx: u64) -> u64 {
    display::set_preferred(idx as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_display_name(idx: u64) -> u64 {
    let n = display::mode_label(idx as usize);
    strings::ling_str_new(n.as_ptr(), n.len())
}

/// Read a lingfs file of any size (manifest-aware) and save `data` HTTP
/// body to it -- the `curl`/`bring --out` sink for large downloads.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_http_download(url: u64, out_name: u64) -> u64 {
    let Some((host, port, path, tls)) = netstack::parse_url(arg_str(url)) else {
        return 0;
    };
    if tls {
        return 2; // https refused -- caller prints the reason
    }
    let Some(ip) = netstack::dns_resolve(host) else {
        return 3; // DNS failure
    };
    static mut DLBUF: [u8; 2 * 1024 * 1024] = [0; 2 * 1024 * 1024];
    let buf = &mut *&raw mut DLBUF;
    let Some(len) = netstack::http_get(ip, port, path, host, buf) else {
        return 0;
    };
    if fs::lingfs::write_file_any(arg_str(out_name), &buf[..len]).is_err() {
        return 4; // stored fetch but couldn't write (too big / fs error)
    }
    1
}

/// Fetch `url` and print its body straight to the console -- `curl` with
/// no `--out`. Returns 1 on success.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_http_print(url: u64) -> u64 {
    let Some((host, port, path, tls)) = netstack::parse_url(arg_str(url)) else {
        return 0;
    };
    if tls {
        return 2;
    }
    let Some(ip) = netstack::dns_resolve(host) else {
        return 3;
    };
    static mut PRBUF: [u8; 256 * 1024] = [0; 256 * 1024];
    let buf = &mut *&raw mut PRBUF;
    let Some(len) = netstack::http_get(ip, port, path, host, buf) else {
        return 0;
    };
    console_write(&buf[..len]);
    console_write(b"\n");
    1
}

// -- Audio: AC'97 + the OS mixer (drivers::{ac97, mixer}) ----------------
// Windows-audio-shaped: per-app shared-mode streams with their own
// volumes, one optional exclusive-mode takeover, a pentatonic synth for
// jingles/sound themes, and the tape player's transport. See mixer.rs's
// module doc for the full design rationale.

/// Find and bring up the AC'97 device. 1 if present, 0 if not (audio
/// calls all no-op safely in that case).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_init() -> u64 {
    mixer::init() as u64
}

/// Keep the DMA ring fed -- call once per frame / shell-loop pass.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_pump() -> u64 {
    mixer::pump();
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_master() -> u64 {
    mixer::master_volume() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_set_master(v: u64) -> u64 {
    mixer::set_master_volume(v as u32);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_stream_count() -> u64 {
    mixer::stream_count() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_stream_name(i: u64) -> u64 {
    let n = mixer::stream_name(i as usize);
    strings::ling_str_new(n.as_ptr(), n.len())
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_stream_volume(i: u64) -> u64 {
    mixer::stream_volume(i as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_set_stream_volume(i: u64, v: u64) -> u64 {
    mixer::set_stream_volume(i as usize, v as u32);
    0
}

/// ASIO-style exclusive takeover for stream `i`; 1 on success, 0 if
/// another stream already holds it.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_exclusive_claim(i: u64) -> u64 {
    mixer::exclusive_claim(i as usize) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_exclusive_release(i: u64) -> u64 {
    mixer::exclusive_release(i as usize);
    0
}

/// Stream index currently holding exclusive mode, or a large sentinel
/// (u64::MAX) when shared -- flat-u64 ABI has no -1.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_exclusive_holder() -> u64 {
    let h = mixer::exclusive_holder();
    if h < 0 {
        u64::MAX
    } else {
        h as u64
    }
}

/// Fire a UI sound event (mixer::EVENT_*) through the active sound theme.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_jingle(event: u64) -> u64 {
    mixer::jingle(event as usize);
    0
}

/// Play one raw synth note on a stream: frequency in centihertz
/// (Hz * 100), duration shaping the decay envelope.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_audio_note(stream: u64, centihz: u64, dur_ms: u64) -> u64 {
    mixer::note(stream as usize, centihz as u32, dur_ms as u32, 0, false);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_sound_theme_count() -> u64 {
    mixer::sound_theme_count() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_sound_theme_current() -> u64 {
    mixer::sound_theme() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_sound_theme_select(i: u64) -> u64 {
    mixer::set_sound_theme(i as usize);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_sound_theme_name(i: u64) -> u64 {
    let n = mixer::sound_theme_name(i as usize);
    strings::ling_str_new(n.as_ptr(), n.len())
}

// Tape player transport (the `play` command's engine).

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_player_load(seed: u64, len_ms: u64) -> u64 {
    mixer::player_load(seed, len_ms as u32);
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_player_play() -> u64 {
    mixer::player_play();
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_player_pause() -> u64 {
    mixer::player_pause();
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_player_stop() -> u64 {
    mixer::player_stop();
    0
}

/// 0 stopped, 1 playing, 2 paused.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_player_state() -> u64 {
    mixer::player_state() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_player_pos_ms() -> u64 {
    mixer::player_pos_ms()
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_player_len_ms() -> u64 {
    mixer::player_len_ms()
}

/// One-line ASCII tape transport (`[>] |####----| 00:12 / 01:00`) --
/// built kernel-side because a variable-length bar is a loop `.ling`
/// can't hold. Redraw with a leading `\r` for the in-place effect.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_player_line() -> u64 {
    let s = mixer::player_line();
    strings::ling_str_new(s.as_ptr(), s.len())
}

/// Top-bar tray (volume + network icons, volume popover when open).
/// `net_up` is the real `ling_kernel_net_init` result.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_wm_draw_tray(net_up: u64) -> u64 {
    wm::draw_tray(net_up != 0);
    0
}

// -- lingfu network client (drivers::{netstack, lingfu}) -- catalog sync,
// search, download, install over real HTTP. --------------------------------

/// Fetch the package catalog from the configured repo (lingfs `/repo`,
/// default 10.0.2.2:8000 -- QEMU SLIRP's host alias). Returns the number
/// of catalog entries; 0 = failure (reasons printed).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_lingfu_sync() -> u64 {
    lingfu::sync() as u64
}

/// Print catalog entries containing `query` (empty string = all).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_lingfu_query(query: u64) -> u64 {
    lingfu::list(arg_str(query)) as u64
}

/// Download + install one catalog package by name; 1 on success.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_lingfu_fetch_install(name: u64) -> u64 {
    lingfu::install(arg_str(name)) as u64
}

/// 1 when this kernel was loaded by the installed-disk boot path
/// (stage1/stage2, no GRUB/Multiboot2 info) -- the desktop's cue to show
/// the login greeter instead of the Live auto-login flow.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_boot_is_disk() -> u64 {
    framebuffer::booted_from_disk() as u64
}

/// Name of the detected boot-target block device ("ahci0" / "ata0") --
/// the installer's disk-selection list. One device today, honestly: the
/// block layer drives AHCI port 0 or the ATA primary master, so the list
/// has one real entry, not an invented menu.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_disk_name() -> u64 {
    let n = fs::blockdev::active_driver_name();
    strings::ling_str_new(n.as_ptr(), n.len())
}

/// Erase the target disk's boot-critical regions for a clean install:
/// zeros the MBR/stage1, stage2 area, kernel header sector, and the
/// lingfs superblock sectors. NOT a full-surface wipe (that claim would
/// need minutes of I/O this returns too fast to have done) -- it makes
/// the disk genuinely non-booting and unformatted, which is what "erase
/// before install" needs. Returns 1 on success.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_disk_erase_quick() -> u64 {
    let zero = [0u8; fs::blockdev::SECTOR_SIZE];
    // LBA 0 (stage1/MBR), 1..16 (stage2), 17 (kernel header).
    for lba in 0..=17u32 {
        if fs::blockdev::write_sector(lba, &zero).is_err() {
            return 0;
        }
    }
    // lingfs superblocks: double-buffered at the base LBA (see
    // lingfs.rs's LINGFS_BASE_LBA reservation comment in packages/README).
    for lba in 8192..8200u32 {
        if fs::blockdev::write_sector(lba, &zero).is_err() {
            return 0;
        }
    }
    1
}

/// Install a `.lpkg` blob already stored at `blob_name` in lingfs (see
/// `packages.rs` for the format and its honest limits — no network fetch,
/// no signing, no way to execute the result). Returns 1 on success, 0 on a
/// malformed package or missing source blob.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_pkg_install(blob_name: u64) -> u64 {
    packages::install(arg_str(blob_name)) as u64
}

/// Install a `.lpkg` directly from the first Multiboot2 module (see
/// `bootmodule.rs` — the same mechanism `live/grub.cfg`'s `module2` uses to
/// hand the installer the disk-boot payload) — the realistic way package
/// bytes get onto a system with no network stack. Returns 1 on success, 0
/// if there's no module or it's malformed. Only usable once per boot in
/// practice today: there's only ever one module loaded (the disk-boot
/// payload on the Install entry), so this is here to prove the mechanism
/// works, not for everyday use yet.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_pkg_install_module() -> u64 {
    let Some((start, end)) = bootmodule::first_module() else { return 0 };
    packages::install_from_ptr(start as *const u8, (end - start) as usize) as u64
}

/// Run the `ling-life` screensaver (see `life.rs`) until a key is pressed
/// or the mouse moves/clicks, then return.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_life_run() -> u64 {
    life::run();
    0
}

/// No-op: `ling_kernel_init()` already runs the PS/2 mouse's synchronous
/// 8042 handshake itself, before IRQ12 is unmasked (see `mouse::init`'s doc
/// comment for why that ordering matters). Calling this afterward — as
/// existing `.ling` WM code does — used to repeat that same handshake with
/// IRQ12 already live, racing the real IRQ12 handler for the controller's
/// ACK/status bytes and hanging forever in `wait_output_full()` (confirmed:
/// boot got through `ling_kernel_init()` fine, then never rendered a single
/// frame). Kept as a real ABI entry point, not removed, for the same reason
/// as `ling_kernel_mouse_poll` just above.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_mouse_init() -> u64 {
    0
}

/// Clamp future tracked positions to `0..w` / `0..h` — set this to the
/// framebuffer's actual width/height (see `ling_kernel_fb_width/height`).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_mouse_set_bounds(w: u64, h: u64) -> u64 {
    mouse::set_bounds(w as i32, h as i32);
    0
}

/// No-op: mouse position/buttons are now updated directly by the IRQ12
/// handler (`arch::x86_64::idt`), so there is nothing left to drain. Kept as
/// a real ABI entry point (not removed) because it's part of the frozen
/// `ling_kernel_*` intrinsic surface `.ling` kernel source calls by name —
/// existing/compiled callers that still call it between reading
/// `ling_kernel_mouse_x/y/buttons` keep working, just reading already-fresh
/// state instead of triggering a drain.
/// Drain any bytes the 8042 is holding (routed keyboard/mouse by the
/// status register's origin bit) with interrupts briefly masked. No longer
/// a pure no-op: QEMU's 8042 was observed parking aux data with OBF high
/// and never raising IRQ12 (see `arch::x86_64::idt::poll_drain_8042`'s
/// doc), so the WM's once-per-frame call to this is what guarantees input
/// progress everywhere; the IRQ path remains for low-latency delivery on
/// hardware that does assert the line.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_mouse_poll() -> u64 {
    arch::idt::poll_drain_8042();
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_mouse_x() -> u64 {
    mouse::x() as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_mouse_y() -> u64 {
    mouse::y() as u64
}

/// Bit 0 = left button, bit 1 = right, bit 2 = middle.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_mouse_buttons() -> u64 {
    mouse::buttons() as u64
}

/// Physical address of the first Multiboot2 module GRUB loaded (see
/// `live/grub.cfg`'s `module2` line), or 0 if there isn't one — the
/// installer's way of getting the disk-boot payload (`diskboot.img`) into
/// memory without an ISO9660 driver.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_bootmodule_addr() -> u64 {
    bootmodule::first_module()
        .map(|(s, _)| s as u64)
        .unwrap_or(0)
}

/// Size in bytes of the first Multiboot2 module, or 0 if there isn't one.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_bootmodule_size() -> u64 {
    bootmodule::first_module()
        .map(|(s, e)| (e - s) as u64)
        .unwrap_or(0)
}

#[cfg(target_arch = "x86_64")]
unsafe fn arg_str(v: u64) -> &'static str {
    if v == 0 {
        ""
    } else {
        core::str::from_utf8(strings::bytes_of_ptr(v as *mut u8)).unwrap_or("")
    }
}

/// Create or update a user account. Each field is its own argument (not
/// one delimited string) so the shell can gather them via separate
/// prompts — password entry through `ling_kernel_read_line_masked`, which
/// doesn't echo or record history. Returns 1 on success, 0 on an empty
/// username or a lingfs write failure.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_user_create(
    username: u64,
    password: u64,
    first: u64,
    last: u64,
    phone: u64,
    email: u64,
    group: u64,
) -> u64 {
    users::create(
        arg_str(username),
        arg_str(password),
        arg_str(first),
        arg_str(last),
        arg_str(phone),
        arg_str(email),
        arg_str(group),
    ) as u64
}

/// On a match, this user becomes who new lingfs writes are attributed to
/// (`lingfs::set_current_user`) — there's no privilege/session model yet,
/// so this doesn't gate access to anything, it only changes ownership
/// attribution going forward. Returns 1 on a match, 0 otherwise (wrong
/// password or no such user).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_user_login(username: u64, password: u64) -> u64 {
    let (username, password) = (arg_str(username), arg_str(password));
    if !users::verify(username, password) {
        return 0;
    }
    let mut group_buf = [0u8; 32];
    let group_len = users::group_of(username, &mut group_buf).unwrap_or(0);
    let group = core::str::from_utf8(&group_buf[..group_len]).unwrap_or("users");
    lingfs::set_current_user(username, group);
    1
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_user_set_password(username: u64, new_password: u64) -> u64 {
    users::set_password(arg_str(username), arg_str(new_password)) as u64
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_user_set_group(username: u64, new_group: u64) -> u64 {
    users::set_group(arg_str(username), arg_str(new_group)) as u64
}

/// The user new lingfs writes are currently attributed to (`"root"` until
/// a successful `ling_kernel_user_login`).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_whoami() -> u64 {
    let name = lingfs::current_user();
    strings::ling_str_new(name.as_ptr(), name.len())
}

/// The currently logged-in user's group, as an LSTR (empty if unset).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_user_group() -> u64 {
    let name = lingfs::current_user();
    let mut buf = [0u8; 32];
    let len = users::group_of(name, &mut buf).unwrap_or(0);
    strings::ling_str_new(buf.as_ptr(), len)
}

/// Whether the currently logged-in user is in the `wheel` group — the
/// `sudo` gate's check. See `fs/users.rs`'s module doc for exactly what
/// this can and can't mean (a shell-level gate, not a kernel security
/// boundary — this kernel has no ring-3/per-process isolation at all).
///
/// `"root"` (the implicit identity before any real `login`/`useradd` has
/// ever happened — `lingfs::current_user()`'s own default) is always
/// treated as wheel, same as real Unix root bypassing group checks —
/// otherwise a fresh install has no way to ever create its first user:
/// `useradd` itself requires wheel, and no lingfs user record (let alone a
/// wheel one) exists until something has already run `useradd` once.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_user_is_wheel() -> u64 {
    let name = lingfs::current_user();
    if name == "root" {
        return 1;
    }
    let mut buf = [0u8; 32];
    let len = users::group_of(name, &mut buf).unwrap_or(0);
    (len == 5 && &buf[..5] == b"wheel") as u64
}

/// `ed25519_verify(pubkey, msg, sig) -> 1|0`. All three arguments are raw
/// Ling byte-strings (binary-safe, not `arg_str`'s UTF-8-only reading) —
/// `pubkey` must be exactly 32 bytes and `sig` exactly 64, anything else
/// returns 0 rather than trapping. No signing/keygen counterpart yet; see
/// `ed25519.rs`'s module doc for why.
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_ed25519_verify(pubkey: u64, msg: u64, sig: u64) -> u64 {
    let raw = |v: u64| -> &'static [u8] {
        if v == 0 {
            &[]
        } else {
            strings::bytes_of_ptr(v as *mut u8)
        }
    };
    ed25519::verify(raw(pubkey), raw(msg), raw(sig)) as u64
}

/// Change the shell's current directory — `""`/`"/"`/`".."` go back to
/// root, anything else must name an existing one-level lingfs directory
/// (lingfs is one level deep today). Returns 1 on success, 0 if `dir`
/// doesn't exist.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_cd(dir: u64) -> u64 {
    lingfs::set_cwd(arg_str(dir)) as u64
}

/// The current directory (`""` for root), for the shell prompt.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_cwd() -> u64 {
    let dir = lingfs::cwd();
    strings::ling_str_new(dir.as_ptr(), dir.len())
}

/// Write `count` 512-byte sectors from `src_ptr` (a raw physical pointer —
/// e.g. a Multiboot2 module's address, already de-tagged the same way any
/// kernel-call argument is) to disk starting at `lba`. Bypasses lingfs
/// entirely: for writing the bootloader payload into LBA 0..8191, the one
/// region `lingfs::LINGFS_BASE_LBA` reserves and lingfs itself never
/// touches. Returns 1 on success, 0 on the first failed sector write.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_disk_write_raw(lba: u64, src_ptr: u64, count: u64) -> u64 {
    let src = src_ptr as *const u8;
    for i in 0..count {
        let mut sector = [0u8; blockdev::SECTOR_SIZE];
        let chunk = core::slice::from_raw_parts(
            src.add(i as usize * blockdev::SECTOR_SIZE),
            blockdev::SECTOR_SIZE,
        );
        sector.copy_from_slice(chunk);
        if blockdev::write_sector((lba + i) as u32, &sector).is_err() {
            return 0;
        }
    }
    1
}

#[cfg(target_arch = "x86_64")]
const MAX_LINE: usize = 256;

/// Erase `n` already-echoed characters from the current input line —
/// backspace `write_char_active` already knows how to blank a cell, this
/// just repeats it, used when arrow-key history recall replaces the whole
/// line being typed.
#[cfg(target_arch = "x86_64")]
fn erase_chars(n: usize) {
    for _ in 0..n {
        console_write(&[0x08]);
    }
}

/// Blocking line read with echo, backspace, and Up/Down arrow-key history
/// recall (bash/ssh-style — see `history.rs`), returned as a real
/// (heap-allocated, via the bump allocator) Ling string — the shell's
/// input primitive. Enter/Return ends the line without including it in
/// the returned string, matching a normal line-editor's behavior, and
/// pushes it into history (unless empty).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_read_line() -> u64 {
    let mut buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    let mut hist_back = 0usize; // 0 = not currently browsing history
    loop {
        let c = keyboard::read_char();
        if c == b'\n' || c == b'\r' {
            console_write(b"\n");
            break;
        } else if c == 0x03 {
            // Ctrl+C: abort the line being typed, same as pressing Enter on
            // an empty prompt. There's no preemptible command execution in
            // this kernel yet (every shell command runs to completion before
            // the next `read_line`), so this only ever has something to
            // cancel while a line is still being typed.
            console_write(b"^C\n");
            len = 0;
            break;
        } else if c == keyboard::UP_ARROW {
            if let Some(entry) = history::get(hist_back + 1) {
                erase_chars(len);
                len = entry.len().min(MAX_LINE);
                buf[..len].copy_from_slice(&entry[..len]);
                console_write(&buf[..len]);
                hist_back += 1;
            }
        } else if c == keyboard::DOWN_ARROW {
            if hist_back > 0 {
                erase_chars(len);
                hist_back -= 1;
                if hist_back == 0 {
                    len = 0;
                } else if let Some(entry) = history::get(hist_back) {
                    len = entry.len().min(MAX_LINE);
                    buf[..len].copy_from_slice(&entry[..len]);
                    console_write(&buf[..len]);
                }
            }
        } else if c == 0x08 {
            if len > 0 {
                len -= 1;
                console_write(&[0x08]);
            }
        } else if len < MAX_LINE {
            buf[len] = c;
            len += 1;
            console_write(&[c]);
        }
    }
    if len > 0 {
        history::push(&buf[..len]);
    }
    strings::ling_str_new(buf.as_ptr(), len)
}

/// Blocking line read for graphics-mode UI (the WM, the graphics installer
/// front-end): same shape as `ling_kernel_read_line`, but echoes into the
/// framebuffer's back buffer at `(x, y)` via `font_unicode::draw_utf8_str`
/// instead of the VGA text console (which isn't the visible surface once a
/// linear framebuffer is in use — see `ling_kernel_init`'s doc for why
/// those are mutually exclusive on this kernel). Simplified relative to
/// `read_line`: no history recall (Up/Down arrows), since a one-off
/// hostname/setting field doesn't need it. Redraws the whole typed string
/// on every keystroke rather than tracking per-character pixel widths
/// incrementally -- simpler and correct for the short strings this is for,
/// at the cost of a full-line repaint per key.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_gui_read_line(x: u64, y: u64, fg: u64, bg: u64) -> u64 {
    let mut buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    loop {
        let c = keyboard::read_char();
        if c == b'\n' || c == b'\r' {
            break;
        } else if c == 0x03 {
            len = 0;
            break;
        } else if c == 0x08 {
            if len > 0 {
                len -= 1;
            }
        } else if len < MAX_LINE && c >= 0x20 && c < 0x7F {
            buf[len] = c;
            len += 1;
        } else {
            continue;
        }
        // Blank a fixed-width field before redrawing -- MAX_LINE chars at
        // font8x8's 8px advance would overflow any reasonable screen, but
        // this field is for short strings (hostnames, package names); clear
        // enough width for ~40 characters, comfortably more than needed.
        framebuffer::back_fill_rect(x as u32, y as u32, 40 * 8, 16, bg as u32);
        font_unicode::draw_utf8_str(x as u32, y as u32, &buf[..len], fg as u32, bg as u32, false);
        framebuffer::present();
    }
    strings::ling_str_new(buf.as_ptr(), len)
}

/// Shared by `ling_kernel_gui_read_line_masked` and the confirmed-password
/// variant below -- reads one masked line at `(x, y)`, returning the raw
/// bytes and length rather than a boxed Ling string, so the confirmed
/// variant can compare two reads without round-tripping through the string
/// runtime.
#[cfg(target_arch = "x86_64")]
unsafe fn gui_read_masked_line_at(x: u32, y: u32, fg: u32, bg: u32) -> ([u8; MAX_LINE], usize) {
    let mut buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    let stars = [b'*'; MAX_LINE];
    loop {
        let c = keyboard::read_char();
        if c == b'\n' || c == b'\r' {
            break;
        } else if c == 0x03 {
            len = 0;
            break;
        } else if c == 0x08 {
            if len > 0 {
                len -= 1;
            }
        } else if len < MAX_LINE && c >= 0x20 && c < 0x7F {
            buf[len] = c;
            len += 1;
        } else {
            continue;
        }
        framebuffer::back_fill_rect(x, y, 40 * 8, 16, bg);
        font_unicode::draw_utf8_str(x, y, &stars[..len], fg, bg, false);
        framebuffer::present();
    }
    (buf, len)
}

/// Like `ling_kernel_gui_read_line`, for password entry: echoes `*` instead
/// of the typed character. No masking-strength claims beyond "not shown on
/// screen" -- same real limitation as the text-mode version.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_gui_read_line_masked(x: u64, y: u64, fg: u64, bg: u64) -> u64 {
    let (buf, len) = gui_read_masked_line_at(x as u32, y as u32, fg as u32, bg as u32);
    strings::ling_str_new(buf.as_ptr(), len)
}

/// Read a password twice (input fields at `y1`/`y2`, an error line at
/// `err_y`) and retry -- internally, in a real loop -- until the two
/// entries match, rather than making the caller re-implement "type it
/// twice, check they're equal" with `.ling`'s own inability to hold a
/// retry loop's state across a rejected attempt. Labels ("Password:",
/// "Confirm password:") are the caller's job to draw once, since they
/// don't change between retries; this only owns the two input fields and
/// the error line.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_gui_read_password_confirmed(
    x: u64,
    y1: u64,
    y2: u64,
    err_y: u64,
    fg: u64,
    bg: u64,
    err_fg: u64,
) -> u64 {
    let (x, y1, y2, err_y) = (x as u32, y1 as u32, y2 as u32, err_y as u32);
    let (fg, bg, err_fg) = (fg as u32, bg as u32, err_fg as u32);
    loop {
        framebuffer::back_fill_rect(x, err_y, 600, 16, bg);
        framebuffer::present();
        let (pw1, len1) = gui_read_masked_line_at(x, y1, fg, bg);
        let (pw2, len2) = gui_read_masked_line_at(x, y2, fg, bg);
        if len1 == len2 && pw1[..len1] == pw2[..len2] {
            return strings::ling_str_new(pw1.as_ptr(), len1);
        }
        framebuffer::back_fill_rect(x, y1, 40 * 8, 16, bg);
        framebuffer::back_fill_rect(x, y2, 40 * 8, 16, bg);
        font_unicode::draw_utf8_str(
            x,
            err_y,
            b"Passwords didn't match -- try again.",
            err_fg,
            bg,
            false,
        );
        framebuffer::present();
    }
}

/// Shared by `ling_kernel_read_line_masked` and the confirmed-password
/// variant below -- see `gui_read_masked_line_at`'s doc for why this
/// returns raw bytes rather than a boxed Ling string.
#[cfg(target_arch = "x86_64")]
unsafe fn read_masked_line() -> ([u8; MAX_LINE], usize) {
    let mut buf = [0u8; MAX_LINE];
    let mut len = 0usize;
    loop {
        let c = keyboard::read_char();
        if c == b'\n' || c == b'\r' {
            console_write(b"\n");
            break;
        } else if c == 0x03 {
            console_write(b"^C\n");
            len = 0;
            break;
        } else if c == 0x08 {
            if len > 0 {
                len -= 1;
                console_write(&[0x08]);
            }
        } else if c == keyboard::UP_ARROW || c == keyboard::DOWN_ARROW {
            // no history recall for password entry
        } else if len < MAX_LINE {
            buf[len] = c;
            len += 1;
            console_write(b"*");
        }
    }
    (buf, len)
}

/// Like `ling_kernel_read_line`, for password entry: echoes `*` instead of
/// the typed character, ignores Up/Down (no history recall — a typed
/// password should never be recoverable that way), and never pushes the
/// result into `history`.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_read_line_masked() -> u64 {
    let (buf, len) = read_masked_line();
    strings::ling_str_new(buf.as_ptr(), len)
}

/// Text-mode counterpart to `ling_kernel_gui_read_password_confirmed`:
/// prompts "Password: "/"Confirm password: " itself (unlike the graphics
/// version, VGA text has no fixed-position field to redraw in place, so
/// this reprints the whole exchange on a retry rather than taking caller-
/// supplied coordinates), reads twice, retries until they match.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_read_line_masked_confirmed() -> u64 {
    loop {
        console_write(b"Password: ");
        let (pw1, len1) = read_masked_line();
        console_write(b"Confirm password: ");
        let (pw2, len2) = read_masked_line();
        if len1 == len2 && pw1[..len1] == pw2[..len2] {
            return strings::ling_str_new(pw1.as_ptr(), len1);
        }
        console_write(b"Passwords didn't match -- try again.\n");
    }
}

/// Print a heap-allocated Ling string (from a literal, concatenation,
/// `ling_kernel_read_line`, ...). NOT the same calling convention as
/// `ling_kernel_vga_write_str`, which expects a raw NUL-terminated
/// pointer — what a bare string-literal argument compiles to, not what a
/// real (length-prefixed, no NUL guarantee) `ling_str_*` value is. Mixing
/// the two up is a real footgun the names are meant to head off.
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_print_lstr(s: u64) -> u64 {
    // `s` arrives already de-tagged (see `dynamic_to_raw` in ling-codegen)
    // — a raw pointer to the string's length-prefixed buffer, not the
    // NaN-boxed value `strings::bytes_of` expects.
    let bytes = strings::bytes_of_ptr(s as *mut u8);
    console_write(bytes);
    0
}

/// Blocking read of a single ASCII character from the console input (PS/2
/// keyboard on x86_64, PL011 UART RX on aarch64).
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_read_char() -> u64 {
    #[cfg(target_arch = "x86_64")]
    return keyboard::read_char() as u64;
    #[cfg(target_arch = "aarch64")]
    return uart::read_byte() as u64;
}

/// Non-blocking poll of console input; returns 0 if nothing is waiting.
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_kbd_poll_char() -> u64 {
    #[cfg(target_arch = "x86_64")]
    return keyboard::poll_char() as u64;
    #[cfg(target_arch = "aarch64")]
    return uart::poll_byte().unwrap_or(0) as u64;
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_panic(msg: u64) -> u64 {
    if msg != 0 {
        let ptr = msg as *const u8;
        let mut i = 0;
        loop {
            let c = ptr::read_volatile(ptr.add(i));
            if c == 0 {
                break;
            }
            i += 1;
        }
        let msg_str = core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr, i));
        kernel_panic(msg_str);
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_init() -> u64 {
    init();
    0
}

#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_cpuid(eax: u64, ecx: u64, out_ptr: u64, _unused: u64) -> u64 {
    let (a, b, c, d) = cpu::cpuid(eax as u32, ecx as u32);
    if out_ptr != 0 {
        let out = out_ptr as *mut u32;
        ptr::write_volatile(out, a);
        ptr::write_volatile(out.add(1), b);
        ptr::write_volatile(out.add(2), c);
        ptr::write_volatile(out.add(3), d);
    }
    0
}

#[no_mangle]
pub unsafe extern "C" fn ling_kernel_asm_exec(ptr: u64, len: u64) -> u64 {
    let template =
        core::str::from_utf8_unchecked(core::slice::from_raw_parts(ptr as *const u8, len as usize));
    match template {
        #[cfg(target_arch = "x86_64")]
        "hlt" | "halt" => loop {
            cpu::halt();
        },
        #[cfg(target_arch = "aarch64")]
        "wfi" | "halt" => loop {
            cpu::halt();
        },
        #[cfg(target_arch = "aarch64")]
        "wfe" => {
            cpu::wfe();
            0
        },
        "cli" => {
            cpu::cli();
            0
        },
        "sti" => {
            cpu::sti();
            0
        },
        "nop" => {
            core::hint::spin_loop();
            0
        },
        "pause" => {
            core::hint::spin_loop();
            0
        },
        #[cfg(target_arch = "x86_64")]
        "int3" => {
            core::arch::asm!("int3");
            0
        },
        #[cfg(target_arch = "aarch64")]
        "brk" => {
            cpu::brk();
            0
        },
        _ => {
            console_write(b"ling_kernel_asm_exec: unknown template: ");
            console_write(template.as_bytes());
            console_write(b"\n");
            loop {
                cpu::halt();
            }
        },
    }
}

/// Bring up the e1000 NIC (see `drivers::net_e1000`'s module doc). Returns
/// 1 if a NIC was found and initialized, 0 if none is present (a normal
/// outcome, not an error -- not every machine has one this driver covers).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_net_init() -> u64 {
    let Ok(()) = net_e1000::init() else {
        serial::write(b"net: no e1000-compatible NIC found\n");
        return 0;
    };
    let m = net_e1000::mac();
    serial::write(b"net: MAC=");
    for (i, b) in m.iter().enumerate() {
        if i > 0 {
            serial::write(b":");
        }
        print_hex_byte(*b);
    }
    serial::write(b" link_up=");
    print_decimal(net_e1000::link_up() as u64);
    serial::write(b"\n");
    // Learn our IP/mask/gateway/DNS from the network we're actually on. This
    // is what makes DNS resolve on a VirtualBox bridged/host-only adapter,
    // where the SLIRP-convention static fallbacks are wrong. Bounded and
    // best-effort: on failure the static fallback stays and plain QEMU still
    // works.
    if netstack::dhcp_configure() {
        let ip = net_e1000::self_ip();
        let gw = net_e1000::gateway_ip();
        let (d1, _) = netstack::dns_servers();
        serial::write(b"net: DHCP lease ip=");
        for (i, b) in ip.iter().enumerate() {
            if i > 0 {
                serial::write(b".");
            }
            print_decimal(*b as u64);
        }
        serial::write(b" gw=");
        for (i, b) in gw.iter().enumerate() {
            if i > 0 {
                serial::write(b".");
            }
            print_decimal(*b as u64);
        }
        serial::write(b" dns=");
        for (i, b) in d1.iter().enumerate() {
            if i > 0 {
                serial::write(b".");
            }
            print_decimal(*b as u64);
        }
        serial::write(b"\n");
    } else {
        serial::write(b"net: DHCP unavailable, using static SLIRP fallback\n");
    }
    1
}
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_net_link_up() -> u64 {
    net_e1000::link_up() as u64
}
/// One byte (0..5) of the NIC's real MAC address.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_net_mac_byte(i: u64) -> u64 {
    net_e1000::mac().get(i as usize).copied().unwrap_or(0) as u64
}
/// One byte (0..3) of our current IPv4 address -- the DHCP lease if one was
/// obtained, else the static SLIRP fallback.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_net_self_ip_byte(i: u64) -> u64 {
    net_e1000::self_ip().get(i as usize).copied().unwrap_or(0) as u64
}
/// One byte (0..3) of our current default gateway -- the DHCP router option
/// if leased, else the static SLIRP fallback.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_net_gateway_ip_byte(i: u64) -> u64 {
    net_e1000::gateway_ip().get(i as usize).copied().unwrap_or(0) as u64
}
/// Hand-built ARP request/reply round trip, bypassing any IP stack --
/// see `net_e1000::arp_selftest`'s doc. Returns 1 and fills `out_mac_byte`-
/// queryable state on a real reply, 0 on timeout/no NIC.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_net_arp_selftest() -> u64 {
    match net_e1000::arp_selftest() {
        Some(mac) => {
            ARP_REPLY_MAC = mac;
            serial::write(b"net: ARP reply from ");
            for (i, b) in mac.iter().enumerate() {
                if i > 0 {
                    serial::write(b":");
                }
                print_hex_byte(*b);
            }
            serial::write(b"\n");
            1
        }
        None => {
            serial::write(b"net: ARP selftest: no reply\n");
            0
        }
    }
}
static mut ARP_REPLY_MAC: [u8; 6] = [0; 6];
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_net_arp_reply_mac_byte(i: u64) -> u64 {
    ARP_REPLY_MAC.get(i as usize).copied().unwrap_or(0) as u64
}
