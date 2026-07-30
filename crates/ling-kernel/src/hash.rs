//! Thin no_std/no-alloc hashing wrapper for `lingfs`'s content addressing.
//! Deliberately separate from `ling-crypto`'s `Blake3` type, which wraps
//! the same underlying crate but assumes a heap/OS (`Vec`-returning APIs,
//! `OsRng`) and can't be used directly in a kernel with no allocator.

/// BLAKE3 hash of `data`, using only the crate's core (non-`std`,
/// non-`alloc`) hasher — safe to call with no heap present.
pub fn blake3(data: &[u8]) -> [u8; 32] {
    let mut hasher = ::blake3::Hasher::new();
    hasher.update(data);
    *hasher.finalize().as_bytes()
}
