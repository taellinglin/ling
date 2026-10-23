//! Networking for programs: the host's `net_*` API (a 2-peer / small-lobby TCP
//! hub with newline-delimited messages, plus UDP-broadcast lobby discovery)
//! and `http_get` / `http_post_async` / `http_job_poll`, over the kernel's
//! netstack (TCP sockets, UDP, DNS, TLS). The semantics follow
//! `ling/src/runtime/net.rs`: `net_status` is 0 off, 2 up (hosting or joined),
//! 1 hosting with no client left; `net_recv_from` gives "id|msg";
//! `net_discover` gives "ip|info" lines seen in the last 5 s.
//!
//! Honest limits: `http_post_async` runs the request synchronously (there is
//! no background runtime in-kernel) and `http_job_poll` then returns its
//! result. The Pi has no network driver yet, so there everything reports
//! "not connected" (status 0, empty strings) until it does. (The Pi 5's wired
//! port is `arch::rp1_eth`.)

use crate::lingrt::{mk_str, num, to_f64, to_string, UNIT};
use alloc::string::String;

mod imp {
    use crate::drivers::netstack as ns;
    use alloc::collections::VecDeque;
    use alloc::format;
    use alloc::string::{String, ToString};
    use alloc::vec::Vec;

    const MAX_PEERS: usize = 8;
    /// TLS records are built in a 4 KiB buffer: request plaintext per record.
    const TLS_CHUNK: usize = 3800;

    struct Peer {
        handle: usize,
        id: u64,
        inbuf: Vec<u8>,
    }

    enum Role {
        Idle,
        Host { port: u16, next_id: u64 },
        Client,
    }

    struct Net {
        role: Role,
        peers: Vec<Peer>,
        inbound: VecDeque<(u64, String)>,
        events: VecDeque<String>,
        status: u8,
        /// (port, info, last broadcast ms) while announcing.
        announce: Option<(u16, String, u64)>,
        discover_port: Option<u16>,
        /// (ip, info, last seen ms)
        found: Vec<([u8; 4], String, u64)>,
        jobs: Vec<(String, String)>,
        next_job: u64,
        last_status: u16,
        /// Header block of the last response (`http_header`).
        last_headers: String,
    }

    static mut NET: Net = Net {
        role: Role::Idle,
        peers: Vec::new(),
        inbound: VecDeque::new(),
        events: VecDeque::new(),
        status: 0,
        announce: None,
        discover_port: None,
        found: Vec::new(),
        jobs: Vec::new(),
        next_job: 1,
        last_status: 0,
        last_headers: String::new(),
    };

    fn net() -> &'static mut Net {
        unsafe { &mut *(&raw mut NET) }
    }

    fn now_ms() -> u64 {
        crate::arch::timer::now_ms()
    }

    fn parse_ip(s: &str) -> Option<[u8; 4]> {
        let mut ip = [0u8; 4];
        let mut n = 0;
        for part in s.trim().split('.') {
            if n >= 4 {
                return None;
            }
            ip[n] = part.parse::<u8>().ok()?;
            n += 1;
        }
        if n == 4 {
            Some(ip)
        } else {
            None
        }
    }

    fn ip_str(ip: [u8; 4]) -> String {
        format!("{}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3])
    }

    pub fn host(port: u16) {
        close();
        let n = net();
        n.role = Role::Host { port, next_id: 1 };
        n.status = 2;
        pump();
    }

    pub fn join(ip: &str, port: u16) {
        close();
        let Some(addr) = parse_ip(ip).or_else(|| ns::dns_resolve(ip)) else { return };
        match ns::tcp_connect_h(addr, port, 3_000_000) {
            Some(h) => {
                let n = net();
                n.role = Role::Client;
                n.peers.push(Peer { handle: h, id: 0, inbuf: Vec::new() });
                n.status = 2;
            },
            None => net().status = 0,
        }
    }

    /// Accept new clients (host), read lines from every peer, drop closed
    /// peers, broadcast the lobby beacon, collect discovered hosts.
    pub fn pump() {
        let n = net();
        if let Role::Host { port, next_id } = &mut n.role {
            if n.peers.len() < MAX_PEERS {
                if let Some(h) = ns::tcp_listen_accept(*port, 1_000) {
                    let id = *next_id;
                    *next_id += 1;
                    n.peers.push(Peer { handle: h, id, inbuf: Vec::new() });
                    n.events.push_back(format!("join {id}"));
                    n.status = 2;
                }
            }
        } else {
            ns::pump_once();
        }
        let mut buf = [0u8; 2048];
        let mut i = 0;
        while i < n.peers.len() {
            let got = ns::tcp_read_h(n.peers[i].handle, &mut buf, 500);
            if got > 0 {
                n.peers[i].inbuf.extend_from_slice(&buf[..got]);
                while let Some(pos) = n.peers[i].inbuf.iter().position(|&b| b == b'\n') {
                    let line: Vec<u8> = n.peers[i].inbuf.drain(..=pos).collect();
                    let text = String::from_utf8_lossy(&line[..line.len() - 1]).trim_end_matches('\r').to_string();
                    let id = n.peers[i].id;
                    n.inbound.push_back((id, text));
                }
            } else if !ns::tcp_established_h(n.peers[i].handle) {
                let p = n.peers.remove(i);
                ns::tcp_close_h(p.handle);
                n.events.push_back(format!("leave {}", p.id));
                match n.role {
                    Role::Host { .. } => {
                        if n.peers.is_empty() {
                            n.status = 1;
                        }
                    },
                    Role::Client => {
                        n.status = 0;
                        n.role = Role::Idle;
                    },
                    Role::Idle => {},
                }
                continue;
            }
            i += 1;
        }
        if let Some((port, info, last)) = &mut n.announce {
            if now_ms().saturating_sub(*last) >= 1000 {
                ns::udp_broadcast(*port, *port, info.as_bytes());
                *last = now_ms();
            }
        }
        if n.discover_port.is_some() {
            let mut b = [0u8; 512];
            if let Some((ip, len)) = ns::udp_recv_beacon(&mut b) {
                let info = String::from_utf8_lossy(&b[..len]).replace(['\n', '\r'], " ");
                let now = now_ms();
                match n.found.iter_mut().find(|e| e.0 == ip) {
                    Some(e) => {
                        e.1 = info;
                        e.2 = now;
                    },
                    None => n.found.push((ip, info, now)),
                }
            }
        }
    }

    pub fn send(s: &str) {
        let mut line = s.as_bytes().to_vec();
        line.push(b'\n');
        for p in &net().peers {
            ns::tcp_write_h(p.handle, &line);
        }
    }

    pub fn send_to(id: u64, s: &str) {
        let mut line = s.as_bytes().to_vec();
        line.push(b'\n');
        if let Some(p) = net().peers.iter().find(|p| p.id == id) {
            ns::tcp_write_h(p.handle, &line);
        }
    }

    pub fn recv() -> String {
        pump();
        net().inbound.pop_front().map(|(_, m)| m).unwrap_or_default()
    }

    pub fn recv_from() -> String {
        pump();
        net().inbound.pop_front().map(|(id, m)| format!("{id}|{m}")).unwrap_or_default()
    }

    pub fn status() -> u8 {
        pump();
        net().status
    }

    pub fn close() {
        let n = net();
        for p in n.peers.drain(..) {
            ns::tcp_close_h(p.handle);
        }
        n.role = Role::Idle;
        n.status = 0;
        n.inbound.clear();
    }

    pub fn clients() -> String {
        let ids: Vec<String> = net().peers.iter().map(|p| p.id.to_string()).collect();
        ids.join(",")
    }

    pub fn client_count() -> usize {
        net().peers.len()
    }

    pub fn events() -> String {
        pump();
        let ev: Vec<String> = net().events.drain(..).collect();
        ev.join("\n")
    }

    pub fn announce(port: u16, info: &str) {
        net().announce = Some((port, String::from(info), 0));
        pump();
    }

    pub fn announce_stop() {
        net().announce = None;
    }

    pub fn discover(port: u16) -> String {
        let n = net();
        if n.discover_port != Some(port) {
            ns::udp_listen(port);
            n.discover_port = Some(port);
            n.found.clear();
        }
        pump();
        let now = now_ms();
        let mut out = String::new();
        for (ip, info, t) in &net().found {
            if now.saturating_sub(*t) < 5000 {
                out.push_str(&ip_str(*ip));
                out.push('|');
                out.push_str(info);
                out.push('\n');
            }
        }
        out
    }

    pub fn test_bind(port: u16) -> String {
        format!(
            "TCP bind 0.0.0.0:{port}: OK (host can listen)\nLAN IP (give to same-network joiners): {}\n",
            ip_str(ns::self_ip())
        )
    }

    // ── HTTP ─────────────────────────────────────────────────────────────────

    static mut RESP: [u8; 256 * 1024] = [0; 256 * 1024];

    fn parse_status(resp: &[u8]) -> u16 {
        // "HTTP/1.x NNN"
        if resp.len() < 12 || !resp.starts_with(b"HTTP/") {
            return 0;
        }
        let s = &resp[9..12];
        if s.iter().all(|b| b.is_ascii_digit()) {
            (s[0] - b'0') as u16 * 100 + (s[1] - b'0') as u16 * 10 + (s[2] - b'0') as u16
        } else {
            0
        }
    }

    /// Plain HTTP/1.0 request over a socket handle; the whole response lands
    /// in `out`.
    fn plain_request(host: &str, port: u16, method: &str, path: &str, content_type: &str, body: &[u8], out: &mut [u8]) -> usize {
        let Some(ip) = parse_ip(host).or_else(|| ns::dns_resolve(host)) else { return 0 };
        let Some(h) = ns::tcp_connect_h(ip, port, 5_000_000) else { return 0 };
        let mut req = format!("{method} {path} HTTP/1.0\r\nHost: {host}\r\nUser-Agent: LingOS\r\nConnection: close\r\n").into_bytes();
        if !body.is_empty() {
            req.extend_from_slice(format!("Content-Type: {content_type}\r\nContent-Length: {}\r\n", body.len()).as_bytes());
        }
        req.extend_from_slice(b"\r\n");
        req.extend_from_slice(body);
        let mut total = 0usize;
        if ns::tcp_write_h(h, &req) {
            while total < out.len() {
                let got = ns::tcp_read_h(h, &mut out[total..], 3_000_000);
                if got == 0 {
                    break;
                }
                total += got;
            }
        }
        ns::tcp_close_h(h);
        total
    }

    /// `(status, body)` of an HTTP(S) request; status 0 = no response at all.
    pub fn request(method: &str, url: &str, content_type: &str, body: &str) -> (u16, String) {
        let Some((host, port, path, https)) = ns::parse_url(url) else { return (0, String::new()) };
        let _ = ns::arp_gateway(); // warm the gateway's MAC so the first SYN is not lost
        let resp = unsafe { &mut *(&raw mut RESP) };
        let n = if https {
            // TLS progress goes to the serial log (a failed fetch says why there).
            // The first connect after boot sometimes fails (ARP/DNS warm-up,
            // the same retry lingfu does), so one retry.
            let mut log = |m: &[u8]| crate::console_write(m);
            let mut got = 0;
            for attempt in 0..2 {
                match crate::tls::https_request(host, port, method, path, content_type, body.as_bytes(), resp, TLS_CHUNK, &mut log) {
                    Ok(n) => {
                        got = n;
                        break;
                    },
                    Err(e) => {
                        crate::console_write(b"[http] ");
                        crate::console_write(e.as_bytes());
                        crate::console_write(if attempt == 0 { b" (retrying)\n" } else { b"\n" });
                    },
                }
            }
            got
        } else {
            plain_request(host, port, method, path, content_type, body.as_bytes(), resp)
        };
        if n == 0 {
            net().last_status = 0;
            net().last_headers.clear();
            return (0, String::new());
        }
        let status = parse_status(&resp[..n]);
        net().last_status = status;
        let off = crate::tls::http_body_offset(&resp[..n]);
        net().last_headers = String::from_utf8_lossy(&resp[..off.min(n)]).into_owned();
        let start = if off > 0 && off <= n { off } else { 0 };
        (status, String::from_utf8_lossy(&resp[start..n]).into_owned())
    }

    pub fn last_status() -> u16 {
        net().last_status
    }

    /// A header of the last response (case-insensitive name), "" if absent.
    pub fn last_header(name: &str) -> String {
        let want = name.trim().to_ascii_lowercase();
        for line in net().last_headers.lines() {
            if let Some((k, v)) = line.split_once(':') {
                if k.trim().to_ascii_lowercase() == want {
                    return String::from(v.trim());
                }
            }
        }
        String::new()
    }

    pub fn post_async(url: &str, body: &str, content_type: &str) -> String {
        let n = net();
        let id = format!("job{}", n.next_job);
        n.next_job += 1;
        let (status, out) = request("POST", url, content_type, body);
        let result = if status == 0 { format!("ERROR: no response from {url}") } else { out };
        net().jobs.push((id.clone(), result));
        id
    }

    pub fn job_poll(id: &str) -> String {
        let n = net();
        match n.jobs.iter().position(|(j, _)| j == id) {
            Some(i) => n.jobs.remove(i).1,
            None => String::new(),
        }
    }
}

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out: alloc::vec::Vec<u8> = alloc::vec::Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        match b[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < b.len() => {
                let hex = |c: u8| (c as char).to_digit(16);
                match (hex(b[i + 1]), hex(b[i + 2])) {
                    (Some(h), Some(l)) => {
                        out.push((h * 16 + l) as u8);
                        i += 2;
                    },
                    _ => out.push(b'%'),
                }
            },
            c => out.push(c),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// `(status, body)` of a GET -- the console's `curl`.
pub fn http_get(url: &str) -> (u16, String) {
    imp::request("GET", url, "", "")
}

/// The networking builtins; `None` for any other name.
pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    let s = |i: usize| args.get(i).map(|&v| to_string(v)).unwrap_or_default();
    let n = |i: usize, d: f64| args.get(i).map(|&v| to_f64(v)).unwrap_or(d);
    let name: &[u8] = match core::str::from_utf8(name).unwrap_or("") {
        "เน็ตโฮสต์" => b"net_host",
        "เน็ตจอย" => b"net_join",
        "เน็ตส่ง" => b"net_send",
        "เน็ตรับ" => b"net_recv",
        "เน็ตสถานะ" => b"net_status",
        "연결종료" => b"net_close",
        "เน็ตประกาศ" => b"net_announce",
        "เน็ตหยุดประกาศ" => b"net_announce_stop",
        "เน็ตค้นหา" => b"net_discover",
        "เน็ตทดสอบ" => b"net_test",
        "เว็บรับ" => b"http_get",
        _ => name,
    };
    Some(match name {
        b"net_host" => {
            imp::host(n(0, 7777.0) as u16);
            UNIT
        },
        b"net_join" => {
            let ip = if args.is_empty() { String::from("127.0.0.1") } else { s(0) };
            imp::join(&ip, n(1, 7777.0) as u16);
            UNIT
        },
        b"net_send" => {
            imp::send(&s(0));
            UNIT
        },
        b"net_send_to" => {
            imp::send_to(n(0, 0.0) as u64, &s(1));
            UNIT
        },
        b"net_recv" => mk_str(&imp::recv()),
        b"net_recv_from" => mk_str(&imp::recv_from()),
        b"net_status" => num(imp::status() as f64),
        b"net_close" => {
            imp::close();
            UNIT
        },
        b"net_clients" => mk_str(&imp::clients()),
        b"net_client_count" => num(imp::client_count() as f64),
        b"net_events" => mk_str(&imp::events()),
        b"net_announce" => {
            imp::announce(n(0, 7778.0) as u16, &s(1));
            UNIT
        },
        b"net_announce_stop" => {
            imp::announce_stop();
            UNIT
        },
        b"net_discover" => mk_str(&imp::discover(n(0, 7778.0) as u16)),
        b"net_test" => mk_str(&imp::test_bind(n(0, 7777.0) as u16)),
        b"http_get" => mk_str(&imp::request("GET", &s(0), "", "").1),
        b"http_get_status" => num(imp::last_status() as f64),
        b"http_header" => mk_str(&imp::last_header(&s(0))),
        b"http_post_async" => {
            let ct = if args.len() > 2 { s(2) } else { String::from("application/json") };
            mk_str(&imp::post_async(&s(0), &s(1), &ct))
        },
        b"http_job_poll" => mk_str(&imp::job_poll(&s(0))),
        b"url_decode" => mk_str(&url_decode(&s(0))),
        _ => return None,
    })
}
