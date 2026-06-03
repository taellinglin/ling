# Window & Camera Builtins

Every builtin below is callable in all five languages — the aliases listed are
accepted by the interpreter **and** produced by `lingfu normalize`, so a program
written (or normalized) in any language runs unchanged.

## Window · 2-D drawing

| EN | 🇨🇳 ZH | 🇯🇵 JA | 🇰🇷 KO | 🇹🇭 TH | Description |
|----|--------|--------|--------|--------|-------------|
| `open_window` | `开窗` | `ウィンドウ開く` | `창열기` | `เปิดหน้าต่าง` | Open a window |
| `open_fullscreen` | `全屏` | `全画面` | `전체화면` | `เปิดหน้าต่างเต็มจอ` | Open fullscreen window |
| `window_is_open` | `窗开` | `開いている` | `창열림` | `หน้าต่างเปิดอยู่` | True while window is open |
| `get_width` | `宽` | `幅取得` | `너비` | `ความกว้าง` | Window width (px) |
| `get_height` | `高` | `高取得` | `높이` | `ความสูง` | Window height (px) |
| `fill` / `clear` | `填` / `清` | `塗り潰し` / `消去` | `채우기` / `지우기` | `เติม` | Clear screen to RGB |
| `set_color` | `设色` | `色設定` | `색설정` | `สีดินสอ` | Set pen colour |
| `set_color_hsl` | `色相` | `HSL色` | `HSL색설정` | `สีHSLวาด` | Set pen colour via HSL |
| `draw_line` | `画线` | `線描く` | `선그리기` | `วาดเส้น` | Draw a 2-D line |
| `draw_pixel` | `画点` | `点描く` | `점그리기` | `วาดจุด` | Plot a pixel |
| `triangle` | `画三角` | `三角形描画` | `삼각형그리기` | `วาดสามเหลี่ยม` | Fill a 2-D triangle |
| `present` | `显` | `表示` | `표시` | `แสดงผล` | Flush depth queue → screen |
| `capture_mouse` | `捕鼠` | `マウス捕捉` | `마우스잡기` | `จับเมาส์` | Lock mouse to window |

See also [`draw_circle` / `draw_disc` / `set_blend`](builtins.md) (procedural set).

## Input

| EN | 🇨🇳 ZH | 🇯🇵 JA | 🇰🇷 KO | 🇹🇭 TH | Description |
|----|--------|--------|--------|--------|-------------|
| `key_down` | `按键` | `キー押す` | `키누름` | `กดค้าง` | True while key held |
| `key_pressed` | `键按` | `キー押した` | `키눌림` | `กดปุ่ม` | True on key-press edge |
| `mouse_dx` | `鼠ΔX` | `マウスΔX` | `마우스ΔX` | `เมาส์X` | Mouse delta X |
| `mouse_dy` | `鼠ΔY` | `マウスΔY` | `마우스ΔY` | `เมาส์Y` | Mouse delta Y |

## Camera

| EN | 🇨🇳 ZH | 🇯🇵 JA | 🇰🇷 KO | 🇹🇭 TH | Description |
|----|--------|--------|--------|--------|-------------|
| `set_camera` | `设镜` | `カメラ設定` | `카메라설정` | `ตั้งกล้อง` | Orient camera (yaw/pitch) |
| `set_camera_pos` | `镜坐标` | `カメラ座標` | `카메라좌표` | `ตั้งตำแหน่งกล้อง` | Position camera |
| `set_zdist` | `镜距` | `Z距離設定` | `Z거리설정` | `ตั้งระยะห่าง` | Near-plane distance |
| `set_projection` | `投影` | `投影設定` | `투영설정` | `ตั้งโปรเจกชัน` | Set projection mode |

## Lighting

| EN | 🇨🇳 ZH | 🇯🇵 JA | 🇰🇷 KO | 🇹🇭 TH | Description |
|----|--------|--------|--------|--------|-------------|
| `set_ambient` | `环境光` | `環境光設定` | `환경광설정` | `ตั้งแสงรอบข้าง` | Ambient level 0–1 |
| `add_light` | `加灯` | `ライト追加` | `조명추가` | `เพิ่มแสง` | Add a point light |
| `clear_lights` | `清灯` | `ライト消去` | `조명초기화` | `ล้างแสง` | Remove all lights |

```ling
# Orient camera by yaw (ry) and pitch (rx) angles
set_camera(cos(ry), sin(ry), cos(rx), sin(rx))
set_camera_pos(x, y, z)
set_zdist(d)
```

## Camera

```ling
# Orient camera by yaw (ry) and pitch (rx) angles
set_camera(cos(ry), sin(ry), cos(rx), sin(rx))

# Position camera in world space
set_camera_pos(x, y, z)

# Set near-plane distance (default 2.0)
set_zdist(d)
```

## Lighting

```ling
# Remove all lights
clear_lights()

# Add a point light
# add_light(x, y, z,  r, g, b,  intensity, radius)
add_light(0, 4, 8,  1.0, 0.85, 0.3,  2.5, 16.0)

# Ambient level (0.0–1.0)
set_ambient(0.1)
```

## Pen colour (for 2D drawing)

```ling
# Set pen colour for subsequent 2D draws
สีดินสอ(r, g, b)   # or: pen_color(r,g,b)
```

## Typical main loop structure

```ling
令 启 = 执 {
    เปิดหน้าต่างเต็มจอ("My Scene")
    capture_mouse()
    set_ambient(0.10)

    令 帧 = 0
    循 หน้าต่างเปิดอยู่() {
        เติม(0, 0, 0)          # clear to black

        # update camera
        set_camera(cos(ry), sin(ry), cos(rx), sin(rx))
        set_camera_pos(cx, cy, cz)
        set_zdist(2.0)

        # draw geometry
        clear_lights()
        add_light(...)
        vtex_grid(...)
        vtex_rings(...)

        แสดงผล()               # flush
        令 帧 = 帧 + 1
    }
}
```
