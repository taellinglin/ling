#!/usr/bin/env python3
import math
import time

PI = 3.141592653589793


def modexp(base, exp, m):
    b = base % m
    e = exp
    out = 1
    while e > 0:
        if e & 1:
            out = (out * b) % m
        b = (b * b) % m
        e >>= 1
    return out


def audio_fm_poly():
    N = 250_000
    sr = 48_000.0
    s = 0.0
    for j in range(N):
        t = j / sr
        v = 0.0
        for vi in range(1, 9):
            f = 110.0 * vi
            v += math.sin(2.0 * PI * f * t + 0.5 * math.sin(2.0 * PI * (f * 2.0) * t))
        s += v
    return s


def audio_iir_bank():
    y1 = y2 = y3 = y4 = 0.0
    acc = 0.0
    for n in range(300_000):
        x = math.sin(0.013 * n) + 0.5 * math.sin(0.017 * n)
        y1 = 0.995 * y1 + 0.005 * x
        y2 = 0.990 * y2 + 0.010 * y1
        y3 = 0.985 * y3 + 0.015 * y2
        y4 = 0.980 * y4 + 0.020 * y3
        acc += y4
    return acc


def audio_delay_net():
    s1 = s2 = s3 = s4 = s5 = s6 = s7 = s8 = 0.0
    acc = 0.0
    for i in range(300_000):
        x = math.sin(0.011 * i) + 0.25 * math.sin(0.029 * i)
        y = x + 0.7 * s8
        s8, s7, s6, s5, s4, s3, s2, s1 = s7, s6, s5, s4, s3, s2, s1, y
        acc += y
    return acc


def gfx_mandelbrot():
    W, H, maxiter = 240, 180, 120
    total = 0
    for py in range(H):
        for px in range(W):
            x0 = (px / W) * 3.5 - 2.5
            y0 = (py / H) * 2.0 - 1.0
            zx, zy = 0.0, 0.0
            it = 0
            while zx * zx + zy * zy <= 4.0 and it < maxiter:
                xt = zx * zx - zy * zy + x0
                zy = 2.0 * zx * zy + y0
                zx = xt
                it += 1
            total += it
    return total


def gfx_particles():
    psum = 0.0
    for p in range(20_000):
        x = (p % 257) * 0.01 - 1.28
        y = (p % 263) * 0.01 - 1.31
        vx = (p % 17) * 0.001
        vy = (p % 19) * 0.001
        for _ in range(120):
            ax = -0.0007 * x + 0.0003 * y
            ay = -0.0007 * y - 0.0003 * x
            vx = (vx + ax) * 0.999
            vy = (vy + ay) * 0.999
            x += vx
            y += vy
        psum += x + y
    return psum


def gfx_triangle_math():
    cover = 0
    for tri in range(200_000):
        x0 = tri % 97
        y0 = tri % 89
        x1 = x0 + 17
        y1 = y0 + 9
        x2 = x0 + 6
        y2 = y0 + 23
        sx = (tri * 13) % 31
        sy = (tri * 7) % 29
        e0 = (sx - x0) * (y1 - y0) - (sy - y0) * (x1 - x0)
        e1 = (sx - x1) * (y2 - y1) - (sy - y1) * (x2 - x1)
        e2 = (sx - x2) * (y0 - y2) - (sy - y2) * (x0 - x2)
        if e0 >= 0 and e1 >= 0 and e2 >= 0:
            cover += 1
    return cover


def crypto_modexp():
    cm1 = 0
    for m in range(1, 200_001):
        base = (m * 17 + 3) % 65521
        cm1 += modexp(base, 65537, 65521)
    return cm1


def crypto_feistel():
    cm2 = 0
    MOD = 104729
    for b in range(1, 300_001):
        l = (b * 73 + 19) % MOD
        r = (b * 91 + 7) % MOD
        for rd in range(12):
            f = (r * r + (rd + 1) * 31 + r * 17) % MOD
            l, r = r, (l + f) % MOD
        cm2 += l + r
    return cm2


def crypto_lcg_stream():
    state = 1
    cm3 = 0
    MOD = 2147483647
    for q in range(1_000_000):
        state = (state * 48271) % MOD
        out = (state + q * 97) % 1000003
        cm3 += out
    return cm3


def run(name, fn):
    t0 = time.perf_counter()
    r = fn()
    dt = time.perf_counter() - t0
    print(f"BENCH {name} RESULT {r} TIME {dt:.6f}")


if __name__ == "__main__":
    run("audio_fm_poly", audio_fm_poly)
    run("audio_iir_bank", audio_iir_bank)
    run("audio_delay_net", audio_delay_net)
    run("gfx_mandelbrot", gfx_mandelbrot)
    run("gfx_particles", gfx_particles)
    run("gfx_triangle_math", gfx_triangle_math)
    run("crypto_modexp", crypto_modexp)
    run("crypto_feistel", crypto_feistel)
    run("crypto_lcg_stream", crypto_lcg_stream)
