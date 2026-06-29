// WASM integration tests — run with: wasm-pack test --headless --chrome crates/ling-wasm
//
// These tests verify that WASM-stub builtins return the correct value types so
// that Ling programs can use their return values without runtime type errors.
// The hallway_runner crash was caused by `audio_sample_load` returning Unit
// instead of Number; these tests guard all similar stubs.

#![cfg(target_arch = "wasm32")]

use wasm_bindgen_test::*;

wasm_bindgen_test_configure!(run_in_node_experimental);

// Helper: run a Ling snippet and panic with the error if it fails.
fn run(src: &str) {
    ling::run(src).unwrap_or_else(|e| panic!("Ling runtime error: {e}"));
}

// ── Audio stubs ───────────────────────────────────────────────────────────────

#[wasm_bindgen_test]
fn music_fft_returns_list() {
    // music_fft must return a list so index access doesn't type-error
    run("bind bands = music_fft(0, 8)  bind _ = bands");
}

#[wasm_bindgen_test]
fn music_pos_returns_number() {
    run("bind pos = music_pos()  bind _ = pos + 0.0");
}

#[wasm_bindgen_test]
fn audio_sample_load_returns_number() {
    // Must not return Unit — the crash case
    run("bind id = audio_sample_load(\"x.wav\")  bind _ = id + 0.0");
}

// ── AI stubs ─────────────────────────────────────────────────────────────────

#[wasm_bindgen_test]
fn nn_new_returns_number() {
    run("bind h = nn_new(3)  bind _ = h + 0.0");
}

#[wasm_bindgen_test]
fn nn_forward_returns_list() {
    run("bind out = nn_forward(0, [1.0, 0.0])  bind _ = out");
}

#[wasm_bindgen_test]
fn bt_build_returns_number() {
    run("bind h = bt_build(\"seq\")  bind _ = h + 0.0");
}

#[wasm_bindgen_test]
fn bt_tick_returns_str() {
    run("bind s = bt_tick(0)  bind _ = s");
}

#[wasm_bindgen_test]
fn bt_status_returns_number() {
    run("bind n = bt_status(0)  bind _ = n + 0.0");
}

#[wasm_bindgen_test]
fn dialog_new_returns_number() {
    run("bind h = dialog_new(3, 32, 64, 1)  bind _ = h + 0.0");
}

#[wasm_bindgen_test]
fn dialog_say_returns_str() {
    run("bind s = dialog_say(0, \"hello\")  bind _ = s");
}

// ── Network stubs ─────────────────────────────────────────────────────────────

#[wasm_bindgen_test]
fn net_recv_returns_str() {
    run("bind s = net_recv(0)  bind _ = s");
}

#[wasm_bindgen_test]
fn net_discover_returns_list() {
    run("bind peers = net_discover()  bind _ = peers");
}

// ── Gamepad stubs ─────────────────────────────────────────────────────────────

#[wasm_bindgen_test]
fn pad_poll_returns_number() {
    run("bind n = pad_poll()  bind _ = n + 0.0");
}

#[wasm_bindgen_test]
fn pad_count_returns_number() {
    run("bind n = pad_count()  bind _ = n + 0.0");
}

#[wasm_bindgen_test]
fn pad_button_returns_bool() {
    run("bind b = pad_button(0, \"south\")  if b { bind _ = 1 }");
}

#[wasm_bindgen_test]
fn pad_lx_returns_number() {
    run("bind v = pad_lx(0)  bind _ = v + 0.0");
}

// ── Gradient lighting (pure Rust, also available on native) ──────────────────
// These tests exercise the new lerp_color / compute_lit_color_vertices path.

#[wasm_bindgen_test]
fn draw_triangle_3d_no_crash_with_light() {
    // A complete frame that adds a light and draws a lit triangle.
    // If gradient lighting produces the wrong type anywhere it will crash here.
    run(r#"
clear_screen()
add_light(0.0, -10.0, 5.0, 1.0, 1.0, 1.0, 2.0)
set_color(200, 100, 50)
draw_triangle_3d(0.0, 0.0, 5.0, 1.0, 0.0, 5.0, 0.5, 1.0, 5.0)
"#);
}

#[wasm_bindgen_test]
fn draw_triangle_3d_flat_shade_no_crash() {
    run(r#"
clear_screen()
set_color(80, 160, 200)
draw_triangle_3d(-1.0, -1.0, 4.0, 1.0, -1.0, 4.0, 0.0, 1.0, 4.0)
"#);
}
