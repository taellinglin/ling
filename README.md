# Ling – The Omniglot Systems Language

**UI/3D/Game • AI/LLM • Cryptography – in perfect harmony**

[![CI](https://github.com/taellinglin/ling/actions/workflows/ci.yml/badge.svg)](https://github.com/taellinglin/ling/actions)
[![Docs](https://docs.rs/ling/badge.svg)](https://docs.rs/ling)

## Overview

Ling is a **polyglot systems language** designed for the next era of software:

```
Pillar 1: Graphics & Game    • wgpu/bevy + xilem/taffy
Pillar 2: AI/ML              • candle/burn + llama.cpp 
Pillar 3: Cryptography       • post-quantum + ZK + FHE
Core: Polyglot compiler     • English/中文/日本語/... unified
```

## Quick Start

```bash
# Install
cargo install ling

# Hello world (English)
echo 'bind start = do { print("Hello!") }' | ling run

# Hello world (中文)
echo '令 启动 = 执行 { 印("你好!") }' | ling run

# Compile
lingc hello.ling -o hello
./hello
```

## Features

- **16 lexicons** simultaneous (no `#lang`)
- **Zero-cost** borrow checker + effects system
- **LLVM/Cranelift/WASM** backends
- **Full-stack** – UI/game/AI/crypto interop

## Roadmap 2030

- [x] Core compiler pipeline (lex → ast → semantics → borrowck → mir → codegen)
- [x] Polyglot lexicons (en/zh/ja/ko/ru/ar/hi/th/...)
- [ ] LLM integration (code completion + semantic suggestions)
- [ ] Game engine (bevy + physics + tooling)
- [ ] UI framework (xilem + taffy + design system)

See full [roadmap](ROADMAP.md).

## Building & Running

### Build the workspace

```bash
cargo build
```

### Run Ling

This repo includes a small Rust CLI entry point. To run the default binary:

```bash
cargo run
```

To run the Ling REPL:

```bash
cargo run --bin ling-repl
```

To compile Ling source code (if enabled in your build configuration):

```bash
cargo run --bin lingc -- <input.ling> -o <output>
```

> Note: Some binaries/features may require additional feature flags depending on the selected backend (LLVM/WASM/etc.).

### Run tests

```bash
cargo test
```

## Project Structure

- `src/` — core compiler and language implementation
- `crates/ling-core/` — core data structures and shared types
- `crates/ling-lex/` (and lexicon files) — lexing/tokenization components
- `crates/ling-polyglot/` — polyglot infrastructure
- `crates/ling-mir/` — intermediate representation
- `crates/ling-ai/`, `crates/ling-crypto/`, `crates/ling-net/`, `crates/ling-audio/`, `crates/ling-ui/` — feature domains

## Contributing

We welcome contributions. Typical workflow:

1. Fork the repo
2. Create a feature branch
3. Implement + add tests
4. Submit a pull request

If you’re unsure where to start, check `TODO.md` and open issues.

## License

Ling Harmony License 1.0

