<div align="center">

<img src="ling-lang.org/images/logo.svg" width="160" height="160" alt="Ling logo">

# ling · 灵 · 霊 · 령 · ลิง

**The Omniglot Systems Language**

[![crates.io](https://img.shields.io/crates/v/ling-lang?style=flat-square&color=c8273e&label=crates.io)](https://crates.io/crates/ling-lang)
[![docs.rs](https://img.shields.io/docsrs/ling-lang?style=flat-square&color=00c4cc&label=docs.rs)](https://docs.rs/ling-lang)
[![Website](https://img.shields.io/badge/website-ling--lang.org-00c4cc?style=flat-square)](https://ling-lang.org)
[![Docs](https://img.shields.io/badge/docs-docs.ling--lang.org-0070ff?style=flat-square)](https://docs.ling-lang.org)
[![License](https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-60e000?style=flat-square)](LICENSE)
[![GitHub Stars](https://img.shields.io/github/stars/taellinglin/ling?style=flat-square&color=ffb000)](https://github.com/taellinglin/ling)

*Write in your language. Run everywhere.*

</div>

---

Ling is a polyglot scripting and systems language whose keywords and builtins are
available in **16+ human languages simultaneously** — Chinese, Thai, Korean, Japanese,
English, Arabic, Hebrew, Russian, and more — in the same source file, with no
`#lang` pragmas or context switches. It compiles to native code via a Rust backend.

---

## Quick Start

```bash
cargo install ling-lang
ling run examples/hello.ling
```

```bash
# Or from source
git clone https://github.com/taellinglin/ling
cd ling
cargo run --bin ling -- run examples/hello.ling
```

---

## Hello World · 五種語言

<table>
<tr>
<th>🇺🇸 English</th>
<th>🇨🇳 Chinese</th>
<th>🇯🇵 Japanese</th>
<th>🇰🇷 Korean</th>
<th>🇹🇭 Thai</th>
</tr>
<tr>
<td>

```ling
fn main() {
  let x = 42
  print("Hello!")
}
```

</td>
<td>

```ling
函 主函数() {
  令 甲 = 42
  打印("你好！")
}
```

</td>
<td>

```ling
関数 メイン() {
  束縛 甲 = 42
  印刷("こんにちは！")
}
```

</td>
<td>

```ling
함수 메인() {
  바인드 갑 = 42
  출력("안녕하세요!")
}
```

</td>
<td>

```ling
ฟังก์ชัน หลัก() {
  ผูก ก = 42
  พิมพ์("สวัสดี!")
}
```

</td>
</tr>
</table>

All five are valid in a **single `.ling` file** — mix and match freely.

---

## Language Keywords

| Concept | 🇺🇸 EN | 🇨🇳 ZH | 🇯🇵 JA | 🇰🇷 KO | 🇹🇭 TH |
|---------|--------|--------|--------|--------|--------|
| Bind/Let | `let` | `令` | `束縛` | `바인드` | `ผูก` |
| Function | `fn` | `函` | `関数` | `함수` | `ฟังก์ชัน` |
| If | `if` | `若` | `もし` | `만약` | `ถ้า` |
| Else | `else` | `否则` | `他` | `아니면` | `มิฉะนั้น` |
| While | `while` | `循` | `間` | `동안` | `ขณะที่` |
| For | `for` | `历` | `繰` | `위해` | `สำหรับ` |
| Return | `return` | `归` | `戻る` | `반환` | `คืน` |
| Do block | `do` | `执` | `実行` | `실행` | `ทำ` |
| Match | `match` | `配` | `一致` | `매치` | `จับคู่` |
| Module | `mod` | `核` | `モジュール` | `모듈` | `โมดูล` |
| Import | `use` | `载` | `使う` | `사용` | `ใช้` |
| True | `true` | `真` | `真` | `참` | `จริง` |
| False | `false` | `假` | `偽` | `거짓` | `เท็จ` |

---

## 3D / Visual Builtins

Ling has a built-in software renderer for interactive 3D rooms and visualisations.
All drawing builtins are multilingual — Thai aliases shown alongside English.

### Window · Camera

```ling
set_camera(cry, sry, crx, srx)    # orient camera
set_camera_pos(x, y, z)
set_zdist(2.0)
set_ambient(0.1)
present()                          # flush depth queue → screen
```

### Vector Geometry · `vtex_*`

All vtex calls draw **line-based 3D geometry** on a plane defined by centre + two tangent
vectors. They are fast (no pixel fills) and depth-sorted automatically.
Every function has aliases in Thai, Chinese, Japanese, and Korean.

| English | 🇹🇭 Thai | 🇨🇳 Chinese | 🇯🇵 Japanese | 🇰🇷 Korean |
|---------|---------|-----------|------------|---------|
| `vtex_grid` | `ลายตาราง` | `纹格` | `格子模様` | `격자무늬` |
| `vtex_rings` | `ลายวงแหวน` | `纹环` | `リング模様` | `링무늬` |
| `vtex_spiral` | `ลายก้นหอย` | `纹螺` | `螺旋模様` | `나선무늬` |
| `vtex_star` | `ลายดาว` | `纹星` | `星模様` | `별무늬` |
| `vtex_flower` | `ลายดอกไม้` | `纹花` | `花模様` | `꽃무늬` |
| `vtex_lotus` | `ลายดอกบัว` | `纹莲` | `蓮模様` | `연꽃무늬` |
| `vtex_chakra` | `ลายจักร` | `纹轮` | `輪模様` | `바퀴무늬` |
| `vtex_yantra` | `ลายยันต์` | `纹扬特拉` | `ヤントラ模様` | `얀트라무늬` |
| `vtex_torii` | `ประตูโทริอิ` | `纹鸟居` | `鳥居` | `도리이` |
| `vtex_pagoda` | `เจดีย์` | `纹塔` | `塔` | `탑` |

```ling
# vtex_grid(cx,cy,cz, ux,uy,uz, vx,vy,vz, cols,rows, cw,ch, fr,hue)
vtex_grid(0, -4, 8,  1,0,0,  0,0,1,  12,8,  1.0,1.0,  FR, 0.0)

# vtex_chakra — same call, Thai name:
ลายจักร(0, 0, 8,  1,0,0,  0,1,0,  4,32,  2.0,0.08,  FR, 1.57)
```

Full vtex reference: **[docs.ling-lang.org/reference/vtex.html](https://docs.ling-lang.org/reference/vtex.html)**

### Audio · `audio_*`

```ling
audio_tone(slot, x, y, z, w, freq_hz, amp, lfo_rate, lfo_depth)
audio_volume(0.5)
audio_bgm("track.wav")
audio_bgm_volume(0.8)
audio_listener(x, y, z)
```

---

## ling-fu · Package Manager

`lingfu` (also `灵符` / `霊符` / `영부` / `ลิงฟู`) is the project manager — think Cargo for Ling.

```bash
lingfu new my-room             # scaffold a new project
lingfu run                     # build & run
lingfu build                   # compile only
lingfu test                    # run tests
lingfu normalize thai          # translate entire project to Thai
lingfu normalize zh --dry-run  # preview Chinese normalization
灵符 运行                       # run in Chinese
ลิงฟู รัน                      # run in Thai
```

The `normalize` command rewrites all `.ling` keywords, builtin names, and
file/folder names to a single target language — great for keeping large
codebases consistent.

---

## Gallery

These rooms are real `.ling` programs rendered live by Ling's built-in renderer:

| The Matrix Cathedral | East Asian Chan Temple | Eno-Ji Meditation Hall |
|---|---|---|
| ![Matrix Cathedral](ling-lang.org/images/gallery/r3d-room.jpg) | ![Chinese Room](ling-lang.org/images/gallery/r3d-chinese-room.jpg) | ![Japanese Room](ling-lang.org/images/gallery/r3d-japanese-room.jpg) |

| Mirror Garden | Thai-Japanese Clocktown | The Synthetic Garden |
|---|---|---|
| ![Mirror Garden](ling-lang.org/images/gallery/mirror-garden.jpg) | ![Clocktown](ling-lang.org/images/gallery/r3d-clocktown.jpg) | ![Garden](ling-lang.org/images/gallery/garden.jpg) |

---

## Workspace Crates

| Crate | Purpose |
|-------|---------|
| [![ling-core](https://img.shields.io/crates/v/ling-core?style=flat-square&color=c8273e&label=ling-core)](https://crates.io/crates/ling-core) | Core types and errors |
| [![ling-audio](https://img.shields.io/crates/v/ling-audio?style=flat-square&color=9900ee&label=ling-audio)](https://crates.io/crates/ling-audio) | 4D spatial audio + FFT |
| [![ling-graphics](https://img.shields.io/crates/v/ling-graphics?style=flat-square&color=0070ff&label=ling-graphics)](https://crates.io/crates/ling-graphics) | 3D/4D line renderer |
| [![ling-game](https://img.shields.io/crates/v/ling-game?style=flat-square&color=00e0cc&label=ling-game)](https://crates.io/crates/ling-game) | ECS + physics |
| [![ling-crypto](https://img.shields.io/crates/v/ling-crypto?style=flat-square&color=60e000&label=ling-crypto)](https://crates.io/crates/ling-crypto) | AES-GCM, Blake3, Curve25519 |
| [![ling-net](https://img.shields.io/crates/v/ling-net?style=flat-square&color=ffb000&label=ling-net)](https://crates.io/crates/ling-net) | Async QUIC networking |
| [![ling-ai](https://img.shields.io/crates/v/ling-ai?style=flat-square&color=dd00bb&label=ling-ai)](https://crates.io/crates/ling-ai) | LLM inference integration |
| [![ling-wasm](https://img.shields.io/crates/v/ling-wasm?style=flat-square&color=ff6010&label=ling-wasm)](https://crates.io/crates/ling-wasm) | WebGL2 WASM target |

---

## Project Structure

```
ling/
├── src/
│   ├── runtime/mod.rs       # builtins, eval loop, vtex_* dispatch
│   ├── parser/              # hand-written polyglot lexer + grammar
│   ├── lexer/mod.rs         # 16-language keyword classifier
│   └── visualize.rs         # SVG AST map generator
├── crates/
│   ├── ling-audio/          # FFT + 4D positional synthesis
│   ├── ling-fu/             # lingfu package manager
│   └── ...
├── examples/                # .ling demo rooms
│   ├── ling-dao-chamber.ling
│   ├── Garden.ling
│   └── ...
├── lexicons/                # language alias documentation
│   ├── en.ling  zh.ling  ja.ling  ko.ling  th.ling
└── docs/                    # mdBook → docs.ling-lang.org
```

---

## Building

```bash
cargo build                                        # debug
cargo build --release                              # release
cargo run --bin ling -- run examples/Garden.ling   # run a room
cargo run --bin ling -- visualize examples/Garden.ling > Garden.svg
mdbook build docs && mdbook serve docs             # local docs
```

---

## Documentation

| Resource | Link |
|----------|------|
| Language reference | [docs.ling-lang.org](https://docs.ling-lang.org) |
| API docs (docs.rs) | [docs.rs/ling-lang](https://docs.rs/ling-lang) |
| Website + gallery | [ling-lang.org](https://ling-lang.org) |
| vtex builtin reference | [docs.ling-lang.org/reference/vtex.html](https://docs.ling-lang.org/reference/vtex.html) |
| Multilingual builtins | [docs.ling-lang.org/multilingual/builtins.html](https://docs.ling-lang.org/multilingual/builtins.html) |
| Source code | [github.com/taellinglin/ling](https://github.com/taellinglin/ling) |

---

## License

Licensed under either of
[Apache License 2.0](LICENSE-APACHE) or [MIT License](LICENSE-MIT)
at your option.

<div align="center">

[![ling-lang.org](https://img.shields.io/badge/ling--lang.org-00e0cc?style=flat-square)](https://ling-lang.org)
[![docs.ling-lang.org](https://img.shields.io/badge/docs.ling--lang.org-0070ff?style=flat-square)](https://docs.ling-lang.org)

*灵语 · 霊語 · 령어 · ภาษาลิง · Ling Language*

</div>
