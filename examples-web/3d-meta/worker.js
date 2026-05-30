// worker.js — runs the Ling interpreter inside a Web Worker
// The main thread transfers an OffscreenCanvas; the WASM runtime renders to it via WebGL2.

importScripts('./ling_wasm.js');

let wasmReady = false;
let pendingCanvas = null;
let pendingSource = null;

// Queue both canvas and source until wasm is instantiated — calling exported
// functions before the wasm_bindgen() promise resolves fails with "wasm is undefined".
wasm_bindgen('./ling_wasm_bg.wasm').then(() => {
    wasmReady = true;
    if (pendingCanvas !== null) {
        wasm_bindgen.init_canvas(pendingCanvas);
        pendingCanvas = null;
    }
    if (pendingSource !== null) {
        wasm_bindgen.run_program(pendingSource);
        pendingSource = null;
    }
});

self.onmessage = function(e) {
    const { type } = e.data;
    if (type === 'init') {
        if (wasmReady) {
            wasm_bindgen.init_canvas(e.data.canvas);
        } else {
            pendingCanvas = e.data.canvas;
        }
    } else if (type === 'run') {
        if (wasmReady) {
            wasm_bindgen.run_program(e.data.source);
        } else {
            pendingSource = e.data.source;
        }
    }
};
