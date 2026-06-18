//! Core types and errors for the Ling compiler

pub mod arena;
pub mod error;
pub mod types;

pub use arena::Arena;
pub use error::LingError;
pub use types::{Type, TypeId};
