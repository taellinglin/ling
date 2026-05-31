# Ling — The Omniglot Systems Language

[![crates.io](https://img.shields.io/crates/v/ling-lang.svg)](https://crates.io/crates/ling-lang)
[![docs.rs](https://docs.rs/ling-lang/badge.svg)](https://docs.rs/ling-lang)
[![License](https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue)](LICENSE)
[![GitHub](https://img.shields.io/github/stars/taellinglin/ling)](https://github.com/taellinglin/ling)

**Write in your language. Run everywhere.**

Ling is a polyglot scripting and systems language whose keywords and builtins are
available in 16+ human languages simultaneously — Chinese, Thai, Korean, Japanese,
English, Arabic, Hebrew, Russian, and more — in the same source file, with no
`#lang` pragmas or context switches.

---

## Quick Start

```bash
cargo install ling-lang
ling run examples/hello.ling
```

Or from source:

```bash
git clone https://github.com/taellinglin/ling
cd ling
cargo run --bin ling -- run examples/hello.ling
```

---

## Hello World in four lexicons

```ling
# Chinese
令 启 = 执 { 印("你好世界") }

# Thai
令 เริ่ม = ดำเนินการ { พิมพ์("สวัสดีโลก") }

# English
bind start = do { print("Hello, World!") }

# Korean
바인드 시작 = 수행 { 인쇄("안녕하세요") }
```

All four are valid in a single `.ling` file.

---

## Language Overview

### Core syntax

| Concept | Chinese | Thai | English | Korean |
|---------|---------|------|---------|--------|
| Bind | `令` | `ให้` | `bind` | `바인드` |
| Function | `函` | `ฟังก์ชัน` | `func` | `펑크` |
| If | `若` | `ถ้า` | `if` | `이프` |
| While | `循` | `ขณะที่` | `while` | `동안` |
| Do block | `执` | `ดำเนินการ` | `do` | `수행` |
| Return | `返` | `คืนค่า` | `return` | `리턴` |
| Print | `印` | `พิมพ์` | `print` | `인쇄` |

### Values

```ling
令 x = 42
令 s = "hello"
令 b = true
令 lst = list_new()
令 lst = list_push(lst, 1)
```

### Functions

```ling
函 add(a, b) {
    返 a + b
}

令 结果 = add(3, 4)
```

### Control flow

```ling
若 x > 10 {
    印("large")
} 否则 {
    印("small")
}

令 i = 0
循 i < 5 {
    印(i)
    令 i = i + 1
}
```

---

## 3D/Visual Builtins

Ling has a built-in software renderer for interactive 3D rooms and visualisations.
All drawing builtins are multilingual (Thai names shown alongside English).

### Window & camera

```ling
เปิดหน้าต่างเต็มจอ("My Room")   # open fullscreen window
set_camera(cry, sry, crx, srx)   # orient camera
set_camera_pos(x, y, z)
set_zdist(2.0)
set_ambient(0.1)
แสดงผล()                         # flush depth queue → screen
```

### Vector geometry (`vtex_*`)

All vtex calls draw line-based 3D geometry on a plane defined by centre + two tangent
vectors. They are fast (no pixel fills) and depth-sorted automatically.

```ling
# vtex_grid(cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows, cw,ch, fr,hue)
vtex_grid(0, -4, 8,  1,0,0,  0,0,1,  12, 8,  1.0, 1.0,  FR, 0.0)

# vtex_rings(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_rings,n_sides, max_r,twist, fr,hue)
vtex_rings(0, 0, 8,  1,0,0,  0,1,0,  4, 32,  2.0, 0.08,  FR, 1.57)

# vtex_spiked_cog(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_teeth,r_body,r_spike,r_hub,n_spokes, fr,hue)
vtex_spiked_cog(0, 0, 8,  1,0,0,  0,1,0,  16, 1.5, 2.0, 0.3, 8,  FR, 1.57)

# vtex_torii(cx,cy,cz, ux,uy,uz, vx,vy,vz, width,height, fr,hue)
vtex_torii(0, 0, 14,  1,0,0,  0,1,0,  6.0, 5.0,  FR, 2.1)

# vtex_pagoda(cx,cy,cz, ux,uy,uz, vx,vy,vz, n_tiers,base_w,tier_h,taper,eave, fr,hue)
vtex_pagoda(0, 0, 14,  1,0,0,  0,1,0,  5, 2.5, 1.0, 0.72, 0.28,  FR, 1.57)
```

Full vtex reference: **[docs/src/reference/vtex.md](docs/src/reference/vtex.md)**

### Procedural pixel textures (`tex_*`)

```ling
tex_noise(x, y, w, h,  scale, octaves, seed, "psychedelic")
tex_julia(x, y, w, h,  c_re, c_im, max_iter, "neon")
tex_mandelbrot(x, y, w, h,  zoom, cx, cy, max_iter, "fire")
tex_voronoi(x, y, w, h,  cells, seed, "ocean")
tex_freq_map(x, y, w, h,  time, speed, "rainbow")   # audio-reactive
```

Full tex reference: **[docs/src/reference/tex.md](docs/src/reference/tex.md)**

### Audio

```ling
# 4D spatial tone synthesis
audio_tone(slot, x, y, z, w, freq_hz, amp, lfo_rate, lfo_depth)
audio_volume(0.5)

# FFT analysis
fft_push(samples_list)
令 bands = fft_bands(32)   # returns list of 32 magnitudes
```

---

## Workspace Crates

| Crate | crates.io | Purpose |
|-------|-----------|---------|
| `ling-core` | [![](https://img.shields.io/crates/v/ling-core.svg)](https://crates.io/crates/ling-core) | Core types and errors |
| `ling-lex` | [![](https://img.shields.io/crates/v/ling-lex.svg)](https://crates.io/crates/ling-lex) | Polyglot tokenizer |
| `ling-ast` | [![](https://img.shields.io/crates/v/ling-ast.svg)](https://crates.io/crates/ling-ast) | AST types |
| `ling-mir` | [![](https://img.shields.io/crates/v/ling-mir.svg)](https://crates.io/crates/ling-mir) | Mid-level IR |
| `ling-codegen` | [![](https://img.shields.io/crates/v/ling-codegen.svg)](https://crates.io/crates/ling-codegen) | Code generation |
| `ling-runtime` | [![](https://img.shields.io/crates/v/ling-runtime.svg)](https://crates.io/crates/ling-runtime) | GC and stdlib |
| `ling-audio` | [![](https://img.shields.io/crates/v/ling-audio.svg)](https://crates.io/crates/ling-audio) | 4D spatial audio + FFT |
| `ling-graphics` | [![](https://img.shields.io/crates/v/ling-graphics.svg)](https://crates.io/crates/ling-graphics) | 3D/4D rendering |
| `ling-game` | [![](https://img.shields.io/crates/v/ling-game.svg)](https://crates.io/crates/ling-game) | ECS + physics |
| `ling-crypto` | [![](https://img.shields.io/crates/v/ling-crypto.svg)](https://crates.io/crates/ling-crypto) | Hashing, encryption |
| `ling-net` | [![](https://img.shields.io/crates/v/ling-net.svg)](https://crates.io/crates/ling-net) | Async networking |
| `ling-ai` | [![](https://img.shields.io/crates/v/ling-ai.svg)](https://crates.io/crates/ling-ai) | Neural networks / LLM |
| `ling-py` | [![](https://img.shields.io/crates/v/ling-py.svg)](https://crates.io/crates/ling-py) | Python bindings |
| `ling-wasm` | [![](https://img.shields.io/crates/v/ling-wasm.svg)](https://crates.io/crates/ling-wasm) | WebGL2 WASM target |

---

## Project Structure

```
ling/
├── src/                  # Main interpreter + renderer
│   ├── runtime/mod.rs    # Builtins, eval loop
│   ├── gfx/vtex.rs       # Vector texture primitives
│   └── parser/           # Lexer + grammar
├── crates/               # Library workspace members
│   ├── ling-audio/       # FFT + synthesis
│   ├── ling-game/        # ECS, mesh, physics
│   └── ...
├── examples/             # .ling demo rooms
│   ├── ling-dao-chamber.ling   # Chinese shrine
│   ├── Garden.ling
│   └── ...
├── lexicons/             # Language definition files
│   ├── th.ling           # Thai
│   ├── zh.ling           # Chinese
│   └── ...
└── docs/                 # mdBook documentation
    ├── book.toml
    └── src/
        ├── SUMMARY.md
        ├── index.md
        └── reference/
```

---

## Building

```bash
# Debug build
cargo build

# Release binary
cargo build --release

# Run a room
cargo run --bin ling -- run examples/ling-dao-chamber.ling

# Build docs (requires mdbook)
cargo install mdbook
mdbook build docs/
mdbook serve docs/
```

---

## Documentation

- **Online reference**: [docs.rs/ling-lang](https://docs.rs/ling-lang)
- **mdBook guide**: [taellinglin.github.io/ling](https://taellinglin.github.io/ling) *(auto-built from `docs/`)*
- **Builtin reference** (multilingual): [docs/src/reference/builtins.md](docs/src/reference/builtins.md)

---

## License

Licensed under either of Apache License 2.0 or MIT license at your option.
