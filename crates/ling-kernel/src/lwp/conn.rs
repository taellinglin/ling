//! A kernel TCP connection for LWP-1: the host's blocking `TcpStream`,
//! rebuilt over the netstack's handle API so that every wait yields the
//! coroutine (the desktop and the other peers keep running). Reads are
//! buffered here, so a connection that is busy writing a big frame keeps
//! draining its socket (two LingOS nodes writing to each other at once can
//! never wedge on full receive windows).

use alloc::collections::BTreeSet;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::cell::{Cell, RefCell};

use crate::drivers::netstack as ns;
use crate::lwp::frame::{valid_wire_len, Io, IoError};
use crate::lwp::sys;

/// The netstack's segment size (`tcp_write_h` takes at most this much).
const SEG: usize = 1400;
/// Stop pulling from the socket past this much unread data (TCP then
/// back-pressures the peer): two maximum-size frames.
const MAX_BUFFERED: usize = 9 << 20;

/// What every connection of one network shares: its stop flag (shutdown
/// interrupts every wait at once) and the set of sockets it holds (freed in
/// one go when the program that started the network is gone).
#[derive(Clone)]
pub(crate) struct Ctx {
    pub stop: Rc<Cell<bool>>,
    pub socks: Rc<RefCell<BTreeSet<usize>>>,
}

impl Ctx {
    pub fn new() -> Self {
        Ctx { stop: Rc::new(Cell::new(false)), socks: Rc::new(RefCell::new(BTreeSet::new())) }
    }
    /// Free every socket still held (no waiting).
    pub fn free_all(&self) {
        let v: Vec<usize> = match self.socks.try_borrow_mut() {
            Ok(mut s) => core::mem::take(&mut *s).into_iter().collect(),
            Err(_) => return,
        };
        for h in v {
            ns::tcp_free_h(h);
        }
    }
}

pub(crate) struct Conn {
    pub h: usize,
    ctx: Ctx,
    buf: Vec<u8>,
    pos: usize,
    eof: bool,
    done: bool,
}

/// One cooperative wait step: let everything else run.
fn idle() {
    sys::yield_now();
}

impl Conn {
    pub fn new(h: usize, ctx: &Ctx) -> Conn {
        if let Ok(mut s) = ctx.socks.try_borrow_mut() {
            s.insert(h);
        }
        Conn { h, ctx: ctx.clone(), buf: Vec::new(), pos: 0, eof: false, done: false }
    }

    fn stopped(&self) -> bool {
        self.ctx.stop.get()
    }

    pub fn buffered(&self) -> usize {
        self.buf.len() - self.pos
    }

    /// The peer closed (or reset) the connection; buffered bytes may remain.
    pub fn peer_closed(&self) -> bool {
        self.eof
    }

    /// The peer closed and every byte it sent has been read.
    pub fn at_eof(&self) -> bool {
        self.eof && self.buffered() == 0
    }

    /// Move whatever the socket holds into our buffer (also services the
    /// NIC). Returns the bytes moved.
    pub fn pull(&mut self) -> usize {
        if self.eof || self.done {
            return 0;
        }
        let mut total = 0;
        let mut tmp = [0u8; 8192];
        while self.buffered() < MAX_BUFFERED {
            let n = ns::tcp_read_h(self.h, &mut tmp, 0);
            if n == 0 {
                if ns::tcp_closed_h(self.h) {
                    self.eof = true;
                }
                break;
            }
            if self.pos > 0 && self.pos >= self.buf.len() / 2 {
                self.buf.drain(..self.pos);
                self.pos = 0;
            }
            self.buf.extend_from_slice(&tmp[..n]);
            total += n;
        }
        total
    }

    fn take_into(&mut self, out: &mut [u8]) -> usize {
        let n = out.len().min(self.buffered());
        out[..n].copy_from_slice(&self.buf[self.pos..self.pos + n]);
        self.pos += n;
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        n
    }

    /// A complete raw frame (`outer`) if one is buffered; `Err` for a length
    /// prefix no frame can have (checked before any payload is buffered by
    /// the caller's accounting).
    pub fn try_frame(&mut self) -> Result<Option<Vec<u8>>, ()> {
        if self.buffered() < 4 {
            return Ok(None);
        }
        let p = self.pos;
        let l = u32::from_le_bytes([self.buf[p], self.buf[p + 1], self.buf[p + 2], self.buf[p + 3]]) as usize;
        if !valid_wire_len(l) {
            return Err(());
        }
        if self.buffered() < 4 + l {
            return Ok(None);
        }
        let outer = self.buf[p + 4..p + 4 + l].to_vec();
        self.pos += 4 + l;
        if self.pos == self.buf.len() {
            self.buf.clear();
            self.pos = 0;
        }
        Ok(Some(outer))
    }

    /// Is a frame's length prefix in (its body may still be arriving)?
    pub fn frame_started(&self) -> bool {
        self.buffered() >= 4
    }

    fn forget(&mut self) {
        if !self.done {
            self.done = true;
            if let Ok(mut s) = self.ctx.socks.try_borrow_mut() {
                s.remove(&self.h);
            }
        }
    }

    /// Close now (no waiting).
    pub fn abort(mut self) {
        if !self.done {
            ns::tcp_free_h(self.h);
        }
        self.forget();
    }

    /// Graceful close, like `tcp_close_h` but yielding while it waits: let
    /// what we sent be acknowledged (≤ 2 s), FIN, wait for its ACK (≤ 1.5 s).
    pub fn close(mut self) {
        if self.done {
            return;
        }
        if !self.stopped() && ns::tcp_established_h(self.h) {
            let until = sys::now_ms() + 2000;
            while ns::tcp_unacked_h(self.h) > 0 && ns::tcp_established_h(self.h) && sys::now_ms() < until && !self.stopped() {
                ns::pump_once();
                idle();
            }
            if ns::tcp_fin_h(self.h) {
                let until = sys::now_ms() + 1500;
                while ns::tcp_unacked_h(self.h) > 0 && sys::now_ms() < until && !self.stopped() {
                    ns::pump_once();
                    idle();
                }
            }
        }
        ns::tcp_free_h(self.h);
        self.forget();
    }
}

impl Drop for Conn {
    fn drop(&mut self) {
        if !self.done {
            ns::tcp_free_h(self.h);
            self.forget();
        }
    }
}

impl Io for Conn {
    fn read_exact_deadline(&mut self, out: &mut [u8], deadline_ms: u64) -> Result<(), IoError> {
        let mut off = 0;
        loop {
            off += self.take_into(&mut out[off..]);
            if off == out.len() {
                return Ok(());
            }
            if self.stopped() {
                return Err(IoError::Closed);
            }
            if self.pull() > 0 {
                continue;
            }
            if self.at_eof() {
                return Err(IoError::Eof);
            }
            if sys::now_ms() >= deadline_ms {
                return Err(IoError::TimedOut);
            }
            idle();
        }
    }

    fn write_all_deadline(&mut self, b: &[u8], deadline_ms: u64) -> Result<(), IoError> {
        for chunk in b.chunks(SEG) {
            loop {
                if self.stopped() || self.done {
                    return Err(IoError::Closed);
                }
                if ns::tcp_writable_h(self.h, chunk.len()) {
                    if ns::tcp_write_h(self.h, chunk) {
                        break;
                    }
                    return Err(IoError::Closed);
                }
                if ns::tcp_closed_h(self.h) {
                    return Err(IoError::Closed);
                }
                if sys::now_ms() >= deadline_ms {
                    return Err(IoError::TimedOut);
                }
                // keep the receive side flowing while we wait for ACKs
                if self.pull() == 0 {
                    idle();
                }
            }
        }
        Ok(())
    }
}

/// Dial `ip:port`, waiting (cooperatively) up to `timeout_ms`.
pub(crate) fn connect_ip(ip: [u8; 4], port: u16, timeout_ms: u64, ctx: &Ctx) -> Result<Conn, IoError> {
    let h = ns::tcp_connect_begin_h(ip, port).ok_or(IoError::Other("no free socket"))?;
    let conn = Conn::new(h, ctx);
    let until = sys::now_ms() + timeout_ms;
    loop {
        if ctx.stop.get() {
            conn.abort();
            return Err(IoError::Closed);
        }
        ns::pump_once();
        if ns::tcp_established_h(h) {
            return Ok(conn);
        }
        if ns::tcp_closed_h(h) {
            conn.abort();
            return Err(IoError::Other("connection refused"));
        }
        if sys::now_ms() >= until {
            crate::console_write(alloc::format!("p2p: connect timed out: {}\n", ns::tcp_debug_h(h)).as_bytes());
            conn.abort();
            return Err(IoError::TimedOut);
        }
        idle();
    }
}
