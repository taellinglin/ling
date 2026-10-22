//! The master effects chain: drive, bit-crusher, ping-pong echo and a Schroeder reverb (the LingStationX effect rack at kernel scale).
//! Each knob is a level 0..=10; level 0 is off and costs nothing.
use alloc::vec;
use alloc::vec::Vec;

struct Comb {
    buf: Vec<f32>,
    i: usize,
    last: f32,
}
impl Comb {
    fn new(n: usize) -> Comb {
        Comb { buf: vec![0.0; n], i: 0, last: 0.0 }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let y = self.buf[self.i];
        self.last = y * 0.7 + self.last * 0.3;
        self.buf[self.i] = x + self.last * 0.84;
        self.i += 1;
        if self.i == self.buf.len() {
            self.i = 0;
        }
        y
    }
}

struct Allpass {
    buf: Vec<f32>,
    i: usize,
}
impl Allpass {
    fn new(n: usize) -> Allpass {
        Allpass { buf: vec![0.0; n], i: 0 }
    }
    #[inline]
    fn run(&mut self, x: f32) -> f32 {
        let b = self.buf[self.i];
        self.buf[self.i] = x + b * 0.5;
        self.i += 1;
        if self.i == self.buf.len() {
            self.i = 0;
        }
        b - x
    }
}

const ECHO_LEN: usize = 18_000; // 375 ms at 48 kHz: a dotted eighth near 120 bpm
const CRUSH_STEPS: [f32; 11] = [0.0, 512.0, 256.0, 128.0, 64.0, 48.0, 32.0, 24.0, 16.0, 12.0, 8.0];

pub struct Chain {
    el: Vec<f32>,
    er: Vec<f32>,
    ei: usize,
    combs: [[Comb; 4]; 2],
    aps: [[Allpass; 2]; 2],
    hold: [f32; 2],
    cnt: u32,
}

impl Chain {
    pub fn new() -> Chain {
        let cl = [1116usize, 1188, 1277, 1356];
        let al = [556usize, 441];
        Chain {
            el: vec![0.0; ECHO_LEN],
            er: vec![0.0; ECHO_LEN],
            ei: 0,
            combs: [
                [Comb::new(cl[0]), Comb::new(cl[1]), Comb::new(cl[2]), Comb::new(cl[3])],
                [Comb::new(cl[0] + 23), Comb::new(cl[1] + 23), Comb::new(cl[2] + 23), Comb::new(cl[3] + 23)],
            ],
            aps: [[Allpass::new(al[0]), Allpass::new(al[1])], [Allpass::new(al[0] + 23), Allpass::new(al[1] + 23)]],
            hold: [0.0; 2],
            cnt: 0,
        }
    }

    /// `lv` = [drive, echo, reverb, crush], each 0..=10
    pub fn process(&mut self, mut l: f32, mut r: f32, lv: [u8; 4]) -> (f32, f32) {
        let [drive, echo, verb, crush] = [lv[0].min(10), lv[1].min(10), lv[2].min(10), lv[3].min(10)];
        if drive | echo | verb | crush == 0 {
            return (l, r);
        }
        if drive > 0 {
            let g = 1.0 + drive as f32 * 0.9;
            let n = 0.8 / libm::tanhf(g);
            l = libm::tanhf(l * g) * n;
            r = libm::tanhf(r * g) * n;
        }
        if crush > 0 {
            let steps = CRUSH_STEPS[crush as usize];
            let hold = 1 + crush as u32 / 2;
            if self.cnt % hold == 0 {
                let h = steps * 0.5;
                self.hold = [libm::floorf(l * h + 0.5) / h, libm::floorf(r * h + 0.5) / h];
            }
            self.cnt = self.cnt.wrapping_add(1);
            l = self.hold[0];
            r = self.hold[1];
        }
        if echo > 0 {
            let (wl, wr) = (self.el[self.ei], self.er[self.ei]);
            self.el[self.ei] = l + wr * 0.45;
            self.er[self.ei] = r + wl * 0.45;
            self.ei += 1;
            if self.ei == ECHO_LEN {
                self.ei = 0;
            }
            let m = echo as f32 * 0.07;
            l += wl * m;
            r += wr * m;
        }
        if verb > 0 {
            let x = (l + r) * 0.5 * 0.3;
            let mut w = [0.0f32; 2];
            for s in 0..2 {
                let mut a = 0.0;
                for c in self.combs[s].iter_mut() {
                    a += c.run(x);
                }
                a *= 0.25;
                for ap in self.aps[s].iter_mut() {
                    a = ap.run(a);
                }
                w[s] = a;
            }
            let m = verb as f32 * 0.09;
            l += w[0] * m;
            r += w[1] * m;
        }
        (l, r)
    }
}
