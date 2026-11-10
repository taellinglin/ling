# Crate Overview

Ling is written in Rust and split into a workspace of crates. The main crate,
`ling-lang`, contains the compiler front end, the interpreter, the JIT driver and the
`ling` command; the crates below supply the language pieces and the builtin
libraries. You only need this page if you want to use these crates from your own
Rust code, or to understand which crate backs which builtins.

The versions below are the ones in the repository at the time of writing. Most
crates follow Ling's `2030.x` numbering; `ling-http` and `ling-web` have their own.

## The main crate

| Crate | Version | What it is |
|---|---|---|
| `ling-lang` | 2030.1.53 | The language: lexer, parser, interpreter, Cranelift JIT and AOT, and the builtins. Library name `ling`; binaries `ling`, `lingc`, `lx`, `ling-repl`. |
| `lingfu` (`crates/ling-fu`) | 2030.1.19 | The project manager: `lingfu new`, `run`, `build`, `normalize` and more. See [lingfu](../tools/lingfu.md). |

## Compiler

| Crate | Version | What it is |
|---|---|---|
| `ling-core` | 2030.0.9 | Core types, errors and compiler infrastructure. |
| `ling-lex` | 2030.0.8 | Lexical analysis and polyglot tokenization. |
| `ling-ast` | 2030.0.8 | Abstract syntax tree types. |
| `ling-polyglot` | 2030.0.8 | The lexicon system for keywords and builtins in 16+ human languages. |
| `ling-mir` | 2030.0.12 | Mid-level intermediate representation for the code generators. |
| `ling-codegen` | 2030.0.13 | Code generation backends (bytecode, WASM, native). |
| `ling-macro` | 2030.0.8 | Procedural macros for the Ling ecosystem. |
| `ling-runtime` | 2030.0.8 | Runtime support: garbage collector, allocator, standard library. |
| `ling-wasm` | 2030.0.16 | The WebGL2 WebAssembly runtime used by `ling build --platform web`. |

## Graphics, UI and input

| Crate | Version | What it is |
|---|---|---|
| `ling-graphics` | 2030.0.13 | 3D/4D rendering, geometry, animation and font tools. |
| `ling-gl` | 2030.0.10 | GPU rasteriser for projected scenes (wgpu: Vulkan, Metal, DX12; WebGL2 on wasm). |
| `ling-gpu` | 2030.0.10 | GPU compute (CUDA via cudarc, with a CPU fallback). |
| `ling-ui` | 2030.0.9 | Retained-mode widgets and flex layout (`ui_*`). |
| `ling-animation` | 2030.0.8 | Anima, the unified animation system: one rig, any target. |
| `ling-input` | 2030.0.9 | Sensorium: gamepads, joysticks, haptics, motion, XR controllers, touch controls. |
| `ling-paint` | 2030.0.1 | LingAnime paint engine: layers, brushes, vectors, halftone, 2.5D rigs, pen tablets (`art_*`). |
| `ling-icon` | 2030.0.9 | Rasterizes SVG/PNG icons to multi-resolution `.ico` and embeds them in Windows executables (`ling build --icon`). |

## Games and physics

| Crate | Version | What it is |
|---|---|---|
| `ling-game` | 2030.1.9 | Game engine: ECS, physics and scene management. |
| `ling-physics` | 2030.1.18 | Physics, world generation, hyperbolic geometry, glTF loading. |
| `ling-body` | 2030.0.2 | Acoustic bodies: humanoid SDF scaffolds, FDTD wave solver, membranes, mesh and WAV codecs. |
| `ling-emu` | 2030.0.1 | Console-emulation host (`emu_*`): SNES and N64 cores. |

## Audio and music

| Crate | Version | What it is |
|---|---|---|
| `ling-audio` | 2030.0.13 | 4D positional audio synthesis and WAV background music. |
| `ling-music` | 2030.0.9 | Music decoding, BPM and key analysis, synthesis, rhythm-game and karaoke tools. |
| `ling-mic` | 2030.0.12 | Microphone input and real-time audio capture. |
| `ling-daw` | 2030.0.2 | DAW engine behind LingStationX: synth, drums, effects, mixer, sequencer. |
| `ling-osdaw` | 2030.0.2 | LingStation for LingOS: a `no_std` tracker sequencer and synthesizer. |

## Network, web and security

| Crate | Version | What it is |
|---|---|---|
| `ling-http` | 0.1.8 | HTTPS/JSON/SQLite framework on axum. Ling's `http_serve` runs on it. See [ling-http and ling-web](web.md). |
| `ling-web` | 0.1.9 | The Flask-style layer on `ling-http`: sessions, flash messages, templates, static files, blueprints, CSRF. |
| `ling-net` | 2030.0.9 | Async networking: HTTP, WebSocket, QUIC and peer-to-peer. |
| `ling-crypto` | 2030.1.13 | Post-quantum and classical cryptography: ML-KEM-768, hybrid KEM, AES-GCM, XChaCha20, BLAKE3, SHA-3, Argon2id, Shamir, Schnorr ZKP, VRF. `no_std`-capable. |

## AI

| Crate | Version | What it is |
|---|---|---|
| `ling-ai` | 2030.0.10 | Tensors, neural networks and LLM inference. |
| `ling-llm` | 2030.0.2 | A `no_std` GGUF inference engine for Qwen3 that runs on the CPU without an operating system (for LingOS). |
| `ling-diffusion` | 2030.0.1 | In-process Stable Diffusion (SD 1.x / SDXL) behind the `sd_*` builtins. |

## LingOS

| Crate | Version | What it is |
|---|---|---|
| `ling-kernel` | 2030.1.37 | Minimal kernel runtime: `no_std` CPU intrinsics, VGA, serial, boot. |
| `ling-user` | 2030.1.4 | Userland runtime for Ling programs on LingOS. |

## Bindings

| Crate | Version | What it is |
|---|---|---|
| `ling-py` | 2030.0.8 | Python bindings for `ling-audio`, `ling-physics` and `ling-net`. |

## Which profile pulls in which crate

The [install profiles](../getting-started/install.md#install-profiles) decide which of
these crates are compiled into `ling`:

| Profile | Crates |
|---|---|
| `web` | `ling-http`, `ling-crypto`, `ling-icon`, `lingfu` |
| `privacy` | `web` plus `ling-net`, `ling-ui` |
| `game` | `ling-physics`, `ling-gpu`, `ling-game`, `ling-animation`, `ling-input`, `ling-body`, `ling-ui`, `ling-icon` |
| `ai` | `ling-ai`, `ling-music` |
| `audio` | `ling-audio`, `ling-music`, `ling-mic`, `ling-daw` |
| `art` (in `full`) | `ling-paint` |
