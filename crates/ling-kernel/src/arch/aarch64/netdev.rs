//! The Raspberry Pi's network interface as `netstack` sees it: the RP1's
//! wired GEM (`rp1_eth`) or, once a Wi-Fi network is joined, the CYW43455
//! (`wifi`). One of them at a time: Wi-Fi takes over the moment it is joined
//! (`netstack::dhcp_configure` then leases on it), the wire otherwise. The IP
//! configuration lives in `rp1_eth`'s statics for both.

use super::{rp1_eth, wifi};

pub fn ensure_init() -> bool {
    wifi::is_joined() || rp1_eth::ensure_init()
}

pub fn link_up() -> bool {
    wifi::is_joined() || rp1_eth::link_up()
}

pub fn mac() -> [u8; 6] {
    if wifi::is_joined() {
        wifi::mac()
    } else {
        rp1_eth::mac()
    }
}

pub fn transmit(buf: &[u8]) -> bool {
    if wifi::is_joined() {
        wifi::transmit(buf)
    } else {
        rp1_eth::transmit(buf)
    }
}

pub fn receive(out: &mut [u8]) -> Option<usize> {
    if wifi::is_joined() {
        wifi::receive(out)
    } else {
        rp1_eth::receive(out)
    }
}

pub fn self_ip() -> [u8; 4] {
    rp1_eth::self_ip()
}
pub fn gateway_ip() -> [u8; 4] {
    rp1_eth::gateway_ip()
}
pub fn subnet_mask() -> [u8; 4] {
    rp1_eth::subnet_mask()
}
pub fn set_ip_config(ip: [u8; 4], mask: [u8; 4], gw: [u8; 4]) {
    rp1_eth::set_ip_config(ip, mask, gw)
}
pub fn arp_selftest() -> Option<[u8; 6]> {
    rp1_eth::arp_selftest()
}

/// "wifi <ssid>" or "ethernet" for the tray.
pub fn kind() -> alloc::string::String {
    if wifi::is_joined() {
        alloc::format!("wifi {}", wifi::ssid())
    } else {
        alloc::string::String::from("ethernet")
    }
}
