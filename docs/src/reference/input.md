# Input and Microphone

Ling reads input by **polling**: once per frame, inside your loop, you ask what
the player is doing right now. There are no callbacks. This page covers the
keyboard and mouse, typed text, game controllers (through the `gilrs` library
and the `ling-input` crate) and the microphone (the `ling-mic` crate).

Every input builtin is safe to call when the device is missing: keys and buttons
read as not pressed, axes as `0`, and the microphone as silence. A game can
therefore fall back to the keyboard without special cases. To find out whether
hardware is present, check the return value of `mic_open` or `pad_poll`.

Keyboard, mouse and text input need an open window to receive events. Without
one, they simply read as idle:

```ling
bind start = do {
  print(key_down("space"))
  print(key_pressed("enter"))
  print(mouse_down())
  print(mouse_scroll())
  print(mic_rms())
  print(mic_fft(4))
}
```

```output
false
false
false
0
0
[0, 0, 0, 0]
```

## Summary

| Builtin | 中文 | 日本語 | 한국어 | ไทย | Purpose |
|---|---|---|---|---|---|
| `key_down(name)` | `按键` | `キー押す` | `키누름` | `กดค้าง` | True while a key is held. |
| `key_pressed(name)` | `键按` | `キー押した` | `키눌림` | `กดปุ่ม` | True only on the frame a key went down. |
| `mouse_x()`, `mouse_y()` | — | — | — | — | Cursor position in pixels. |
| `mouse_down()` | — | — | — | — | Left button held. |
| `mouse_down_right()` | — | — | — | `เมาส์ขวา` | Right button held. |
| `mouse_down_middle()` | — | — | — | `เมาส์กลาง` | Middle button held. |
| `mouse_dx()` | `鼠ΔX` | `マウスΔX` | `마우스ΔX` | `เมาส์X` | Horizontal movement while captured. |
| `mouse_dy()` | `鼠ΔY` | `マウスΔY` | `마우스ΔY` | `เมาส์Y` | Vertical movement while captured. |
| `mouse_scroll()` | `滚轮` | `ホイール` | `스크롤` | `ล้อเมาส์` | Vertical wheel movement this frame. |
| `mouse_scroll_x()` | — | — | — | — | Horizontal wheel movement this frame. |
| `capture_mouse()` | `捕鼠` | `マウス捕捉` | `마우스잡기` | `จับเมาส์` | Hide the cursor and switch to relative motion. |
| `release_mouse()` | — | — | — | — | Undo `capture_mouse`. |
| `cursor_hide()`, `cursor_show()` | — | — | — | — | Hide or show the pointer only. |
| `text_poll()` | — | — | — | — | Add newly typed characters to the text buffer and return it. |
| `text_get()`, `text_set(s)`, `text_clear()` | — | — | — | — | Read, replace or empty the text buffer. |
| `pad_poll()` | `手柄轮询` | `パッド更新` | `패드폴링` | `อัปเดตแพด` | Advance controller input one frame. Returns the pad count. |
| `pad_count()` | `手柄数` | `パッド数` | `패드수` | `จำนวนแพด` | Connected pads, as of the last poll. |
| `pad_connected(player)` | `手柄连接` | `パッド接続` | `패드연결` | `แพดเชื่อม` | Whether a player slot has a pad. |
| `pad_button(player, name)` | `手柄按键` | `パッドボタン` | `패드버튼` | `ปุ่มแพด` | True while a button is held. |
| `pad_pressed(player, name)` | `手柄按下` | `パッド押下` | `패드눌림` | `แพดกด` | True only on the frame a button went down. |
| `pad_lx(player)`, `pad_ly(player)` | `手柄左X`, `手柄左Y` | `パッド左X`, `パッド左Y` | `패드왼X`, `패드왼Y` | `แพดซ้ายX`, `แพดซ้ายY` | Left stick, -1 to 1. |
| `pad_rx(player)`, `pad_ry(player)` | `手柄右X`, `手柄右Y` | `パッド右X`, `パッド右Y` | `패드오X`, `패드오Y` | `แพดขวาX`, `แพดขวาY` | Right stick, -1 to 1. |
| `pad_lt(player)`, `pad_rt(player)` | `手柄左扳机`, `手柄右扳机` | `パッド左トリガー`, `パッド右トリガー` | `패드왼트리거`, `패드오트리거` | `ไกแพดซ้าย`, `ไกแพดขวา` | Analog triggers, 0 to 1. |
| `pad_rumble(player, low, high)` | `手柄震动` | `パッド振動` | `패드진동` | `แพดสั่น` | Request vibration (currently inert). |
| `gamepad_poll()` | — | — | — | `จอยโพล` | Pump controller events (older family). |
| `gamepad_button(name)` | — | — | — | `จอยปุ่ม` | `1` while a named button is held. |
| `gamepad_axis(name)` | — | — | — | `จอยแกน` | Value of a named axis. |
| `gamepad_rumble(low, high, ms)` | — | — | — | `จอยสั่น` | Vibrate the current controller. |
| `gamepad_list()` | — | — | — | `จอยรายการ` | Describe the controllers the system sees. |
| `gamepad_any()` | — | — | — | `จอยใดๆ` | `1` if any button is down. |
| `mic_open()` | `开麦克风` | `マイク開く` | `마이크열기` | `เปิดไมค์` | Open the default microphone. |
| `mic_rms()` | `麦克风音量` | `マイクRMS` | `마이크RMS` | `เสียงRMS` | Loudness of the latest buffer, 0 to 1. |
| `mic_peak()` | `麦克风峰值` | `マイクピーク` | `마이크피크` | `เสียงพีค` | Peak level of the latest buffer, 0 to 1. |
| `mic_fft(bands)` | `实时频谱` | `リアルタイムFFT` | `실시간FFT` | `วิเคราะห์เสียงสด` | Spectrum in `bands` bands, bass first. |
| `mic_capture()` | — | — | — | — | Append the latest samples to the record buffer. |
| `mic_seed()` | — | — | — | — | SHA3-256 of the recorded audio, as hex. |
| `mic_clear()` | — | — | — | — | Empty the record buffer. |

## Keyboard

### `key_down(name)` and `key_pressed(name)`

`key_down` is true for as long as the key is held: use it for movement.
`key_pressed` is true only on the single frame the key went down, so holding the
key doesn't repeat: use it for toggles, menus and firing once per press.

Both take a lower-case key name. An unknown name is simply `false`. Both read
`false` while another window has focus. Pressing Start on a game controller also
counts as `key_pressed("enter")`, so menus that wait for Enter work with a pad.

| Group | Names |
|---|---|
| Letters, digits | `"a"` to `"z"`, `"0"` to `"9"` |
| Arrows and navigation | `"left"`, `"right"`, `"up"`, `"down"`, `"home"`, `"end"`, `"pageup"`, `"pagedown"`, `"insert"`, `"delete"` |
| Editing | `"space"`, `"enter"`, `"escape"`, `"tab"`, `"backspace"`, `"capslock"` |
| Modifiers | `"lshift"`, `"rshift"`, `"lctrl"`, `"rctrl"`, `"lalt"`, `"ralt"` |
| Function keys | `"f1"` to `"f12"` |
| Number pad | `"numpad0"` to `"numpad9"` (or `"kp0"` to `"kp9"`), `"numpad+"`, `"numpad-"`, `"numpad*"`, `"numpad/"` |
| Punctuation | `"minus"`, `"equals"`, `"comma"`, `"period"`, `"slash"`, `"semicolon"`, `"quote"`, `"lbracket"`, `"rbracket"`, `"backslash"`, `"grave"` (the symbols themselves also work) |

```ling-gui
bind start = do {
  open_window(800, 600)
  bind x = 400
  bind paused = 0
  while window_is_open() {
    if key_pressed("p") { bind paused = 1 - paused }
    if paused == 0 {
      if key_down("left") { bind x = x - 4 }
      if key_down("right") { bind x = x + 4 }
    }
    fill(0, 0, 0)
    set_color(255, 255, 255)
    draw_disc(x, 300, 20)
    present()
  }
}
```

### Typed text

For name entry and chat, let Ling collect characters for you. `text_poll()` adds
the keys typed since the last call to a text buffer (Backspace deletes, and
holding a key repeats) and returns the whole buffer. Call it once per frame
while the field is active. `text_get()` reads the buffer without polling,
`text_set(s)` replaces it, and `text_clear()` empties it.

```ling
bind start = do {
  text_set("Ada")
  print(text_get())
  text_clear()
  print("[" + text_get() + "]")
}
```

```output
Ada
[]
```

## Mouse

- `mouse_x()`, `mouse_y()`: the cursor position inside the window, in pixels.
  Both are `0` until a window exists.
- `mouse_down()`, `mouse_down_right()`, `mouse_down_middle()`: true while that
  button is held. They are level checks: for a click, compare with last frame's
  value.
- `mouse_scroll()`, `mouse_scroll_x()`: wheel movement during the last frame,
  `0` when idle.
- `mouse_dx()`, `mouse_dy()`: how far the mouse moved since the last frame.
  Positive `dx` is right, positive `dy` is down. They are only non-zero while the
  mouse is captured.
- `capture_mouse()` hides the cursor and switches to relative motion, so
  `mouse_dx` and `mouse_dy` keep reporting however far the mouse travels. This is
  the basis of first-person look. `release_mouse()` undoes it. Escape still closes
  the window.
- `cursor_hide()`, `cursor_show()` hide or show only the pointer. The position
  keeps tracking normally, which suits games that draw their own cursor.

```ling-gui
bind start = do {
  open_window(800, 600)
  bind was = false
  while window_is_open() {
    bind now = mouse_down()
    if now && !was {
      print("click at " + str(mouse_x()) + ", " + str(mouse_y()))
    }
    bind was = now
    fill(0, 0, 0)
    present()
  }
}
```

## Game controllers

There are two families. The **`pad_*`** family is the one to use: up to four
players, edge-detected buttons, analog triggers, and the same code on the desktop
and in the browser (where it uses the W3C Gamepad API). The older
**`gamepad_*`** family reads the most recently used controller by name and is the
one that can make a controller vibrate.

### The `pad_*` family

Call `pad_poll()` once per frame, before reading anything. It drains controller
events, advances the edge detection and returns the number of connected pads.
Player slots are `0` to `3`. The first call opens the controller backend; if
that fails, it returns `0` and every read stays neutral.

- `pad_button(player, name)`: true while the button is held.
- `pad_pressed(player, name)`: true only on the frame of the press. In a browser
  build only the current state is known, so it behaves like `pad_button` there.
- `pad_lx`, `pad_ly`, `pad_rx`, `pad_ry`: stick axes, `-1` to `1`. Pushing a stick
  **up** gives a **negative** `y`, matching the screen. No dead zone is applied, so
  ignore small values yourself.
- `pad_lt`, `pad_rt`: analog triggers, `0` (released) to `1` (fully pulled).
- `pad_connected(player)`, `pad_count()`: which slots are filled.
- `pad_rumble(player, low, high)`: currently does nothing on desktop or in the
  browser. Use `gamepad_rumble` to vibrate.

Button names are case-insensitive, and several names mean the same button. On the
desktop the face buttons are positional: the bottom one is `a` on every brand.

| Button | Names |
|---|---|
| bottom face | `a`, `south`, `cross` |
| right face | `b`, `east`, `circle` |
| left face | `x`, `west`, `square` |
| top face | `y`, `north`, `triangle` |
| shoulders | `lb`, `l1`, `left_shoulder`; `rb`, `r1`, `right_shoulder` |
| triggers (digital) | `lt`, `l2`, `left_trigger`; `rt`, `r2`, `right_trigger` |
| start / select | `start`, `menu`, `options`; `select`, `back`, `share`, `view` |
| guide | `guide`, `home` |
| stick clicks | `l3`, `left_stick`; `r3`, `right_stick` |
| d-pad | `up`/`dpad_up`, `down`/`dpad_down`, `left`/`dpad_left`, `right`/`dpad_right` |

```ling-gui
bind start = do {
  open_window(800, 600)
  bind x = 400.0
  bind y = 300.0
  while window_is_open() {
    pad_poll()
    if pad_connected(0) {
      bind x = x + pad_lx(0) * 5.0
      bind y = y + pad_ly(0) * 5.0
      if pad_pressed(0, "a") { print("jump") }
    }
    fill(0, 0, 0)
    set_color(255, 255, 255)
    draw_disc(x, y, 20)
    present()
  }
}
```

### The `gamepad_*` family

Call `gamepad_poll()` once per frame; the most recently active controller becomes
the current one. In a browser build `gamepad_poll` and `gamepad_rumble` do
nothing, so use `pad_*` there.

- `gamepad_button(name)`: `1` while held, else `0`. Names: `a`, `b`, `x`, `y`,
  `l1`, `r1`, `l2`, `r2`, `l3`, `r3`, `start`, `select`, `dup`, `ddown`, `dleft`,
  `dright`.
- `gamepad_axis(name)`: `lx`, `ly`, `rx`, `ry` (sticks, `-1` to `1`) and `l2`, `r2`
  (triggers). Here pushing a stick up gives a **positive** `y`, the opposite of
  `pad_ly`.
- `gamepad_rumble(low, high, ms)`: vibrate the heavy (`low`) and light (`high`)
  motors, each `0` to `1`, for `ms` milliseconds (default 200). Nothing happens if
  the controller has no force feedback.
- `gamepad_list()`: one line per controller the system can see: name, whether it's
  connected, whether it can rumble, and where its button mapping came from. Use it
  to diagnose a controller that behaves oddly.
- `gamepad_any()`: `1` if any standard button is down.

```ling-gui
bind start = do {
  print(gamepad_list())
  gamepad_poll()
  if gamepad_button("a") == 1 {
    gamepad_rumble(0.4, 0.8, 250)
  }
}
```

## Microphone

`mic_open()` starts capturing from the default input device at 44.1 kHz mono and
returns `1`, or `0` if there is no device or access was refused. Call it once;
reading levels afterwards costs almost nothing.

- `mic_rms()`: loudness of the latest buffer (root mean square), `0` to `1`.
- `mic_peak()`: the highest absolute sample in the latest buffer, `0` to `1`.
- `mic_fft(bands)`: a list of `bands` (default 8) smoothed magnitudes in
  logarithmically spaced bands from 20 Hz up, bass first. All zeros while the
  microphone is closed.

```ling-gui
bind start = do {
  open_window(640, 240)
  if mic_open() == 0 { print("no microphone") }
  while window_is_open() {
    bind bars = mic_fft(16)
    fill(0, 0, 0)
    set_color(80, 200, 255)
    bind i = 0
    while i < len(bars) {
      bind h = bars[i] * 200.0
      bind x = i * 40
      triangle(x, 230, x + 30, 230, x + 30, 230 - h)
      triangle(x, 230, x + 30, 230 - h, x, 230 - h)
      bind i = i + 1
    }
    present()
  }
}
```

For sing-along scoring, combine the microphone with the music builtins:
`music_mic_pitch()` gives the sung frequency each frame. See [Music](../reference/music.md).

### Recording a seed

`mic_capture()` appends the latest samples to a record buffer (the most recent
96000 samples are kept) and returns its length. Call it every frame while
recording. `mic_seed()` returns the SHA3-256 hash of the buffered audio as 64 hex
characters, ready to seed a random generator. `mic_clear()` empties the buffer.
With the microphone closed the buffer stays empty, and the seed is the hash of
empty input:

```ling
bind start = do {
  print(mic_capture())
  print(mic_seed())
  print(mic_clear())
}
```

```output
0
d75a5e3ee36c258bec7dced963d55ef24bda7d06d0cef7708f2ecca8371a95f7
0
```

## See also

- [Window and Camera](../reference/window.md): the frame loop and `mouse_ray` for picking in 3D.
- [Fonts and UI](../reference/fonts-ui.md): `ui_*` controls read the mouse for you.
- [AI and Language Models](../reference/ai.md): speech-to-text from recorded audio.
