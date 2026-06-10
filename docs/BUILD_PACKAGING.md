# Build & Packaging — icons and bundled resources

`ling build <file.ling|dir> [--platform win|lin|mac|web] [--out dir]` compiles a
Ling project into a native executable. Two packaging features control what ships
with it: an **app icon** and **included resources**.

## App icon

Built Windows executables get an embedded icon. Resolution order:

1. `--icon <file>` on the command line (highest priority)
2. the manifest `icon` field
3. the bundled Ling logo (default)

A missing explicit icon warns and falls back to the default. Sources may be
`.svg` (rendered faithfully — gradients, filters, blur — via resvg), `.png` /
`.jpg` / `.bmp` (resized), or a ready-made `.ico` (used as-is). All are packed
into a multi-resolution `.ico` (16–256 px).

```toml
# Ling.toml
[project]
name = "my-game"
entry = "main.ling"
icon = "images/logo.svg"     # relative to the project root
```

```toml
# 灵符.toml  (ling-fu projects)
[灵符]
名 = "我的游戏"
图标 = "images/logo.svg"     # 图标 = icon
```

```sh
ling build my-game --icon branding/app.png   # overrides the manifest
```

The Ling toolchain binaries (`ling.exe`, `lingc.exe`, `lingfu.exe`, …) embed the
default logo from `assets/ling.ico` at build time.

> Convert any SVG/PNG to a multi-size `.ico` directly:
> `cargo run -p ling-icon --example svg2ico -- logo.svg out.ico`

## Bundled resources — `[includes]`

Declare base folders and files (relative to the project root) to ship with the
build. Glob patterns are supported: `*` (within a path segment), `**` (across
segments), `?` (one char). A bare folder name includes everything under it.

```toml
# Ling.toml
[includes]
"/music/*.wav",
"/font/*.otf",
"/levels/**/*.json",
"somefile.bin"
```

The leading `/` (meaning "from the project root") is optional. An inline array
(`includes = ["music/*.wav", ...]`) and the Chinese header `[包含]` also work.

### Default: copied next to the exe

By default the matched files are copied into `dist/<platform>/`, preserving their
folder structure:

```
dist/windows/
├── my-game.exe
├── music/song.wav
├── font/title.otf
└── somefile.bin
```

The app loads them by their normal relative paths.

### `--pack`: embed everything into the exe

```sh
ling build my-game --platform win --pack
```

With `--pack`, the included files are embedded **inside** the executable
(`include_bytes!`) producing a single self-contained `.exe`. At startup the app
transparently extracts them to a per-app temp directory and switches its working
directory there, so every existing asset loader (`read_file`, `font_load`,
`music_load`, `audio_sample_load`, …) finds its files unchanged — no code changes
required. Ship just the one file.

| Mode | Output | Use when |
|------|--------|----------|
| default | exe + resource folders in `dist/<platform>/` | assets may be patched/modded separately |
| `--pack` | one self-contained exe | simplest distribution; single file |
