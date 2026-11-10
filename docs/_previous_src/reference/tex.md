# Pixel Texture Builtins (`tex_*`)

`tex_*` functions write RGBA pixels **directly to the framebuffer** before the depth
queue is flushed. They are drawn underneath all `vtex_*` geometry.

> **Performance warning:** Each `tex_*` call iterates over every pixel in the
> destination rectangle. A full-screen 1920×1080 call costs ~2 million iterations.
> For smooth 60fps, keep tex_* calls to a small off-centre region or reduce
> window resolution.

All functions share the signature prefix:

```
tex_*(dst_x, dst_y, width, height,  ...params,  palette)
```

**Palette names:** `"rainbow"` · `"fire"` · `"ocean"` · `"psychedelic"` · `"neon"` · `"forest"`

---

## `tex_checkerboard` · `ลายตารางหมากรุก`

```
tex_checkerboard(x, y, w, h,  tiles,  r1,g1,b1,  r2,g2,b2)
```

Two-colour checker pattern. `tiles` = number of tiles across the width.

---

## `tex_gradient` · `ลายไล่สี`

```
tex_gradient(x, y, w, h,  angle_deg,  r1,g1,b1,  r2,g2,b2)
```

Linear gradient at `angle_deg` degrees.

---

## `tex_noise` · `ลายนอยส์`

```
tex_noise(x, y, w, h,  scale, octaves, seed, palette)
```

Fractal Brownian Motion noise.

| Param | Description |
|-------|-------------|
| `scale` | Noise frequency (higher = finer) |
| `octaves` | FBM octave count (1–8) |
| `seed` | Randomisation seed |

---

## `tex_mandelbrot` · `ลายแมนเดลบรอต`

```
tex_mandelbrot(x, y, w, h,  zoom, cx, cy, max_iter, palette)
```

| Param | Description |
|-------|-------------|
| `zoom` | Zoom level (1.0 = full set) |
| `cx, cy` | Centre of view in the complex plane |
| `max_iter` | Iteration cap (higher = more detail, slower) |

---

## `tex_julia` · `ลายจูเลีย`

```
tex_julia(x, y, w, h,  c_re, c_im, max_iter, palette)
```

Julia set with complex parameter `c = c_re + i·c_im`.

---

## `tex_voronoi` · `ลายโวโรนอย`

```
tex_voronoi(x, y, w, h,  cells, seed, palette)
```

---

## `tex_ripple` · `ลายระลอก`

```
tex_ripple(x, y, w, h,  freq, cx, cy, time, palette)
```

Concentric ripple emanating from `(cx, cy)` in normalised [0,1] coords.

---

## `tex_spiral` · `ลายเกลียวหมุน`

```
tex_spiral(x, y, w, h,  freq, n_bands, time, palette)
```

---

## `tex_halftone` · `ลายฮาล์ฟโทน`

```
tex_halftone(x, y, w, h,  dot_size, time, palette)
```

---

## `tex_freq_map` · `ลายความถี่`

```
tex_freq_map(x, y, w, h,  time, speed, palette)
```

Audio-reactive frequency bar display. Reads from the last `fft_bands()` call.
Call `fft_push(samples)` then `fft_bands(n)` each frame to keep it animated.

---

## Palette reference

| Name | Description |
|------|-------------|
| `"rainbow"` | Full spectrum cycle |
| `"fire"` | Red → orange → yellow |
| `"ocean"` | Blue → cyan → white |
| `"psychedelic"` | High-saturation cycling |
| `"neon"` | Bright neon green/cyan/magenta |
| `"forest"` | Earthy greens and browns |
