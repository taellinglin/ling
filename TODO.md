# oTODO

## Ling: make `ling run examples/hello.ling` print output
- [ ] Implement minimal lexer for subset used by examples/hello.ling (string literals, identifiers, keywords: bind/do/print/start)
- [ ] Implement minimal parser for subset: `bind <name> = do { <stmts> }` with nested `bind <name> = <expr>` and `print(<expr>)`
- [ ] Implement small interpreter that evaluates this subset and prints to stdout
- [x] Wire CLI `ling run <file>` to the interpreter (enabled in this build)
- [x] Add integration test that runs `examples/hello.ling` and asserts stdout contains `Hello, World!`

## Examples: gfx compatibility (ling-graphics)
- [x] Make `examples/basics/gfx.ling` parse and run by removing unsupported `&&` conditions

## 5-Language Builtin Parity (EN · ZH · JA · KO · TH)
Goal: every runtime builtin accepts aliases in all 5 languages, mirrored in the
lexicons and `lingfu normalize` so a program normalized to any language runs
unchanged. Runtime is the **superset** — it accepts every alias any lexicon or
normalizer table can emit.

- [x] **Batch 1 — math + core** (`sin`…`tau`, `print`/`format`/`ok`/`bad`/`sleep`)
      added to runtime, lexicons (th fixes), and a new `BUILTINS_MATH` normalize
      table; verified by normalizing a math program to zh/ja/ko/th and running
      each (identical output). Docs: `docs/src/reference/builtins.md`.
- [x] **Batch 2 — graphics / window / camera / input / lighting** (24 arms:
      `open_window`, `fill`, `set_color`, `draw_line`, `draw_pixel`, `triangle`,
      `present`, `get_width/height`, `key_down/pressed`, `mouse_dx/dy`,
      `capture_mouse`, `set_camera`/`_pos`, `set_zdist`, `set_projection`,
      `set_ambient`, `add_light`, `clear_lights`, `open_fullscreen`,
      `window_is_open`, `set_color_hsl`): ZH/JA/KO aliases added to runtime
      (superset), new `BUILTINS_GFX` normalize table + reconciled `BUILTINS_OTHER`
      output forms; verified by normalizing a graphics program to zh/ja/ko/th and
      confirming every builtin resolves. Docs: `docs/src/reference/window.md`.
      (Shapes/`vtex_*` already had 5-language coverage.)
- [x] **Batch 3 — audio** (`audio_tone/volume/listener/bgm/bgm_volume`,
      `fft_push/bands/beat/beat_ratio/rms/dominant_freq`): ZH/JA/KO aliases added
      to runtime (both native + wasm cfg blocks), new `BUILTINS_AUDIO` normalize
      table; `docs/src/reference/audio.md` 5-language table. Verified by
      normalize round-trip + `tests/language_system.rs`.
- [x] **Batch 4 — collections** (`list_new/push/get/join`, `len`/`str_len`):
      ZH/JA/KO aliases + normalize entries + docs. **Crypto/physics are not Ling
      builtins** — they are Rust crate APIs (`ling-crypto`, `ling-physics`),
      covered by their own test suites; wiring them in as multilingual builtins
      remains future work (documented in `docs/src/reference/builtins.md`).
- [ ] Fill the `—` cells in the math table (Thai for `exp`/`hypot`/`log2`/`fract`,
      etc.) once preferred terms are chosen.

## Test suite & CI (tests badge)
- [x] `tests/language_system.rs` — 16 tests: multilingual hello-world (5 langs),
      math builtins in 5 langs, mixed-language file, polyglot detect, error paths.
- [x] `crates/ling-fu/tests/normalize_cli.rs` — drives the `lingfu` binary,
      normalizes a program to zh/ja/ko/th + back to `bind`-canonical English.
- [x] `crates/ling-crypto/tests/crypto_roundtrip.rs` — Blake3 + AES-GCM-256.
- [x] `crates/ling-physics/tests/vector_ops.rs` — vector algebra.
- [x] `crates/ling-audio/tests/fft_smoke.rs` — FFT analyzer.
- [x] `crates/ling-game/tests/ecs_smoke.rs` — entity/component store.
- [x] Fixed pre-existing failures: `ling-polyglot` detect heuristic (neutral
      chars no longer outweigh CJK/Thai), `ling-audio` fft doctest, stale
      `physics_demo_rust` example target in Cargo.toml.
- [x] `.github/workflows/ci.yml` rewritten (master/main triggers, system deps for
      minifb/cpal, lint non-blocking) + `tests` badge in README.
      Full workspace: **66 passed, 0 failed**.

## Requested: rotating cube “window/viewport” instance API
- [ ] Add Rust-to-language integration so `.ling` can trigger a render loop
- [ ] Add a small “software viewport” presenter (RGBA framebuffer -> terminal output)
- [ ] Add sample rotating cube program (or Rust demo) using `ling-graphics`:
  - cube mesh: `ling_graphics::geometry::cube(half)`
  - scene: `ling_graphics::scene::Scene`
  - camera: `ling_graphics::camera::Camera3D`
  - renderer: `ling_graphics::renderer::SoftwareRenderer`
  - update: rotate transform over frames

