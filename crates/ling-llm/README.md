# ling-llm

A freestanding Qwen3 inference engine: `no_std` + `alloc` + `libm`, no operating system, no GPU. It reads the model, its tokenizer and its
vocabulary out of one GGUF file, runs it on the CPU, and streams text. It exists so **LingOS can ask a language model a question with
nothing else installed**, but it runs anywhere Rust does (it is also what the host tests compare against `candle`).

* `gguf`   the container, read through a `Source` trait (a file, a disk image, bytes in memory)
* `quant`  F32 / F16 / Q8_0 / Q4_K / Q6_K tensors, held in chunks of at most 8 MiB, and the matrix products (Q8_0 in integers against
           Q8_0 activations, the way llama.cpp and candle do it; K-quants dequantized a block at a time)
* `tok`    byte-level BPE built from the GGUF's own tokens and merges, with Qwen's pre-tokenizer written out as a scanner
* `model`  the Qwen3 transformer with an f16 KV cache; prompts go through in batches so each weight row is read once per batch
* `sample` greedy and temperature / top-p
* `chat`   the ChatML prompt (the same one `ling-ai` builds) and the generation loop

## Checked against the real thing

`cargo test -p ling-llm --features std --release --test vs_candle -- --nocapture` loads `assets/models/Qwen3-0.6B-Q8_0.gguf` and compares:
the tokenizer with the HF `tokenizers` crate on 14 awkward strings (Unicode, specials, contractions, digits, tabs) - identical; the first
logits against candle's quantized Qwen3 - cosine 0.9985; ten greedy tokens - identical ("1. Blue / 2. Red / 3."). On a desktop CPU,
built for generic x86-64, it does about 16 tokens/s with the 0.6B model.

`cargo check -p ling-llm --target x86_64-unknown-none` builds it bare-metal.

## In LingOS

`crates/ling-kernel/src/drivers/llm.rs` loads the weights from a raw second IDE disk and runs generation in a background kernel task;
the Terminal gets `ask <question>` and `llm [status|load|stop]`. See LingOS's README.
