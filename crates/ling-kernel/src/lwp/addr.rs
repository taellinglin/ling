//! Peer address parsing and network-group (eclipse-resistance) bucketing.
//! (Port of ling-net `lwp/addr.rs`; `std::net` → `core::net`.)
//!
//! Accepted forms: `1.2.3.4:port`, `[v6]:port`, `hostname:port`,
//! `xyz.onion:port`. Hostnames are never resolved here; the dialer decides
//! whether to resolve (direct) or hand the name to SOCKS5 (proxied).

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt;
use core::net::{IpAddr, Ipv4Addr, Ipv6Addr};

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) enum Host {
    Ip(IpAddr),
    Onion(String),
    Name(String),
}

#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub(crate) struct NetAddr {
    pub host: Host,
    pub port: u16,
}

pub(crate) const MAX_ADDR_STR: usize = 255;

fn valid_hostname(h: &str) -> bool {
    if h.is_empty() || h.len() > 253 {
        return false;
    }
    let labels: Vec<&str> = h.split('.').collect();
    for l in &labels {
        if l.is_empty() || l.len() > 63 || l.starts_with('-') || l.ends_with('-') {
            return false;
        }
        if !l.bytes().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-') {
            return false;
        }
    }
    // A numeric last label could be interpreted by inet_aton-style resolvers
    // as an IPv4 shorthand ("127.1"); refuse that ambiguity outright.
    !labels.last().map(|l| l.bytes().all(|c| c.is_ascii_digit())).unwrap_or(true)
}

/// Canonical IPv4 for v4-mapped IPv6 addresses.
pub(crate) fn canon_ip(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => match v6.to_ipv4_mapped() {
            Some(v4) => IpAddr::V4(v4),
            None => IpAddr::V6(v6),
        },
        v4 => v4,
    }
}

/// Loopback, private, link-local, unspecified: addresses that only make
/// sense on this machine or LAN.
pub(crate) fn ip_is_local(ip: IpAddr) -> bool {
    match canon_ip(ip) {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified(),
        IpAddr::V6(v6) => {
            let s = v6.segments();
            v6.is_loopback() || v6.is_unspecified() || (s[0] & 0xfe00) == 0xfc00 || (s[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Publicly routable unicast address (eligible for address gossip).
pub(crate) fn ip_is_routable(ip: IpAddr) -> bool {
    if ip_is_local(ip) {
        return false;
    }
    match canon_ip(ip) {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            !(v4.is_broadcast()
                || v4.is_multicast()
                || v4.is_documentation()
                || o[0] == 0
                || o[0] >= 240
                || (o[0] == 100 && (o[1] & 0xc0) == 64))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            !(v6.is_multicast() || (s[0] == 0x2001 && s[1] == 0x0db8))
        }
    }
}

impl NetAddr {
    pub fn parse(s: &str) -> Option<NetAddr> {
        if s.is_empty() || s.len() > MAX_ADDR_STR {
            return None;
        }
        let (host_s, port_s) = if let Some(rest) = s.strip_prefix('[') {
            let (h, p) = rest.split_once("]:")?;
            let ip: Ipv6Addr = h.parse().ok()?;
            let port: u16 = parse_port(p)?;
            return Some(NetAddr { host: Host::Ip(canon_ip(IpAddr::V6(ip))), port });
        } else {
            s.rsplit_once(':')?
        };
        if host_s.contains(':') {
            return None; // bare IPv6 must be bracketed
        }
        let port = parse_port(port_s)?;
        if let Ok(v4) = host_s.parse::<Ipv4Addr>() {
            return Some(NetAddr { host: Host::Ip(IpAddr::V4(v4)), port });
        }
        let h = host_s.to_ascii_lowercase();
        if !valid_hostname(&h) {
            return None;
        }
        if h.ends_with(".onion") {
            Some(NetAddr { host: Host::Onion(h), port })
        } else {
            Some(NetAddr { host: Host::Name(h), port })
        }
    }

    pub fn ip(&self) -> Option<IpAddr> {
        match self.host {
            Host::Ip(ip) => Some(ip),
            _ => None,
        }
    }

    /// Network group for outbound diversity. Public IPv4 → /16, public IPv6
    /// → /32. Local addresses (loopback/private/link-local) are each their
    /// own group *per port*. Each onion and each DNS name is its own group.
    pub fn group(&self) -> String {
        match &self.host {
            Host::Ip(ip) if ip_is_local(*ip) => format!("local:{}", self),
            Host::Ip(IpAddr::V4(v4)) => {
                let o = v4.octets();
                format!("v4:{}.{}", o[0], o[1])
            }
            Host::Ip(IpAddr::V6(v6)) => {
                let s = v6.segments();
                format!("v6:{:x}:{:x}", s[0], s[1])
            }
            Host::Onion(h) => format!("onion:{h}"),
            Host::Name(h) => format!("name:{h}"),
        }
    }
}

/// Group for a peer whose real IP we observed (direct connections).
pub(crate) fn ip_group(ip: IpAddr, port: u16) -> String {
    NetAddr { host: Host::Ip(canon_ip(ip)), port }.group()
}

fn parse_port(p: &str) -> Option<u16> {
    if p.is_empty() || p.len() > 5 || !p.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let v: u16 = p.parse().ok()?;
    (v != 0).then_some(v)
}

impl fmt::Display for NetAddr {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.host {
            Host::Ip(IpAddr::V4(v4)) => write!(f, "{v4}:{}", self.port),
            Host::Ip(IpAddr::V6(v6)) => write!(f, "[{v6}]:{}", self.port),
            Host::Onion(h) | Host::Name(h) => write!(f, "{h}:{}", self.port),
        }
    }
}

