#![no_std]
#![cfg_attr(target_arch = "x86_64", feature(abi_x86_interrupt))]

// ─── Per-architecture backend (CPU intrinsics, boot stub, port/MMIO I/O) ────
pub mod arch;

// ─── Device drivers (console, input, storage, framebuffer) ─────────────────
pub mod drivers;

// ─── lingfs and everything built on it (block device, users, packages) ─────
pub mod fs;

// ─── Task scheduling ─────────────────────────────────────────────────────
pub mod proc;

// ─── Physical memory management (frame allocator; paging/heap follow) ──────
pub mod mm;

// ─── Arch-neutral Ling runtime support: hashing, allocation, strings, sigs ──
pub mod ed25519;
pub mod hash;
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
use crate::drivers::{font8x8, framebuffer, keyboard, mouse, serial, term, vga, wm_liquid};
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
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_rtc_second() -> u64 {
    arch::rtc::read().second as u64
}

/// Spawn a task, yield to it, confirm it actually ran and yielded back.
/// Returns 1 on success, 0 on failure. Honest scope: proves cooperative
/// task creation/switching works, nothing about preemption or isolation
/// (this scheduler has neither — see `sched`'s module doc).
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_proc_selftest() -> u64 {
    if sched::selftest() {
        1
    } else {
        0
    }
}

/// Print each task slot's state to the console. Returns the count of
/// non-`Unused` slots.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_proc_ps() -> u64 {
    let mut count = 0u64;
    for i in 0..sched::MAX_TASKS {
        let Some(state) = sched::task_state(i) else { continue };
        let label: &[u8] = match state {
            sched::TaskStateInfo::Unused => b"unused",
            sched::TaskStateInfo::Ready => b"ready",
            sched::TaskStateInfo::Running => b"running",
            sched::TaskStateInfo::Exited => b"exited",
        };
        console_write(b"  slot ");
        print_decimal(i as u64);
        console_write(b": ");
        console_write(label);
        console_write(b"\n");
        if state != sched::TaskStateInfo::Unused {
            count += 1;
        }
    }
    count
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
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_mouse_poll() -> u64 {
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

/// Like `ling_kernel_read_line`, for password entry: echoes `*` instead of
/// the typed character, ignores Up/Down (no history recall — a typed
/// password should never be recoverable that way), and never pushes the
/// result into `history`.
#[cfg(target_arch = "x86_64")]
#[no_mangle]
pub unsafe extern "C" fn ling_kernel_read_line_masked() -> u64 {
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
    strings::ling_str_new(buf.as_ptr(), len)
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
