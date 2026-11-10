# Drawing and Games

On this page you draw flat 2D pictures with circles, triangles, lines and gradients,
make them react to the keyboard and the mouse, and put it all together into a small
game you can play and change. Everything here uses builtins that come with Ling, so
you don't need any image files or a game engine.

These programs open a window, so their code blocks have no output. Press **Escape**
or close the window to stop any of them.

## Open a window and run the frame loop

Every graphical program has the same shape: open a window once, then repeat
**clear, draw, present** until the window closes.

```ling-gui
bind start = do {
    open_window(640, 400, "Frame loop")
    bind t = 0
    while window_is_open() {
        fill(10, 12, 30)                         # 1. clear
        set_color(255, 200, 80)                  # 2. draw
        draw_disc(320 + 200 * sin(t * 0.05), 200, 30)
        present()                                # 3. show the frame
        bind t = t + 1
    }
}
```

- `open_window(w, h, title)` opens the window. Size first, title last.
- `window_is_open()` becomes false when the window is closed or Escape is pressed.
- `fill(r, g, b)` paints the whole window. Colours are red, green and blue from 0 to 255.
- `present()` puts the finished frame on screen. A frame without `present()` is invisible.
- `t` counts frames. `sin(t * 0.05)` swings smoothly between -1 and 1, which moves the disc
  back and forth.

There is no delta-time builtin. If you need real seconds, use `time_now()` (seconds since
the program started) or `frame_count()`.

## The 2D canvas

The window is a grid of pixels. Two things differ from maths class:

- The origin `(0, 0)` is the **top-left** corner.
- **y grows downward.** `y = 300` is lower on the screen than `y = 100`.

You draw with a **pen**. `set_color(r, g, b)` picks the pen colour, and every shape after
it uses that colour until you change it.

| Builtin | Draws |
|---|---|
| `fill(r, g, b)` | the whole window in one colour |
| `draw_disc(cx, cy, r)` | a filled circle |
| `draw_circle(cx, cy, r)` | a circle outline |
| `triangle(x0, y0, x1, y1, x2, y2)` | a filled triangle |
| `draw_line(x0, y0, x1, y1)` | a thin line |
| `draw_arc(cx, cy, r, a0, a1, segments)` | part of a circle outline; angles in radians, clockwise |
| `draw_poly([x0, y0, x1, y1, ...])` | a filled polygon from a flat list of points |
| `grad_rect(x, y, w, h, r0, g0, b0, r1, g1, b1, dir)` | a rectangle fading between two colours; `dir` 1 is top to bottom, 0 is left to right |
| `set_color_hsl(h, s, l)` | pen colour by hue 0-360, saturation 0-100, lightness 0-100 |

There is no plain filled-rectangle builtin. A gradient from a colour to the same colour is
a flat rectangle, so write your own helper:

```ling-module
fn rect(x, y, w, h, r, g, b) {
    grad_rect(x, y, w, h, r, g, b, r, g, b, 1)
}
```

> **Tip:** If a drawing call seems to do nothing, check its spelling. An unknown name
> such as `draw_rect` is silently ignored.

### A landscape

Draw in layers, from back to front. Later shapes paint over earlier ones.

```ling-gui
fn rect(x, y, w, h, r, g, b) {
    grad_rect(x, y, w, h, r, g, b, r, g, b, 1)
}

fn tree(x, ground) {
    rect(x - 6, ground - 50, 12, 50, 110, 70, 40)
    set_color(40, 150, 70)
    draw_disc(x, ground - 70, 32)
    draw_disc(x - 20, ground - 50, 24)
    draw_disc(x + 20, ground - 50, 24)
}

bind start = do {
    open_window(800, 500, "Landscape")
    bind t = 0
    while window_is_open() {
        grad_rect(0, 0, 800, 380, 30, 40, 110, 250, 150, 140, 1)   # sky
        set_color(255, 220, 120)
        draw_disc(600, 300, 60)                                     # sun
        set_color(70, 60, 110)
        triangle(0, 380, 150, 190, 300, 380)                        # mountains
        triangle(200, 380, 380, 150, 560, 380)
        rect(0, 380, 800, 120, 50, 110, 60)                         # ground
        tree(120, 440)
        tree(330, 460)
        tree(690, 450)
        set_color(20, 20, 30)                                       # a bird
        bind flap = sin(t * 0.15) * 6
        draw_arc(150, 120 - flap, 18, 3.4, 6.0, 16)
        draw_arc(186, 120 - flap, 18, 3.4, 6.0, 16)
        present()
        bind t = t + 1
    }
}
```

`tree(x, ground)` places the trunk and leaves relative to its arguments, so you can plant
as many trees as you like. The bird's wings are two arcs. Because y points down, angles run
clockwise: `0` points right and `3.14` points left.

## Read the keyboard

Ling has no event handlers. Once per frame you **ask** whether a key is down:

- `key_down("left")` is true for as long as the key is held.
- `key_pressed("space")` is true only on the one frame when the key goes down. Use it
  for things that happen once per press, such as jumping or restarting.

Keys are named in lower case: `"a"` to `"z"`, digits, `"left"`, `"right"`, `"up"`,
`"down"`, `"space"`, `"enter"`, `"escape"`, `"lshift"` and more. An unknown name is
just false.

```ling-gui
bind start = do {
    open_window(640, 400, "Move the ball")
    bind x = 320
    bind y = 200
    while window_is_open() {
        bind speed = 3
        if key_down("lshift") {
            bind speed = 8
        }
        if key_down("left") {
            bind x = x - speed
        }
        if key_down("right") {
            bind x = x + speed
        }
        if key_down("up") {
            bind y = y - speed
        }
        if key_down("down") {
            bind y = y + speed
        }
        bind x = clamp(x, 20, 620)
        bind y = clamp(y, 20, 380)

        fill(10, 10, 30)
        set_color(255, 180, 60)
        draw_disc(x, y, 20)
        present()
    }
}
```

Nearly every game has these three parts:

1. **State:** `x` and `y` live outside the loop, so they survive from frame to frame.
2. **Input and update:** the `key_down` checks change the state, and `clamp` keeps the
   ball inside the window.
3. **Draw:** the picture is made from the state.

Keep that order each frame: input, update, draw.

## Read the mouse

`mouse_x()` and `mouse_y()` give the pointer position in pixels. `mouse_down()` is true
while the left button is held, `mouse_down_right()` while the right one is.

```ling-gui
bind start = do {
    open_window(640, 400, "Mouse paint")
    fill(245, 240, 225)
    bind hue = 0
    while window_is_open() {
        if mouse_down() {
            set_color_hsl(hue, 80, 50)
            draw_disc(mouse_x(), mouse_y(), 10)
            bind hue = (hue + 3) % 360
        }
        if mouse_down_right() {
            fill(245, 240, 225)
        }
        present()
    }
}
```

This program doesn't call `fill` every frame, so the paint stays on the canvas. A right
click calls `fill` once to wipe it.

## A tiny game: Star Catcher

Stars fall from the sky and you slide a basket to catch them. Each catch scores a point
and makes the next star faster. Miss three and the game is over; press Space to play again.

Before writing a game, list its **state** (the basket's x, the star's position, the fall
speed, the score, the lives), its **rules** (how the state changes each frame) and its
**picture** (how the state is drawn).

```ling-gui
fn rect(x, y, w, h, r, g, b) {
    grad_rect(x, y, w, h, r, g, b, r, g, b, 1)
}

# A five-pointed star: a polygon with ten corners.
fn star(cx, cy, size, spin) {
    bind pts = []
    bind i = 0
    while i < 10 {
        bind a = spin + i * 0.6283185
        bind r = size
        if i % 2 == 1 {
            bind r = size * 0.45
        }
        bind pts = list_push(pts, cx + cos(a) * r)
        bind pts = list_push(pts, cy + sin(a) * r)
        bind i = i + 1
    }
    draw_poly(pts)
}

bind start = do {
    open_window(640, 480, "Star Catcher")
    bind basket_x = 320
    bind star_x = 100 + rand() * 440
    bind star_y = 0
    bind fall = 3
    bind score = 0
    bind lives = 3
    bind t = 0

    while window_is_open() {
        if lives > 0 {
            # input
            if key_down("left") || key_down("a") {
                bind basket_x = basket_x - 7
            }
            if key_down("right") || key_down("d") {
                bind basket_x = basket_x + 7
            }
            bind basket_x = clamp(basket_x, 50, 590)
            # update
            bind star_y = star_y + fall
            if star_y > 440 {
                if abs(star_x - basket_x) < 55 {
                    bind score = score + 1
                    bind fall = fall + 0.25
                } else {
                    bind lives = lives - 1
                }
                bind star_x = 50 + rand() * 540
                bind star_y = 0
            }
        } else {
            if key_pressed("space") {
                bind lives = 3
                bind score = 0
                bind fall = 3
                bind star_y = 0
            }
        }

        # draw
        grad_rect(0, 0, 640, 480, 15, 15, 50, 40, 25, 80, 1)
        set_color(255, 220, 80)
        star(star_x, star_y, 24, t * 0.05)
        rect(basket_x - 50, 445, 100, 14, 90, 190, 255)
        set_color(255, 255, 255)
        ui_text(16, 14, 22, "SCORE " + to_str(score))
        ui_text(500, 14, 22, "LIVES " + to_str(lives))
        if lives <= 0 {
            set_color(255, 110, 110)
            ui_text(170, 200, 36, "GAME OVER")
        }
        present()
        bind t = t + 1
    }
    print("Final score: ", score)
}
```

How it works:

- **The star.** `star` builds a flat list of ten corner coordinates with `cos` and `sin`,
  alternating a long and a short radius, and hands it to `draw_poly`. The `spin` angle
  makes it turn as it falls.
- **Game state.** `if lives > 0 { ... } else { ... }` switches between "playing" and
  "game over". In the game-over branch only the restart key is checked.
- **Catching.** When the star reaches the basket's height, `abs(star_x - basket_x) < 55`
  decides whether it was caught. `rand()` returns a number from 0 up to 1, so
  `50 + rand() * 540` is a random x between 50 and 590.
- **Difficulty.** `bind fall = fall + 0.25` makes every next star a little faster.
- **Text.** `ui_text(x, y, size, text)` draws text with a built-in line font in the pen
  colour. It needs no font file and draws Latin capital letters and digits.
- **The end.** When you close the window, the loop ends and the last line prints your
  score in the terminal.

## Try it

1. Draw a face: a big yellow disc, two small dark discs for eyes, and an arc for the mouth
   (try `draw_arc(200, 210, 60, 0.4, 2.74, 24)` under a face centred at `(200, 200)`).
2. Make the ball bounce: keep speeds `vx` and `vy`, add them to `x` and `y` each frame,
   and flip the sign (`bind vx = 0 - vx`) when the ball reaches a wall.
3. In the paint program, make the colour follow the pointer's x position:
   `bind hue = mouse_x() / 640 * 360`.
4. Add a best score to Star Catcher and show it under the score with `ui_text`.
5. Add a bomb: a second falling object drawn as a dark disc that costs a life if you
   catch it.

Next: [Sound and Music](sound-music.md). For every drawing and input builtin, see
[Window and Camera](../reference/window.md) and [2D Drawing and Images](../reference/drawing.md).
