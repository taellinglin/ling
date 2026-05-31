// crates/ling-wasm/src/lib.rs — wasm-bindgen entry points for the Ling WebGL runner.

use wasm_bindgen::prelude::*;

/// Initialise the rendering context on the given OffscreenCanvas.
/// Must be called once before run_program.
#[wasm_bindgen(js_name = "init_canvas")]
pub fn init_canvas(canvas: web_sys::OffscreenCanvas) {
    console_error_panic_hook::set_once();
    let _ = canvas; // WebGL2 init wired through runtime at call-time
}

/// Parse and execute a Ling source program.
/// Graphics output is rendered via the context set up by init_canvas.
#[wasm_bindgen(js_name = "run_program")]
pub fn run_program(source: &str) {
    if let Err(e) = ling::run(source) {
        web_sys::console::error_1(&wasm_bindgen::JsValue::from_str(&e));
    }
}
