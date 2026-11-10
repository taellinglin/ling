// crates/ling-wasm/src/lib.rs — wasm-bindgen entry points for the Ling WebGL runner.
#[cfg(test)]
mod tests;

use wasm_bindgen::prelude::*;

/// The physical display size (device px) the page shows the game on — read by
/// the `display_size()` builtin to size the UI layer for crisp pixel text.
#[wasm_bindgen(js_name = "set_display")]
pub fn set_display(w: f64, h: f64) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_set_display(w, h);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (w, h);
}

/// Resize the game's framebuffer (applied at the next frame boundary).
#[wasm_bindgen(js_name = "resize_canvas")]
pub fn resize_canvas(w: u32, h: u32) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_request_resize(w as usize, h as usize);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (w, h);
}

/// Initialise the rendering context on the given OffscreenCanvas.
/// Must be called once before run_program.
#[wasm_bindgen(js_name = "init_canvas")]
pub fn init_canvas(canvas: web_sys::OffscreenCanvas) {
    console_error_panic_hook::set_once();
    #[cfg(target_arch = "wasm32")]
    ling::gfx::webgl::init_canvas(canvas);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = canvas;
}

/// Turn the WebGL2 triangle path on or off (default on; falls back to the CPU rasteriser by itself
/// if the shaders don't compile). Call before `run_program`.
#[wasm_bindgen(js_name = "set_gpu_scene")]
pub fn set_gpu_scene(on: bool) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::webgl_scene::set_enabled(on);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = on;
}

/// Register a module source so that `use "path"` statements resolve on wasm32.
/// Call this for every module before calling `run_program`.
#[wasm_bindgen(js_name = "register_module")]
pub fn register_module(path: &str, source: &str) {
    #[cfg(target_arch = "wasm32")]
    ling::runtime::register_wasm_module(path, source);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (path, source);
}

/// Parse and execute a Ling source program.
/// All modules used via `use "..."` must have been registered with
/// `register_module` first (on wasm32).
#[wasm_bindgen(js_name = "run_program")]
pub fn run_program(source: &str) {
    if let Err(e) = ling::run(source) {
        web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(&e));
    }
}

/// Notify Ling of a keydown event. Call this from JavaScript's keydown event listener.
#[wasm_bindgen(js_name = "on_key_down")]
pub fn on_key_down(key: &str) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_key_down(key);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = key;
}

/// Notify Ling of a keyup event. Call this from JavaScript's keyup event listener.
#[wasm_bindgen(js_name = "on_key_up")]
pub fn on_key_up(key: &str) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_key_up(key);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = key;
}

/// Clear the per-frame key press state. Call this at the start of each frame.
#[wasm_bindgen(js_name = "clear_frame_keys")]
pub fn clear_frame_keys() {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_clear_frame_keys();
}

/// Notify Ling of pointer movement.
#[wasm_bindgen(js_name = "on_mouse_move")]
pub fn on_mouse_move(x: f32, y: f32) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_mouse_move(x, y);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (x, y);
}

/// Notify Ling of mouse button press/release.
#[wasm_bindgen(js_name = "on_mouse_button")]
pub fn on_mouse_button(button: u32, pressed: bool, x: f32, y: f32) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_mouse_button(button, pressed, x, y);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (button, pressed, x, y);
}

/// Notify Ling of a gamepad button state change from JS Gamepad API.
#[wasm_bindgen(js_name = "on_gamepad_button")]
pub fn on_gamepad_button(index: u32, button: &str, pressed: bool, value: f32) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_gamepad_button(index, button, pressed, value);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (index, button, pressed, value);
}

/// Notify Ling of a gamepad axis analog change from JS Gamepad API.
#[wasm_bindgen(js_name = "on_gamepad_axis")]
pub fn on_gamepad_axis(index: u32, axis: u32, value: f32) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_gamepad_axis(index, axis, value);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (index, axis, value);
}

/// Notify Ling when a gamepad connects in JS.
#[wasm_bindgen(js_name = "on_gamepad_connected")]
pub fn on_gamepad_connected(index: u32, name: &str) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_gamepad_connected(index, name);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (index, name);
}

/// Notify Ling when a gamepad disconnects in JS.
#[wasm_bindgen(js_name = "on_gamepad_disconnected")]
pub fn on_gamepad_disconnected(index: u32) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_gamepad_disconnected(index);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = index;
}

/// Resume the Web Audio AudioContext. Must be called from a user-gesture handler.
#[wasm_bindgen(js_name = "audio_ready")]
pub fn audio_ready() {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::audio_resume();
}

/// Queue raw PCM audio samples for playback through Web Audio API.
#[wasm_bindgen(js_name = "play_audio_buffer")]
pub fn play_audio_buffer(pcm_data: &[f32], sample_rate: u32) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_play_audio_buffer(pcm_data, sample_rate);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = (pcm_data, sample_rate);
}

/// Set master output volume (0.0 to 1.0) for Web Audio engine.
#[wasm_bindgen(js_name = "set_master_volume")]
pub fn set_master_volume(volume: f32) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_set_master_volume(volume);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = volume;
}

/// Hand a streamed asset's bytes to the runtime (answers an `asset_request`).
#[wasm_bindgen(js_name = "register_asset")]
pub fn register_asset(path: &str, bytes: &[u8]) {
    ling::assets::put(path, bytes.to_vec());
}

/// Drop a streamed asset from memory (proximity eviction).
#[wasm_bindgen(js_name = "drop_asset")]
pub fn drop_asset(path: &str) {
    ling::assets::remove(path);
}

/// Newline-separated paths the running program has requested since the last call.
#[wasm_bindgen(js_name = "take_asset_requests")]
pub fn take_asset_requests() -> String {
    ling::assets::take_requests()
}

/// [count, bytes] currently resident.
#[wasm_bindgen(js_name = "asset_stats")]
pub fn asset_stats() -> Vec<f64> {
    vec![ling::assets::count() as f64, ling::assets::bytes_total() as f64]
}

/// Typed, pasted or IME-composed text for `text_poll` (send "\u{8}" for Backspace).
#[wasm_bindgen(js_name = "on_text_input")]
pub fn on_text_input(text: &str) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_text_input(text);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = text;
}
