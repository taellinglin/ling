//! An opt-in per-stage frame profiler for the running program. Off normally: every hook is one
//! `static` read. `prof 300` (console / SSH) or `os_prof(300)` (a program) measures the next N
//! frames (at most 600) and prints ONE report when they are done:
//!
//!  * per stage -- script (the interpreter between builtins), asset loads (SD I/O + decode), 3-D
//!    builtins with their raster / tone+ink post / half-res layer parts, text (glyph raster), 2-D
//!    draw calls, all other builtins, present (compose / screen blit / audio pump), the collector,
//!    isolates + tasks -- p50 / p95 / max and the share of the frame;
//!  * counts (builtin calls, triangles, glyphs rasterized, characters drawn, asset loads);
//!  * the slowest builtins by total time, the slowest frames with what dominated them, and a
//!    frame-time histogram;
//!  * named sections a program marks with `os_prof_mark("hud:portrait")` (the time between marks).
//!
//! Only timings, counts and builtin / section names are kept: never text the user typed or any
//! account data. The timer reads add a little to every call while it runs, so the figures are
//! relative: compare stages with each other, on the device.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

pub const ASSET: usize = 0;
pub const G3D: usize = 1;
pub const RASTER: usize = 2;
pub const POST: usize = 3;
pub const LAYER: usize = 4;
pub const TEXT: usize = 5;
pub const UI2D: usize = 6;
pub const OTHER: usize = 7;
pub const PRESENT: usize = 8;
pub const BLIT: usize = 9;
pub const PUMP: usize = 10;
pub const GC: usize = 11;
pub const TASKS: usize = 12;
pub const FRAME: usize = 13;
pub const VM: usize = 14;
pub const SCALE: usize = 15;
/// waiting for the frame's slot at the display refresh (idle, not work)
pub const PACE: usize = 16;
const N: usize = 17;
const NAMES: [&str; N] = [
    "asset loads (SD I/O + decode)",
    "3-D builtins, total",
    "   of it: raster",
    "   of it: tone ramp + ink",
    "   of it: half-res layer",
    "text (glyph raster)",
    "2-D draw builtins",
    "other builtins (st_*, math, lists)",
    "present: compose + cursor",
    "present: framebuffer write",
    "present: audio pump + input",
    "garbage collector",
    "isolates + tasks",
    "FRAME (wall)",
    "script / VM (no builtins)",
    "present: scale 720p -> screen",
    "frame pacing wait (idle, refresh cap)",
];

pub const C_TRIS: usize = 0;
pub const C_GLYPHS: usize = 1;
pub const C_CHARS: usize = 2;
pub const C_LOADS: usize = 3;
pub const C_CALLS: usize = 4;
const NC: usize = 5;

const WIN: usize = 600;
const MAX_SEC: usize = 16;
const TOP: usize = 128;

#[derive(Clone, Copy)]
struct Top {
    hash: u64,
    name: [u8; 28],
    len: u8,
    calls: u32,
    us: u64,
}

const TOP0: Top = Top { hash: 0, name: [0; 28], len: 0, calls: 0, us: 0 };

static mut ON: bool = false;
static mut LEFT: u32 = 0;
static mut NFR: usize = 0;
static mut LAST_END: u64 = 0;
static mut T_START: u64 = 0;
static mut CUR: [u64; N] = [0; N];
static mut S: [[u32; WIN]; N] = [[0; WIN]; N];
static mut CNT: [u64; NC] = [0; NC];
static mut LOAD_MAX: u64 = 0;
static mut DEPTH: u32 = 0;
static mut EF: u64 = 0;
static mut PUMP_T0: u64 = 0;
static mut TOPS: [Top; TOP] = [TOP0; TOP];
static mut SEC_NAME: [[u8; 24]; MAX_SEC] = [[0; 24]; MAX_SEC];
static mut SEC_LEN: [u8; MAX_SEC] = [0; MAX_SEC];
static mut SEC_N: usize = 0;
static mut SEC_CUR: [u64; MAX_SEC] = [0; MAX_SEC];
static mut SEC_S: [[u32; WIN]; MAX_SEC] = [[0; WIN]; MAX_SEC];
static mut SEC_ACTIVE: usize = usize::MAX;
static mut SEC_T0: u64 = 0;
static mut REPORT: String = String::new();
static mut GC0: u64 = 0;
static mut FREE0: usize = 0;
/// The SMP job counters when the window started (jobs, microseconds).
#[cfg(not(target_arch = "aarch64"))]
static mut PAR0: (u64, u64) = (0, 0);

#[inline]
pub fn on() -> bool {
    unsafe { ON }
}

fn now() -> u64 {
    crate::arch::timer::now_us()
}

/// Add `us` microseconds to `stage` for the frame being measured.
#[inline]
pub fn add(stage: usize, us: u64) {
    if unsafe { ON } && stage < N {
        unsafe { CUR[stage] += us };
    }
}

#[inline]
pub fn count(kind: usize, n: u64) {
    if unsafe { ON } && kind < NC {
        unsafe { CNT[kind] += n };
    }
}

/// Start measuring `frames` frames (1..=600); 0 stops and discards.
pub fn start(frames: u32) {
    unsafe {
        if frames == 0 {
            ON = false;
            return;
        }
        CUR = [0; N];
        CNT = [0; NC];
        TOPS = [TOP0; TOP];
        SEC_N = 0;
        SEC_CUR = [0; MAX_SEC];
        SEC_ACTIVE = usize::MAX;
        NFR = 0;
        LOAD_MAX = 0;
        LEFT = frames.min(WIN as u32);
        GC0 = crate::lingrt::gc_runs() as u64;
        FREE0 = crate::mm::frame::free_frame_count() * 4 / 1024;
        #[cfg(not(target_arch = "aarch64"))]
        {
            let (j, u, _) = crate::arch::smp::stats();
            PAR0 = (j, u);
        }
        T_START = now();
        LAST_END = T_START;
        ON = true;
    }
}

pub fn last_report() -> String {
    unsafe { (*(&raw const REPORT)).clone() }
}

/// A builtin finished: attribute its wall time to a stage and to its name.
pub fn builtin_done(name: &[u8], hash: u64, dt: u64) {
    let st = class(name);
    unsafe {
        CNT[C_CALLS] += 1;
        let tops = &mut *(&raw mut TOPS);
        let mut i = (hash as usize) % TOP;
        for _ in 0..8 {
            let t = &mut tops[i];
            if t.len == 0 {
                t.hash = hash;
                let n = name.len().min(28);
                t.name[..n].copy_from_slice(&name[..n]);
                t.len = n as u8;
            }
            if t.hash == hash {
                t.calls += 1;
                t.us += dt;
                break;
            }
            i = (i + 1) % TOP;
        }
    }
    match st {
        Some(s) => {
            unsafe { CUR[s] += dt };
            if s == ASSET {
                unsafe {
                    CNT[C_LOADS] += 1;
                    LOAD_MAX = LOAD_MAX.max(dt);
                }
            }
        },
        None => {},
    }
}

/// `Some(stage)` a builtin's wall time belongs to; `None` for the ones that are not "work" of the frame
/// (present itself, the profiler's own calls).
fn class(name: &[u8]) -> Option<usize> {
    let s = core::str::from_utf8(name).unwrap_or("");
    match s {
        "present" | "show" | "gfx_present" => return None,
        _ if s.starts_with("os_prof") => return None,
        "read_file" | "txm_tex" | "fs_read" => return Some(ASSET),
        _ if s.ends_with("_load") => return Some(ASSET),
        "font_text" | "font_text_fill" | "px_text" | "px_fit" | "font_width" | "font_width_px" | "text" => return Some(TEXT),
        "draw_poly" | "fill" | "line" | "draw_line" | "rect" | "circle" | "triangle" | "pixel" | "image_draw" | "ui_button" => {
            return Some(UI2D);
        },
        _ if s.starts_with("px_") => return Some(UI2D),
        _ if s.starts_with("gltf_")
            || s.starts_with("tm_")
            || s.starts_with("txm_")
            || s.starts_with("flush_")
            || s.starts_with("terrain_")
            || s.starts_with("mesh_")
            || s.starts_with("set_camera")
            || matches!(
                s,
                "set_projection" | "mouse_ray" | "project_3d" | "cube" | "box" | "sphere" | "cylinder" | "cone" | "capsule" | "torus" | "frustum" | "render_3d" | "post_now"
            ) =>
        {
            return Some(G3D);
        },
        _ => {},
    }
    Some(OTHER)
}

/// How deep builtin calls nest (only the outermost is timed).
pub fn enter() -> bool {
    unsafe {
        DEPTH += 1;
        DEPTH == 1
    }
}

pub fn leave() {
    unsafe { DEPTH = DEPTH.saturating_sub(1) };
}

/// `os_prof_mark(name)`: the time from now to the next mark (or the frame's end) belongs to `name`.
/// "" ends the section.
pub fn mark(name: &str) {
    if !on() {
        return;
    }
    let t = now();
    unsafe {
        close_section(t);
        if name.is_empty() {
            return;
        }
        let nb = name.as_bytes();
        let n = nb.len().min(24);
        let mut idx = usize::MAX;
        for i in 0..SEC_N {
            if SEC_LEN[i] as usize == n && SEC_NAME[i][..n] == nb[..n] {
                idx = i;
                break;
            }
        }
        if idx == usize::MAX && SEC_N < MAX_SEC {
            SEC_NAME[SEC_N][..n].copy_from_slice(&nb[..n]);
            SEC_LEN[SEC_N] = n as u8;
            idx = SEC_N;
            SEC_N += 1;
        }
        SEC_ACTIVE = idx;
        SEC_T0 = t;
    }
}

unsafe fn close_section(t: u64) {
    if SEC_ACTIVE < MAX_SEC {
        SEC_CUR[SEC_ACTIVE] += t.saturating_sub(SEC_T0);
    }
    SEC_ACTIVE = usize::MAX;
}

/// The 3-D flush `present()` does at the end of a frame: counted in "3-D builtins" but it is inside
/// present's time, so it is subtracted from the script share and from present's compose share.
pub fn add_endframe(us: u64) {
    if unsafe { ON } {
        unsafe {
            EF += us;
            CUR[G3D] += us;
        }
    }
}

/// present() reached the input / audio part of its frame.
pub fn mark_pump_start() {
    if unsafe { ON } {
        unsafe { PUMP_T0 = now() };
    }
}

/// `present()` finished a frame: `t0` is when it began, `end` when it returned.
pub fn frame_end(t0: u64, end: u64) {
    unsafe {
        if !ON {
            return;
        }
        close_section(end);
        let frame = end.saturating_sub(LAST_END);
        let present_total = end.saturating_sub(t0);
        if PUMP_T0 >= t0 {
            CUR[PUMP] = end.saturating_sub(PUMP_T0);
        }
        CUR[PRESENT] = present_total.saturating_sub(CUR[GC] + CUR[TASKS] + CUR[BLIT] + CUR[SCALE] + CUR[PUMP] + CUR[PACE] + EF);
        let builtins = CUR[ASSET] + CUR[G3D].saturating_sub(EF) + CUR[TEXT] + CUR[UI2D] + CUR[OTHER];
        CUR[VM] = frame.saturating_sub(builtins + present_total);
        CUR[FRAME] = frame;
        EF = 0;
        PUMP_T0 = 0;
        if NFR < WIN {
            for i in 0..N {
                S[i][NFR] = CUR[i].min(u32::MAX as u64) as u32;
            }
            for i in 0..SEC_N {
                SEC_S[i][NFR] = SEC_CUR[i].min(u32::MAX as u64) as u32;
            }
            NFR += 1;
        }
        CUR = [0; N];
        SEC_CUR = [0; MAX_SEC];
        LAST_END = end;
        LEFT = LEFT.saturating_sub(1);
        if LEFT == 0 {
            ON = false;
            let r = build_report(end.saturating_sub(T_START));
            crate::console_write(r.as_bytes());
            REPORT = r;
        }
    }
}

fn ms(us: u64) -> String {
    format!("{:.1}", us as f64 / 1000.0)
}

fn pct(v: &mut Vec<u32>, p: usize) -> u64 {
    if v.is_empty() {
        return 0;
    }
    let i = ((v.len() * p) / 100).min(v.len() - 1);
    v[i] as u64
}

unsafe fn build_report(wall: u64) -> String {
    let n = NFR;
    let mut out = String::new();
    if n == 0 {
        return String::from("prof: no frames were measured (does the program present frames?)\n");
    }
    let sum = |row: &[u32]| row[..n].iter().map(|&x| x as u64).sum::<u64>();
    let frame_avg = sum(&S[FRAME]) / n as u64;
    let fps = if frame_avg > 0 { 1_000_000.0 / frame_avg as f64 } else { 0.0 };
    out.push_str(&format!(
        "prof: {n} frames in {:.1} s -- frame avg {} ms ({:.1} fps).  Timings include the profiler's own cost; compare stages, not absolutes.\n",
        wall as f64 / 1e6,
        ms(frame_avg),
        fps
    ));
    let pace_avg = sum(&S[PACE]) / n as u64;
    if pace_avg > 0 {
        let busy = frame_avg.saturating_sub(pace_avg);
        out.push_str(&format!(
            "  capped at {} Hz: {} ms of each frame is work ({:.0} fps possible), the rest waits for the next refresh slot\n",
            crate::drivers::framebuffer::present_hz(),
            ms(busy),
            if busy > 0 { 1_000_000.0 / busy as f64 } else { 0.0 }
        ));
    }
    out.push_str(&machine_line());
    out.push_str(&format!(
        "  memory: free {} -> {} MiB over the window; collector ran {} times\n",
        FREE0,
        crate::mm::frame::free_frame_count() * 4 / 1024,
        (crate::lingrt::gc_runs() as u64).saturating_sub(GC0)
    ));
    out.push_str("  stage                                  avg    p50    p95    max  %frame   (ms)\n");
    let order = [FRAME, VM, OTHER, G3D, RASTER, POST, LAYER, TEXT, UI2D, ASSET, PRESENT, SCALE, BLIT, PUMP, GC, TASKS, PACE];
    for &st in &order {
        let mut v: Vec<u32> = S[st][..n].to_vec();
        v.sort_unstable();
        let avg = sum(&S[st]) / n as u64;
        let share = if frame_avg > 0 { avg as f64 * 100.0 / frame_avg as f64 } else { 0.0 };
        out.push_str(&format!(
            "  {:<36} {:>6} {:>6} {:>6} {:>6}  {:>5.1}%\n",
            NAMES[st],
            ms(avg),
            ms(pct(&mut v, 50)),
            ms(pct(&mut v, 95)),
            ms(*v.last().unwrap_or(&0) as u64),
            share
        ));
    }
    let per = |k: usize| CNT[k] as f64 / n as f64;
    out.push_str(&format!(
        "  per frame: {:.0} builtin calls, {:.0} triangles, {:.0} chars drawn;  glyphs rasterized {} (cache misses);  asset loads {} (slowest {} ms)\n",
        per(C_CALLS),
        per(C_TRIS),
        per(C_CHARS),
        CNT[C_GLYPHS],
        CNT[C_LOADS],
        ms(LOAD_MAX)
    ));
    // frame-time histogram + the slowest frames and what dominated them
    let mut h = [0usize; 5];
    for i in 0..n {
        let f = S[FRAME][i] as u64;
        h[if f > 100_000 { 4 } else if f > 50_000 { 3 } else if f > 33_400 { 2 } else if f > 16_700 { 1 } else { 0 }] += 1;
    }
    out.push_str(&format!("  frames: <=16.7 ms {}, <=33 ms {}, <=50 ms {}, <=100 ms {}, over 100 ms {}\n", h[0], h[1], h[2], h[3], h[4]));
    let mut worst: Vec<(u32, usize)> = (0..n).map(|i| (S[FRAME][i], i)).collect();
    worst.sort_unstable_by(|a, b| b.cmp(a));
    for &(f, i) in worst.iter().take(5) {
        let mut best = (0u32, VM);
        for st in [VM, OTHER, G3D, TEXT, UI2D, ASSET, PRESENT, SCALE, BLIT, PUMP, GC, TASKS] {
            if S[st][i] > best.0 {
                best = (S[st][i], st);
            }
        }
        out.push_str(&format!("  slow frame #{i}: {} ms, mostly {} ({} ms)\n", ms(f as u64), NAMES[best.1].trim(), ms(best.0 as u64)));
    }
    // the builtins that cost the most in total
    let mut tops: Vec<Top> = (*(&raw const TOPS)).iter().filter(|t| t.len > 0).copied().collect();
    tops.sort_unstable_by(|a, b| b.us.cmp(&a.us));
    out.push_str("  slowest builtins (total in window):\n");
    for t in tops.iter().take(16) {
        let name = core::str::from_utf8(&t.name[..t.len as usize]).unwrap_or("?");
        out.push_str(&format!("    {:<28} {:>8} calls {:>8} ms  ({:.1} us each)\n", name, t.calls, ms(t.us), t.us as f64 / t.calls.max(1) as f64));
    }
    if SEC_N > 0 {
        out.push_str("  marked sections (script + builtins between marks):\n");
        for i in 0..SEC_N {
            let mut v: Vec<u32> = SEC_S[i][..n].to_vec();
            v.sort_unstable();
            let name = core::str::from_utf8(&SEC_NAME[i][..SEC_LEN[i] as usize]).unwrap_or("?");
            let avg = sum(&SEC_S[i]) / n as u64;
            out.push_str(&format!("    {:<24} avg {:>6} p50 {:>6} p95 {:>6} max {:>6} ms\n", name, ms(avg), ms(pct(&mut v, 50)), ms(pct(&mut v, 95)), ms(*v.last().unwrap_or(&0) as u64)));
        }
    }
    out
}

/// Board, screen mode and cores, so a report says what it was measured on.
fn machine_line() -> String {
    let fbm = match crate::drivers::framebuffer::get() {
        Some(f) => format!("framebuffer {}x{} {} bpp pitch {}", f.width, f.height, f.bpp, f.pitch),
        None => String::from("no framebuffer"),
    };
    #[cfg(target_arch = "aarch64")]
    let (board, cores) = (
        match crate::arch::mmio::board() {
            crate::arch::mmio::RpiBoard::Pi5 => "Raspberry Pi 5",
            crate::arch::mmio::RpiBoard::Pi4 => "Raspberry Pi 4",
            crate::arch::mmio::RpiBoard::Pi3 => "Raspberry Pi 3 (or QEMU raspi3b)",
            _ => "Raspberry Pi (unknown)",
        },
        "1 core runs everything (the other cores are parked in boot.rs)",
    );
    #[cfg(not(target_arch = "aarch64"))]
    let (board, cores) = (
        "x86-64",
        format!(
            "{} of {} cores in use for pixel work (cores N to change)",
            crate::arch::smp::cores(),
            crate::arch::smp::cores_online()
        ),
    );
    #[cfg(not(target_arch = "aarch64"))]
    let par = {
        let (jobs, us, _) = crate::arch::smp::stats();
        let (j0, u0) = unsafe { (PAR0.0, PAR0.1) };
        format!(
            "  parallel jobs in the window: {} ({:.1} ms of the boot core's time spent in them)\n",
            jobs.saturating_sub(j0),
            us.saturating_sub(u0) as f64 / 1000.0
        )
    };
    #[cfg(target_arch = "aarch64")]
    let par = String::new();
    format!("  machine: {board}; {fbm}; {cores}\n{par}")
}

/// The `prof` command (console, SSH shell): `prof [frames]` start, `prof show` last report, `prof stop`.
pub fn command(arg: &str) -> String {
    let a = arg.trim();
    if a == "stop" || a == "off" {
        start(0);
        return String::from("prof: stopped\n");
    }
    if a == "show" || a == "report" {
        let r = last_report();
        return if r.is_empty() { String::from("prof: nothing measured yet (prof 300, then use the program)\n") } else { r };
    }
    let frames: u32 = if a.is_empty() { 300 } else { a.parse().unwrap_or(300) };
    start(frames.clamp(1, WIN as u32));
    format!("prof: measuring the next {} presented frames of the running program; one report is printed here when done (prof show repeats it)\n", frames.clamp(1, WIN as u32))
}
