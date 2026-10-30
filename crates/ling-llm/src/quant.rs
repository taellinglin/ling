//! Weights and the matrix-vector products over them.
//!
//! A tensor is held as a list of chunks of whole rows, each at most [`CHUNK_BYTES`], so no single allocation is large (LingOS's frame
//! allocator hands out 64 MiB at most, and the token embedding of Qwen3-0.6B alone is 165 MB). Products run over a *batch* of vectors:
//! the weight rows are the outer loop and the vectors the inner one, so prompt processing reads every weight once per batch instead of
//! once per token.
//!
//! Q8_0 weights are multiplied in integers against the activations quantized to Q8_0 as well (the way llama.cpp and candle do it, so the
//! results agree to rounding); the K-quants are dequantized a block at a time and multiplied in f32.
use crate::error::Error;
use crate::f16;
use crate::gguf::{DType, Source};
use alloc::vec;
use alloc::vec::Vec;

/// the largest piece of a weight tensor held in one allocation
pub const CHUNK_BYTES: usize = 8 << 20;

/// Rows per chunk: the chunk size (a power of two from 64 KiB to [`CHUNK_BYTES`], less a little room for an allocator's header) that
/// wastes the least once every chunk is rounded up to a power of two, as a buddy allocator does (LingOS's: a 2.2 MB tensor in one
/// piece takes 4 MiB there, an exact 4 MiB piece 8 MiB). Ties go to the bigger chunks.
fn rows_per_chunk(rows: usize, row_bytes: usize) -> usize {
    const SLACK: usize = 64;
    let held = |b: usize| (b + SLACK).next_power_of_two();
    let mut best = ((CHUNK_BYTES - SLACK) / row_bytes).max(1);
    let mut best_cost = usize::MAX;
    let mut p = CHUNK_BYTES;
    while p >= 64 << 10 {
        let rpc = (p - SLACK) / row_bytes;
        if rpc >= 1 {
            let (full, rem) = (rows / rpc, rows % rpc);
            let cost = full * held(rpc * row_bytes) + if rem > 0 { held(rem * row_bytes) } else { 0 };
            if cost < best_cost {
                best_cost = cost;
                best = rpc;
            }
        }
        p /= 2;
    }
    best
}

pub struct QTensor {
    pub dtype: DType,
    pub rows: usize,
    pub cols: usize,
    row_bytes: usize,
    rows_per_chunk: usize,
    chunks: Vec<Vec<u8>>,
}

impl QTensor {
    /// Read `rows` x `cols` elements of `dtype` starting at the absolute offset `abs`.
    pub fn load(src: &dyn Source, abs: u64, rows: usize, cols: usize, dtype: DType) -> Result<QTensor, Error> {
        if cols == 0 || rows == 0 || cols % dtype.block() != 0 {
            return Err(Error::Format("a tensor's row length does not fit its block size"));
        }
        let row_bytes = dtype.row_bytes(cols);
        if abs.checked_add((rows * row_bytes) as u64).map_or(true, |end| end > src.len()) {
            return Err(Error::Format("a tensor runs past the end of the file"));
        }
        let rows_per_chunk = rows_per_chunk(rows, row_bytes);
        let mut chunks = Vec::new();
        let mut start = 0;
        while start < rows {
            let n = rows_per_chunk.min(rows - start);
            let mut v = vec![0u8; n * row_bytes];
            src.read_at(abs + (start * row_bytes) as u64, &mut v)?;
            chunks.push(v);
            start += n;
        }
        Ok(QTensor { dtype, rows, cols, row_bytes, rows_per_chunk, chunks })
    }

    /// Build from rows of f32 (tests and tiny tensors).
    pub fn from_f32(rows: usize, cols: usize, data: &[f32]) -> QTensor {
        assert_eq!(data.len(), rows * cols);
        let bytes: Vec<u8> = data.iter().flat_map(|x| x.to_le_bytes()).collect();
        QTensor { dtype: DType::F32, rows, cols, row_bytes: cols * 4, rows_per_chunk: rows, chunks: vec![bytes] }
    }

    #[inline]
    pub fn row(&self, r: usize) -> &[u8] {
        let c = r / self.rows_per_chunk;
        let o = (r % self.rows_per_chunk) * self.row_bytes;
        &self.chunks[c][o..o + self.row_bytes]
    }

    pub fn bytes(&self) -> usize {
        self.rows * self.row_bytes
    }

    /// one row as f32 (the embedding lookup)
    pub fn dequant_row(&self, r: usize, out: &mut [f32]) {
        let row = self.row(r);
        match self.dtype {
            DType::F32 => {
                for (i, o) in out.iter_mut().enumerate().take(self.cols) {
                    *o = f32::from_le_bytes([row[i * 4], row[i * 4 + 1], row[i * 4 + 2], row[i * 4 + 3]]);
                }
            }
            DType::F16 => {
                for (i, o) in out.iter_mut().enumerate().take(self.cols) {
                    *o = f16::to_f32(u16::from_le_bytes([row[i * 2], row[i * 2 + 1]]));
                }
            }
            DType::Q8_0 => {
                for b in 0..self.cols / 32 {
                    let blk = &row[b * 34..b * 34 + 34];
                    let d = f16::to_f32(u16::from_le_bytes([blk[0], blk[1]]));
                    for i in 0..32 {
                        out[b * 32 + i] = d * (blk[2 + i] as i8) as f32;
                    }
                }
            }
            DType::Q4K => {
                let mut tmp = [0.0f32; 256];
                for b in 0..self.cols / 256 {
                    dequant_q4k(&row[b * 144..b * 144 + 144], &mut tmp);
                    out[b * 256..b * 256 + 256].copy_from_slice(&tmp);
                }
            }
            DType::Q6K => {
                let mut tmp = [0.0f32; 256];
                for b in 0..self.cols / 256 {
                    dequant_q6k(&row[b * 210..b * 210 + 210], &mut tmp);
                    out[b * 256..b * 256 + 256].copy_from_slice(&tmp);
                }
            }
        }
    }
}

// ── block dequantizers (the K-quants follow ggml's layouts) ─────────────────────
#[inline]
fn scale_min_k4(j: usize, q: &[u8]) -> (u8, u8) {
    if j < 4 {
        (q[j] & 63, q[j + 4] & 63)
    } else {
        ((q[j + 4] & 0xf) | ((q[j - 4] >> 6) << 4), (q[j + 4] >> 4) | ((q[j] >> 6) << 4))
    }
}

/// 144 bytes: d (f16), dmin (f16), 12 bytes of packed 6-bit scales and mins, 128 bytes of 4-bit quants
pub fn dequant_q4k(blk: &[u8], y: &mut [f32; 256]) {
    let d = f16::to_f32(u16::from_le_bytes([blk[0], blk[1]]));
    let dmin = f16::to_f32(u16::from_le_bytes([blk[2], blk[3]]));
    let scales = &blk[4..16];
    let qs = &blk[16..144];
    let mut is = 0;
    for j in 0..4 {
        let q = &qs[j * 32..j * 32 + 32];
        let (sc, m) = scale_min_k4(is, scales);
        let (d1, m1) = (d * sc as f32, dmin * m as f32);
        let (sc, m) = scale_min_k4(is + 1, scales);
        let (d2, m2) = (d * sc as f32, dmin * m as f32);
        for l in 0..32 {
            y[j * 64 + l] = d1 * (q[l] & 0xf) as f32 - m1;
            y[j * 64 + 32 + l] = d2 * (q[l] >> 4) as f32 - m2;
        }
        is += 2;
    }
}

/// 210 bytes: 128 bytes of low 4 bits, 64 bytes of high 2 bits, 16 signed scales, d (f16)
pub fn dequant_q6k(blk: &[u8], y: &mut [f32; 256]) {
    let ql = &blk[0..128];
    let qh = &blk[128..192];
    let sc = &blk[192..208];
    let d = f16::to_f32(u16::from_le_bytes([blk[208], blk[209]]));
    for half in 0..2 {
        let (ql, qh, sc, y) = (&ql[half * 64..], &qh[half * 32..], &sc[half * 8..], &mut y[half * 128..]);
        for l in 0..32 {
            let is = l / 16;
            let q1 = ((ql[l] & 0xf) | ((qh[l] & 3) << 4)) as i32 - 32;
            let q2 = ((ql[l + 32] & 0xf) | (((qh[l] >> 2) & 3) << 4)) as i32 - 32;
            let q3 = ((ql[l] >> 4) | (((qh[l] >> 4) & 3) << 4)) as i32 - 32;
            let q4 = ((ql[l + 32] >> 4) | (((qh[l] >> 6) & 3) << 4)) as i32 - 32;
            y[l] = d * (sc[is] as i8) as f32 * q1 as f32;
            y[l + 32] = d * (sc[is + 2] as i8) as f32 * q2 as f32;
            y[l + 64] = d * (sc[is + 4] as i8) as f32 * q3 as f32;
            y[l + 96] = d * (sc[is + 6] as i8) as f32 * q4 as f32;
        }
    }
}

// ── activations, in f32 and quantized to Q8_0 ───────────────────────────────────
/// `b` vectors of length `n`: write `x` (b * n floats), then call [`XBatch::set_len`] with the batch size
pub struct XBatch {
    pub n: usize,
    pub b: usize,
    pub x: Vec<f32>,
    q: Vec<i8>,
    d: Vec<f32>,
    quantized: bool,
}

impl XBatch {
    pub fn new(max_batch: usize, n: usize) -> XBatch {
        XBatch { n, b: 0, x: vec![0.0; max_batch * n], q: vec![0; max_batch * n], d: vec![0.0; max_batch * (n / 32).max(1)], quantized: false }
    }
    /// `b` vectors are now in `x`
    pub fn set_len(&mut self, b: usize) {
        self.b = b;
        self.quantized = false;
    }
    pub fn vec(&self, t: usize) -> &[f32] {
        &self.x[t * self.n..(t + 1) * self.n]
    }
    pub fn vec_mut(&mut self, t: usize) -> &mut [f32] {
        &mut self.x[t * self.n..(t + 1) * self.n]
    }
    fn quantize(&mut self) {
        if self.quantized || self.n % 32 != 0 {
            return;
        }
        let nb = self.n / 32;
        for t in 0..self.b {
            for blk in 0..nb {
                let xs = &self.x[t * self.n + blk * 32..t * self.n + blk * 32 + 32];
                let amax = xs.iter().fold(0.0f32, |a, v| a.max(crate::m::abs(*v)));
                let d = amax / 127.0;
                let id = if d > 0.0 { 1.0 / d } else { 0.0 };
                self.d[t * nb + blk] = d;
                let qs = &mut self.q[t * self.n + blk * 32..t * self.n + blk * 32 + 32];
                for i in 0..32 {
                    qs[i] = crate::m::round(xs[i] * id).clamp(-127.0, 127.0) as i8;
                }
            }
        }
        self.quantized = true;
    }
}

#[inline]
pub fn dot_f32(a: &[f32], b: &[f32]) -> f32 {
    let mut s = [0.0f32; 4];
    let n = a.len().min(b.len());
    let mut i = 0;
    while i + 4 <= n {
        s[0] += a[i] * b[i];
        s[1] += a[i + 1] * b[i + 1];
        s[2] += a[i + 2] * b[i + 2];
        s[3] += a[i + 3] * b[i + 3];
        i += 4;
    }
    let mut t = s[0] + s[1] + s[2] + s[3];
    while i < n {
        t += a[i] * b[i];
        i += 1;
    }
    t
}

#[inline]
fn dot_q8_0(row: &[u8], qx: &[i8], dx: &[f32]) -> f32 {
    let mut sum = 0.0f32;
    for (b, blk) in row.chunks_exact(34).enumerate() {
        let d = f16::to_f32(u16::from_le_bytes([blk[0], blk[1]]));
        let q = &blk[2..34];
        let x = &qx[b * 32..b * 32 + 32];
        let mut acc = 0i32;
        for i in 0..32 {
            acc += (q[i] as i8 as i32) * (x[i] as i32);
        }
        sum += d * dx[b] * acc as f32;
    }
    sum
}

static YIELD: core::sync::atomic::AtomicUsize = core::sync::atomic::AtomicUsize::new(0);

/// Register a function the engine calls every few dozen weight rows. A kernel uses it to hand the CPU back to the desktop while a
/// long generation runs (the function itself decides whether it is time to).
pub fn set_yield(f: fn()) {
    YIELD.store(f as usize, core::sync::atomic::Ordering::Relaxed);
}

#[inline]
fn pump() {
    let p = YIELD.load(core::sync::atomic::Ordering::Relaxed);
    if p != 0 {
        // SAFETY: only `set_yield` stores here, and it stores a `fn()`
        let f: fn() = unsafe { core::mem::transmute(p) };
        f();
    }
}

/// out[t * rows + r] = row r of `w` . vector t of `x`, for every t in the batch
pub fn matmul(w: &QTensor, x: &mut XBatch, out: &mut [f32]) {
    debug_assert_eq!(x.n, w.cols);
    let b = x.b;
    let rows = w.rows;
    match w.dtype {
        DType::Q8_0 => {
            x.quantize();
            let nb = w.cols / 32;
            for r in 0..rows {
                if r & 31 == 31 {
                    pump();
                }
                let row = w.row(r);
                for t in 0..b {
                    out[t * rows + r] = dot_q8_0(row, &x.q[t * x.n..(t + 1) * x.n], &x.d[t * nb..(t + 1) * nb]);
                }
            }
        }
        DType::F32 => {
            let mut tmp = vec![0.0f32; w.cols];
            for r in 0..rows {
                if r & 31 == 31 {
                    pump();
                }
                w.dequant_row(r, &mut tmp);
                for t in 0..b {
                    out[t * rows + r] = dot_f32(&tmp, x.vec(t));
                }
            }
        }
        DType::F16 => {
            let mut tmp = vec![0.0f32; w.cols];
            for r in 0..rows {
                if r & 31 == 31 {
                    pump();
                }
                w.dequant_row(r, &mut tmp);
                for t in 0..b {
                    out[t * rows + r] = dot_f32(&tmp, x.vec(t));
                }
            }
        }
        DType::Q4K | DType::Q6K => {
            let mut tmp = vec![0.0f32; w.cols];
            for r in 0..rows {
                if r & 31 == 31 {
                    pump();
                }
                w.dequant_row(r, &mut tmp);
                for t in 0..b {
                    out[t * rows + r] = dot_f32(&tmp, x.vec(t));
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn q8_0_matmul_matches_the_f32_product_closely() {
        // a 5 x 64 matrix quantized to Q8_0, and three vectors
        let (rows, cols) = (5usize, 64usize);
        let w: Vec<f32> = (0..rows * cols).map(|i| ((i * 37 % 101) as f32 - 50.0) / 25.0).collect();
        let mut bytes = Vec::new();
        for r in 0..rows {
            for b in 0..cols / 32 {
                let blk = &w[r * cols + b * 32..r * cols + b * 32 + 32];
                let amax = blk.iter().fold(0.0f32, |a, v| a.max(v.abs()));
                let d = amax / 127.0;
                bytes.extend(f16::from_f32(d).to_le_bytes());
                for v in blk {
                    bytes.push(((v / d).round() as i8) as u8);
                }
            }
        }
        let t = QTensor::load(&bytes.as_slice(), 0, rows, cols, DType::Q8_0).unwrap();
        let mut x = XBatch::new(3, cols);
        for k in 0..3 {
            for (i, v) in x.vec_mut(k).iter_mut().enumerate() {
                *v = ((i * (k + 3) * 13 % 29) as f32 - 14.0) / 7.0;
            }
        }
        x.set_len(3);
        let mut out = vec![0.0f32; 3 * rows];
        let xs: Vec<Vec<f32>> = (0..3).map(|k| x.vec(k).to_vec()).collect();
        matmul(&t, &mut x, &mut out);
        // the same product made by hand from the dequantized weights and the dequantized activations: the kernel must agree exactly
        let mut wdq = vec![0.0f32; cols];
        for k in 0..3 {
            let mut xdq = vec![0.0f32; cols];
            for blk in 0..cols / 32 {
                let part = &xs[k][blk * 32..blk * 32 + 32];
                let amax = part.iter().fold(0.0f32, |a, v| a.max(v.abs()));
                let d = amax / 127.0;
                for (i, v) in part.iter().enumerate() {
                    xdq[blk * 32 + i] = ((v / d).round().clamp(-127.0, 127.0)) * d;
                }
            }
            for r in 0..rows {
                t.dequant_row(r, &mut wdq);
                let by_hand: f32 = (0..cols).map(|i| wdq[i] * xdq[i]).sum();
                assert!((out[k * rows + r] - by_hand).abs() < 1e-3 * (1.0 + by_hand.abs()), "{k},{r}: {} vs {by_hand}", out[k * rows + r]);
                // and it stays near the true f32 product (the quantization noise of an 8-bit weight and an 8-bit activation)
                let exact: f32 = (0..cols).map(|i| w[r * cols + i] * xs[k][i]).sum();
                assert!((out[k * rows + r] - exact).abs() < 0.2 + 0.05 * exact.abs(), "{k},{r}: {} vs exact {exact}", out[k * rows + r]);
            }
        }
    }

    #[test]
    fn big_tensors_are_held_in_chunks() {
        let (rows, cols) = (40000usize, 64usize); // 40000 * 4 bytes... as F32 rows of 256 bytes = ~10 MB
        let data = vec![0.5f32; rows * cols];
        let bytes: Vec<u8> = data.iter().flat_map(|v| v.to_le_bytes()).collect();
        let t = QTensor::load(&bytes.as_slice(), 0, rows, cols, DType::F32).unwrap();
        assert!(t.chunks.len() >= 2, "{} chunk(s)", t.chunks.len());
        assert!(t.chunks.iter().all(|c| c.len() <= CHUNK_BYTES));
        let mut r = vec![0.0f32; cols];
        t.dequant_row(rows - 1, &mut r);
        assert!(r.iter().all(|v| *v == 0.5));
    }
}
