# Introduction

Ling is a programming language you can write in your own language. Its keywords and
builtins have names in more than 16 human languages, including Chinese, Japanese,
Korean, Thai, English, Arabic, Hebrew and Russian, and you can mix them in the same
file. These docs cover five of them: English, 中文, 日本語, 한국어 and ไทย. Pick yours
from the language selector at the top of the page.

This guide covers Ling 2030.1.53.

## A first look

Here is a small program. `bind start = do { ... }` is where every program begins.

```ling
# Greet a few friends
bind start = do {
    bind friends = ["Ada", "Lin", "Sora"]
    for name in friends {
        print("Hello, " + name + "!")
    }
    print(len(friends))
}
```

```output
Hello, Ada!
Hello, Lin!
Hello, Sora!
3
```

The same program works with Chinese, Japanese, Korean or Thai keywords. Switch the page
language to see it, or read [Writing in Five Languages](language/multilingual.md).

## What you get

One `ling` binary includes:

- **A JIT compiler and an interpreter.** Programs compile to native code through
  Cranelift, with an interpreter for everything else.
- **Graphics and 3D.** A window, 2D drawing, images, fonts, a software 3D renderer with
  cameras, lights, cel shading and vector geometry (`vtex_*`).
- **Audio and music.** Tones, spatial sound, WAV playback, music decoding, BPM and key
  analysis, a GM-style synth and MIDI.
- **Physics.** Rigid bodies, soft bodies and a liquid simulation.
- **UI.** HUDs, meters, buttons and other widgets.
- **A web server.** Routes, static files and SQLite through the `http_*` and `db_*` builtins.
- **Cryptography.** Hashes, encryption, signatures and post-quantum key exchange.
- **AI.** Neural networks, behaviour trees and optional language-model support.

`lingfu`, the project manager, creates, runs and tests projects, and `lingfu normalize`
rewrites a whole project into one language.

## Install

On macOS or Linux:

```sh
curl --proto '=https' --tlsv1.2 -sSf https://ling-lang.org/install.sh | sh
```

On Windows (PowerShell):

```powershell
irm https://ling-lang.org/install.ps1 | iex
```

The script installs Rust if you don't have it, then builds `ling` and `lingfu`. See
[Installation](getting-started/install.md) for other options and install profiles.

## Where to go next

- **[Getting Started](getting-started/install.md):** install Ling, run Hello World and
  build your first 3D room.
- **[Tutorials](tutorials/values.md):** learn the language step by step, from values to
  games, music and a web service.
- **[Language Guide](language/syntax.md):** syntax, keywords, types, errors, modules and
  known limits.
- **[Builtins Reference](reference/index.md):** every builtin, grouped by topic.
- **[The Ling Book](books.md):** four free PDF books, from beginner to complete reference.
- **[Showcase](showcase.md):** apps and demos built with Ling.
