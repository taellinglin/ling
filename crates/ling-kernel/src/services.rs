//! Boot-time service configuration. Right now the only managed service is the
//! SSH server; the choice ("start sshd at boot?") is asked once on first boot
//! via a small graphical prompt and persisted to lingfs `/services`, then
//! honored (and reported) on every subsequent boot.
//!
//! Honest scope: this is the *service-management* half. A working sshd also
//! needs server-side TCP (accept/listen -- netstack is client-only today) and
//! the SSH transport itself (KEX/auth/channels -- a large protocol like TLS,
//! built on `crypto.rs`). So an "enabled" service here is configured and
//! announced at boot; the listener/protocol is the remaining work.

use crate::drivers::{font8x8, framebuffer, keyboard};
use crate::fs::lingfs;

// Absolute path: a bare "services" resolves relative to the current working
// directory (see lingfs::resolve), so a non-empty CWD at boot would write it
// somewhere the next read (under a different CWD) can't find it -- which
// silently dropped the SSH-at-boot choice. "/hostname" etc. already use the
// leading-slash (root) form for exactly this reason.
const SERVICES_PATH: &str = "/services";

fn read_services(buf: &mut [u8; lingfs::BLOCK_SIZE]) -> Option<usize> {
    lingfs::read_file(SERVICES_PATH, buf).ok().flatten()
}

/// Has the user been asked about services yet (does `/services` exist)?
pub fn is_configured() -> bool {
    let mut buf = [0u8; lingfs::BLOCK_SIZE];
    read_services(&mut buf).is_some()
}

/// Is the SSH server marked to start at boot?
pub fn ssh_enabled() -> bool {
    let mut buf = [0u8; lingfs::BLOCK_SIZE];
    if let Some(len) = read_services(&mut buf) {
        for line in buf[..len].split(|&b| b == b'\n') {
            let mut f = line.split(|&b| b == b' ' || b == b'\t').filter(|x| !x.is_empty());
            if let (Some(name), Some(val)) = (f.next(), f.next()) {
                if name == b"ssh" {
                    return val == b"1";
                }
            }
        }
    }
    false
}

/// Persist whether SSH should start at boot.
pub fn set_ssh(on: bool) {
    let line: &[u8] = if on { b"ssh 1\n" } else { b"ssh 0\n" };
    let _ = lingfs::write_file(SERVICES_PATH, line);
}

/// Run once at boot: on first boot (no `/services` yet) ask whether to enable
/// SSH, persist the answer, then report the SSH service state.
pub fn boot_configure() {
    if !is_configured() && framebuffer::available() {
        prompt_ssh();
    }
    if ssh_enabled() {
        crate::console_write(b"services: sshd enabled at boot (SSH transport is WIP)\n");
    } else {
        crate::console_write(b"services: sshd off (enable with 'services enable ssh')\n");
    }
}

/// A one-time first-boot prompt: "Start the SSH server at boot? (y/n)".
fn prompt_ssh() {
    let w = framebuffer::width();
    let h = framebuffer::height();
    if w == 0 || h == 0 {
        set_ssh(false);
        return;
    }
    let cw = 540u32;
    let ch = 150u32;
    let cx = w.saturating_sub(cw) / 2;
    let cy = h.saturating_sub(ch) / 2;
    framebuffer::back_fill_rect(0, 0, w, h, 0x0a0a18);
    framebuffer::back_fill_rounded_rect(cx, cy, cw, ch, 12, 0x1c1c38);
    font8x8::draw_str(cx + 26, cy + 26, b"LingOS  -  Services", 0xffb733, 0x1c1c38);
    font8x8::draw_str(cx + 26, cy + 62, b"Start the SSH server at boot?", 0xeae8f4, 0x1c1c38);
    font8x8::draw_str(cx + 26, cy + 92, b"Y = enable      N = keep it off", 0x9a98b4, 0x1c1c38);
    framebuffer::present();
    // Drain keystrokes buffered before this prompt (notably the Enter that just
    // selected the locale) so a leftover keypress can't silently auto-answer
    // the question -- only the user's actual Y/N below should count.
    let mut drain = 0;
    while keyboard::poll_char() != 0 && drain < 256 {
        drain += 1;
    }
    // Wait up to 30s for an answer, then default to off -- so a headless or
    // automated boot (no keyboard) proceeds instead of hanging here forever.
    let mut on = false;
    let mut answered = false;
    crate::arch::timer::poll_until(30_000_000, || {
        let k = keyboard::poll_char();
        match k {
            b'y' | b'Y' => {
                on = true;
                answered = true;
                true
            },
            b'n' | b'N' | b'\r' | b'\n' | 0x1b => {
                answered = true;
                true
            },
            _ => false,
        }
    });
    let _ = answered;
    set_ssh(on);
}
