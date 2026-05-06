// src/mir/mod.rs


pub struct MirProgram {
    pub functions: Vec<MirFunction>,
}

pub struct MirFunction {
    pub name: String,
    pub blocks: Vec<()>,
}

