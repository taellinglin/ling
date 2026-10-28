//! Isolates and channels for compiled Ling (`thread_*`, `chan_*`, `shared_*`),
//! with the host runtime's semantics (src/runtime/threads.rs) on green
//! threads (coro.rs).
//!
//! * `thread_spawn(file, fn, args, caps)` runs another program of the image
//!   as an isolate: the module `isolates/node.ling` is the built-in program
//!   `iso_node` -- a small entry file (bundled next to the app) that calls the
//!   module's function with `isolate_args()`. Each isolate has its own
//!   globals and `st_*` store, swapped in while it runs.
//! * Channels are bounded queues; handles are strings (`"chan:<n>:tx"`,
//!   `"chan:<n>:rx"`). Waiting (a full queue, an empty one) yields to the
//!   other coroutines. Values are shared, not copied: compiled Ling never
//!   mutates a string or a list a receiver could see.
//! * `shared_put` / `shared_get`: a write-once store every isolate reads.

use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;

use crate::lingrec::{mk_bad_str, mk_ok, ok_unit};
use crate::lingrt::{boolean, list_from, list_items, mk_str, num, str_of, to_f64, UNIT};

struct Chan {
    q: VecDeque<u64>,
    cap: usize,
    closed: bool,
}

static mut CHANS: BTreeMap<u64, Chan> = BTreeMap::new();
static mut NEXT_CHAN: u64 = 1;
static mut SHARED: BTreeMap<String, u64> = BTreeMap::new();
/// Arguments each isolate was started with (by isolate id).
static mut ISO_ARGS: BTreeMap<u64, u64> = BTreeMap::new();
static mut INIT: bool = false;

fn chans() -> &'static mut BTreeMap<u64, Chan> {
    unsafe { &mut *(&raw mut CHANS) }
}
fn shared() -> &'static mut BTreeMap<String, u64> {
    unsafe { &mut *(&raw mut SHARED) }
}
fn iso_args() -> &'static mut BTreeMap<u64, u64> {
    unsafe { &mut *(&raw mut ISO_ARGS) }
}

fn init() {
    unsafe {
        if INIT {
            return;
        }
        INIT = true;
    }
    crate::lingrt::add_roots(roots);
    crate::coro::on_kill(cleanup);
}

fn roots(mark: &mut dyn FnMut(u64)) {
    for c in chans().values() {
        for &v in c.q.iter() {
            mark(v);
        }
    }
    for &v in shared().values() {
        mark(v);
    }
    for &v in iso_args().values() {
        mark(v);
    }
}

/// Isolates that ended: forget their arguments.
fn cleanup() {
    iso_args().retain(|&id, _| crate::coro::alive(id));
}

/// "chan:<n>:tx" -> (n, is_tx)
fn parse_handle(h: &str) -> Option<(u64, bool)> {
    let rest = h.strip_prefix("chan:")?;
    let (n, end) = rest.split_once(':')?;
    Some((n.parse().ok()?, end == "tx"))
}

fn chan_of(v: u64) -> Option<(u64, bool)> {
    parse_handle(str_of(v))
}

/// The built-in program for an isolate module: `isolates/node.ling` -> `iso_node`.
fn program_for(file: &str) -> String {
    let stem = file.rsplit(['/', '\\']).next().unwrap_or(file);
    let stem = stem.strip_suffix(".ling").unwrap_or(stem);
    alloc::format!("iso_{stem}")
}

pub fn dispatch(name: &[u8], args: &[u64]) -> Option<u64> {
    let arg = |i: usize| args.get(i).copied().unwrap_or(UNIT);
    let is_ours = name.starts_with(b"thread_") || name.starts_with(b"chan_") || name.starts_with(b"shared_") || name == b"isolate_args";
    if !is_ours {
        return None;
    }
    init();
    Some(match name {
        b"thread_spawn" => {
            let file = String::from(str_of(arg(0)));
            let prog = program_for(&file);
            let Some(entry) = crate::app_named(prog.as_bytes()) else {
                crate::console_write(b"thread_spawn: no built-in program ");
                crate::console_write(prog.as_bytes());
                crate::console_write(b" for ");
                crate::console_write(file.as_bytes());
                crate::console_write(b"\n");
                return Some(mk_str(""));
            };
            let a = arg(2);
            let id = crate::coro::spawn(
                &prog,
                true,
                alloc::boxed::Box::new(move || unsafe {
                    entry();
                }),
            );
            if id == 0 {
                return Some(mk_str(""));
            }
            iso_args().insert(id, a);
            mk_str(&alloc::format!("thread:{id}"))
        },
        b"isolate_args" => iso_args().get(&crate::coro::current_isolate()).copied().unwrap_or_else(|| list_from(Vec::new())),
        b"thread_self" => mk_str(&alloc::format!("thread:{}", crate::coro::current_isolate())),
        // one core runs every isolate: one miner is the honest count
        b"thread_cpus" => num(1.0),
        b"thread_alive" => {
            let id = str_of(arg(0)).strip_prefix("thread:").and_then(|s| s.parse::<u64>().ok()).unwrap_or(0);
            boolean(crate::coro::alive(id))
        },
        b"thread_caps" => list_from(["fs", "db", "net", "p2p", "keys", "clipboard"].iter().map(|s| mk_str(s)).collect()),
        b"chan_new" | b"chan_new_secure" => {
            let cap = to_f64(arg(0));
            let cap = if cap >= 1.0 { (cap as usize).min(65_536) } else { 1 };
            let n = unsafe {
                let n = NEXT_CHAN;
                NEXT_CHAN += 1;
                n
            };
            chans().insert(n, Chan { q: VecDeque::new(), cap, closed: false });
            list_from(alloc::vec![mk_str(&alloc::format!("chan:{n}:tx")), mk_str(&alloc::format!("chan:{n}:rx"))])
        },
        b"chan_send" | b"chan_try_send" => {
            let Some((n, _)) = chan_of(arg(0)) else { return Some(mk_bad_str("closed")) };
            let block = name == b"chan_send";
            loop {
                let Some(c) = chans().get_mut(&n) else { return Some(mk_bad_str("closed")) };
                if c.closed {
                    return Some(mk_bad_str("closed"));
                }
                if c.q.len() < c.cap {
                    c.q.push_back(arg(1));
                    return Some(ok_unit());
                }
                if !block {
                    return Some(mk_bad_str("full"));
                }
                crate::coro::yield_now();
            }
        },
        b"chan_recv" | b"chan_try_recv" | b"chan_recv_timeout" => {
            let Some((n, _)) = chan_of(arg(0)) else { return Some(mk_bad_str("closed")) };
            let deadline = match name {
                b"chan_recv_timeout" => Some(crate::arch::timer::now_ms() + to_f64(arg(1)).max(0.0) as u64),
                _ => None,
            };
            let block = name != b"chan_try_recv";
            loop {
                let Some(c) = chans().get_mut(&n) else { return Some(mk_bad_str("closed")) };
                if let Some(v) = c.q.pop_front() {
                    return Some(mk_ok(v));
                }
                if c.closed {
                    return Some(mk_bad_str("closed"));
                }
                if !block {
                    return Some(mk_bad_str("empty"));
                }
                if let Some(d) = deadline {
                    if crate::arch::timer::now_ms() >= d {
                        return Some(mk_bad_str("timeout"));
                    }
                }
                // main (the UI) never waits here for long: let the others run
                crate::coro::yield_now();
                if !crate::coro::in_coro() && deadline.is_none() && crate::coro::count() == 0 {
                    return Some(mk_bad_str("closed"));
                }
            }
        },
        b"chan_close" => match chan_of(arg(0)).and_then(|(n, _)| chans().get_mut(&n)) {
            Some(c) => {
                let was = c.closed;
                c.closed = true;
                boolean(!was)
            },
            None => boolean(false),
        },
        b"chan_len" => num(chan_of(arg(0)).and_then(|(n, _)| chans().get(&n)).map(|c| c.q.len()).unwrap_or(0) as f64),
        b"shared_put" => {
            let key = String::from(str_of(arg(0)));
            if shared().contains_key(&key) {
                mk_bad_str("exists")
            } else {
                shared().insert(key, arg(1));
                ok_unit()
            }
        },
        b"shared_get" => shared().get(str_of(arg(0))).copied().unwrap_or(arg(1)),
        _ => return None,
    })
}

/// For `ps`: the isolates and the channel count.
pub fn summary() -> String {
    let mut s = String::new();
    for (id, iso, name, owner, done) in crate::coro::list() {
        s.push_str(&alloc::format!("{id:>4} {iso:>4} {name:<20} {owner:<14} {}\n", if done { "done" } else { "running" }));
    }
    s.push_str(&alloc::format!("{} channel(s)\n", chans().len()));
    let _ = list_items;
    s
}
