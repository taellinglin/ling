// crates/ling-wasm/src/lib.rs — wasm-bindgen entry points for the Ling WebGL runner.
#[cfg(test)]
mod tests;

use wasm_bindgen::prelude::*;

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
/// The key parameter should be event.key (e.g., "w", "ArrowUp", "Enter", " " for space).
#[wasm_bindgen(js_name = "on_key_down")]
pub fn on_key_down(key: &str) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_key_down(key);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = key;
}

/// Notify Ling of a keyup event. Call this from JavaScript's keyup event listener.
/// The key parameter should be event.key (e.g., "w", "ArrowUp", "Enter", " " for space).
#[wasm_bindgen(js_name = "on_key_up")]
pub fn on_key_up(key: &str) {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_key_up(key);
    #[cfg(not(target_arch = "wasm32"))]
    let _ = key;
}

/// Clear the per-frame key press state. Call this at the start of each frame
/// before processing input.
#[wasm_bindgen(js_name = "clear_frame_keys")]
pub fn clear_frame_keys() {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::wasm_clear_frame_keys();
}

/// Resume the Web Audio AudioContext. Must be called from a user-gesture handler
/// (click, keydown, etc.) to satisfy the browser's autoplay policy.
/// After this call `music_play` and `set_tone` will produce audible output.
#[wasm_bindgen(js_name = "audio_ready")]
pub fn audio_ready() {
    #[cfg(target_arch = "wasm32")]
    ling::gfx::audio_resume();
}
