//! `Mutex` and `OnceLock` for the no_std build. The engine runs on one thread there (jobs handed to the host's
//! cores only read shared data and write their own rows), so the Mutex is a RefCell with std's `lock()` shape.

use core::cell::{RefCell, RefMut, UnsafeCell};
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, Ordering};

#[derive(Default)]
pub struct Mutex<T>(RefCell<T>);
impl<T> Mutex<T> {
    pub fn new(v: T) -> Mutex<T> {
        Mutex(RefCell::new(v))
    }
    pub const fn new_const_with(v: T) -> Mutex<T> {
        Mutex(RefCell::new(v))
    }
    pub fn lock(&self) -> Result<RefMut<'_, T>, ()> {
        self.0.try_borrow_mut().map_err(|_| ())
    }
}

/// Initialised once, by whoever gets there first (others spin until it is ready).
pub struct OnceLock<T> {
    state: AtomicU8,
    v: UnsafeCell<MaybeUninit<T>>,
}
unsafe impl<T: Sync + Send> Sync for OnceLock<T> {}
unsafe impl<T: Send> Send for OnceLock<T> {}
impl<T> Default for OnceLock<T> {
    fn default() -> Self {
        Self::new()
    }
}
impl<T> OnceLock<T> {
    pub const fn new() -> OnceLock<T> {
        OnceLock { state: AtomicU8::new(0), v: UnsafeCell::new(MaybeUninit::uninit()) }
    }
    pub fn get_or_init<F: FnOnce() -> T>(&self, f: F) -> &T {
        loop {
            match self.state.compare_exchange(0, 1, Ordering::Acquire, Ordering::Acquire) {
                Ok(_) => {
                    unsafe { (*self.v.get()).write(f()) };
                    self.state.store(2, Ordering::Release);
                    break;
                }
                Err(2) => break,
                Err(_) => core::hint::spin_loop(),
            }
        }
        unsafe { (*self.v.get()).assume_init_ref() }
    }
}
