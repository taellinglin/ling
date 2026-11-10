# Fonts and UI

This page covers text and interface: TrueType and OpenType fonts drawn as sharp vector outlines,
the built-in stroke font, and the `ui_*` toolkit of meters, gauges and controls. All of them draw
into the same framebuffer as the [2D drawing](drawing.md) calls, so they belong between `fill()`
and `present()`. To draw them over a 3D scene, call `flush_3d()` first. The typewriter dialog box
has its own page: [Dialog](dialog.md).

## Fonts

Ling draws two kinds of text:

* **Vector fonts.** `font_load` reads a `.ttf` or `.otf` file; `font_text_fill`, `font_text` and
  `font_text_3d` draw its glyph outlines. The curves are kept, so text is sharp at any size. Latin,
  Chinese, Japanese, Korean and Thai all work if the font has the glyphs.
* **The stroke font.** `ui_text` draws a simple monospaced "holographic" face that needs no font
  file, for basic ASCII text.

The first time a glyph is used, its outline is cached on disk under `cache/fonts/` in the folder the
program runs in, so later runs start faster. In the web runtime `font_load` returns `-1`; check the
handle and fall back to `ui_text` if you want a program to run in both.

```ling-gui
bind start = do {
    open_window(720, 320, "Fonts")
    bind f = font_load("assets/fonts/Exo2.ttf", 600)
    bind cjk = font_load("assets/fonts/NotoSansSC.ttf")
    while window_is_open() {
        fill(8, 10, 24)
        set_color(0, 230, 255)
        font_text_fill(f, 30, 40, 48, "Hello Ling")
        set_color(255, 200, 80)
        font_text(f, 30, 110, 48, "Stroked Outline")
        set_color(255, 255, 255)
        font_text_fill(cjk, 30, 180, 40, "你好 こんにちは")
        set_color(120, 255, 160)
        ui_text(30, 260, 16, "UI TEXT 0123")
        present()
    }
}
```

### Summary: fonts

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `font_load(path, weight)` | `加载字体` | `フォント読込` | `글꼴로드` | `โหลดฟอนต์` | Load a font, get a handle |
| `font_text_fill(font, x, y, px, text)` | `填充字体` | `フォント塗り` | `글꼴채움` | `เติมฟอนต์` | Filled text |
| `font_text(font, x, y, px, text)` | `字体文本` | `フォント文字` | `글꼴텍스트` | `ข้อความฟอนต์` | Outlined text |
| `font_text_3d(font, ..., size, text, rows)` | `字体3D` | `フォント3D` | `글꼴3D` | `ข้อความฟอนต์3มิติ` | Text on a 3D plane |
| `font_width(font, px, text)` | `字体宽度` | `フォント幅` | `글꼴너비` | `ความกว้างฟอนต์` | Measure a string in pixels |
| `font_advance(font, char)` | — | — | — | — | Advance of one glyph, in ems |
| `font_glyph_outline(font, char, tol)` | — | — | — | — | A glyph's outline as polylines |
| `font_fallback(primary, fallback, scale)` | — | — | — | — | Fallback font for missing characters |
| `set_font_antialias(on)`, `get_font_antialias()` | — | — | — | — | Smooth glyph edges |
| `ui_text(x, y, scale, text)` | `界面文字` | `UI文字` | `UI텍스트` | `ข้อความหน้าจอ` | Built-in stroke font |

### `font_load(path, weight)`

Loads a font and returns a handle (0 for the first font, 1 for the next, and so on), or `-1` if it
can't be read; a message then goes to the error stream. The path is tried as given, then relative to
the script's folder. `weight` is optional: for a variable font, a number such as 600 gives a bold
look; 0 or nothing uses the font's default.

The Ling repository ships Exo2, Orbitron, Rajdhani, Noto Sans (SC, JP, KR, Thai) and Galmuri in
`assets/fonts/`. If your program may start from different folders, try a second path:

```ling-gui
bind start = do {
    bind f = font_load("assets/fonts/Exo2.ttf", 600)
    if f < 0 { bind f = font_load("../../assets/fonts/Exo2.ttf", 600) }
}
```

### `font_text_fill(font, x, y, px, text)` and `font_text(font, x, y, px, text)`

`font_text_fill` draws solid glyphs in the pen colour; `font_text` draws their outlines, which is
cheaper and honours `set_blend` and `set_antialias`. `x, y` is the **top-left** of the line, not the
baseline: capitals start about `0.3 * px` below `y`, and the baseline is about `0.97 * px` below it.
`px` is the size in pixels (default 16). Edges are crisp unless you call `set_font_antialias(1)`.
A bad handle draws nothing.

### `font_text_3d(font, cx, cy, cz, ux, uy, uz, vx, vy, vz, size, text, fill_rows)`

Draws text on a plane in 3D, depth-sorted with the scene so it turns with the camera. `c` is where the
line starts, `u` the direction the text advances (default `1, 0, 0`), `v` the glyphs' up direction
(default `0, 1, 0`), and `size` the height of one em in world units (default 1). Because +y is down,
upright text needs `v = (0, -1, 0)`. Outlines only, unless `fill_rows` is above 0: then each glyph is
filled with that many scan lines.

```ling-gui
bind start = do {
    bind f = font_load("assets/fonts/Exo2.ttf", 600)
    set_color(120, 230, 255)
    font_text_3d(f, -2, 0, 8, 1, 0, 0, 0, -1, 0, 0.6, "LING 3D")
}
```

### `font_width(font, px, text)`

The width of a string in pixels (0 for a bad handle). Note the order: the size comes **before** the
text. To centre a label in a box `bx, by, bw, bh`, draw it at
`x = bx + (bw - font_width(f, px, text)) / 2` and `y = by + bh / 2 - 0.64 * px`.

### `font_advance(font, char)` and `font_glyph_outline(font, char, tol)`

`font_advance` gives the advance width of one character in ems (multiply by the pixel size).
`font_glyph_outline` returns a character's outline for your own effects: a list of contours, each a
flat list `[x0, y0, x1, y1, ...]` in em units with the baseline at 0 and **y pointing up**. `tol` is
how far the polyline may stray from the true curve (default 0.01 em). It is empty on failure.

### `font_fallback(primary, fallback, scale)`

Uses `fallback` for every character `primary` lacks, for example a wide Unicode font behind a stylish
Latin one. `scale` sizes the fallback relative to the primary (default 1). Pass `-1` as `fallback` to
remove it. It returns the primary handle, or `-1` if a handle is bad.

```ling-gui
bind start = do {
    bind latin = font_load("assets/fonts/Orbitron.ttf", 600)
    bind wide = font_load("assets/fonts/NotoSansSC.ttf")
    font_fallback(latin, wide, 0.9)
}
```

### `set_font_antialias(on)` and `get_font_antialias()`

Turns glyph smoothing on or off (off by default, for crisp text). `set_font_antialias()` with no
argument turns it on. This is separate from `set_antialias`, which affects line strokes.

### `ui_text(x, y, scale, text)`

Draws text in the built-in stroke font, in the pen colour. `x, y` is the top-left; glyphs are `scale`
pixels tall (default 16), each `scale * 0.62` wide with a gap of `scale * 0.24`. Letters are drawn as
capitals, and characters the face lacks (including all non-ASCII text) show as a small box.

## The UI toolkit

The `ui_*` builtins are an **immediate-mode** toolkit. Nothing is stored between frames: you call
each widget every frame with the current value, it draws itself, and an interactive control returns
the new value. Your program keeps all the state in ordinary variables.

```ling-gui
bind start = do {
    open_window(480, 220, "Controls")
    bind volume = 0.4
    bind enabled = 0
    while window_is_open() {
        fill(6, 10, 22)
        bind volume = ui_slider(40, 60, 280, volume, 0, 1)
        bind enabled = ui_toggle(40, 100, 52, 24, enabled)
        if ui_button(40, 150, 120, 36) == 1 { println("clicked") }
        set_color(220, 240, 255)
        ui_text(64, 160, 14, "PLAY")
        present()
    }
}
```

Shared conventions:

* **Coordinates** are screen pixels. Rectangular widgets take `x, y, w, h`; round ones a centre
  `cx, cy` and radius `r`.
* **Colours** come from the theme (`ui_theme`). Most widgets accept an optional trailing `r, g, b`
  that overrides their main colour for that call. Pass all three or none.
* **Widgets draw no text.** A button has no label and a tab strip no captions. Draw labels yourself
  with `ui_text` or `font_text_fill`.
* **Interaction** reads the left mouse button. A button returns `1` on the single frame the mouse
  goes down over it, so compare with `== 1`. This needs `present()` once per frame.
* **Blend.** Widgets honour `set_blend`, so `set_blend(1)` makes them glow.

### Summary: UI toolkit

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `ui_theme(...)` | `界面主题` | `UIテーマ` | `인터페이스테마` | `ธีมส่วนติดต่อ` | Set the six theme colours |
| `ui_theme_colors()` | — | — | — | — | Read the theme back |
| `ui_hot(x, y, w, h)` | `热区` | `ホットエリア` | `핫존` | `พื้นที่สัมผัส` | Is the mouse inside? |
| `ui_frame(x, y, w, h, bracket)` | `边框` | `フレーム枠` | `프레임틀` | `กรอบ` | Corner brackets |
| `ui_bevel(x, y, w, h, bevel)` | `斜角框` | `ベベル枠` | `베벨틀` | `กรอบเฉียง` | Corner-cut outline |
| `ui_panel(x, y, w, h, bevel)` | `面板` | `パネル` | `패널` | `แผง` | Filled backdrop panel |
| `ui_scanlines(x, y, w, h, density)` | `扫描线` | `走査線` | `스캔라인` | `เส้นสแกน` | CRT scan lines |
| `ui_radar(cx, cy, r, sweep)` | `雷达` | `レーダー` | `레이더` | `เรดาร์` | Radar scope |
| `ui_compass(x, y, w, h, heading)` | `罗盘` | `コンパス` | `나침반` | `เข็มทิศ` | Compass strip (degrees) |
| `ui_reticle(cx, cy, r, spread)` | `准星` | `照準` | `조준선` | `เป้าเล็ง` | Aiming reticle |
| `ui_target(x, y, w, h, lock)` | `锁定框` | `ターゲット` | `표적` | `กรอบเป้า` | Target-lock brackets |
| `ui_bar(x, y, w, h, value, max)` | `进度条` | `バー` | `막대` | `แถบ` | Fill bar |
| `ui_segbar(x, y, w, h, value, max, segments)` | `分段条` | `分割バー` | `분할막대` | `แถบแบ่ง` | Bar of blocks |
| `ui_gauge(cx, cy, r, value, max)` | `仪表` | `ゲージ` | `게이지` | `มาตรวัด` | Dial with a needle |
| `ui_ring(cx, cy, r, value, max)` | `环表` | `リングメーター` | `링미터` | `วงแหวนวัด` | Progress ring |
| `ui_vu(x, y, w, h, levels)` | `音量条` | `VUメーター` | `음량막대` | `มาตรเสียง` | Level meter from a list |
| `ui_spark(x, y, w, h, values)` | `迷你图` | `スパークライン` | `스파크라인` | `กราฟจิ๋ว` | Tiny line graph |
| `ui_battery(x, y, w, h, value, max)` | `电池` | `バッテリー` | `배터리` | `แบตเตอรี่` | Battery icon |
| `ui_progress(x, y, w, h, fraction)` | `进度` | `プログレス` | `진행바` | `ความคืบหน้า` | Thin progress bar |
| `ui_button(x, y, w, h)` | `按钮` | `ボタン` | `버튼` | `ปุ่ม` | Button; `1` when clicked |
| `ui_toggle(x, y, w, h, state)` | `开关` | `トグル` | `토글` | `สวิตช์` | On/off switch |
| `ui_slider(x, y, w, value, min, max)` | `滑块` | `スライダー` | `슬라이더` | `แถบเลื่อน` | Horizontal slider |
| `ui_checkbox(x, y, size, checked)` | `复选框` | `チェックボックス` | `체크박스` | `ช่องเลือก` | Check box |
| `ui_tabs(x, y, w, h, count, active)` | `标签页` | `タブ` | `탭` | `แท็บ` | Tab strip |
| `ui_stepper(x, y, w, h, value, step)` | `步进器` | `ステッパー` | `스테퍼` | `ตัวปรับค่า` | Minus and plus buttons |
| `ui_tooltip(x, y, w, h)` | `提示框` | `ツールチップ` | `툴팁` | `คำแนะนำ` | Callout bubble |
| `ui_healthbar(x, y, w, h, value, max, pulse)` | `血条` | `体力バー` | `체력바` | `แถบพลังชีวิต` | Game health bar |
| `ui_cooldown(cx, cy, r, fraction)` | `冷却` | `クールダウン` | `쿨다운` | `คูลดาวน์` | Cooldown pie |
| `ui_counter(x, y, dw, dh, value, digits)` | `计数器` | `カウンター` | `카운터` | `ตัวนับ` | Seven-segment number |
| `ui_minimap(x, y, w, h)` | `小地图` | `ミニマップ` | `미니맵` | `แผนที่ย่อ` | Minimap frame |
| `ui_dpad(cx, cy, r)` | `方向键` | `方向パッド` | `방향패드` | `ปุ่มทิศทาง` | On-screen d-pad |
| `ui_slotgrid(x, y, cols, rows, cell, selected)` | `物品格` | `スロットグリッド` | `슬롯격자` | `ช่องไอเทม` | Inventory grid |
| `ui_vignette(intensity)` | `暗角` | `ビネット` | `비네트` | `ขอบมืด` | Screen-edge alert glow |
| `ui_gauge3d(cx, cy, r, value, max, spin)` | `立体仪表` | `立体ゲージ` | `입체게이지` | `มาตรวัด3มิติ` | Spinning ring gauge |
| `ui_panel3d(x, y, w, h, depth)` | `立体面板` | `立体パネル` | `입체패널` | `แผง3มิติ` | Extruded panel |
| `ui_radar3d(cx, cy, r, tilt, sweep)` | `立体雷达` | `立体レーダー` | `입체레이더` | `เรดาร์3มิติ` | Tilted radar dish |
| `ui_sound(name)` | `界面音` | `UI音` | `인터페이스음` | `เสียงปุ่ม` | Short interface sound |

### `ui_theme(...)` and `ui_theme_colors()`

`ui_theme` sets the colours every widget draws from: six `r, g, b` triples in the order primary,
accent, track (the dim background of meters), warn, text and bg (panel fill). Triples you leave out
keep their value, so `ui_theme(255, 0, 128)` changes only the primary. `ui_theme_colors()` returns
the live theme as a list of 18 numbers, so hand-drawn interface can match it:

```ling
bind start = do {
    ui_theme(255, 0, 128)
    println(ui_theme_colors())
}
```

```output
[255, 0, 128, 40, 255, 180, 44, 62, 100, 255, 90, 90, 190, 235, 255, 10, 16, 24]
```

The output also shows the built-in defaults for the other five colours.

### Layout helpers: `ui_hot`, `ui_frame`, `ui_bevel`, `ui_panel`, `ui_scanlines`

`ui_hot` returns `true` when the mouse is inside a rectangle; use it for hover effects and your own
controls. `ui_frame` (bracket arm length, default 14) and `ui_bevel` (corner cut, default 10) draw
outlines in the pen colour. `ui_panel` draws a bevelled panel filled with the theme's bg colour
(bevel default 12; optional border colour from the sixth argument). `ui_scanlines` draws `density`
horizontal lines (default 24).

```ling-gui
bind start = do {
    ui_panel(20, 20, 340, 200, 14)
    if ui_hot(40, 140, 120, 36) { set_color(255, 255, 255) } else { set_color(120, 140, 160) }
    ui_bevel(40, 140, 120, 36, 8)
}
```

### HUD: `ui_radar`, `ui_compass`, `ui_reticle`, `ui_target`

`ui_radar` draws range rings and a sweep at angle `sweep` in radians; drive it from `time_now()`.
`ui_compass` shows a strip centred on `heading` in **degrees**, with a tick every 15 degrees.
`ui_reticle` draws a ring whose gaps open as `spread` goes from 0 to 1. `ui_target` draws brackets
that close in as `lock` goes from 0 to 1.

### Meters: `ui_bar`, `ui_segbar`, `ui_gauge`, `ui_ring`, `ui_vu`, `ui_spark`, `ui_battery`, `ui_progress`

Each meter shows `value` out of `max` (default 1). `ui_segbar` splits the bar into `segments` blocks
(default 10). `ui_gauge` is a dial with a needle; `ui_ring` fills clockwise. `ui_vu` takes a list of
levels from 0 to 1, one bar each, shading toward the warn colour as they rise. `ui_spark` scales a list
of at least two numbers to fit its box. `ui_battery` turns to the warn colour below 25 percent.
`ui_progress` takes a single `fraction` from 0 to 1.

```ling-gui
bind start = do {
    ui_bar(40, 70, 280, 14, 70, 100)
    ui_segbar(40, 96, 280, 14, 6, 10, 10)
    ui_gauge(450, 120, 50, 62, 100)
    ui_vu(330, 330, 160, 60, [0.2, 0.5, 0.8, 1.0, 0.7, 0.3])
}
```

### Controls: `ui_button`, `ui_toggle`, `ui_slider`, `ui_checkbox`, `ui_tabs`, `ui_stepper`, `ui_tooltip`

- `ui_button` highlights on hover and returns `1` on the frame it is clicked, otherwise `0`.
- `ui_toggle` and `ui_checkbox` take the current state and return the new one, `1` or `0`.
- `ui_slider` has no height argument (the grab area is 20 pixels tall). It returns the new value,
  which follows the cursor while the button is held.
- `ui_tabs` draws `count` equal tabs and returns the active index (from 0). Divide the width by
  `count` to place captions.
- `ui_stepper` returns `value` plus or minus `step` when a side is clicked. It draws no number.
- `ui_tooltip` is a callout bubble; draw it near the cursor when something is hovered.

```ling-gui
bind start = do {
    bind tab = 0
    bind count = 3
    bind tab = ui_tabs(20, 250, 300, 28, 3, tab)
    bind count = ui_stepper(160, 178, 140, 28, count, 1)
    ui_text(222, 186, 14, str(count))
}
```

### Game UI: `ui_healthbar`, `ui_cooldown`, `ui_counter`, `ui_minimap`, `ui_dpad`, `ui_slotgrid`, `ui_vignette`

`ui_healthbar` shifts from the warn colour when empty to the accent colour when full; `pulse` (0 to 1)
flashes it toward white below 30 percent. `ui_cooldown` fills a pie wedge clockwise from twelve
o'clock. `ui_counter` shows `value` as `digits` seven-segment digits. `ui_minimap` draws an empty map
frame; plot your own dots with `draw_disc`. `ui_dpad` returns `0` none, `1` up, `2` right, `3` down
or `4` left while the mouse is held on it. `ui_slotgrid` highlights slot `selected` (`-1` for none).
`ui_vignette` glows along the screen edges, for a damage flash.

```ling-gui
bind start = do {
    bind hp = 24
    ui_healthbar(20, 300, 220, 16, hp, 100, 0.5 + 0.5 * sin(time_now() * 8))
    ui_counter(20, 340, 16, 28, 1234, 5)
    ui_slotgrid(400, 260, 5, 2, 40, 1)
}
```

### Pseudo-3D: `ui_gauge3d`, `ui_panel3d`, `ui_radar3d`

`ui_gauge3d` draws a wireframe band that turns with `spin`, lit up to `value`. `ui_panel3d` lifts a
bevelled panel off the screen by `depth` pixels (default 14). `ui_radar3d` tilts the radar into the
screen (`tilt` 0 to 1, default 0.9).

### `ui_sound(name)`

Plays a short synthesised interface sound: `"click"` (also the fallback for unknown names),
`"hover"`, `"confirm"`, `"toggle"`, `"tick"` or `"error"`. It is silent if audio isn't running.

```ling-gui
bind start = do {
    if ui_button(20, 400, 120, 36) == 1 { ui_sound("confirm") }
}
```
