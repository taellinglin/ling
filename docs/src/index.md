# Ling — The Omniglot Systems Language

Ling is a polyglot scripting language whose keywords and builtins are available in
**16+ human languages simultaneously** — Chinese, Thai, Korean, Japanese, English,
Arabic, Hebrew, Russian, and more — in the same source file.

```ling
# All four in one file:
令 启 = 执 { 印("你好") }          # Chinese
令 เริ่ม = ดำเนินการ { พิมพ์("สวัสดี") }  # Thai
bind start = do { print("Hello") }   # English
바인드 시작 = 수행 { 인쇄("안녕") }      # Korean
```

## Core features

- **Polyglot keywords** — write loops in Chinese, functions in Thai, variables in English
- **3D software renderer** — vector geometry (`vtex_*`) and pixel textures (`tex_*`)
- **4D spatial audio** — pentatonic synthesis with positional mixing
- **FFT analysis** — frequency bands from any audio source
- **Cross-platform** — native Windows/macOS/Linux and WebGL2 WASM

## Crates

| Crate | Version | Purpose |
|-------|---------|---------|
| `ling-lang` | 2030.1.3 | Main interpreter binary |
| `ling-core` | 2030.0.0 | Core types |
| `ling-audio` | 2030.0.0 | 4D audio + FFT |
| `ling-game` | 2030.0.0 | ECS, mesh, physics |
| `ling-graphics` | 2030.0.0 | 3D/4D rendering |
| `ling-crypto` | 2030.0.0 | Cryptography |
| `ling-net` | 2030.0.0 | Async networking |
| `ling-ai` | 2030.0.0 | LLM / neural nets |

## Quick navigation

- [Syntax reference](reference/syntax.md)
- [Vector geometry builtins](reference/vtex.md)
- [Multilingual keyword table](multilingual/keywords.md)
- [All builtin aliases](multilingual/builtins.md)
