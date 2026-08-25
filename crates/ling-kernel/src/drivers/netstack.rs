//! Minimal IPv4/TCP/HTTP client stack over the (now actually working --
//! see net_e1000's module doc) e1000 driver: enough real protocol to GET
//! a file from a real HTTP server, which is what `lingfu` needs to stop
//! being local-only. Static-IP SLIRP defaults (10.0.2.15/24, gateway
//! 10.0.2.2 -- which QEMU aliases to the host's loopback, so a plain
//! `python -m http.server` on the host is a reachable package repo).
//!
//! Honest scope, per driver-idiom: one TCP connection at a time, client
//! only, in-order segments only (out-of-order data is dropped and the
//! peer's retransmit is relied on), no local retransmit queue (requests
//! are sent once; against SLIRP's local, lossless link this is fine and
//! is disclosed rather than hidden), no window management beyond a fixed
//! receive window, no TLS (a public HTTPS repo needs a real TLS stack --
//! see packages/README.md; the repo URL is plain HTTP by design until
//! then). This is a real wire-protocol implementation, not a simulation:
//! every byte below goes through the e1000 ring to a real peer.

use crate::arch::timer;
use crate::drivers::net_e1000 as nic;

pub const SELF_IP: [u8; 4] = nic::SELF_IP;
pub const GATEWAY_IP: [u8; 4] = nic::GATEWAY_IP;
const MSS: usize = 1400;
const RX_WINDOW: u16 = 8192;
/// Kernel-time poll budgets (TSC under TCG runs fast -- see the e1000
/// module doc's resolution notes; these are sized for that).
const WAIT_BUDGET_US: u64 = 30_000_000;

static mut GW_MAC: [u8; 6] = [0; 6];
static mut GW_MAC_KNOWN: bool = false;

fn checksum(data: &[u8], init: u32) -> u16 {
    let mut sum = init;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

/// Resolve (and cache) the gateway MAC via ARP. Brings the NIC up first
/// if nothing else has yet (idempotent).
pub fn arp_gateway() -> Option<[u8; 6]> {
    if !nic::ensure_init() {
        return None;
    }
    unsafe {
        if GW_MAC_KNOWN {
            return Some(GW_MAC);
        }
    }
    let mac = nic::arp_selftest()?;
    unsafe {
        GW_MAC = mac;
        GW_MAC_KNOWN = true;
    }
    Some(mac)
}

// -- One static TCP connection ------------------------------------------------

struct Tcp {
    remote_ip: [u8; 4],
    remote_port: u16,
    local_port: u16,
    snd_nxt: u32,
    rcv_nxt: u32,
    established: bool,
}

static mut CONN: Tcp = Tcp {
    remote_ip: [0; 4],
    remote_port: 0,
    local_port: 0,
    snd_nxt: 0,
    rcv_nxt: 0,
    established: false,
};

const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_ACK: u8 = 0x10;

/// Build + transmit one TCP segment (IPv4, no options except MSS on SYN).
fn tcp_send(flags: u8, payload: &[u8]) -> bool {
    let Some(gw) = arp_gateway() else { return false };
    let c = unsafe { &*&raw const CONN };
    let opt_len = if flags & TCP_SYN != 0 { 4 } else { 0 };
    let tcp_len = 20 + opt_len + payload.len();
    let ip_len = 20 + tcp_len;
    let mut f = [0u8; 14 + 20 + 20 + 4 + MSS];
    let total = 14 + ip_len;
    // Ethernet
    f[0..6].copy_from_slice(&gw);
    f[6..12].copy_from_slice(&nic::mac());
    f[12..14].copy_from_slice(&[0x08, 0x00]);
    // IPv4
    let ip = &mut f[14..];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&(ip_len as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&0x4C49u16.to_be_bytes()); // "LI" ident, unused
    ip[6] = 0x40; // don't fragment
    ip[8] = 64; // TTL
    ip[9] = 6; // TCP
    ip[12..16].copy_from_slice(&SELF_IP);
    ip[16..20].copy_from_slice(&c.remote_ip);
    let ipsum = checksum(&ip[..20], 0);
    ip[10..12].copy_from_slice(&ipsum.to_be_bytes());
    // TCP
    let t = &mut ip[20..];
    t[0..2].copy_from_slice(&c.local_port.to_be_bytes());
    t[2..4].copy_from_slice(&c.remote_port.to_be_bytes());
    t[4..8].copy_from_slice(&c.snd_nxt.to_be_bytes());
    t[8..12].copy_from_slice(&c.rcv_nxt.to_be_bytes());
    t[12] = (((20 + opt_len) / 4) as u8) << 4;
    t[13] = flags;
    t[14..16].copy_from_slice(&RX_WINDOW.to_be_bytes());
    if opt_len > 0 {
        t[20..24].copy_from_slice(&[0x02, 0x04, (MSS >> 8) as u8, (MSS & 0xFF) as u8]);
    }
    t[20 + opt_len..20 + opt_len + payload.len()].copy_from_slice(payload);
    // TCP checksum over pseudo-header + segment.
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(&SELF_IP);
    pseudo[4..8].copy_from_slice(&c.remote_ip);
    pseudo[9] = 6;
    pseudo[10..12].copy_from_slice(&(tcp_len as u16).to_be_bytes());
    let mut sum = 0u32;
    let mut i = 0;
    while i + 1 < pseudo.len() {
        sum += u16::from_be_bytes([pseudo[i], pseudo[i + 1]]) as u32;
        i += 2;
    }
    let tsum = checksum(&ip[20..20 + tcp_len], sum);
    ip[20 + 16..20 + 18].copy_from_slice(&tsum.to_be_bytes());

    nic::transmit(&f[..total.max(60)])
}

/// Poll for the next TCP segment addressed to our connection. Returns
/// (flags, seq, ack, payload_len) with the payload copied into `out`.
fn tcp_poll(out: &mut [u8]) -> Option<(u8, u32, u32, usize)> {
    let mut buf = [0u8; 2048];
    let len = nic::receive(&mut buf)?;
    if len < 54 || buf[12] != 0x08 || buf[13] != 0x00 || buf[23] != 6 {
        return None; // not IPv4/TCP -- ARP noise etc.
    }
    let c = unsafe { &*&raw const CONN };
    let ihl = ((buf[14] & 0x0F) as usize) * 4;
    let ip_total = u16::from_be_bytes([buf[16], buf[17]]) as usize;
    let t = 14 + ihl;
    let src_port = u16::from_be_bytes([buf[t], buf[t + 1]]);
    let dst_port = u16::from_be_bytes([buf[t + 2], buf[t + 3]]);
    if src_port != c.remote_port || dst_port != c.local_port {
        return None;
    }
    let seq = u32::from_be_bytes([buf[t + 4], buf[t + 5], buf[t + 6], buf[t + 7]]);
    let ack = u32::from_be_bytes([buf[t + 8], buf[t + 9], buf[t + 10], buf[t + 11]]);
    let doff = ((buf[t + 12] >> 4) as usize) * 4;
    let flags = buf[t + 13];
    let payload_off = t + doff;
    let payload_len = (14 + ip_total).saturating_sub(payload_off).min(out.len());
    out[..payload_len].copy_from_slice(&buf[payload_off..payload_off + payload_len]);
    Some((flags, seq, ack, payload_len))
}

/// Open a connection to `ip:port` (three-way handshake). One connection
/// at a time; a still-open previous connection is reset-forgotten first.
pub fn tcp_connect(ip: [u8; 4], port: u16) -> bool {
    unsafe {
        let c = &mut *&raw mut CONN;
        c.remote_ip = ip;
        c.remote_port = port;
        // Ephemeral-ish local port off the timer -- fine for a client that
        // holds one connection at a time.
        c.local_port = 49152 + (timer::now_ms() % 16000) as u16;
        c.snd_nxt = 0x4C49_4E47; // "LING" as ISN, no security claim
        c.rcv_nxt = 0;
        c.established = false;
    }
    if !tcp_send(TCP_SYN, &[]) {
        return false;
    }
    unsafe {
        (*&raw mut CONN).snd_nxt = (*&raw const CONN).snd_nxt.wrapping_add(1);
    }
    let mut scratch = [0u8; 2048];
    let mut ok = false;
    timer::poll_until(WAIT_BUDGET_US, || {
        if let Some((flags, seq, _ack, _n)) = tcp_poll(&mut scratch) {
            if flags & TCP_SYN != 0 && flags & TCP_ACK != 0 {
                unsafe {
                    let c = &mut *&raw mut CONN;
                    c.rcv_nxt = seq.wrapping_add(1);
                    c.established = true;
                }
                ok = true;
                return true;
            }
            if flags & TCP_RST != 0 {
                return true; // refused -- stop waiting
            }
        }
        false
    });
    if ok {
        tcp_send(TCP_ACK, &[]);
    }
    ok
}

/// Send application data (fits in one segment; HTTP requests are small).
pub fn tcp_write(data: &[u8]) -> bool {
    if !unsafe { (*&raw const CONN).established } || data.len() > MSS {
        return false;
    }
    if !tcp_send(TCP_ACK | 0x08 /* PSH */, data) {
        return false;
    }
    unsafe {
        let c = &mut *&raw mut CONN;
        c.snd_nxt = c.snd_nxt.wrapping_add(data.len() as u32);
    }
    true
}

/// Read the response body stream until the peer FINs, the sink is full,
/// or the budget expires. Every in-order segment is ACKed; out-of-order
/// segments are dropped (disclosed above). Returns bytes written to
/// `sink`.
pub fn tcp_read_to_end(sink: &mut [u8]) -> usize {
    let mut got = 0usize;
    let mut seg = [0u8; 2048];
    let mut finished = false;
    timer::poll_until(WAIT_BUDGET_US, || {
        while let Some((flags, seq, _ack, n)) = tcp_poll(&mut seg) {
            let expected = unsafe { (*&raw const CONN).rcv_nxt };
            if n > 0 && seq == expected {
                let take = n.min(sink.len() - got);
                sink[got..got + take].copy_from_slice(&seg[..take]);
                got += take;
                unsafe {
                    (*&raw mut CONN).rcv_nxt = expected.wrapping_add(n as u32);
                }
                tcp_send(TCP_ACK, &[]);
                if got >= sink.len() {
                    finished = true;
                    return true;
                }
            }
            if flags & TCP_FIN != 0 {
                unsafe {
                    let c = &mut *&raw mut CONN;
                    c.rcv_nxt = c.rcv_nxt.wrapping_add(1);
                }
                tcp_send(TCP_ACK | TCP_FIN, &[]);
                unsafe {
                    (*&raw mut CONN).established = false;
                }
                finished = true;
                return true;
            }
            if flags & TCP_RST != 0 {
                unsafe {
                    (*&raw mut CONN).established = false;
                }
                finished = true;
                return true;
            }
        }
        false
    });
    let _ = finished;
    got
}

/// HTTP/1.0 GET `path` from `ip:port`. Returns the body length written to
/// `body` (headers parsed and stripped), or None on connect/protocol
/// failure. HTTP/1.0 keeps it simple: no chunked encoding, connection
/// closes at end-of-body -- exactly the framing `tcp_read_to_end` gives.
pub fn http_get(ip: [u8; 4], port: u16, path: &str, host: &str, body: &mut [u8]) -> Option<usize> {
    if !tcp_connect(ip, port) {
        return None;
    }
    let mut req = [0u8; 512];
    let mut n = 0;
    for part in [b"GET " as &[u8], path.as_bytes(), b" HTTP/1.0\r\nHost: ", host.as_bytes(), b"\r\nUser-Agent: lingfu/1 (LingOS)\r\n\r\n"] {
        if n + part.len() > req.len() {
            return None;
        }
        req[n..n + part.len()].copy_from_slice(part);
        n += part.len();
    }
    if !tcp_write(&req[..n]) {
        return None;
    }
    // Response into a static-side scratch first (headers + body).
    static mut RESP: [u8; 128 * 1024] = [0; 128 * 1024];
    let resp = unsafe { &mut *&raw mut RESP };
    let total = tcp_read_to_end(resp);
    if total < 12 || &resp[..7] != b"HTTP/1." {
        return None;
    }
    // Status code.
    let code = (resp[9] - b'0') as u32 * 100 + (resp[10] - b'0') as u32 * 10 + (resp[11] - b'0') as u32;
    if code != 200 {
        return None;
    }
    // Find the blank line ending the headers.
    let mut body_start = 0;
    for i in 0..total.saturating_sub(3) {
        if &resp[i..i + 4] == b"\r\n\r\n" {
            body_start = i + 4;
            break;
        }
    }
    if body_start == 0 {
        return None;
    }
    let len = (total - body_start).min(body.len());
    body[..len].copy_from_slice(&resp[body_start..body_start + len]);
    Some(len)
}
