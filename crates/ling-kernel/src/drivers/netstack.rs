//! Minimal IPv4/TCP/HTTP client stack over the (now actually working --
//! see net_e1000's module doc) e1000 driver: enough real protocol to GET
//! a file from a real HTTP server, which is what `lingfu` needs to stop
//! being local-only. Static-IP SLIRP defaults (10.0.2.15/24, gateway
//! 10.0.2.2 -- which QEMU aliases to the host's loopback, so a plain
//! `python -m http.server` on the host is a reachable package repo).
//!
//! Honest scope, per driver-idiom: one TCP connection at a time, client
//! only, out-of-order segments reassembled via a small reorder buffer (a
//! duplicate ACK prompts the peer to fast-retransmit the gap), no local
//! retransmit queue (requests are sent once; against SLIRP's local, lossless
//! link this is fine and is disclosed rather than hidden), no window
//! management beyond a fixed
//! receive window, no TLS (a public HTTPS repo needs a real TLS stack --
//! see packages/README.md; the repo URL is plain HTTP by design until
//! then). This is a real wire-protocol implementation, not a simulation:
//! every byte below goes through the e1000 ring to a real peer.

use crate::arch::timer;
use crate::drivers::net_e1000 as nic;

/// Live IP config, resolved through the NIC's runtime config (a DHCP lease
/// if one was obtained, else the SLIRP fallback) — never the compile-time
/// const, so a fresh lease takes effect for every packet we build.
pub fn self_ip() -> [u8; 4] {
    nic::self_ip()
}
pub fn gateway_ip() -> [u8; 4] {
    nic::gateway_ip()
}
const MSS: usize = 1400;
// Advertised receive window. Sized to let several MSS-sized segments be in
// flight per round trip (large bodies -- the ~84KiB package avatars, bigger
// web pages -- otherwise trickle one window per RTT). 32KiB: fits the TLS rx
// buffer (64KiB) with room to spare, and stays under the e1000 RX ring's
// capacity (32 descriptors x 2KiB = 64KiB) so a brief non-drain can't drop
// segments. Halves the round trips for an 84KiB image versus a 16KiB window.
const RX_WINDOW: u16 = 32768;
/// Kernel-time poll budget. This bounds how long a *failed* fetch blocks
/// the (single-threaded) desktop loop -- a healthy fetch returns as soon
/// as the data arrives, well under this. Kept modest (8s) because the
/// whole UI is frozen while it runs: a dead host or an https redirect
/// must not hang the machine. Healthy DNS/TCP complete in well under a
/// second in practice (verified: curl example.com over the real
/// internet), so this only bites on genuine failure.
const WAIT_BUDGET_US: u64 = 8_000_000;

// -- Cooperative net lock --------------------------------------------------
// The netstack has one shared TCP connection (`CONN`) and shared UDP/DNS
// state, so two cooperative tasks must not run overlapping network
// transactions -- the desktop's background icon fetcher (a spawned task) and
// task 0 (browser/installer/terminal fetches). This is a re-entrant
// cooperative mutex: a task that already owns it can re-enter (https_get owns
// it while its own dns_resolve re-acquires), a different task yields until
// it's free. Safe without atomics because context switches only ever happen
// at a `yield_now` point, and the claim below has none between test and set.
static mut NET_OWNER: i64 = -1;
static mut NET_DEPTH: u32 = 0;

/// Acquire the net lock (re-entrant per task). Yields until free. x86_64 only
/// -- aarch64 has no cooperative kernel scheduler (no concurrent netstack
/// users there), so the lock is a no-op.
#[cfg(target_arch = "x86_64")]
pub fn net_acquire() {
    let me = crate::proc::sched::getpid() as i64;
    unsafe {
        while NET_OWNER != -1 && NET_OWNER != me {
            crate::proc::sched::yield_now();
        }
        NET_OWNER = me;
        NET_DEPTH += 1;
    }
}
#[cfg(not(target_arch = "x86_64"))]
pub fn net_acquire() {}

/// Release one level of the net lock.
#[cfg(target_arch = "x86_64")]
pub fn net_release() {
    unsafe {
        if NET_DEPTH > 0 {
            NET_DEPTH -= 1;
        }
        if NET_DEPTH == 0 {
            NET_OWNER = -1;
        }
    }
}
#[cfg(not(target_arch = "x86_64"))]
pub fn net_release() {}

/// RAII guard for `net_acquire`/`net_release` -- covers the many early returns
/// in a fetch path without a manual release at each.
pub struct NetGuard;
impl NetGuard {
    pub fn new() -> Self {
        net_acquire();
        NetGuard
    }
}
impl Drop for NetGuard {
    fn drop(&mut self) {
        net_release();
    }
}

static mut GW_MAC: [u8; 6] = [0; 6];
static mut GW_MAC_KNOWN: bool = false;
/// Timestamp (ms) of the last *failed* gateway ARP, or 0 for "never failed".
/// Without this, a dead gateway made every `dns_query` re-run the 8-second
/// `arp_selftest` — three servers per resolve froze the desktop for ~24s.
/// With it, one failure suppresses further ARP attempts for a cool-off
/// window, so an offline machine fails a lookup in well under a second while
/// still recovering automatically once the network comes back.
static mut GW_FAIL_AT_MS: u64 = 0;
/// How long to trust a cached ARP failure before trying the gateway again.
const GW_FAIL_COOLOFF_MS: u64 = 20_000;

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
/// if nothing else has yet (idempotent). A recent failure is cached (see
/// `GW_FAIL_AT_MS`) so a down network doesn't re-eat the 8s ARP timeout on
/// every call.
pub fn arp_gateway() -> Option<[u8; 6]> {
    if !nic::ensure_init() {
        return None;
    }
    unsafe {
        if GW_MAC_KNOWN {
            return Some(GW_MAC);
        }
        // Suppress re-ARP during the cool-off after a recent failure.
        if GW_FAIL_AT_MS != 0 && timer::now_ms().wrapping_sub(GW_FAIL_AT_MS) < GW_FAIL_COOLOFF_MS {
            return None;
        }
    }
    match nic::arp_selftest() {
        Some(mac) => unsafe {
            GW_MAC = mac;
            GW_MAC_KNOWN = true;
            GW_FAIL_AT_MS = 0;
            Some(mac)
        },
        None => unsafe {
            GW_FAIL_AT_MS = timer::now_ms().max(1);
            None
        },
    }
}

/// Drop the cached gateway MAC (and any failure cool-off) so the next send
/// re-ARPs. Called after a new IP config (DHCP lease / static IP) lands, in
/// case the gateway changed.
pub fn invalidate_gateway() {
    unsafe {
        GW_MAC_KNOWN = false;
        GW_FAIL_AT_MS = 0;
    }
}

/// Is `ip` on our own IPv4 subnet (per the current mask)?
fn same_subnet(ip: [u8; 4]) -> bool {
    let me = self_ip();
    let mask = nic::subnet_mask();
    (0..4).all(|i| (me[i] & mask[i]) == (ip[i] & mask[i]))
}

/// Ethernet next-hop MAC for `dst_ip`: ARP a same-subnet peer directly (LAN
/// peer-to-peer -- two LingOS machines on the same segment), otherwise route
/// via the gateway (the internet/SLIRP case). Broadcast/all-ones stays a
/// direct broadcast. The TCP layer routes every segment through this so a
/// same-subnet peer is reachable even on a segment with no gateway.
fn next_hop_mac(dst_ip: [u8; 4]) -> Option<[u8; 6]> {
    if dst_ip == [255, 255, 255, 255] {
        return Some([0xFF; 6]);
    }
    if same_subnet(dst_ip) {
        arp_resolve(dst_ip)
    } else {
        arp_gateway()
    }
}

// -- Multi-socket TCP ---------------------------------------------------------
// Several concurrent connections over the one NIC: a reserved client socket
// (the legacy `tcp_connect`/`tcp_write`/`tcp_read_*`/`tcp_close` API, used by
// TLS/browser/lingfu -- unchanged for those callers) plus server/accepted
// sockets via the handle API (`tcp_listen_accept` -> handle, `tcp_*_h`). A
// single RX pump (`pump`) reads the shared NIC queue and fans each packet to
// the matching socket by 4-tuple (or a listening socket by local port), so an
// SSH session and an HTTPS fetch can run at the same time. Crucially, reads and
// writes take the net lock only briefly (around one pump/segment), never for a
// whole session -- so flows interleave instead of one starving the others,
// which the old whole-session lock + single `CONN` couldn't do.

const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_ACK: u8 = 0x10;
const TCP_PSH: u8 = 0x08;

const MAX_SOCKS: usize = 4;
const SOCK_RX_CAP: usize = 64 * 1024;

#[derive(Clone, Copy, PartialEq)]
enum SockState {
    Free,
    Listen,
    SynSent,
    SynRcvd,
    Established,
    Closed,
}

struct Sock {
    state: SockState,
    remote_ip: [u8; 4],
    remote_port: u16,
    local_port: u16,
    snd_nxt: u32,
    rcv_nxt: u32,
    rx: [u8; SOCK_RX_CAP],
    rx_len: usize,
    got_fin: bool,
    /// A passively-opened (accepted) socket stays `Established` for the life of
    /// the connection, so `tcp_listen_accept` would keep re-matching and
    /// re-returning it. Set once it's been handed to a caller so the accept
    /// scan skips it thereafter -- otherwise a second "accept" runs a second
    /// handshake on the LIVE socket and closes it.
    accepted: bool,
}

const EMPTY_SOCK: Sock = Sock {
    state: SockState::Free,
    remote_ip: [0; 4],
    remote_port: 0,
    local_port: 0,
    snd_nxt: 0,
    rcv_nxt: 0,
    rx: [0; SOCK_RX_CAP],
    rx_len: 0,
    got_fin: false,
    accepted: false,
};

static mut SOCKS: [Sock; MAX_SOCKS] = [const { EMPTY_SOCK }; MAX_SOCKS];
/// The socket the legacy (implicit) client API operates on. `usize::MAX` = none.
static mut CLIENT_HANDLE: usize = usize::MAX;

fn sock_alloc() -> Option<usize> {
    unsafe {
        for i in 0..MAX_SOCKS {
            if SOCKS[i].state == SockState::Free {
                let s = &mut SOCKS[i];
                s.remote_ip = [0; 4];
                s.remote_port = 0;
                s.local_port = 0;
                s.snd_nxt = 0;
                s.rcv_nxt = 0;
                s.rx_len = 0;
                s.got_fin = false;
                s.accepted = false;
                return Some(i);
            }
        }
    }
    None
}

fn sock_free(idx: usize) {
    if idx >= MAX_SOCKS {
        return;
    }
    ooo_reset_sock(idx);
    unsafe {
        SOCKS[idx].state = SockState::Free;
        SOCKS[idx].rx_len = 0;
        SOCKS[idx].got_fin = false;
    }
}

/// `a` is at or after `b` in sequence space (mod 2^32).
fn seq_ge(a: u32, b: u32) -> bool {
    a.wrapping_sub(b) < 0x8000_0000
}

/// Build + transmit one TCP segment for socket `idx`. The advertised window is
/// that socket's remaining receive-buffer space (real flow control), capped at
/// `RX_WINDOW`.
fn sock_send(idx: usize, flags: u8, payload: &[u8]) -> bool {
    let (remote_ip, remote_port, local_port, snd_nxt, rcv_nxt, win) = unsafe {
        let s = &SOCKS[idx];
        let free = SOCK_RX_CAP.saturating_sub(s.rx_len);
        (
            s.remote_ip,
            s.remote_port,
            s.local_port,
            s.snd_nxt,
            s.rcv_nxt,
            free.min(RX_WINDOW as usize) as u16,
        )
    };
    // Next hop: a same-subnet peer (LAN peer-to-peer, e.g. two LingOS on a
    // QEMU socket segment) is ARP'd directly; anything off-subnet goes via the
    // gateway. This used to always use the gateway MAC, which can't reach a
    // peer on a segment that has no gateway.
    let Some(dmac) = next_hop_mac(remote_ip) else { return false };
    let opt_len = if flags & TCP_SYN != 0 { 4 } else { 0 };
    let tcp_len = 20 + opt_len + payload.len();
    let ip_len = 20 + tcp_len;
    let mut f = [0u8; 14 + 20 + 20 + 4 + MSS];
    let total = 14 + ip_len;
    f[0..6].copy_from_slice(&dmac);
    f[6..12].copy_from_slice(&nic::mac());
    f[12..14].copy_from_slice(&[0x08, 0x00]);
    let ip = &mut f[14..];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&(ip_len as u16).to_be_bytes());
    ip[4..6].copy_from_slice(&0x4C49u16.to_be_bytes());
    ip[6] = 0x40;
    ip[8] = 64;
    ip[9] = 6;
    ip[12..16].copy_from_slice(&self_ip());
    ip[16..20].copy_from_slice(&remote_ip);
    let ipsum = checksum(&ip[..20], 0);
    ip[10..12].copy_from_slice(&ipsum.to_be_bytes());
    let t = &mut ip[20..];
    t[0..2].copy_from_slice(&local_port.to_be_bytes());
    t[2..4].copy_from_slice(&remote_port.to_be_bytes());
    t[4..8].copy_from_slice(&snd_nxt.to_be_bytes());
    t[8..12].copy_from_slice(&rcv_nxt.to_be_bytes());
    t[12] = (((20 + opt_len) / 4) as u8) << 4;
    t[13] = flags;
    t[14..16].copy_from_slice(&win.to_be_bytes());
    if opt_len > 0 {
        t[20..24].copy_from_slice(&[0x02, 0x04, (MSS >> 8) as u8, (MSS & 0xFF) as u8]);
    }
    t[20 + opt_len..20 + opt_len + payload.len()].copy_from_slice(payload);
    let mut pseudo = [0u8; 12];
    pseudo[0..4].copy_from_slice(&self_ip());
    pseudo[4..8].copy_from_slice(&remote_ip);
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

// -- Out-of-order reassembly (per socket) -------------------------------------
// A small shared reorder pool, each slot tagged with its owning socket, so a
// large multi-segment flight (e.g. a TLS certificate chain) survives the normal
// reordering of the real internet instead of stalling on a retransmit timeout.
const OOO_SLOTS: usize = 24;
const OOO_SEG: usize = 1600;

struct OooSeg {
    used: bool,
    sock: usize,
    seq: u32,
    len: usize,
    data: [u8; OOO_SEG],
}

static mut OOO: [OooSeg; OOO_SLOTS] = [const {
    OooSeg { used: false, sock: 0, seq: 0, len: 0, data: [0; OOO_SEG] }
}; OOO_SLOTS];

fn ooo_reset_sock(idx: usize) {
    unsafe {
        for s in (&mut *&raw mut OOO).iter_mut() {
            if s.sock == idx {
                s.used = false;
            }
        }
    }
}

fn ooo_store(idx: usize, seq: u32, data: &[u8]) {
    if data.is_empty() || data.len() > OOO_SEG {
        return;
    }
    unsafe {
        let o = &mut *&raw mut OOO;
        for s in o.iter() {
            if s.used && s.sock == idx && s.seq == seq {
                return;
            }
        }
        for s in o.iter_mut() {
            if !s.used {
                s.used = true;
                s.sock = idx;
                s.seq = seq;
                s.len = data.len();
                s.data[..data.len()].copy_from_slice(data);
                return;
            }
        }
    }
}

/// Deliver any buffered segments now contiguous with the socket's `rcv_nxt` into
/// its receive buffer. Stops if the receive buffer is full (leaves the rest
/// buffered, unacked-advanced -- the reader drains and we resume next pump).
fn ooo_drain(idx: usize) {
    unsafe {
        let o = &mut *&raw mut OOO;
        loop {
            let expected = SOCKS[idx].rcv_nxt;
            let mut found = None;
            for (k, s) in o.iter().enumerate() {
                if s.used && s.sock == idx && s.seq == expected {
                    found = Some(k);
                    break;
                }
            }
            let Some(k) = found else { break };
            let len = o[k].len;
            let free = SOCK_RX_CAP.saturating_sub(SOCKS[idx].rx_len);
            if len > free {
                break; // no room yet; keep it buffered
            }
            let off = SOCKS[idx].rx_len;
            SOCKS[idx].rx[off..off + len].copy_from_slice(&o[k].data[..len]);
            SOCKS[idx].rx_len += len;
            SOCKS[idx].rcv_nxt = expected.wrapping_add(len as u32);
            o[k].used = false;
        }
    }
}

/// Process one received data segment into socket `idx`'s receive buffer:
/// in-order data is appended (+ any now-contiguous buffered data) and ACKed;
/// future data is buffered with a dup-ACK; a retransmit re-ACKs (delivering any
/// still-missing tail). Mirrors the old single-socket `recv_segment`.
fn recv_into(idx: usize, seq: u32, data: &[u8]) {
    unsafe {
        let expected = SOCKS[idx].rcv_nxt;
        if seq == expected {
            let free = SOCK_RX_CAP.saturating_sub(SOCKS[idx].rx_len);
            let take = data.len().min(free);
            let off = SOCKS[idx].rx_len;
            SOCKS[idx].rx[off..off + take].copy_from_slice(&data[..take]);
            SOCKS[idx].rx_len += take;
            SOCKS[idx].rcv_nxt = expected.wrapping_add(take as u32);
            if take == data.len() {
                ooo_drain(idx);
            }
            sock_send(idx, TCP_ACK, &[]);
        } else if seq_ge(seq, expected) {
            ooo_store(idx, seq, data);
            sock_send(idx, TCP_ACK, &[]);
        } else {
            let end = seq.wrapping_add(data.len() as u32);
            if seq_ge(end, expected) {
                let skip = expected.wrapping_sub(seq) as usize;
                if skip < data.len() {
                    let tail = &data[skip..];
                    let free = SOCK_RX_CAP.saturating_sub(SOCKS[idx].rx_len);
                    let take = tail.len().min(free);
                    let off = SOCKS[idx].rx_len;
                    SOCKS[idx].rx[off..off + take].copy_from_slice(&tail[..take]);
                    SOCKS[idx].rx_len += take;
                    SOCKS[idx].rcv_nxt = expected.wrapping_add(take as u32);
                    if take == tail.len() {
                        ooo_drain(idx);
                    }
                }
            }
            sock_send(idx, TCP_ACK, &[]);
        }
    }
}

/// Advance one connected socket's state machine for a received segment.
fn handle_conn(idx: usize, flags: u8, seq: u32, ack: u32, payload: &[u8]) {
    unsafe {
        if flags & TCP_RST != 0 {
            SOCKS[idx].state = SockState::Closed;
            return;
        }
        match SOCKS[idx].state {
            SockState::SynSent => {
                if flags & TCP_SYN != 0 && flags & TCP_ACK != 0 && ack == SOCKS[idx].snd_nxt {
                    SOCKS[idx].rcv_nxt = seq.wrapping_add(1);
                    SOCKS[idx].state = SockState::Established;
                    sock_send(idx, TCP_ACK, &[]);
                }
            },
            SockState::SynRcvd => {
                if flags & TCP_ACK != 0 && ack == SOCKS[idx].snd_nxt {
                    SOCKS[idx].state = SockState::Established;
                }
                if SOCKS[idx].state == SockState::Established && !payload.is_empty() {
                    recv_into(idx, seq, payload);
                }
            },
            SockState::Established => {
                if !payload.is_empty() {
                    recv_into(idx, seq, payload);
                }
                if flags & TCP_FIN != 0 {
                    SOCKS[idx].rcv_nxt = SOCKS[idx].rcv_nxt.wrapping_add(1);
                    sock_send(idx, TCP_ACK | TCP_FIN, &[]);
                    SOCKS[idx].got_fin = true;
                    SOCKS[idx].state = SockState::Closed;
                }
            },
            _ => {},
        }
    }
}

/// Route one received Ethernet frame to the matching socket (or a listener).
// A one-slot inbox for UDP datagrams to a single registered port (the
// Messenger discovery beacon). `route_packet` stashes them here so the ONE
// task that pumps the NIC (the messenger task) captures beacons as a side
// effect of servicing TCP/ARP -- the desktop's draw no longer needs to poll
// the NIC itself, which was racing that task for the shared receive queue and
// dropping frames (a lost SYN-ACK led to a duplicate connection that closed
// the live chat socket). Single slot is fine: beacons repeat every 2s.
static mut UDP_INBOX_PORT: u16 = 0;
static mut UDP_INBOX_IP: [u8; 4] = [0; 4];
static mut UDP_INBOX_BUF: [u8; 512] = [0; 512];
static mut UDP_INBOX_LEN: usize = 0;
static mut UDP_INBOX_READY: bool = false;

/// Register the UDP port whose datagrams `route_packet` should capture into
/// the inbox (read with [`udp_recv_beacon`]).
pub fn udp_listen(port: u16) {
    unsafe { UDP_INBOX_PORT = port };
}

/// Take the last captured datagram for the registered port, if any. Does not
/// touch the NIC -- safe to call from the UI thread without racing the pump.
pub fn udp_recv_beacon(out: &mut [u8]) -> Option<([u8; 4], usize)> {
    unsafe {
        if !UDP_INBOX_READY {
            return None;
        }
        let n = UDP_INBOX_LEN.min(out.len());
        out[..n].copy_from_slice(&UDP_INBOX_BUF[..n]);
        UDP_INBOX_READY = false;
        Some((UDP_INBOX_IP, n))
    }
}

fn route_packet(buf: &[u8]) {
    if buf.len() < 14 {
        return;
    }
    // ARP (ethertype 0x0806): answer requests for our own IP so a LAN peer can
    // reach us with no gateway/SLIRP to proxy ARP on our behalf, and learn the
    // sender's MAC either way. Without this, two LingOS machines on a bare L2
    // segment can discover each other (broadcast beacons) but never open a TCP
    // connection (unicast, needs the peer's MAC).
    if buf[12] == 0x08 && buf[13] == 0x06 {
        handle_arp(buf);
        return;
    }
    // IPv4 UDP to the registered discovery port -> stash in the one-slot inbox.
    if buf.len() >= 42 && buf[12] == 0x08 && buf[13] == 0x00 && buf[23] == 17 {
        let ihl = ((buf[14] & 0x0F) as usize) * 4;
        let u = 14 + ihl;
        let port = unsafe { UDP_INBOX_PORT };
        if port != 0 && u + 8 <= buf.len() && u16::from_be_bytes([buf[u + 2], buf[u + 3]]) == port {
            let ulen = u16::from_be_bytes([buf[u + 4], buf[u + 5]]) as usize;
            let n = ulen.saturating_sub(8).min(buf.len() - u - 8);
            unsafe {
                let m = n.min(UDP_INBOX_BUF.len());
                UDP_INBOX_BUF[..m].copy_from_slice(&buf[u + 8..u + 8 + m]);
                UDP_INBOX_LEN = m;
                UDP_INBOX_IP.copy_from_slice(&buf[26..30]);
                UDP_INBOX_READY = true;
            }
            return;
        }
    }
    if buf.len() < 54 || buf[12] != 0x08 || buf[13] != 0x00 || buf[23] != 6 {
        return; // not IPv4/TCP
    }
    let ihl = ((buf[14] & 0x0F) as usize) * 4;
    let ip_total = u16::from_be_bytes([buf[16], buf[17]]) as usize;
    let mut src_ip = [0u8; 4];
    src_ip.copy_from_slice(&buf[26..30]);
    let t = 14 + ihl;
    if t + 20 > buf.len() {
        return;
    }
    let src_port = u16::from_be_bytes([buf[t], buf[t + 1]]);
    let dst_port = u16::from_be_bytes([buf[t + 2], buf[t + 3]]);
    let seq = u32::from_be_bytes([buf[t + 4], buf[t + 5], buf[t + 6], buf[t + 7]]);
    let ack = u32::from_be_bytes([buf[t + 8], buf[t + 9], buf[t + 10], buf[t + 11]]);
    let doff = ((buf[t + 12] >> 4) as usize) * 4;
    let flags = buf[t + 13];
    let payload_off = t + doff;
    let payload_end = (14 + ip_total).min(buf.len());
    let payload: &[u8] = if payload_end > payload_off {
        &buf[payload_off..payload_end]
    } else {
        &[]
    };

    // Existing connection (any non-Free, non-Listen socket) by 4-tuple.
    let mut idx = None;
    unsafe {
        for i in 0..MAX_SOCKS {
            let s = &SOCKS[i];
            if s.state != SockState::Free
                && s.state != SockState::Listen
                && s.remote_ip == src_ip
                && s.remote_port == src_port
                && s.local_port == dst_port
            {
                idx = Some(i);
                break;
            }
        }
    }
    if let Some(i) = idx {
        handle_conn(i, flags, seq, ack, payload);
        return;
    }

    // A SYN to a listening socket -> spawn an accepted socket, SYN-ACK it.
    if flags & TCP_SYN != 0 && flags & TCP_ACK == 0 {
        let listening = unsafe {
            (0..MAX_SOCKS)
                .any(|i| SOCKS[i].state == SockState::Listen && SOCKS[i].local_port == dst_port)
        };
        if listening {
            if let Some(n) = sock_alloc() {
                unsafe {
                    let s = &mut SOCKS[n];
                    s.state = SockState::SynRcvd;
                    s.remote_ip = src_ip;
                    s.remote_port = src_port;
                    s.local_port = dst_port;
                    s.rcv_nxt = seq.wrapping_add(1);
                    s.snd_nxt = 0x4C49_4E47;
                }
                sock_send(n, TCP_SYN | TCP_ACK, &[]);
                unsafe { SOCKS[n].snd_nxt = SOCKS[n].snd_nxt.wrapping_add(1) };
            }
        }
    }
    // else: stray/unmatched -- ignore.
}

/// Drain all currently-available NIC packets, routing each to its socket.
/// Bounded per call so a busy link can't starve the caller's own progress.
fn pump() {
    let mut buf = [0u8; 2048];
    let mut n = 0;
    while let Some(len) = nic::receive(&mut buf) {
        route_packet(&buf[..len]);
        n += 1;
        if n >= 64 {
            break;
        }
    }
}

/// Copy up to `sink.len()` bytes of socket `handle`'s buffered receive data into
/// `sink`, shifting the remainder down. Returns bytes copied.
fn drain_rx(handle: usize, sink: &mut [u8]) -> usize {
    if handle >= MAX_SOCKS || sink.is_empty() {
        return 0;
    }
    unsafe {
        let s = &mut SOCKS[handle];
        let take = s.rx_len.min(sink.len());
        if take == 0 {
            return 0;
        }
        sink[..take].copy_from_slice(&s.rx[..take]);
        s.rx.copy_within(take..s.rx_len, 0);
        s.rx_len -= take;
        take
    }
}

// -- Handle API (servers: sshd, messenger) ------------------------------------

/// Passive open: ensure a listener on `port`, then wait up to `budget_us` for an
/// inbound connection to complete its handshake. Returns the accepted socket
/// handle, or None on timeout. The listener persists for the next accept.
pub fn tcp_listen_accept(port: u16, budget_us: u64) -> Option<usize> {
    {
        let _net = NetGuard::new();
        let have = unsafe {
            (0..MAX_SOCKS).any(|i| SOCKS[i].state == SockState::Listen && SOCKS[i].local_port == port)
        };
        if !have {
            let l = sock_alloc()?;
            unsafe {
                SOCKS[l].state = SockState::Listen;
                SOCKS[l].local_port = port;
            }
        }
    }
    let mut accepted = None;
    timer::poll_until(budget_us, || {
        let _net = NetGuard::new();
        pump();
        unsafe {
            for i in 0..MAX_SOCKS {
                if SOCKS[i].state == SockState::Established
                    && SOCKS[i].local_port == port
                    && !SOCKS[i].accepted
                {
                    // Hand it out exactly once: mark it so a later accept on the
                    // same port doesn't re-return this now-live socket and run a
                    // second handshake that would tear it down.
                    SOCKS[i].accepted = true;
                    accepted = Some(i);
                    return true;
                }
            }
        }
        false
    });
    accepted
}

/// Send application data on socket `handle` (one segment, <= MSS).
pub fn tcp_write_h(handle: usize, data: &[u8]) -> bool {
    if handle >= MAX_SOCKS || data.len() > MSS {
        return false;
    }
    let _net = NetGuard::new();
    if unsafe { SOCKS[handle].state } != SockState::Established {
        return false;
    }
    if !sock_send(handle, TCP_ACK | TCP_PSH, data) {
        return false;
    }
    unsafe {
        SOCKS[handle].snd_nxt = SOCKS[handle].snd_nxt.wrapping_add(data.len() as u32);
    }
    true
}

/// Read whatever arrives next on socket `handle` (returns as soon as any data is
/// buffered, or the peer closes, or `budget_us` elapses). Pumps the shared RX
/// under a brief lock each poll, so other sockets keep flowing meanwhile.
pub fn tcp_read_h(handle: usize, sink: &mut [u8], budget_us: u64) -> usize {
    if handle >= MAX_SOCKS {
        return 0;
    }
    let mut got = 0usize;
    timer::poll_until(budget_us, || {
        {
            let _net = NetGuard::new();
            pump();
            got = drain_rx(handle, sink);
        }
        let closed =
            unsafe { SOCKS[handle].state == SockState::Closed && SOCKS[handle].rx_len == 0 };
        got > 0 || closed
    });
    got
}

/// Best-effort close: FIN the connection and free the socket.
pub fn tcp_close_h(handle: usize) {
    if handle >= MAX_SOCKS {
        return;
    }
    {
        let _net = NetGuard::new();
        if unsafe { SOCKS[handle].state } == SockState::Established {
            let _ = sock_send(handle, TCP_FIN | TCP_ACK, &[]);
        }
    }
    sock_free(handle);
}

pub fn tcp_established_h(handle: usize) -> bool {
    handle < MAX_SOCKS && unsafe { SOCKS[handle].state == SockState::Established }
}

/// Active open on a dedicated socket (its own handle, distinct from the reserved
/// legacy client socket) so a caller like Messenger can dial a peer while TLS
/// and sshd use their own sockets. Returns the connected handle, or None.
pub fn tcp_connect_h(ip: [u8; 4], port: u16, budget_us: u64) -> Option<usize> {
    let h = {
        let _net = NetGuard::new();
        let h = sock_alloc()?;
        unsafe {
            let s = &mut SOCKS[h];
            s.state = SockState::SynSent;
            s.remote_ip = ip;
            s.remote_port = port;
            s.local_port = 49152 + (timer::now_ms() % 16000) as u16;
            s.snd_nxt = 0x4C49_4E47;
            s.rcv_nxt = 0;
        }
        if !sock_send(h, TCP_SYN, &[]) {
            sock_free(h);
            return None;
        }
        unsafe { SOCKS[h].snd_nxt = SOCKS[h].snd_nxt.wrapping_add(1) };
        h
    };
    let mut ok = false;
    timer::poll_until(budget_us, || {
        let _net = NetGuard::new();
        pump();
        match unsafe { SOCKS[h].state } {
            SockState::Established => {
                ok = true;
                true
            },
            SockState::Closed => true,
            _ => false,
        }
    });
    if ok {
        Some(h)
    } else {
        tcp_close_h(h);
        None
    }
}

// -- Legacy client API (TLS/browser/lingfu, one connection at a time) ---------

/// Open a client connection to `ip:port` (three-way handshake), reusing a single
/// reserved client socket. A still-open previous client connection is dropped.
pub fn tcp_connect(ip: [u8; 4], port: u16) -> bool {
    let h = {
        let _net = NetGuard::new();
        unsafe {
            if CLIENT_HANDLE != usize::MAX {
                sock_free(CLIENT_HANDLE);
                CLIENT_HANDLE = usize::MAX;
            }
        }
        let Some(h) = sock_alloc() else { return false };
        unsafe {
            let s = &mut SOCKS[h];
            s.state = SockState::SynSent;
            s.remote_ip = ip;
            s.remote_port = port;
            s.local_port = 49152 + (timer::now_ms() % 16000) as u16;
            s.snd_nxt = 0x4C49_4E47; // "LING" ISN, no security claim
            s.rcv_nxt = 0;
            CLIENT_HANDLE = h;
        }
        if !sock_send(h, TCP_SYN, &[]) {
            return false;
        }
        unsafe { SOCKS[h].snd_nxt = SOCKS[h].snd_nxt.wrapping_add(1) };
        h
    };
    let mut ok = false;
    timer::poll_until(WAIT_BUDGET_US, || {
        let _net = NetGuard::new();
        pump();
        match unsafe { SOCKS[h].state } {
            SockState::Established => {
                ok = true;
                true
            },
            SockState::Closed => true,
            _ => false,
        }
    });
    ok
}

pub fn tcp_write(data: &[u8]) -> bool {
    tcp_write_h(unsafe { CLIENT_HANDLE }, data)
}

pub fn tcp_read_some(sink: &mut [u8], budget_us: u64) -> usize {
    tcp_read_h(unsafe { CLIENT_HANDLE }, sink, budget_us)
}

/// Read the client stream until the peer FINs, the sink is full, or the budget
/// expires -- accumulating across pumps.
pub fn tcp_read_to_end(sink: &mut [u8]) -> usize {
    let h = unsafe { CLIENT_HANDLE };
    if h >= MAX_SOCKS {
        return 0;
    }
    let mut got = 0usize;
    timer::poll_until(WAIT_BUDGET_US, || {
        {
            let _net = NetGuard::new();
            pump();
            if got < sink.len() {
                got += drain_rx(h, &mut sink[got..]);
            }
        }
        let done = unsafe { SOCKS[h].state == SockState::Closed && SOCKS[h].rx_len == 0 };
        got >= sink.len() || done
    });
    got
}

pub fn tcp_close() {
    let h = unsafe { CLIENT_HANDLE };
    tcp_close_h(h);
    unsafe { CLIENT_HANDLE = usize::MAX };
}

/// Is the client connection currently established?
pub fn tcp_is_established() -> bool {
    let h = unsafe { CLIENT_HANDLE };
    h != usize::MAX && tcp_established_h(h)
}

// -- UDP + DNS ---------------------------------------------------------------

/// Send one UDP datagram (gateway-routed, like everything here).
/// Shared UDP/IPv4/Ethernet frame builder. `udp_send` (gateway-routed),
/// `udp_broadcast`, and `udp_send_to` (direct LAN peer) all funnel through
/// this -- only the destination MAC and destination IP differ.
fn build_and_send_udp(dst_mac: [u8; 6], dst_ip: [u8; 4], dst_port: u16, src_port: u16, payload: &[u8]) -> bool {
    if payload.len() > 512 {
        return false;
    }
    let udp_len = 8 + payload.len();
    let ip_len = 20 + udp_len;
    let mut f = [0u8; 14 + 20 + 8 + 512];
    f[0..6].copy_from_slice(&dst_mac);
    f[6..12].copy_from_slice(&nic::mac());
    f[12..14].copy_from_slice(&[0x08, 0x00]);
    let ip = &mut f[14..];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&(ip_len as u16).to_be_bytes());
    ip[6] = 0x40;
    ip[8] = 64;
    ip[9] = 17; // UDP
    ip[12..16].copy_from_slice(&self_ip());
    ip[16..20].copy_from_slice(&dst_ip);
    let ipsum = checksum(&ip[..20], 0);
    ip[10..12].copy_from_slice(&ipsum.to_be_bytes());
    let u = &mut ip[20..];
    u[0..2].copy_from_slice(&src_port.to_be_bytes());
    u[2..4].copy_from_slice(&dst_port.to_be_bytes());
    u[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    // UDP checksum optional over IPv4: zero = none. SLIRP accepts that.
    u[8..8 + payload.len()].copy_from_slice(payload);
    nic::transmit(&f[..(14 + ip_len).max(60)])
}

fn udp_send(dst_ip: [u8; 4], dst_port: u16, src_port: u16, payload: &[u8]) -> bool {
    let Some(gw) = arp_gateway() else { return false };
    build_and_send_udp(gw, dst_ip, dst_port, src_port, payload)
}

/// Send a UDP datagram to the local broadcast address (255.255.255.255) --
/// used for LAN peer discovery (see `drivers/messenger.rs`). The Ethernet
/// destination is the broadcast MAC, so every host on the LAN segment
/// receives it regardless of ARP state, unlike `udp_send` which always
/// routes via the gateway's MAC (wrong for reaching a same-subnet peer
/// directly, and useless for a destination with no single fixed address).
pub fn udp_broadcast(dst_port: u16, src_port: u16, payload: &[u8]) -> bool {
    if !nic::ensure_init() {
        return false;
    }
    build_and_send_udp([0xFF; 6], [255, 255, 255, 255], dst_port, src_port, payload)
}

/// Send a UDP datagram directly to a known LAN peer, ARP-resolving the
/// peer's own MAC (see `arp_resolve` -- `arp_gateway`'s cache is
/// gateway-only and can't be reused here).
pub fn udp_send_to(dst_ip: [u8; 4], dst_port: u16, src_port: u16, payload: &[u8]) -> bool {
    let Some(mac) = arp_resolve(dst_ip) else { return false };
    build_and_send_udp(mac, dst_ip, dst_port, src_port, payload)
}

/// Poll for a UDP datagram to `port`; payload into `out`.
fn udp_poll(port: u16, out: &mut [u8]) -> Option<usize> {
    udp_poll_from(port, out).map(|(_ip, n)| n)
}

/// Poll for a UDP datagram to `port` from any sender, also returning the
/// sender's IP -- needed to learn a discovered peer's address (LAN
/// discovery replies come from whoever answered a broadcast, not a
/// pre-known address) or to reply directly instead of re-broadcasting.
///
/// Shares the NIC's single receive queue with every other poller in this
/// driver (DNS, DHCP, TCP, ARP) -- same "one thing polls at a time"
/// contract the module doc already discloses for TCP; a caller waiting on
/// this should not be mid-poll on something else concurrently.
pub fn udp_poll_from(port: u16, out: &mut [u8]) -> Option<([u8; 4], usize)> {
    let mut buf = [0u8; 2048];
    let len = nic::receive(&mut buf)?;
    // Is this the UDP datagram on `port` we're after?
    if len >= 42 && buf[12] == 0x08 && buf[13] == 0x00 && buf[23] == 17 {
        let ihl = ((buf[14] & 0x0F) as usize) * 4;
        let u = 14 + ihl;
        if u + 8 <= len && u16::from_be_bytes([buf[u + 2], buf[u + 3]]) == port {
            let mut src_ip = [0u8; 4];
            src_ip.copy_from_slice(&buf[26..30]);
            let ulen = u16::from_be_bytes([buf[u + 4], buf[u + 5]]) as usize;
            let n = ulen.saturating_sub(8).min(out.len()).min(len - u - 8);
            out[..n].copy_from_slice(&buf[u + 8..u + 8 + n]);
            return Some((src_ip, n));
        }
    }
    // Not our datagram -- but the desktop's per-frame beacon poll shares the one
    // NIC receive queue with the background chat task's pump. Dropping whatever
    // else arrives (ARP requests, TCP for open sockets/listeners) would starve
    // that task's connections. Route it through the central handler instead so
    // no frame is lost to whichever consumer happened to dequeue it.
    route_packet(&buf[..len]);
    None
}

// -- Generic ARP resolution -------------------------------------------------
// `arp_gateway` above only ever resolves the gateway. LAN peer traffic
// needs the peer's OWN MAC (sending an Ethernet frame addressed to the
// gateway with a same-subnet destination IP doesn't reliably reach a LAN
// peer). Small fixed-size cache: a messenger's peer count is inherently
// small (friends + nearby discovered hosts), so a handful of entries is
// enough, no eviction policy needed yet.
const ARP_CACHE_LEN: usize = 16;
static mut ARP_CACHE_IP: [[u8; 4]; ARP_CACHE_LEN] = [[0; 4]; ARP_CACHE_LEN];
static mut ARP_CACHE_MAC: [[u8; 6]; ARP_CACHE_LEN] = [[0; 6]; ARP_CACHE_LEN];
static mut ARP_CACHE_USED: [bool; ARP_CACHE_LEN] = [false; ARP_CACHE_LEN];

/// Resolve `ip`'s MAC via ARP (cached after the first success). Shorter
/// timeout than `arp_gateway`'s (which affords 8s for CI/TSC-calibration
/// edge cases on a link known to exist): a messenger resolving several
/// LAN peers interactively can't afford to block the desktop loop that
/// long per unresolved address, and healthy LAN ARP replies arrive in ms.
pub fn arp_resolve(ip: [u8; 4]) -> Option<[u8; 6]> {
    if !nic::ensure_init() {
        return None;
    }
    unsafe {
        for i in 0..ARP_CACHE_LEN {
            if ARP_CACHE_USED[i] && ARP_CACHE_IP[i] == ip {
                return Some(ARP_CACHE_MAC[i]);
            }
        }
    }
    let my_mac = nic::mac();
    let mut frame = [0u8; 60];
    frame[0..6].copy_from_slice(&[0xFF; 6]);
    frame[6..12].copy_from_slice(&my_mac);
    frame[12..14].copy_from_slice(&[0x08, 0x06]);
    frame[14..16].copy_from_slice(&[0x00, 0x01]);
    frame[16..18].copy_from_slice(&[0x08, 0x00]);
    frame[18] = 6;
    frame[19] = 4;
    frame[20..22].copy_from_slice(&[0x00, 0x01]);
    frame[22..28].copy_from_slice(&my_mac);
    frame[28..32].copy_from_slice(&self_ip());
    frame[32..38].copy_from_slice(&[0x00; 6]);
    frame[38..42].copy_from_slice(&ip);
    if !nic::transmit(&frame) {
        return None;
    }
    let mut buf = [0u8; 2048];
    let mut found = None;
    timer::poll_until(1_500_000, || {
        if let Some(len) = nic::receive(&mut buf) {
            if len >= 42 && buf[12] == 0x08 && buf[13] == 0x06 {
                if buf[20] == 0x00 && buf[21] == 0x02 && buf[28..32] == ip {
                    let mut reply_mac = [0u8; 6];
                    reply_mac.copy_from_slice(&buf[22..28]);
                    found = Some(reply_mac);
                    return true;
                }
                // Any other ARP frame (notably the peer's OWN request, when it
                // is dialing us at the same moment) -- answer + cache it, so a
                // simultaneous two-way connect doesn't deadlock with both sides
                // waiting for a reply neither is free to send.
                handle_arp(&buf[..len]);
            }
        }
        false
    });
    if let Some(mac) = found {
        arp_cache_put(ip, mac);
    }
    found
}

/// DNS servers, tried in order with failover. Primary is a LAN resolver
/// (the requested 192.168.0.2); secondary is Cloudflare 1.1.1.1 -- a
/// cutting-edge, privacy-first public resolver (not Google/Microsoft/
/// Yahoo), reachable through QEMU *and* VirtualBox NAT, so names resolve
/// even when the LAN resolver is absent. Both overridable at runtime via
/// `set_dns` (the desktop reads lingfs `/dns` at boot: two ip lines).
/// Insert or refresh an `ip -> mac` mapping in the ARP cache.
fn arp_cache_put(ip: [u8; 4], mac: [u8; 6]) {
    unsafe {
        for i in 0..ARP_CACHE_LEN {
            if ARP_CACHE_USED[i] && ARP_CACHE_IP[i] == ip {
                ARP_CACHE_MAC[i] = mac;
                return;
            }
        }
        for i in 0..ARP_CACHE_LEN {
            if !ARP_CACHE_USED[i] {
                ARP_CACHE_IP[i] = ip;
                ARP_CACHE_MAC[i] = mac;
                ARP_CACHE_USED[i] = true;
                return;
            }
        }
    }
}

/// Handle an inbound ARP frame: cache the sender's mapping, and if it's a
/// request for our own IP, transmit a reply. This is the passive counterpart
/// to `arp_resolve` -- required for a LAN peer to reach us, since on a bare L2
/// segment (e.g. two LingOS machines on a QEMU socket/mcast link) there's no
/// gateway or SLIRP to answer ARP for us.
fn handle_arp(buf: &[u8]) {
    // Ethernet+ARP header: htype(2) ptype(2) hlen plen oper(2) sha(6) spa(4)
    // tha(6) tpa(4), starting at byte 14. Require Ethernet/IPv4 ARP.
    if buf.len() < 42 || buf[14] != 0x00 || buf[15] != 0x01 || buf[16] != 0x08 || buf[17] != 0x00 {
        return;
    }
    let oper = u16::from_be_bytes([buf[20], buf[21]]);
    let mut sha = [0u8; 6];
    sha.copy_from_slice(&buf[22..28]);
    let mut spa = [0u8; 4];
    spa.copy_from_slice(&buf[28..32]);
    let mut tpa = [0u8; 4];
    tpa.copy_from_slice(&buf[38..42]);
    if spa != [0; 4] {
        arp_cache_put(spa, sha); // learn from requests and replies alike
    }
    if oper != 1 || tpa != self_ip() {
        return; // only reply to a request aimed at us
    }
    let my_mac = nic::mac();
    let mut f = [0u8; 42];
    f[0..6].copy_from_slice(&sha); // to the requester
    f[6..12].copy_from_slice(&my_mac);
    f[12..14].copy_from_slice(&[0x08, 0x06]);
    f[14..16].copy_from_slice(&[0x00, 0x01]); // htype Ethernet
    f[16..18].copy_from_slice(&[0x08, 0x00]); // ptype IPv4
    f[18] = 6;
    f[19] = 4;
    f[20..22].copy_from_slice(&[0x00, 0x02]); // oper = reply
    f[22..28].copy_from_slice(&my_mac);
    f[28..32].copy_from_slice(&self_ip());
    f[32..38].copy_from_slice(&sha);
    f[38..42].copy_from_slice(&spa);
    let _ = nic::transmit(&f);
}

pub const PRIMARY_DNS: [u8; 4] = [192, 168, 0, 2];
pub const SECONDARY_DNS: [u8; 4] = [1, 1, 1, 1]; // Cloudflare
static mut DNS1: [u8; 4] = PRIMARY_DNS;
static mut DNS2: [u8; 4] = SECONDARY_DNS;

pub fn set_dns(primary: [u8; 4], secondary: [u8; 4]) {
    unsafe {
        DNS1 = primary;
        DNS2 = secondary;
    }
}

pub fn dns_servers() -> ([u8; 4], [u8; 4]) {
    unsafe { (DNS1, DNS2) }
}

/// A short budget per DNS server so failover is snappy on VirtualBox,
/// where the LAN primary (192.168.0.2) isn't on the NAT and would
/// otherwise stall every lookup before Cloudflare answers.
const DNS_BUDGET_US: u64 = 1_500_000;

/// One DNS A-query to a specific server. Returns the first A record, or
/// None on timeout / NXDOMAIN.
fn dns_query(server: [u8; 4], host: &str) -> Option<[u8; 4]> {
    let mut q = [0u8; 300];
    q[0] = b'L';
    q[1] = b'Q';
    q[2] = 0x01; // RD
    q[5] = 1; // QDCOUNT
    let mut n = 12;
    for label in host.split('.') {
        if label.is_empty() || label.len() > 63 || n + label.len() + 1 > 280 {
            return None;
        }
        q[n] = label.len() as u8;
        n += 1;
        q[n..n + label.len()].copy_from_slice(label.as_bytes());
        n += label.len();
    }
    q[n] = 0;
    n += 1;
    q[n..n + 4].copy_from_slice(&[0, 1, 0, 1]); // QTYPE A, QCLASS IN
    n += 4;

    let src_port = 33000 + (timer::now_ms() % 8000) as u16;
    if !udp_send(server, 53, src_port, &q[..n]) {
        return None;
    }
    let mut resp = [0u8; 512];
    let mut found: Option<[u8; 4]> = None;
    timer::poll_until(DNS_BUDGET_US, || {
        let Some(rlen) = udp_poll(src_port, &mut resp) else { return false };
        if rlen < 12 || resp[0] != b'L' || resp[1] != b'Q' {
            return false;
        }
        let ancount = u16::from_be_bytes([resp[6], resp[7]]) as usize;
        if ancount == 0 {
            return true; // authoritative "no such name" -- stop waiting
        }
        let mut p = 12;
        while p < rlen && resp[p] != 0 {
            p += resp[p] as usize + 1;
        }
        p += 5; // 0 terminator + QTYPE/QCLASS
        for _ in 0..ancount {
            if p + 12 > rlen {
                return true;
            }
            if resp[p] & 0xC0 == 0xC0 {
                p += 2;
            } else {
                while p < rlen && resp[p] != 0 {
                    p += resp[p] as usize + 1;
                }
                p += 1;
            }
            let rtype = u16::from_be_bytes([resp[p], resp[p + 1]]);
            let rdlen = u16::from_be_bytes([resp[p + 8], resp[p + 9]]) as usize;
            p += 10;
            if rtype == 1 && rdlen == 4 && p + 4 <= rlen {
                found = Some([resp[p], resp[p + 1], resp[p + 2], resp[p + 3]]);
                return true;
            }
            p += rdlen;
        }
        true
    });
    found
}

/// The NAT built-in resolver (QEMU SLIRP *and* VirtualBox NAT both expose
/// their DNS proxy here). Used as a guaranteed last-resort fallback: a LAN
/// resolver like 192.168.0.2 or an external one like 1.1.1.1 isn't always
/// reachable from inside a NAT, but 10.0.2.3 always is -- so resolution
/// works out of the box on both hypervisors while still honoring the
/// configured servers first on real hardware/LANs.
pub const NAT_RESOLVER: [u8; 4] = [10, 0, 2, 3];

/// Resolve `host` to an IPv4 address: dotted-quad short-circuits; otherwise
/// a real DNS A query -- configured primary, then secondary, then the NAT
/// resolver as a guaranteed fallback.
pub fn dns_resolve(host: &str) -> Option<[u8; 4]> {
    if let Some(ip) = parse_ipv4(host) {
        return Some(ip);
    }
    // Re-entrant: no-op cost when https_get/http_get already hold the lock,
    // real acquisition when called standalone (terminal `dns`) concurrently
    // with the background icon fetcher.
    let _net = NetGuard::new();
    // Resolve the gateway MAC ONCE. Every server below routes through the
    // same gateway, so if the gateway is unreachable there's no point paying
    // the ARP timeout three times over -- bail immediately. `arp_gateway`
    // caches the failure, so this returns in microseconds on an offline box
    // (the old path froze the desktop for ~24s: 3 servers x 8s ARP each).
    if arp_gateway().is_none() {
        return None;
    }
    let (d1, d2) = unsafe { (DNS1, DNS2) };
    for server in [d1, d2, NAT_RESOLVER] {
        if server == [0, 0, 0, 0] {
            continue;
        }
        if let Some(ip) = dns_query(server, host) {
            return Some(ip);
        }
    }
    None
}

// -- DHCP client -------------------------------------------------------------

/// What we parse out of a DHCP OFFER/ACK. Any field left zero means the
/// server didn't send that option.
#[derive(Clone, Copy)]
struct DhcpReply {
    yiaddr: [u8; 4],
    mask: [u8; 4],
    router: [u8; 4],
    dns: [u8; 4],
    server_id: [u8; 4],
    msg_type: u8,
}
impl DhcpReply {
    const EMPTY: Self = Self {
        yiaddr: [0; 4],
        mask: [0; 4],
        router: [0; 4],
        dns: [0; 4],
        server_id: [0; 4],
        msg_type: 0,
    };
}

/// Build and broadcast one BOOTREQUEST (op=1) with the given options. Uses
/// raw frames (not `udp_send`) because DHCP happens before we have an IP or
/// a known gateway: Ethernet dst = broadcast, IP src 0.0.0.0 -> 255.255.
/// 255.255, UDP 68 -> 67, and the BOOTP broadcast flag set so the server
/// broadcasts its reply (we can't receive a unicast to an IP we don't hold
/// yet).
fn dhcp_send(mac: [u8; 6], xid: u32, opts: &[u8]) -> bool {
    let dhcp_len = 240 + opts.len();
    let udp_len = 8 + dhcp_len;
    let ip_len = 20 + udp_len;
    let total = 14 + ip_len;
    let mut f = [0u8; 14 + 20 + 8 + 240 + 40];
    if total > f.len() {
        return false;
    }
    f[0..6].copy_from_slice(&[0xFF; 6]);
    f[6..12].copy_from_slice(&mac);
    f[12..14].copy_from_slice(&[0x08, 0x00]);
    let ip = &mut f[14..];
    ip[0] = 0x45;
    ip[2..4].copy_from_slice(&(ip_len as u16).to_be_bytes());
    ip[6] = 0x40;
    ip[8] = 64;
    ip[9] = 17; // UDP
    ip[12..16].copy_from_slice(&[0, 0, 0, 0]);
    ip[16..20].copy_from_slice(&[255, 255, 255, 255]);
    let ipsum = checksum(&ip[..20], 0);
    ip[10..12].copy_from_slice(&ipsum.to_be_bytes());
    let u = &mut ip[20..];
    u[0..2].copy_from_slice(&68u16.to_be_bytes());
    u[2..4].copy_from_slice(&67u16.to_be_bytes());
    u[4..6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    // UDP checksum optional over IPv4 -> left zero (SLIRP and real servers
    // accept it), same as udp_send.
    let d = &mut u[8..];
    d[0] = 1; // op: BOOTREQUEST
    d[1] = 1; // htype: Ethernet
    d[2] = 6; // hlen
    d[4..8].copy_from_slice(&xid.to_be_bytes());
    d[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // flags: broadcast
    d[28..34].copy_from_slice(&mac); // chaddr
    d[236..240].copy_from_slice(&[99, 130, 83, 99]); // DHCP magic cookie
    d[240..240 + opts.len()].copy_from_slice(opts);
    nic::transmit(&f[..total.max(60)])
}

/// Poll (bounded) for a broadcast BOOTREPLY matching `xid` and, if given, a
/// specific DHCP message type (2=OFFER, 5=ACK). Fills `out` and returns true
/// on a match.
fn dhcp_await(xid: u32, want_type: u8, out: &mut DhcpReply, budget_us: u64) -> bool {
    let mut buf = [0u8; 2048];
    let mut done = false;
    timer::poll_until(budget_us, || {
        let Some(len) = nic::receive(&mut buf) else { return false };
        if len < 14 + 20 + 8 + 240 || buf[12] != 0x08 || buf[13] != 0x00 || buf[23] != 17 {
            return false;
        }
        let ihl = ((buf[14] & 0x0F) as usize) * 4;
        let u = 14 + ihl;
        if u + 8 > len || u16::from_be_bytes([buf[u + 2], buf[u + 3]]) != 68 {
            return false; // not to our DHCP client port
        }
        let d = u + 8;
        if d + 240 > len
            || buf[d] != 2 // BOOTREPLY
            || buf[d + 4..d + 8] != xid.to_be_bytes()
            || buf[d + 236..d + 240] != [99, 130, 83, 99]
        {
            return false;
        }
        let mut r = DhcpReply::EMPTY;
        r.yiaddr.copy_from_slice(&buf[d + 16..d + 20]);
        let mut p = d + 240;
        while p + 1 < len {
            let code = buf[p];
            if code == 255 {
                break;
            }
            if code == 0 {
                p += 1;
                continue;
            }
            let l = buf[p + 1] as usize;
            if p + 2 + l > len {
                break;
            }
            let v = &buf[p + 2..p + 2 + l];
            match code {
                53 if l >= 1 => r.msg_type = v[0],
                1 if l >= 4 => r.mask.copy_from_slice(&v[..4]),
                3 if l >= 4 => r.router.copy_from_slice(&v[..4]),
                6 if l >= 4 => r.dns.copy_from_slice(&v[..4]),
                54 if l >= 4 => r.server_id.copy_from_slice(&v[..4]),
                _ => {},
            }
            p += 2 + l;
        }
        if want_type != 0 && r.msg_type != want_type {
            return false;
        }
        *out = r;
        done = true;
        true
    });
    done
}

/// Configure our IPv4 via DHCP (DISCOVER -> OFFER -> REQUEST -> ACK). This is
/// what makes DNS work on a VirtualBox *bridged*/*host-only* adapter: the
/// lease's IP/mask/router/DNS come from the LAN's real server, so ARP finds
/// the real gateway and off-subnet lookups (1.1.1.1, or the LAN resolver in
/// option 6) actually route -- instead of the wrong 10.0.2.x SLIRP defaults.
///
/// When no DHCP server answered, derive a static IPv4 from the MAC so two
/// LingOS machines on the *same* DHCP-less L2 segment don't both sit on the
/// SLIRP default `10.0.2.15`. This is exactly the QEMU `-netdev socket`
/// (or `mcast`) peer-to-peer setup used to test two instances against each
/// other: there's no DHCP there, but each guest has a distinct MAC. Keeps the
/// `10.0.2.0/24` net and `.2` gateway convention; the last octet is
/// `10 + (mac[5] & 0x7f)`, so it stays clear of `.0/.1/.2/.255` and differs
/// whenever the low MAC bytes differ. A real network's DHCP lease overrides
/// all of this (this only runs when `dhcp_configure` returned false).
pub fn apply_static_ip_from_mac() {
    let mac = nic::mac();
    let last = 10u8.wrapping_add(mac[5] & 0x7f);
    nic::set_ip_config([10, 0, 2, last], [255, 255, 255, 0], [10, 0, 2, 2]);
    invalidate_gateway();
}

/// Best-effort and bounded: on a network with no DHCP server it returns
/// false after the timeouts and the static SLIRP fallback stays in place, so
/// plain QEMU and manual static-IP setups still work. Runs once at boot.
pub fn dhcp_configure() -> bool {
    if !nic::ensure_init() {
        return false;
    }
    let mac = nic::mac();
    let xid = (timer::now_ms() as u32) ^ ((mac[4] as u32) << 8) ^ (mac[5] as u32) ^ 0x4C69_6E00;

    let mut tries = 0;
    while tries < 2 {
        tries += 1;
        let disc: &[u8] = &[53, 1, 1, 55, 4, 1, 3, 6, 15, 255];
        if !dhcp_send(mac, xid, disc) {
            return false;
        }
        let mut offer = DhcpReply::EMPTY;
        if !dhcp_await(xid, 2, &mut offer, 2_000_000) || offer.yiaddr == [0; 4] {
            continue;
        }
        // REQUEST the offered address (SELECTING state: ciaddr stays 0, the
        // wanted IP goes in option 50, and we echo the server id in 54).
        let mut req = [0u8; 32];
        let mut n = 0;
        req[n..n + 3].copy_from_slice(&[53, 1, 3]);
        n += 3;
        req[n..n + 2].copy_from_slice(&[50, 4]);
        n += 2;
        req[n..n + 4].copy_from_slice(&offer.yiaddr);
        n += 4;
        req[n..n + 2].copy_from_slice(&[54, 4]);
        n += 2;
        req[n..n + 4].copy_from_slice(&offer.server_id);
        n += 4;
        req[n..n + 5].copy_from_slice(&[55, 3, 1, 3, 6]);
        n += 5;
        req[n] = 255;
        n += 1;
        if !dhcp_send(mac, xid, &req[..n]) {
            return false;
        }
        // Some servers' ACK omits fields already sent in the OFFER; fall back
        // to the OFFER's values when the ACK is silent (or never arrives).
        let mut ack = DhcpReply::EMPTY;
        if !dhcp_await(xid, 5, &mut ack, 2_000_000) {
            ack = offer;
        }
        let ip = if ack.yiaddr != [0; 4] { ack.yiaddr } else { offer.yiaddr };
        let gw = if ack.router != [0; 4] { ack.router } else { offer.router };
        if ip == [0; 4] || gw == [0; 4] {
            continue; // unusable lease -- keep the static fallback
        }
        let mask = if ack.mask != [0; 4] {
            ack.mask
        } else if offer.mask != [0; 4] {
            offer.mask
        } else {
            [255, 255, 255, 0]
        };
        nic::set_ip_config(ip, mask, gw);
        invalidate_gateway();
        // Prefer the lease's DNS as primary, keep Cloudflare as secondary.
        let dns = if ack.dns != [0; 4] { ack.dns } else { offer.dns };
        if dns != [0; 4] {
            set_dns(dns, SECONDARY_DNS);
        }
        return true;
    }
    false
}

fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut ip = [0u8; 4];
    let mut part = 0;
    let mut acc: u32 = 0;
    let mut digits = 0;
    for b in s.bytes() {
        match b {
            b'0'..=b'9' => {
                acc = acc * 10 + (b - b'0') as u32;
                digits += 1;
                if acc > 255 || digits > 3 {
                    return None;
                }
            },
            b'.' => {
                if digits == 0 || part >= 3 {
                    return None;
                }
                ip[part] = acc as u8;
                part += 1;
                acc = 0;
                digits = 0;
            },
            _ => return None,
        }
    }
    if part == 3 && digits > 0 {
        ip[3] = acc as u8;
        Some(ip)
    } else {
        None
    }
}

/// Parse `http://host[:port]/path` (https is recognized and REFUSED with
/// None-plus-reason -- no TLS stack yet, said out loud rather than
/// silently downgraded). Returns (host, port, path-start-index, is_tls).
pub fn parse_url(url: &str) -> Option<(&str, u16, &str, bool)> {
    let (rest, tls) = if let Some(r) = url.strip_prefix("http://") {
        (r, false)
    } else if let Some(r) = url.strip_prefix("https://") {
        (r, true)
    } else {
        (url, false)
    };
    let (hostport, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, "/"),
    };
    let (host, port) = match hostport.find(':') {
        Some(i) => {
            let mut p: u32 = 0;
            for b in hostport[i + 1..].bytes() {
                if !b.is_ascii_digit() {
                    return None;
                }
                p = p * 10 + (b - b'0') as u32;
                if p > 65535 {
                    return None;
                }
            }
            (&hostport[..i], p as u16)
        },
        // No explicit port: default by scheme (443 for https, 80 for http).
        // Returning the scheme default here -- rather than a fixed 80 -- means
        // every caller connects to the right port even if it forgets to
        // special-case the TLS default. (A TLS handshake against port 80 gets
        // a plaintext HTTP 400 back, which the record parser then chokes on.)
        None => (hostport, if tls { 443 } else { 80 }),
    };
    if host.is_empty() {
        return None;
    }
    Some((host, port, path, tls))
}

/// HTTP/1.0 GET `path` from `ip:port`, returning the FULL raw response
/// (status line + headers + body, whatever the status code) into `out`.
/// Unlike `http_get`, this doesn't filter on status 200 or strip headers --
/// callers that need to see a redirect (a `Location` header on a 3xx) use
/// this directly and parse the header block themselves (see
/// `drivers::horizon`'s `fetch_raw`). `http_get` below stays the simple
/// 200-only/body-only convenience wrapper existing callers already rely on.
pub fn http_get_raw(ip: [u8; 4], port: u16, path: &str, host: &str, out: &mut [u8]) -> Option<usize> {
    let _net = NetGuard::new();
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
    static mut RAW_RESP: [u8; 128 * 1024] = [0; 128 * 1024];
    let resp = unsafe { &mut *&raw mut RAW_RESP };
    let total = tcp_read_to_end(resp);
    if total < 12 || &resp[..7] != b"HTTP/1." {
        return None;
    }
    let len = total.min(out.len());
    out[..len].copy_from_slice(&resp[..len]);
    Some(len)
}

/// HTTP/1.0 GET `path` from `ip:port`. Returns the body length written to
/// `body` (headers parsed and stripped), or None on connect/protocol
/// failure. HTTP/1.0 keeps it simple: no chunked encoding, connection
/// closes at end-of-body -- exactly the framing `tcp_read_to_end` gives.
pub fn http_get(ip: [u8; 4], port: u16, path: &str, host: &str, body: &mut [u8]) -> Option<usize> {
    let _net = NetGuard::new();
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
