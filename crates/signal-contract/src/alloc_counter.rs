//! The counting global allocator (ADR 0017): the workspace's only `unsafe` code.
//!
//! [`CountingAllocator`] forwards every call unchanged to [`System`] and, while a
//! [`Guard`] is active on the calling thread, counts that thread's allocations,
//! reallocations and frees. A test binary installs it once with
//! [`install_counting_allocator!`](crate::install_counting_allocator); the
//! `no_allocation` check refuses to pass in a binary that has not.
//!
//! The counters are `const`-initialised thread-locals of `Cell<u64>`, which have
//! no destructor: reading or writing them never allocates, so the allocator
//! cannot recurse into itself.

#![allow(unsafe_code)]

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::marker::PhantomData;
use std::thread::LocalKey;

/// A [`GlobalAlloc`] that forwards to [`System`] and counts the current
/// thread's allocations while a [`Guard`] is active. Install it with
/// [`install_counting_allocator!`](crate::install_counting_allocator).
#[derive(Clone, Copy, Debug, Default)]
pub struct CountingAllocator;

thread_local! {
    static ACTIVE_GUARDS: Cell<u64> = const { Cell::new(0) };
    static ALLOCATIONS: Cell<u64> = const { Cell::new(0) };
    static DEALLOCATIONS: Cell<u64> = const { Cell::new(0) };
}

/// Counts one call on `counter` if a guard is active on this thread. During
/// thread teardown the thread-locals may be gone; such calls are not counted.
fn record(counter: &'static LocalKey<Cell<u64>>) {
    let active = ACTIVE_GUARDS.try_with(|g| g.get() > 0).unwrap_or(false);
    if active {
        let _ = counter.try_with(|c| c.set(c.get().saturating_add(1)));
    }
}

fn read(counter: &'static LocalKey<Cell<u64>>) -> u64 {
    counter.try_with(Cell::get).unwrap_or(0)
}

// SAFETY: every method passes its arguments unchanged to `System` and returns
// its result unchanged, so `System` upholds `GlobalAlloc`'s contract. The
// counting in between only touches destructor-free thread-locals: it neither
// allocates nor unwinds.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        record(&ALLOCATIONS);
        // SAFETY: the caller upholds `GlobalAlloc::alloc`'s contract for
        // `layout`, which is the contract `System::alloc` requires.
        unsafe { System.alloc(layout) }
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        record(&ALLOCATIONS);
        // SAFETY: as for `alloc`: the caller's guarantees for `layout` are
        // passed on unchanged.
        unsafe { System.alloc_zeroed(layout) }
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        record(&DEALLOCATIONS);
        // SAFETY: `ptr` was returned by this allocator, hence by `System`,
        // with this `layout`, as the caller guarantees.
        unsafe { System.dealloc(ptr, layout) }
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        record(&ALLOCATIONS);
        // SAFETY: `ptr` and `layout` come from `System` as for `dealloc`, and
        // the caller guarantees `new_size` is valid for `layout`'s alignment.
        unsafe { System.realloc(ptr, layout, new_size) }
    }
}

/// Installs [`CountingAllocator`] as the `#[global_allocator]` of the test
/// binary that invokes it. Use it once per test binary (a test file under
/// `tests/`), at the top level.
#[macro_export]
macro_rules! install_counting_allocator {
    () => {
        #[global_allocator]
        static SIGNAL_CONTRACT_COUNTING_ALLOCATOR: $crate::alloc_counter::CountingAllocator =
            $crate::alloc_counter::CountingAllocator;
    };
}

/// What the current thread did with the heap while a guard was active.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Counts {
    /// Allocations and reallocations.
    pub allocations: u64,
    /// Frees.
    pub deallocations: u64,
}

impl Counts {
    /// Whether nothing was allocated, reallocated or freed.
    pub fn is_zero(&self) -> bool {
        self.allocations == 0 && self.deallocations == 0
    }
}

/// While alive, counts the current thread's heap calls. Guards nest; each
/// reports only what happened during its own lifetime. Not `Send`: the count
/// belongs to the thread that started it.
#[derive(Debug)]
pub struct Guard {
    allocations: u64,
    deallocations: u64,
    _thread_bound: PhantomData<*const ()>,
}

impl Guard {
    /// Starts counting on the current thread.
    pub fn start() -> Guard {
        let _ = ACTIVE_GUARDS.try_with(|g| g.set(g.get().saturating_add(1)));
        Guard {
            allocations: read(&ALLOCATIONS),
            deallocations: read(&DEALLOCATIONS),
            _thread_bound: PhantomData,
        }
    }

    /// The heap calls counted since this guard started.
    pub fn counts(&self) -> Counts {
        Counts {
            allocations: read(&ALLOCATIONS).saturating_sub(self.allocations),
            deallocations: read(&DEALLOCATIONS).saturating_sub(self.deallocations),
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        let _ = ACTIVE_GUARDS.try_with(|g| g.set(g.get().saturating_sub(1)));
    }
}

/// Whether [`CountingAllocator`] is this binary's global allocator: a guarded
/// probe allocation is counted only if it is.
pub fn is_installed() -> bool {
    let guard = Guard::start();
    drop(std::hint::black_box(Box::new(0u8)));
    guard.counts().allocations > 0
}
