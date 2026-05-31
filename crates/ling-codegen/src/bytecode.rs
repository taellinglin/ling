//! Ling bytecode emitter — compact stack-VM instruction stream.

use crate::{CodegenBackend, MirProgram};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Op {
    Nop  = 0,
    Push = 1,
    Pop  = 2,
    Add  = 3, Sub  = 4, Mul  = 5, Div  = 6, Rem = 7,
    Eq   = 8, Ne   = 9, Lt   = 10, Le   = 11, Gt = 12, Ge = 13,
    And  = 14, Or  = 15, Not = 16,
    Load = 17, Store = 18,
    Call = 19, Ret  = 20,
    Jump = 21, JumpIf = 22,
    Halt = 0xFF,
}

#[derive(Debug, Default)]
pub struct Chunk {
    pub code:      Vec<u8>,
    pub constants: Vec<f64>,
}

impl Chunk {
    pub fn emit(&mut self, op: Op) { self.code.push(op as u8); }
    pub fn emit_u16(&mut self, v: u16) { self.code.extend_from_slice(&v.to_le_bytes()); }
    pub fn push_const(&mut self, v: f64) -> u16 {
        let idx = self.constants.len() as u16;
        self.constants.push(v);
        idx
    }
}

pub struct BytecodeBackend;

impl CodegenBackend for BytecodeBackend {
    fn emit(&mut self, mir: &MirProgram, out: &std::path::Path) -> anyhow::Result<()> {
        let chunk = Chunk::default();
        let bytes = chunk.code;
        std::fs::write(out.with_extension("lingbc"), bytes)?;
        Ok(())
    }
}
