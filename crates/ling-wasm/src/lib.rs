// crates/ling-wasm/src/lib.rs — wasm-bindgen entry points for the Ling WebGL runner.

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
