# Vector Geometry Builtins (`vtex_*`)

All `vtex_*` functions draw **line-based 3D geometry** onto a plane defined by:

- **Centre** `(cx, cy, cz)` — world-space anchor point
- **U tangent** `(ux, uy, uz)` — "right" direction in the plane
- **V tangent** `(vx, vy, vz)` — "up" direction in the plane

Lines are depth-sorted and drawn on top of the pixel buffer when `present()` / `แสดงผล()` / `显()` / `表示()` / `표시()` is called.

> **Performance tip:** vtex calls are fast (pure line rasterisation). Prefer them
> over `tex_*` for real-time use — a `tex_mandelbrot` on a 1920×1080 window is
> ~2 million pixel ops per call; a `vtex_rings` with 32 segments is ~32 line ops.

---

## Common plane orientations

```ling
# Horizontal plane (floor / ceiling): U=right, V=forward
1,0,0,  0,0,1

# Vertical front-facing plane (wall / mandala disc): U=right, V=up
1,0,0,  0,1,0

# Vertical side-facing plane (left/right wall): U=forward, V=up
0,0,1,  0,1,0
```

---

## Primitives

### `vtex_grid` · `ลายตาราง` · `纹格` · `格子模様` · `격자무늬`

Rectilinear grid.

```
vtex_grid(cx,cy,cz, ux,uy,uz, vx,vy,vz,  cols, rows, cell_w, cell_h,  fr, hue)
```

| Param | Default | Description |
|-------|---------|-------------|
| `cols` | — | number of columns |
| `rows` | — | number of rows |
| `cell_w` | — | cell width in world units |
| `cell_h` | — | cell height in world units |
| `fr` | — | frame counter (drives colour animation) |
| `hue` | — | colour phase offset |

---

### `vtex_rings` · `ลายวงซ้อน` · `纹环` · `同心円` · `동심원`

Concentric N-sided polygon rings with optional twist.

```
vtex_rings(cx,cy,cz, ux,uy,uz, vx,vy,vz,  n_rings, n_sides, max_r, twist,  fr, hue)
```

| Param | Description |
|-------|-------------|
| `n_rings` | number of concentric rings |
| `n_sides` | polygon sides per ring (32 = smooth circle) |
| `max_r` | outer ring radius |
| `twist` | extra rotation per ring in radians (creates vortex) |

---

### `vtex_star` · `ลายดาว` · `纹星` · `星模様` · `별무늬`

Alternating outer/inner star polygon.

```
vtex_star(cx,cy,cz, ux,uy,uz, vx,vy,vz,  n_points, r_outer, r_inner, rot_speed,  fr, hue)
```

---

### `vtex_spiral` · `ลายเกลียว` · `纹螺` · `螺旋` · `나선`

Archimedean spiral.

```
vtex_spiral(cx,cy,cz, ux,uy,uz, vx,vy,vz,  n_turns, max_r, n_steps,  fr, hue)
```

---

### `vtex_flower` · `ลายดอก` · `纹花` · `花模様` · `꽃무늬`

Flower of Life: centre circle plus 6 surrounding circles.

```
vtex_flower(cx,cy,cz, ux,uy,uz, vx,vy,vz,  radius, n_sides,  fr, hue)
```

---

### `vtex_lotus` · `ลายดอกบัว` · `纹莲` · `蓮模様` · `연꽃무늬`

Radial kite-petal lotus.

```
vtex_lotus(cx,cy,cz, ux,uy,uz, vx,vy,vz,  r_inner, r_outer, n_petals,  fr, hue)
```

---

### `vtex_chakra` · `ลายจักร` · `纹轮` · `輪模様` · `바퀴무늬`

Dhamma wheel: outer rim + inner hub + N spokes + tick marks.

```
vtex_chakra(cx,cy,cz, ux,uy,uz, vx,vy,vz,  r, n_spokes,  fr, hue)
```

---

### `vtex_yantra` · `ลายยันต์` · `纹咒` · `護符模様` · `부적무늬`

Nested interlocked triangle pairs (Sri Yantra style) with bhupura gate square.

```
vtex_yantra(cx,cy,cz, ux,uy,uz, vx,vy,vz,  n_layers, max_r,  fr, hue)
```

---

### `vtex_spiked_cog` · `ฟันเฟืองหนาม` · `纹棘轮` · `歯車模様` · `톱니바퀴`

Mechanical gear with triangular spike teeth, inner body ring, hub, and spokes.

```
vtex_spiked_cog(cx,cy,cz, ux,uy,uz, vx,vy,vz,
                n_teeth, r_body, r_spike, r_hub, n_spokes,
                fr, hue)
```

| Param | Default | Description |
|-------|---------|-------------|
| `n_teeth` | 12 | number of spike teeth |
| `r_body` | 1.0 | radius of the flat gear body |
| `r_spike` | 1.3 | radius of spike tips |
| `r_hub` | 0.2 | inner hub radius |
| `n_spokes` | 6 | spokes from hub to body |

```ling
# Example: ornate gold dharma gear
vtex_spiked_cog(0, 0, 8,  1,0,0,  0,1,0,  18, 1.6, 2.0, 0.28, 9,  FR * 0.0004, 1.57)
```

---

### `vtex_torii` · `ประตูโทริอิ` · `纹鸟居` · `鳥居` · `도리이`

Japanese torii gate silhouette: two pillars, straight nuki crossbar, curved kasagi top beam.

```
vtex_torii(cx,cy,cz, ux,uy,uz, vx,vy,vz,  width, height,  fr, hue)
```

| Param | Default | Description |
|-------|---------|-------------|
| `width` | 4.0 | total gate width |
| `height` | 5.0 | pillar height |

```ling
# Gate on the far wall, crimson
vtex_torii(0, 0, 16,  1,0,0,  0,1,0,  6.0, 5.0,  FR * 0.0001, 2.1)
```

---

### `vtex_pagoda` · `เจดีย์` · `纹塔` · `塔` · `탑`

Multi-tiered pagoda silhouette with upturned eaves and spire finial.

```
vtex_pagoda(cx,cy,cz, ux,uy,uz, vx,vy,vz,
            n_tiers, base_w, tier_h, taper, eave_out,
            fr, hue)
```

| Param | Default | Description |
|-------|---------|-------------|
| `n_tiers` | 5 | number of storeys |
| `base_w` | 2.0 | half-width of base tier |
| `tier_h` | 1.0 | height of each tier |
| `taper` | 0.72 | width reduction per tier |
| `eave_out` | 0.28 | eave overhang fraction |

```ling
# 5-tier gold pagoda on back wall
vtex_pagoda(0, 0, 16,  1,0,0,  0,1,0,  5, 2.2, 1.0, 0.72, 0.28,  FR * 0.0001, 1.57)
```

---

### `vtex_halftone` · `ลายจุด` · `纹半调` · `網点模様` · `망점`

Cross-hatch dithered fill.

```
vtex_halftone(cx,cy,cz, ux,uy,uz, vx,vy,vz,  cols, rows, cell_w, cell_h, density,  fr, hue)
```

---

### `vtex_tessellated` · `ลายตาข่าย` · `纹镶嵌` · `網目模様` · `격자망`

Animated triangle mesh fill (sine-displaced vertices).

```
vtex_tessellated(cx,cy,cz, ux,uy,uz, vx,vy,vz,  cols, rows, cell, amplitude, freq,  fr, hue)
```

---

### `vtex_hyperbolic_uv` · `ลายไฮเพอร์โบลิก` · `纹曲面` · `双曲線` · `쌍곡선`

Poincaré-disc hyperbolic tiling (concentric circles + geodesic rays).

```
vtex_hyperbolic_uv(cx,cy,cz, ux,uy,uz, vx,vy,vz,  max_r, n_circles, n_rays,  fr, hue)
```

---

### `vtex_letter_rain` · `ลายอักษรไหล` · `纹字雨` · `文字雨` · `글자비`

Falling-glyph rain (Matrix-style, using simplified stroke glyphs).

```
vtex_letter_rain(cx,cy,cz, ux,uy,uz, vx,vy,vz,  n_cols, n_visible, col_w, row_h, speed,  fr, hue)
```

---

## Colour system

All vtex functions use the `cycle(phase)` colour formula:

```
R = sin(phase)         * 127 + 128
G = sin(phase + 2.094) * 127 + 128   # 120° offset
B = sin(phase + 4.189) * 127 + 128   # 240° offset
```

Useful `hue` anchor values for cultural palettes:

| Colour | `hue` value |
|--------|------------|
| Gold / amber | `1.57` (π/2) |
| Jade green | `3.50` |
| Crimson red | `2.10` |
| Ivory white | `0.30` |
| Deep blue | `4.70` |
| Purple | `5.80` |
