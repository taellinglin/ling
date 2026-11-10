# Window and Camera

Every graphical Ling program opens a window, runs a frame loop, and aims a single 3D camera.
This page covers those three things: the window and its frame loop, frame pacing and timing,
and the camera that projects 3D points onto the screen. Drawing is on the next pages:
[2D Drawing and Images](drawing.md) and [3D Shapes and Meshes](shapes.md).

## The frame loop

A graphics program always has the same shape. Open a window, then repeat until it closes:
clear, draw, `present()`.

```ling-gui
bind start = do {
    open_window(800, 600, "Frame loop")
    bind frame = 0
    while window_is_open() {
        fill(10, 12, 30)                  # 1. clear to a background colour
        set_color(255, 200, 80)           # 2. draw
        draw_disc(400 + 200 * sin(frame * 0.05), 300, 40)
        present()                         # 3. show the frame
        bind frame = frame + 1
    }
}
```

Three facts are worth remembering:

1. `window_is_open()` turns false when the user closes the window **or presses Escape**.
   You don't need to handle Escape yourself.
2. `present()` is the only call that puts pixels on screen. It also draws the queued 3D
   geometry, runs post-processing and samples the mouse. A frame without `present()` is invisible.
3. There is no delta-time builtin. Subtract the previous `time_now()` from the current one,
   or count frames with `frame_count()`.

Calling a builtin that doesn't exist (for example `draw_rect`, which Ling doesn't have) is
silently ignored. If a drawing call seems to do nothing, check its spelling first.

### Coordinates

| Space | Origin | +x | +y | +z |
|---|---|---|---|---|
| Screen (2D) | top-left pixel | right | **down** | not used |
| World (3D) | camera pivot | right | **down** | away from the camera |

Because +y points down, "above" is negative y: a ceiling sits at `y = -3` and a floor at `y = 3`.

The default projection centres the view on the window, uses a focal length equal to the window
height in pixels, and adds a depth offset `zdist = 5` before the perspective divide:

```text
screen_x = width/2  + height * x / (z + zdist)
screen_y = height/2 + height * y / (z + zdist)
```

## Summary: window and frame

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `open_window(w, h, title, resizable)` | `开窗` | `ウィンドウ開く` | `창열기` | `เปิดหน้าต่าง` | Open a window |
| `open_fullscreen(title, w, h)` | `全屏` | `全画面` | `전체화면` | `เปิดหน้าต่างเต็มจอ` | Borderless full-screen window |
| `open_fullscreen_scaled(title, scale)` | — | — | — | — | Full screen at 1/scale resolution |
| `window_is_open()` | `窗开` | `開いている` | `창열림` | `หน้าต่างเปิดอยู่` | Loop condition |
| `present()` | `显` | `表示` | `표시` | `แสดงผล` | Finish and show the frame |
| `flush_3d()`, `flush_post()` | — | — | — | — | Draw queued 3D now, mid-frame |
| `quit()` | — | — | — | — | Close the window from code |
| `get_width()`, `get_height()` | `宽`, `高` | `幅取得`, `高取得` | `너비`, `높이` | `ความกว้าง`, `ความสูง` | Framebuffer size |
| `set_fps(n)` | `设帧率` | `フレームレート設定` | `프레임설정` | `ตั้งเฟรมเรต` | Cap the frame rate |
| `set_vsync(on)` | `垂直同步` | `垂直同期` | `수직동기` | `ตั้งวีซิงก์` | Pace to the monitor refresh |
| `monitor_width()`, `monitor_height()` | `屏宽`, `屏高` | `画面幅`, `画面高` | `화면너비`, `화면높이` | `ความกว้างจอ`, `ความสูงจอ` | Monitor size |
| `monitor_refresh()` | `刷新率` | `リフレッシュレート` | `주사율` | `อัตรารีเฟรช` | Monitor refresh rate in Hz |
| `monitor_info()` | `屏幕信息` | `画面情報` | `화면정보` | `ข้อมูลจอ` | `[width, height, hz]` |
| `wait_window()` | — | — | — | — | Keep the last frame on screen |
| `screenshot(tag)` | — | — | — | — | Save the frame as PNG |
| `record_frame()`, `record_count()` | — | — | — | — | Save numbered frames for a video |
| `frame_count()` | `帧数` | `フレーム数` | `프레임수` | `เฟรม` | Frames presented so far |
| `time_now()` | `当前时间` | `経過時間` | `현재시간` | `เวลาปัจจุบัน` | Seconds since the program started |

## Window and frame

### `open_window(w, h, title, resizable)`

Opens the program's window and allocates its framebuffer.

- **Parameters:** `w`, `h` size in pixels (default 800 by 600); `title` (default `"Ling"`);
  `resizable` optional, `1` makes the window resizable (default 0).
- **Returns:** nothing.
- **Note:** the numbers come first and the title third. Opening a window also resets the camera
  projection to its defaults. With a resizable window, read `get_width()` and `get_height()`
  every frame. If the window can't be created, the program stops with `cannot open window`.

```ling-gui
bind start = do {
    open_window(1024, 640, "My game", 1)
}
```

### `open_fullscreen(title, w, h)` and `open_fullscreen_scaled(title, scale)`

`open_fullscreen` opens a borderless window that covers the primary monitor. `w` and `h` are
optional (default: the monitor's resolution). Note that the **title comes first** here.

`open_fullscreen_scaled` renders at a fraction of the monitor resolution and scales the result
up, trading sharpness for speed. `scale` is `1`, `2` or `4` (anything else counts as 1);
`get_width()` and `get_height()` report the reduced size.

```ling-gui
bind start = do {
    open_fullscreen_scaled("Retro", 2)
}
```

### `window_is_open()`

`true` while the window is open; `false` after the user closes it, presses Escape, or your code
calls `quit()`. Before any window is opened it returns `false`, so a loop guarded by it doesn't run.

### `present()`

Ends the frame: draws every queued 3D primitive, applies post-processing, copies the framebuffer
to the window and reads the mouse.

3D shapes (`cube`, `vtex_*`, `draw_line_3d` and the rest) are depth-sorted and only rasterised
here, so you can issue them in any order. Direct 2D calls (`draw_line`, `triangle`, `draw_disc`,
`tex_*`, `image_draw`) write to the framebuffer immediately, so queued 3D geometry ends up
**on top of** them. To draw a HUD over a 3D scene, call `flush_3d()` first. The `ui_*` widgets
detect clicks across frames, so they need `present()` once per frame.

### `flush_3d()` and `flush_post()`

`flush_3d()` draws the queued 3D geometry into the framebuffer now, without presenting. Anything
you draw afterwards lands on top. `flush_post()` does the same and then runs the post-processing
chain immediately, so a HUD drawn after it isn't blurred by bloom. See
[Post-FX and Lighting](postfx.md).

```ling-gui
bind start = do {
    cube(0, 0, 8, 1, 1, 1, 0, 0, 0, 0)
    flush_3d()                          # the cube is now in the framebuffer
    ui_bar(20, 20, 200, 14, 0.5, 1)     # drawn over it
    present()
}
```

### `quit()`

Closes the window from your code, exactly like pressing Escape. `exit_game` and `close_window`
are English synonyms.

```ling-gui
bind start = do {
    if ui_button(20, 20, 100, 32) == 1 { quit() }
}
```

### `get_width()` and `get_height()`

The current framebuffer size in pixels. For a fixed window these equal the `open_window`
arguments; for a resizable one they change when the user resizes it. Without a window they return 0.

```ling-gui
bind start = do {
    bind cx = get_width() / 2
    bind cy = get_height() / 2
}
```

### `set_fps(n)` and `set_vsync(on)`

`set_fps(n)` caps the frame rate (default 60, minimum 1). Call it after `open_window`.
`set_vsync(on)` paces the loop to the monitor's refresh rate instead. It is frame pacing, not
tear-free vertical sync. An explicit `set_fps`, or the environment variable `LING_FPS_CAP`, wins.

```ling-gui
bind start = do {
    open_window(800, 600, "Capped")
    set_fps(30)
}
```

### `monitor_width()`, `monitor_height()`, `monitor_refresh()`, `monitor_info()`

The primary monitor's width and height in pixels and its refresh rate in hertz.
`monitor_info()` returns all three as a list `[width, height, hz]`. You can call them before
opening a window, for example to size it.

```ling-gui
bind start = do {
    bind info = monitor_info()
    open_window(info[0] / 2, info[1] / 2, "Half the screen")
}
```

### `wait_window()`

Blocks, keeping the last frame on screen, until the window is closed or Escape is pressed.
Handy for a program that draws one still picture.

```ling-gui
bind start = do {
    open_window(400, 300, "Still")
    fill(20, 40, 80)
    present()
    wait_window()
}
```

### `screenshot(tag)`

Saves the current framebuffer as a PNG and returns its path,
`screenshots/ss_<unix-time>_<tag>_<width>x<height>.png`. `tag` defaults to `"game"`; characters
other than letters and digits become underscores. The folder is created where the program runs.
Call it after `present()` to capture a complete frame.

```ling-gui
bind start = do {
    present()
    println(screenshot("title-screen"))
}
```

### `record_frame()` and `record_count()`

`record_frame()` writes the current framebuffer to `recordings/frame_00000.ppm`,
`frame_00001.ppm` and so on, for assembling into a video later, and returns the index of the frame
it wrote. `record_count()` returns how many frames have been written.

### `frame_count()` and `time_now()`

`frame_count()` is the number of frames presented since the program started. `time_now()` is the
number of seconds since the program started, as a fraction. For a frame-time delta, remember the
previous value:

```ling-gui
bind start = do {
    bind last = time_now()
    while window_is_open() {
        bind now = time_now()
        bind dt = now - last
        bind last = now
        fill(0, 0, 0)
        present()
    }
}
```

## Camera

All 3D builtins project world points through one global camera, defined by three things:

* the **pivot** `(x, y, z)`, set with `set_camera_pos`, which is subtracted from every point;
* the **orientation**, set with `set_camera` as the cosine and sine of a yaw and a pitch angle;
* the **depth offset** `zdist`, set with `set_zdist`, added before the perspective divide.

A positive yaw turns the camera to its right, so things straight ahead slide left on screen.
A positive pitch tilts it down, so things ahead slide up. For a yaw angle `a`, "forward" in the
world is `(sin a, cos a)` in the x-z plane. Set the camera every frame, before drawing 3D geometry.

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `set_camera(cy, sy, cp, sp)` | `设镜` | `カメラ設定` | `카메라설정` | `ตั้งกล้อง` | Orientation from cos/sin of yaw and pitch |
| `set_camera_pos(x, y, z)` | `镜坐标` | `カメラ座標` | `카메라좌표` | `ตั้งตำแหน่งกล้อง` | Move the pivot |
| `move_camera(dx, dy, dz)` | — | — | — | — | Move the pivot by an offset |
| `set_zdist(d)` | `镜距` | `Z距離設定` | `Z거리설정` | `ตั้งระยะห่าง` | Depth offset |
| `set_projection(cx, cy, focal, zdist)` | `投影` | `投影設定` | `투영설정` | `ตั้งโปรเจกชัน` | All lens parameters at once |
| `tm_fov(degrees, width)` | — | — | — | — | Set the focal length from a field of view |
| `set_camera_roll(angle)` | — | — | — | — | Roll about the view axis |
| `set_camera_hyper(R, saddle, px, mid_y, pz)` | — | — | — | — | Hyperbolic-space view |
| `project_3d(x, y, z)` | `投影3D` | `3D投影` | `3D투영` | `ฉาย3มิติ` | World point to screen |
| `mouse_ray()` | — | — | — | — | Screen (mouse) to world ray |

### `set_camera(cy, sy, cp, sp)`

Sets the orientation from precomputed values: `cy`, `sy` are the cosine and sine of the yaw angle,
`cp`, `sp` the cosine and sine of the pitch angle (defaults `1, 0, 1, 0`, looking straight along +z).
Pass the four numbers, not the angles.

```ling-gui
bind start = do {
    bind yaw = 0.3
    bind pitch = 0.1
    set_camera(cos(yaw), sin(yaw), cos(pitch), sin(pitch))
}
```

### `set_camera_pos(x, y, z)` and `move_camera(dx, dy, dz)`

`set_camera_pos` moves the camera pivot to a world position (default 0, 0, 0); `move_camera` adds
an offset to it. Remember y is down: to raise the camera, decrease y. When audio is running, the
audio listener follows the camera position.

```ling-gui
bind start = do {
    set_camera_pos(0, -2, -6)      # two units up, six back
}
```

### `set_zdist(d)`, `set_projection(cx, cy, focal, zdist)` and `tm_fov(degrees, width)`

`set_zdist(d)` sets the depth offset (default 5, reset when a window opens). A larger value pushes
the scene away and flattens the perspective; geometry nearer than `-zdist` is clipped.

`set_projection` overrides every lens parameter: `cx`, `cy` the optical centre in pixels, `focal`
the focal length in pixels (larger zooms in), and `zdist`. You need it only for unusual setups such
as split screen.

`tm_fov(degrees, width)` sets the focal length so the view spans a horizontal angle; `width` is the
viewport width in pixels (default: the window width). `tm_fov(0)` restores the default focal length.

```ling-gui
bind start = do {
    set_projection(get_width() / 2, get_height() / 2, get_height() * 1.5, 5)   # zoomed in
}
```

### `set_camera_roll(angle)` and `set_camera_hyper(R, saddle, px, mid_y, pz)`

`set_camera_roll` rolls the view about its forward axis, in radians (alias `camera_roll`).
`set_camera_hyper` switches on a hyperbolic view around the pivot `(px, mid_y, pz)`: ground
distances are compressed as `R * tanh(d / R)` and space bends like a saddle of strength `saddle`.
`R = 0` turns it off.

### `project_3d(x, y, z)`

Projects a world point with the current camera and returns `[screen_x, screen_y, depth]`. A point
behind the camera gives `[-99999, -99999, depth]`. Use it to pin 2D labels to 3D objects.
Without a window the projection assumes a 1920 by 1080 frame, so this runs in a terminal:

```ling
bind start = do {
    set_camera(1, 0, 1, 0)
    println("ahead:   " + str(project_3d(0, 0, 5)))
    println("right:   " + str(project_3d(1, 0, 5)))
    set_camera_pos(2, 0, 0)
    println("moved:   " + str(project_3d(0, 0, 5)))
    set_camera_pos(0, 0, 0)
    println("behind:  " + str(project_3d(0, 0, -20)))
}
```

```output
ahead:   [960, 540, 5]
right:   [1068, 540, 5]
moved:   [744, 540, 5]
behind:  [-99999, -99999, -20]
```

One unit to the right at depth 10 (5 plus `zdist`) moves 108 pixels with a focal length of 1080.

The ja and ko aliases `3D投影` and `3D투영` start with a digit, so they are read as the number 3
and can't be called. Use `project_3d` in Japanese and Korean code.

### `mouse_ray()`

The world-space ray from the camera pivot through the pixel under the mouse, as a list
`[ox, oy, oz, dx, dy, dz]`: an origin and a direction. It is the inverse of `project_3d`.
Step along it to find the ground under the cursor for click-to-move.

```ling-gui
bind start = do {
    bind ray = mouse_ray()
    bind t = (2 - ray[1]) / ray[4]          # where the ray meets the floor at y = 2
    bind gx = ray[0] + ray[3] * t
    bind gz = ray[2] + ray[5] * t
}
```

## A free-fly camera

Mouse look, WASD walking and a spinning cube. Moving along `(sin yaw, cos yaw)` keeps walking
aligned with the view. Mouse and keyboard builtins are on [Input and Microphone](input.md);
lights on [Post-FX and Lighting](postfx.md).

```ling-gui
bind start = do {
    open_window(900, 600, "Fly")
    capture_mouse()
    set_depth_test(1)
    bind yaw = 0.0
    bind pitch = 0.0
    bind px = 0.0
    bind pz = -4.0
    bind t = 0.0
    while window_is_open() {
        bind yaw = yaw + mouse_dx() * 0.003
        bind pitch = clamp(pitch + mouse_dy() * 0.003, -1.5, 1.5)
        bind fx = sin(yaw) * 0.08
        bind fz = cos(yaw) * 0.08
        if key_down("w") { bind px = px + fx  bind pz = pz + fz }
        if key_down("s") { bind px = px - fx  bind pz = pz - fz }
        if key_down("d") { bind px = px + fz  bind pz = pz - fx }
        if key_down("a") { bind px = px - fz  bind pz = pz + fx }
        set_camera(cos(yaw), sin(yaw), cos(pitch), sin(pitch))
        set_camera_pos(px, 0, pz)
        fill(8, 10, 24)
        clear_lights()
        add_light(-3, -6, 0, 1.0, 0.95, 0.85, 1.0, 0)
        vtex_grid(0, 2, 8, 1, 0, 0, 0, 0, 1, 20, 20, 1, 1, t, 0)
        set_color(240, 150, 110)
        cube(0, 0, 8, 1, 1, 1, t * 0.5, t, 0, 0)
        present()
        bind t = t + 0.02
    }
}
```
