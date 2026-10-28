//! `p2ptest` (console): a full LWP-1 handshake between two coroutines over an
//! in-memory pipe -- the same code a node runs against a peer, with no
//! network -- reporting the outcome and how much of its stack each side used.

use alloc::collections::VecDeque;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::lwp::crypto::identity::IdentityKey;
use crate::lwp::frame::{Io, IoError};
use crate::lwp::handshake::{self, Params, SelfNonces};
use crate::lwp::sys;

type Q = Rc<RefCell<VecDeque<u8>>>;

struct Pipe {
    rx: Q,
    tx: Q,
}

impl Io for Pipe {
    fn read_exact_deadline(&mut self, buf: &mut [u8], deadline_ms: u64) -> Result<(), IoError> {
        let mut got = 0;
        while got < buf.len() {
            {
                let mut q = self.rx.borrow_mut();
                while got < buf.len() {
                    match q.pop_front() {
                        Some(b) => {
                            buf[got] = b;
                            got += 1;
                        },
                        None => break,
                    }
                }
            }
            if got < buf.len() {
                if sys::now_ms() > deadline_ms {
                    return Err(IoError::TimedOut);
                }
                crate::coro::yield_now();
            }
        }
        Ok(())
    }

    fn write_all_deadline(&mut self, b: &[u8], _deadline_ms: u64) -> Result<(), IoError> {
        self.tx.borrow_mut().extend(b.iter().copied());
        Ok(())
    }
}

pub fn run() -> String {
    sys::install_entropy();
    let a2b: Q = Rc::new(RefCell::new(VecDeque::new()));
    let b2a: Q = Rc::new(RefCell::new(VecDeque::new()));
    let out: Rc<RefCell<Vec<String>>> = Rc::new(RefCell::new(Vec::new()));
    let t0 = sys::now_ms();
    let side = |name: &'static str, initiator: bool, rx: Q, tx: Q, out: Rc<RefCell<Vec<String>>>| {
        alloc::boxed::Box::new(move || {
            let me = IdentityKey::generate();
            let own = me.public();
            let nonces = SelfNonces::default();
            let p = Params { magic: *b"LTST", network: 1, deadline: sys::now_ms() + 60_000, self_nonces: &nonces, own_identity: &own };
            let mut pipe = Pipe { rx, tx };
            let r = if initiator { handshake::initiate(&mut pipe, &p, &me, None).map(|_| ()) } else { handshake::respond(&mut pipe, &p, &me).map(|_| ()) };
            let used = crate::coro::stack_used_self();
            out.borrow_mut().push(alloc::format!("{name}: {} (stack {} KiB of {} KiB)", match r {
                Ok(()) => String::from("ok"),
                Err(e) => alloc::format!("{e:?}"),
            }, used / 1024, (4096usize << crate::coro::STACK_ORDER) / 1024));
        }) as alloc::boxed::Box<dyn FnOnce()>
    };
    let a = crate::coro::spawn_owned("p2ptest-initiator", false, "system", side("initiator", true, b2a.clone(), a2b.clone(), out.clone()));
    let b = crate::coro::spawn_owned("p2ptest-responder", false, "system", side("responder", false, a2b, b2a, out.clone()));
    while (crate::coro::alive(a) || crate::coro::alive(b)) && sys::now_ms() < t0 + 90_000 {
        crate::coro::yield_now();
    }
    let mut s = out.borrow().join("\n");
    s.push_str(&alloc::format!("\nhandshake test took {} ms\n", sys::now_ms() - t0));
    s
}
