# TODO - make `cargo run --bin ling` compile

## Step 1: Identify missing modules/types
- Inspect current `src/*/mod.rs` module declarations that refer to non-existent files.
- Inspect `src/lib.rs` re-exports that refer to non-existent items.

## Step 2: Add feature gating to keep minimal build path
- In `src/codegen/mod.rs`, gate `llvm`/other backends behind features, and provide a stub `Codegen` API if needed.
- In `src/polyglot/mod.rs`, gate normalization/script detection behind features; provide stubs for `normalize_source`.
- In `src/runtime/mod.rs` and `src/utils/mod.rs`, gate missing submodules and/or add minimal placeholder modules.

## Step 3: Stub missing internal modules referenced by `mod.rs`
- Fix `src/mir/mod.rs` missing `build/simplify/optimize` modules (remove for now or create minimal stubs).
- Add minimal module files for `src/codegen/{cranelift,wasm,type_mapping}.rs` if required for compilation.

## Step 4: Fix core/utility missing exports
- Ensure `core::OptimizationLevel`, `core::LingResult`, `core::LingError` exist (add stubs if not).
- Ensure `lexicon::Lexicon`, `lexicon::CanonicalToken` exist (or gate lexicon integration for now).

## Step 5: Fix parser visibility + result mismatch
- Make `parser::ast::Program` publicly accessible or adjust imports.
- Fix `src/parser/mod.rs` `Result<Program, ()>` vs `Result<Program, String>` mismatch.

## Step 6: Add minimal dependencies or remove usage
- Either add required dependencies to `Cargo.toml` (only for minimal path) OR gate code that uses missing crates (e.g., `inkwell`, `phf`, `once_cell`, `unicode_normalization`, `thiserror`, `lalrpop_util`).

## Step 7: Compile/test loop
- Run `cargo check --bin ling`.
- Run `cargo run --bin ling`.
- Repeat until success.

