//! The host engine's noise, bit for bit: rand 0.8's `StdRng` (ChaCha12, `seed_from_u64` through PCG32), `gen::<f64>()`, and
//! `pipeline::randn`'s Box-Muller. The same seed therefore starts from the same latent on the GPU server and in LingOS.

pub struct StdRng {
    key: [u32; 8],
    counter: u64,
    buf: [u32; 64],
    idx: usize,
}

#[inline(always)]
fn qr(s: &mut [u32; 16], a: usize, b: usize, c: usize, d: usize) {
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(16);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(12);
    s[a] = s[a].wrapping_add(s[b]);
    s[d] = (s[d] ^ s[a]).rotate_left(8);
    s[c] = s[c].wrapping_add(s[d]);
    s[b] = (s[b] ^ s[c]).rotate_left(7);
}

impl StdRng {
    pub fn seed_from_u64(mut state: u64) -> StdRng {
        let mut seed = [0u8; 32];
        for chunk in seed.chunks_exact_mut(4) {
            const MUL: u64 = 6364136223846793005;
            const INC: u64 = 11634580027462260723;
            state = state.wrapping_mul(MUL).wrapping_add(INC);
            let s = state;
            let xorshifted = (((s >> 18) ^ s) >> 27) as u32;
            let rot = (s >> 59) as u32;
            chunk.copy_from_slice(&xorshifted.rotate_right(rot).to_le_bytes());
        }
        let mut key = [0u32; 8];
        for (i, k) in key.iter_mut().enumerate() {
            *k = u32::from_le_bytes([seed[i * 4], seed[i * 4 + 1], seed[i * 4 + 2], seed[i * 4 + 3]]);
        }
        StdRng { key, counter: 0, buf: [0; 64], idx: 64 }
    }

    fn refill(&mut self) {
        for b in 0..4 {
            let ctr = self.counter + b as u64;
            let mut s = [0u32; 16];
            s[0] = 0x6170_7865;
            s[1] = 0x3320_646e;
            s[2] = 0x7962_2d32;
            s[3] = 0x6b20_6574;
            s[4..12].copy_from_slice(&self.key);
            s[12] = ctr as u32;
            s[13] = (ctr >> 32) as u32;
            s[14] = 0;
            s[15] = 0;
            let init = s;
            for _ in 0..6 {
                qr(&mut s, 0, 4, 8, 12);
                qr(&mut s, 1, 5, 9, 13);
                qr(&mut s, 2, 6, 10, 14);
                qr(&mut s, 3, 7, 11, 15);
                qr(&mut s, 0, 5, 10, 15);
                qr(&mut s, 1, 6, 11, 12);
                qr(&mut s, 2, 7, 8, 13);
                qr(&mut s, 3, 4, 9, 14);
            }
            for i in 0..16 {
                self.buf[b * 16 + i] = s[i].wrapping_add(init[i]);
            }
        }
        self.counter += 4;
    }

    pub fn next_u64(&mut self) -> u64 {
        let len = 64;
        if self.idx < len - 1 {
            let v = (self.buf[self.idx + 1] as u64) << 32 | self.buf[self.idx] as u64;
            self.idx += 2;
            v
        } else if self.idx >= len {
            self.refill();
            self.idx = 2;
            (self.buf[1] as u64) << 32 | self.buf[0] as u64
        } else {
            let x = self.buf[len - 1] as u64;
            self.refill();
            self.idx = 1;
            (self.buf[0] as u64) << 32 | x
        }
    }

    /// `rng.gen::<f64>()`: 53 random bits in [0, 1).
    pub fn gen_f64(&mut self) -> f64 {
        (self.next_u64() >> 11) as f64 * (1.0 / (1u64 << 53) as f64)
    }

    /// `pipeline::randn`: `n` standard normal samples (Box-Muller, both of each pair).
    pub fn randn(&mut self, n: usize, out: &mut alloc::vec::Vec<f32>) {
        let start = out.len();
        while out.len() - start < n {
            let u1 = self.gen_f64().max(1e-12);
            let u2 = self.gen_f64();
            let r = libm::sqrt(-2.0 * libm::log(u1));
            let a = 2.0 * core::f64::consts::PI * u2;
            out.push((r * libm::cos(a)) as f32);
            out.push((r * libm::sin(a)) as f32);
        }
        out.truncate(start + n);
    }
}
