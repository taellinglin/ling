//! Startup: boot services and the idle screen lock.
//!
//! * **Services** start when the desktop first draws, before anyone signs in,
//!   and run for as long as the machine is up. The SSH server is one (its
//!   switch lives in `/services`, see services.rs); any program built into
//!   the image can be another: it runs headless on a coroutine of its own
//!   (coro.rs, owner "system"), so its loop keeps going under the sign-in
//!   screen, under apps, and while the screen is locked. Which programs start
//!   is `/startup` in LingOS storage, one `service <name>` line each.
//! * **Sign-in apps** are the desktop's business (each user's list is in
//!   their home); the kernel only adds a way to start an app straight into
//!   the background: it runs until its first frame, then parks as if Alt+Tab
//!   had been pressed.
//! * **Idle lock**: every frame with input counts as activity. After the
//!   desktop's chosen timeout without any, an app that has the screen is
//!   parked (still running its isolates) and the desktop is asked to show
//!   the sign-in screen; services and parked apps go on.

use alloc::string::String;
use alloc::vec::Vec;

const STARTUP_PATH: &str = "/startup";

static mut BOOTED: bool = false;
static mut LAST_ACTIVE_MS: u64 = 0;
/// Lock after this many seconds without input (0 = never).
static mut LOCK_AFTER_S: u64 = 0;
static mut LOCK_REQUESTED: bool = false;
/// The next app started parks at its first frame.
static mut BG_START: bool = false;

fn read_list() -> Vec<String> {
    if !crate::fs::lingfs::is_mounted() {
        return Vec::new();
    }
    let mut buf = [0u8; 2048];
    match crate::fs::lingfs::read_file_all(STARTUP_PATH, &mut buf) {
        Ok(Some(n)) => core::str::from_utf8(&buf[..n])
            .unwrap_or("")
            .lines()
            .filter_map(|l| l.trim().strip_prefix("service "))
            .map(|s| String::from(s.trim()))
            .filter(|s| !s.is_empty())
            .collect(),
        _ => Vec::new(),
    }
}

fn write_list(list: &[String]) -> bool {
    let mut text = String::new();
    for s in list {
        text.push_str("service ");
        text.push_str(s);
        text.push('\n');
    }
    crate::fs::lingfs::write_file_any(STARTUP_PATH, text.as_bytes()).is_ok()
}

/// Every service that can be switched on: the SSH server, then each program
/// in the image.
pub fn available() -> Vec<String> {
    let mut v = alloc::vec![String::from("ssh")];
    for n in crate::app_names() {
        v.push(String::from(n));
    }
    v
}

pub fn enabled(name: &str) -> bool {
    if name == "ssh" {
        return ssh_at_boot();
    }
    read_list().iter().any(|s| s == name)
}

/// Is it running right now?
pub fn running(name: &str) -> bool {
    if name == "ssh" {
        return crate::drivers::sshd::enabled();
    }
    crate::coro::running(name, "system")
}

/// Switch a service on or off for the next boots; switching on also starts
/// it now (off leaves a running one alone until the next boot).
pub fn set(name: &str, on: bool) -> bool {
    if name == "ssh" {
        set_ssh_at_boot(on);
        crate::drivers::sshd::set_enabled(on);
        return true;
    }
    if crate::app_named(name.as_bytes()).is_none() {
        return false;
    }
    let mut l = read_list();
    l.retain(|s| s != name);
    if on {
        l.push(String::from(name));
    }
    let ok = write_list(&l);
    if on && !running(name) {
        start(name);
    }
    ok
}

/// Start one program as a service (headless, owner "system").
fn start(name: &str) -> bool {
    let Some(entry) = crate::app_named(name.as_bytes()) else { return false };
    let id = crate::coro::spawn_owned(
        name,
        true,
        "system",
        alloc::boxed::Box::new(move || unsafe {
            entry();
        }),
    );
    if id != 0 {
        crate::console_write(b"startup: service ");
        crate::console_write(name.as_bytes());
        crate::console_write(b" started\n");
    }
    id != 0
}

/// From the first `present()`: start the switched-on services once.
pub fn boot_once() {
    unsafe {
        if BOOTED {
            return;
        }
        BOOTED = true;
        LAST_ACTIVE_MS = crate::arch::timer::now_ms();
    }
    for name in read_list() {
        if !running(&name) {
            start(&name);
        }
    }
}

/// The SSH server's boot switch (x86: lingfs `/services`; the Pi: setup.conf).
fn ssh_at_boot() -> bool {
    #[cfg(target_arch = "x86_64")]
    return crate::services::ssh_enabled();
    #[cfg(target_arch = "aarch64")]
    return crate::setup::saved("ssh") == "1";
}

fn set_ssh_at_boot(on: bool) {
    #[cfg(target_arch = "x86_64")]
    crate::services::set_ssh(on);
    #[cfg(target_arch = "aarch64")]
    crate::setup::set_saved("ssh", if on { "1" } else { "0" });
}

// ── Idle lock ────────────────────────────────────────────────────────────────

/// A frame had input.
pub fn touch() {
    unsafe { LAST_ACTIVE_MS = crate::arch::timer::now_ms() };
}

pub fn idle_secs() -> f64 {
    crate::arch::timer::now_ms().saturating_sub(unsafe { LAST_ACTIVE_MS }) as f64 / 1000.0
}

pub fn set_lock_after(secs: u64) {
    unsafe { LOCK_AFTER_S = secs };
}

pub fn lock_after() -> u64 {
    unsafe { LOCK_AFTER_S }
}

/// Has the idle time run out?
pub fn lock_due() -> bool {
    let after = lock_after();
    after > 0 && idle_secs() >= after as f64
}

pub fn request_lock() {
    unsafe { LOCK_REQUESTED = true };
}

/// The desktop asks (once) whether an app was parked for the idle lock.
pub fn take_lock_request() -> bool {
    unsafe { core::mem::replace(&mut *(&raw mut LOCK_REQUESTED), false) }
}

// ── Starting an app in the background ────────────────────────────────────────

pub fn set_bg_start(on: bool) {
    unsafe { BG_START = on };
}

pub fn take_bg_start() -> bool {
    unsafe { core::mem::replace(&mut *(&raw mut BG_START), false) }
}
