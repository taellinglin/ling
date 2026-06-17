#[derive(Clone, Debug, Default)]
pub struct CompilerConfig {
    pub optimization: OptimizationLevel,
}

impl CompilerConfig {
    // CLI integration is WIP; keep a stub so compilation works.
    pub fn from_cli(_cmd: &()) -> Self {
        Self::default()
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub enum OptimizationLevel {
    #[default]
    None,
    O1,
    O2,
    O3,
}
