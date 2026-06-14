// C++ mirror of bench.ling. Emits: BENCH <name> RESULT <checksum> TIME <s>
// Build: g++ -O2 -ffp-contract=off bench.cpp -o bench_cpp
#include <chrono>
#include <cmath>
#include <cstdio>

static const double PI = 3.141592653589793;

static double now() {
    using namespace std::chrono;
    return duration<double>(steady_clock::now().time_since_epoch()).count();
}

static long long fib(int n) {
    if (n <= 1) return n;
    return fib(n - 1) + fib(n - 2);
}

static long long loop_sum() {
    long long s = 0;
    for (long long i = 0; i < 10000000LL; i++) s += i % 7;
    return s;
}

static double leibniz() {
    double acc = 0.0, sign = 1.0;
    for (long long k = 0; k < 5000000LL; k++) {
        acc += sign / (2.0 * (double)k + 1.0);
        sign = -sign;
    }
    return 4.0 * acc;
}

static long long primes() {
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

static long long mandelbrot() {
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

static double fm_synth() {
    const long long N = 1000000LL;
    const double sr = 44100.0;
    double s = 0.0;
    for (long long j = 0; j < N; j++) {
        double t = (double)j / sr;
        s += std::sin(2.0 * PI * 220.0 * t + std::sin(2.0 * PI * 440.0 * t));
    }
    return s;
}

int main() {
    double t0;
    t0 = now(); auto a = fib(30);      std::printf("BENCH fib RESULT %lld TIME %.6f\n", a, now() - t0);
    t0 = now(); auto b = loop_sum();   std::printf("BENCH loop_sum RESULT %lld TIME %.6f\n", b, now() - t0);
    t0 = now(); auto c = leibniz();    std::printf("BENCH leibniz RESULT %.15g TIME %.6f\n", c, now() - t0);
    t0 = now(); auto d = primes();     std::printf("BENCH primes RESULT %lld TIME %.6f\n", d, now() - t0);
    t0 = now(); auto e = mandelbrot(); std::printf("BENCH mandelbrot RESULT %lld TIME %.6f\n", e, now() - t0);
    t0 = now(); auto f = fm_synth();   std::printf("BENCH fm_synth RESULT %.15g TIME %.6f\n", f, now() - t0);
    return 0;
}
