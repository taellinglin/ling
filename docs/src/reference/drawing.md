# 2D Drawing and Images

These builtins paint straight into the window's framebuffer in screen pixels: lines, triangles,
circles, polygons, gradients and soft shadows, plus images loaded from files or built pixel by
pixel. They all need an open window and belong between `fill()` and `present()`
(see [Window and Camera](window.md)).

## How 2D drawing works

- **Coordinates** are pixels. The origin is the top-left corner, +x points right and **+y points
  down**. Coordinates outside the window are simply not drawn.
- **The pen colour.** Shapes are drawn in the current pen colour, set with `set_color`. It stays in
  effect until you change it. 3D shapes also use it as their base colour.
- **Colour values** are numbers from 0 to 255. They are **not clamped**: the runtime packs
  `(r << 16) | (g << 8) | b`, so a value above 255 spills into the next channel. Negative values
  become 0.
- **Immediate writes.** 2D calls write pixels right away. Queued 3D geometry is drawn later, at
  `present()`, so it covers your 2D drawing. To draw 2D on top of a 3D scene, call `flush_3d()` first.
- **Packed colours.** Some builtins take one number `0xRRGGBB`. Ling has no hex literals, so write it
  as `r * 65536 + g * 256 + b` (for example `51 * 65536 + 102 * 256 + 255`).

```ling-gui
bind start = do {
    open_window(640, 360, "2D")
    while window_is_open() {
        fill(12, 14, 30)
        set_color(230, 90, 90)
        triangle(60, 300, 160, 140, 260, 300)
        set_color(90, 200, 255)
        draw_circle(380, 150, 60)
        set_color(120, 255, 160)
        draw_disc(520, 150, 40)
        set_color(255, 220, 90)
        draw_arc(380, 280, 50, 0, 3.14159)
        grad_rect(20, 320, 600, 24, 255, 80, 0, 0, 80, 255, 0)
        present()
    }
}
```

## Summary: pen and shapes

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `fill(r, g, b)` | `填` | `塗り潰し` | `채우기` | `เติม` | Clear the frame to a colour |
| `set_color(r, g, b)` | `设色` | `色設定` | `색설정` | `สีดินสอ` | Pen colour |
| `set_color_hsl(h, s, l)` | `色相` | `HSL色` | `HSL색설정` | `สีHSLวาด` | Pen colour from hue, saturation, lightness |
| `mix_color(r0, g0, b0, r1, g1, b1, t)` | `混合颜色` | `色混合` | `색혼합` | `ผสมสี` | Pen colour blended between two colours |
| `set_alpha(a)` | `设透明` | `アルファ設定` | `투명도설정` | `ตั้งความโปร่งใส` | Opacity for gradients and 3D |
| `set_blend(mode)` | `混合模式` | `ブレンドモード` | `블렌드모드` | `โหมดผสม` | Normal or additive blending |
| `set_antialias(on)`, `get_antialias()` | — | — | — | — | Smooth line strokes |
| `draw_pixel(x, y)` | `画点` | `点描く` | `점그리기` | `วาดจุด` | One pixel |
| `draw_line(x0, y0, x1, y1)` | `画线` | `線描く` | `선그리기` | `วาดเส้น` | A line |
| `triangle(x0, y0, x1, y1, x2, y2)` | `画三角` | `三角形描画` | `삼각형그리기` | `วาดสามเหลี่ยม` | Filled triangle |
| `draw_circle(cx, cy, r)` | — | — | — | — | Circle outline |
| `draw_disc(cx, cy, r)` | `画实心圆` | `塗りつぶし円` | `원채우기` | `วาดวงกลมทึบ` | Filled circle |
| `draw_arc(cx, cy, r, a0, a1, segments)` | — | — | — | — | Part of a circle |
| `draw_poly(points)` | `填充多边形` | `ポリゴン塗り` | `다각형채우기` | `เติมรูปหลายเหลี่ยม` | Filled polygon |
| `grad_rect(x, y, w, h, r0, g0, b0, r1, g1, b1, dir)` | `渐变矩形` | `グラデ矩形` | `그라데사각` | `สี่เหลี่ยมไล่สี` | Gradient rectangle |
| `grad_triangle(...)` | `渐变三角` | `グラデ三角` | `그라데삼각` | `สามเหลี่ยมไล่สี` | Triangle with a colour per corner |
| `set_gradient_space(mode)` | `渐变空间` | `グラデ空間` | `그라데이션공간` | `ปริภูมิไล่สี` | Perceptual or sRGB gradients |
| `shadow_blob(cx, cy, rx, ry, alpha)` | `阴影斑` | `影ブロブ` | `그림자블롭` | `เงาวงรี` | Soft shadow ellipse |
| `cast_shadow(cx, cy, height)` | `投射阴影` | `影を落とす` | `그림자드리우기` | `ทอดเงา` | Shadow that follows height |
| `shadow_params(base, grow, alpha, fade, soft)` | `阴影参数` | `影設定` | `그림자설정` | `ตั้งค่าเงา` | Tune `cast_shadow` |
| `depth_triangle(...)`, `depth_line(...)` | `深度三角`, `深度线` | `深度三角形`, `深度線` | `깊이삼각`, `깊이선` | `สามเหลี่ยมเรียงลึก`, `เส้นเรียงลึก` | 2D shapes sorted with 3D |
| `set_frame_blur(amount)` | — | — | — | — | Afterimage trails |

## Pen and blending

### `fill(r, g, b)`

Clears the whole framebuffer to a colour and resets the depth buffer. Call it first in every frame.
Each channel defaults to 0, so `fill()` clears to black. It doesn't change the pen colour.
`clear` is an English synonym.

### `set_color(r, g, b)`, `set_color_hsl(h, s, l)` and `mix_color(...)`

`set_color` sets the pen colour (each channel defaults to 255).

`set_color_hsl` sets it from hue in degrees (0 to 360), saturation and lightness (0 to 100;
defaults 70 and 50). Stepping the hue is the easy way to a rainbow.

`mix_color(r0, g0, b0, r1, g1, b1, t)` sets the pen to a blend of two colours, `t` from 0 to 1
(default 0.5). It blends in the OkLab colour space, so mid-tones look clean rather than muddy.

```ling-gui
bind start = do {
    open_window(700, 300, "Rainbow")
    fill(0, 0, 0)
    bind i = 0
    while i < 12 {
        set_color_hsl(i * 30, 80, 55)
        draw_disc(60 + i * 50, 150, 20)
        bind i = i + 1
    }
    present()
    wait_window()
}
```

### `set_alpha(a)`

Pen opacity from 0 (invisible) to 1 (opaque), default 1. It applies to the gradient fills and to
queued 3D shapes.

### `set_blend(mode)`

How new pixels combine with what is already there: `0` normal (default), `1` additive. With
additive blending overlapping shapes get brighter, which is the basis of every glow effect.
Queued 3D draws also accept `2` multiply, `3` screen, `4` subtract and `5` overlay. Set it back
to 0 when you are done.

```ling-gui
bind start = do {
    set_blend(1)
    set_color(60, 40, 20)
    draw_disc(500, 280, 40)
    draw_disc(530, 280, 40)       # the overlap is brighter
    set_blend(0)
}
```

### `set_antialias(on)` and `get_antialias()`

Smooths line strokes: lines, arcs, circle outlines and 3D edges. Off by default, which gives crisp
pixel lines; `set_antialias()` with no argument turns it on. `get_antialias()` reads the setting back.
Text smoothing is separate (see [Fonts and UI](fonts-ui.md)).

## Shapes

### `draw_pixel(x, y)` and `draw_line(x0, y0, x1, y1)`

`draw_pixel` sets one pixel. `draw_line` draws a one-pixel line between two points; it honours
`set_antialias` and additive blending.

### `triangle(x0, y0, x1, y1, x2, y2)`

A filled triangle. Build rectangles from two triangles, or use `draw_poly`. `draw_triangle` is an
English synonym.

```ling-gui
bind start = do {
    set_color(230, 90, 90)
    triangle(100, 100, 300, 100, 300, 200)
    triangle(100, 100, 300, 200, 100, 200)      # together: a rectangle
}
```

### `draw_circle(cx, cy, r)` and `draw_disc(cx, cy, r)`

A circle outline and a filled circle, centre and radius in pixels (radius default 10).
`draw_disc` honours the blend mode, so with `set_blend(1)` it paints a glow.

### `draw_arc(cx, cy, r, a0, a1, segments)`

Strokes part of a circle from angle `a0` to `a1` in radians (default 0 to 2π, a full circle);
`segments` is optional. Because +y is down, angles grow **clockwise** on screen: 0 is to the right,
`pi() / 2` is straight down.

### `draw_poly(points)`

A filled polygon that closes itself. `points` is a flat list `[x0, y0, x1, y1, ...]`. Combine it
with `project_3d` to fill a shape between projected 3D points.

```ling-gui
bind start = do {
    set_color(200, 140, 255)
    draw_poly([100, 100, 200, 120, 180, 200, 90, 180])
}
```

### `grad_rect(x, y, w, h, r0, g0, b0, r1, g1, b1, dir)` and `grad_triangle(...)`

`grad_rect` fills a rectangle with a gradient from the first colour to the second. `dir` is `0` for
left to right and anything else for top to bottom (default 1).

`grad_triangle(x0, y0, r0, g0, b0, x1, y1, r1, g1, b1, x2, y2, r2, g2, b2)` gives each corner its
own colour and blends smoothly between them: a cheap "lit" surface.

Both honour `set_alpha`. Gradients blend perceptually by default; `set_gradient_space(0)` switches
to plain sRGB.

### `shadow_blob`, `cast_shadow` and `shadow_params`

`shadow_blob(cx, cy, rx, ry, alpha)` draws a soft ellipse in the pen colour (radii default 16 and 8,
`alpha` default 0.5). A dark pen gives an ordinary shadow; any other colour a tinted one.

`cast_shadow(cx, cy, height)` draws a contact shadow whose size and softness follow the height of the
object above the ground: low objects get small, dark, sharp shadows, high ones large faint ones.
`shadow_params(base, grow, alpha, fade, soft)` tunes it; each argument you leave out keeps its value.

```ling-gui
bind start = do {
    set_color(0, 0, 20)
    cast_shadow(320, 330, 24)
}
```

### `depth_triangle(x0, y0, x1, y1, x2, y2, z)` and `depth_line(x0, y0, x1, y1, z)`

Queue a 2D triangle or line to be depth-sorted together with the 3D geometry and drawn at
`present()`. Larger `z` is farther away. Use them for 2D sprites that must pass in front of and
behind 3D objects.

### `set_frame_blur(amount)`

Afterimage trails: each frame is mixed with a faded copy of the last one. `amount` goes from 0 to
0.95; about 0.9 leaves trails that last roughly a second at 60 frames per second. `0` turns it off.

## Images

Images live in a table inside the runtime. Each load or creation returns a **handle**, a small
number you pass to the other `image_*` builtins. `-1` means failure.

| Builtin | Purpose |
|---|---|
| `image_load(path)` | Load a PNG, JPEG or other raster image; returns a handle or `-1` |
| `image_draw(id, x, y, w, h)` | Draw it scaled to `w` by `h` pixels |
| `image_new(w, h)` | A new transparent image |
| `image_set_pixel(id, x, y, r, g, b, a)` | Paint one pixel |
| `image_pixel_r(id, x, y)`, `_g`, `_b`, `_a` | Read one channel (0 to 255) |
| `image_width(id)`, `image_height(id)` | Size in pixels |
| `image_save(id, path)` | Write a PNG; `1` or `-1` |
| `image_replace(id, path)` | Load a new file into an existing handle |
| `image_tint(id, color)` | A one-colour copy shaped by the alpha |
| `image_resized(id, w, h)` | A resized copy |
| `image_svg(svg_text, w, h)` | Render SVG source text to an image |
| `image_free(id)` | Release the pixels |

### `image_load(path)`

Loads a raster image and returns its handle, or `-1` (with a message on the error stream) if the
file can't be read. The path is tried as given, then relative to the script's folder. Loading the
same path again returns the cached handle, so it is safe to call inside the frame loop.

### `image_draw(id, x, y, w, h, cx, cy, cw, ch, opacity)`

Draws the image with its top-left corner at `x, y`, scaled (nearest neighbour) to `w` by `h` pixels
and alpha-blended over the frame. **`w` and `h` are required**: if they are 0 nothing is drawn, so
pass `image_width(id)` and `image_height(id)` for the natural size. Optional: a clip rectangle
`cx, cy, cw, ch` outside which nothing is drawn, and an `opacity` from 0 to 1 after it. Images are
shown as they are, without the toon shading the post-processing applies to 3D.

```ling-gui
bind start = do {
    open_window(640, 400, "Sprite")
    bind hero = image_load("hero.png")
    bind x = 0
    while window_is_open() {
        fill(20, 24, 40)
        if hero >= 0 {
            image_draw(hero, x, 160, image_width(hero) * 2, image_height(hero) * 2)
        }
        present()
        bind x = (x + 2) % 640
    }
}
```

### `image_new`, `image_set_pixel` and the pixel readers

`image_new(w, h)` creates a blank, fully transparent image. `image_set_pixel(id, x, y, r, g, b, a)`
paints one pixel (`a` defaults to 255). `image_pixel_r`, `image_pixel_g`, `image_pixel_b` and
`image_pixel_a` read one channel of a pixel. Out-of-range positions and bad handles are ignored
when writing and read as 0. These work without a window:

```ling
bind start = do {
    bind img = image_new(8, 4)
    image_set_pixel(img, 2, 1, 255, 128, 0)
    println("size:  " + str(image_width(img)) + " x " + str(image_height(img)))
    println("pixel: " + str(image_pixel_r(img, 2, 1)) + ", " + str(image_pixel_g(img, 2, 1)))
    println("empty: " + str(image_pixel_a(img, 0, 0)))
    bind blue = image_tint(img, 51 * 65536 + 102 * 256 + 255)
    println("tint:  " + str(image_pixel_b(blue, 2, 1)))
}
```

```output
size:  8 x 4
pixel: 255, 128
empty: 0
tint:  255
```

`image_pixel_u16(id, x, y)` returns `r * 256 + g` in one call, for 16-bit height maps.

### `image_save(id, path)` and `image_replace(id, path)`

`image_save` writes an image (loaded or created) to a file, keeping alpha, and creates missing
folders. The path is relative to where the program runs. It returns `1`, or `-1` on failure.
`image_replace` decodes a new file into an existing handle, so a picture you reload often doesn't
grow the image table; it returns `1` or `-1`.

To save the window itself, use `screenshot` (see [Window and Camera](window.md)).

### `image_tint`, `image_resized`, `image_svg` and `image_free`

`image_tint(id, color)` returns a new image in one packed colour, shaped by the original's alpha:
an icon redrawn in any colour. `image_resized(id, w, h)` returns a resized copy.
`image_svg(svg_text, w, h)` renders SVG **source text** (not a path) to a new image; read a file
first with `read_file`. All three return a new handle or `-1`.

`image_free(id)` releases an image's pixels. The handle stays valid but draws nothing.

## Text

For text, use the vector fonts or the built-in stroke font, both on [Fonts and UI](fonts-ui.md).
The quickest way to put a label on screen needs no font file:

```ling-gui
bind start = do {
    set_color(120, 255, 160)
    ui_text(30, 30, 16, "SCORE 0420")
}
```

`ui_text` draws letters as capitals and knows only basic ASCII characters; anything else shows
as a small box. For other scripts, load a font with `font_load` and draw with `font_text_fill`.
