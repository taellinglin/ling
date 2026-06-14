// Rust mirror of bench.ling. Emits: BENCH <name> RESULT <checksum> TIME <s>
// Build: rustc -O bench.rs -o bench_rs
use std::time::Instant;

const PI: f64 = 3.141592653589793;

fn fib(n: i64) -> i64 {
    if n <= 1 {
        n
    } else {
        fib(n - 1) + fib(n - 2)
    }
}

fn loop_sum() -> i64 {
    let mut s: i64 = 0;
    let mut i: i64 = 0;
    while i < 10_000_000 {
        s += i % 7;
        i += 1;
    }
    s
}

fn leibniz() -> f64 {
    let mut acc = 0.0_f64;
    let mut sign = 1.0_f64;
    let mut k: i64 = 0;
    while k < 5_000_000 {
        acc += sign / (2.0 * k as f64 + 1.0);
        sign = -sign;
        k += 1;
    }
    4.0 * acc
}

fn primes() -> i64 {
    let mut count: i64 = 0;
    let mut n: i64 = 2;
    while n < 50_000 {
        let mut d: i64 = 2;
        let mut is_p: i64 = 1;
        while d * d <= n {
            if n % d == 0 {
                is_p = 0;
                break;
            }
            d += 1;
        }
        count += is_p;
        n += 1;
    }
    count
}

fn mandelbrot() -> i64 {
    let (w, h, maxiter) = (200, 200, 100);
    let mut total: i64 = 0;
    for py in 0..h {
        for px in 0..w {
            let x0 = (px as f64 / w as f64) * 3.5 - 2.5;
            let y0 = (py as f64 / h as f64) * 2.0 - 1.0;
            let mut zx = 0.0_f64;
            let mut zy = 0.0_f64;
            let mut it = 0;
            while zx * zx + zy * zy <= 4.0 && it < maxiter {
                let xt = zx * zx - zy * zy + x0;
                zy = 2.0 * zx * zy + y0;
                zx = xt;
                it += 1;
            }
            total += it;
        }
    }
    total
}

fn fm_synth() -> f64 {
    let n: i64 = 1_000_000;
    let sr = 44100.0_f64;
    let mut s = 0.0_f64;
    let mut j: i64 = 0;
    while j < n {
        let t = j as f64 / sr;
        s += (2.0 * PI * 220.0 * t + (2.0 * PI * 440.0 * t).sin()).sin();
        j += 1;
    }
    s
}

fn main() {
    let t = Instant::now();
    let a = fib(30);
    println!("BENCH fib RESULT {} TIME {:.6}", a, t.elapsed().as_secs_f64());
    let t = Instant::now();
    let b = loop_sum();
    println!("BENCH loop_sum RESULT {} TIME {:.6}", b, t.elapsed().as_secs_f64());
    let t = Instant::now();
    let c = leibniz();
    println!("BENCH leibniz RESULT {:.15} TIME {:.6}", c, t.elapsed().as_secs_f64());
    let t = Instant::now();
    let d = primes();
    println!("BENCH primes RESULT {} TIME {:.6}", d, t.elapsed().as_secs_f64());
    let t = Instant::now();
    let e = mandelbrot();
    println!("BENCH mandelbrot RESULT {} TIME {:.6}", e, t.elapsed().as_secs_f64());
    let t = Instant::now();
    let f = fm_synth();
    println!("BENCH fm_synth RESULT {:.15} TIME {:.6}", f, t.elapsed().as_secs_f64());
}
