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
