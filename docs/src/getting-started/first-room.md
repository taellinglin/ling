# Your First 3D Room

Time to leave the terminal. On this page you open a window, learn the frame loop
that drives every Ling picture, and build a small 3D room you can walk around with
the arrow keys. Ling draws everything with its built-in software renderer, so you
don't need a game engine or a GPU.

Programs that open a window can't be checked in the terminal, so their code blocks
have no output. Press **Escape** or close the window to stop any of them.

## The frame loop

A window shows one still picture (a frame) at a time. Your program draws a frame,
shows it, and starts the next one. Every graphical program has the same rhythm:

1. **Open** the window, once.
2. **Clear** it with `fill`, so the last frame doesn't linger.
3. **Draw** your shapes.
4. **Show** the frame with `present()`.

The loop runs `while window_is_open()`. That becomes false when the window is closed
or Escape is pressed, so you don't need to write your own exit.

```ling-gui
bind start = do {
    open_window(640, 400, "My first window")
    bind angle = 0
    while window_is_open() {
        fill(15, 18, 40)
        set_color(255, 120, 60)
        cube(0, 0, 3, 1, 1, 1, angle, angle * 0.7, 0, 0)
        present()
        bind angle = angle + 0.02
    }
}
```

- `open_window(640, 400, "My first window")` opens a 640 x 400 window. Size first,
  title last.
- `fill(15, 18, 40)` paints the whole window. Colours are red, green and blue from 0
  to 255.
- `set_color(...)` picks the colour for the shapes that follow.
- `cube(...)` draws a cube. The arguments come in groups of three: centre `x, y, z`,
  size, rotation in radians, and a final mode (`0` solid, `1` wireframe, `2` both).
- `present()` shows the frame. Nothing appears without it.
- Rebinding `angle` each frame is what makes the cube turn.

## Where things are

In Ling's 3D world, **x** points right, **y** points **down**, and **z** points away
from you into the screen. So "up" is negative y: in the room below, the floor is at
`y = 3` and the ceiling at `y = -3`.

The camera sits at the origin and looks along z. Three calls change what it sees:

| Call | What it does |
|---|---|
| `set_camera_pos(x, y, z)` | Moves the camera. |
| `set_camera(cos_yaw, sin_yaw, cos_pitch, sin_pitch)` | Turns it. You pass the cosine and sine of each angle, so to turn by `a`, call `set_camera(cos(a), sin(a), 1, 0)`. |
| `set_zdist(d)` | How far your eye sits behind the picture. A small value such as `0.5` suits a first-person view. |

Call the camera functions every frame, before you draw.

## A room you can walk around

This program puts it all together: a floor and ceiling made of grids, four walls,
four pillars, a light, a spinning centrepiece, and keyboard control. Save it as
`room.ling`.

```ling-gui
# A room you can walk around in.
# Left/right arrows turn, up/down arrows walk. Escape quits.

fn pillar(x, z) {
    set_color(200, 190, 170)
    cylinder(x, 0, z, 0.5, 3, 0.5, 0, 0, 0, 0)
}

fn walls(t) {
    # floor and ceiling (y points down, so the floor is at y = 3)
    vtex_grid(0, 3, 0,   1, 0, 0,  0, 0, 1,   16, 16, 1, 1,  t, 0)
    vtex_grid(0, -3, 0,  1, 0, 0,  0, 0, 1,   16, 16, 1, 1,  t, 2)
    # four upright walls
    vtex_grid(0, 0, 8,   1, 0, 0,  0, -1, 0,  16, 6, 1, 1,  t, 1)
    vtex_grid(0, 0, -8,  1, 0, 0,  0, -1, 0,  16, 6, 1, 1,  t, 1)
    vtex_grid(-8, 0, 0,  0, 0, 1,  0, -1, 0,  16, 6, 1, 1,  t, 3)
    vtex_grid(8, 0, 0,   0, 0, 1,  0, -1, 0,  16, 6, 1, 1,  t, 3)
    # a flower on the far wall
    vtex_flower(0, 0, 7.9,  1, 0, 0,  0, -1, 0,  1.6, 24,  t, 0)
}

bind start = do {
    open_window(960, 600, "My 3D room")
    set_shade_mode(1)
    set_rim(0, 0, 0, 0)
    set_shadow_color(30, 30, 50)
    set_ambient(0.3)

    bind px = 0
    bind pz = -5
    bind yaw = 0
    bind t = 0

    while window_is_open() {
        # input: turn and walk
        if key_down("left") {
            bind yaw = yaw - 0.03
        }
        if key_down("right") {
            bind yaw = yaw + 0.03
        }
        if key_down("up") {
            bind px = px + sin(yaw) * 0.1
            bind pz = pz + cos(yaw) * 0.1
        }
        if key_down("down") {
            bind px = px - sin(yaw) * 0.1
            bind pz = pz - cos(yaw) * 0.1
        }

        # draw
        fill(8, 8, 20)
        set_camera(cos(yaw), sin(yaw), 1, 0)
        set_camera_pos(px, 0, pz)
        set_zdist(0.5)
        clear_lights()
        add_light(0, -2.5, 0, 1.0, 0.85, 0.6, 1.0, 14)

        walls(t * 0.3)
        pillar(-6, -6)
        pillar(6, -6)
        pillar(-6, 6)
        pillar(6, 6)

        set_color(240, 200, 80)
        torus(0, 1.5, 0, 0.7, 0.7, 0.7, 1.2, t * 0.03, 0)

        present()
        bind t = t + 1
    }
}
```

Run it:

```sh
ling run room.ling
```

How it works:

- `pillar` and `walls` are functions that draw parts of the room. They are called
  inside the loop, so they draw again every frame. `walls` gets `t` as a parameter
  because functions only see their parameters and top-level names.
- The four lines after `open_window` switch to calm, faithful colours. Without them,
  Ling's default look tints shadows indigo and adds a glowing rim.
- `px`, `pz` and `yaw` hold the camera's position and heading. `key_down("left")` is
  true while the key is held. Walking moves by `sin(yaw)` along x and `cos(yaw)` along
  z, the direction you are facing.
- `clear_lights()` and `add_light(x, y, z, r, g, b, intensity, radius)` run every
  frame, because lights add up otherwise. The lamp is at `y = -2.5`, near the ceiling.
- `vtex_grid` and `vtex_flower` draw line patterns on a flat sheet. The first three
  numbers are the centre, the next six are the sheet's "right" and "up" directions.
  For upright walls, "up" is `0, -1, 0`, because y points down.

The walls are only drawn, not solid: you can walk straight through them.

## Try it

- Change the last number of each `vtex_grid` call (the hue) to get other colours.
- Add a fifth pillar, or a second `add_light` in a different colour.
- Add `if key_pressed("p") { screenshot("room") }` after `present()`. Pressing P saves
  a PNG in a `screenshots` folder.

## Next steps

- [Window and Camera](../reference/window.md) and [3D Shapes and Meshes](../reference/shapes.md)
  list every drawing builtin.
- [Drawing and Games](../tutorials/drawing-games.md) goes further with drawing and simple games.
- Chapter 6 of *Learn Ling* in the [Ling Book](../books.md) covers this room in more depth.
