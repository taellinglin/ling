# Ling — Showcase

**Ling is the omniglot creative-systems language**: you write the *same* program in
**English, 中文, 日本語, 한국어, or ไทย** — keywords, builtins, even file names — and it runs
identically. One small native binary gives you a 3D/4D holographic renderer, vector fonts,
a UI toolkit, a music studio (decode · BPM/key · GM synth · MIDI · karaoke), spatial audio,
post-quantum crypto, and a physics lab — no engine install, no package soup.

```ling
# the same program, four ways — all run:
bind start = do { open_window(800, 600)  draw_disc(400.0, 300.0, 120.0)  present() }
绑定 开始 = 做 { 开窗(800.0, 600.0)  画实心圆(400.0, 300.0, 120.0)  显() }
バインド 開始 = 実行 { ウィンドウ開く(800,600)  塗りつぶし円(400,300,120)  表示() }
ผูก เริ่ม = ทำ { เปิดหน้าต่าง(800,600)  วาดวงกลมทึบ(400,300,120)  แสดงผล() }
```

`lingfu normalize <lang>` rewrites any Ling project into a single language — so a team can
each read the code in their own tongue and commit a normalized form.

## What makes Ling unique

- **Polyglot by design** — five human languages are first-class, not comments or i18n strings.
  Every keyword and ~hundreds of builtins have native aliases; the lexer/parser accept any mix.
- **Batteries *welded* in** — graphics, vector fonts, UI, music, MIDI, spatial audio, crypto,
  physics and AI tensors ship in the one toolchain. First pixel in ~5 lines.
- **3D *and* 4D / holographic** — a depth-sorted software renderer with a cel/holographic shader,
  4D camera projection, and vector-texture primitives (`vtex_*`).
- **Resolution-independent vector fonts** — TrueType outlines cached as `.ling` glyph files and
  rendered (stroke or fill) crisp in 2D/3D/4D, in every script (灵 · ABC · あ · 가 · ก).
- **A real music layer** — load WAV/FLAC/OGG/MP3, extract BPM & musical key, synthesize *any*
  GM-style instrument from a `.ling` patch, read MIDI, build rhythm games & karaoke.
- **Post-quantum crypto built in** — hashes, AEAD, signatures, ML-KEM/X25519 hybrids, Shamir,
  Schnorr/VRF — usable from a script in one call.
- **Physics from a script** — rigid + **angular** dynamics, **deformable soft-body** meshes, and
  a **fast immiscible water/oil liquid sim** that maps onto plane/sphere/cylinder/cone/dome.
- **Tiny & native** — one binary, software-rendered (no GPU driver required), runs headless for
  CI frame-capture; also targets WASM.

## How Ling compares

Legend: ● built-in · ◐ partial / via add-on · ○ not a goal.

| Capability | **Ling** | Unity | Godot | Bevy (Rust) | Pygame | p5.js | LÖVE |
|---|:--:|:--:|:--:|:--:|:--:|:--:|:--:|
| Source in 5 human languages | ● | ○ | ○ | ○ | ○ | ○ | ○ |
| Single self-contained binary | ● | ○ | ◐ | ● | ◐ | ○ | ◐ |
| No GPU/driver required (software) | ● | ○ | ○ | ○ | ● | ◐ | ○ |
| 3D rendering | ● | ● | ● | ● | ○ | ◐ | ○ |
| 4D / hyperbolic / holographic | ● | ○ | ○ | ○ | ○ | ○ | ○ |
| Vector fonts (2D/3D/4D) | ● | ◐ | ◐ | ◐ | ◐ | ◐ | ◐ |
| Immediate-mode UI toolkit | ● | ◐ | ● | ◐ | ○ | ○ | ◐ |
| Audio playback | ● | ● | ● | ● | ● | ● | ● |
| Spatial (3D/4D) audio + FX | ● | ● | ● | ◐ | ○ | ◐ | ◐ |
| BPM / musical-key analysis | ● | ○ | ○ | ○ | ○ | ◐ | ○ |
| GM-style synth from a patch file | ● | ◐ | ◐ | ○ | ○ | ◐ | ○ |
| MIDI load + rhythm/karaoke kit | ● | ◐ | ◐ | ○ | ◐ | ◐ | ○ |
| Rigid + angular physics | ● | ● | ● | ◐ | ○ | ○ | ◐ |
| Soft-body / deformable mesh | ● | ◐ | ◐ | ◐ | ○ | ○ | ○ |
| Fast immiscible liquid sim | ● | ◐ | ◐ | ○ | ○ | ○ | ○ |
| Post-quantum crypto built in | ● | ○ | ○ | ○ | ○ | ○ | ○ |
| Headless CI frame capture | ● | ◐ | ◐ | ◐ | ◐ | ○ | ◐ |
| Lines to first pixel | **~5** | many | ~15 | ~40 | ~10 | ~5 | ~8 |

> Comparisons are about *what ships in the box*. Mature engines like Unity/Godot/Bevy have far
> deeper 3D pipelines, tooling and ecosystems — Ling's angle is breadth-in-one-binary plus the
> things no one else has: **truly multilingual source**, **4D/holographic vector rendering**,
> built-in **music analysis + GM synth**, and **post-quantum crypto**.

## See it run

| Demo | What it shows |
|---|---|
| `examples/music/hallway_runner.ling` | Auto-playing FFT-reactive infinite runner; MIDI-driven coins ring the melody as spatial SFX |
| `examples/music/karaoke_hero.ling` | Karaoke-Hero pitch bars from a vocal MIDI + live mic tuner |
| `examples/music/synth_jam.ling` | GM instruments synthesized from `.ling` patches |
| `examples/physics/bouncy_ball.ling` | Spring-mass soft body squashing as it bounces |
| `examples/physics/tumbling.ling` | Rigid bodies tumbling & rolling (angular dynamics) |
| `examples/physics/liquid_marble.ling` | Water+oil sim mapped onto a spinning sphere/cone/dome |
| `examples/ui/hud_showcase.ling` | The full vector UI toolkit (HUD, meters, controls, game UI) |
| `examples/crypto/crypto_donut_physics.ling` | A SHA3 hash → a bouncing knot-torus with reactive audio |

```sh
cargo run --bin ling -- examples/physics/liquid_marble.ling
```

See the full reference (in 5 languages) at **https://docs.ling-lang.org**.
