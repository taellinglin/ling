// Rust AGC benchmark suite
use std::time::Instant;

const PI: f64 = 3.141592653589793;

fn modexp(base: i64, exp: i64, m: i64) -> i64 {
    let mut b = base % m;
    let mut e = exp;
    let mut out = 1_i64;
    while e > 0 {
        if e & 1 == 1 {
            out = (out * b) % m;
        }
        b = (b * b) % m;
        e >>= 1;
    }
    out
}

fn audio_fm_poly() -> f64 {
    let n = 250_000_i64;
    let sr = 48_000.0_f64;
    let mut s = 0.0_f64;
    let mut j = 0_i64;
    while j < n {
        let t = j as f64 / sr;
        let mut v = 0.0_f64;
        let mut vi = 1_i64;
        while vi <= 8 {
            let f = 110.0 * vi as f64;
            v += (2.0 * PI * f * t + 0.5 * (2.0 * PI * (f * 2.0) * t).sin()).sin();
            vi += 1;
        }
        s += v;
        j += 1;
    }
    s
}

fn audio_iir_bank() -> f64 {
    let (mut y1, mut y2, mut y3, mut y4) = (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    let mut acc = 0.0_f64;
    let mut n = 0_i64;
    while n < 300_000 {
        let x = (0.013 * n as f64).sin() + 0.5 * (0.017 * n as f64).sin();
        y1 = 0.995 * y1 + 0.005 * x;
        y2 = 0.990 * y2 + 0.010 * y1;
        y3 = 0.985 * y3 + 0.015 * y2;
        y4 = 0.980 * y4 + 0.020 * y3;
        acc += y4;
        n += 1;
    }
    acc
}

fn audio_delay_net() -> f64 {
    let (mut s1, mut s2, mut s3, mut s4, mut s5, mut s6, mut s7, mut s8) =
        (0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64, 0.0_f64);
    let mut acc = 0.0_f64;
    let mut i = 0_i64;
    while i < 300_000 {
        let x = (0.011 * i as f64).sin() + 0.25 * (0.029 * i as f64).sin();
        let y = x + 0.7 * s8;
        s8 = s7;
        s7 = s6;
        s6 = s5;
        s5 = s4;
        s4 = s3;
        s3 = s2;
        s2 = s1;
        s1 = y;
        acc += y;
        i += 1;
    }
    acc
}

fn gfx_mandelbrot() -> i64 {
    let (w, h, maxiter) = (240_i64, 180_i64, 120_i64);
    let mut total = 0_i64;
    let mut py = 0_i64;
    while py < h {
        let mut px = 0_i64;
        while px < w {
            let x0 = (px as f64 / w as f64) * 3.5 - 2.5;
            let y0 = (py as f64 / h as f64) * 2.0 - 1.0;
            let (mut zx, mut zy) = (0.0_f64, 0.0_f64);
            let mut it = 0_i64;
            while zx * zx + zy * zy <= 4.0 && it < maxiter {
                let xt = zx * zx - zy * zy + x0;
                zy = 2.0 * zx * zy + y0;
                zx = xt;
                it += 1;
            }
            total += it;
            px += 1;
        }
        py += 1;
    }
    total
}

fn gfx_particles() -> f64 {
    let mut psum = 0.0_f64;
    let mut p = 0_i64;
    while p < 20_000 {
        let mut x = (p % 257) as f64 * 0.01 - 1.28;
        let mut y = (p % 263) as f64 * 0.01 - 1.31;
        let mut vx = (p % 17) as f64 * 0.001;
        let mut vy = (p % 19) as f64 * 0.001;
        let mut s = 0_i64;
        while s < 120 {
            let ax = -0.0007 * x + 0.0003 * y;
            let ay = -0.0007 * y - 0.0003 * x;
            vx = (vx + ax) * 0.999;
            vy = (vy + ay) * 0.999;
            x += vx;
            y += vy;
            s += 1;
        }
        psum += x + y;
        p += 1;
    }
    psum
}

fn gfx_triangle_math() -> i64 {
    let mut cover = 0_i64;
    let mut tri = 0_i64;
    while tri < 200_000 {
        let x0 = tri % 97;
        let y0 = tri % 89;
        let x1 = x0 + 17;
        let y1 = y0 + 9;
        let x2 = x0 + 6;
        let y2 = y0 + 23;
        let sx = (tri * 13) % 31;
        let sy = (tri * 7) % 29;
        let e0 = (sx - x0) * (y1 - y0) - (sy - y0) * (x1 - x0);
        let e1 = (sx - x1) * (y2 - y1) - (sy - y1) * (x2 - x1);
        let e2 = (sx - x2) * (y0 - y2) - (sy - y2) * (x0 - x2);
        if e0 >= 0 && e1 >= 0 && e2 >= 0 {
            cover += 1;
        }
        tri += 1;
    }
    cover
}

fn crypto_modexp() -> i64 {
    let mut cm1 = 0_i64;
    let mut m = 1_i64;
    while m <= 200_000 {
        let base = (m * 17 + 3) % 65521;
        cm1 += modexp(base, 65537, 65521);
        m += 1;
    }
    cm1
}

fn crypto_feistel() -> i64 {
    const MOD: i64 = 104729;
    let mut cm2 = 0_i64;
    let mut b = 1_i64;
    while b <= 300_000 {
        let mut l = (b * 73 + 19) % MOD;
        let mut r = (b * 91 + 7) % MOD;
        let mut rd = 0_i64;
        while rd < 12 {
            let f = (r * r + (rd + 1) * 31 + r * 17) % MOD;
            let nl = r;
            let nr = (l + f) % MOD;
            l = nl;
            r = nr;
            rd += 1;
        }
        cm2 += l + r;
        b += 1;
    }
    cm2
}

fn crypto_lcg_stream() -> i64 {
    let mut state = 1_i64;
    let mut cm3 = 0_i64;
    let mut q = 0_i64;
    while q < 1_000_000 {
        state = (state * 48271) % 2147483647;
        let out = (state + q * 97) % 1000003;
        cm3 += out;
        q += 1;
    }
    cm3
}

fn main() {
    let t = Instant::now();
    let a = audio_fm_poly();
    println!("BENCH audio_fm_poly RESULT {:.15} TIME {:.6}", a, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let b = audio_iir_bank();
    println!("BENCH audio_iir_bank RESULT {:.15} TIME {:.6}", b, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let c = audio_delay_net();
    println!("BENCH audio_delay_net RESULT {:.15} TIME {:.6}", c, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let d = gfx_mandelbrot();
    println!("BENCH gfx_mandelbrot RESULT {} TIME {:.6}", d, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let e = gfx_particles();
    println!("BENCH gfx_particles RESULT {:.15} TIME {:.6}", e, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let f = gfx_triangle_math();
    println!("BENCH gfx_triangle_math RESULT {} TIME {:.6}", f, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let g = crypto_modexp();
    println!("BENCH crypto_modexp RESULT {} TIME {:.6}", g, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let h = crypto_feistel();
    println!("BENCH crypto_feistel RESULT {} TIME {:.6}", h, t.elapsed().as_secs_f64());

    let t = Instant::now();
    let i = crypto_lcg_stream();
    println!("BENCH crypto_lcg_stream RESULT {} TIME {:.6}", i, t.elapsed().as_secs_f64());
}
