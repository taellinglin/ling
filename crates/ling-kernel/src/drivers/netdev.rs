//! x86's network interface as `netstack` sees it: the e1000 (what QEMU and
//! VirtualBox emulate) when present, else the Intel I225/I226 (`net_igc`,
//! the real PC's 2.5 GbE port). One of them, chosen once at the first
//! `ensure_init`. Mirrors `arch::aarch64::netdev`. The IP configuration lives
//! in `net_e1000`'s statics for both.

use super::{iwlwifi, net_e1000, net_igc};

/// Joined to a Wi-Fi network on the Intel AX201: it takes over from the wire
/// the moment it is joined, as on the Pi (`arch::aarch64::netdev`).
fn wifi() -> bool {
    iwlwifi::is_joined()
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Nic {
    Unprobed,
    None,
    E1000,
    Igc,
}

static mut ACTIVE: Nic = Nic::Unprobed;

fn active() -> Nic {
    unsafe { ACTIVE }
}

/// Probe once: e1000 first (VMs), then igc. A machine without either stays
/// without -- PCI isn't hot-plugged, so there's no point rescanning on every
/// netstack call.
pub fn ensure_init() -> bool {
    if wifi() {
        return true;
    }
    match active() {
        Nic::E1000 | Nic::Igc => return true,
        Nic::None => return false,
        Nic::Unprobed => {},
    }
    let n = if net_e1000::ensure_init() {
        Nic::E1000
    } else if net_igc::ensure_init() {
        Nic::Igc
    } else {
        Nic::None
    };
    unsafe { ACTIVE = n };
    n != Nic::None
}

pub fn link_up() -> bool {
    if wifi() {
        return true;
    }
    match active() {
        Nic::E1000 => net_e1000::link_up(),
        Nic::Igc => net_igc::link_up(),
        _ => false,
    }
}

pub fn mac() -> [u8; 6] {
    if wifi() {
        return iwlwifi::mac();
    }
    match active() {
        Nic::Igc => net_igc::mac(),
        _ => net_e1000::mac(),
    }
}

pub fn transmit(buf: &[u8]) -> bool {
    if wifi() {
        return iwlwifi::transmit(buf);
    }
    match active() {
        Nic::E1000 => net_e1000::transmit(buf),
        Nic::Igc => net_igc::transmit(buf),
        _ => false,
    }
}

pub fn receive(out: &mut [u8]) -> Option<usize> {
    if wifi() {
        return iwlwifi::receive(out);
    }
    match active() {
        Nic::E1000 => net_e1000::receive(out),
        Nic::Igc => net_igc::receive(out),
        _ => None,
    }
}

pub fn self_ip() -> [u8; 4] {
    net_e1000::self_ip()
}
pub fn gateway_ip() -> [u8; 4] {
    net_e1000::gateway_ip()
}
pub fn subnet_mask() -> [u8; 4] {
    net_e1000::subnet_mask()
}
pub fn set_ip_config(ip: [u8; 4], mask: [u8; 4], gw: [u8; 4]) {
    net_e1000::set_ip_config(ip, mask, gw)
}

/// Hand-built ARP request for the gateway, reply awaited (see
/// `net_e1000::arp_selftest`), over whichever NIC is active.
pub fn arp_selftest() -> Option<[u8; 6]> {
    // Wi-Fi uses the generic hand-built ARP below (over transmit/receive)
    if !wifi() {
        match active() {
            Nic::E1000 => return net_e1000::arp_selftest(),
            Nic::Igc => {},
            _ => return None,
        }
    }
    let my_mac = mac();
    let mut frame = [0u8; 60];
    frame[0..6].copy_from_slice(&[0xFF; 6]);
    frame[6..12].copy_from_slice(&my_mac);
    frame[12..14].copy_from_slice(&[0x08, 0x06]);
    frame[14..22].copy_from_slice(&[0x00, 0x01, 0x08, 0x00, 6, 4, 0x00, 0x01]);
    frame[22..28].copy_from_slice(&my_mac);
    frame[28..32].copy_from_slice(&self_ip());
    frame[38..42].copy_from_slice(&gateway_ip());
    if !transmit(&frame) {
        return None;
    }
    let mut buf = [0u8; 2048];
    let mut found = None;
    crate::arch::timer::poll_until(8_000_000, || {
        if let Some(len) = receive(&mut buf) {
            if len >= 42 && buf[12..14] == [0x08, 0x06] && buf[20..22] == [0x00, 0x02] && buf[28..32] == gateway_ip() {
                let mut m = [0u8; 6];
                m.copy_from_slice(&buf[22..28]);
                found = Some(m);
                return true;
            }
        }
        false
    });
    found
}

/// "wifi <ssid>" or "ethernet" for the tray (same shape as the Pi's `netdev::kind`).
pub fn kind() -> alloc::string::String {
    if wifi() {
        return alloc::format!("wifi {}", crate::wifi::ssid());
    }
    alloc::string::String::from("ethernet")
}

/// One-line status: which NIC, its PCI id, MAC, link/speed and the IP
/// config -- the terminal's `nic` command and the boot log.
pub fn status_line() -> alloc::string::String {
    use alloc::format;
    let m = mac();
    let mac_s = format!("{:02x}:{:02x}:{:02x}:{:02x}:{:02x}:{:02x}", m[0], m[1], m[2], m[3], m[4], m[5]);
    let (ip, mask, gw) = (self_ip(), subnet_mask(), gateway_ip());
    let ip_s = format!(
        "ip {}.{}.{}.{} mask {}.{}.{}.{} gw {}.{}.{}.{}",
        ip[0], ip[1], ip[2], ip[3], mask[0], mask[1], mask[2], mask[3], gw[0], gw[1], gw[2], gw[3]
    );
    if wifi() {
        return format!("nic: wifi (Intel AX201)  MAC {mac_s}  {}  {ip_s}", iwlwifi::link_line());
    }
    match active() {
        Nic::E1000 => {
            let id = net_e1000::pci_id();
            let link = if net_e1000::link_up() {
                format!("link up {} Mb/s", net_e1000::speed_mbps())
            } else {
                alloc::string::String::from("NO LINK")
            };
            format!("nic: e1000 ({:04x}:{:04x})  MAC {mac_s}  {link}  {ip_s}", id & 0xFFFF, id >> 16)
        },
        Nic::Igc => {
            let (b, d, f) = net_igc::pci_location();
            let link = if net_igc::link_up() {
                format!(
                    "link up {} Mb/s {}",
                    net_igc::speed_mbps(),
                    if net_igc::full_duplex() { "full-duplex" } else { "half-duplex" }
                )
            } else {
                alloc::string::String::from("NO LINK (cable unplugged?)")
            };
            format!(
                "nic: igc {} (8086:{:04x}) at {:02x}:{:02x}.{}  MAC {mac_s}  {link}  {ip_s}",
                net_igc::name(),
                net_igc::device_id(),
                b,
                d,
                f
            )
        },
        Nic::None => {
            let why = net_igc::last_error();
            if why.is_empty() || why == "no I225/I226 controller" {
                alloc::string::String::from("nic: none (no e1000 or I225/I226 Ethernet controller found)")
            } else {
                format!("nic: none (I225/I226 present but failed: {why})")
            }
        },
        Nic::Unprobed => alloc::string::String::from("nic: not probed yet"),
    }
}
