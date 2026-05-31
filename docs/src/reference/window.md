# Window & Camera Builtins

## Window management

| Function | Thai alias | Description |
|----------|-----------|-------------|
| `open_window(title)` | `เปิดหน้าต่าง(title)` | Open a window |
| `open_fullscreen(title)` | `เปิดหน้าต่างเต็มจอ(title)` | Open fullscreen window |
| `window_open()` | `หน้าต่างเปิดอยู่()` | Returns true while window is open |
| `get_width()` | — | Current window width in pixels |
| `get_height()` | — | Current window height in pixels |
| `fill(r,g,b)` | `เติม(r,g,b)` | Clear screen to RGB colour |
| `display()` | `แสดงผล()` | Flush depth queue → screen |
| `capture_mouse()` | — | Lock mouse cursor to window |

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
