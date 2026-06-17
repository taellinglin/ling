//! Ling heap allocation.

use std::alloc::{GlobalAlloc, Layout, System};

/// A heap-allocated Ling value box backed by the system allocator
/// (or mimalloc when the `mimalloc` feature is enabled).
pub struct LingBox<T>(Box<T>);

impl<T> LingBox<T> {
    pub fn new(val: T) -> Self {
        Self(Box::new(val))
    }
    pub fn into_inner(self) -> T {
        *self.0
    }
}

impl<T> std::ops::Deref for LingBox<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T> std::ops::DerefMut for LingBox<T> {
    fn deref_mut(&mut self) -> &mut T {
        &mut self.0
    }
}
impl<T: std::fmt::Debug> std::fmt::Debug for LingBox<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "LingBox({:?})", &*self.0)
    }
}

/// Allocate `size` bytes with `align` alignment using the system allocator.
/// Returns a null pointer on allocation failure.
pub unsafe fn raw_alloc(size: usize, align: usize) -> *mut u8 {
    let layout = Layout::from_size_align_unchecked(size, align);
    System.alloc(layout)
}

/// Deallocate a previously-allocated block.
pub unsafe fn raw_dealloc(ptr: *mut u8, size: usize, align: usize) {
    let layout = Layout::from_size_align_unchecked(size, align);
    System.dealloc(ptr, layout)
}
