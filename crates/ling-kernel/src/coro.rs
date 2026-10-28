//! Green threads for the running program: coroutines on their own stacks,
//! switched cooperatively, alongside the program's main flow ("main" -- the
//! desktop, or the app that has the screen).
//!
//! A coroutine runs until it calls [`yield_now`] (directly, or through a
//! blocking builtin -- a channel wait, a socket read, `sleep`); then the next
//! one in turn runs. `present()` yields once a frame, so every coroutine gets
//! the CPU at least once per frame. Ling isolates (`thread_spawn`) are
//! coroutines with a Ling scope of their own: their globals, `st_*` store and
//! arguments are swapped in while they run. Rust workers (a peer connection's
//! reader) are coroutines too, attached to the isolate that started them.
//!
//! The collector runs only on main (`present`); it scans every parked
//! coroutine stack ([`for_each_parked_stack`]) and every parked scope
//! ([`roots`]). When the app that started them returns, [`kill_all`] drops
//! every coroutine (their stacks are freed; Rust state in them is leaked,
//! never run on).

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use crate::lingrt::LingScope;

/// 4 MiB: post-quantum crypto (ML-DSA, ML-KEM, the LW-1 ring signatures) is
/// a heavy stack user, more so on aarch64. The lowest 64 KiB are a guard
/// zone filled with a pattern; a switch that finds it disturbed names the
/// coroutine that overflowed and stops it before it corrupts anything else.
pub const STACK_ORDER: usize = 10;
const GUARD: usize = 64 * 1024;
const PAINT: u64 = 0x5354_4b5f_4755_4152; // STK_GUAR

struct Coro {
    id: u64,
    name: String,
    stack: u64,
    top: u64,
    /// Saved sp while not running.
    sp: u64,
    entry: Option<Box<dyn FnOnce()>>,
    done: bool,
    /// Its Ling scope while not running.
    scope: LingScope,
    /// The isolate it belongs to (its own id for an isolate, the starter's
    /// for a worker; 0 = main).
    iso: u64,
    /// Not before this time (ms) -- a sleeping coroutine.
    wake_ms: u64,
    /// The program it belongs to: the app that started it (its isolates and
    /// workers end when that app quits), or "system" for a boot service.
    owner: String,
}

static mut COROS: Vec<Coro> = Vec::new();
/// Index into COROS of the running coroutine, or None for main.
static mut CUR: Option<usize> = None;
static mut MAIN_SP: u64 = 0;
static mut MAIN_SCOPE: Option<LingScope> = None;
static mut NEXT_ID: u64 = 1;
/// Round-robin cursor (next index to try).
static mut RR: usize = 0;

fn coros() -> &'static mut Vec<Coro> {
    unsafe { &mut *(&raw mut COROS) }
}

extern "C" {
    fn ling_switch_stack(save: *mut u64, to: u64);
}

/// Is a coroutine (not main) running?
pub fn in_coro() -> bool {
    unsafe { CUR.is_some() }
}

/// The running coroutine's id (0 = main).
pub fn current() -> u64 {
    unsafe { CUR.map(|i| coros()[i].id).unwrap_or(0) }
}

/// The isolate the running code belongs to (0 = main).
pub fn current_isolate() -> u64 {
    unsafe { CUR.map(|i| coros()[i].iso).unwrap_or(0) }
}

/// How many coroutines exist (finished ones not yet reaped included).
pub fn count() -> usize {
    coros().len()
}

/// Is coroutine `id` still running (not finished)?
pub fn alive(id: u64) -> bool {
    coros().iter().any(|c| c.id == id && !c.done)
}

/// Start `f` on a new coroutine. `isolate`: a Ling isolate (own scope, its
/// own isolate id) rather than a worker of the current isolate. Returns its
/// id, or 0 with no memory for a stack.
pub fn spawn(name: &str, isolate: bool, f: Box<dyn FnOnce()>) -> u64 {
    let owner = match unsafe { CUR } {
        Some(i) => coros()[i].owner.clone(),
        None => crate::lingapp::active_app_name(),
    };
    spawn_owned(name, isolate, &owner, f)
}

/// [`spawn`] for a given owner ("system": a boot service, outlives every app).
pub fn spawn_owned(name: &str, isolate: bool, owner: &str, f: Box<dyn FnOnce()>) -> u64 {
    let Some(stack) = crate::mm::frame::alloc_frames(STACK_ORDER) else {
        crate::console_write(b"coro: no memory for a stack\n");
        return 0;
    };
    let top = (stack + (4096u64 << STACK_ORDER)) & !0xF;
    unsafe {
        // the whole stack: the guard zone, and a high-water mark for stack_used_self
        let g = core::slice::from_raw_parts_mut(stack as *mut u64, (4096usize << STACK_ORDER) / 8);
        g.fill(PAINT);
    }
    let id = unsafe {
        let id = NEXT_ID;
        NEXT_ID += 1;
        id
    };
    let iso = if isolate { id } else { current_isolate() };
    let sp = unsafe { fresh_stack(top) };
    coros().push(Coro { id, name: String::from(name), stack, top, sp, entry: Some(f), done: false, scope: LingScope::default(), iso, wake_ms: 0, owner: String::from(owner) });
    id
}

/// Let every other runnable coroutine run once (round robin), then return.
/// From main: runs each coroutine in turn until it yields back. From a
/// coroutine: switches to the next one (main included).
pub fn yield_now() {
    reap();
    if coros().is_empty() {
        return;
    }
    let now = crate::arch::timer::now_ms();
    let n = coros().len();
    let from = unsafe { CUR };
    // the next runnable coroutine after the current one, wrapping to main
    let start = match from {
        Some(i) => i + 1,
        None => unsafe { RR.min(n) },
    };
    let mut pick: Option<usize> = None;
    for k in 0..n {
        let i = start + k;
        if i >= n && from.is_some() {
            break; // past the end: main's turn
        }
        let i = i % n;
        let c = &coros()[i];
        if !c.done && c.wake_ms <= now && Some(i) != from {
            pick = Some(i);
            break;
        }
    }
    match (from, pick) {
        (None, None) => {},
        (None, Some(i)) => {
            unsafe { RR = 0 };
            switch(None, Some(i));
        },
        (Some(_), Some(i)) => switch(from, Some(i)),
        (Some(_), None) => switch(from, None),
    }
}

/// Sleep the running coroutine (others run meanwhile); main just yields
/// until the time is up.
pub fn sleep_ms(ms: u64) {
    let end = crate::arch::timer::now_ms() + ms;
    if let Some(i) = unsafe { CUR } {
        coros()[i].wake_ms = end;
    }
    loop {
        yield_now();
        if crate::arch::timer::now_ms() >= end {
            break;
        }
        if !in_coro() {
            idle_pause();
        }
    }
    if let Some(i) = unsafe { CUR } {
        coros()[i].wake_ms = 0;
    }
}

/// Main waiting with nothing to do: let the CPU breathe a little.
fn idle_pause() {
    #[cfg(target_arch = "aarch64")]
    crate::arch::timer::delay_us(200);
    #[cfg(target_arch = "x86_64")]
    core::hint::spin_loop();
}

/// Has this coroutine's stack run into its guard zone?
fn overflowed(c: &Coro) -> bool {
    // the top of the guard zone is hit first
    let g = unsafe { core::slice::from_raw_parts(c.stack as *const u64, GUARD / 8) };
    g[g.len() - 64..].iter().any(|&w| w != PAINT)
}

/// How much of its stack the running coroutine has used so far (bytes).
pub fn stack_used_self() -> usize {
    let Some(i) = (unsafe { CUR }) else { return 0 };
    let c = &coros()[i];
    let words = unsafe { core::slice::from_raw_parts(c.stack as *const u64, ((c.top - c.stack) / 8) as usize) };
    let untouched = words.iter().take_while(|&&w| w == PAINT).count() * 8;
    (c.top - c.stack) as usize - untouched
}

/// The running coroutine's name ("main" for the program itself), for crash
/// reports.
pub fn current_name() -> &'static [u8] {
    match unsafe { CUR } {
        Some(i) => unsafe { &*(coros()[i].name.as_bytes() as *const [u8]) },
        None => b"main",
    }
}

/// Switch from `from` to `to` (None = main), swapping Ling scopes.
fn switch(from: Option<usize>, to: Option<usize>) {
    if let Some(i) = from {
        if overflowed(&coros()[i]) {
            crate::console_write(b"\n!!! coroutine '");
            crate::console_write(coros()[i].name.as_bytes());
            crate::console_write(b"' overran its 4 MiB stack -- stopped\n");
            coros()[i].done = true;
        }
    }
    unsafe {
        // park the running scope with its owner, bring in the target's
        let live = crate::lingrt::scope_take();
        match from {
            Some(i) => coros()[i].scope = live,
            None => *(&raw mut MAIN_SCOPE) = Some(live),
        }
        let incoming = match to {
            Some(i) => core::mem::take(&mut coros()[i].scope),
            None => (*(&raw mut MAIN_SCOPE)).take().unwrap_or_default(),
        };
        crate::lingrt::scope_put(incoming);
        let save: *mut u64 = match from {
            Some(i) => &raw mut coros()[i].sp,
            None => &raw mut MAIN_SP,
        };
        let target = match to {
            Some(i) => coros()[i].sp,
            None => MAIN_SP,
        };
        CUR = to;
        ling_switch_stack(save, target);
        // back here: whoever switched to us set CUR and our scope already
    }
}

/// First thing on a coroutine's stack: run its entry, then finish.
extern "C" fn coro_trampoline() -> ! {
    let entry = unsafe { CUR.and_then(|i| coros()[i].entry.take()) };
    if let Some(f) = entry {
        f();
    }
    if let Some(i) = unsafe { CUR } {
        coros()[i].done = true;
    }
    // never returns: done coroutines are not picked again
    loop {
        yield_now();
    }
}

unsafe fn fresh_stack(top: u64) -> u64 {
    #[cfg(target_arch = "aarch64")]
    {
        let sp = top - 176;
        core::ptr::write_bytes(sp as *mut u8, 0, 176);
        core::ptr::write((sp + 88) as *mut u64, coro_trampoline as *const () as usize as u64); // x30
        sp
    }
    #[cfg(target_arch = "x86_64")]
    {
        let mut sp = top - 16;
        core::ptr::write(sp as *mut u64, coro_trampoline as *const () as usize as u64);
        for _ in 0..6 {
            sp -= 8;
            core::ptr::write(sp as *mut u64, 0);
        }
        sp
    }
}

/// Free finished coroutines (only from main: never the stack we are on).
fn reap() {
    if in_coro() {
        return;
    }
    let v = coros();
    let mut i = 0;
    while i < v.len() {
        if v[i].done {
            let c = v.remove(i);
            crate::mm::frame::free_frames(c.stack, STACK_ORDER);
            crate::lingrt::scope_drop(c.scope);
        } else {
            i += 1;
        }
    }
}

/// End every coroutine `owner` started (that app quit). From main.
pub fn kill_owner(owner: &str) {
    if in_coro() {
        return;
    }
    let v = coros();
    let mut i = 0;
    let mut any = false;
    while i < v.len() {
        if v[i].owner == owner {
            let c = v.remove(i);
            crate::mm::frame::free_frames(c.stack, STACK_ORDER);
            core::mem::forget(c.entry);
            crate::lingrt::scope_drop(c.scope);
            any = true;
        } else {
            i += 1;
        }
    }
    unsafe { RR = 0 };
    if !any {
        return;
    }
    for f in unsafe { (*(&raw const EXIT_HOOKS)).iter().flatten() } {
        f();
    }
}

static mut EXIT_HOOKS: [Option<fn()>; 8] = [None; 8];

/// Run `f` after [`kill_owner`] ended some coroutines: it drops the state of
/// isolates that are gone (`alive(iso)` false) -- sockets, files, keys.
pub fn on_kill(f: fn()) {
    let hooks = unsafe { &mut *(&raw mut EXIT_HOOKS) };
    if hooks.iter().any(|h| *h == Some(f)) {
        return;
    }
    if let Some(slot) = hooks.iter_mut().find(|h| h.is_none()) {
        *slot = Some(f);
    }
}

/// For the collector: every coroutine stack not running now (sp..top), and
/// main's when a coroutine runs.
pub fn for_each_parked_stack(mut f: impl FnMut(u64, u64)) {
    let cur = unsafe { CUR };
    for (i, c) in coros().iter().enumerate() {
        if Some(i) != cur && !c.done {
            f(c.sp, c.top);
        }
    }
}

/// For the collector: the Ling scopes put aside by coroutines (and main's).
pub fn roots(mark: &mut dyn FnMut(u64)) {
    for c in coros().iter() {
        crate::lingrt::scope_values(&c.scope, mark);
    }
    if let Some(s) = unsafe { (*(&raw const MAIN_SCOPE)).as_ref() } {
        crate::lingrt::scope_values(s, mark);
    }
}

/// `ps`-style listing: id, isolate, name, owner, finished.
pub fn list() -> Vec<(u64, u64, String, String, bool)> {
    coros().iter().map(|c| (c.id, c.iso, c.name.clone(), c.owner.clone(), c.done)).collect()
}

/// Is a coroutine with this name and owner running?
pub fn running(name: &str, owner: &str) -> bool {
    coros().iter().any(|c| !c.done && c.name == name && c.owner == owner)
}
