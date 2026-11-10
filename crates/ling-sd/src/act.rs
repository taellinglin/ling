//! Activations: an `h x w` grid of tokens, `c` f32 channels each (channels innermost, "HWC"), held in pieces of whole grid rows,
//! each under 32 MiB. LingOS's allocator hands out at most 64 MiB in one piece and rounds every large request up to a power of
//! two, so one 512x512x256 VAE activation (256 MiB) has to be several pieces, and pieces just under 32 MiB waste nothing.
use alloc::vec::Vec;

const SEG_FLOATS: usize = (32 << 20) / 4 - 4096;

pub struct Act {
    pub h: usize,
    pub w: usize,
    pub c: usize,
    /// grid rows per piece
    rps: usize,
    segs: Vec<Vec<f32>>,
    ptrs: Vec<*mut f32>,
}

// Jobs on other cores read and write disjoint tokens through the raw piece pointers.
unsafe impl Sync for Act {}
unsafe impl Send for Act {}

impl Act {
    pub fn new(h: usize, w: usize, c: usize) -> Act {
        let row = (w * c).max(1);
        let rps = (SEG_FLOATS / row).max(1).min(h.max(1));
        let mut segs = Vec::new();
        let mut y = 0;
        while y < h.max(1) {
            let rows = rps.min(h.max(1) - y);
            segs.push(alloc::vec![0f32; rows * row]);
            y += rows;
        }
        let ptrs = segs.iter_mut().map(|s| s.as_mut_ptr()).collect();
        Act { h, w, c, rps, segs, ptrs }
    }

    /// A 1 x n grid (a token sequence).
    pub fn seq(n: usize, c: usize) -> Act {
        Act::new(1, n, c)
    }

    pub fn tokens(&self) -> usize {
        self.h * self.w
    }

    /// Token `p`'s channels.
    #[inline(always)]
    pub fn px(&self, p: usize) -> *mut f32 {
        let (y, x) = (p / self.w, p % self.w);
        let s = y / self.rps;
        unsafe { self.ptrs[s].add(((y - s * self.rps) * self.w + x) * self.c) }
    }

    pub fn tok(&self, p: usize) -> &[f32] {
        unsafe { core::slice::from_raw_parts(self.px(p), self.c) }
    }

    pub fn tok_mut(&mut self, p: usize) -> &mut [f32] {
        unsafe { core::slice::from_raw_parts_mut(self.px(p), self.c) }
    }

    /// The pieces, as (first token, slice): for whole-tensor element-wise work.
    pub fn pieces(&self) -> impl Iterator<Item = &[f32]> {
        self.segs.iter().map(|s| s.as_slice())
    }

    pub fn pieces_mut(&mut self) -> impl Iterator<Item = &mut [f32]> {
        self.segs.iter_mut().map(|s| s.as_mut_slice())
    }

    pub fn fill(&mut self, v: f32) {
        for s in self.segs.iter_mut() {
            s.fill(v);
        }
    }

    /// Everything as one flat Vec (tokens x channels), for tests and small tensors.
    pub fn to_vec(&self) -> Vec<f32> {
        let mut v = Vec::with_capacity(self.tokens() * self.c);
        for s in &self.segs {
            v.extend_from_slice(s);
        }
        v
    }

    pub fn from_slice(h: usize, w: usize, c: usize, data: &[f32]) -> Act {
        let mut a = Act::new(h, w, c);
        for p in 0..h * w {
            a.tok_mut(p).copy_from_slice(&data[p * c..(p + 1) * c]);
        }
        a
    }

    /// A copy (same shape).
    pub fn copy(&self) -> Act {
        let mut a = Act::new(self.h, self.w, self.c);
        for (d, s) in a.segs.iter_mut().zip(self.segs.iter()) {
            d.copy_from_slice(s);
        }
        a
    }

    /// From channel-major planes (CHW, as torch / candle hold them).
    pub fn from_chw(c: usize, h: usize, w: usize, data: &[f32]) -> Act {
        let mut a = Act::new(h, w, c);
        for p in 0..h * w {
            let t = a.tok_mut(p);
            for ch in 0..c {
                t[ch] = data[ch * h * w + p];
            }
        }
        a
    }

    /// To channel-major planes.
    pub fn to_chw(&self) -> Vec<f32> {
        let n = self.tokens();
        let mut v = alloc::vec![0f32; n * self.c];
        for p in 0..n {
            let t = self.tok(p);
            for ch in 0..self.c {
                v[ch * n + p] = t[ch];
            }
        }
        v
    }
}
