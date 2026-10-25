//! Minimal SOCKS5 client (RFC 1928 CONNECT). Hostnames and `.onion` names
//! are sent to the proxy as ATYP=3 (domain), never resolved locally, so DNS
//! cannot leak around the proxy.
//!
//! We offer methods {USERNAME/PASSWORD, NO AUTH}. If the proxy picks
//! username/password (RFC 1929) we send fresh random credentials per
//! connection: Tor's IsolateSOCKSAuth (on by default) then puts every peer
//! connection on its own circuit, so one exit/guard observer cannot link our
//! peer connections by circuit.

use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr, TcpStream};
use std::time::{Duration, Instant};

use crate::lwp::crypto::kdf;

use crate::lwp::addr::{Host, NetAddr};
use crate::lwp::frame::read_exact_deadline;
use crate::lwp::wire::hex;

fn err(m: &'static str) -> io::Error {
    io::Error::other(m)
}

pub(crate) fn connect(proxy: SocketAddr, target: &NetAddr, timeout: Duration) -> io::Result<TcpStream> {
    let deadline = Instant::now() + timeout;
    let mut s = TcpStream::connect_timeout(&proxy, timeout)?;
    s.set_nodelay(true)?;
    s.set_write_timeout(Some(timeout))?;
    s.write_all(&[5, 2, 2, 0])?;
    let mut m = [0u8; 2];
    read_exact_deadline(&mut s, &mut m, deadline)?;
    if m[0] != 5 {
        return Err(err("socks5: bad version"));
    }
    match m[1] {
        0 => {}
        2 => {
            let mut c = [0u8; 16];
            kdf::random_bytes(&mut c);
            let (u, p) = (hex(&c[..8]), hex(&c[8..]));
            let mut a = vec![1, u.len() as u8];
            a.extend_from_slice(u.as_bytes());
            a.push(p.len() as u8);
            a.extend_from_slice(p.as_bytes());
            s.write_all(&a)?;
            let mut r = [0u8; 2];
            read_exact_deadline(&mut s, &mut r, deadline)?;
            if r[1] != 0 {
                return Err(err("socks5: auth rejected"));
            }
        }
        _ => return Err(err("socks5: no acceptable method")),
    }
    let mut req = vec![5, 1, 0];
    match &target.host {
        Host::Ip(IpAddr::V4(v4)) => {
            req.push(1);
            req.extend_from_slice(&v4.octets());
        }
        Host::Ip(IpAddr::V6(v6)) => {
            req.push(4);
            req.extend_from_slice(&v6.octets());
        }
        Host::Onion(h) | Host::Name(h) => {
            if h.len() > 255 {
                return Err(err("socks5: name too long"));
            }
            req.push(3);
            req.push(h.len() as u8);
            req.extend_from_slice(h.as_bytes());
        }
    }
    req.extend_from_slice(&target.port.to_be_bytes());
    s.write_all(&req)?;
    let mut rep = [0u8; 4];
    read_exact_deadline(&mut s, &mut rep, deadline)?;
    if rep[0] != 5 || rep[1] != 0 {
        return Err(err("socks5: connect failed"));
    }
    let skip = match rep[3] {
        1 => 4,
        4 => 16,
        3 => {
            let mut l = [0u8; 1];
            read_exact_deadline(&mut s, &mut l, deadline)?;
            l[0] as usize
        }
        _ => return Err(err("socks5: bad reply")),
    };
    let mut rest = vec![0u8; skip + 2];
    read_exact_deadline(&mut s, &mut rest, deadline)?;
    Ok(s)
}
