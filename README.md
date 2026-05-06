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

- [x] Core compiler (lex/ast/semantics/borrowck/mir/codegen)
- [x] Polyglot lexicons (en/zh/ja/ko/ru/ar/hi/th/...)
- [ ] LLM integration (code completion)
- [ ] Game engine (bevy + physics)
- [ ] UI framework (xilem + taffy)

See full [roadmap](ROADMAP.md).

## License

Ling Harmony License 1.0
