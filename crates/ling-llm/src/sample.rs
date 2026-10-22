//! Picking the next token from the logits: greedy, or temperature with top-p (nucleus) sampling.
use crate::m;
use alloc::vec::Vec;

pub struct Sampler {
    state: u64,
    pub temperature: f32,
    pub top_p: f32,
}

impl Sampler {
    pub fn new(seed: u64, temperature: f32, top_p: f32) -> Sampler {
        Sampler { state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1, temperature, top_p }
    }

    /// xorshift64*, a uniform number in [0, 1)
    fn uniform(&mut self) -> f32 {
        let mut x = self.state;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.state = x;
        let r = x.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((r >> 40) as f32) / (1u64 << 24) as f32
    }

    pub fn sample(&mut self, logits: &[f32]) -> u32 {
        let mut best = 0usize;
        let mut maxv = f32::NEG_INFINITY;
        for (i, l) in logits.iter().enumerate() {
            if *l > maxv {
                maxv = *l;
                best = i;
            }
        }
        if self.temperature <= 0.0 || !maxv.is_finite() {
            return best as u32;
        }
        // keep the tokens that matter (a thousandth of the best one or more), then the nucleus of those
        let inv_t = 1.0 / self.temperature;
        let mut cand: Vec<(u32, f32)> = Vec::new();
        for (i, l) in logits.iter().enumerate() {
            let p = m::exp((*l - maxv) * inv_t);
            if p >= 1e-3 {
                cand.push((i as u32, p));
            }
        }
        cand.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(core::cmp::Ordering::Equal));
        let total: f32 = cand.iter().map(|c| c.1).sum();
        let mut keep = cand.len();
        if self.top_p > 0.0 && self.top_p < 1.0 {
            let mut acc = 0.0;
            for (k, c) in cand.iter().enumerate() {
                acc += c.1 / total;
                if acc >= self.top_p {
                    keep = k + 1;
                    break;
                }
            }
        }
        let cand = &cand[..keep];
        let sum: f32 = cand.iter().map(|c| c.1).sum();
        let mut r = self.uniform() * sum;
        for c in cand {
            if r < c.1 {
                return c.0;
            }
            r -= c.1;
        }
        cand.last().map_or(best as u32, |c| c.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn greedy_picks_the_largest_and_sampling_stays_in_the_nucleus() {
        let logits = [0.1f32, 3.0, 0.2, 2.9, -5.0];
        assert_eq!(Sampler::new(1, 0.0, 1.0).sample(&logits), 1);
        let mut s = Sampler::new(7, 1.0, 0.5);
        let mut seen = [0usize; 5];
        for _ in 0..400 {
            seen[s.sample(&logits) as usize] += 1;
        }
        // the nucleus of 50 % holds the top token (and maybe the second); the tail never appears
        assert_eq!(seen[0] + seen[2] + seen[4], 0, "{seen:?}");
        assert!(seen[1] > 100, "{seen:?}");
        // a hot sampler with the whole distribution reaches the second token too
        let mut hot = Sampler::new(9, 1.0, 1.0);
        let mut seen2 = [0usize; 5];
        for _ in 0..400 {
            seen2[hot.sample(&logits) as usize] += 1;
        }
        assert!(seen2[1] > 50 && seen2[3] > 50, "{seen2:?}");
        // repeatable
        let a: Vec<u32> = { let mut s = Sampler::new(3, 0.8, 0.9); (0..20).map(|_| s.sample(&logits)).collect() };
        let b: Vec<u32> = { let mut s = Sampler::new(3, 0.8, 0.9); (0..20).map(|_| s.sample(&logits)).collect() };
        assert_eq!(a, b);
    }
}
