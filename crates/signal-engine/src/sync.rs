//! The atomics, the spin hint and the cell the scheduler's state machines
//! ([`gate`](crate::gate), [`board`](crate::board), [`park`](crate::park)) are
//! written with (M2.7-6): std's, or loom's in this crate's own model tests
//! (`cfg(all(test, loom))`, `docs/testing-and-evaluation.md`), so that the
//! model tests run the same code as the engine.

#[cfg(all(test, loom))]
pub(crate) use loom::{cell::UnsafeCell, hint, sync::atomic};
#[cfg(not(all(test, loom)))]
pub(crate) use std::{hint, sync::atomic};

/// std's `UnsafeCell` with loom's interface: the cell hands its pointer to a
/// closure, and dereferencing it is the caller's (`unsafe`) business.
#[cfg(not(all(test, loom)))]
#[derive(Debug)]
pub(crate) struct UnsafeCell<T>(std::cell::UnsafeCell<T>);

#[cfg(not(all(test, loom)))]
impl<T> UnsafeCell<T> {
    pub(crate) const fn new(value: T) -> UnsafeCell<T> {
        UnsafeCell(std::cell::UnsafeCell::new(value))
    }

    /// Calls `f` with a pointer for reading.
    #[inline(always)]
    pub(crate) fn with<R>(&self, f: impl FnOnce(*const T) -> R) -> R {
        f(self.0.get())
    }

    /// Calls `f` with a pointer for writing.
    #[inline(always)]
    pub(crate) fn with_mut<R>(&self, f: impl FnOnce(*mut T) -> R) -> R {
        f(self.0.get())
    }
}

/// Sets an atomic held exclusively (`&mut`), without synchronisation: std's
/// `get_mut`, loom's `with_mut` (which loom checks for concurrent access).
pub(crate) trait SetExclusive<T> {
    fn set_exclusive(&mut self, value: T);
}

macro_rules! set_exclusive {
    ($($atomic:ident: $int:ty),*) => {$(
        impl SetExclusive<$int> for atomic::$atomic {
            #[inline(always)]
            fn set_exclusive(&mut self, value: $int) {
                #[cfg(not(all(test, loom)))]
                {
                    *self.get_mut() = value;
                }
                #[cfg(all(test, loom))]
                self.with_mut(|v| *v = value);
            }
        }
    )*};
}

set_exclusive!(AtomicU8: u8, AtomicU64: u64, AtomicUsize: usize);
