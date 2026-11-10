# Overview

Builtins are the functions that come with Ling: printing, math, text, files, windows, sound, the
web server and much more. This section lists them by topic, with their names in each language, what
they take and return, and short examples you can run.

## How builtins work

**Always available.** You never import a builtin. Call it by name anywhere in your program:

```ling
bind start = do {
  print(sqrt(16) + max(2, 5))
}
```
```output
9
```

**Positional arguments.** Arguments have no names; their order matters. A missing trailing argument
usually takes a documented default instead of causing an error, so `clamp(1.5)` limits to the range
0..1.

**Failure is a value.** Most builtins report problems by returning something you can test: `false`,
`0`, `-1`, an empty list, or a `bad(...)` result. Each entry says which. Builtins that can fail in
interesting ways return `ok(value)` or `bad(reason)`; see [Errors and Results](../language/errors.md).

**Names in five languages.** Many builtins have names in Chinese, Japanese, Korean and Thai as well
as English. They all call the same function, and the English name always works, in any file, next
to any other language:

```ling
bind start = do {
  print(sqrt(16))
  印(根(16))
  印刷(平方根(16))
  출력(제곱근(16))
  พิมพ์(รากที่สอง(16))
}
```
```output
4
4
4
4
4
```

`lingfu normalize --to <lang>` rewrites a program's keywords and builtins into one language. See
[Writing in Five Languages](../language/multilingual.md). The runtime also knows some names in
other languages (French, German, Russian, Arabic, Persian, Hebrew, Urdu); these pages list only
the five documented languages.

## Profiles

`cargo install ling-lang` installs the `full` profile, which includes every builtin. If you
installed a smaller [install profile](../getting-started/install.md), some families of builtins
are left out of your build. Calling one stops the program with an error that names the profile
to add:

```text
'http_get' needs the `web` profile, which this build of ling leaves out (reinstall with: cargo install ling-lang --features web, or the default full install)
```

| Profile | Builtin families (examples) |
|---|---|
| `core` | The Output, Text and Files pages (except processes), windows, 2D drawing, fonts, images, meshes |
| `web` | `http_*`, `db_*`, `zip_*`, `tar_gz_*`, `video_*`, `run_process`, `proc_*`, hashing and crypto |
| `privacy` | `p2p_*`, `lingtp_*`, `ssh_*`, `lw_*` |
| `game` | `rb_*`, `soft_*`, `liquid_*`, `gltf_*`, `gpu_*`, `body_*`, `pad_*`, `dialog_*`, `ui_*` |
| `ai` | `nn_*`, `bt_*`, `stt_*`, learned dialog models |
| `audio` | `audio_*`, `music_*`, `mic_*`, `fft_*` |

Heavy extras such as `llm_*` (local language models) and `vision_*` are opt-in on top of any
profile.

## Reading these pages

Each reference page has the same shape:

1. An introduction with the rules that apply to the whole page.
2. **Summary tables.** The first column is the signature, such as `clamp(x, low, high)`. The
   language columns give the localized names; `—` means there is no localized name yet, so use
   the English one. Only your reading language's column is shown when the page is translated.
3. **One section per builtin or group**: parameters, return value, gotchas, and an example.

Examples marked `ling` were run with `ling run` and the output shown under them is real. Examples
that need a window, sound, the network or a server are checked for syntax only, so they have no
output block. Fragments without a `start` block are pieces of a larger program.

Ling has two backends: the default compiled one and the interpreter (`ling run --interp`). Where
they differ, for example functions passed as arguments, which only work on the interpreter, the
entry says so. [Known Limits](../language/limits.md) collects these differences.

## Pages in this section

| Page | What it covers |
|---|---|
| [Output, Math and Logic](core.md) | `print`, `format`, `ok`/`bad`, rounding, trigonometry, noise, `rand`, `sleep` |
| [Text and Lists](text-lists.md) | Measuring, searching, cutting and converting text; building and reading lists |
| [Files, Time and System](files.md) | Files and directories, time, command line, environment, processes, threads |
| [Window and Camera](window.md) | Opening a window, the frame loop, the 3D camera |
| [2D Drawing and Images](drawing.md) | Pixels, lines, shapes, text on screen, loading and drawing images |
| [3D Shapes and Meshes](shapes.md) | Built-in 3D shapes, meshes, textures |
| [Fonts and UI](fonts-ui.md) | Fonts, text layout and interface widgets |
| [Animation](animation.md) | Tweens, easing and animated motion |
| [Post-FX and Lighting](postfx.md) | Lights and screen effects |
| [Audio](audio.md) | Playing sounds and synthesizing audio |
| [Music](music.md) | Notes, scales, music playback and MIDI |
| [Physics](physics.md) | Rigid bodies, soft bodies and liquids |
| [Acoustic Bodies](body.md) | Physical objects that make sound |
| [Input and Microphone](input.md) | Keyboard, mouse, gamepads and the microphone |
| [Dialog](dialog.md) | Cinematic dialog boxes that type out text, for games |
| [Web Server and HTTP](web.md) | Routes, handlers, HTTP client requests |
| [Database (SQLite)](database.md) | Opening a database, running queries |
| [Networking](networking.md) | Peer-to-peer, `lingtp://` and SSH |
| [Cryptography](crypto.md) | Hashes, passwords, signatures and encryption |
| [AI and Language Models](ai.md) | Neural networks, behaviour trees, speech-to-text, local language models |
