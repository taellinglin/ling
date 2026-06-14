/* C mirror of bench.ling. Emits: BENCH <name> RESULT <checksum> TIME <s>
 * Build: gcc -O2 -ffp-contract=off bench.c -o bench_c -lm
 * (-ffp-contract=off keeps float results identical to the other languages.) */
#include <math.h>
#include <stdio.h>

static const double PI = 3.141592653589793;

/* High-resolution monotonic clock (portable, no extra link libs). */
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

static long long fib(int n) {
    if (n <= 1) return n;
    return fib(n - 1) + fib(n - 2);
}

static long long loop_sum(void) {
    long long s = 0;
    for (long long i = 0; i < 10000000LL; i++) s += i % 7;
    return s;
}

static double leibniz(void) {
    double acc = 0.0, sign = 1.0;
    for (long long k = 0; k < 5000000LL; k++) {
        acc += sign / (2.0 * (double)k + 1.0);
        sign = -sign;
    }
    return 4.0 * acc;
}

static long long primes(void) {
    long long count = 0;
    for (long long n = 2; n < 50000LL; n++) {
        int is_p = 1;
        for (long long d = 2; d * d <= n; d++) {
            if (n % d == 0) { is_p = 0; break; }
        }
        count += is_p;
    }
    return count;
}

static long long mandelbrot(void) {
    const int W = 200, H = 200, maxiter = 100;
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

static double fm_synth(void) {
    const long long N = 1000000LL;
    const double sr = 44100.0;
    double s = 0.0;
    for (long long j = 0; j < N; j++) {
        double t = (double)j / sr;
        s += sin(2.0 * PI * 220.0 * t + sin(2.0 * PI * 440.0 * t));
    }
    return s;
}

int main(void) {
    double t0;
    t0 = now(); long long a = fib(30);        printf("BENCH fib RESULT %lld TIME %.6f\n", a, now() - t0);
    t0 = now(); long long b = loop_sum();      printf("BENCH loop_sum RESULT %lld TIME %.6f\n", b, now() - t0);
    t0 = now(); double c = leibniz();          printf("BENCH leibniz RESULT %.15g TIME %.6f\n", c, now() - t0);
    t0 = now(); long long d = primes();        printf("BENCH primes RESULT %lld TIME %.6f\n", d, now() - t0);
    t0 = now(); long long e = mandelbrot();    printf("BENCH mandelbrot RESULT %lld TIME %.6f\n", e, now() - t0);
    t0 = now(); double f = fm_synth();         printf("BENCH fm_synth RESULT %.15g TIME %.6f\n", f, now() - t0);
    return 0;
}
