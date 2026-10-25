//! First-boot setup wizard for the Raspberry Pi (aarch64) framebuffer.
//!
//! The aarch64 kernel has no window manager — just the linear framebuffer,
//! `font8x8`, and the PL011/debug UART — so this is a self-contained form drawn
//! straight to the framebuffer and driven by a single key-event stream. It
//! collects the first-boot configuration the user asked for: language, time
//! zone, hostname, root + user passwords, and SSH.
//!
//! **Input model.** `read_key()` turns raw bytes into [`Key`] events. Today the
//! sources are the serial console and the USB keyboard (`crate::poll_input`
//! merges both; USB keys arrive pre-encoded like a terminal's), which is what makes
//! this testable in QEMU (scripted key input + screendump) *before* the USB HID
//! stack exists. When USB keyboard/mouse land they feed the same [`Key`] stream
//! (and, for the mouse, pointer/click events hit-testing the same widget rows —
//! the row geometry below is the single source of truth so hit-testing can reuse
//! it). Navigation is identical whether driven by keyboard alone, mouse alone,
//! or both.
//!
//! **Persistence.** On apply the collected config is stashed in-process
//! ([`last_config`]). Writing it to the SD card is deferred until the Pi has a
//! storage driver — the wizard says so on the final screen rather than
//! pretending it persisted.

use crate::drivers::{font8x8, framebuffer};

// ── Palette ────────────────────────────────────────────────────────────────
const BG: u32 = 0x10_10_18;
const ACCENT: u32 = 0x6C_3A_FF;
const ACCENT_LT: u32 = 0xA6_8C_FF;
const FG: u32 = 0xC8_D0_E0;
const WHITE: u32 = 0xFF_FF_FF;
const DIM: u32 = 0x70_76_88;
const GREEN: u32 = 0x4A_D9_7A;
const FIELD_BG: u32 = 0x1C_1E_2A;
const FIELD_FOCUS: u32 = 0x2B_25_50;
const ROW_FOCUS: u32 = 0x1A_17_30;

// ── Row geometry (also the future mouse hit-test rects) ──────────────────────
const LABEL_X: u32 = 40;
const VAL_X: u32 = 300;
const VAL_W: u32 = 380;
const ROW_Y0: u32 = 104;
const ROW_H: u32 = 30;
/// Number of focusable rows, including the Apply button (the last one).
const ROWS: usize = 9;
const APPLY_ROW: usize = ROWS - 1;

const LANGS: [&[u8]; 5] = [b"English", b"Chinese", b"Korean", b"Thai", b"Celestial"];
const TZS: [&[u8]; 8] = [
    b"UTC",
    b"UTC-8  Pacific",
    b"UTC-6  Central",
    b"UTC-5  Eastern",
    b"UTC+0  GMT/London",
    b"UTC+1  Central Europe",
    b"UTC+8  China",
    b"UTC+9  Japan/Korea",
];

// ── Text field ───────────────────────────────────────────────────────────────
const TB_CAP: usize = 40;

#[derive(Clone, Copy)]
pub(crate) struct TextBuf {
    buf: [u8; TB_CAP],
    len: usize,
}

impl TextBuf {
    const fn new() -> Self {
        TextBuf { buf: [0; TB_CAP], len: 0 }
    }
    fn push(&mut self, c: u8) {
        if self.len < TB_CAP {
            self.buf[self.len] = c;
            self.len += 1;
        }
    }
    fn backspace(&mut self) {
        if self.len > 0 {
            self.len -= 1;
        }
    }
    fn as_bytes(&self) -> &[u8] {
        &self.buf[..self.len]
    }
    fn len(&self) -> usize {
        self.len
    }
    fn set(&mut self, s: &[u8]) {
        self.len = 0;
        for &c in s {
            self.push(c);
        }
    }
}

// ── Collected configuration ──────────────────────────────────────────────────
#[derive(Clone, Copy)]
pub(crate) struct SetupConfig {
    pub(crate) lang: usize,
    pub(crate) tz: usize,
    pub(crate) hostname: TextBuf,
    pub(crate) root_pw: TextBuf,
    pub(crate) username: TextBuf,
    pub(crate) user_pw: TextBuf,
    pub(crate) ssh: bool,
    pub(crate) ssh_port: TextBuf,
}

impl SetupConfig {
    fn new() -> Self {
        let mut c = SetupConfig {
            lang: 0,
            tz: 0,
            hostname: TextBuf::new(),
            root_pw: TextBuf::new(),
            username: TextBuf::new(),
            user_pw: TextBuf::new(),
            ssh: false,
            ssh_port: TextBuf::new(),
        };
        c.hostname.set(b"lingos");
        c.ssh_port.set(b"22");
        c
    }
}

static mut LAST_CONFIG: Option<SetupConfig> = None;

/// The configuration from the most recent completed wizard run, if any. Lets
/// later boot stages (and a future SD-persistence step) read what was chosen.
/// Not wired to a consumer yet — that's the storage-driver step.
#[allow(dead_code)]
pub(crate) fn last_config() -> Option<SetupConfig> {
    unsafe { core::ptr::read(&raw const LAST_CONFIG) }
}

// ── Input ────────────────────────────────────────────────────────────────────
enum Key {
    Char(u8),
    Enter,
    Backspace,
    Tab,
    Up,
    Down,
    Left,
    Right,
    Escape,
}

fn read_byte_blocking() -> u8 {
    loop {
        if let Some(b) = crate::poll_input() {
            return b;
        }
        core::hint::spin_loop();
    }
}

/// Wait a bounded moment for a follow-on byte — CSI arrow sequences (ESC `[` A)
/// arrive as a burst, so a short spin catches the rest without blocking forever
/// on a lone ESC.
fn poll_soon() -> Option<u8> {
    for _ in 0..300_000 {
        if let Some(b) = crate::poll_input() {
            return Some(b);
        }
        core::hint::spin_loop();
    }
    None
}

/// Block until a full key is decoded from the serial stream. (USB HID will feed
/// the same `Key` values once it exists.)
fn read_key() -> Key {
    loop {
        let b = read_byte_blocking();
        match b {
            0x1b => {
                // ESC alone, or a CSI/SS3 cursor sequence.
                match poll_soon() {
                    Some(b'[') | Some(b'O') => match poll_soon() {
                        Some(b'A') => return Key::Up,
                        Some(b'B') => return Key::Down,
                        Some(b'C') => return Key::Right,
                        Some(b'D') => return Key::Left,
                        _ => return Key::Escape,
                    },
                    _ => return Key::Escape,
                }
            },
            b'\r' | b'\n' => return Key::Enter,
            0x08 | 0x7f => return Key::Backspace,
            b'\t' => return Key::Tab,
            0x20..=0x7e => return Key::Char(b),
            _ => continue,
        }
    }
}

fn cycle(i: usize, n: usize, forward: bool) -> usize {
    if n == 0 {
        return 0;
    }
    if forward {
        (i + 1) % n
    } else {
        (i + n - 1) % n
    }
}

// ── Drawing helpers ──────────────────────────────────────────────────────────
fn draw_label(y: u32, focused: bool, label: &[u8]) {
    let w = framebuffer::width();
    let bg = if focused { ROW_FOCUS } else { BG };
    framebuffer::fill_rect(20, y.saturating_sub(5), w.saturating_sub(40), 26, bg);
    if focused {
        font8x8::draw_str(26, y, b">", ACCENT_LT, bg);
    }
    font8x8::draw_str(LABEL_X, y, label, if focused { WHITE } else { FG }, bg);
}

fn field_bg(focused: bool) -> u32 {
    if focused {
        FIELD_FOCUS
    } else {
        FIELD_BG
    }
}

fn draw_text_row(y: u32, focused: bool, label: &[u8], val: &[u8], masked: bool) {
    draw_label(y, focused, label);
    let fbg = field_bg(focused);
    framebuffer::fill_rect(VAL_X, y.saturating_sub(4), VAL_W, 22, fbg);
    let mut stars = [b'*'; TB_CAP];
    let shown: &[u8] = if masked {
        let n = val.len().min(TB_CAP);
        &stars[..n]
    } else {
        let _ = &mut stars; // silence unused in the non-masked arm
        val
    };
    font8x8::draw_str(VAL_X + 10, y, shown, WHITE, fbg);
    if focused {
        let cx = VAL_X + 10 + shown.len() as u32 * 8;
        framebuffer::fill_rect(cx, y, 8, 10, ACCENT_LT);
    }
}

fn draw_choice_row(y: u32, focused: bool, label: &[u8], value: &[u8]) {
    draw_label(y, focused, label);
    let fbg = field_bg(focused);
    framebuffer::fill_rect(VAL_X, y.saturating_sub(4), VAL_W, 22, fbg);
    font8x8::draw_str(VAL_X + 8, y, b"<", ACCENT_LT, fbg);
    font8x8::draw_str(VAL_X + 30, y, value, WHITE, fbg);
    font8x8::draw_str(VAL_X + VAL_W - 16, y, b">", ACCENT_LT, fbg);
}

fn draw_toggle_row(y: u32, focused: bool, label: &[u8], on: bool) {
    draw_label(y, focused, label);
    let fbg = field_bg(focused);
    framebuffer::fill_rect(VAL_X, y.saturating_sub(4), VAL_W, 22, fbg);
    let (txt, col): (&[u8], u32) = if on { (b"< On  >", GREEN) } else { (b"< Off >", DIM) };
    font8x8::draw_str(VAL_X + 8, y, txt, col, fbg);
}

fn draw_button(y: u32, focused: bool, label: &[u8]) {
    let w = framebuffer::width();
    let bw = 180u32;
    let bx = (w.saturating_sub(bw)) / 2;
    let col = if focused { ACCENT } else { 0x33_33_44 };
    framebuffer::fill_rect(bx, y, bw, 30, col);
    let tx = bx + (bw.saturating_sub(label.len() as u32 * 8)) / 2;
    font8x8::draw_str(tx, y + 10, label, WHITE, col);
}

fn header(title: &[u8]) {
    let w = framebuffer::width();
    let h = framebuffer::height();
    framebuffer::fill_rect(0, 0, w, h, BG);
    framebuffer::fill_rect(0, 0, w, 46, ACCENT);
    font8x8::set_scale(2);
    font8x8::draw_str(16, 15, title, WHITE, ACCENT);
    font8x8::set_scale(1);
}

fn render_form(cfg: &SetupConfig, focus: usize) {
    header(b"LingOS Setup");
    font8x8::draw_str(16, 58, b"First-boot configuration", ACCENT_LT, BG);
    font8x8::draw_str(
        16,
        76,
        b"Up/Down move   Left/Right change   type to edit   Enter next/apply   Esc cancel",
        DIM,
        BG,
    );

    draw_choice_row(ROW_Y0, focus == 0, b"Language", LANGS[cfg.lang]);
    draw_choice_row(ROW_Y0 + ROW_H, focus == 1, b"Time zone", TZS[cfg.tz]);
    draw_text_row(ROW_Y0 + 2 * ROW_H, focus == 2, b"Hostname", cfg.hostname.as_bytes(), false);
    draw_text_row(ROW_Y0 + 3 * ROW_H, focus == 3, b"Root password", cfg.root_pw.as_bytes(), true);
    draw_text_row(ROW_Y0 + 4 * ROW_H, focus == 4, b"Username", cfg.username.as_bytes(), false);
    draw_text_row(ROW_Y0 + 5 * ROW_H, focus == 5, b"User password", cfg.user_pw.as_bytes(), true);
    draw_toggle_row(ROW_Y0 + 6 * ROW_H, focus == 6, b"Enable SSH", cfg.ssh);
    draw_text_row(ROW_Y0 + 7 * ROW_H, focus == 7, b"SSH port", cfg.ssh_port.as_bytes(), false);
    draw_button(ROW_Y0 + 8 * ROW_H + 12, focus == APPLY_ROW, b"Apply");
}

fn kv(y: u32, label: &[u8], value: &[u8]) {
    font8x8::draw_str(LABEL_X, y, label, DIM, BG);
    font8x8::draw_str(VAL_X, y, value, WHITE, BG);
}

fn render_review(cfg: &SetupConfig) {
    header(b"Review");
    let y0 = 80u32;
    let rh = 26u32;
    let mut stars = [b'*'; TB_CAP];
    kv(y0, b"Language", LANGS[cfg.lang]);
    kv(y0 + rh, b"Time zone", TZS[cfg.tz]);
    kv(y0 + 2 * rh, b"Hostname", cfg.hostname.as_bytes());
    kv(y0 + 3 * rh, b"Root password", &stars[..cfg.root_pw.len().min(TB_CAP)]);
    kv(y0 + 4 * rh, b"Username", cfg.username.as_bytes());
    kv(y0 + 5 * rh, b"User password", &stars[..cfg.user_pw.len().min(TB_CAP)]);
    kv(y0 + 6 * rh, b"SSH", if cfg.ssh { b"enabled" } else { b"disabled" });
    if cfg.ssh {
        kv(y0 + 7 * rh, b"SSH port", cfg.ssh_port.as_bytes());
    }
    let _ = &mut stars;
    font8x8::draw_str(LABEL_X, y0 + 9 * rh, b"Enter: apply      Esc / Left: go back", ACCENT_LT, BG);
}

fn render_done() {
    header(b"Setup complete");
    font8x8::draw_str(LABEL_X, 90, b"Configuration captured.", WHITE, BG);
    font8x8::draw_str(
        LABEL_X,
        120,
        b"Note: saving to the SD card is pending the Pi storage driver,",
        DIM,
        BG,
    );
    font8x8::draw_str(LABEL_X, 138, b"so these settings live in memory for now.", DIM, BG);
    font8x8::draw_str(LABEL_X, 176, b"Press any key to return to the console.", ACCENT_LT, BG);
}

// ── Edits ────────────────────────────────────────────────────────────────────
fn edit_choice(cfg: &mut SetupConfig, focus: usize, forward: bool) {
    match focus {
        0 => cfg.lang = cycle(cfg.lang, LANGS.len(), forward),
        1 => cfg.tz = cycle(cfg.tz, TZS.len(), forward),
        6 => cfg.ssh = !cfg.ssh,
        _ => {},
    }
}

fn edit_char(cfg: &mut SetupConfig, focus: usize, c: u8) {
    match focus {
        2 => {
            if c.is_ascii_alphanumeric() || c == b'-' {
                cfg.hostname.push(c);
            }
        },
        3 => cfg.root_pw.push(c),
        4 => {
            if c.is_ascii_alphanumeric() || c == b'_' || c == b'-' {
                cfg.username.push(c);
            }
        },
        5 => cfg.user_pw.push(c),
        7 => {
            if c.is_ascii_digit() {
                cfg.ssh_port.push(c);
            }
        },
        _ => {},
    }
}

fn edit_backspace(cfg: &mut SetupConfig, focus: usize) {
    match focus {
        2 => cfg.hostname.backspace(),
        3 => cfg.root_pw.backspace(),
        4 => cfg.username.backspace(),
        5 => cfg.user_pw.backspace(),
        7 => cfg.ssh_port.backspace(),
        _ => {},
    }
}

/// The saved choices (`setup.conf` on the card): `key=value` lines.
fn save(cfg: &SetupConfig) -> bool {
    if !crate::fs::lingfs::is_mounted() {
        return false;
    }
    let mut text = alloc::string::String::new();
    text.push_str("hostname=");
    text.push_str(core::str::from_utf8(cfg.hostname.as_bytes()).unwrap_or("lingos"));
    text.push_str(&alloc::format!("\nlang={}\ntz={}\nssh={}\nssh_port=", cfg.lang, cfg.tz, if cfg.ssh { 1 } else { 0 }));
    text.push_str(core::str::from_utf8(cfg.ssh_port.as_bytes()).unwrap_or("22"));
    text.push('\n');
    crate::fs::lingfs::write_path_any("setup.conf", text.as_bytes()).is_ok()
}

/// The card's sector reader for the FAT driver (the boot partition).
struct SdDisk;

impl crate::fs::fat::Disk for SdDisk {
    fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), ()> {
        let mut s = [0u8; crate::arch::sdcard::SECTOR_SIZE];
        for (i, chunk) in buf.chunks_mut(s.len()).enumerate() {
            crate::arch::sdcard::read_sector(lba + i as u32, &mut s)?;
            let n = chunk.len();
            chunk.copy_from_slice(&s[..n]);
        }
        Ok(())
    }
    fn write(&mut self, _lba: u32, _buf: &[u8]) -> Result<(), ()> {
        Err(()) // read-only: the boot partition belongs to the firmware
    }
}

/// `lingos_reset_password=<token>[:<user>]` in the boot partition's
/// config.txt (the Pi firmware ignores keys it does not know): each new token
/// puts the named account -- or every account -- into "choose a new
/// password" mode once. Recovery needs the card in another computer, the
/// same trust as any other edit of the boot files.
pub(crate) fn check_password_reset() {
    if !crate::fs::lingfs::is_mounted() || !crate::arch::sdcard::ready() {
        return;
    }
    let mut mbr = [0u8; crate::arch::sdcard::SECTOR_SIZE];
    if crate::arch::sdcard::read_sector(0, &mut mbr).is_err() || mbr[510] != 0x55 || mbr[511] != 0xAA {
        return;
    }
    let mut disk = SdDisk;
    for i in 0..4 {
        let e = &mbr[446 + i * 16..446 + (i + 1) * 16];
        if !matches!(e[4], 0x0b | 0x0c | 0x0e | 0x06) {
            continue;
        }
        let start = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
        let Some(mut vol) = crate::fs::fat::Volume::mount(&mut disk, start) else { continue };
        let Ok(cfg) = vol.read_file(&mut disk, "config.txt") else { continue };
        let text = alloc::string::String::from_utf8_lossy(&cfg).into_owned();
        let Some(v) = text.lines().find_map(|l| l.trim().strip_prefix("lingos_reset_password=")) else { return };
        let v = v.trim();
        if v.is_empty() || v == "0" {
            return;
        }
        // one-shot per token
        let mut seen = [0u8; 128];
        if let Ok(Some(n)) = crate::fs::lingfs::read_file_all("password_reset_token", &mut seen) {
            if &seen[..n] == v.as_bytes() {
                return;
            }
        }
        let who = match v.split_once(':') {
            Some((_, u)) if !u.trim().is_empty() => u.trim(),
            _ => "*",
        };
        crate::fs::users::mark_reset(who);
        let _ = crate::fs::lingfs::write_file_any("password_reset_token", v.as_bytes());
        crate::console_write(b"users: password reset requested in config.txt for ");
        crate::console_write(if who == "*" { b"every account" } else { who.as_bytes() });
        crate::console_write(b" -- the sign-in screen asks for a new password\n");
        return;
    }
}

/// One value from the saved `setup.conf` ("" when unset or no storage).
pub(crate) fn saved(key: &str) -> alloc::string::String {
    if !crate::fs::lingfs::is_mounted() {
        return alloc::string::String::new();
    }
    let mut buf = [0u8; 512];
    let n = match crate::fs::lingfs::read_file_all("setup.conf", &mut buf) {
        Ok(Some(n)) => n,
        _ => return alloc::string::String::new(),
    };
    let text = core::str::from_utf8(&buf[..n]).unwrap_or("");
    for line in text.lines() {
        if let Some(v) = line.strip_prefix(key).and_then(|r| r.strip_prefix('=')) {
            return alloc::string::String::from(v.trim());
        }
    }
    alloc::string::String::new()
}

/// The machine's name: the wizard's choice when it was saved, else "lingos".
pub(crate) fn hostname() -> alloc::string::String {
    let h = saved("hostname");
    if h.is_empty() {
        alloc::string::String::from("lingos")
    } else {
        h
    }
}

fn apply(cfg: &SetupConfig) {
    unsafe { core::ptr::write(&raw mut LAST_CONFIG, Some(*cfg)) };
    crate::console_write(if save(cfg) { b"setup: saved to the card (setup.conf)\n" } else { b"setup: not saved (no LingOS storage on the card yet)\n" });
    // Echo a summary to the serial log (invisible on HDMI, useful when a cable
    // is attached / under QEMU).
    crate::console_write(b"\nsetup: applied configuration\n");
    crate::console_write(b"  hostname: ");
    crate::console_write(cfg.hostname.as_bytes());
    crate::console_write(b"\n  ssh: ");
    crate::console_write(if cfg.ssh { b"enabled\n" } else { b"disabled\n" });
}

/// Run the first-boot setup wizard, modal, until the user applies or cancels.
/// Returns when done; the caller's console/REPL resumes afterward.
pub(crate) fn run() {
    if !framebuffer::available() {
        crate::console_write(b"setup: no framebuffer available\n");
        return;
    }
    let mut cfg = SetupConfig::new();
    let mut focus = 0usize;
    let mut review = false;

    loop {
        if review {
            render_review(&cfg);
        } else {
            render_form(&cfg, focus);
        }
        let key = read_key();

        if review {
            match key {
                Key::Enter => {
                    apply(&cfg);
                    render_done();
                    let _ = read_key();
                    break;
                },
                Key::Escape | Key::Left => review = false,
                _ => {},
            }
            continue;
        }

        match key {
            Key::Escape => break,
            Key::Up => focus = cycle(focus, ROWS, false),
            Key::Down | Key::Tab => focus = cycle(focus, ROWS, true),
            Key::Left => edit_choice(&mut cfg, focus, false),
            Key::Right => edit_choice(&mut cfg, focus, true),
            Key::Enter => {
                if focus == APPLY_ROW {
                    review = true;
                } else if focus == 6 {
                    cfg.ssh = !cfg.ssh;
                } else {
                    focus = cycle(focus, ROWS, true);
                }
            },
            Key::Backspace => edit_backspace(&mut cfg, focus),
            Key::Char(c) => edit_char(&mut cfg, focus, c),
        }
    }

    font8x8::set_scale(1);
    framebuffer::clear(BG);
    crate::console_write(b"\nsetup closed.\n");
}
