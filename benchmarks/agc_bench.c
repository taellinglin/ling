/* C AGC benchmark suite */
#include <math.h>
#include <stdio.h>

#if defined(_WIN32)
#include <windows.h>
static double now(void) {
    LARGE_INTEGER f, c;
    QueryPerformanceFrequency(&f);
    QueryPerformanceCounter(&c);
    return (double)c.QuadPart / (double)f.QuadPart;
}
#else
#include <time.h>
static double now(void) {
    struct timespec ts;
    clock_gettime(CLOCK_MONOTONIC, &ts);
    return (double)ts.tv_sec + (double)ts.tv_nsec * 1e-9;
}
#endif

static const double PI = 3.141592653589793;

static long long modexp(long long base, long long exp, long long m) {
    long long b = base % m;
    long long e = exp;
    long long out = 1;
    while (e > 0) {
        if (e & 1LL) out = (out * b) % m;
        b = (b * b) % m;
        e >>= 1;
    }
    return out;
}

static double audio_fm_poly(void) {
    const long long N = 250000;
    const double sr = 48000.0;
    double s = 0.0;
    for (long long j = 0; j < N; j++) {
        double t = (double)j / sr;
        double v = 0.0;
        for (int vi = 1; vi <= 8; vi++) {
            double f = 110.0 * (double)vi;
            v += sin(2.0 * PI * f * t + 0.5 * sin(2.0 * PI * (f * 2.0) * t));
        }
        s += v;
    }
    return s;
}

static double audio_iir_bank(void) {
    double y1 = 0.0, y2 = 0.0, y3 = 0.0, y4 = 0.0;
    double acc = 0.0;
    for (long long n = 0; n < 300000; n++) {
        double x = sin(0.013 * (double)n) + 0.5 * sin(0.017 * (double)n);
        y1 = 0.995 * y1 + 0.005 * x;
        y2 = 0.990 * y2 + 0.010 * y1;
        y3 = 0.985 * y3 + 0.015 * y2;
        y4 = 0.980 * y4 + 0.020 * y3;
        acc += y4;
    }
    return acc;
}

static double audio_delay_net(void) {
    double s1 = 0, s2 = 0, s3 = 0, s4 = 0, s5 = 0, s6 = 0, s7 = 0, s8 = 0;
    double acc = 0.0;
    for (long long i = 0; i < 300000; i++) {
        double x = sin(0.011 * (double)i) + 0.25 * sin(0.029 * (double)i);
        double y = x + 0.7 * s8;
        s8 = s7; s7 = s6; s6 = s5; s5 = s4; s4 = s3; s3 = s2; s2 = s1; s1 = y;
        acc += y;
    }
    return acc;
}

static long long gfx_mandelbrot(void) {
    const int W = 240, H = 180, maxiter = 120;
    long long total = 0;
    for (int py = 0; py < H; py++) {
        for (int px = 0; px < W; px++) {
            double x0 = ((double)px / (double)W) * 3.5 - 2.5;
            double y0 = ((double)py / (double)H) * 2.0 - 1.0;
            double zx = 0.0, zy = 0.0;
            int it = 0;
            while (zx * zx + zy * zy <= 4.0 && it < maxiter) {
                double xt = zx * zx - zy * zy + x0;
                zy = 2.0 * zx * zy + y0;
                zx = xt;
                it++;
            }
            total += it;
        }
    }
    return total;
}

static double gfx_particles(void) {
    double psum = 0.0;
    for (long long p = 0; p < 20000; p++) {
        double x = (double)(p % 257) * 0.01 - 1.28;
        double y = (double)(p % 263) * 0.01 - 1.31;
        double vx = (double)(p % 17) * 0.001;
        double vy = (double)(p % 19) * 0.001;
        for (int s = 0; s < 120; s++) {
            double ax = -0.0007 * x + 0.0003 * y;
            double ay = -0.0007 * y - 0.0003 * x;
            vx = (vx + ax) * 0.999;
            vy = (vy + ay) * 0.999;
            x += vx;
            y += vy;
        }
        psum += x + y;
    }
    return psum;
}

static long long gfx_triangle_math(void) {
    long long cover = 0;
    for (long long tri = 0; tri < 200000; tri++) {
        long long x0 = tri % 97, y0 = tri % 89;
        long long x1 = x0 + 17, y1 = y0 + 9;
        long long x2 = x0 + 6, y2 = y0 + 23;
        long long sx = (tri * 13) % 31;
        long long sy = (tri * 7) % 29;
        long long e0 = (sx - x0) * (y1 - y0) - (sy - y0) * (x1 - x0);
        long long e1 = (sx - x1) * (y2 - y1) - (sy - y1) * (x2 - x1);
        long long e2 = (sx - x2) * (y0 - y2) - (sy - y2) * (x0 - x2);
        if (e0 >= 0 && e1 >= 0 && e2 >= 0) cover++;
    }
    return cover;
}

static long long crypto_modexp(void) {
    long long cm1 = 0;
    for (long long m = 1; m <= 200000; m++) {
        long long base = (m * 17 + 3) % 65521;
        cm1 += modexp(base, 65537, 65521);
    }
    return cm1;
}

static long long crypto_feistel(void) {
    const long long MOD = 104729;
    long long cm2 = 0;
    for (long long b = 1; b <= 300000; b++) {
        long long l = (b * 73 + 19) % MOD;
        long long r = (b * 91 + 7) % MOD;
        for (long long rd = 0; rd < 12; rd++) {
            long long f = (r * r + (rd + 1) * 31 + r * 17) % MOD;
            long long nl = r;
            long long nr = (l + f) % MOD;
            l = nl;
            r = nr;
        }
        cm2 += l + r;
    }
    return cm2;
}

static long long crypto_lcg_stream(void) {
    long long state = 1;
    long long cm3 = 0;
    for (long long q = 0; q < 1000000; q++) {
        state = (state * 48271) % 2147483647;
        long long out = (state + q * 97) % 1000003;
        cm3 += out;
    }
    return cm3;
}

int main(void) {
    double t0;
    t0 = now(); double a = audio_fm_poly();      printf("BENCH audio_fm_poly RESULT %.15g TIME %.6f\n", a, now() - t0);
    t0 = now(); double b = audio_iir_bank();     printf("BENCH audio_iir_bank RESULT %.15g TIME %.6f\n", b, now() - t0);
    t0 = now(); double c = audio_delay_net();    printf("BENCH audio_delay_net RESULT %.15g TIME %.6f\n", c, now() - t0);
    t0 = now(); long long d = gfx_mandelbrot();  printf("BENCH gfx_mandelbrot RESULT %lld TIME %.6f\n", d, now() - t0);
    t0 = now(); double e = gfx_particles();      printf("BENCH gfx_particles RESULT %.15g TIME %.6f\n", e, now() - t0);
    t0 = now(); long long f = gfx_triangle_math(); printf("BENCH gfx_triangle_math RESULT %lld TIME %.6f\n", f, now() - t0);
    t0 = now(); long long g = crypto_modexp();   printf("BENCH crypto_modexp RESULT %lld TIME %.6f\n", g, now() - t0);
    t0 = now(); long long h = crypto_feistel();  printf("BENCH crypto_feistel RESULT %lld TIME %.6f\n", h, now() - t0);
    t0 = now(); long long i = crypto_lcg_stream(); printf("BENCH crypto_lcg_stream RESULT %lld TIME %.6f\n", i, now() - t0);
    return 0;
}
