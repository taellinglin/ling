# Installation

You can install Ling with one command. You get two programs: `ling`, which runs
and builds Ling programs, and `lingfu`, the project manager.

## One-line install

```sh
# macOS / Linux
curl --proto '=https' --tlsv1.2 -sSf https://ling-lang.org/install.sh | sh
```

```powershell
# Windows (PowerShell)
irm https://ling-lang.org/install.ps1 | iex
```

The script installs a Rust toolchain first if you don't have one (the same way
[rustup](https://rustup.rs) does). Then it builds `ling` and `lingfu` from crates.io.

## With Cargo

If you already have Rust, install both programs from crates.io:

```sh
cargo install ling-lang lingfu
```

Both end up in the same folder (`~/.cargo/bin`). `lingfu` looks for `ling` next to
itself first, then on your `PATH`.

## From source

```sh
git clone https://github.com/taellinglin/ling
cd ling
cargo build --release
# the binaries are in target/release/ (ling, plus lingc, lx and ling-repl)
```

To build `lingfu` from the same checkout, run `cargo build --release -p lingfu`.

## Check that it works

Run `ling` with no arguments. The first line of the usage text shows the version:

```sh
ling
# ling 2030.1.53 — The Omniglot Systems Language
# Usage:
#   ling run <file.ling>   ...
lingfu --version
# lingfu v2030.1.19
```

`ling` has no `--version` flag; the usage text is the way to see it.

## Install profiles

`cargo install ling-lang` installs everything (the `full` profile). To build
less, turn the default off and choose the profiles you need:

```sh
cargo install ling-lang --no-default-features --features core        # compiler + interpreter only
cargo install ling-lang --no-default-features --features web         # + HTTP server/client, SQLite
cargo install ling-lang --no-default-features --features game,audio  # a game kit with sound
```

| Profile | What it adds |
|---|---|
| `core` | Lexer, parser, Cranelift JIT and AOT, the interpreter, the software window and drawing (`draw_*`, fonts, images, meshes). Every profile includes it. |
| `web` | `http_*` server and client ([Web Server and HTTP](../reference/web.md)), `db_*` SQLite ([Database](../reference/database.md)), static files, TOTP, zip/tar, video playback, processes, hashes and `password_hash`. |
| `privacy` | Post-quantum identities, `lingtp://`, peer-to-peer `p2p_*`, Ling Wallet `lw_*`, `ssh_*`, plus `web` and the `ui_*` widgets. |
| `game` | Physics (soft, rigid, liquid, glTF), GPU raster, animation, dialogs, gamepads, acoustic bodies, `ui_*` widgets. |
| `ai` | Neural networks (`nn_*`), behaviour trees (`bt_*`), learned dialog and speech-to-text (`stt_*`). |
| `audio` | Sound engine, music decoding, synthesis and MIDI, microphone, the LingStationX DAW. |
| `os` | `ling build --platform kernel`, `rpi`, `lingos`, `lingos-rpi` (LingOS images). |
| `full` | All of the above (the default). It also includes `art`, the LingAnime paint engine (`art_*`). |

If a program calls a builtin from a profile your build leaves out, it stops with an
error that names the profile to reinstall with. Some heavy extras are opt-in on top
of any profile: `llm`, `llm-cuda`, `vision`, `cuda`, `gpu-cuda`, `gpu`, `llvm`, `emu`.

## Requirements

- Rust 1.85 or newer (the install script sets it up for you).
- On Linux: `libasound2-dev` for audio.
- On macOS and Windows: nothing extra.
- `ling build` for desktop targets compiles a small Rust project, so it also needs the
  Rust toolchain. See [The ling Command](../tools/ling.md).

## Next steps

- Write your first program: [Hello World](hello.md).
- Learn the commands: [Running Programs](running.md).
- Read the [Ling Book](../books.md), a free four-volume PDF series: Beginner,
  Intermediate, Advanced and a complete Reference.
