//! "Liquid" window motion for the WM (`kernel/x86_64-wm`): a window's
//! rendered rect springs toward its drag target with damped-oscillator
//! physics instead of snapping there instantly, plus a velocity-driven
//! squash/stretch on top -- a soap-film/jelly feel while dragging.
//!
//! State lives here (kernel-side `static mut`), not in `.ling`: the AOT
//! kernel-build path has no mutable-variable/reassignment construct at all
//! (confirmed against `ling`'s own parser -- no `Assign` expression exists,
//! `bind` only ever introduces a *new* name, it can't be re-bound across
//! loop iterations), so there's nowhere on the `.ling` side to hold physics
//! state that persists frame to frame. The WM calls [`step`] once per
//! frame with its current drag target, then reads the deformed rect back
//! via the plain getters below.
use crate::arch::timer;

const SPRING_K: f64 = 0.028; // stiffness -- higher snaps back to target faster
const DAMPING: f64 = 0.22; // higher settles faster, less overshoot/wobble
const STRETCH_SENSITIVITY: f64 = 0.006;
const MAX_STRETCH: f64 = 0.35; // caps how extreme the squash/stretch gets

static mut POS_X: f64 = 100.0;
static mut POS_Y: f64 = 100.0;
static mut VEL_X: f64 = 0.0;
static mut VEL_Y: f64 = 0.0;
static mut SCALE_W: f64 = 1.0;
static mut SCALE_H: f64 = 1.0;
static mut LAST_STEP_MS: u64 = 0;
static mut STARTED: bool = false;

/// Advance the spring simulation by however long it's actually been since
/// the last call (a real per-frame delta, not an assumed fixed step -- the
/// WM's frame pacing isn't guaranteed constant). Call once per frame,
/// before reading the position/scale getters.
pub fn step(target_x: f64, target_y: f64) {
    unsafe {
        let now = timer::now_ms();
        if !STARTED {
            // First call: nothing to spring *from* yet -- snap straight to
            // the target so the window doesn't fly in from (100,100) the
            // instant the WM starts.
            POS_X = target_x;
            POS_Y = target_y;
            LAST_STEP_MS = now;
            STARTED = true;
            return;
        }

        // Clamp the timestep: a long stall (window just started, a debug
        // breakpoint, a slow first frame) shouldn't inject one huge,
        // unstable spring impulse -- treat it as an ordinary frame instead.
        let dt = (now.saturating_sub(LAST_STEP_MS)).min(50) as f64;
        LAST_STEP_MS = now;
        if dt <= 0.0 {
            return;
        }

        let ax = (target_x - POS_X) * SPRING_K - VEL_X * DAMPING;
        let ay = (target_y - POS_Y) * SPRING_K - VEL_Y * DAMPING;
        VEL_X += ax * dt;
        VEL_Y += ay * dt;
        POS_X += VEL_X * dt;
        POS_Y += VEL_Y * dt;

        // Squash/stretch from velocity: fast horizontal motion stretches
        // width and squashes height a little (and vice versa for vertical
        // motion) -- cartoon-physics elasticity, capped so it can't look
        // broken at high drag speed.
        let stretch_x = (VEL_X.abs() * STRETCH_SENSITIVITY).min(MAX_STRETCH);
        let stretch_y = (VEL_Y.abs() * STRETCH_SENSITIVITY).min(MAX_STRETCH);
        SCALE_W = 1.0 + stretch_x - stretch_y * 0.5;
        SCALE_H = 1.0 + stretch_y - stretch_x * 0.5;
    }
}

pub fn x() -> f64 {
    unsafe { POS_X }
}
pub fn y() -> f64 {
    unsafe { POS_Y }
}
/// Width/height scale as a percentage (100 = unscaled) -- the `.ling` side
/// only has integer arithmetic over the flat-u64 FFI boundary, so this
/// hands back a percentage rather than a raw float for it to multiply and
/// divide by 100 itself, instead of exposing float bits across the ABI.
// `f64::round()` needs libm on this target (not available, no_std, and this
// crate stays away from pulling it in for one call site) -- both operands
// here are always positive (a scale factor near 1.0), so "add 0.5 and
// truncate" is a correct, dependency-free round-half-up.
pub fn scale_w_pct() -> u64 {
    unsafe { (SCALE_W * 100.0 + 0.5) as u64 }
}
pub fn scale_h_pct() -> u64 {
    unsafe { (SCALE_H * 100.0 + 0.5) as u64 }
}
