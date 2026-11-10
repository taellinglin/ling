# Installation

## One-line install

```sh
# macOS / Linux
curl --proto '=https' --tlsv1.2 -sSf https://ling-lang.org/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://ling-lang.org/install.ps1 | iex
```

The scripts install a Rust toolchain if you don't have one (the same way
[rustup](https://rustup.rs) does), then build `ling` and `lingfu` from crates.io.

## From crates.io

```sh
cargo install ling-lang lingfu
```

## From source

```sh
git clone https://github.com/taellinglin/ling
cd ling
cargo build --release
# binary at target/release/ling
```

## Install profiles

`cargo install ling-lang` installs everything (the `full` profile). To build
less, turn the default off and choose profiles:

```sh
cargo install ling-lang --no-default-features --features core        # compiler + interpreter only
cargo install ling-lang --no-default-features --features web         # + HTTP server/client, SQLite
cargo install ling-lang --no-default-features --features game,audio  # a game kit with sound
```

| Profile | What it adds |
|---|---|
| `core` | Lexer, parser, JIT/AOT, interpreter, software window and drawing. Every profile includes it. |
| `web` | `http_*` server and client ([Web Server & HTTP](../reference/web.md)), `db_*` SQLite, static files and virtual hosts, zip/tar, video, processes. |
| `privacy` | Post-quantum identities, `lingtp://`, peer-to-peer, Ling Wallet, SSH, plus `web`. |
| `game` | Physics, GPU raster, animation, dialogs, gamepads, UI widgets. |
| `ai` | Neural nets, behaviour trees, speech-to-text. |
| `audio` | Sound engine, music, microphone, the LingStationX DAW. |
| `os` | LingOS kernel and userland images. |
| `full` | All of the above (the default). |

A program that calls a builtin your build leaves out stops with an error.
For example, `unknown function 'http_static'` means the `web` profile is
missing.

## Requirements

- Rust 1.75+
- On Linux: `libasound2-dev` (for audio)
- On macOS/Windows: no extra dependencies

## Run a script

```sh
ling run examples/basics/thai_hello_world.ling          # Cranelift JIT (default)
ling run --interp examples/basics/thai_hello_world.ling # tree-walking interpreter
```

When something "does nothing", run it once with `--interp`: the interpreter
reports unknown names that the JIT silently ignores.

## Learn more

The [Ling Book](../books.md) is a free four-volume PDF series: Beginner,
Intermediate, Advanced, and a complete Reference.
