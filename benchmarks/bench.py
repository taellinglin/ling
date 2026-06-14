#!/usr/bin/env python3
# Python mirror of bench.ling. Emits: BENCH <name> RESULT <checksum> TIME <s>
import math
import sys
import time

PI = 3.141592653589793
sys.setrecursionlimit(10000)


def fib(n):
    if n <= 1:
        return n
    return fib(n - 1) + fib(n - 2)


def loop_sum():
    s = 0
    i = 0
    while i < 10_000_000:
        s += i % 7
        i += 1
    return s


def leibniz():
    acc = 0.0
    sign = 1.0
    k = 0
    while k < 5_000_000:
        acc += sign / (2.0 * k + 1.0)
        sign = -sign
        k += 1
    return 4.0 * acc


def primes():
    count = 0
    n = 2
    while n < 50_000:
        d = 2
        is_p = 1
        while d * d <= n:
            if n % d == 0:
                is_p = 0
                break
            d += 1
        count += is_p
        n += 1
    return count


def mandelbrot():
    W, H, maxiter = 200, 200, 100
    total = 0
    py = 0
    while py < H:
        px = 0
        while px < W:
            x0 = (px / W) * 3.5 - 2.5
            y0 = (py / H) * 2.0 - 1.0
            zx = 0.0
            zy = 0.0
            it = 0
            while zx * zx + zy * zy <= 4.0 and it < maxiter:
                xt = zx * zx - zy * zy + x0
                zy = 2.0 * zx * zy + y0
                zx = xt
                it += 1
            total += it
            px += 1
        py += 1
    return total


def fm_synth():
    N = 1_000_000
    sr = 44100.0
    s = 0.0
    j = 0
    while j < N:
        t = j / sr
        s += math.sin(2.0 * PI * 220.0 * t + math.sin(2.0 * PI * 440.0 * t))
        j += 1
    return s


def run(name, fn):
    t0 = time.perf_counter()
    r = fn()
    dt = time.perf_counter() - t0
    print(f"BENCH {name} RESULT {r} TIME {dt:.6f}")


if __name__ == "__main__":
    run("fib", lambda: fib(30))
    run("loop_sum", loop_sum)
    run("leibniz", leibniz)
    run("primes", primes)
    run("mandelbrot", mandelbrot)
    run("fm_synth", fm_synth)
