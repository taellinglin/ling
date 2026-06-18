//! Simple reference-counting GC shim.
//! A full tracing GC can be plugged in behind this API later.

use std::sync::Arc;

/// A garbage-collected smart pointer (currently just Arc).
pub struct Gc<T: ?Sized>(Arc<T>);

impl<T> Gc<T> {
    pub fn new(val: T) -> Self {
        Self(Arc::new(val))
    }
}

impl<T: ?Sized> Clone for Gc<T> {
    fn clone(&self) -> Self {
        Self(Arc::clone(&self.0))
    }
}
impl<T: ?Sized> std::ops::Deref for Gc<T> {
    type Target = T;
    fn deref(&self) -> &T {
        &self.0
    }
}
impl<T: std::fmt::Debug + ?Sized> std::fmt::Debug for Gc<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Gc({:?})", &*self.0)
    }
}
