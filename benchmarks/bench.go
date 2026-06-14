// Go mirror of bench.ling. Emits: BENCH <name> RESULT <checksum> TIME <s>
// Build: go build -o bench_go bench.go
package main

import (
	"fmt"
	"math"
	"time"
)

const PI = 3.141592653589793

func fib(n int) int64 {
	if n <= 1 {
		return int64(n)
	}
	return fib(n-1) + fib(n-2)
}

func loopSum() int64 {
	var s int64 = 0
	for i := int64(0); i < 10000000; i++ {
		s += i % 7
	}
	return s
}

func leibniz() float64 {
	acc := 0.0
	sign := 1.0
	for k := int64(0); k < 5000000; k++ {
		acc += sign / (2.0*float64(k) + 1.0)
		sign = -sign
	}
	return 4.0 * acc
}

func primes() int64 {
	var count int64 = 0
	for n := int64(2); n < 50000; n++ {
		isP := int64(1)
		for d := int64(2); d*d <= n; d++ {
			if n%d == 0 {
				isP = 0
				break
			}
		}
		count += isP
	}
	return count
}

func mandelbrot() int64 {
	const W, H, maxiter = 200, 200, 100
	var total int64 = 0
	for py := 0; py < H; py++ {
		for px := 0; px < W; px++ {
			x0 := (float64(px)/float64(W))*3.5 - 2.5
			y0 := (float64(py)/float64(H))*2.0 - 1.0
			zx, zy := 0.0, 0.0
			it := 0
			for zx*zx+zy*zy <= 4.0 && it < maxiter {
				xt := zx*zx - zy*zy + x0
				zy = 2.0*zx*zy + y0
				zx = xt
				it++
			}
			total += int64(it)
		}
	}
	return total
}

func fmSynth() float64 {
	const N = 1000000
	const sr = 44100.0
	s := 0.0
	for j := 0; j < N; j++ {
		t := float64(j) / sr
		s += math.Sin(2.0*PI*220.0*t + math.Sin(2.0*PI*440.0*t))
	}
	return s
}

func main() {
	t0 := time.Now()
	a := fib(30)
	fmt.Printf("BENCH fib RESULT %d TIME %.6f\n", a, time.Since(t0).Seconds())
	t0 = time.Now()
	b := loopSum()
	fmt.Printf("BENCH loop_sum RESULT %d TIME %.6f\n", b, time.Since(t0).Seconds())
	t0 = time.Now()
	c := leibniz()
	fmt.Printf("BENCH leibniz RESULT %.15g TIME %.6f\n", c, time.Since(t0).Seconds())
	t0 = time.Now()
	d := primes()
	fmt.Printf("BENCH primes RESULT %d TIME %.6f\n", d, time.Since(t0).Seconds())
	t0 = time.Now()
	e := mandelbrot()
	fmt.Printf("BENCH mandelbrot RESULT %d TIME %.6f\n", e, time.Since(t0).Seconds())
	t0 = time.Now()
	f := fmSynth()
	fmt.Printf("BENCH fm_synth RESULT %.15g TIME %.6f\n", f, time.Since(t0).Seconds())
}
