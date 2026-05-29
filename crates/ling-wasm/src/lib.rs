// crates/ling-wasm/src/lib.rs — wasm-bindgen entry points for the Ling WebGL runner.
//
// Usage (from JavaScript / worker.js):
//   await wasm_bindgen('./ling_wasm_bg.wasm');
//   wasm_bindgen.init_canvas(offscreenCanvas);
//   wasm_bindgen.run_program(sourceCode);

use wasm_bindgen::prelude::*;

/// Initialise the WebGL2 context on the given OffscreenCanvas.
/// Must be called once before run_program.
#[wasm_bindgen(js_name = "init_canvas")]
pub fn init_canvas(canvas: web_sys::OffscreenCanvas) {
    console_error_panic_hook::set_once();
    ling::gfx::webgl::init(canvas);
}

/// Parse and execute a Ling source program.
/// Graphics output is rendered via the WebGL2 context set up by init_canvas.
#[wasm_bindgen(js_name = "run_program")]
pub fn run_program(source: &str) {
    if let Err(e) = ling::run(source) {
        web_sys::console::error_1(&e.into());
    }
}
