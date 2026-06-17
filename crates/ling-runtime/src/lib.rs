//! Ling runtime library

pub mod alloc;
pub mod gc;
pub mod std;

pub use alloc::LingBox;
pub use gc::Gc;
