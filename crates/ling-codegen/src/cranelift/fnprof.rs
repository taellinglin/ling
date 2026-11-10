// fnprof.rs — per-Ling-function profiler for JIT-compiled code.
//
//   LING_PROFILE_FN=1          instrument every JIT-compiled Ling function
//   LING_PROFILE_EVERY=N       report every N presented frames (default 240)
//
// With the switch on, the JIT emits a call to `ling_fn_enter(id)` in each
// function's entry block and to `ling_fn_exit(id)` before each return. A
// per-thread call stack turns those into inclusive time (function + callees,
// builtins included) and self time (inclusive minus instrumented callees).
// Off (the default) nothing is emitted, so normal runs pay nothing. The hooks
// cost two clock reads per call, so tiny hot functions read a little high.
// AOT builds are never instrumented (only the JIT sets the current id).

use std::cell::{Cell, RefCell};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

/// Is per-function profiling switched on for this process?
pub fn enabled() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var("LING_PROFILE_FN").map(|v| !v.is_empty() && v != "0").unwrap_or(false))
}

static NAMES: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Stable id for a function name (registered at JIT time).
pub fn id_for(name: &str) -> i64 {
    let mut n = NAMES.lock().unwrap_or_else(|e| e.into_inner());
    if let Some(i) = n.iter().position(|x| x == name) {
        return i as i64;
    }
    n.push(name.to_string());
    (n.len() - 1) as i64
}

thread_local! {
    /// Id of the function the JIT is translating right now (None = don't instrument).
    static CURRENT: Cell<Option<i64>> = const { Cell::new(None) };
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub(crate) fn set_current(id: Option<i64>) {
    CURRENT.with(|c| c.set(id));
}

pub(crate) fn current() -> Option<i64> {
    CURRENT.with(|c| c.get())
}

#[derive(Default)]
struct State {
    /// (function id, entry time, time spent in instrumented callees, ns)
    stack: Vec<(u32, Instant, u64)>,
    /// per id: (calls, inclusive ns, self ns)
    totals: Vec<(u64, u64, u64)>,
    frames: u32,
}

pub extern "C" fn ling_fn_enter(id: i64) -> i64 {
    let now = Instant::now();
    STATE.with(|s| s.borrow_mut().stack.push((id as u32, now, 0)));
    0
}

pub extern "C" fn ling_fn_exit(id: i64) -> i64 {
    let now = Instant::now();
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        // pop to the matching frame (a frame abandoned by an error is dropped)
        while let Some((fid, t0, child)) = s.stack.pop() {
            let incl = now.duration_since(t0).as_nanos() as u64;
            if fid as i64 == id {
                let i = fid as usize;
                if s.totals.len() <= i {
                    s.totals.resize(i + 1, (0, 0, 0));
                }
                let t = &mut s.totals[i];
                t.0 += 1;
                t.1 += incl;
                t.2 += incl.saturating_sub(child);
                if let Some(parent) = s.stack.last_mut() {
                    parent.2 += incl;
                }
                break;
            }
        }
    });
    0
}

/// Call once per presented frame: every `LING_PROFILE_EVERY` frames prints the
/// top functions by self time and resets the counters.
pub fn frame_tick() {
    if !enabled() {
        return;
    }
    static EVERY: OnceLock<u32> = OnceLock::new();
    let every = *EVERY.get_or_init(|| std::env::var("LING_PROFILE_EVERY").ok().and_then(|v| v.parse().ok()).unwrap_or(240).max(1));
    STATE.with(|s| {
        let mut s = s.borrow_mut();
        // Credit the frames still open on the stack (a game's frame loop lives in
        // a function that never returns) with their time so far, innermost first,
        // and restart their clocks.
        let now = Instant::now();
        let mut open_child = 0u64;
        for k in (0..s.stack.len()).rev() {
            let (fid, t0, child) = s.stack[k];
            let incl = now.duration_since(t0).as_nanos() as u64;
            let slf = incl.saturating_sub(child).saturating_sub(open_child);
            let i = fid as usize;
            if s.totals.len() <= i {
                s.totals.resize(i + 1, (0, 0, 0));
            }
            s.totals[i].1 += incl;
            s.totals[i].2 += slf;
            s.stack[k] = (fid, now, 0);
            open_child = incl;
        }
        s.frames += 1;
        if s.frames < every {
            return;
        }
        let frames = s.frames as f64;
        let names = NAMES.lock().unwrap_or_else(|e| e.into_inner());
        let mut rows: Vec<(usize, (u64, u64, u64))> = s.totals.iter().copied().enumerate().filter(|(_, t)| t.1 > 0).collect();
        rows.sort_by(|a, b| b.1 .2.cmp(&a.1 .2));
        let self_total: u64 = rows.iter().map(|r| r.1 .2).sum();
        eprintln!("┌─ LING FN PROFILE ── frames={} ─ JIT functions by SELF time (incl = with callees + builtins) ─", s.frames);
        eprintln!("│ {:<34} {:>10} {:>9} {:>10} {:>10} {:>6}", "function", "calls", "calls/fr", "self ms/fr", "incl ms/fr", "%self");
        for (i, (calls, incl, slf)) in rows.iter().take(30) {
            let name = names.get(*i).map(String::as_str).unwrap_or("?");
            let name: String = if name.chars().count() > 34 { name.chars().take(33).collect::<String>() + "…" } else { name.to_string() };
            eprintln!(
                "│ {:<34} {:>10} {:>9.1} {:>10.3} {:>10.3} {:>5.1}%",
                name,
                calls,
                *calls as f64 / frames,
                *slf as f64 / 1e6 / frames,
                *incl as f64 / 1e6 / frames,
                *slf as f64 * 100.0 / self_total.max(1) as f64
            );
        }
        eprintln!("└─ total self time in instrumented functions: {:.2} ms/frame", self_total as f64 / 1e6 / frames);
        drop(names);
        s.frames = 0;
        for t in s.totals.iter_mut() {
            *t = (0, 0, 0);
        }
    });
}
