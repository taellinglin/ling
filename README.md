# 🌐 Ling – The Omniglot Systems Language

[![Rust](https://img.shields.io/badge/rust-100%25-orange)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-Ling%20Harmony%201.0-blue)](LICENSE)
[![Stars](https://img.shields.io/github/stars/taellinglin/ling)](https://github.com/taellinglin/ling/stargazers)

**Write once, speak to everyone. Performance of Rust, poetry of human language.**

Ling is a **polyglot systems programming language** that lets you write high-performance code using your native lexicon. Whether you think in English, 中文, 한국어, or ภาษาไทย – Ling understands you.

## ✨ The Vision

Three pillars. One language. Infinite possibilities.

| Pillar | Domain | Technologies |
|--------|--------|--------------|
| 🎮 **Pillar 1** | Graphics & Games | wgpu, bevy, xilem, taffy |
| 🧠 **Pillar 2** | AI/ML | candle, burn, llama.cpp |
| 🔐 **Pillar 3** | Cryptography | Post-quantum, ZK, FHE |

## 🚀 Quick Start

```bash
# Install from crates.io
cargo install ling

# Run your first program (English)
echo 'bind start = do { print("Hello, 世界!") }' | ling run

# Run your first program (中文)
echo '令 启动 = 执行 { 印("你好，世界！") }' | ling run
```

# Code Examples in 4 Languages
## Hello World & Fibonacci
### English
```
// Hello World - English Lexicon
bind start = do {
    print("Hello, World!");
    
    // Fibonacci sequence
    let fib = func (n) {
        if n <= 1 {
            return n;
        } else {
            return fib(n - 1) + fib(n - 2);
        }
    };
    
    print("First 10 Fibonacci numbers:");
    for i in range(0, 10) {
        print(fib(i));
    }
}
```
# 中文 (Chinese)
```
// 你好世界 - 中文词库
令 启动 = 执行 {
    印("你好，世界！");
    
    // 斐波那契数列
    令 斐波那契 = 函数 (n) {
        如果 n <= 1 {
            返回 n;
        } 否则 {
            返回 斐波那契(n - 1) + 斐波那契(n - 2);
        }
    };
    
    印("前10个斐波那契数:");
    对于 i 于 范围(0, 10) {
        印(斐波那契(i));
    }
}
```
# 한국어 (Korean)

```
// 헬로 월드 - 한국어 렉시콘
바인드 시작 = 수행 {
    인쇄("안녕하세요, 세계!");
    
    // 피보나치 수열
    렛 피보나치 = 펑크 (n) {
        이프 n <= 1 {
            리턴 n;
        } 엘스 {
            리턴 피보나치(n - 1) + 피보나치(n - 2);
        }
    };
    
    인쇄("처음 10개의 피보나치 수:");
    포 i 인 레인지(0, 10) {
        인쇄(피보나치(i));
    }
}

## ภาษาไทย (Thai)
```
// สวัสดีชาวโลก - พจนานุกรมไทย
ผูก เริ่มต้น = ทำ {
    พิมพ์("สวัสดีชาวโลก!");
    
    // ลำดับฟีโบนัชชี
    ให้ ฟีโบ = ฟังก์ชัน (n) {
        ถ้า n <= 1 {
            คืนค่า n;
        } มิฉะนั้น {
            คืนค่า ฟีโบ(n - 1) + ฟีโบ(n - 2);
        }
    };
    
    พิมพ์("ตัวเลขฟีโบนัชชี 10 ตัวแรก:");
    สำหรับ i ใน พิสัย(0, 10) {
        พิมพ์(ฟีโบ(i));
    }
}
```
## 🎯 Core Features
### 1. 16 Native Lexicons (Simultaneous)

No #lang directives. No switching contexts. Write English functions alongside Chinese variables alongside Korean loops. All in the same file.
### 2. Zero-Cost Abstractions

    Borrow checker with effects system

    No garbage collection overhead

    Compiles to native code via LLVM/Cranelift/WASM

### 3. Full-Stack Ready
```
// Example: Game + AI + Crypto hybrid
bind start = do {
    let model = load_llm("llama.cpp");
    let key = generate_post_quantum_key();
    render_game_loop(model, key);
}
```
# 📁 Project Structure

ling/
├── src/              # Core compiler
├── crates/
│   ├── ling-core/    # Data structures & types
│   ├── ling-lex/     # Lexicon tokenization
│   ├── ling-polyglot/# Multi-language infra
│   ├── ling-mir/     # Intermediate representation
│   ├── ling-ai/      # AI/LLM integration
│   ├── ling-crypto/  # Cryptographic primitives
│   └── ling-ui/      # UI/game framework
├── lexicons/         # Language definitions
└── examples/         # Sample programs

# Building & Testing
## Build the workspace
```
cargo build
```

# Run the REPL
```
cargo run --bin ling-repl
```

# Run tests
```
cargo test
```

# Compile a Ling file
```
cargo run --bin lingc -- program.ling -o output
```

## 🗺️ Roadmap 2030

-    Core compiler pipeline (lex → ast → semantics → borrowck → mir → codegen)

-    Polyglot lexicons (en/zh/ja/ko/ru/ar/hi/th)

-    LLM integration (code completion + semantic suggestions)

-    Game engine (bevy + physics + tooling)

-    UI framework (xilem + taffy + design system)

## 🤝 Contributing

### We welcome contributions! Typical workflow:

    Fork the repo

    Create a feature branch (git checkout -b feature/amazing)

    Implement + add tests

    Submit a pull request

Check TODO.md for open tasks and good first issues.
📄 License

Ling Harmony License 1.0 – Free for open-source and personal use. Commercial licensing available upon request.
🌟 Show Your Support

If you find Ling interesting, star the repository and spread the word!

Built with 🦀 Rust and ❤️ for human languages