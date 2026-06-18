/*
 * Placeholder module: src/core/result.rs
 */

use super::error::LingError;

/// Project-wide result alias.
pub type LingResult<T> = Result<T, LingError>;
