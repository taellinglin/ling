//! Remote control of the desktop: the SSH shell (or the console) queues a command and the desktop's
//! frame loop takes it with `os_remote_poll` and does it -- open an app, close one, lock the screen.
//! One-way and tiny on purpose: the desktop decides what a command means, and nothing here runs code.
//!
//! Commands are `verb:argument` strings: `open:<app>`, `close:<app>`, `lock:`. The queue holds a few;
//! `desktop_is_polling` tells the shell whether a desktop is up to answer (it polls every frame).

use alloc::string::String;
use alloc::vec::Vec;

static mut QUEUE: Vec<String> = Vec::new();
static mut LAST_POLL_MS: u64 = 0;

const MAX_QUEUED: usize = 16;

fn queue() -> &'static mut Vec<String> {
    unsafe { &mut *(&raw mut QUEUE) }
}

/// Queue a command for the desktop. False when the queue is full or the text is not `verb:argument`.
pub fn push(cmd: &str) -> bool {
    if queue().len() >= MAX_QUEUED || !cmd.contains(':') || cmd.len() > 120 || cmd.bytes().any(|b| b < 0x20) {
        return false;
    }
    queue().push(String::from(cmd));
    true
}

/// The oldest queued command, if any. Called by the desktop's frame loop; that is also how the
/// shell knows a desktop is up.
pub fn poll() -> Option<String> {
    unsafe { LAST_POLL_MS = crate::arch::timer::now_ms().max(1) };
    if queue().is_empty() {
        None
    } else {
        Some(queue().remove(0))
    }
}

/// Has a desktop asked for commands lately (within three seconds)?
pub fn desktop_is_polling() -> bool {
    let last = unsafe { LAST_POLL_MS };
    last != 0 && crate::arch::timer::now_ms().saturating_sub(last) < 3000
}
