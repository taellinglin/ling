# 3D Shapes and Meshes

These builtins put things into the 3D scene: about two dozen parametric solids, raw triangles and
lines, `vtex_*` vector patterns laid on planes, `tex_*` pixel textures, and meshes loaded from
files (Ling's own formats and glTF). Everything here needs an open window and the camera from
[Window and Camera](window.md). Lights, shading styles and post-processing are on
[Post-FX and Lighting](postfx.md).

3D calls don't draw immediately. They join a **depth queue** that is sorted and drawn at
`present()` (or `flush_3d()`), so you can issue them in any order within a frame.

## Primitive solids

Every solid takes the same arguments:

```text
name(cx, cy, cz,  sx, sy, sz,  rx, ry, rz,  mode,  e0, e1, e2)
```

| Arguments | Meaning |
|---|---|
| `cx, cy, cz` | Centre in world coordinates |
| `sx, sy, sz` | Scale per axis (default 1). Shapes are about one unit in radius, so the scale is roughly the half-size: `cube(0, 0, 8, 1, 1, 1)` is a 2-unit cube |
| `rx, ry, rz` | Rotation in radians (default 0), applied X, then Y, then Z |
| `mode` | `0` filled (default), `1` wireframe, `2` both |
| `e0, e1, e2` | Shape-specific extras (see the table below); 0 or omitted means the default |

Only the position is required; trailing arguments can be dropped. The pen colour (`set_color`) is
the fill and wire colour. Solids are lit by the scene lights and shaded in the toon style.

Remember +y points down. A `cone` has its apex at +y, so unrotated it points down the screen;
rotate it by π on x to flip it. `gear` and `torus` lie in the x-z plane; rotate by about 1.57 on x to
face the camera.

```ling-gui
bind start = do {
    open_window(900, 600, "Shapes")
    set_depth_test(1)
    bind n = 0
    while window_is_open() {
        fill(8, 10, 24)
        set_camera(1, 0, 1, 0)
        clear_lights()
        add_light(-2, -6, -2, 1.0, 0.95, 0.85, 0.6, 0)
        bind a = n * 0.03
        set_color(230, 120, 90)
        cube(-2.8, 0, 7.5, 0.8, 0.8, 0.8, a, a * 0.7, 0, 0)
        set_color(120, 200, 255)
        sphere(0, 0, 7.5, 0.9, 0.9, 0.9, 0, 0, 0, 0)
        set_color(190, 190, 200)
        gear(2.8, 0, 7.5, 0.9, 0.3, 0.9, 1.57, a * 2, 0, 0, 14, 0.3)
        present()
        bind n = n + 1
    }
}
```

### Summary: solids

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Extras `e0, e1, e2` |
|---|---|---|---|---|---|
| `cube` | `立方体` | `立方体` | `정육면체` | `ลูกบาศก์` | none (alias `box`) |
| `sphere` | `球体` | `球` | `구` | `ทรงกลม` | segments 16, rings 12 |
| `icosphere` | `二十面球` | `アイコ球` | `아이코구체` | `ทรงกลมเหลี่ยม` | subdivisions 0 to 4 (1) |
| `dome` | `穹顶` | `ドーム` | `돔` | `โดม` | segments 24, rings 8 (a closed hemisphere) |
| `cylinder` | `圆柱` | `円柱` | `원기둥` | `ทรงกระบอก` | segments 24 |
| `cone` | `圆锥` | `円錐` | `원뿔` | `กรวย` | segments 24 |
| `capsule` | `胶囊` | `カプセル` | `캡슐` | `แคปซูล` | segments 16, rings per cap 6 |
| `torus` | `圆环` | `トーラス` | `토러스` | `ทอรัส` | segments 32, sides 12, tube 0.35 (alias `ring`) |
| `pyramid` | `金字塔` | `ピラミッド` | `피라미드` | `พีระมิด` | sides 4 |
| `prism` | `棱柱` | `角柱` | `각기둥` | `ปริซึม` | sides 6 |
| `frustum` | `棱台` | `錐台` | `원뿔대` | `กรวยตัด` | sides 24, top ratio 0.5 |
| `tetrahedron` | `四面体` | `四面体` | `정사면체` | `ทรงสี่หน้า` | none (alias `d4`) |
| `octahedron` | `八面体` | `八面体` | `정팔면체` | `ทรงแปดหน้า` | none (alias `d8`) |
| `dodecahedron` | `十二面体` | `十二面体` | `정십이면체` | `ทรงสิบสองหน้า` | none (alias `d12`) |
| `icosahedron` | `二十面体` | `二十面体` | `정이십면체` | `ทรงยี่สิบหน้า` | none (alias `d20`) |
| `gear` | `齿轮` | `歯車` | `톱니바퀴` | `เฟือง` | teeth 12, tooth depth 0.25 (alias `cog`) |
| `gyro` | `陀螺` | `ジャイロ` | `자이로` | `ไจโร` | rings 3 |
| `arch` | `拱门` | `アーチ` | `아치` | `ซุ้มโค้ง` | segments 24, tube 0.18 |
| `stairs` | `楼梯` | `階段` | `계단` | `บันได` | steps 5 (2 to 40) |
| `helix` | `螺旋线` | `らせん` | `나선` | `เกลียว` | turns 3, tube 0.15, sides 8 |
| `spring` | `弹簧` | `ばね` | `스프링` | `สปริง` | turns 6, tube 0.12, sides 8 |
| `star_prism` | `星柱` | `星柱` | `별기둥` | `แท่งดาว` | points 5, inner ratio 0.5 (alias `star`) |
| `capsule_chain` | `胶囊链` | `カプセル鎖` | `캡슐체인` | `โซ่แคปซูล` | beads 3 (1 to 12) (alias `chain`) |
| `mobius` | `莫比乌斯` | `メビウス` | `뫼비우스` | `เมอบีอุส` | segments 60, width 0.3 |

### Notes on individual solids

- **`cylinder`** runs along local y from -1 to 1, so it is `2 * sy` tall. A small `sy` such as 0.05
  makes a disc or plate.
- **`torus`**: `tube` is the tube radius as a fraction of the outer radius (0.02 to 0.9).
- **`frustum`**: the top ratio is the top radius over the base radius. Near 0 is a cone, 1 a
  cylinder; a small ratio with a tall `sy` makes an obelisk.
- **`gear`**: the thickness comes from `sy`; teeth range from 3 to 96, tooth depth from 0.02 to 0.6.
- **`stairs`**: each step is one step further in +y and +z. Since +y is down, they seem to descend
  as they recede; rotate by π on `rz` to flip them.
- **`gyro`** nests gimbal rings on alternating axes; **`capsule_chain`** lines beads up along x.
- The dice (`tetrahedron` to `icosahedron`) look like gems with `mode` 2 and a bright colour.

```ling-gui
bind start = do {
    set_color(255, 140, 180)
    icosahedron(0, 0, 8, 1, 1, 1, 0.4, 0.7, 0, 2)
    cone(3, 0, 8, 0.8, 1, 0.8, 3.14159, 0, 0, 0)       # apex up
}
```

## Triangles, lines and the depth test

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `draw_triangle_3d(ax, ay, az, bx, by, bz, cx, cy, cz)` | — | — | — | — | One lit triangle |
| `draw_quad_3d(...)`, `draw_polygon_3d(...)` | — | — | — | — | A lit convex polygon |
| `draw_line_3d(ax, ay, az, bx, by, bz)` | — | — | — | — | A depth-sorted line |
| `set_depth_test(on)` | `深度测试` | `深度テスト` | `깊이테스트` | `ทดสอบความลึก` | Per-pixel depth buffer |
| `clear_depth()` | — | — | — | — | Clear the depth buffer mid-frame |

### `draw_triangle_3d`, `draw_quad_3d` and `draw_polygon_3d`

Draw world-space geometry in the pen colour, lit, fogged and clipped at the near plane.
`draw_triangle_3d` takes three points; the winding order decides which side faces the light.
`draw_quad_3d` takes four `x, y, z` triples (12 numbers); `draw_pent_3d` and `draw_hex_3d` take 5 and 6.
They share one implementation with `draw_polygon_3d`, which also accepts a single flat list
`[x0, y0, z0, x1, y1, z1, ...]`. At most 8 vertices are used.

```ling-gui
bind start = do {
    set_color(120, 200, 255)
    draw_quad_3d(-2, 1, 8, 2, 1, 8, 2, 1, 12, -2, 1, 12)     # a floor tile
    draw_polygon_3d([0, -1, 8, 1, 0, 8, 0, 1, 8, -1, 0, 8])  # a diamond
}
```

### `draw_line_3d(ax, ay, az, bx, by, bz)`

A line between two world points, depth-sorted with the rest and clipped at the near plane. Use
`set_antialias(1)` for smooth lines.

### `set_depth_test(on)` and `clear_depth()`

The depth test is off by default: queued geometry is painted far to near. Turn it on (`1`) whenever
solids may intersect, and for ambient occlusion, outlines and depth of field. `fill()` already
clears the depth buffer each frame; `clear_depth()` is for starting a fresh depth pass mid-frame.

## Vector patterns: `vtex_*`

Each `vtex_*` call draws an animated line pattern on a **plane in 3D space**. The first nine
arguments are the same for all of them, and the last two are always `fr` and `hue`:

```text
vtex_name(cx, cy, cz,  ux, uy, uz,  vx, vy, vz,  ...pattern arguments...,  fr, hue)
```

`cx, cy, cz` is the centre. `u` is the direction "right" runs in the pattern and `v` the direction
"up" runs. `fr` is a frame counter you supply, which drives colour and motion; `hue` offsets the
colour so neighbouring patterns differ. The colour cycles through a rainbow on its own; it doesn't
use the pen colour.

| Plane | `u` | `v` |
|---|---|---|
| Floor or ceiling | `1, 0, 0` | `0, 0, 1` |
| Wall facing the camera, upright | `1, 0, 0` | `0, -1, 0` |
| Side wall, upright | `0, 0, 1` | `0, -1, 0` |

Upright patterns use `v = (0, -1, 0)` because +y is down. The patterns are plain line segments, so
they are cheap, and they are drawn slightly toward the camera so they sit on top of a surface in the
same place.

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Pattern arguments (defaults) |
|---|---|---|---|---|---|
| `vtex_grid` | `纹格` | `格子模様` | `격자무늬` | `ลายตาราง` | cols 10, rows 10, cell_w 1, cell_h 1 |
| `vtex_rings` | `纹环` | `リング模様` | `링무늬` | `ลายวงซ้อน` | rings 6, sides 6, max_r 3, twist 0 |
| `vtex_star` | `纹星` | `星模様` | `별무늬` | `ลายดาว` | points 6, r_out 2, r_in 1, spin per frame 0.01 |
| `vtex_spiral` | `纹螺` | `螺旋模様` | `나선무늬` | `ลายเกลียว` | turns 3, max_r 3, steps 120 |
| `vtex_flower` | `纹花` | `花模様` | `꽃무늬` | `ลายดอกไม้` | radius 1, sides 24 (Flower of Life) |
| `vtex_halftone` | `纹半调` | `ハーフトーン` | `하프톤무늬` | `ลายจุด` | cols 16, rows 12, cell_w 0.5, cell_h 0.5, density 0.4 |
| `vtex_tessellated` | — | — | — | — | cols 14, rows 10, cell 0.6, amplitude 0.25, freq 4 |
| `vtex_hyperbolic_uv` | — | — | — | — | max_r 5, circles 12, rays 18 (Poincaré-disc grid) |
| `vtex_letter_rain` | — | — | — | — | cols 16, visible 14, col_w 0.65, row_h 0.6, speed 0.025 |
| `vtex_lotus` | `纹莲` | `蓮模様` | `연꽃무늬` | `ลายดอกบัว` | r_inner 1, r_outer 2, petals 12 |
| `vtex_chakra` | `纹轮` | `輪模様` | `바퀴무늬` | `ลายจักร` | r 2, spokes 8 (Dhamma wheel) |
| `vtex_yantra` | `纹扬特拉` | `ヤントラ模様` | `얀트라무늬` | `ลายยันต์` | layers 4, max_r 3 |
| `vtex_spiked_cog` | `纹刺轮` | `スパイクコグ` | `스파이크톱니무늬` | `ฟันเฟืองหนาม` | teeth 12, r_body 1, r_spike 1.3, r_hub 0.2, spokes 6 |
| `vtex_torii` | `纹鸟居` | `鳥居` | `도리이` | `ประตูโทริอิ` | width 4, height 5 |
| `vtex_pagoda` | `纹塔` | `塔` | `탑` | `เจดีย์` | tiers 5, base_w 2, tier_h 1, taper 0.72, eave 0.28 |

Some details: `vtex_rings` turns each ring by `twist` radians more than the last, making a vortex.
`vtex_halftone` density runs from 0 (invisible) to 1 (full hatching). `vtex_tessellated` shakes a
triangle grid with a travelling sine wave. `vtex_letter_rain` is "matrix" rain made of Thai letters.
`vtex_pagoda` has 3 to 7 tiers, each `taper` times narrower than the one below.

```ling-gui
bind start = do {
    open_window(900, 600, "vtex")
    bind n = 0
    while window_is_open() {
        fill(4, 5, 12)
        set_camera(1, 0, 1, 0)
        vtex_grid(0, 3, 9, 1, 0, 0, 0, 0, 1, 12, 8, 1, 1, n, 0)
        vtex_rings(-3, -1, 8, 1, 0, 0, 0, -1, 0, 6, 6, 1.2, 0.2, n, 0)
        vtex_lotus(0, -1, 8, 1, 0, 0, 0, -1, 0, 0.4, 1.2, 12, n, 1)
        vtex_pagoda(3, 1.9, 8, 1, 0, 0, 0, -1, 0, 5, 1.6, 0.5, 0.72, 0.2, n, 2)
        present()
        bind n = n + 1
    }
}
```

`orb_shell(cx, cy, cz, radius, rot_y, rot_x, density, r, g, b)` is a relative of the patterns: a
translucent shell of interleaved spirals wound around a sphere (`density` 1 to 48, default 10;
tint defaults to a pale grey). Aliases: zh `球壳`, ja `オーブ殻`, ko `오브껍질`, th `เปลือกทรงกลม`.

## Pixel textures: `tex_*`

The `tex_*` builtins fill a screen rectangle with a procedural pattern, one pixel at a time,
**immediately**. Like other 2D calls, they are covered by queued 3D geometry. All start with the
rectangle `x, y, w, h` in pixels (default 0, 0, 256, 256).

Every pixel is computed on every call, so a large texture is slow. Keep them small, or draw them
once and leave them on screen.

Where a `palette` is accepted it is one of `"rainbow"`, `"fire"`, `"ocean"`, `"psychedelic"`,
`"neon"` or `"forest"`; an unknown name falls back to rainbow.

| Builtin | Purpose |
|---|---|
| `tex_checkerboard(x, y, w, h, tiles, r1, g1, b1, r2, g2, b2)` | Two-colour checker (8 tiles, white and black) |
| `tex_gradient(x, y, w, h, angle_deg, r1, g1, b1, r2, g2, b2)` | Linear gradient along an angle (0 is left to right) |
| `tex_noise(x, y, w, h, scale, octaves, seed, palette)` | Fractal noise (scale 4, octaves 1 to 8) |
| `tex_spiral(x, y, w, h, freq, bands, time, palette)` | Spiral bands that turn with `time` |
| `tex_ripple(x, y, w, h, freq, cx, cy, time, palette)` | Ripples from `cx, cy` (0 to 1 inside the rectangle) |
| `tex_mandelbrot(x, y, w, h, zoom, cx, cy, max_iter, palette)` | Mandelbrot set |
| `tex_julia(x, y, w, h, c_re, c_im, max_iter, palette)` | Julia set (c = -0.7 + 0.27i) |
| `tex_voronoi(x, y, w, h, cells, seed, palette)` | Voronoi cells (16 cells) |
| `tex_halftone(x, y, w, h, dot_size, time, palette)` | Halftone dots |
| `tex_freq_map(x, y, w, h, time, speed, palette)` | Spectrum from the last `fft_bands` call (see [Audio](audio.md)) |

`tex_mandelbrot` shows `1 / zoom` units of the complex plane in both directions, so `zoom` 1 shows
only the main cardioid; use about 0.4 to see the whole set.

```ling-gui
bind start = do {
    open_window(660, 340, "tex")
    fill(0, 0, 0)
    tex_checkerboard(10, 10, 200, 150, 8, 240, 240, 240, 30, 30, 60)
    tex_noise(230, 10, 200, 150, 5, 4, 7, "fire")
    tex_mandelbrot(450, 10, 200, 150, 0.4, -0.5, 0, 64, "psychedelic")
    present()
    wait_window()
}
```

## Meshes

Ling can draw meshes you build in lists, its own binary mesh formats, and glTF models. All of
them are projected through the camera and queued with the rest of the 3D scene.

### `draw_mesh(positions, indices, ox, oy, oz, scale, mode)`

Draws a triangle mesh stored in two lists: `positions` is flat `[x0, y0, z0, x1, y1, z1, ...]`
and `indices` lists three vertex numbers per triangle. Every vertex is scaled by `scale`
(default 1) and moved by `ox, oy, oz`. `mode` `0` lights it in the pen colour; `1` gives each
triangle a rainbow colour. Bad indices are ignored.

```ling-gui
bind start = do {
    bind pos = [0, -1, 8, 1, 1, 8, -1, 1, 8, 0, 0, 10]
    bind idx = [0, 1, 2, 0, 2, 3, 0, 3, 1]
    set_color(200, 160, 255)
    draw_mesh(pos, idx, 0, 0, 0, 1, 0)
}
```

### `mesh_load(path)` and `mesh_draw(id, x, y, z, scale, yaw)`

`mesh_load` reads a `.lmesh` file (a compact format with one flat colour per triangle) and returns a
handle or `-1`. `mesh_draw` draws it **unlit**, with its own colours, at a position, uniform scale and
yaw angle. Further optional arguments (`sway`, `arm`, `lean`, `leg`, `tuck`, default 0) deform a
character mesh: `sway` leans the upper body and `arm` swings the arms. `mesh_free(id)` releases the
geometry. Aliases for `mesh_load`: zh `载入网格`, ja `メッシュ読込`, ko `메시로드`, th `โหลดเมช`.

### glTF models: `gltf_*`

`gltf_load(path)` reads a `.glb` or `.gltf` file, including its skeleton, skin and animations, and
returns a handle or `-1`. glTF is Y-up; the draw calls flip it into Ling's Y-down world for you.

| Builtin | Purpose |
|---|---|
| `gltf_load(path)` | Load a model; handle or `-1` |
| `gltf_draw(h, x, y, z, scale, yaw)` | Draw it lit, in its rest pose |
| `gltf_anim_draw_tex(h, x, y, z, scale, yaw, clip, time)` | Draw it textured, posed by an animation clip at `time` seconds |
| `gltf_anim_count(h)`, `gltf_anim_name(h, i)`, `gltf_anim_dur(h, i)` | Animation clips: count, name, length in seconds |
| `gltf_bounds(h, clip, time)` | `[minx, miny, minz, maxx, maxy, maxz]` as drawn, in glTF space |
| `gltf_tris(h)` | Triangle count |
| `gltf_mesh_count(h)`, `gltf_mesh_name(h, i)` | The model's meshes |
| `gltf_mesh_show(h, i, on)` | Show or hide one mesh in later draws (equipment, outfits) |
| `gltf_mesh_tint(h, i, color)` | Multiply one mesh by a packed colour |
| `gltf_joint_count(h)`, `gltf_joint_name(h, j)` | Skeleton bones |

In `gltf_bounds` y is up, so the top of a model drawn at ground height `gy` with scale `s` is at
`gy - maxy * s`.

```ling-gui
bind start = do {
    open_window(800, 600, "Model")
    set_depth_test(1)
    bind fox = gltf_load("models/fox.glb")
    bind clip = 0
    while window_is_open() {
        fill(20, 24, 40)
        clear_lights()
        add_light(-3, -6, 2, 1.0, 1.0, 1.0, 1.0, 0)
        if fox >= 0 {
            bind len = gltf_anim_dur(fox, clip)
            gltf_anim_draw_tex(fox, 0, 2, 8, 0.02, time_now(), clip, time_now() % max(len, 0.01))
        }
        present()
    }
}
```

## Textured meshes: `tm_*`

The `tm_*` family draws textured geometry for 3D games. Textures and meshes each get a handle.

| Builtin | Purpose |
|---|---|
| `tm_tex_load(path, size)` | Load a PNG, SVG or SVGZ as a square texture of `size` pixels (default 256); id or `-1` |
| `tm_load(path)` | Load an `.ltm` mesh (named groups of textured triangles); id or `-1` |
| `tm_groups(m)`, `tm_group(m, name)`, `tm_group_name(m, g)`, `tm_group_tris(m, g)` | Inspect a mesh's groups |
| `tm_bounds(m, g)` | `[minx, miny, minz, maxx, maxy, maxz]` of one group, or all with `g = -1` |
| `tm_edges(m, g, max)` | Outline edges as a flat list of segments, for minimaps and wireframes |
| `tm_draw(m, g, tex, x, y, z, scale, rot, tint, alpha, ...)` | Queue a group as textured triangles |
| `tm_sprite(tex, cx, cy, w, h, rot, tint, alpha, blend)` | A rotated textured rectangle drawn straight onto the screen |
| `tm_billboard(tex, x, y, z, size, rot, tint, alpha, blend)` | A camera-facing textured square in 3D |
| `tm_quad(tex, x, y, z, ux, uy, uz, vx, vy, vz, tint, alpha, blend)` | A textured quad in any orientation (decals) |
| `tm_wire(kind, x, y, z, size, rx, ry, rz, color, detail, thick)` | A wireframe shape |
| `tm_solid(kind, x, y, z, size, rx, ry, rz, color, alpha, blend)` | A faceted, flat-shaded crystal solid |

Common conventions: `tint` and `color` are packed `0xRRGGBB` numbers (default white), `alpha` goes
from 0 to 1, and `blend` is `0` normal or `1` additive. A texture id of `-1` means a built-in soft
glow dot; for `tm_quad`, `-2` is a soft round shadow.

- **`tm_draw`** continues with `uoff, voff, uscale, vscale` (texture scroll and scale), `shade`
  (0 to 1, darkens faces turned from the light), `blend` and `cull` (`0` none, `1` or `2` one
  winding). `rot` is a yaw angle or a row-major 3 by 3 list. It returns the number of triangles queued.
- **`tm_sprite`** draws onto the 2D framebuffer, so call it after `flush_3d()` for a HUD or title card.
- **`tm_wire`** kinds: `0` cube, `1` tetrahedron, `2` octahedron, `3` icosahedron, `4` torus,
  `5` sphere, `6` ring, `7` star prism, `8` grid. `size` is the radius; `thick` 1 doubles the strokes.
- **`tm_solid`** kinds: `0` cube, `1` tetrahedron, `2` octahedron, `3` icosahedron, `9` a shard
  (a stretched octahedron).

```ling-gui
bind start = do {
    open_window(800, 600, "Crystals")
    bind t = 0.0
    while window_is_open() {
        fill(6, 8, 20)
        set_blend(1)
        tm_wire(3, -2, 0, 8, 1, t, t * 0.7, 0, 65535, 12, 1)
        set_blend(0)
        tm_solid(9, 2, 0, 8, 0.8, 0, t, 0, 16744576, 0.8, 1)
        present()
        bind t = t + 0.02
    }
}
```
