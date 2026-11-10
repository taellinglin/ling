# Builtin Aliases by Language

Every builtin function has at least one Thai alias and one English alias.
All aliases listed here are valid in the same source file simultaneously.

## Output

| English | Thai | Chinese | Description |
|---------|------|---------|-------------|
| `print(v)` | `พิมพ์(v)` | `印(v)` | Print value |
| `println(v)` | `พิมพ์(v)` | `印(v)` | Print with newline |
| `format(fmt, ...)` | `รูปแบบ(...)` | `格式(...)` | Format string |

## Math

| English | Thai | Description |
|---------|------|-------------|
| `sin(x)` | `ไซน์(x)` | Sine |
| `cos(x)` | `โคไซน์(x)` | Cosine |
| `tan(x)` | `แทนเจนต์(x)` | Tangent |
| `asin(x)` `arcsin(x)` | — | Arcsine |
| `acos(x)` `arccos(x)` | — | Arccosine |
| `atan(x)` `arctan(x)` | — | Arctangent |
| `atan2(y,x)` | — | 2-argument arctangent |
| `sqrt(x)` | `รากที่สอง(x)` | Square root |
| `pow(x,y)` | `ยกกำลัง(x,y)` | Power |
| `log(x)` `ln(x)` | `ลอการิทึม(x)` | Natural logarithm |
| `exp(x)` | — | e^x |
| `abs(x)` | `ค่าสัมบูรณ์(x)` | Absolute value |
| `floor(x)` | `ปัดลง(x)` | Floor |
| `ceil(x)` | `ปัดขึ้น(x)` | Ceiling |
| `round(x)` | `ปัดเศษ(x)` | Round |
| `trunc(x)` `int(x)` | `ตัดทศนิยม(x)` | Truncate to integer |
| `min(a,b)` | `ต่ำสุด(a,b)` | Minimum |
| `max(a,b)` | `สูงสุด(a,b)` | Maximum |
| `clamp(v,lo,hi)` | `จำกัด(v,lo,hi)` | Clamp to range |
| `tanh(x)` | — | Hyperbolic tangent |

## Window & display

| English | Thai | Description |
|---------|------|-------------|
| `open_window(title)` | `เปิดหน้าต่าง(title)` | Open window |
| `open_fullscreen(title)` | `เปิดหน้าต่างเต็มจอ(title)` | Open fullscreen |
| `window_is_open()` | `หน้าต่างเปิดอยู่()` | Poll window open state |
| `fill(r,g,b)` | `เติม(r,g,b)` | Clear screen |
| `display()` `present()` | `แสดงผล()` | Flush to screen |
| `get_width()` | `ความกว้าง()` | Window width in pixels |
| `get_height()` | `ความสูง()` | Window height in pixels |
| `capture_mouse()` | `จับเมาส์()` | Lock cursor |
| `wait_window()` | `รอหน้าต่าง()` | Block until window closes |
| `set_color(r,g,b)` | `สีดินสอ(r,g,b)` | Set pen colour |

## Camera

| English | Thai | Description |
|---------|------|-------------|
| `set_camera(cry,sry,crx,srx)` | `ตั้งกล้อง(...)` | Orient camera |
| `set_camera_pos(x,y,z)` | `ตั้งตำแหน่งกล้อง(x,y,z)` | Move camera |
| `set_zdist(d)` | `ตั้งระยะห่าง(d)` | Set near-plane distance |

## Lighting

| English | Thai | Description |
|---------|------|-------------|
| `add_light(x,y,z, r,g,b, intensity, radius)` | `เพิ่มแสง(...)` | Add point light |
| `clear_lights()` | `ล้างแสง()` | Remove all lights |
| `set_ambient(level)` | `ตั้งแสงรอบข้าง(level)` | Ambient light level |

## Surfaces, shadows & depth

Translucent fills, smooth gradient surfaces (cheap per-vertex "lighting"), soft
colored shadows, and a painter's-algorithm queue for depth-sorting 2-D draws.
All names in a row are aliases for the same function.

| English | Chinese | Japanese | Korean | Thai | Description |
|---------|---------|----------|--------|------|-------------|
| `set_alpha(a)` | `设透明` | `アルファ設定` | `투명도설정` | `ตั้งความโปร่งใส` | Pen opacity 0–1 for the alpha fills below |
| `grad_triangle(x0,y0,r,g,b, x1,y1,r,g,b, x2,y2,r,g,b)` | `渐变三角` | `グラデ三角` | `그라데삼각` | `สามเหลี่ยมไล่สี` | Smooth per-vertex gradient triangle (fake directional lighting) |
| `grad_rect(x,y,w,h, r0,g0,b0, r1,g1,b1, dir)` | `渐变矩形` | `グラデ矩形` | `그라데사각` | `สี่เหลี่ยมไล่สี` | Linear-gradient rectangle (`dir` 0=horizontal, 1=vertical) |
| `shadow_blob(cx,cy, rx,ry, alpha)` | `阴影斑` | `影ブロブ` | `그림자블롭` | `เงาวงรี` | Soft colored shadow ellipse in the current pen colour |
| `cast_shadow(cx,cy, height)` | `投射阴影` | `影を落とす` | `그림자드리우기` | `ทอดเงา` | Height-driven contact shadow (closer=smaller/darker, farther=bigger/softer) |
| `shadow_params(base,grow,alpha,fade,soft)` | `阴影参数` | `影設定` | `그림자설정` | `ตั้งค่าเงา` | Tune the `cast_shadow` height ramp |
| `depth_triangle(x0,y0, x1,y1, x2,y2, z)` | `深度三角` | `深度三角形` | `깊이삼각` | `สามเหลี่ยมเรียงลึก` | Queue a depth-sorted triangle (drawn back-to-front at `present`) |
| `depth_line(x0,y0, x1,y1, z)` | `深度线` | `深度線` | `깊이선` | `เส้นเรียงลึก` | Queue a depth-sorted line |

## Toon shading & post-FX

Holographic-cel shading controls, the unified tone ramp, and the screen-space
post chain (ambient occlusion → outlines → tone ramp → bloom → FXAA), plus
volumetric light volumes. All names in a row are aliases for the same function.

| English | Chinese | Japanese | Korean | Thai | Description |
|---------|---------|----------|--------|------|-------------|
| `set_shade_mode(m)` | `设置着色` | `シェード設定` | `셰이드모드` | `ตั้งการแรเงา` | Mesh shading: 0 flat · 1 cel · 2 holo (default) |
| `set_cel_bands(n)` | `设置色阶` | `セル段数` | `셀밴드` | `ตั้งระดับสี` | Posterisation band count (≥2) |
| `set_shadow_color(r,g,b)` | `设置阴影色` | `影の色` | `그림자색` | `ตั้งสีเงา` | Coloured-shadow tint for unlit regions |
| `set_rim(s, r,g,b)` | `设置边缘光` | `リム設定` | `림라이트` | `ตั้งขอบเรือง` | Fresnel rim glow strength + colour (0 = off) |
| `tone_stop(t, value)` | `色调停止` | `トーンストップ` | `톤스톱` | `ตั้งจุดโทน` | Add a tone-ramp stop (luminance → brightness) |
| `tone_smooth(on)` | `色调平滑` | `トーンスムーズ` | `톤스무스` | `ตั้งโทนนุ่ม` | 0 hard cel snap · 1 smooth gradient lerp |
| `tone_bezier(y1, y2)` | `色调贝塞尔` | `トーンベジェ` | `톤베지어` | `ตั้งโทนเบซิเยร์` | Bézier remap of input luminance (S-curves) |
| `tone_ramp_reset()` | `重置色调渐变` | `トーンランプリセット` | `톤램프리셋` | `รีเซ็ตการไล่โทน` | Restore the default 3-band cel ramp |
| `tone_ramp_clear()` | `清除色调渐变` | `トーンランプクリア` | `톤램프클리어` | `ล้างการไล่โทน` | Clear all stops (build your own ramp) |
| `tone_soft(soft, sheen)` | `色调柔边` | `トーンソフト` | `톤소프트` | `โทนขอบนุ่ม` | Soft band edges + smooth highlight sheen |
| `set_ssao(strength, radius, zrange)` | `环境光遮蔽` | `アンビエントオクルージョン` | `앰비언트오클루전` | `ตั้งเงาสัมผัส` | Ambient occlusion from the z-buffer (needs `set_depth_test(1)`) |
| `set_fxaa(on)` | `屏幕抗锯齿` | `画面アンチエイリアス` | `화면안티앨리어싱` | `ลบรอยหยัก` | Screen-space edge anti-aliasing (FXAA-lite) |
| `set_bloom(strength, threshold)` | `泛光` | `ブルーム` | `블룸` | `ตั้งบลูม` | Soft HDR-style glow from bright pixels |
| `depth_blur(focus, range, radius, oil)` `dof(...)` | `景深` | — | — | `เบลอความลึก` | Tilt-shift depth-of-field; `oil` adds an iridescent oil-slick shimmer |
| `light_pool(x,y,z, radius, r,g,b, intensity)` | `光池` | `ライトプール` | `빛웅덩이` | `แอ่งแสง` | Volumetric light splash on a floor — soft additive radial gradient |
| `light_beam(x,y,z, floor_y, radius, r,g,b, intensity)` | `光柱` | `ライトビーム` | `빛기둥` | `ลำแสงไฟ` | Volumetric god-ray cone from a light down to the floor |

## Vector geometry (vtex)

All names in the same row are valid aliases for the same function.

| English | Thai | Chinese | Japanese | Korean | Description |
|---------|------|---------|----------|--------|-------------|
| `vtex_grid` | `ลายตาราง` | `纹格` | `格子模様` | `격자무늬` | Rectilinear grid |
| `vtex_rings` | `ลายวงซ้อน` | `纹环` | `同心円` | `동심원` | Concentric rings |
| `vtex_star` | `ลายดาว` | `纹星` | `星模様` | `별무늬` | Star polygon |
| `vtex_spiral` | `ลายเกลียว` | `纹螺` | `螺旋` | `나선` | Archimedean spiral |
| `vtex_flower` | `ลายดอก` | `纹花` | `花模様` | `꽃무늬` | Flower of Life |
| `vtex_lotus` | `ลายดอกบัว` | `纹莲` | `蓮模様` | `연꽃무늬` | Lotus petals |
| `vtex_chakra` | `ลายจักร` | `纹轮` | `輪模様` | `바퀴무늬` | Dhamma wheel |
| `vtex_yantra` | `ลายยันต์` | `纹咒` | `護符模様` | `부적무늬` | Sri Yantra |
| `vtex_spiked_cog` | `ฟันเฟืองหนาม` | `纹棘轮` | `歯車模様` | `톱니바퀴` | Spiked gear |
| `vtex_torii` | `ประตูโทริอิ` | `纹鸟居` | `鳥居` | `도리이` | Torii gate |
| `vtex_pagoda` | `เจดีย์` | `纹塔` | `塔` | `탑` | Pagoda silhouette |
| `vtex_halftone` | `ลายจุด` | `纹半调` | `網点模様` | `망점` | Cross-hatch fill |
| `vtex_tessellated` | `ลายตาข่าย` | `纹镶嵌` | `網目模様` | `격자망` | Triangle mesh fill |
| `vtex_hyperbolic_uv` | `ลายไฮเพอร์โบลิก` | `纹曲面` | `双曲線` | `쌍곡선` | Hyperbolic tiling |
| `vtex_letter_rain` | `ลายอักษรไหล` | `纹字雨` | `文字雨` | `글자비` | Glyph rain |

## Pixel textures (tex)

| English | Thai | Description |
|---------|------|-------------|
| `tex_checkerboard(...)` | `ลายตารางหมากรุก(...)` | Checker pattern |
| `tex_gradient(...)` | `ลายไล่สี(...)` | Linear gradient |
| `tex_noise(...)` | `ลายนอยส์(...)` | FBM noise |
| `tex_mandelbrot(...)` | `ลายแมนเดลบรอต(...)` | Mandelbrot fractal |
| `tex_julia(...)` | `ลายจูเลีย(...)` | Julia fractal |
| `tex_voronoi(...)` | `ลายโวโรนอย(...)` | Voronoi cells |
| `tex_ripple(...)` | `ลายระลอก(...)` | Ripple / interference |
| `tex_spiral(...)` | `ลายเกลียวหมุน(...)` | Pixel spiral |
| `tex_halftone(...)` | `ลายฮาล์ฟโทน(...)` | Halftone dots |
| `tex_freq_map(...)` | `ลายความถี่(...)` | FFT frequency bars |

## Audio

| English | Thai | Description |
|---------|------|-------------|
| `audio_tone(slot,x,y,z,w, freq,amp,lfo,depth)` | `เสียงโทน(...)` | Spatial tone synth |
| `audio_volume(v)` | `ระดับเสียง(v)` | Master volume |
| `audio_bgm(path,vol)` | `เพลงพื้นหลัง(path,vol)` | Background music |
| `audio_bgm_volume(v)` | `ระดับเสียงพื้นหลัง(v)` | BGM volume |
| `audio_listener(cry,sry,crx,srx)` | `ผู้ฟัง(...)` | Listener orientation |

## FFT (native only)

| English | Thai | Description |
|---------|------|-------------|
| `fft_push(samples)` | `วิเคราะห์เสียง(samples)` | Feed samples to analyzer |
| `fft_bands(n)` | `แถบความถี่(n)` | Get n frequency bands |
| `fft_beat()` | `จังหวะเสียง()` | Beat detection |
| `fft_beat_ratio()` | `อัตราจังหวะ()` | Beat strength ratio |
| `fft_rms()` | `ระดับRMS()` | RMS level |
| `fft_dominant_freq()` | `ความถี่หลัก()` | Peak frequency in Hz |

## Lists

| English | Description |
|---------|-------------|
| `list_new()` | Create empty list |
| `list_push(lst, v)` | Append and return new list |
| `list_pop(lst)` | Remove last element |
| `list_get(lst, i)` | Get element at index |
| `list_set(lst, i, v)` | Set element at index |
| `list_len(lst)` | Number of elements |

## Strings

| English | Thai | Description |
|---------|------|-------------|
| `split(s, delim)` `str_split(s, delim)` | `แยก(s, delim)` | Split string |
| `trim(s)` `str_trim(s)` | `ตัดช่องว่าง(s)` | Trim whitespace |
| `starts_with(s, prefix)` | `เริ่มด้วย(s, prefix)` | Prefix test |
| `str_len(s)` | — | String length |
| `str_concat(a,b)` | — | Concatenate |
| `num_to_str(n)` | — | Number to string |
| `str_to_num(s)` | — | Parse number |

## File I/O

| English | Thai | Description |
|---------|------|-------------|
| `read_file(path)` | `อ่านไฟล์(path)` | Read file to string |
| `write_file(path, content)` | `เขียนไฟล์(path, content)` | Write string to file |

## Input

| English | Thai | Description |
|---------|------|-------------|
| `key_down(key)` | `กดค้าง(key)` | True while key held |
| `key_pressed(key)` | `กดปุ่ม(key)` | True on key press event |
| `mouse_dx()` | `เมาส์X()` | Mouse X delta |
| `mouse_dy()` | `เมาส์Y()` | Mouse Y delta |

## Gamepad / Joystick

Powered by the [`ling-input`](https://docs.rs/ling-input) "Sensorium" crate
(native gamepads via `gilrs`; rumble, sticks, triggers). Call `pad_poll()` once
per frame before reading state. Player index `i` is 0-based; button names accept
vendor-neutral aliases (`a`/`south`/`cross`, `b`/`east`/`circle`, …, `lb`, `rt`,
`start`, `dpad_up`/`up`, `l3`, `guide`).

| English | Chinese | Japanese | Korean | Thai | Description |
|---------|---------|----------|--------|------|-------------|
| `pad_poll()` | `手柄轮询()` | `パッド更新()` | `패드폴링()` | `อัปเดตแพด()` | Advance input one frame → # connected pads |
| `pad_count()` | `手柄数()` | `パッド数()` | `패드수()` | `จำนวนแพด()` | Number of connected gamepads |
| `pad_connected(i)` | `手柄连接(i)` | `パッド接続(i)` | `패드연결(i)` | `แพดเชื่อม(i)` | Is player `i`'s pad connected? |
| `pad_button(i,name)` | `手柄按键(i,name)` | `パッドボタン(i,name)` | `패드버튼(i,name)` | `ปุ่มแพด(i,name)` | Is the button held? |
| `pad_pressed(i,name)` | `手柄按下(i,name)` | `パッド押下(i,name)` | `패드눌림(i,name)` | `แพดกด(i,name)` | Pressed this frame (edge)? |
| `pad_lx(i)` / `pad_ly(i)` | `手柄左X(i)` / `手柄左Y(i)` | `パッド左X(i)` / `パッド左Y(i)` | `패드왼X(i)` / `패드왼Y(i)` | `แพดซ้ายX(i)` / `แพดซ้ายY(i)` | Left stick axes (−1..1) |
| `pad_rx(i)` / `pad_ry(i)` | `手柄右X(i)` / `手柄右Y(i)` | `パッド右X(i)` / `パッド右Y(i)` | `패드오X(i)` / `패드오Y(i)` | `แพดขวาX(i)` / `แพดขวาY(i)` | Right stick axes (−1..1) |
| `pad_lt(i)` / `pad_rt(i)` | `手柄左扳机(i)` / `手柄右扳机(i)` | `パッド左トリガー(i)` / `パッド右トリガー(i)` | `패드왼트리거(i)` / `패드오트리거(i)` | `ไกแพดซ้าย(i)` / `ไกแพดขวา(i)` | Analog triggers (0..1) |
| `pad_rumble(i,lo,hi)` | `手柄震动(i,lo,hi)` | `パッド振動(i,lo,hi)` | `패드진동(i,lo,hi)` | `แพดสั่น(i,lo,hi)` | Set rumble motor amplitudes (0..1) |

```ling
bind start = do {
    open_window(640, 480, "pad test")
    while true {
        pad_poll()
        if pad_pressed(0, "a") {
            print("jump!")
        }
        bind mx = pad_lx(0)
        // ... move player by mx, my ...
        gfx_present()
    }
}
```
