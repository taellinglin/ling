//! Wi-Fi for the desktops and the console: scan, pick a network, join, and
//! remember it (`wifi.conf` on storage, joined again at boot). The radio
//! driver is `arch::wifi` on the Raspberry Pi 5; PCs have no radio driver
//! yet, so there everything reports "no Wi-Fi radio". `demo` mode (console
//! `wifi demo`) answers scans with a made-up list so the picker can be
//! exercised in QEMU, where no radio exists; it is never on by default.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

/// (ssid, signal dBm, security: 0 open, 1 WEP, 2 WPA, 3 WPA2 (or WPA2/WPA3 mixed), 4 WPA3-only, channel)
pub struct Network {
    pub ssid: String,
    pub rssi: i32,
    pub secure: u8,
    pub channel: u16,
}

static mut DEMO: bool = false;
static mut LAST: String = String::new();

const CONF: &str = "wifi.conf";

pub fn set_demo(on: bool) {
    unsafe { DEMO = on };
}

fn demo() -> bool {
    unsafe { DEMO }
}

/// Is there a radio this kernel can drive?
pub fn available() -> bool {
    if demo() {
        return true;
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::mmio::board() == crate::arch::mmio::RpiBoard::Pi5
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        false
    }
}

pub fn scan() -> Result<Vec<Network>, String> {
    if demo() {
        return Ok(alloc::vec![
            Network { ssid: String::from("LingCountry"), rssi: -41, secure: 3, channel: 6 },
            Network { ssid: String::from("Moon Palace 5G"), rssi: -58, secure: 3, channel: 36 },
            Network { ssid: String::from("cafe-open"), rssi: -70, secure: 0, channel: 11 },
            Network { ssid: String::from("old-router"), rssi: -82, secure: 1, channel: 1 },
            Network { ssid: String::from("Neo WPA3"), rssi: -66, secure: 4, channel: 44 },
        ]);
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::wifi::scan()
            .map(|v| v.into_iter().map(|n| Network { ssid: n.ssid, rssi: n.rssi, secure: n.secure, channel: n.channel }).collect())
            .map_err(String::from)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        Err(String::from("no Wi-Fi radio driver on this machine"))
    }
}

/// Join and, on success, remember the network and get an address.
pub fn connect(ssid: &str, psk: &str) -> Result<String, String> {
    if demo() {
        if psk.len() < 8 && !psk.is_empty() {
            return Err(String::from("a WPA2 passphrase is 8-63 characters"));
        }
        unsafe { *(&raw mut LAST) = String::from(ssid) };
        remember(ssid, psk);
        return Ok(format!("joined {ssid} (demo radio, no traffic)"));
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::wifi::join(ssid, psk)?;
        unsafe { *(&raw mut LAST) = String::from(ssid) };
        remember(ssid, psk);
        // The static address from Settings > Network when there is one, DHCP otherwise.
        if crate::drivers::netstack::configure() {
            let ip = crate::drivers::netstack::self_ip();
            Ok(format!("joined {ssid}: {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]))
        } else {
            Ok(format!("joined {ssid}, but DHCP gave no address yet (Settings > Network sets a static one)"))
        }
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        let _ = (ssid, psk);
        Err(String::from("no Wi-Fi radio driver on this machine"))
    }
}

/// Leave the network but keep it remembered (the tray's Disconnect).
pub fn disconnect() {
    unsafe { (*(&raw mut LAST)).clear() };
    #[cfg(target_arch = "aarch64")]
    crate::arch::wifi::leave();
}

/// Joined right now?
pub fn joined() -> bool {
    if demo() {
        return !unsafe { (&*(&raw const LAST)).is_empty() };
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::wifi::is_joined()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        false
    }
}

/// Signal of the joined network in dBm (0 when not joined / unknown).
pub fn signal() -> i32 {
    if demo() {
        return if joined() { -58 } else { 0 };
    }
    #[cfg(target_arch = "aarch64")]
    {
        crate::arch::wifi::rssi().unwrap_or(0)
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        0
    }
}

pub fn forget() {
    unsafe { (*(&raw mut LAST)).clear() };
    #[cfg(target_arch = "aarch64")]
    crate::arch::wifi::leave();
    if crate::fs::lingfs::is_mounted() {
        let _ = crate::fs::lingfs::write_path_any(CONF, b"");
    }
}

/// One line for the picker's header and the tray.
pub fn status() -> String {
    if demo() {
        let l = unsafe { (&*(&raw const LAST)).clone() };
        return if l.is_empty() { String::from("demo radio: not joined") } else { format!("demo radio: joined {l}") };
    }
    #[cfg(target_arch = "aarch64")]
    {
        if !available() {
            return String::from("no Wi-Fi radio on this board (Pi 5 only)");
        }
        crate::arch::wifi::status()
    }
    #[cfg(not(target_arch = "aarch64"))]
    {
        String::from("no Wi-Fi radio driver on this machine")
    }
}

pub fn ssid() -> String {
    unsafe { (&*(&raw const LAST)).clone() }
}

fn remember(ssid: &str, psk: &str) {
    if crate::fs::lingfs::is_mounted() {
        let _ = crate::fs::lingfs::write_path_any(CONF, format!("{ssid}\n{psk}\n").as_bytes());
    }
}

/// The remembered network, if storage holds one.
pub fn remembered() -> Option<(String, String)> {
    if !crate::fs::lingfs::is_mounted() || crate::fs::lingfs::path_kind(CONF) != Some(false) {
        return None;
    }
    let size = crate::fs::lingfs::file_size(CONF)?;
    let mut buf = alloc::vec![0u8; size.min(256)];
    let n = crate::fs::lingfs::read_file_all(CONF, &mut buf).ok()??;
    let text = String::from_utf8_lossy(&buf[..n]).into_owned();
    let mut lines = text.lines();
    let ssid = String::from(lines.next()?.trim());
    if ssid.is_empty() {
        return None;
    }
    let psk = String::from(lines.next().unwrap_or("").trim());
    Some((ssid, psk))
}

/// At boot, once storage is mounted: join the remembered network when the
/// wire gave no address.
pub fn boot_autojoin() {
    if !available() || demo() {
        return;
    }
    let Some((ssid, psk)) = remembered() else { return };
    if crate::drivers::netstack::self_ip() != [0, 0, 0, 0] && crate::drivers::netstack::self_ip() != [10, 0, 2, 15] {
        return;
    }
    crate::console_write(b"wifi: joining the remembered network ");
    crate::console_write(ssid.as_bytes());
    crate::console_write(b"\n");
    match connect(&ssid, &psk) {
        Ok(m) | Err(m) => {
            crate::console_write(b"wifi: ");
            crate::console_write(m.as_bytes());
            crate::console_write(b"\n");
        },
    }
}

/// Console `wifi [scan | join <ssid> [psk] | leave | demo | status]`.
pub fn console(args: &[u8]) {
    let a = core::str::from_utf8(args).unwrap_or("").trim();
    let w = |s: &str| crate::console_write(s.as_bytes());
    if a == "demo" {
        set_demo(true);
        w("wifi: demo radio on (made-up networks, for testing the picker)\n");
    } else if a == "scan" {
        match scan() {
            Ok(list) if list.is_empty() => w("wifi: no networks found\n"),
            Ok(list) => {
                for n in list {
                    let sec = ["open", "WEP", "WPA", "WPA2", "WPA3"][n.secure.min(4) as usize];
                    w(&format!("  {:<32} {:>4} dBm  ch {:<3} {}\n", n.ssid, n.rssi, n.channel, sec));
                }
            },
            Err(e) => w(&format!("wifi: {e}\n")),
        }
    } else if let Some(rest) = a.strip_prefix("join ") {
        let mut it = rest.trim().splitn(2, ' ');
        let ssid = it.next().unwrap_or("");
        let psk = it.next().unwrap_or("").trim();
        match connect(ssid, psk) {
            Ok(m) | Err(m) => w(&format!("wifi: {m}\n")),
        }
    } else if a == "leave" || a == "disconnect" {
        disconnect();
        w("wifi: left the network (still remembered; `wifi forget` drops it)\n");
    } else if a == "forget" {
        forget();
        w("wifi: left (and forgot the saved network)\n");
    } else if let Some(cc) = a.strip_prefix("country ") {
        #[cfg(target_arch = "aarch64")]
        match crate::arch::wifi::set_country(cc) {
            Ok(()) => w(&format!("wifi: country {}\n", cc.trim().to_ascii_uppercase())),
            Err(e) => w(&format!("wifi: {e}\n")),
        }
        #[cfg(not(target_arch = "aarch64"))]
        w(&format!("wifi: no radio to set {cc} on\n"));
    } else if a.is_empty() || a == "status" {
        #[cfg(target_arch = "aarch64")]
        {
            if a.is_empty() && !demo() {
                crate::arch::wifi::probe();
            }
        }
        w(&format!("wifi: {}\n", status()));
    } else {
        w("usage: wifi [scan | join <ssid> [passphrase] | leave | forget | country <CC> | status | demo]\n");
    }
}
