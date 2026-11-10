//! The engine: packed fp16 weights and the layers built from them, every one split over the cores.
//!
//! Weights live in an [`Arena`] of big blocks (the host decides where those come from: LingOS hands out RAM above 4 GiB when the
//! machine has it). A linear layer `W[N][K]` is stored as `ceil(N/16)` panels of `K x 16` f16; a 3x3 convolution `W[N][Cin][3][3]`
//! as 9 taps of such panels. Right before a layer runs its panels are widened to f32 into the engine's weight buffers (`wseg`,
//! about 120 MiB at most: the biggest SD 1.5 convolution), then the GEMM tiles stream them.
//!
//! A 3x3 convolution never builds an im2col matrix: each job takes one output row, writes the three input rows it needs into its
//! scratch buffer (zero-padded, with the group norm + SiLU of the previous layer applied on the way, and the 2x nearest upsample
//! folded in), and runs 9 small GEMMs whose A rows point straight into that buffer.
use crate::act::Act;
use crate::kern::{self, run_jobs, Slots, P};
use crate::st::{DType, Info, SafeTensors, Source};
use crate::Error;
use alloc::string::{String, ToString};
use alloc::vec::Vec;
use core::sync::atomic::{AtomicBool, Ordering::Relaxed};

// ── weight storage ───────────────────────────────────────────────────────────

pub type BlockAlloc = fn(min_bytes: usize) -> Option<(*mut u8, usize)>;
pub type BlockFree = fn(ptr: *mut u8, bytes: usize);

static mut BLOCK_ALLOC: Option<(BlockAlloc, BlockFree)> = None;

/// Where the weight blocks come from (default: the global allocator, blocks just under 64 MiB).
pub fn set_block_alloc(a: BlockAlloc, f: BlockFree) {
    unsafe { BLOCK_ALLOC = Some((a, f)) };
}

const HEAP_BLOCK: usize = (64 << 20) - (64 << 10);

fn heap_block(min: usize) -> Option<(*mut u8, usize)> {
    let size = min.max(HEAP_BLOCK);
    let lay = core::alloc::Layout::from_size_align(size, 64).ok()?;
    let p = unsafe { alloc::alloc::alloc(lay) };
    if p.is_null() {
        None
    } else {
        Some((p, size))
    }
}

fn heap_free(p: *mut u8, size: usize) {
    if let Ok(lay) = core::alloc::Layout::from_size_align(size, 64) {
        unsafe { alloc::alloc::dealloc(p, lay) };
    }
}

fn hooks() -> (BlockAlloc, BlockFree) {
    unsafe { (*(&raw const BLOCK_ALLOC)).unwrap_or((heap_block as BlockAlloc, heap_free as BlockFree)) }
}

/// First-fit over big blocks; everything goes when the arena does.
pub struct Arena {
    blocks: Vec<(*mut u8, usize, usize)>,
    pub bytes: usize,
}

unsafe impl Send for Arena {}

impl Arena {
    pub fn new() -> Arena {
        Arena { blocks: Vec::new(), bytes: 0 }
    }
    pub fn alloc16(&mut self, n: usize) -> Result<W16, Error> {
        let need = (n * 2 + 63) & !63;
        for b in self.blocks.iter_mut() {
            if b.2 - b.1 >= need {
                let p = unsafe { b.0.add(b.1) } as *mut u16;
                b.1 += need;
                self.bytes += need;
                return Ok(W16 { ptr: p, len: n });
            }
        }
        let (a, _) = hooks();
        let Some((p, cap)) = a(need) else {
            return Err(alloc::format!("out of memory for the model's weights ({} MB held so far)", self.bytes >> 20));
        };
        self.blocks.push((p, need, cap));
        self.bytes += need;
        Ok(W16 { ptr: p as *mut u16, len: n })
    }
    pub fn reserved(&self) -> usize {
        self.blocks.iter().map(|b| b.2).sum()
    }
}

impl Drop for Arena {
    fn drop(&mut self) {
        let (_, f) = hooks();
        for b in self.blocks.drain(..) {
            f(b.0, b.2);
        }
    }
}

#[derive(Clone, Copy)]
pub struct W16 {
    pub ptr: *mut u16,
    pub len: usize,
}
unsafe impl Send for W16 {}
unsafe impl Sync for W16 {}

impl W16 {
    pub fn slice(&self) -> &[u16] {
        unsafe { core::slice::from_raw_parts(self.ptr, self.len) }
    }
    pub fn slice_mut(&mut self) -> &mut [u16] {
        unsafe { core::slice::from_raw_parts_mut(self.ptr, self.len) }
    }
}

/// `y = x W^T + b`, W stored as panels: `[npan][k][16]`.
pub struct Lin {
    pub k: usize,
    pub n: usize,
    pub npan: usize,
    pub w: W16,
    /// padded to npan * 16
    pub bias: Option<Vec<f32>>,
}

/// 3x3 convolution, padding 1: `[9][npan][cin][16]`.
pub struct Conv {
    pub cin: usize,
    pub n: usize,
    pub npan: usize,
    pub w: W16,
    pub bias: Vec<f32>,
}

pub struct Norm {
    pub g: Vec<f32>,
    pub b: Vec<f32>,
}

/// Per-channel `x * sc + sh` (a group norm folded into one multiply-add).
pub struct Aff {
    pub sc: Vec<f32>,
    pub sh: Vec<f32>,
}

// ── loading ─────────────────────────────────────────────────────────────────

pub struct Loader<'a> {
    pub st: &'a SafeTensors,
    pub src: &'a dyn Source,
    pub arena: &'a mut Arena,
    pub prefix: String,
    /// bytes read so far, for progress
    pub done: u64,
    pub progress: &'a mut dyn FnMut(u64),
}

const READ_CHUNK: usize = 2 << 20;

impl Loader<'_> {
    pub fn info(&self, name: &str) -> Result<Info, Error> {
        let full = alloc::format!("{}{}", self.prefix, name);
        self.st.get(&full).cloned().ok_or_else(|| alloc::format!("the checkpoint has no {full}"))
    }

    pub fn has(&self, name: &str) -> bool {
        self.st.get(&alloc::format!("{}{}", self.prefix, name)).is_some()
    }

    /// Elements [e0, e0 + n) of a tensor as f16 bits.
    fn read16(&mut self, info: &Info, e0: usize, n: usize, out: &mut Vec<u16>) -> Result<(), Error> {
        let sz = info.dtype.size();
        if info.dtype == DType::Other {
            return Err("a tensor in the checkpoint has an unsupported type (only F16 / BF16 / F32)".to_string());
        }
        let mut raw = alloc::vec![0u8; n * sz];
        let at = info.at + (e0 * sz) as u64;
        let mut off = 0;
        while off < raw.len() {
            let m = (raw.len() - off).min(1 << 20);
            if !self.src.read_at(at + off as u64, &mut raw[off..off + m]) {
                return Err("could not read the checkpoint (disk error, or the file is incomplete)".to_string());
            }
            off += m;
            self.done += m as u64;
            (self.progress)(self.done);
        }
        out.clear();
        out.reserve(n);
        match info.dtype {
            DType::F16 => out.extend(raw.chunks_exact(2).map(|b| u16::from_le_bytes([b[0], b[1]]))),
            DType::BF16 => out.extend(raw.chunks_exact(2).map(|b| crate::f16::from_f32(crate::f16::bf16_to_f32(u16::from_le_bytes([b[0], b[1]]))))),
            DType::F32 => out.extend(raw.chunks_exact(4).map(|b| crate::f16::from_f32(f32::from_le_bytes([b[0], b[1], b[2], b[3]])))),
            DType::Other => {},
        }
        Ok(())
    }

    pub fn vecf(&mut self, name: &str) -> Result<Vec<f32>, Error> {
        let info = self.info(name)?;
        let n = info.numel();
        let mut h = Vec::new();
        self.read16(&info, 0, n, &mut h)?;
        Ok(h.iter().map(|&v| crate::f16::to_f32(v)).collect())
    }

    pub fn norm(&mut self, base: &str) -> Result<Norm, Error> {
        Ok(Norm { g: self.vecf(&alloc::format!("{base}.weight"))?, b: self.vecf(&alloc::format!("{base}.bias"))? })
    }

    fn padded_bias(&mut self, name: &str, npan: usize) -> Result<Option<Vec<f32>>, Error> {
        if !self.has(name) {
            return Ok(None);
        }
        let mut b = self.vecf(name)?;
        b.resize(npan * 16, 0.0);
        Ok(Some(b))
    }

    /// `base.weight` [N, K] or [N, K, 1, 1] (+ `base.bias` if present) as a linear layer.
    pub fn lin(&mut self, base: &str) -> Result<Lin, Error> {
        let info = self.info(&alloc::format!("{base}.weight"))?;
        if info.shape.len() < 2 || info.shape[2..].iter().any(|&d| d != 1) {
            return Err(alloc::format!("{base}: expected a linear / 1x1 weight, got {:?}", info.shape));
        }
        let (n, k) = (info.shape[0], info.shape[1]);
        let npan = n.div_ceil(16);
        let mut w = self.arena.alloc16(npan * k * 16)?;
        let dst = w.slice_mut();
        dst.fill(0);
        let rows_per = (READ_CHUNK / (k * 2)).max(16) / 16 * 16;
        let mut buf = Vec::new();
        let mut r0 = 0;
        while r0 < n {
            let r1 = (r0 + rows_per).min(n);
            self.read16(&info, r0 * k, (r1 - r0) * k, &mut buf)?;
            for r in r0..r1 {
                let (p, j) = (r / 16, r % 16);
                let src = &buf[(r - r0) * k..(r - r0 + 1) * k];
                let base_i = p * k * 16 + j;
                for (kk, &v) in src.iter().enumerate() {
                    dst[base_i + kk * 16] = v;
                }
            }
            r0 = r1;
            kern::pump();
        }
        let bias = self.padded_bias(&alloc::format!("{base}.bias"), npan)?;
        Ok(Lin { k, n, npan, w, bias })
    }

    /// `base.weight` [N, Cin, 3, 3] + `base.bias`.
    pub fn conv(&mut self, base: &str) -> Result<Conv, Error> {
        let info = self.info(&alloc::format!("{base}.weight"))?;
        if info.shape.len() != 4 || info.shape[2] != 3 || info.shape[3] != 3 {
            return Err(alloc::format!("{base}: expected a 3x3 convolution, got {:?}", info.shape));
        }
        let (n, cin) = (info.shape[0], info.shape[1]);
        let npan = n.div_ceil(16);
        let mut w = self.arena.alloc16(9 * npan * cin * 16)?;
        let dst = w.slice_mut();
        dst.fill(0);
        let row = cin * 9;
        let rows_per = (READ_CHUNK / (row * 2)).max(16) / 16 * 16;
        let mut buf = Vec::new();
        let mut r0 = 0;
        while r0 < n {
            let r1 = (r0 + rows_per).min(n);
            self.read16(&info, r0 * row, (r1 - r0) * row, &mut buf)?;
            for r in r0..r1 {
                let (p, j) = (r / 16, r % 16);
                let src = &buf[(r - r0) * row..(r - r0 + 1) * row];
                for ci in 0..cin {
                    for t in 0..9 {
                        dst[((t * npan + p) * cin + ci) * 16 + j] = src[ci * 9 + t];
                    }
                }
            }
            r0 = r1;
            kern::pump();
        }
        let bias = self.padded_bias(&alloc::format!("{base}.bias"), npan)?.unwrap_or_else(|| alloc::vec![0.0; npan * 16]);
        Ok(Conv { cin, n, npan, w, bias })
    }

    /// Rows [r0, r1) of a 2-D tensor as f32 (the token embedding is read a row at a time).
    pub fn rows_f16(&mut self, name: &str) -> Result<(Vec<W16>, usize, usize), Error> {
        let info = self.info(name)?;
        let (rows, cols) = (info.shape[0], info.numel() / info.shape[0].max(1));
        // in pieces of at most 16384 rows: the whole table (72 MiB for CLIP's) is more than the kernel's biggest block
        let mut parts = Vec::new();
        let mut buf = Vec::new();
        let mut r0 = 0;
        while r0 < rows {
            let r1 = (r0 + 16384).min(rows);
            let mut w = self.arena.alloc16((r1 - r0) * cols)?;
            let mut q = r0;
            while q < r1 {
                let q1 = (q + (READ_CHUNK / (cols * 2)).max(1)).min(r1);
                self.read16(&info, q * cols, (q1 - q) * cols, &mut buf)?;
                w.slice_mut()[(q - r0) * cols..(q1 - r0) * cols].copy_from_slice(&buf);
                q = q1;
                kern::pump();
            }
            parts.push(w);
            r0 = r1;
        }
        Ok((parts, rows, cols))
    }
}

// ── the engine ──────────────────────────────────────────────────────────────

/// f32 floats per weight-buffer piece: one tap of the biggest SD 1.5 convolution (2560 x 1280) and a bit.
const WSEG: usize = 3_300_000;

static GELU_TANH: AtomicBool = AtomicBool::new(false);

/// Use the tanh approximation of GELU (what candle's `gelu()` computes) instead of the exact erf form (PyTorch's, the WebUI's).
/// Only for comparing against the host engine.
pub fn set_gelu_tanh(on: bool) {
    GELU_TANH.store(on, Relaxed);
}

/// The f32 copy of the panels a layer is using now.
#[derive(Clone, Copy)]
pub struct Panels {
    segs: [*const f32; 16],
    pps: usize,
    pe: usize,
}
unsafe impl Sync for Panels {}
unsafe impl Send for Panels {}

impl Panels {
    #[inline(always)]
    pub fn get(&self, p: usize) -> *const f32 {
        unsafe { self.segs[p / self.pps].add((p % self.pps) * self.pe) }
    }
}

pub struct Eng {
    wseg: Vec<Vec<f32>>,
    scratch: Vec<Vec<f32>>,
    slots: Slots,
    part: Vec<f64>,
}

impl Default for Eng {
    fn default() -> Self {
        Self::new()
    }
}

#[inline(always)]
fn silu_inplace(v: &mut [f32]) {
    let mut t = [0f32; 64];
    let mut i = 0;
    while i < v.len() {
        let m = (v.len() - i).min(64);
        for j in 0..m {
            t[j] = -v[i + j];
        }
        kern::vexp(&mut t[..m]);
        for j in 0..m {
            v[i + j] /= 1.0 + t[j];
        }
        i += m;
    }
}

#[inline(always)]
fn gelu(x: f32) -> f32 {
    if GELU_TANH.load(Relaxed) {
        0.5 * x * (1.0 + libm::tanhf(0.797_884_6 * (x + 0.044_715 * x * x * x)))
    } else {
        0.5 * x * (1.0 + libm::erff(x * core::f32::consts::FRAC_1_SQRT_2))
    }
}

/// What a convolution reads: `x`, optionally upsampled 2x (nearest), optionally through a per-channel affine and SiLU.
pub struct In<'a> {
    pub x: &'a Act,
    pub up: bool,
    pub aff: Option<&'a Aff>,
    pub silu: bool,
}

impl Eng {
    pub fn new() -> Eng {
        kern::init_lut();
        Eng { wseg: Vec::new(), scratch: Vec::new(), slots: Slots::new(), part: Vec::new() }
    }

    /// Drop the big buffers (between generations, to give the RAM back).
    pub fn trim(&mut self) {
        self.wseg = Vec::new();
        self.scratch = Vec::new();
    }

    fn ensure_scratch(&mut self, floats: usize) {
        let n = kern::cores().min(64);
        while self.scratch.len() < n {
            self.scratch.push(Vec::new());
        }
        for s in self.scratch.iter_mut() {
            if s.len() < floats {
                *s = alloc::vec![0f32; floats];
            }
        }
    }

    fn scratch_ptrs(&self) -> Vec<P<f32>> {
        self.scratch.iter().map(|s| P(s.as_ptr() as *mut f32)).collect()
    }

    /// Widen `npanels` panels of `pe` f16 each (contiguous at `w`) into the weight buffers.
    pub fn panels(&mut self, w: &W16, pe: usize, npanels: usize) -> Panels {
        assert!(pe <= WSEG, "a weight panel bigger than the buffer");
        let pps = WSEG / pe;
        let nseg = npanels.div_ceil(pps);
        assert!(nseg <= 16, "a layer too big for the weight buffers");
        while self.wseg.len() < nseg {
            self.wseg.push(alloc::vec![0f32; WSEG]);
        }
        let mut segs = [core::ptr::null::<f32>(); 16];
        for (i, s) in self.wseg.iter().enumerate().take(16) {
            segs[i] = s.as_ptr();
        }
        let pan = Panels { segs, pps, pe };
        let per_job = (32768 / pe).max(1);
        let jobs = npanels.div_ceil(per_job);
        let src = P(w.ptr);
        run_jobs(jobs, &|j: usize| {
            let p0 = j * per_job;
            let p1 = (p0 + per_job).min(npanels);
            for p in p0..p1 {
                unsafe { kern::cvt_f16(src.get().add(p * pe), pan.get(p) as *mut f32, pe) };
            }
        });
        pan
    }

    // ── normalization ────────────────────────────────────────────────────────

    /// Group norm statistics folded with gamma / beta into a per-channel affine.
    pub fn gn_affine(&mut self, x: &Act, n: &Norm, groups: usize, eps: f32) -> Aff {
        let c = x.c;
        let cpg = c / groups;
        let toks = x.tokens();
        let per = 256usize;
        let jobs = toks.div_ceil(per);
        self.part.clear();
        self.part.resize(jobs * groups * 2, 0.0);
        let part = P(self.part.as_mut_ptr());
        run_jobs(jobs, &|j: usize| {
            let mut acc = [0f64; 64];
            let t0 = j * per;
            let t1 = (t0 + per).min(toks);
            for t in t0..t1 {
                let v = unsafe { core::slice::from_raw_parts(x.px(t), c) };
                for g in 0..groups {
                    let (mut s, mut q) = (0f32, 0f32);
                    for &e in &v[g * cpg..(g + 1) * cpg] {
                        s += e;
                        q += e * e;
                    }
                    acc[g * 2] += s as f64;
                    acc[g * 2 + 1] += q as f64;
                }
            }
            let out = unsafe { core::slice::from_raw_parts_mut(part.get().add(j * groups * 2), groups * 2) };
            out.copy_from_slice(&acc[..groups * 2]);
        });
        let mut sc = alloc::vec![0f32; c];
        let mut sh = alloc::vec![0f32; c];
        let cnt = (toks * cpg) as f64;
        for g in 0..groups {
            let (mut s, mut q) = (0f64, 0f64);
            for j in 0..jobs {
                s += self.part[(j * groups + g) * 2];
                q += self.part[(j * groups + g) * 2 + 1];
            }
            let mean = s / cnt;
            let var = (q / cnt - mean * mean).max(0.0);
            let rstd = 1.0 / libm::sqrt(var + eps as f64);
            for ch in g * cpg..(g + 1) * cpg {
                let a = rstd * n.g[ch] as f64;
                sc[ch] = a as f32;
                sh[ch] = (n.b[ch] as f64 - mean * a) as f32;
            }
        }
        Aff { sc, sh }
    }

    /// `silu?(x * sc + sh)` as a new activation.
    pub fn apply(&mut self, x: &Act, a: &Aff, silu: bool) -> Act {
        let out = Act::new(x.h, x.w, x.c);
        let c = x.c;
        let toks = x.tokens();
        let per = 64usize;
        run_jobs(toks.div_ceil(per), &|j: usize| {
            for t in j * per..((j + 1) * per).min(toks) {
                let s = unsafe { core::slice::from_raw_parts(x.px(t), c) };
                let d = unsafe { core::slice::from_raw_parts_mut(out.px(t), c) };
                for i in 0..c {
                    d[i] = s[i] * a.sc[i] + a.sh[i];
                }
                if silu {
                    silu_inplace(d);
                }
            }
        });
        out
    }

    pub fn layer_norm(&mut self, x: &Act, n: &Norm, eps: f32) -> Act {
        let out = Act::new(x.h, x.w, x.c);
        let c = x.c;
        let toks = x.tokens();
        let per = 16usize;
        run_jobs(toks.div_ceil(per), &|j: usize| {
            for t in j * per..((j + 1) * per).min(toks) {
                let s = unsafe { core::slice::from_raw_parts(x.px(t), c) };
                let d = unsafe { core::slice::from_raw_parts_mut(out.px(t), c) };
                let mean = s.iter().sum::<f32>() / c as f32;
                let var = s.iter().map(|v| (v - mean) * (v - mean)).sum::<f32>() / c as f32;
                let r = 1.0 / libm::sqrtf(var + eps);
                for i in 0..c {
                    d[i] = (s[i] - mean) * r * n.g[i] + n.b[i];
                }
            }
        });
        out
    }

    // ── GEMMs ───────────────────────────────────────────────────────────────

    /// `x W^T + b` for every token, added onto `residual` when given (which is then the output).
    pub fn linear(&mut self, l: &Lin, x: &Act, residual: Option<Act>) -> Act {
        assert_eq!(x.c, l.k, "linear: input width");
        let acc = residual.is_some();
        let out = residual.unwrap_or_else(|| Act::new(x.h, x.w, l.n));
        assert_eq!(out.c, l.n, "linear: residual width");
        let pan = self.panels(&l.w, l.k * 16, l.npan);
        let toks = x.tokens();
        const MB: usize = 32;
        let pg = if toks <= 4 { 1 } else { 8 };
        let mblocks = toks.div_ceil(MB);
        let pgroups = l.npan.div_ceil(pg);
        let bias = l.bias.as_ref().map(|b| P(b.as_ptr() as *mut f32));
        let (k, n, npan) = (l.k, l.n, l.npan);
        run_jobs(mblocks * pgroups, &|j: usize| {
            let (mb, g) = (j / pgroups, j % pgroups);
            let (t0, t1) = (mb * MB, ((mb + 1) * MB).min(toks));
            for p in g * pg..((g + 1) * pg).min(npan) {
                let ncols = (n - p * 16).min(16);
                let b = pan.get(p);
                let bp = bias.map(|b| unsafe { b.get().add(p * 16) as *const f32 }).unwrap_or(core::ptr::null());
                let mut t = t0;
                while t < t1 {
                    let rows = (t1 - t).min(4);
                    let mut a = [core::ptr::null::<f32>(); 4];
                    let mut c = [core::ptr::null_mut::<f32>(); 4];
                    for r in 0..rows {
                        a[r] = x.px(t + r);
                        c[r] = unsafe { out.px(t + r).add(p * 16) };
                    }
                    unsafe { kern::tile(k, &a, rows, b, &c, ncols, acc, bp) };
                    t += rows;
                }
            }
        });
        out
    }

    /// 3x3 convolution, padding 1, stride 1 or 2. `bias` overrides the layer's own (a ResNet adds its time embedding there).
    /// With `residual`, the result is added onto it (and it is the output).
    pub fn conv3(&mut self, cv: &Conv, inp: In, stride: usize, bias: Option<&[f32]>, residual: Option<Act>) -> Act {
        let x = inp.x;
        assert_eq!(x.c, cv.cin, "conv: input channels");
        let up = inp.up as usize;
        let (he, we) = (x.h << up, x.w << up);
        let (ho, wo) = ((he - 1) / stride + 1, (we - 1) / stride + 1);
        let acc = residual.is_some();
        let out = residual.unwrap_or_else(|| Act::new(ho, wo, cv.n));
        assert!(out.h == ho && out.w == wo && out.c == cv.n, "conv: residual shape");
        let cin = cv.cin;
        let pan = self.panels(&cv.w, cin * 16, 9 * cv.npan);
        let rowf = (we + 2) * cin;
        self.ensure_scratch(3 * rowf);
        let scratch = self.scratch_ptrs();
        let slots = &self.slots;
        let bias_v: &[f32] = bias.unwrap_or(&cv.bias);
        let bias_p = P(bias_v.as_ptr() as *mut f32);
        let (npan, n) = (cv.npan, cv.n);
        // jobs: output rows x panel groups, enough of them to keep every core busy
        let want = kern::cores() * 3;
        let ngroups = want.div_ceil(ho).clamp(1, npan);
        let pg = npan.div_ceil(ngroups);
        let ngroups = npan.div_ceil(pg);
        let aff = inp.aff.map(|a| (P(a.sc.as_ptr() as *mut f32), P(a.sh.as_ptr() as *mut f32)));
        let silu = inp.silu;
        run_jobs(ho * ngroups, &|j: usize| {
            let (yo, g) = (j / ngroups, j % ngroups);
            let slot = slots.claim();
            let buf = scratch[slot].get();
            // the three padded input rows
            for ky in 0..3 {
                let r = unsafe { core::slice::from_raw_parts_mut(buf.add(ky * rowf), rowf) };
                let yi = (yo * stride + ky) as isize - 1;
                if yi < 0 || yi >= he as isize {
                    r.fill(0.0);
                    continue;
                }
                r[..cin].fill(0.0);
                r[(we + 1) * cin..].fill(0.0);
                let ys = (yi as usize) >> up;
                for xi in 0..we {
                    let s = unsafe { core::slice::from_raw_parts(x.px(ys * x.w + (xi >> up)), cin) };
                    let d = &mut r[(xi + 1) * cin..(xi + 2) * cin];
                    match aff {
                        Some((sc, sh)) => {
                            let (sc, sh) = unsafe { (core::slice::from_raw_parts(sc.get(), cin), core::slice::from_raw_parts(sh.get(), cin)) };
                            for i in 0..cin {
                                d[i] = s[i] * sc[i] + sh[i];
                            }
                        },
                        None => d.copy_from_slice(s),
                    }
                    if silu {
                        silu_inplace(d);
                    }
                }
            }
            for p in g * pg..((g + 1) * pg).min(npan) {
                let ncols = (n - p * 16).min(16);
                let mut xo = 0;
                while xo < wo {
                    let rows = (wo - xo).min(4);
                    let mut c = [core::ptr::null_mut::<f32>(); 4];
                    for r in 0..rows {
                        c[r] = unsafe { out.px(yo * wo + xo + r).add(p * 16) };
                    }
                    for t in 0..9 {
                        let (ky, kx) = (t / 3, t % 3);
                        let mut a = [core::ptr::null::<f32>(); 4];
                        for r in 0..rows {
                            a[r] = unsafe { buf.add(ky * rowf + ((xo + r) * stride + kx) * cin) };
                        }
                        let first = t == 0;
                        let bp = if first { unsafe { bias_p.get().add(p * 16) as *const f32 } } else { core::ptr::null() };
                        unsafe { kern::tile(cin, &a, rows, pan.get(t * npan + p), &c, ncols, acc || !first, bp) };
                    }
                    xo += rows;
                }
            }
            slots.release(slot);
        });
        out
    }

    // ── attention ───────────────────────────────────────────────────────────

    /// softmax(q k^T / sqrt(d)) v per head; q: T x C, k / v: S x C.
    pub fn attention(&mut self, q: &Act, k: &Act, v: &Act, heads: usize, causal: bool) -> Act {
        let (t_n, s_n, c) = (q.tokens(), k.tokens(), q.c);
        let d = c / heads;
        let nps = s_n.div_ceil(16);
        let npd = d.div_ceil(16);
        let spad = nps * 16;
        // K panels per head: [nps][d][16]; V panels per head: [npd][S][16]
        let mut kp = alloc::vec![0f32; heads * nps * d * 16];
        let mut vp = alloc::vec![0f32; heads * npd * s_n * 16];
        {
            let kpp = P(kp.as_mut_ptr());
            let vpp = P(vp.as_mut_ptr());
            run_jobs(heads * nps, &|j: usize| {
                let (h, ps) = (j / nps, j % nps);
                let base = unsafe { kpp.get().add((h * nps + ps) * d * 16) };
                for jj in 0..16 {
                    let s = ps * 16 + jj;
                    if s >= s_n {
                        break;
                    }
                    let row = unsafe { core::slice::from_raw_parts(k.px(s).add(h * d), d) };
                    for kk in 0..d {
                        unsafe { *base.add(kk * 16 + jj) = row[kk] };
                    }
                }
            });
            run_jobs(heads * npd, &|j: usize| {
                let (h, pd) = (j / npd, j % npd);
                let base = unsafe { vpp.get().add((h * npd + pd) * s_n * 16) };
                let cols = (d - pd * 16).min(16);
                for s in 0..s_n {
                    let row = unsafe { v.px(s).add(h * d + pd * 16) };
                    for jj in 0..cols {
                        unsafe { *base.add(s * 16 + jj) = *row.add(jj) };
                    }
                }
            });
        }
        let out = Act::new(q.h, q.w, c);
        const QB: usize = 32;
        self.ensure_scratch(QB * spad);
        let scratch = self.scratch_ptrs();
        let slots = &self.slots;
        let scale = 1.0 / libm::sqrtf(d as f32);
        let qblocks = t_n.div_ceil(QB);
        let (kpp, vpp) = (P(kp.as_ptr() as *mut f32), P(vp.as_ptr() as *mut f32));
        run_jobs(heads * qblocks, &|j: usize| {
            let (h, qb) = (j / qblocks, j % qblocks);
            let (t0, t1) = (qb * QB, ((qb + 1) * QB).min(t_n));
            let slot = slots.claim();
            let sb = scratch[slot].get();
            // scores
            let mut t = t0;
            while t < t1 {
                let rows = (t1 - t).min(4);
                let mut a = [core::ptr::null::<f32>(); 4];
                let mut cc = [core::ptr::null_mut::<f32>(); 4];
                for r in 0..rows {
                    a[r] = unsafe { q.px(t + r).add(h * d) };
                }
                for ps in 0..nps {
                    for r in 0..rows {
                        cc[r] = unsafe { sb.add((t + r - t0) * spad + ps * 16) };
                    }
                    unsafe { kern::tile(d, &a, rows, kpp.get().add((h * nps + ps) * d * 16), &cc, 16, false, core::ptr::null()) };
                }
                t += rows;
            }
            // softmax
            for t in t0..t1 {
                let row = unsafe { core::slice::from_raw_parts_mut(sb.add((t - t0) * spad), s_n) };
                let live = if causal { (t + 1).min(s_n) } else { s_n };
                let mut m = f32::MIN;
                for e in row[..live].iter_mut() {
                    *e *= scale;
                    if *e > m {
                        m = *e;
                    }
                }
                for e in row[..live].iter_mut() {
                    *e -= m;
                }
                kern::vexp(&mut row[..live]);
                let sum: f32 = row[..live].iter().sum();
                let inv = 1.0 / sum;
                for e in row[..live].iter_mut() {
                    *e *= inv;
                }
                for e in row[live..].iter_mut() {
                    *e = 0.0;
                }
            }
            // weighted values
            let mut t = t0;
            while t < t1 {
                let rows = (t1 - t).min(4);
                let mut a = [core::ptr::null::<f32>(); 4];
                let mut cc = [core::ptr::null_mut::<f32>(); 4];
                for r in 0..rows {
                    a[r] = unsafe { sb.add((t + r - t0) * spad) };
                }
                for pd in 0..npd {
                    for r in 0..rows {
                        cc[r] = unsafe { out.px(t + r).add(h * d + pd * 16) };
                    }
                    let cols = (d - pd * 16).min(16);
                    unsafe { kern::tile(s_n, &a, rows, vpp.get().add((h * npd + pd) * s_n * 16), &cc, cols, false, core::ptr::null()) };
                }
                t += rows;
            }
            slots.release(slot);
        });
        out
    }

    // ── element-wise ────────────────────────────────────────────────────────

    /// GEGLU: `h[:, :n] * gelu(h[:, n:])` with n = c / 2.
    pub fn geglu(&mut self, x: &Act) -> Act {
        let n = x.c / 2;
        let out = Act::new(x.h, x.w, n);
        let toks = x.tokens();
        let per = 16usize;
        run_jobs(toks.div_ceil(per), &|j: usize| {
            for t in j * per..((j + 1) * per).min(toks) {
                let s = unsafe { core::slice::from_raw_parts(x.px(t), 2 * n) };
                let d = unsafe { core::slice::from_raw_parts_mut(out.px(t), n) };
                for i in 0..n {
                    d[i] = s[i] * gelu(s[n + i]);
                }
            }
        });
        out
    }

    /// x * sigmoid(1.702 x), CLIP's activation, in place.
    pub fn quick_gelu(&mut self, x: &mut Act) {
        let toks = x.tokens();
        let c = x.c;
        let xr = &*x;
        run_jobs(toks.div_ceil(16), &|j: usize| {
            for t in j * 16..((j + 1) * 16).min(toks) {
                let d = unsafe { core::slice::from_raw_parts_mut(xr.px(t), c) };
                let mut tmp = [0f32; 64];
                let mut i = 0;
                while i < c {
                    let m = (c - i).min(64);
                    for q in 0..m {
                        tmp[q] = -1.702 * d[i + q];
                    }
                    kern::vexp(&mut tmp[..m]);
                    for q in 0..m {
                        d[i + q] /= 1.0 + tmp[q];
                    }
                    i += m;
                }
            }
        });
    }

    pub fn silu(&mut self, x: &mut Act) {
        let toks = x.tokens();
        let c = x.c;
        let xr = &*x;
        run_jobs(toks.div_ceil(64), &|j: usize| {
            for t in j * 64..((j + 1) * 64).min(toks) {
                silu_inplace(unsafe { core::slice::from_raw_parts_mut(xr.px(t), c) });
            }
        });
    }

    /// Channel concatenation [a | b].
    pub fn concat(&mut self, a: &Act, b: &Act) -> Act {
        let out = Act::new(a.h, a.w, a.c + b.c);
        let toks = a.tokens();
        run_jobs(toks.div_ceil(256), &|j: usize| {
            for t in j * 256..((j + 1) * 256).min(toks) {
                unsafe {
                    core::ptr::copy_nonoverlapping(a.px(t), out.px(t), a.c);
                    core::ptr::copy_nonoverlapping(b.px(t), out.px(t).add(a.c), b.c);
                }
            }
        });
        out
    }

    // ── ResNet block (UNet and VAE) ─────────────────────────────────────────

    /// GN -> SiLU -> conv (+ time embedding) -> GN -> SiLU -> conv, plus the (1x1-projected) input.
    pub fn res(&mut self, r: &Res, x: Act, emb: Option<&Act>) -> Act {
        let a1 = self.gn_affine(&x, &r.n1, 32, r.eps);
        let mut bias = r.c1.bias.clone();
        if let (Some(tl), Some(e)) = (&r.temb, emb) {
            let eo = self.linear(tl, e, None);
            for (b, v) in bias.iter_mut().zip(eo.tok(0).iter()) {
                *b += *v;
            }
        }
        let h = self.conv3(&r.c1, In { x: &x, up: false, aff: Some(&a1), silu: true }, 1, Some(&bias), None);
        let a2 = self.gn_affine(&h, &r.n2, 32, r.eps);
        let base = match &r.skip {
            Some(l) => {
                let b = self.linear(l, &x, None);
                drop(x);
                b
            },
            None => x,
        };
        self.conv3(&r.c2, In { x: &h, up: false, aff: Some(&a2), silu: true }, 1, None, Some(base))
    }
}

pub struct Res {
    pub n1: Norm,
    pub c1: Conv,
    pub temb: Option<Lin>,
    pub n2: Norm,
    pub c2: Conv,
    pub skip: Option<Lin>,
    pub eps: f32,
}

impl Res {
    /// LDM names: UNet `in_layers.0/2, emb_layers.1, out_layers.0/3, skip_connection`; VAE `norm1, conv1, norm2, conv2, nin_shortcut`.
    pub fn load(l: &mut Loader, base: &str, vae: bool) -> Result<Res, Error> {
        let f = |s: &str| alloc::format!("{base}.{s}");
        if vae {
            Ok(Res {
                n1: l.norm(&f("norm1"))?,
                c1: l.conv(&f("conv1"))?,
                temb: None,
                n2: l.norm(&f("norm2"))?,
                c2: l.conv(&f("conv2"))?,
                skip: if l.has(&f("nin_shortcut.weight")) { Some(l.lin(&f("nin_shortcut"))?) } else { None },
                eps: 1e-6,
            })
        } else {
            Ok(Res {
                n1: l.norm(&f("in_layers.0"))?,
                c1: l.conv(&f("in_layers.2"))?,
                temb: Some(l.lin(&f("emb_layers.1"))?),
                n2: l.norm(&f("out_layers.0"))?,
                c2: l.conv(&f("out_layers.3"))?,
                skip: if l.has(&f("skip_connection.weight")) { Some(l.lin(&f("skip_connection"))?) } else { None },
                eps: 1e-5,
            })
        }
    }
}
