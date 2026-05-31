# Installation

## From crates.io

```sh
cargo install ling-lang
```

## From source

```sh
git clone https://github.com/taellinglin/ling
cd ling
cargo build --release
# binary at target/release/ling
```

## Requirements

- Rust 1.75+
- On Linux: `libasound2-dev` (for audio)
- On macOS/Windows: no extra dependencies

## Run a script

```sh
ling examples/hello.ling
```
