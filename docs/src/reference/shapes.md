# 3-D Primitive Shapes

The shape builtins are Ling's **"Inkscape for 3-D"** — a library of parametric
solids (boxes, spheres, dice, cogs, gyros…) that render as **lit filled
triangles**, **wireframe**, or **both**. Every shape shares one uniform call
signature, so once you know one you know them all.

```
name(cx,cy,cz,  sx,sy,sz,  rx,ry,rz,  mode,  e0, e1, e2)
```

| Group | Params | Meaning |
|-------|--------|---------|
| Centre | `cx, cy, cz` | world-space position of the shape's centre |
| Scale  | `sx, sy, sz` | per-axis size (half-extent). Non-uniform values give stretched cubes, ellipsoids, flat discs, … |
| Rotation | `rx, ry, rz` | Euler rotation in **radians**, applied X → Y → Z |
| Mode | `mode` | `0` = filled · `1` = wireframe · `2` = both (wire drawn on top) |
| Extras | `e0, e1, e2` | shape-specific (segment counts, sides, ratios). Omit or pass `0` for sensible defaults |

The current **pen colour** (`set_color` / `สีดินสอ` / `设色` …) drives both the
fill lighting (cel-shaded against the active lights) and the wireframe colour.
Shapes are depth-sorted with the rest of the scene and flushed on `present()`.

> **Camera note:** like all 3-D draws, shapes are projected through the camera
> set with `set_camera(...)`. Keep the camera at the origin and place shapes in
> front at positive Z (the same convention the room demos use).

---

## Quick start

```ling
bind start = do {
    open_fullscreen("shapes")
    set_ambient(0.25)
    add_light(4, 4, 8, 4.0, 255, 240, 220)

    bind FR = 0
    while window_is_open() {
        fill(4, 5, 12)
        set_camera(1, 0, 1, 0)
        bind a = FR * 0.02

        set_color(230, 120, 90)
        cube(-2, 0, 9,  0.8,0.8,0.8,  a, a*0.7, 0,  0)        # filled cube

        set_color(120, 200, 255)
        icosahedron(0, 0, 9,  0.9,0.9,0.9,  a, a, 0,  2)      # d20, fill+wire

        set_color(190, 190, 200)
        gear(2, 0, 9,  0.8,0.3,0.8,  1.57, a*2, 0,  0,  14, 0.3)  # spinning cog

        present()
        bind FR = FR + 1
    }
}
```

---

## Draw modes

| `mode` | Result |
|--------|--------|
| `0` | **Filled** — cel-lit triangles |
| `1` | **Wireframe** — edges only, in the pen colour |
| `2` | **Both** — filled, with the wireframe biased slightly nearer so it reads on top |

---

## Rendering & lighting modes (holographic cel)

Shape fills use an **anime / holographic cel** model by default: surfaces are
lit with **smooth (averaged) vertex normals**, the lit colour is interpolated
across each triangle, then **posterised per pixel** into crisp luminance bands.
The result is smooth shading with no faceted triangle edges, but clean toon
bands that curve over the surface — plus coloured lights, coloured shadows, and
a Fresnel rim glow.

| Builtin | Args | Effect |
|---------|------|--------|
| `set_shade_mode(m)` | `0` flat · `1` cel · `2` holo (default) | overall style |
| `set_cel_bands(n)` | `n ≥ 2` (default 4) | number of posterisation bands — lower = chunkier |
| `set_shadow_color(r,g,b)` | 0–255 | tint of unlit regions (coloured shadows) |
| `set_rim(strength, r,g,b)` | strength + 0–255 | Fresnel edge-glow colour/intensity |

```ling
set_shade_mode(2)            # holographic cel (default)
set_cel_bands(4)             # 4 bands
set_shadow_color(28,36,90)   # cool indigo shadows
set_rim(0.7, 120,220,255)    # cyan holographic edge
```

- **Mode 0 (flat):** legacy per-face cel — every triangle one flat band (faceted).
- **Mode 1 (cel):** smooth bands, no rim/sheen — clean toon.
- **Mode 2 (holo):** smooth bands + Fresnel rim + a subtle normal-gradient sheen.

Lighting comes from `add_light(x,y,z, intensity, r,g,b)` (each light contributes
its own colour) and `set_ambient(level)`. The lighting math lives in the
`ling-graphics` crate (`ling_graphics::shading`).

---

## Round & swept solids

### `cube` · `box` — `立方体` · `方块` · `정육면체` · `ลูกบาศก์`  (box: `方块`/`箱`/`상자`/`กล่อง`)
Axis-aligned box. Non-uniform `sx,sy,sz` make any rectangular block.
```
cube(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode)
```

### `sphere` — `球体` · `球` · `구` · `ทรงกลม`
UV sphere. Scale axes independently for an ellipsoid.
```
sphere(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  segments=16, rings=12)
```

### `icosphere` — `二十面球` · `アイコ球` · `아이코구체` · `ทรงกลมเหลี่ยม`
Geodesic sphere from a subdivided icosahedron (more even than a UV sphere).
```
icosphere(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  subdivisions=1)   # 0..4
```

### `dome` — `穹顶` · `ドーム` · `돔` · `โดม`
Hemisphere with a closing base — useful for cupolas and skies.
```
dome(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  segments=24, rings=8)
```

### `cylinder` — `圆柱` · `円柱` · `원기둥` · `ทรงกระบอก`
Capped tube (height runs along local Y, scaled by `sy`).
```
cylinder(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  segments=24)
```

### `cone` — `圆锥` · `円錐` · `원뿔` · `กรวย`
Capped cone, apex at +Y.
```
cone(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  segments=24)
```

### `capsule` — `胶囊` · `カプセル` · `캡슐` · `แคปซูล`
Cylinder with hemispherical caps (a "pill").
```
capsule(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  segments=16, rings=6)
```

### `torus` · `ring` — `圆环` · `トーラス` · `토러스` · `ทอรัส`
Donut. `tube` is the tube radius as a fraction of the outer radius.
```
torus(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  segments=32, sides=12, tube=0.35)
```

---

## Prisms & pyramids

### `pyramid` — `金字塔` · `ピラミッド` · `피라미드` · `พีระมิด`
N-sided pyramid. `sides=4` is the classic square pyramid.
```
pyramid(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  sides=4)
```

### `prism` — `棱柱` · `角柱` · `각기둥` · `ปริซึม`
N-sided prism (extruded polygon). `sides=3` → triangular prism, `6` → hex column.
```
prism(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  sides=6)
```

### `frustum` — `棱台` · `錐台` · `원뿔대` · `กรวยตัด`
Truncated cone/pyramid. `top_ratio` is the top radius as a fraction of the base
(`0` → cone, `1` → cylinder).
```
frustum(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  sides=24, top_ratio=0.5)
```

---

## Dice (Platonic solids)

Perfect for dice, gems, and crystal shapes — all take no extra params.

| Builtin | Die | Aliases |
|---------|-----|---------|
| `tetrahedron` | d4 | `d4` · `四面体` · `정사면체` · `ทรงสี่หน้า` |
| `cube` | d6 | (see above) |
| `octahedron` | d8 | `d8` · `八面体` · `정팔면체` · `ทรงแปดหน้า` |
| `dodecahedron` | d12 | `d12` · `十二面体` · `정십이면체` · `ทรงสิบสองหน้า` |
| `icosahedron` | d20 | `d20` · `二十面体` · `정이십면체` · `ทรงยี่สิบหน้า` |

```
icosahedron(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode)
d20(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode)          # same thing
```

---

## Mechanical & architectural

### `gear` · `cog` — `齿轮` · `歯車` · `톱니바퀴` · `เฟือง`
Extruded spur gear in the XZ plane. `teeth` count and `tooth` depth are
configurable; thickness comes from `sy`.
```
gear(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  teeth=12, tooth=0.25)
```

### `gyro` — `陀螺` · `ジャイロ` · `자이로` · `ไจโร`
Nested gimbal: `rings` concentric tori on alternating axes — a gyroscope / armillary.
```
gyro(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  rings=3)
```

### `arch` — `拱门` · `アーチ` · `아치` · `ซุ้มโค้ง`
Semicircular archway (tube swept over 180°).
```
arch(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  segments=24, tube=0.18)
```

### `stairs` — `楼梯` · `階段` · `계단` · `บันได`
Staircase of `steps` cuboid steps rising along +Y/+Z.
```
stairs(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  steps=5)
```

---

## Curves & surfaces

### `helix` — `螺旋线` · `らせん` · `나선` · `เกลียว`
Tube swept along a helix around Y.
```
helix(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  turns=3, tube=0.15, sides=8)
```

### `spring` — `弹簧` · `ばね` · `스프링` · `สปริง`
A tighter, thinner helix (coil). Same params as `helix`.
```
spring(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  turns=6, tube=0.12, sides=8)
```

### `star_prism` · `star` — `星柱` · `별기둥` · `แท่งดาว`
An N-point star cross-section extruded along Y.
```
star_prism(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  points=5, inner_ratio=0.5)
```

### `capsule_chain` · `chain` — `胶囊链` · `カプセル鎖` · `캡슐체인` · `โซ่แคปซูล`
A row of `count` capsule beads along X — a chain / caterpillar.
```
capsule_chain(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  count=3)
```

### `mobius` — `莫比乌斯` · `メビウス` · `뫼비우스` · `เมอบีอุส`
A half-twisted Möbius band.
```
mobius(cx,cy,cz, sx,sy,sz, rx,ry,rz, mode,  segments=60, width=0.3)
```

---

## Tips

- **Flat disc / plate:** `cylinder` with a small `sy` (e.g. `0.05`).
- **Ellipsoid:** `sphere` with unequal `sx,sy,sz`.
- **Obelisk:** `frustum` with a small `top_ratio` and tall `sy`.
- **Animate** by feeding the frame counter into `rx,ry,rz` each frame.
- **Outlined solid:** use `mode = 2` and a bright pen colour for a crisp
  cel-look with ink edges.

All shape names are available in every Ling language; see
[Builtin Aliases by Language](../multilingual/builtins.md).
