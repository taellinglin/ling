// src/borrowck/mod.rs
mod region;
mod move_check;
mod lifetime;
mod constraints;

use crate::core::LingResult;

// Borrow checking is WIP.
#[derive(Clone, Debug, Default)]
pub struct BorrowChecker;

impl BorrowChecker {
    pub fn check(&self, _hir: &()) -> LingResult<()> {
        Ok(())
    }
}

