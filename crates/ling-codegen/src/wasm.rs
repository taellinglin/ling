//! Ling → WebAssembly (binary format) emitter.

use crate::{CodegenBackend, MirProgram};

/// Minimal WASM binary writer.
pub struct WasmBackend {
    buf: Vec<u8>,
}

impl WasmBackend {
    pub fn new() -> Self {
        let buf = vec![
            0x00, 0x61, 0x73, 0x6D, // magic: \0asm
            0x01, 0x00, 0x00, 0x00, // version: 1
        ];
        Self { buf }
    }
}

impl Default for WasmBackend {
    fn default() -> Self {
        Self::new()
    }
}

impl CodegenBackend for WasmBackend {
    fn emit(&mut self, _mir: &MirProgram, out: &std::path::Path) -> anyhow::Result<()> {
        std::fs::write(out.with_extension("wasm"), &self.buf)?;
        Ok(())
    }
}
