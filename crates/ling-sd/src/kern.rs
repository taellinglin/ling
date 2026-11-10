//! The hooks the host gives the engine, and the inner loops everything else is built on.
//!
//! * `set_parallel(f, cores)`: how to run `f(ctx, 0..n)` over several cores (LingOS: `par::for_each`; a host test: std threads).
//!   Jobs never allocate, print or yield: they only read the inputs and write their own part of the output.
//! * `set_yield(f)`: called between rounds of jobs, so a kernel coroutine can hand the CPU to the desktop.
//! * `set_avx2(on)`: the caller says AVX2 + FMA + F16C are usable (CPUID *and* the OS saved the YMM state). Off: SSE (x86-64's
//!   baseline) or portable code.
//!
//! The one GEMM shape: `C[r][0..16] (+)= sum_k A[r][k] * B[k][0..16]` for 4 rows at a time, where `B` is a *panel*: 16 columns of
//! a weight matrix laid out k-major (`K x 16` floats, contiguous). Every weight is stored as panels, so the tile streams B linearly
//! and broadcasts A; the rows of A are given as pointers, which lets a 3x3 convolution read shifted input rows in place (no im2col).
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering::Relaxed};

pub type ParFn = fn(n: usize, ctx: *const (), f: unsafe fn(*const (), usize));

static PAR: AtomicUsize = AtomicUsize::new(0);
static CORES: AtomicUsize = AtomicUsize::new(1);
static YIELD: AtomicUsize = AtomicUsize::new(0);
static AVX2: AtomicBool = AtomicBool::new(false);
static CANCEL: AtomicBool = AtomicBool::new(false);

pub fn set_parallel(f: ParFn, cores: usize) {
    PAR.store(f as usize, Relaxed);
    CORES.store(cores.clamp(1, 64), Relaxed);
}

pub fn set_yield(f: fn()) {
    YIELD.store(f as usize, Relaxed);
}

/// Use the AVX2 + FMA + F16C kernels (only when the CPU has them AND the OS enabled the YMM state).
pub fn set_avx2(on: bool) {
    AVX2.store(on, Relaxed);
}

pub fn avx2() -> bool {
    AVX2.load(Relaxed)
}

/// Ask a running generation to stop at the next round of work.
pub fn set_cancel(on: bool) {
    CANCEL.store(on, Relaxed);
}

pub fn cancelled() -> bool {
    CANCEL.load(Relaxed)
}

pub fn cores() -> usize {
    CORES.load(Relaxed)
}

/// Give the host a chance to run something else (between rounds of jobs).
pub fn pump() {
    let y = YIELD.load(Relaxed);
    if y != 0 {
        // SAFETY: only set_yield stores here, and it stores an fn()
        let f: fn() = unsafe { core::mem::transmute(y) };
        f();
    }
}

unsafe fn par_tramp<F: Fn(usize) + Sync>(ctx: *const (), i: usize) {
    (*(ctx as *const F))(i)
}

/// `f(0) .. f(n-1)`, over the cores the host lends (in order on one core without a hook).
pub fn par_for<F: Fn(usize) + Sync>(n: usize, f: &F) {
    if n == 0 {
        return;
    }
    let p = PAR.load(Relaxed);
    if p == 0 || n == 1 {
        for i in 0..n {
            f(i);
        }
        return;
    }
    // SAFETY: only set_parallel stores here, and it stores a ParFn
    let pf: ParFn = unsafe { core::mem::transmute(p) };
    pf(n, f as *const F as *const (), par_tramp::<F>);
}

/// `f(0) .. f(n-1)` in rounds of a few jobs per core, yielding to the host between rounds (keeps the desktop drawing while a big
/// layer runs, and lets a cancel stop a long layer early).
pub fn run_jobs<F: Fn(usize) + Sync>(n: usize, f: &F) {
    let round = (cores() * 6).max(4);
    let mut i0 = 0;
    while i0 < n {
        let m = round.min(n - i0);
        par_for(m, &|j: usize| f(i0 + j));
        i0 += m;
        pump();
    }
}

/// Per-job scratch buffers: a job claims a free slot for its duration. At most `cores()` jobs run at once, so that many slots.
pub struct Slots {
    busy: AtomicU64,
}

impl Slots {
    pub const fn new() -> Slots {
        Slots { busy: AtomicU64::new(0) }
    }
    pub fn claim(&self) -> usize {
        loop {
            let b = self.busy.load(core::sync::atomic::Ordering::Acquire);
            let free = (!b).trailing_zeros() as usize;
            if free < 64 && self.busy.compare_exchange_weak(b, b | (1 << free), core::sync::atomic::Ordering::AcqRel, Relaxed).is_ok() {
                return free;
            }
            core::hint::spin_loop();
        }
    }
    pub fn release(&self, i: usize) {
        self.busy.fetch_and(!(1u64 << i), core::sync::atomic::Ordering::Release);
    }
}

/// A raw pointer that may be shared with the job cores; each job writes only its own part.
#[derive(Clone, Copy)]
pub struct P<T>(pub *mut T);
unsafe impl<T> Sync for P<T> {}
unsafe impl<T> Send for P<T> {}
impl<T> P<T> {
    #[inline(always)]
    pub fn get(&self) -> *mut T {
        self.0
    }
}

// ── the GEMM tile ─────────────────────────────────────────────────────────────

/// `C[r][0..ncols] = (acc ? C[r] : 0) + bias + sum_k A[r][k] * B[k][..]` for r < rows (<= 4). `B`: a K x 16 panel. `bias`: null or
/// 16 floats (this panel's columns).
#[inline]
pub unsafe fn tile(k: usize, a: &[*const f32; 4], rows: usize, b: *const f32, c: &[*mut f32; 4], ncols: usize, acc: bool, bias: *const f32) {
    #[cfg(target_arch = "x86_64")]
    {
        if AVX2.load(Relaxed) {
            return tile_avx2(k, a, rows, b, c, ncols, acc, bias);
        }
        return tile_sse(k, a, rows, b, c, ncols, acc, bias);
    }
    #[allow(unreachable_code)]
    tile_generic(k, a, rows, b, c, ncols, acc, bias)
}

#[inline(always)]
unsafe fn store_row(c: *mut f32, vals: &[f32; 16], ncols: usize, acc: bool, bias: *const f32) {
    for j in 0..ncols {
        let mut v = vals[j];
        if !bias.is_null() {
            v += *bias.add(j);
        }
        if acc {
            v += *c.add(j);
        }
        *c.add(j) = v;
    }
}

#[allow(dead_code)]
unsafe fn tile_generic(k: usize, a: &[*const f32; 4], rows: usize, b: *const f32, c: &[*mut f32; 4], ncols: usize, acc: bool, bias: *const f32) {
    let mut s = [[0f32; 16]; 4];
    for kk in 0..k {
        let bp = b.add(kk * 16);
        for r in 0..rows {
            let av = *a[r].add(kk);
            for j in 0..16 {
                s[r][j] += av * *bp.add(j);
            }
        }
    }
    for r in 0..rows {
        store_row(c[r], &s[r], ncols, acc, bias);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn tile_avx2(k: usize, a: &[*const f32; 4], rows: usize, b: *const f32, c: &[*mut f32; 4], ncols: usize, acc: bool, bias: *const f32) {
    use core::arch::x86_64::*;
    let a0 = a[0];
    let a1 = if rows > 1 { a[1] } else { a0 };
    let a2 = if rows > 2 { a[2] } else { a0 };
    let a3 = if rows > 3 { a[3] } else { a0 };
    let (mut c00, mut c01, mut c10, mut c11) = (_mm256_setzero_ps(), _mm256_setzero_ps(), _mm256_setzero_ps(), _mm256_setzero_ps());
    let (mut c20, mut c21, mut c30, mut c31) = (_mm256_setzero_ps(), _mm256_setzero_ps(), _mm256_setzero_ps(), _mm256_setzero_ps());
    let mut bp = b;
    for kk in 0..k {
        let b0 = _mm256_loadu_ps(bp);
        let b1 = _mm256_loadu_ps(bp.add(8));
        let x = _mm256_broadcast_ss(&*a0.add(kk));
        c00 = _mm256_fmadd_ps(x, b0, c00);
        c01 = _mm256_fmadd_ps(x, b1, c01);
        let x = _mm256_broadcast_ss(&*a1.add(kk));
        c10 = _mm256_fmadd_ps(x, b0, c10);
        c11 = _mm256_fmadd_ps(x, b1, c11);
        let x = _mm256_broadcast_ss(&*a2.add(kk));
        c20 = _mm256_fmadd_ps(x, b0, c20);
        c21 = _mm256_fmadd_ps(x, b1, c21);
        let x = _mm256_broadcast_ss(&*a3.add(kk));
        c30 = _mm256_fmadd_ps(x, b0, c30);
        c31 = _mm256_fmadd_ps(x, b1, c31);
        bp = bp.add(16);
    }
    let pairs = [(c00, c01), (c10, c11), (c20, c21), (c30, c31)];
    for r in 0..rows {
        let (lo, hi) = pairs[r];
        let cp = c[r];
        if ncols == 16 {
            let mut lo = lo;
            let mut hi = hi;
            if !bias.is_null() {
                lo = _mm256_add_ps(lo, _mm256_loadu_ps(bias));
                hi = _mm256_add_ps(hi, _mm256_loadu_ps(bias.add(8)));
            }
            if acc {
                lo = _mm256_add_ps(lo, _mm256_loadu_ps(cp));
                hi = _mm256_add_ps(hi, _mm256_loadu_ps(cp.add(8)));
            }
            _mm256_storeu_ps(cp, lo);
            _mm256_storeu_ps(cp.add(8), hi);
        } else {
            let mut v = [0f32; 16];
            _mm256_storeu_ps(v.as_mut_ptr(), lo);
            _mm256_storeu_ps(v.as_mut_ptr().add(8), hi);
            store_row(cp, &v, ncols, acc, bias);
        }
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn tile_sse(k: usize, a: &[*const f32; 4], rows: usize, b: *const f32, c: &[*mut f32; 4], ncols: usize, acc: bool, bias: *const f32) {
    use core::arch::x86_64::*;
    let a0 = a[0];
    let a1 = if rows > 1 { a[1] } else { a0 };
    let a2 = if rows > 2 { a[2] } else { a0 };
    let a3 = if rows > 3 { a[3] } else { a0 };
    // two passes of 8 columns: 8 accumulators + 2 B + 1 A fit the 16 xmm registers
    for half in 0..2 {
        if half * 8 >= ncols {
            break;
        }
        let (mut c00, mut c01, mut c10, mut c11) = (_mm_setzero_ps(), _mm_setzero_ps(), _mm_setzero_ps(), _mm_setzero_ps());
        let (mut c20, mut c21, mut c30, mut c31) = (_mm_setzero_ps(), _mm_setzero_ps(), _mm_setzero_ps(), _mm_setzero_ps());
        let mut bp = b.add(half * 8);
        for kk in 0..k {
            let b0 = _mm_loadu_ps(bp);
            let b1 = _mm_loadu_ps(bp.add(4));
            let x = _mm_set1_ps(*a0.add(kk));
            c00 = _mm_add_ps(c00, _mm_mul_ps(x, b0));
            c01 = _mm_add_ps(c01, _mm_mul_ps(x, b1));
            let x = _mm_set1_ps(*a1.add(kk));
            c10 = _mm_add_ps(c10, _mm_mul_ps(x, b0));
            c11 = _mm_add_ps(c11, _mm_mul_ps(x, b1));
            let x = _mm_set1_ps(*a2.add(kk));
            c20 = _mm_add_ps(c20, _mm_mul_ps(x, b0));
            c21 = _mm_add_ps(c21, _mm_mul_ps(x, b1));
            let x = _mm_set1_ps(*a3.add(kk));
            c30 = _mm_add_ps(c30, _mm_mul_ps(x, b0));
            c31 = _mm_add_ps(c31, _mm_mul_ps(x, b1));
            bp = bp.add(16);
        }
        let pairs = [(c00, c01), (c10, c11), (c20, c21), (c30, c31)];
        let n = (ncols - half * 8).min(8);
        let bb = if bias.is_null() { bias } else { bias.add(half * 8) };
        for r in 0..rows {
            let (lo, hi) = pairs[r];
            let cp = c[r].add(half * 8);
            if n == 8 {
                let mut lo = lo;
                let mut hi = hi;
                if !bb.is_null() {
                    lo = _mm_add_ps(lo, _mm_loadu_ps(bb));
                    hi = _mm_add_ps(hi, _mm_loadu_ps(bb.add(4)));
                }
                if acc {
                    lo = _mm_add_ps(lo, _mm_loadu_ps(cp));
                    hi = _mm_add_ps(hi, _mm_loadu_ps(cp.add(4)));
                }
                _mm_storeu_ps(cp, lo);
                _mm_storeu_ps(cp.add(4), hi);
            } else {
                let mut v = [0f32; 16];
                _mm_storeu_ps(v.as_mut_ptr(), lo);
                _mm_storeu_ps(v.as_mut_ptr().add(4), hi);
                store_row(cp, &v, n, acc, bb);
            }
        }
    }
}

// ── f16 -> f32 ───────────────────────────────────────────────────────────────

static LUT: AtomicUsize = AtomicUsize::new(0);

/// The 64 Ki-entry half -> single table for the non-F16C path (built once, on the calling core).
pub fn init_lut() {
    if LUT.load(Relaxed) != 0 {
        return;
    }
    let mut t = alloc::vec![0f32; 65536];
    for (i, v) in t.iter_mut().enumerate() {
        *v = crate::f16::to_f32(i as u16);
    }
    let p = alloc::boxed::Box::leak(t.into_boxed_slice()).as_ptr();
    LUT.store(p as usize, Relaxed);
}

/// `dst[i] = f32(src[i])` for i < n. `init_lut` must have run.
pub unsafe fn cvt_f16(src: *const u16, dst: *mut f32, n: usize) {
    #[cfg(target_arch = "x86_64")]
    {
        if AVX2.load(Relaxed) {
            return cvt_f16c(src, dst, n);
        }
    }
    let lut = LUT.load(Relaxed) as *const f32;
    if lut.is_null() {
        for i in 0..n {
            *dst.add(i) = crate::f16::to_f32(*src.add(i));
        }
        return;
    }
    for i in 0..n {
        *dst.add(i) = *lut.add(*src.add(i) as usize);
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,f16c")]
unsafe fn cvt_f16c(src: *const u16, dst: *mut f32, n: usize) {
    use core::arch::x86_64::*;
    let mut i = 0;
    while i + 8 <= n {
        let h = _mm_loadu_si128(src.add(i) as *const __m128i);
        _mm256_storeu_ps(dst.add(i), _mm256_cvtph_ps(h));
        i += 8;
    }
    while i < n {
        *dst.add(i) = crate::f16::to_f32(*src.add(i));
        i += 1;
    }
}

// ── exp over a slice (softmax, SiLU) ──────────────────────────────────────────

/// `x[i] = exp(x[i])`, Cephes' expf (about 1 ulp) four or eight at a time.
pub fn vexp(x: &mut [f32]) {
    #[cfg(target_arch = "x86_64")]
    unsafe {
        if AVX2.load(Relaxed) {
            return vexp_avx2(x);
        }
        return vexp_sse(x);
    }
    #[allow(unreachable_code)]
    for v in x.iter_mut() {
        *v = libm::expf(*v);
    }
}

const EXP_HI: f32 = 88.376_26;
const EXP_LO: f32 = -88.376_26;
const LOG2E: f32 = 1.442_695;
const C1: f32 = 0.693_359_4;
const C2: f32 = -2.121_944_4e-4;
const P0: f32 = 1.987_569_1e-4;
const P1: f32 = 1.398_199_9e-3;
const P2: f32 = 8.333_452e-3;
const P3: f32 = 4.166_579_6e-2;
const P4: f32 = 1.666_666_5e-1;
const P5: f32 = 5.000_000_1e-1;

#[inline(always)]
fn exp_scalar(x: f32) -> f32 {
    libm::expf(x)
}

#[cfg(target_arch = "x86_64")]
unsafe fn vexp_sse(x: &mut [f32]) {
    use core::arch::x86_64::*;
    let n = x.len();
    let p = x.as_mut_ptr();
    let mut i = 0;
    while i + 4 <= n {
        let mut v = _mm_loadu_ps(p.add(i));
        v = _mm_min_ps(_mm_max_ps(v, _mm_set1_ps(EXP_LO)), _mm_set1_ps(EXP_HI));
        let mut fx = _mm_add_ps(_mm_mul_ps(v, _mm_set1_ps(LOG2E)), _mm_set1_ps(0.5));
        // floor without SSE4.1: truncate, then step down where that rounded up
        let t = _mm_cvtepi32_ps(_mm_cvttps_epi32(fx));
        let up = _mm_and_ps(_mm_cmpgt_ps(t, fx), _mm_set1_ps(1.0));
        fx = _mm_sub_ps(t, up);
        v = _mm_sub_ps(v, _mm_mul_ps(fx, _mm_set1_ps(C1)));
        v = _mm_sub_ps(v, _mm_mul_ps(fx, _mm_set1_ps(C2)));
        let z = _mm_mul_ps(v, v);
        let mut y = _mm_set1_ps(P0);
        y = _mm_add_ps(_mm_mul_ps(y, v), _mm_set1_ps(P1));
        y = _mm_add_ps(_mm_mul_ps(y, v), _mm_set1_ps(P2));
        y = _mm_add_ps(_mm_mul_ps(y, v), _mm_set1_ps(P3));
        y = _mm_add_ps(_mm_mul_ps(y, v), _mm_set1_ps(P4));
        y = _mm_add_ps(_mm_mul_ps(y, v), _mm_set1_ps(P5));
        y = _mm_add_ps(_mm_mul_ps(y, z), v);
        y = _mm_add_ps(y, _mm_set1_ps(1.0));
        let e = _mm_slli_epi32(_mm_add_epi32(_mm_cvttps_epi32(fx), _mm_set1_epi32(127)), 23);
        _mm_storeu_ps(p.add(i), _mm_mul_ps(y, _mm_castsi128_ps(e)));
        i += 4;
    }
    while i < n {
        *p.add(i) = exp_scalar(*p.add(i));
        i += 1;
    }
}

#[cfg(target_arch = "x86_64")]
#[target_feature(enable = "avx2,fma")]
unsafe fn vexp_avx2(x: &mut [f32]) {
    use core::arch::x86_64::*;
    let n = x.len();
    let p = x.as_mut_ptr();
    let mut i = 0;
    while i + 8 <= n {
        let mut v = _mm256_loadu_ps(p.add(i));
        v = _mm256_min_ps(_mm256_max_ps(v, _mm256_set1_ps(EXP_LO)), _mm256_set1_ps(EXP_HI));
        let fx = _mm256_floor_ps(_mm256_fmadd_ps(v, _mm256_set1_ps(LOG2E), _mm256_set1_ps(0.5)));
        v = _mm256_fnmadd_ps(fx, _mm256_set1_ps(C1), v);
        v = _mm256_fnmadd_ps(fx, _mm256_set1_ps(C2), v);
        let z = _mm256_mul_ps(v, v);
        let mut y = _mm256_set1_ps(P0);
        y = _mm256_fmadd_ps(y, v, _mm256_set1_ps(P1));
        y = _mm256_fmadd_ps(y, v, _mm256_set1_ps(P2));
        y = _mm256_fmadd_ps(y, v, _mm256_set1_ps(P3));
        y = _mm256_fmadd_ps(y, v, _mm256_set1_ps(P4));
        y = _mm256_fmadd_ps(y, v, _mm256_set1_ps(P5));
        y = _mm256_fmadd_ps(y, z, v);
        y = _mm256_add_ps(y, _mm256_set1_ps(1.0));
        let e = _mm256_slli_epi32(_mm256_add_epi32(_mm256_cvttps_epi32(fx), _mm256_set1_epi32(127)), 23);
        _mm256_storeu_ps(p.add(i), _mm256_mul_ps(y, _mm256_castsi256_ps(e)));
        i += 8;
    }
    while i < n {
        *p.add(i) = exp_scalar(*p.add(i));
        i += 1;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use alloc::vec;

    fn check_tile() {
        let k = 37;
        let a: std::vec::Vec<std::vec::Vec<f32>> = (0..4).map(|r| (0..k).map(|i| ((r * 31 + i * 7) % 13) as f32 * 0.1 - 0.5).collect()).collect();
        let b: std::vec::Vec<f32> = (0..k * 16).map(|i| ((i * 11) % 17) as f32 * 0.05 - 0.4).collect();
        let bias: std::vec::Vec<f32> = (0..16).map(|j| j as f32 * 0.01).collect();
        for rows in 1..=4 {
            for ncols in [3usize, 8, 11, 16] {
                let mut c = vec![vec![1.0f32; 16]; 4];
                let ap = [a[0].as_ptr(), a[1].as_ptr(), a[2].as_ptr(), a[3].as_ptr()];
                let cp = [c[0].as_mut_ptr(), c[1].as_mut_ptr(), c[2].as_mut_ptr(), c[3].as_mut_ptr()];
                unsafe { tile(k, &ap, rows, b.as_ptr(), &cp, ncols, true, bias.as_ptr()) };
                for r in 0..4 {
                    for j in 0..16 {
                        let want = if r < rows && j < ncols { 1.0 + bias[j] + (0..k).map(|kk| a[r][kk] * b[kk * 16 + j]).sum::<f32>() } else { 1.0 };
                        assert!((c[r][j] - want).abs() < 1e-4, "rows {rows} ncols {ncols} r {r} j {j}: {} vs {want}", c[r][j]);
                    }
                }
            }
        }
    }

    #[test]
    fn tiles_and_exp_agree_on_every_path() {
        let mut paths = vec![false];
        #[cfg(target_arch = "x86_64")]
        if std::is_x86_feature_detected!("avx2") && std::is_x86_feature_detected!("fma") && std::is_x86_feature_detected!("f16c") {
            paths.push(true);
        }
        for p in paths {
            set_avx2(p);
            check_tile();
            let mut x: std::vec::Vec<f32> = (0..103).map(|i| i as f32 * 0.37 - 20.0).collect();
            let want: std::vec::Vec<f32> = x.iter().map(|v| v.exp()).collect();
            vexp(&mut x);
            for (g, w) in x.iter().zip(want.iter()) {
                assert!(((g - w) / w).abs() < 2e-6, "{g} vs {w}");
            }
            init_lut();
            let h: std::vec::Vec<u16> = (0..50u16).map(|i| i * 1311).collect();
            let mut out = vec![0f32; 50];
            unsafe { cvt_f16(h.as_ptr(), out.as_mut_ptr(), 50) };
            for i in 0..50 {
                let w = crate::f16::to_f32(h[i]);
                assert!(out[i] == w || (out[i].is_nan() && w.is_nan()));
            }
        }
        set_avx2(false);
    }
}
