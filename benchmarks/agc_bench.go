package main

import (
	"fmt"
	"math"
	"time"
)

const PI = 3.141592653589793

func modexp(base, exp, m int64) int64 {
	b := base % m
	e := exp
	out := int64(1)
	for e > 0 {
		if e&1 == 1 {
			out = (out * b) % m
		}
		b = (b * b) % m
		e >>= 1
	}
	return out
}

func audioFmPoly() float64 {
	const N int64 = 250000
	const sr = 48000.0
	s := 0.0
	for j := int64(0); j < N; j++ {
		t := float64(j) / sr
		v := 0.0
		for vi := 1; vi <= 8; vi++ {
			f := 110.0 * float64(vi)
			v += math.Sin(2.0*PI*f*t + 0.5*math.Sin(2.0*PI*(f*2.0)*t))
		}
		s += v
	}
	return s
}

func audioIirBank() float64 {
	y1, y2, y3, y4 := 0.0, 0.0, 0.0, 0.0
	acc := 0.0
	for n := int64(0); n < 300000; n++ {
		x := math.Sin(0.013*float64(n)) + 0.5*math.Sin(0.017*float64(n))
		y1 = 0.995*y1 + 0.005*x
		y2 = 0.990*y2 + 0.010*y1
		y3 = 0.985*y3 + 0.015*y2
		y4 = 0.980*y4 + 0.020*y3
		acc += y4
	}
	return acc
}

func audioDelayNet() float64 {
	s1, s2, s3, s4, s5, s6, s7, s8 := 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0
	acc := 0.0
	for i := int64(0); i < 300000; i++ {
		x := math.Sin(0.011*float64(i)) + 0.25*math.Sin(0.029*float64(i))
		y := x + 0.7*s8
		s8, s7, s6, s5, s4, s3, s2, s1 = s7, s6, s5, s4, s3, s2, s1, y
		acc += y
	}
	return acc
}

func gfxMandelbrot() int64 {
	const W, H, maxiter = 240, 180, 120
	total := int64(0)
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

func gfxParticles() float64 {
	psum := 0.0
	for p := int64(0); p < 20000; p++ {
		x := float64(p%257)*0.01 - 1.28
		y := float64(p%263)*0.01 - 1.31
		vx := float64(p%17) * 0.001
		vy := float64(p%19) * 0.001
		for s := 0; s < 120; s++ {
			ax := -0.0007*x + 0.0003*y
			ay := -0.0007*y - 0.0003*x
			vx = (vx + ax) * 0.999
			vy = (vy + ay) * 0.999
			x += vx
			y += vy
		}
		psum += x + y
	}
	return psum
}

func gfxTriangleMath() int64 {
	cover := int64(0)
	for tri := int64(0); tri < 200000; tri++ {
		x0, y0 := tri%97, tri%89
		x1, y1 := x0+17, y0+9
		x2, y2 := x0+6, y0+23
		sx, sy := (tri*13)%31, (tri*7)%29
		e0 := (sx-x0)*(y1-y0) - (sy-y0)*(x1-x0)
		e1 := (sx-x1)*(y2-y1) - (sy-y1)*(x2-x1)
		e2 := (sx-x2)*(y0-y2) - (sy-y2)*(x0-x2)
		if e0 >= 0 && e1 >= 0 && e2 >= 0 {
			cover++
		}
	}
	return cover
}

func cryptoModexp() int64 {
	cm1 := int64(0)
	for m := int64(1); m <= 200000; m++ {
		base := (m*17 + 3) % 65521
		cm1 += modexp(base, 65537, 65521)
	}
	return cm1
}

func cryptoFeistel() int64 {
	const MOD int64 = 104729
	cm2 := int64(0)
	for b := int64(1); b <= 300000; b++ {
		l := (b*73 + 19) % MOD
		r := (b*91 + 7) % MOD
		for rd := int64(0); rd < 12; rd++ {
			f := (r*r + (rd+1)*31 + r*17) % MOD
			l, r = r, (l+f)%MOD
		}
		cm2 += l + r
	}
	return cm2
}

func cryptoLcgStream() int64 {
	state := int64(1)
	cm3 := int64(0)
	for q := int64(0); q < 1000000; q++ {
		state = (state * 48271) % 2147483647
		out := (state + q*97) % 1000003
		cm3 += out
	}
	return cm3
}

func main() {
	t0 := time.Now()
	a := audioFmPoly()
	fmt.Printf("BENCH audio_fm_poly RESULT %.15g TIME %.6f\n", a, time.Since(t0).Seconds())
	t0 = time.Now()
	b := audioIirBank()
	fmt.Printf("BENCH audio_iir_bank RESULT %.15g TIME %.6f\n", b, time.Since(t0).Seconds())
	t0 = time.Now()
	c := audioDelayNet()
	fmt.Printf("BENCH audio_delay_net RESULT %.15g TIME %.6f\n", c, time.Since(t0).Seconds())
	t0 = time.Now()
	d := gfxMandelbrot()
	fmt.Printf("BENCH gfx_mandelbrot RESULT %d TIME %.6f\n", d, time.Since(t0).Seconds())
	t0 = time.Now()
	e := gfxParticles()
	fmt.Printf("BENCH gfx_particles RESULT %.15g TIME %.6f\n", e, time.Since(t0).Seconds())
	t0 = time.Now()
	f := gfxTriangleMath()
	fmt.Printf("BENCH gfx_triangle_math RESULT %d TIME %.6f\n", f, time.Since(t0).Seconds())
	t0 = time.Now()
	g := cryptoModexp()
	fmt.Printf("BENCH crypto_modexp RESULT %d TIME %.6f\n", g, time.Since(t0).Seconds())
	t0 = time.Now()
	h := cryptoFeistel()
	fmt.Printf("BENCH crypto_feistel RESULT %d TIME %.6f\n", h, time.Since(t0).Seconds())
	t0 = time.Now()
	i := cryptoLcgStream()
	fmt.Printf("BENCH crypto_lcg_stream RESULT %d TIME %.6f\n", i, time.Since(t0).Seconds())
}
