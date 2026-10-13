//! Disjoint access to a plan's mutable state from several threads (ADR 0020):
//! the workspace's third audited `unsafe` module, beside ADR 0017's counting
//! allocator and ADR 0019 §4's `rt_thread.rs`.
//!
//! A block's units write only their own instances' rows of the plan's flat
//! buffers and read the rows of the instances they depend on (ADR 0019 §2).
//! This module is the one place that turns that schedule into references:
//!
//! - [`Cells`] holds one buffer. Without a job, [`Cells::get_mut`] and
//!   [`Cells::get`] (for `Sync` elements only) are the whole slice.
//! - [`PlanCells`] holds every per-instance buffer of a plan. An [`Access`] hands
//!   out one [`CallView`] of one instance's call at a time: each view borrows
//!   the access mutably, so two views of one access never coexist. A view
//!   holds the instance's own parts mutably (of the per-frame block buffers,
//!   a [`Window`] of the call's frames and the frame before) and reads other
//!   instances' rows through readers that refuse its own. An access is
//!   either whole-plan (made from `&mut PlanCells`, one thread) or one
//!   claimed task of a [`Job`], whose views must be of instances the task
//!   owns.
//! - A [`Reader`] reads other instances' rows: those of the reader's own task,
//!   or of a task whose completion it has observed (Release on completion,
//!   Acquire before the read). Only for `Sync` elements.
//! - A [`Job`] publishes a block's tasks to the worker threads through a
//!   [`Gate`]: a worker reads the job only after joining it, and the caller
//!   closes the gate and waits until every worker has left before it
//!   returns or unwinds, so no worker ever holds a reference past the job.
//!
//! In debug builds (and so in every `cargo test`) every [`Cells`] carries an
//! atomic claim per element: a view claims each element it holds mutably
//! for writing, once, when it is made, and a reader each element of a row it
//! reads; a conflicting claim is refused (no reference is made) and counted
//! in [`exclusivity`].
//!
//! Invariants (ADR 0020): a `&Cells` and a live view never coexist, because
//! views exist only inside an [`Access`] or a [`Job`] that holds the plan's
//! cells mutably; every shared read (`get`, [`Reader`]) requires `T: Sync`,
//! exclusive access requires `T: Send`, and the primitives are never read
//! shared.
//!
//! The access below, from [`PlanCells::exclusive`], has no log, so its views
//! are `None`: this sibling of the `compile_fail` tests proves only that
//! taking one view after another has the borrow shape the compiler accepts.
//! Real views and their exclusivity at run time are proved by the checked
//! mode in the determinism matrix and the stress test.
//!
//! ```
//! use signal_engine::shared::{Cells, PlanCells};
//! let mut plan = PlanCells::default();
//! let mut access = plan.exclusive();
//! drop(access.call(0, 0, 1));
//! drop(access.call(0, 0, 1)); // one view at a time
//! let cells = Cells::new(vec![1.0_f64, 2.0]);
//! std::thread::scope(|s| {
//!     s.spawn(|| cells.get().first().copied());
//!     s.spawn(|| cells.get().first().copied());
//! });
//! fn readable(_: Option<signal_engine::shared::Reader<'static, f64>>) {}
//! ```
//!
//! Two views of one access at once do not compile:
//!
//! ```compile_fail,E0499
//! use signal_engine::shared::PlanCells;
//! let mut plan = PlanCells::default();
//! let mut access = plan.exclusive();
//! let a = access.call(0, 0, 1);
//! let b = access.call(0, 0, 1);
//! drop(a);
//! ```
//!
//! Nor do views of two instances:
//!
//! ```compile_fail,E0499
//! use signal_engine::shared::PlanCells;
//! let mut plan = PlanCells::default();
//! let mut access = plan.exclusive();
//! let first = access.call(0, 0, 1);
//! let second = access.call(1, 0, 1);
//! drop(first);
//! ```
//!
//! A shared read of an element that is not `Sync` does not compile, through
//! `get`:
//!
//! ```compile_fail,E0277
//! use signal_engine::shared::Cells;
//! use std::cell::Cell;
//! let cells = Cells::new(vec![Cell::new(0_u32)]);
//! std::thread::scope(|s| {
//!     s.spawn(|| cells.get().first().map(|c| c.set(1)));
//!     s.spawn(|| cells.get().first().map(|c| c.set(2)));
//! });
//! ```
//!
//! nor through a [`Reader`]:
//!
//! ```compile_fail,E0277
//! fn readable(_: Option<signal_engine::shared::Reader<'static, std::cell::Cell<u32>>>) {}
//! ```
//!
//! Exclusive access needs only `Send`:
//!
//! ```
//! use signal_engine::shared::Cells;
//! let mut cells: Cells<Option<Box<dyn FnMut() + Send>>> = Cells::new(vec![None]);
//! if let Some(Some(f)) = cells.get_mut().first_mut() {
//!     f();
//! }
//! ```

#![allow(unsafe_code)]

use std::cell::UnsafeCell;
use std::fmt;
use std::marker::PhantomData;
use std::ops::{Deref, Range as Span};
use std::panic::AssertUnwindSafe;
#[cfg(debug_assertions)]
use std::sync::atomic::AtomicU32;
use std::sync::atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering};

use signal_core::{Event, EventSink, Primitive};

use crate::engine::{Lane, Out, Param, Slot};
use crate::log::FailureReason;

/// Claims made and conflicts refused by the checked mode, process-wide.
#[cfg(any(debug_assertions, test, feature = "test-hooks"))]
static CLAIMS: AtomicU64 = AtomicU64::new(0);
static VIOLATIONS: AtomicU64 = AtomicU64::new(0);

/// What the checked mode has seen in this process (tests only: feature
/// `test-hooks`).
#[cfg(any(test, feature = "test-hooks"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Exclusivity {
    /// Whether the per-element claim tracker is compiled in (debug builds).
    pub checked: bool,
    /// Elements claimed (each element of each view part or read).
    pub claims: u64,
    /// Conflicting claims and refused accesses: always 0 in a correct run.
    pub violations: u64,
}

/// The checked mode's counters so far. Claims are counted per thread and
/// added to the process's count in batches: `claims` covers this thread's
/// claims and every full batch of the others'. Tests only (feature
/// `test-hooks`).
#[cfg(any(test, feature = "test-hooks"))]
pub fn exclusivity() -> Exclusivity {
    #[cfg(debug_assertions)]
    let local = LOCAL_CLAIMS.with(std::cell::Cell::get);
    #[cfg(not(debug_assertions))]
    let local = 0;
    Exclusivity {
        checked: cfg!(debug_assertions),
        claims: CLAIMS.load(Ordering::Relaxed) + local,
        violations: VIOLATIONS.load(Ordering::Relaxed),
    }
}

#[cfg(debug_assertions)]
thread_local! {
    /// This thread's claims not yet added to `CLAIMS`.
    static LOCAL_CLAIMS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
    /// Every element this thread has claimed (for the unit tests).
    static THREAD_CLAIMS: std::cell::Cell<u64> = const { std::cell::Cell::new(0) };
}

/// Counts `elements` claimed elements (checked mode).
#[cfg(debug_assertions)]
fn count_claims(elements: usize) {
    const BATCH: u64 = 4096;
    let elements = elements as u64;
    THREAD_CLAIMS.with(|total| total.set(total.get() + elements));
    LOCAL_CLAIMS.with(|local| {
        let count = local.get() + elements;
        if count >= BATCH {
            CLAIMS.fetch_add(count, Ordering::Relaxed);
            local.set(0);
        } else {
            local.set(count);
        }
    });
}

/// Counts a refused access (never on a correct run's path), process-wide and
/// in the plan's `refusals`, which stops the engine (`docs/semantics.md` §14).
#[cold]
#[inline(never)]
fn violation(refusals: &AtomicU64) {
    VIOLATIONS.fetch_add(1, Ordering::Relaxed);
    refusals.fetch_add(1, Ordering::Relaxed);
}

/// Runs `f`; false if it panicked (`docs/semantics.md` §14). The panic goes
/// no further, and neither does one from its payload's destructor: each
/// payload is dropped under `catch_unwind` too, and after eight in a row the
/// last is leaked rather than dropped.
pub(crate) fn caught(f: impl FnOnce()) -> bool {
    match std::panic::catch_unwind(AssertUnwindSafe(f)) {
        Ok(()) => true,
        Err(mut payload) => {
            for _ in 0..8 {
                match std::panic::catch_unwind(AssertUnwindSafe(move || drop(payload))) {
                    Ok(()) => return false,
                    Err(next) => payload = next,
                }
            }
            std::mem::forget(payload);
            false
        }
    }
}

/// `found`, counting a refusal ([`violation`]) where it is `None`: an
/// index outside the plan's layout. Every refusal is counted once, where it
/// arises.
#[inline(always)]
fn counted<T>(found: Option<T>, refusals: &AtomicU64) -> Option<T> {
    if found.is_none() {
        violation(refusals);
    }
    found
}

/// The writer bit of an element's claim; the low bits count readers.
#[cfg(debug_assertions)]
const WRITER: u32 = 1 << 31;

/// A claim on elements of a [`Cells`], released when dropped. Empty in
/// release builds.
pub struct Claim<'t> {
    #[cfg(debug_assertions)]
    held: Option<Held<'t>>,
    _cells: PhantomData<&'t ()>,
}

/// The elements a claim holds: `len` elements from `first`, then from
/// `first + stride`, … `rows` times (one run per port of a window).
#[cfg(debug_assertions)]
#[derive(Clone, Copy)]
struct Held<'t> {
    claims: &'t [AtomicU32],
    first: usize,
    stride: usize,
    rows: usize,
    len: usize,
    write: bool,
}

#[cfg(debug_assertions)]
impl Held<'_> {
    /// The claims of every element, in order.
    fn each(&self) -> impl Iterator<Item = &AtomicU32> {
        let held = *self;
        (0..held.rows).flat_map(move |r| {
            let start = held.first + r * held.stride;
            held.claims.get(start..start + held.len).unwrap_or(&[])
        })
    }

    fn release(claim: &AtomicU32, write: bool) {
        if write {
            claim.store(0, Ordering::Release);
        } else {
            claim.fetch_sub(1, Ordering::Release);
        }
    }
}

#[cfg(debug_assertions)]
impl Drop for Claim<'_> {
    fn drop(&mut self) {
        if let Some(held) = self.held.take() {
            for claim in held.each() {
                Held::release(claim, held.write);
            }
        }
    }
}

/// One buffer of a plan: elements behind `UnsafeCell`s, handed out whole to
/// whoever holds it (mutably, or shared when `T: Sync`), and in rows (or
/// windows of rows) to the views of this module.
pub struct Cells<T> {
    data: Box<[UnsafeCell<T>]>,
    /// The instance that owns each row, for rows other instances read.
    owner: Box<[u32]>,
    /// Elements per row.
    stride: usize,
    /// One claim per element.
    #[cfg(debug_assertions)]
    claims: Box<[AtomicU32]>,
}

// SAFETY: a `&Cells<T>` gives `&mut T` only through the views of this module,
// each to one thread for elements no other live view or reader holds (see the
// view constructors), and `&T` only where `T: Sync` (`get`, `Reader`, the
// shared parts of views). Handing a `T` to another thread mutably needs only
// `T: Send`, as for `Mutex<T>`.
unsafe impl<T: Send> Sync for Cells<T> {}

impl<T> Default for Cells<T> {
    fn default() -> Cells<T> {
        Cells::new(Vec::new())
    }
}

impl<T> fmt::Debug for Cells<T> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Cells")
            .field("len", &self.data.len())
            .field("stride", &self.stride)
            .finish()
    }
}

impl<T> Cells<T> {
    /// A buffer of `values`, one row per element (control-thread code; it
    /// allocates).
    pub fn new(values: Vec<T>) -> Cells<T> {
        Cells::with_rows(values, 1, Vec::new())
    }

    /// A buffer of rows of `stride` elements; `owner` names the instance of
    /// each row others read (empty when nobody reads another's rows).
    pub(crate) fn with_rows(values: Vec<T>, stride: usize, owner: Vec<u32>) -> Cells<T> {
        let stride = stride.max(1);
        #[cfg(debug_assertions)]
        let claims = values.iter().map(|_| AtomicU32::new(0)).collect();
        Cells {
            data: values.into_iter().map(UnsafeCell::new).collect(),
            owner: owner.into_boxed_slice(),
            stride,
            #[cfg(debug_assertions)]
            claims,
        }
    }

    /// The number of elements.
    pub fn len(&self) -> usize {
        self.data.len()
    }

    /// Whether there are no elements.
    pub fn is_empty(&self) -> bool {
        self.data.is_empty()
    }

    /// Every element, mutably.
    pub fn get_mut(&mut self) -> &mut [T] {
        let len = self.data.len();
        // SAFETY: `&mut self` is exclusive, so no view, reader or other
        // reference to any element exists; `UnsafeCell<T>` has `T`'s layout,
        // and the pointer covers `len` initialised elements.
        unsafe { std::slice::from_raw_parts_mut(UnsafeCell::raw_get(self.data.as_ptr()), len) }
    }

    /// Every element, shared: only for `Sync` elements, since any number of
    /// threads may hold a `&Cells`.
    pub fn get(&self) -> &[T]
    where
        T: Sync,
    {
        let len = self.data.len();
        // SAFETY: views write only inside an `Access` or a `Job`, both of
        // which hold the cells mutably for their whole life, so while this
        // `&self` exists nothing writes any element; `T: Sync` makes `&T`
        // shareable between the threads that hold `&self`.
        unsafe { std::slice::from_raw_parts(UnsafeCell::raw_get(self.data.as_ptr()), len) }
    }

    /// Claims every element of element range `range` for writing (`write`)
    /// or reading. Checked mode only: a conflict is refused and counted.
    #[inline(always)]
    fn claim(&self, refusals: &AtomicU64, range: &Span<usize>, write: bool) -> Option<Claim<'_>> {
        self.track(
            refusals,
            range.start,
            (self.data.len(), 1),
            range.len(),
            write,
        )
    }

    /// The checked mode's half of [`claim`](Cells::claim) and of
    /// [`window`](Cells::window): `len` elements from `first`, then from
    /// `first + stride`, … `rows` times, each claimed, or none of them.
    #[cfg(debug_assertions)]
    fn track(
        &self,
        refusals: &AtomicU64,
        first: usize,
        (stride, rows): (usize, usize),
        len: usize,
        write: bool,
    ) -> Option<Claim<'_>> {
        if rows == 0 || len == 0 {
            return Some(Claim::none());
        }
        let end = first
            .checked_add((rows - 1).saturating_mul(stride))
            .and_then(|last| last.checked_add(len));
        if end.is_none_or(|end| end > self.data.len()) {
            violation(refusals);
            return None;
        }
        let held = Held {
            claims: &self.claims,
            first,
            stride,
            rows,
            len,
            write,
        };
        for (n, claim) in held.each().enumerate() {
            let ok = if write {
                claim
                    .compare_exchange(0, WRITER, Ordering::Acquire, Ordering::Relaxed)
                    .is_ok()
            } else {
                claim.fetch_add(1, Ordering::Acquire) & WRITER == 0
            };
            if !ok {
                if !write {
                    claim.fetch_sub(1, Ordering::Release);
                }
                for undo in held.each().take(n) {
                    Held::release(undo, write);
                }
                violation(refusals);
                return None;
            }
        }
        count_claims(rows * len);
        Some(Claim {
            held: Some(held),
            _cells: PhantomData,
        })
    }

    /// Release builds track nothing; the slice or window taken after the
    /// claim checks the bounds.
    #[cfg(not(debug_assertions))]
    #[inline(always)]
    fn track(
        &self,
        _refusals: &AtomicU64,
        _first: usize,
        _rows: (usize, usize),
        _len: usize,
        _write: bool,
    ) -> Option<Claim<'_>> {
        Some(Claim::none())
    }

    /// `range`, mutably, with its claim.
    ///
    /// # Safety
    /// For the returned lifetime no other reference to an element of `range`
    /// may exist on any thread, and every earlier write to it by another
    /// thread must happen before this call.
    // A `&mut` from `&self` is this function's purpose; the caller's proof of
    // exclusivity is its safety contract.
    #[allow(clippy::mut_from_ref)]
    #[inline(always)]
    unsafe fn slice_mut(
        &self,
        refusals: &AtomicU64,
        range: Span<usize>,
    ) -> Option<(&mut [T], Claim<'_>)> {
        if range.is_empty() {
            return Some((&mut [], Claim::none()));
        }
        let claim = self.claim(refusals, &range, true)?;
        let start = counted(self.data.get(range.clone()), refusals)?.as_ptr();
        // SAFETY: `range` is in bounds (checked above); the caller guarantees
        // exclusivity and ordering; `UnsafeCell::raw_get` is the sanctioned way
        // to write through a shared `UnsafeCell`.
        let slice =
            unsafe { std::slice::from_raw_parts_mut(UnsafeCell::raw_get(start), range.len()) };
        Some((slice, claim))
    }

    /// `range`, shared, with its claim.
    ///
    /// # Safety
    /// For the returned lifetime nothing may write an element of `range`,
    /// and every earlier write to it by another thread must happen before
    /// this call.
    #[inline(always)]
    unsafe fn slice(&self, refusals: &AtomicU64, range: Span<usize>) -> Option<(&[T], Claim<'_>)>
    where
        T: Sync,
    {
        if range.is_empty() {
            return Some((&[], Claim::none()));
        }
        let claim = self.claim(refusals, &range, false)?;
        let start = counted(self.data.get(range.clone()), refusals)?.as_ptr();
        // SAFETY: in bounds (checked above); the caller guarantees no writer
        // and the ordering; `T: Sync` lets other threads hold `&T` too.
        let slice = unsafe { std::slice::from_raw_parts(UnsafeCell::raw_get(start), range.len()) };
        Some((slice, claim))
    }
}

impl Cells<f64> {
    /// Frames `from..to` of each row of `rows`, mutably, with their claim
    /// (each element, once).
    ///
    /// # Safety
    /// For the returned lifetime no other reference to an element of those
    /// frames may exist on any thread, and every earlier write to them by
    /// another thread must happen before this call.
    #[inline(always)]
    unsafe fn window<'s>(
        &'s self,
        refusals: &'s AtomicU64,
        rows: &Span<usize>,
        frames: Span<usize>,
    ) -> Option<Window<'s>> {
        let stride = self.stride;
        if frames.start > frames.end || frames.end > stride {
            violation(refusals);
            return None;
        }
        let span = rows
            .start
            .checked_mul(stride)
            .zip(rows.end.checked_mul(stride));
        let region = counted(span.and_then(|(a, b)| self.data.get(a..b)), refusals)?;
        let ports = rows.len();
        let first = rows.start * stride + frames.start;
        let claim = self.track(refusals, first, (stride, ports), frames.len(), true)?;
        Some(Window {
            data: UnsafeCell::raw_get(region.as_ptr()),
            stride,
            ports,
            frames,
            refusals,
            _claim: claim,
            _rows: PhantomData,
        })
    }
}

/// Frames `frames` of an instance's own rows of a per-frame block buffer
/// (port `p`'s frame `f` at `p·K + f`), claimed element by element when the
/// window is made; its accessors claim nothing and reach no other element.
/// The frames are a call's, and for outputs and parameter values the frame
/// before the call as well, which the holds of a suspended or faulted
/// instance read.
pub struct Window<'t> {
    /// The first element of the instance's first row.
    data: *mut f64,
    stride: usize,
    ports: usize,
    frames: Span<usize>,
    /// The plan's refusal count, for a refused frame.
    refusals: &'t AtomicU64,
    _claim: Claim<'t>,
    _rows: PhantomData<&'t mut [f64]>,
}

impl Window<'_> {
    /// The first element of port `p`'s frames `a..b`, if the window holds
    /// them; refused and counted when not.
    #[inline(always)]
    fn at(&self, p: usize, (a, b): (usize, usize)) -> Option<usize> {
        if p < self.ports && self.frames.start <= a && a <= b && b <= self.frames.end {
            Some(p * self.stride + a)
        } else {
            violation(self.refusals);
            None
        }
    }

    /// Port `p`'s value at frame `f`.
    #[inline(always)]
    pub(crate) fn get(&self, p: usize, f: usize) -> Option<f64> {
        let at = self.at(p, (f, f + 1))?;
        // SAFETY: `at` is one of the window's elements (checked above, inside
        // the region bounds-checked when the window was made); the window
        // holds them exclusively for its lifetime (see `Cells::window`'s
        // contract), and this `&self` excludes a `&mut` from the accessors.
        Some(unsafe { *self.data.add(at) })
    }

    /// Sets port `p`'s value at frame `f`; false if the window lacks it.
    #[inline(always)]
    pub(crate) fn set(&mut self, p: usize, f: usize, value: f64) -> bool {
        let Some(at) = self.at(p, (f, f + 1)) else {
            return false;
        };
        // SAFETY: as in `get`; `&mut self` excludes every other reference
        // made through this window.
        unsafe { *self.data.add(at) = value };
        true
    }

    /// Port `p`'s frames `a..b`.
    #[inline(always)]
    pub(crate) fn frames(&self, p: usize, (a, b): (usize, usize)) -> Option<&[f64]> {
        let at = self.at(p, (a, b))?;
        // SAFETY: as in `get`: frames `a..b` of port `p` are the window's
        // (checked above), a shared reference under `&self`.
        Some(unsafe { std::slice::from_raw_parts(self.data.add(at), b - a) })
    }

    /// Port `p`'s frames `a..b`, mutably.
    #[inline(always)]
    pub(crate) fn frames_mut(&mut self, p: usize, (a, b): (usize, usize)) -> Option<&mut [f64]> {
        let at = self.at(p, (a, b))?;
        // SAFETY: as in `frames`, and `&mut self` makes the reference unique.
        Some(unsafe { std::slice::from_raw_parts_mut(self.data.add(at), b - a) })
    }
}

/// An instance's rows in each buffer, fixed when the plan is built.
#[derive(Clone, Debug, Default)]
pub(crate) struct Layout {
    pub(crate) signal_in: Span<usize>,
    pub(crate) signal_out: Span<usize>,
    pub(crate) event_out: Span<usize>,
    pub(crate) params: Span<usize>,
}

/// The per-instance buffers of [`PlanCells::new`], before they are wrapped.
pub(crate) struct Parts {
    pub(crate) capacity: usize,
    pub(crate) layout: Vec<Layout>,
    pub(crate) instance_task: Vec<u32>,
    pub(crate) slots: Vec<Slot>,
    pub(crate) primitives: Vec<Option<Primitive>>,
    pub(crate) params: Vec<Param>,
    pub(crate) outputs: Vec<f64>,
    pub(crate) output_owner: Vec<u32>,
    pub(crate) held: Vec<f64>,
    pub(crate) signal_in: Vec<f64>,
    pub(crate) param_rows: Vec<f64>,
    pub(crate) sinks: Vec<EventSink>,
    pub(crate) emitted: Vec<Vec<Event>>,
    pub(crate) sink_owner: Vec<u32>,
    pub(crate) lanes: Vec<Lane>,
    pub(crate) outs: Vec<Out>,
    pub(crate) preds: Vec<u32>,
    pub(crate) successors: Vec<Vec<u32>>,
}

/// Every per-instance buffer of a plan (`docs/semantics.md` §13 block
/// buffers), with the task each instance belongs to.
#[derive(Debug, Default)]
pub struct PlanCells {
    layout: Box<[Layout]>,
    instance_task: Box<[u32]>,
    pub(crate) slots: Cells<Slot>,
    pub(crate) primitives: Cells<Option<Primitive>>,
    pub(crate) params: Cells<Param>,
    /// What readers see of output `o` at block frame `f`: `o·K + f`.
    pub(crate) outputs: Cells<f64>,
    pub(crate) held: Cells<f64>,
    /// Signal input `s`'s sum at `s·K + f`.
    pub(crate) signal_in: Cells<f64>,
    /// Parameter `j`'s effective value at `j·K + f`.
    pub(crate) param_rows: Cells<f64>,
    pub(crate) sinks: Cells<EventSink>,
    pub(crate) emitted: Cells<Vec<Event>>,
    /// Per thread: lane 0 is the caller's, lane `w + 1` worker `w`'s.
    pub(crate) lanes: Cells<Lane>,
    /// Per task: its log and counters, in a parallel segment.
    pub(crate) outs: Cells<Out>,
    board: TaskBoard,
    /// Refused accesses since the engine last read it (`take_refusals`): any
    /// stops the engine (`docs/semantics.md` §14).
    refusals: AtomicU64,
    /// Test hooks only: the instance whose views, or whose rows' reads, are
    /// refused as a broken invariant would refuse them.
    #[cfg(feature = "test-hooks")]
    refuse: Option<Refuse>,
}

/// Test hooks only: what [`PlanCells::inject`] refuses.
#[cfg(feature = "test-hooks")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Refuse {
    /// Every view of this instance.
    View(usize),
    /// Every read of this instance's rows.
    Read(usize),
}

/// Test hooks only: a broken invariant for the next jobs to find
/// ([`PlanCells::inject`]).
#[cfg(feature = "test-hooks")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Armed {
    /// See [`Refuse`].
    Refuse(Refuse),
    /// The next push of a ready task finds no slot.
    DropReady,
    /// The next claim finds its slot empty.
    MissingSlot,
}

impl PlanCells {
    pub(crate) fn new(parts: Parts) -> PlanCells {
        let k = parts.capacity.max(1);
        PlanCells {
            layout: parts.layout.into_boxed_slice(),
            instance_task: parts.instance_task.into_boxed_slice(),
            slots: Cells::new(parts.slots),
            primitives: Cells::new(parts.primitives),
            params: Cells::new(parts.params),
            outputs: Cells::with_rows(parts.outputs, k, parts.output_owner),
            held: Cells::new(parts.held),
            signal_in: Cells::with_rows(parts.signal_in, k, Vec::new()),
            param_rows: Cells::with_rows(parts.param_rows, k, Vec::new()),
            sinks: Cells::new(parts.sinks),
            emitted: Cells::with_rows(parts.emitted, 1, parts.sink_owner),
            lanes: Cells::new(parts.lanes),
            outs: Cells::new(parts.outs),
            board: TaskBoard::new(parts.preds, parts.successors),
            refusals: AtomicU64::new(0),
            #[cfg(feature = "test-hooks")]
            refuse: None,
        }
    }

    /// The accesses refused since the last call (0 in a correct run); starts
    /// counting again.
    pub(crate) fn take_refusals(&mut self) -> u64 {
        std::mem::take(self.refusals.get_mut())
    }

    /// Test hooks only: the instance whose views are refused, for a segment
    /// on the calling thread alone (which takes no views).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn refused_view(&self) -> Option<usize> {
        match self.refuse {
            Some(Refuse::View(i)) => Some(i),
            _ => None,
        }
    }

    /// Test hooks only: arms a broken invariant.
    #[cfg(feature = "test-hooks")]
    pub(crate) fn inject(&mut self, armed: Armed) {
        match armed {
            Armed::Refuse(refuse) => self.refuse = Some(refuse),
            Armed::DropReady => *self.board.inject.get_mut() = INJECT_DROP_READY,
            Armed::MissingSlot => *self.board.inject.get_mut() = INJECT_MISSING_SLOT,
        }
    }

    /// The whole plan, for one thread, without a log (the doctests'; a
    /// sequential segment takes the buffers whole with `get_mut` instead):
    /// exclusivity proven by `&mut self`. Its views are always `None`, so it
    /// shows the borrow shape of views, not a view.
    pub fn exclusive(&mut self) -> Access<'_> {
        Access {
            cells: self,
            token: Token::Whole,
            lane: 0,
            main: None,
            _thread: PhantomData,
        }
    }
}

/// What an [`Access`] may touch.
#[derive(Clone, Copy, Debug)]
enum Token {
    /// Everything: the access holds the cells mutably.
    Whole,
    /// The instances of a task claimed in a job of epoch `epoch`.
    Task { task: u32, epoch: u64 },
}

/// The right to view instances: the whole plan, or one claimed task. Views
/// borrow it mutably, so it has at most one view at a time.
pub struct Access<'a> {
    cells: &'a PlanCells,
    token: Token,
    lane: usize,
    main: Option<&'a mut Out>,
    /// An access belongs to the thread it was made on.
    _thread: PhantomData<*const ()>,
}

/// One call of one instance over block frames `a..b`: everything of its own,
/// mutably (its slot, primitive, parameters, its windows of the per-frame
/// block buffers, held outputs, sinks and emitted events, the thread's lane
/// and the log), and the outputs and events of the instances it reads,
/// shared, through readers that refuse its own rows.
pub struct CallView<'t> {
    pub(crate) slot: &'t mut Slot,
    pub(crate) primitive: &'t mut Option<Primitive>,
    pub(crate) params: &'t mut [Param],
    /// Its signal inputs at frames `a..b`.
    pub(crate) signal_in: Window<'t>,
    /// Its effective parameter values at frames `a − 1..b` (`a..b` at 0).
    pub(crate) param_rows: Window<'t>,
    /// Its outputs at frames `a − 1..b` (`a..b` at 0).
    pub(crate) outputs: Window<'t>,
    pub(crate) held: &'t mut [f64],
    pub(crate) sinks: &'t mut [EventSink],
    pub(crate) emitted: &'t mut [Vec<Event>],
    /// Other instances' outputs at frames `a..b`.
    pub(crate) sources: Reader<'t, f64>,
    /// Other instances' emitted events.
    pub(crate) events: Reader<'t, Vec<Event>>,
    pub(crate) lane: &'t mut Lane,
    pub(crate) out: &'t mut Out,
    _claims: [Claim<'t>; 8],
}

/// The first element of a one-element slice, mutably.
#[inline(always)]
fn one<T>(slice: &mut [T]) -> Option<&mut T> {
    slice.first_mut()
}

impl<'a> Access<'a> {
    /// Whether this access may view instance `i`; refused and counted when not.
    #[inline(always)]
    fn owns(&self, i: usize) -> bool {
        let owns = match self.token {
            Token::Whole => i < self.cells.layout.len(),
            Token::Task { task, .. } => self.cells.instance_task.get(i) == Some(&task),
        };
        #[cfg(feature = "test-hooks")]
        let owns = owns && self.cells.refuse != Some(Refuse::View(i));
        if !owns {
            violation(&self.cells.refusals);
        }
        owns
    }

    /// The log this access writes: the task's own, or the plan's.
    #[inline(always)]
    fn out(&mut self) -> Option<(&mut Out, Claim<'a>)> {
        match self.token {
            Token::Whole => Some((self.main.as_deref_mut()?, Claim::none())),
            Token::Task { task, .. } => {
                let t = task as usize;
                // SAFETY: task `t` was claimed by this access alone (the board
                // hands each task to one claimant per epoch), and only its
                // claimant's views touch `outs[t]`; the caller reset the log
                // before publishing the job (Release on opening the gate,
                // Acquire on joining it).
                unsafe { self.cells.outs.slice_mut(&self.cells.refusals, t..t + 1) }
                    .and_then(|(s, c)| Some((counted(one(s), &self.cells.refusals)?, c)))
            }
        }
    }

    /// A reader of `cells` that refuses rows `own` (the viewed instance's,
    /// which its view holds mutably) and reads frames `frames` of a row.
    #[inline(always)]
    fn reader<T: Sync>(
        &self,
        cells: &'a Cells<T>,
        own: Span<usize>,
        frames: Span<usize>,
    ) -> Reader<'a, T> {
        Reader {
            cells,
            check: match self.token {
                Token::Whole => Check::Whole,
                Token::Task { task, epoch } => Check::Task {
                    task,
                    epoch,
                    instance_task: &self.cells.instance_task,
                    board: &self.cells.board,
                },
            },
            own,
            frames,
            refusals: &self.cells.refusals,
            #[cfg(feature = "test-hooks")]
            refuse: match self.cells.refuse {
                Some(Refuse::Read(i)) => Some(i),
                _ => None,
            },
        }
    }

    /// Instance `i`'s view for one call over block frames `a..b`. A task's
    /// access that cannot hand it out counts one refusal, where it arises,
    /// which stops the engine; the whole-plan access of
    /// [`PlanCells::exclusive`] has no log, so its views are `None` by
    /// design, not refused.
    #[inline(always)]
    pub fn call(&mut self, i: usize, a: usize, b: usize) -> Option<CallView<'_>> {
        if let Token::Whole = self.token {
            return None;
        }
        self.view(i, a, b)
    }

    #[inline(always)]
    fn view(&mut self, i: usize, a: usize, b: usize) -> Option<CallView<'_>> {
        if !self.owns(i) {
            return None;
        }
        let c = self.cells;
        let r = &c.refusals;
        let l = counted(c.layout.get(i), r)?;
        let lane = self.lane;
        let before = a.saturating_sub(1);
        let sources = self.reader(&c.outputs, l.signal_out.clone(), a..b);
        let events = self.reader(&c.emitted, l.event_out.clone(), 0..1);
        let (out, out_claim) = self.out()?;
        // SAFETY (each part): instance `i` belongs to this access's task (or
        // the access is whole-plan), and these are `i`'s own rows: no other
        // instance's view touches them (readers refuse them: `own`), and
        // this access has no other live view (views borrow it mutably).
        // Their last writer was this task's earlier view on this thread, a
        // view of an earlier segment (complete before this one began), or
        // the caller before the job was published (Release on opening the
        // gate, Acquire on joining it). Other tasks read `i`'s outputs and
        // events only after this task's completion (Release on `done`,
        // Acquire before their read), so never while this view lives; the
        // frame before the call (`before`), read by the holds, is `i`'s too.
        let (slot, c0) = unsafe { c.slots.slice_mut(r, i..i + 1) }?;
        let (primitive, c1) = unsafe { c.primitives.slice_mut(r, i..i + 1) }?;
        let (params, c2) = unsafe { c.params.slice_mut(r, l.params.clone()) }?;
        let signal_in = unsafe { c.signal_in.window(r, &l.signal_in, a..b) }?;
        let param_rows = unsafe { c.param_rows.window(r, &l.params, before..b) }?;
        let outputs = unsafe { c.outputs.window(r, &l.signal_out, before..b) }?;
        let (held, c3) = unsafe { c.held.slice_mut(r, l.signal_out.clone()) }?;
        let (sinks, c4) = unsafe { c.sinks.slice_mut(r, l.event_out.clone()) }?;
        let (emitted, c5) = unsafe { c.emitted.slice_mut(r, l.event_out.clone()) }?;
        // SAFETY: lane `lane` is used by this thread alone: lane 0 by the
        // caller, lane `w + 1` by worker `w`, each joined once per job.
        let (lanes, c6) = unsafe { c.lanes.slice_mut(r, lane..lane + 1) }?;
        Some(CallView {
            slot: counted(one(slot), r)?,
            primitive: counted(one(primitive), r)?,
            params,
            signal_in,
            param_rows,
            outputs,
            held,
            sinks,
            emitted,
            sources,
            events,
            lane: counted(one(lanes), r)?,
            out,
            _claims: [c0, c1, c2, c3, c4, c5, c6, out_claim],
        })
    }
}

impl<'t> Claim<'t> {
    /// No claim (a whole-plan access's log).
    fn none() -> Claim<'t> {
        Claim {
            #[cfg(debug_assertions)]
            held: None,
            _cells: PhantomData,
        }
    }
}

/// How a [`Reader`] decides that a row is complete.
#[derive(Clone, Copy)]
enum Check<'t> {
    Whole,
    Task {
        task: u32,
        epoch: u64,
        instance_task: &'t [u32],
        board: &'t TaskBoard,
    },
}

/// Shared reads of the rows other instances wrote: those of the reader's own
/// task, or of a task whose completion it observes; never the rows of the
/// instance whose view holds the reader (`own`), which the view holds
/// mutably. A row of a per-frame block buffer is read at the call's frames.
pub struct Reader<'t, T: Sync> {
    cells: &'t Cells<T>,
    check: Check<'t>,
    own: Span<usize>,
    frames: Span<usize>,
    /// The plan's refusal count.
    refusals: &'t AtomicU64,
    /// Test hooks only: the instance whose rows are refused.
    #[cfg(feature = "test-hooks")]
    refuse: Option<usize>,
}

/// A row read through a [`Reader`], with its claim.
pub struct Shared<'t, T> {
    slice: &'t [T],
    _claim: Claim<'t>,
}

impl<T> Deref for Shared<'_, T> {
    type Target = [T];
    fn deref(&self) -> &[T] {
        self.slice
    }
}

impl<'t, T: Sync> Reader<'t, T> {
    /// Row `r` of a buffer with one element per row.
    #[inline]
    pub(crate) fn row(&self, r: usize) -> Option<Shared<'t, T>> {
        self.read(r, (0, 1))
    }

    /// Frames `a..b` of row `r` of a per-frame block buffer, inside the
    /// call's frames.
    #[inline]
    pub(crate) fn frames(&self, r: usize, (a, b): (usize, usize)) -> Option<Shared<'t, T>> {
        if a > b || a < self.frames.start || b > self.frames.end {
            violation(self.refusals);
            return None;
        }
        self.read(r, (a, b))
    }

    /// Elements `a..b` of row `r`, if its owner's task is this reader's or
    /// has completed in this epoch, and it is not the viewed instance's own;
    /// a refused read counts one refusal, where it arises, which stops the
    /// engine.
    #[inline(always)]
    fn read(&self, r: usize, (a, b): (usize, usize)) -> Option<Shared<'t, T>> {
        if self.own.contains(&r) {
            violation(self.refusals);
            return None;
        }
        if let Check::Task {
            task,
            epoch,
            instance_task,
            board,
        } = self.check
        {
            let owner = *counted(self.cells.owner.get(r), self.refusals)?;
            let owner_task = *counted(instance_task.get(owner as usize), self.refusals)?;
            // Acquire: pairs with the Release store of the producer's
            // completion, so its writes to this row happen before the read.
            let complete = owner_task == task
                || board
                    .done
                    .get(owner_task as usize)
                    .is_some_and(|d| d.load(Ordering::Acquire) == epoch);
            // A failed task never stores its `done`: its rows are refused.
            #[cfg(feature = "test-hooks")]
            let complete = complete && self.refuse != Some(owner as usize);
            if !complete {
                violation(self.refusals);
                return None;
            }
        }
        let stride = self.cells.stride;
        if b > stride {
            violation(self.refusals);
            return None;
        }
        let row = counted(r.checked_mul(stride), self.refusals)?;
        let range = row + a..row + b;
        // SAFETY: the row's owner is this reader's own task (its writes were
        // on this thread, by views now dropped) or a task whose completion was
        // observed above (Release on completion, Acquire before this read);
        // a completed task writes no more in this epoch. The only live view
        // of the reader's task is the one holding this reader, and the row
        // is not that view's instance's (refused above).
        let (slice, claim) = unsafe { self.cells.slice(self.refusals, range) }?;
        Some(Shared {
            slice,
            _claim: claim,
        })
    }
}

/// What the task board says when asked for work.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Next {
    Task(usize),
    /// Some task is not claimed yet but none is ready.
    NotYet,
    /// Every task is claimed.
    AllClaimed,
}

/// The tasks of a plan and their dependency counters (atomics only), and
/// whether the job failed (`docs/semantics.md` §14).
#[derive(Default)]
struct TaskBoard {
    preds: Box<[u32]>,
    succ_start: Box<[usize]>,
    succ: Box<[u32]>,
    pending: Box<[AtomicU64]>,
    /// The epoch in which each task last completed successfully; a failed
    /// task's stays older, so its rows are never read.
    done: Box<[AtomicU64]>,
    /// Ready tasks in the order they became ready, with the epoch each was
    /// pushed in.
    ready: Box<[AtomicU64]>,
    ready_epoch: Box<[AtomicU64]>,
    head: AtomicUsize,
    tail: AtomicUsize,
    /// Claims settled: one per claim that advanced `head`, by the task's
    /// completion or, for a claim that found no task, by the claim itself.
    completed: AtomicUsize,
    /// Why this job failed, the first reason only: 0 (it has not),
    /// [`FAILED_PANIC`] or [`FAILED_INVARIANT`].
    failed: AtomicU8,
    /// Test hooks only: a one-shot broken invariant ([`PlanCells::inject`]).
    #[cfg(feature = "test-hooks")]
    inject: AtomicU8,
}

/// Something unwound in a task outside a primitive's call.
const FAILED_PANIC: u8 = 1;
/// The board found itself inconsistent.
const FAILED_INVARIANT: u8 = 2;

/// Test hooks only: [`TaskBoard::inject`]'s values.
#[cfg(feature = "test-hooks")]
const INJECT_DROP_READY: u8 = 1;
#[cfg(feature = "test-hooks")]
const INJECT_MISSING_SLOT: u8 = 2;

impl fmt::Debug for TaskBoard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TaskBoard")
            .field("tasks", &self.preds.len())
            .finish()
    }
}

impl TaskBoard {
    fn new(preds: Vec<u32>, successors: Vec<Vec<u32>>) -> TaskBoard {
        let n = preds.len();
        let mut succ_start = Vec::with_capacity(n + 1);
        let mut succ = Vec::new();
        for list in &successors {
            succ_start.push(succ.len());
            succ.extend_from_slice(list);
        }
        succ_start.resize(n + 1, succ.len());
        let atomics = |_| AtomicU64::new(0);
        TaskBoard {
            preds: preds.into_boxed_slice(),
            succ_start: succ_start.into_boxed_slice(),
            succ: succ.into_boxed_slice(),
            pending: (0..n).map(atomics).collect(),
            done: (0..n).map(atomics).collect(),
            ready: (0..n).map(atomics).collect(),
            ready_epoch: (0..n).map(atomics).collect(),
            head: AtomicUsize::new(0),
            tail: AtomicUsize::new(0),
            completed: AtomicUsize::new(0),
            failed: AtomicU8::new(0),
            #[cfg(feature = "test-hooks")]
            inject: AtomicU8::new(0),
        }
    }

    /// Resets the counters for a segment of epoch `epoch` and pushes the
    /// tasks without predecessors, in task order. Exclusive: before the job
    /// is published.
    fn reset(&mut self, epoch: u64) {
        for (pending, &preds) in self.pending.iter_mut().zip(self.preds.iter()) {
            *pending.get_mut() = u64::from(preds);
        }
        *self.head.get_mut() = 0;
        *self.tail.get_mut() = 0;
        *self.completed.get_mut() = 0;
        *self.failed.get_mut() = 0;
        for t in 0..self.preds.len() {
            if self.preds.get(t) == Some(&0) {
                self.push(t, epoch);
            }
        }
    }

    /// Test hooks only: whether the armed one-shot invariant `which` fires.
    #[cfg(feature = "test-hooks")]
    fn injected(&self, which: u8) -> bool {
        self.inject
            .compare_exchange(which, 0, Ordering::Relaxed, Ordering::Relaxed)
            .is_ok()
    }

    fn push(&self, t: usize, epoch: u64) {
        #[cfg(feature = "test-hooks")]
        if self.injected(INJECT_DROP_READY) {
            self.fail(FAILED_INVARIANT);
            return;
        }
        let at = self.tail.fetch_add(1, Ordering::AcqRel);
        if let (Some(slot), Some(slot_epoch)) = (self.ready.get(at), self.ready_epoch.get(at)) {
            slot.store(t as u64, Ordering::Relaxed);
            // Release: a claimant that sees this epoch sees the task index.
            slot_epoch.store(epoch, Ordering::Release);
        } else {
            // No slot: the task would never run. Nothing waits for it after
            // the failure (it is never claimed).
            self.fail(FAILED_INVARIANT);
        }
    }

    fn claim(&self, epoch: u64) -> Next {
        loop {
            let head = self.head.load(Ordering::Acquire);
            if head >= self.preds.len() {
                return Next::AllClaimed;
            }
            let ready = self
                .ready_epoch
                .get(head)
                .is_some_and(|e| e.load(Ordering::Acquire) == epoch);
            if !ready {
                return Next::NotYet;
            }
            let task = self.ready.get(head).map(|s| s.load(Ordering::Relaxed));
            if self
                .head
                .compare_exchange(head, head + 1, Ordering::AcqRel, Ordering::Acquire)
                .is_ok()
            {
                #[cfg(feature = "test-hooks")]
                let task = task.filter(|_| !self.injected(INJECT_MISSING_SLOT));
                return match task {
                    Some(t) => Next::Task(t as usize),
                    None => {
                        // A claim with no task: it settles itself, as a
                        // failed completion, so a drain never waits for it.
                        self.fail(FAILED_INVARIANT);
                        self.completed.fetch_add(1, Ordering::AcqRel);
                        Next::NotYet
                    }
                };
            }
        }
    }

    /// Task `t` is complete: Release its writes (if it succeeded), then
    /// release its successors. A failed task (`ok` false) fails the job and
    /// never marks itself done, so no successor reads its rows.
    fn complete(&self, t: usize, epoch: u64, ok: bool) {
        if !ok {
            self.fail(FAILED_PANIC);
        } else if let Some(done) = self.done.get(t) {
            done.store(epoch, Ordering::Release);
        }
        let start = self.succ_start.get(t).copied().unwrap_or(0);
        let end = self.succ_start.get(t + 1).copied().unwrap_or(start);
        for &s in self.succ.get(start..end).unwrap_or(&[]) {
            let s = s as usize;
            // AcqRel: the last predecessor's decrement sees every other
            // predecessor's completion before it pushes the successor.
            match self.pending.get(s) {
                Some(p) => {
                    if p.fetch_sub(1, Ordering::AcqRel) == 1 {
                        self.push(s, epoch);
                    }
                }
                None => self.fail(FAILED_INVARIANT),
            }
        }
        self.completed.fetch_add(1, Ordering::AcqRel);
    }

    fn all_complete(&self) -> bool {
        self.completed.load(Ordering::Acquire) >= self.preds.len()
    }

    /// The job failed for `why` (Release: before any claimant that sees it
    /// skips its task); the first reason stays.
    fn fail(&self, why: u8) {
        let _ = self
            .failed
            .compare_exchange(0, why, Ordering::AcqRel, Ordering::Acquire);
    }

    fn failed(&self) -> bool {
        self.failed.load(Ordering::Acquire) != 0
    }

    /// Every claim made so far is settled: read `head`, then `completed`.
    /// After a failure the caller waits only for this, never for tasks that
    /// were never claimed.
    fn drained(&self) -> bool {
        let head = self.head.load(Ordering::Acquire);
        self.completed.load(Ordering::Acquire) >= head
    }
}

/// Where the test hook is called (`Engine::set_task_hook`).
#[cfg(feature = "test-hooks")]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HookPoint {
    /// A thread starts taking part in a job (lane 0 is the caller).
    Enter {
        /// The thread's lane.
        lane: usize,
    },
    /// A thread is about to run a task.
    Task {
        /// The thread's lane.
        lane: usize,
        /// The task.
        task: usize,
    },
    /// A thread's task body has returned, before the task is marked
    /// complete (not called when the body unwinds).
    Done {
        /// The thread's lane.
        lane: usize,
        /// The task.
        task: usize,
    },
    /// A thread stops taking part in a job.
    Leave {
        /// The thread's lane.
        lane: usize,
    },
}

/// Counts kept by a worker pool.
#[derive(Debug, Default)]
pub(crate) struct PoolStats {
    pub(crate) worker_tasks: AtomicU64,
    /// Tasks that failed (something unwound outside a primitive's call), on
    /// any lane.
    pub(crate) task_failures: AtomicU64,
    /// Whether task bodies are timed (the wake-up trace, M2.7-1).
    pub(crate) trace: bool,
    /// The longest task body since the callback began, in ns (0: none).
    pub(crate) slowest_task_ns: AtomicU64,
}

/// The work of one task: the engine runs the task's units with the access.
pub(crate) type Body<'a> = &'a (dyn Fn(&mut Access<'_>, usize) + Sync);

/// One parallel segment: the plan's cells, its tasks and their body.
pub(crate) struct Job<'a> {
    cells: &'a PlanCells,
    epoch: u64,
    body: Body<'a>,
    #[cfg(feature = "test-hooks")]
    hook: Option<fn(HookPoint)>,
    stats: &'a PoolStats,
}

impl<'a> Job<'a> {
    /// A job of epoch `epoch` over `cells`, holding them for its whole life.
    pub(crate) fn new(
        cells: &'a mut PlanCells,
        epoch: u64,
        body: Body<'a>,
        stats: &'a PoolStats,
    ) -> Job<'a> {
        cells.board.reset(epoch);
        for out in cells.outs.get_mut() {
            out.clear();
        }
        Job {
            cells,
            epoch,
            body,
            #[cfg(feature = "test-hooks")]
            hook: None,
            stats,
        }
    }

    /// Test hooks only: the hook its threads call (`Engine::set_task_hook`).
    #[cfg(feature = "test-hooks")]
    pub(crate) fn with_hook(mut self, hook: Option<fn(HookPoint)>) -> Job<'a> {
        self.hook = hook;
        self
    }

    /// Why the job failed, if it did (a task's unwind, or the board found
    /// itself inconsistent): nothing of it may be published
    /// (`docs/semantics.md` §14).
    pub(crate) fn failure(&self) -> Option<FailureReason> {
        match self.cells.board.failed.load(Ordering::Acquire) {
            0 => None,
            FAILED_PANIC => Some(FailureReason::TaskPanic),
            _ => Some(FailureReason::InvariantViolation),
        }
    }

    /// Test hooks only: calls the hook; an unwind from it fails the job.
    #[cfg(feature = "test-hooks")]
    fn hook(&self, point: HookPoint) {
        if let Some(hook) = self.hook
            && !caught(|| hook(point))
        {
            self.stats.task_failures.fetch_add(1, Ordering::Relaxed);
            self.cells.board.fail(FAILED_PANIC);
        }
    }

    /// Takes part as lane `lane` until every task is claimed (a worker) or
    /// complete (the caller, lane 0), or, after a failure, until it has
    /// nothing in hand (a worker) or every claim is settled (the caller).
    pub(crate) fn participate(&self, lane: usize) {
        if lane >= self.cells.lanes.len() {
            return;
        }
        #[cfg(feature = "test-hooks")]
        self.hook(HookPoint::Enter { lane });
        let board = &self.cells.board;
        loop {
            let next = board.claim(self.epoch);
            if let Next::Task(t) = next {
                // Settled before anything else below: a failed completion if
                // the job has failed or the body unwinds, so no thread waits
                // for this claim (and lane 0 never waits on its own).
                let ran = !board.failed() && self.run(lane, t);
                board.complete(t, self.epoch, ran);
                if ran && lane != 0 {
                    self.stats.worker_tasks.fetch_add(1, Ordering::Relaxed);
                }
            }
            if board.failed() {
                if lane == 0 {
                    while !board.drained() {
                        std::hint::spin_loop();
                    }
                }
                break;
            }
            match next {
                Next::Task(_) => {}
                Next::NotYet => {
                    if lane == 0 && board.all_complete() {
                        break;
                    }
                    std::hint::spin_loop();
                }
                Next::AllClaimed => {
                    if lane == 0 {
                        while !board.all_complete() && !(board.failed() && board.drained()) {
                            std::hint::spin_loop();
                        }
                    }
                    break;
                }
            }
        }
        #[cfg(feature = "test-hooks")]
        self.hook(HookPoint::Leave { lane });
    }

    /// Runs task `t`'s body on lane `lane`; false if anything unwound in it
    /// outside a primitive's call (a primitive's own panic is caught at its
    /// call and mutes the instance, `docs/semantics.md` §14).
    fn run(&self, lane: usize, t: usize) -> bool {
        let body = || {
            #[cfg(feature = "test-hooks")]
            if let Some(hook) = self.hook {
                hook(HookPoint::Task { lane, task: t });
            }
            let began = self.stats.trace.then(std::time::Instant::now);
            let mut access = Access {
                cells: self.cells,
                token: Token::Task {
                    task: t as u32,
                    epoch: self.epoch,
                },
                lane,
                main: None,
                _thread: PhantomData,
            };
            (self.body)(&mut access, t);
            if let Some(began) = began {
                let body = u64::try_from(began.elapsed().as_nanos()).unwrap_or(u64::MAX);
                self.stats
                    .slowest_task_ns
                    .fetch_max(body.max(1), Ordering::Relaxed);
            }
            #[cfg(feature = "test-hooks")]
            if let Some(hook) = self.hook {
                hook(HookPoint::Done { lane, task: t });
            }
        };
        let ran = caught(body);
        if !ran {
            self.stats.task_failures.fetch_add(1, Ordering::Relaxed);
        }
        ran
    }
}

/// The gate word: the epoch above bit 16, then the `open` bit, then the
/// number of joined workers.
const OPEN: u64 = 1 << 15;
const JOINED: u64 = OPEN - 1;
const EPOCH_SHIFT: u32 = 16;

/// How the caller publishes a [`Job`] to the workers and takes it back.
pub(crate) struct Gate {
    word: AtomicU64,
    /// The published job; written only while the gate is closed and no
    /// worker is joined, read only by joined workers.
    job: UnsafeCell<*const Job<'static>>,
}

// SAFETY: `job` is written only by the gate's single opener while the gate is
// closed and `joined == 0`, and read only by workers between a successful
// join (Acquire) and their leave (Release), so its accesses never race; the
// job it points to is `Sync` (`assert_sync`, checked by the compiler) and
// outlives every such read (see `serve`).
unsafe impl Sync for Gate {}
// SAFETY: as above; the pointer is only a published reference.
unsafe impl Send for Gate {}

impl fmt::Debug for Gate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Gate")
            .field("word", &self.word.load(Ordering::Relaxed))
            .finish()
    }
}

/// The only handle that can open a [`Gate`] (one per gate).
#[derive(Debug)]
pub(crate) struct Opener {
    gate: std::sync::Arc<Gate>,
}

impl Gate {
    /// A closed gate and its opener.
    pub(crate) fn new() -> (std::sync::Arc<Gate>, Opener) {
        let gate = std::sync::Arc::new(Gate {
            word: AtomicU64::new(0),
            job: UnsafeCell::new(std::ptr::null()),
        });
        (gate.clone(), Opener { gate })
    }

    fn epoch(word: u64) -> u64 {
        word >> EPOCH_SHIFT
    }

    /// Whether a job newer than `seen` is open (SeqCst: the worker's half of
    /// the park handshake).
    pub(crate) fn joinable(&self, seen: u64) -> bool {
        let word = self.word.load(Ordering::SeqCst);
        word & OPEN != 0 && Gate::epoch(word) != seen
    }

    /// Joins the open job if it is newer than `seen`, runs `work` on it and
    /// leaves. Returns whether it joined.
    pub(crate) fn join(&self, seen: &mut u64, work: impl FnOnce(&Job<'_>)) -> bool {
        let word = self.word.load(Ordering::Acquire);
        if word & OPEN == 0 || Gate::epoch(word) == *seen || word & JOINED == JOINED {
            return false;
        }
        // AcqRel: Acquire pairs with the opener's SeqCst (so Release) open,
        // so the job pointer and everything the caller wrote before opening
        // are visible below. It succeeds only if the gate is still open for
        // this epoch: a worker that paused after loading `word` fails here
        // once the gate has closed, and never reads the pointer.
        if self
            .word
            .compare_exchange(word, word + 1, Ordering::AcqRel, Ordering::Relaxed)
            .is_err()
        {
            return false;
        }
        *seen = Gate::epoch(word);
        let _leave = Leave(&self.word);
        // SAFETY: the pointer was written before the open this join synchronised
        // with (Acquire above), and the opener does not return, unwind,
        // close for good or rewrite it until `joined` is back to 0 (its
        // `Close` guard), which needs this worker's Release decrement in
        // `Leave`: the job outlives every use here. `Job` is `Sync`
        // (`assert_sync`, checked by the compiler), so sharing it is sound.
        let job = unsafe { &**self.job.get() };
        work(job);
        true
    }
}

/// Leaves the gate when dropped (also while unwinding).
struct Leave<'a>(&'a AtomicU64);

impl Drop for Leave<'_> {
    fn drop(&mut self) {
        // Release: every access this worker made to the job happens before
        // the opener's Acquire load that sees `joined == 0`.
        self.0.fetch_sub(1, Ordering::Release);
    }
}

impl Opener {
    /// Publishes `job`, calls `after_open` (to wake workers), runs `caller`
    /// on it, then closes the gate and waits until every joined worker has
    /// left. Only then does it return, or unwind if `after_open` or `caller`
    /// panics, so no worker can touch the job (or what it borrows) afterwards.
    pub(crate) fn serve(
        &mut self,
        job: &Job<'_>,
        after_open: impl FnOnce(),
        caller: impl FnOnce(&Job<'_>),
    ) {
        let gate = &*self.gate;
        let word = gate.word.load(Ordering::Acquire);
        // SAFETY: the gate is closed and `joined == 0` here (every `serve`
        // waits for that before it returns or unwinds, and this `&mut self`
        // is the only opener), so no worker reads the pointer while it is
        // written. The lifetime is erased only for the gate's storage: `job`
        // outlives every read, because `Close` below keeps this function from
        // returning or unwinding until the last joined worker has left. The
        // job is `Sync` (`assert_sync`, checked by the compiler), so workers
        // may share it.
        unsafe {
            *gate.job.get() = (job as *const Job<'_>).cast::<Job<'static>>();
        }
        let epoch = Gate::epoch(word).wrapping_add(1) & (u64::MAX >> EPOCH_SHIFT);
        // SeqCst: publishes the pointer and the job (Release), and orders the
        // open before `after_open`'s SeqCst reads of the parked flags.
        gate.word
            .swap((epoch << EPOCH_SHIFT) | OPEN, Ordering::SeqCst);
        let close = Close(&gate.word);
        after_open();
        caller(job);
        drop(close);
    }
}

/// Closes a gate and waits until every joined worker has left, when dropped:
/// on `serve`'s return and while it unwinds.
struct Close<'a>(&'a AtomicU64);

impl Drop for Close<'_> {
    fn drop(&mut self) {
        self.0.fetch_and(!OPEN, Ordering::AcqRel);
        // Acquire: pairs with every worker's Release leave.
        while self.0.load(Ordering::Acquire) & JOINED != 0 {
            std::hint::spin_loop();
        }
    }
}

/// `Job` must be `Sync`: the gate shares one job between the workers. Checked
/// here so that a field that is not `Sync` fails to compile rather than make
/// the gate's SAFETY comments false.
const fn assert_sync<T: Sync + ?Sized>() {}
const _: () = assert_sync::<Job<'static>>();

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_and_get_mut_see_every_element() {
        let mut cells = Cells::new(vec![1.0, 2.0, 3.0]);
        cells.get_mut()[1] = 5.0;
        assert_eq!(cells.get(), &[1.0, 5.0, 3.0]);
    }

    /// The checked mode refuses a second write claim and a read of a claimed
    /// element, and counts both, in the plan's refusals too (path 8 of
    /// `docs/semantics.md` §14: detectable in checked builds only).
    #[cfg(debug_assertions)]
    #[test]
    fn checked_mode_refuses_and_counts_conflicts() {
        let cells = Cells::new(vec![0.0_f64; 4]);
        let refusals = AtomicU64::new(0);
        let before = exclusivity().violations;
        // SAFETY: test of the tracker only; the first claim is the only
        // reference made, and the refused ones make none.
        let first = unsafe { cells.slice_mut(&refusals, 0..2) };
        assert!(first.is_some());
        assert!(unsafe { cells.slice_mut(&refusals, 1..3) }.is_none());
        assert!(unsafe { cells.slice(&refusals, 0..1) }.is_none());
        assert!(exclusivity().violations >= before + 2);
        assert_eq!(refusals.load(Ordering::Relaxed), 2);
        drop(first);
        assert!(unsafe { cells.slice(&refusals, 0..2) }.is_some());
    }

    /// This thread's claimed elements so far.
    #[cfg(debug_assertions)]
    fn thread_claims() -> u64 {
        THREAD_CLAIMS.with(std::cell::Cell::get)
    }

    /// A reader of `cells`' rows outside `own`, at `frames`, whole-plan.
    fn whole_reader<'t, T: Sync>(
        cells: &'t Cells<T>,
        own: Span<usize>,
        frames: Span<usize>,
        refusals: &'t AtomicU64,
    ) -> Reader<'t, T> {
        Reader {
            cells,
            check: Check::Whole,
            own,
            frames,
            refusals,
            #[cfg(feature = "test-hooks")]
            refuse: None,
        }
    }

    /// The checked mode claims each element: a view's window once, when it
    /// is made (ports × frames, the frame before the call included), none in
    /// a commit-shaped loop over it, and a reader each frame of a row each
    /// time it reads one (a source read twice in a call is claimed twice).
    /// A claim on one element another holds is refused; disjoint frames of
    /// one row are not in conflict.
    #[cfg(debug_assertions)]
    #[test]
    fn checked_mode_claims_each_element() {
        const K: usize = 8;
        // Rows 0 and 1: the viewed instance's two outputs; row 2: a source's.
        let cells = Cells::with_rows(vec![0.0_f64; 3 * K], K, vec![0, 0, 1]);
        let refusals = AtomicU64::new(0);
        assert_eq!(cells.claims.len(), cells.data.len());
        let before = thread_claims();
        // A call over frames 2..4, with the frame before it.
        // SAFETY: test of the tracker only; nothing else references `cells`.
        let mut window = unsafe { cells.window(&refusals, &(0..2), 1..4) }.unwrap();
        assert_eq!(thread_claims() - before, 2 * 3);
        for f in 2..4 {
            for p in 0..2 {
                window.set(p, f, f as f64);
            }
        }
        assert_eq!(thread_claims() - before, 2 * 3, "the loop claims nothing");
        let reader = whole_reader(&cells, 0..2, 2..4, &refusals);
        let signal = reader.frames(2, (2, 4));
        let modulation = reader.frames(2, (2, 4));
        assert!(signal.is_some() && modulation.is_some());
        assert_eq!(
            thread_claims() - before,
            2 * 3 + 2 * 2,
            "two reads of two frames"
        );
        drop((signal, modulation));

        let violations = exclusivity().violations;
        // SAFETY: as above; refused claims make no reference.
        assert!(
            unsafe { cells.window(&refusals, &(0..1), 1..2) }.is_none(),
            "frame 1 is held"
        );
        assert!(exclusivity().violations > violations);
        let later = unsafe { cells.window(&refusals, &(0..1), 4..K) };
        assert!(later.is_some(), "other frames of the row are free");
        drop((later, window));
        assert!(unsafe { cells.window(&refusals, &(0..1), 1..2) }.is_some());
    }

    /// A window for a call over frames 2..4 reaches the frame before the
    /// call and no other; at the block's first frame there is none before.
    /// Each refused frame counts in the plan's refusals (path 7).
    #[test]
    fn a_window_reaches_its_predecessor_frame() {
        const K: usize = 8;
        let cells = Cells::with_rows((0..2 * K).map(|x| x as f64).collect(), K, Vec::new());
        let refusals = AtomicU64::new(0);
        // SAFETY: test of the window only; nothing else references `cells`.
        let mut window = unsafe { cells.window(&refusals, &(0..2), 1..4) }.unwrap();
        assert_eq!(window.get(1, 1), Some((K + 1) as f64), "the frame before");
        assert_eq!(window.frames(0, (2, 4)), Some(&[2.0, 3.0][..]));
        window.set(0, 3, -1.0);
        assert_eq!(window.get(0, 3), Some(-1.0));
        assert_eq!(refusals.load(Ordering::Relaxed), 0);
        assert_eq!(window.get(0, 0), None, "two frames before");
        assert_eq!(window.get(0, 4), None, "after the call");
        assert_eq!(window.get(2, 2), None, "another instance's port");
        assert_eq!(refusals.load(Ordering::Relaxed), 3);
        drop(window);
        let first = unsafe { cells.window(&refusals, &(0..2), 0..2) }.unwrap();
        assert_eq!(first.get(0, 0), Some(0.0));
        assert_eq!(
            first.frames(1, (0, 2)),
            Some(&[K as f64, (K + 1) as f64][..])
        );
        assert!(
            unsafe { cells.window(&refusals, &(0..1), 3..K + 1) }.is_none(),
            "past the block"
        );
        assert_eq!(refusals.load(Ordering::Relaxed), 4);
    }

    /// A reader refuses the viewed instance's own row, a row past the
    /// stride and frames outside the call, each counted (paths 4 and 5).
    #[test]
    fn a_reader_counts_its_refusals() {
        let cells = Cells::with_rows(vec![1.0, 2.0, 3.0, 4.0], 2, vec![0, 1]);
        let refusals = AtomicU64::new(0);
        let reader = whole_reader(&cells, 0..1, 0..2, &refusals);
        assert!(reader.frames(1, (0, 2)).is_some());
        assert_eq!(refusals.load(Ordering::Relaxed), 0);
        let refused = |read: &dyn Fn() -> bool, what: &str| {
            let before = refusals.load(Ordering::Relaxed);
            assert!(read(), "{what}");
            assert!(refusals.load(Ordering::Relaxed) > before, "{what} counted");
        };
        refused(&|| reader.frames(0, (0, 1)).is_none(), "its own row");
        refused(&|| reader.frames(1, (1, 3)).is_none(), "outside the call");
        refused(&|| reader.read(1, (0, 3)).is_none(), "past the stride");
        refused(&|| reader.row(7).is_none(), "no such row");
    }

    /// A task access may not view an instance of another task, and counts
    /// it (path 1); the doctests' whole-plan access is `None` by design and
    /// counts nothing.
    #[test]
    fn a_task_access_refuses_a_foreign_instance() {
        let mut cells = PlanCells {
            layout: vec![Layout::default(); 2].into_boxed_slice(),
            instance_task: vec![0, 1].into_boxed_slice(),
            ..PlanCells::default()
        };
        cells.board = TaskBoard::new(vec![0, 0], vec![Vec::new(), Vec::new()]);
        {
            let mut access = Access {
                cells: &cells,
                token: Token::Task { task: 0, epoch: 1 },
                lane: 0,
                main: None,
                _thread: PhantomData,
            };
            assert!(access.call(1, 0, 1).is_none());
        }
        assert!(cells.take_refusals() > 0);
        assert!(cells.exclusive().call(0, 0, 1).is_none());
        assert_eq!(
            cells.take_refusals(),
            0,
            "whole-plan views are not refusals"
        );
    }

    /// A caller that panics while a worker is in the job still closes the
    /// gate and waits for the worker to leave before it unwinds out of
    /// `serve` (the job lives on the caller's stack).
    #[test]
    fn a_caller_panic_closes_the_gate_and_waits_for_joined_workers() {
        use std::sync::atomic::AtomicBool;
        let (gate, mut opener) = Gate::new();
        let mut cells = PlanCells::default();
        let stats = PoolStats::default();
        let body = |_: &mut Access<'_>, _: usize| {};
        let job = Job::new(&mut cells, 1, &body, &stats);
        let (joined, left) = (AtomicBool::new(false), AtomicBool::new(false));
        std::thread::scope(|s| {
            s.spawn(|| {
                let mut seen = 0;
                let work = || {
                    joined.store(true, Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(20));
                    left.store(true, Ordering::SeqCst);
                };
                while !gate.join(&mut seen, |_| work()) {
                    std::hint::spin_loop();
                }
            });
            let unwound = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                opener.serve(
                    &job,
                    || {},
                    |_| {
                        while !joined.load(Ordering::SeqCst) {
                            std::hint::spin_loop();
                        }
                        panic!("the caller panics while a worker is in the job");
                    },
                );
            }));
            assert!(unwound.is_err());
            assert!(
                left.load(Ordering::SeqCst),
                "serve unwound before the worker left"
            );
            let word = gate.word.load(Ordering::SeqCst);
            assert_eq!(word & OPEN, 0, "the gate is closed");
            assert_eq!(word & JOINED, 0, "no worker is joined");
        });
    }

    /// A reader of task 1's of epoch 3 over `cells`.
    fn task_reader<'t>(
        cells: &'t Cells<f64>,
        board: &'t TaskBoard,
        instance_task: &'t [u32],
        refusals: &'t AtomicU64,
    ) -> Reader<'t, f64> {
        Reader {
            cells,
            check: Check::Task {
                task: 1,
                epoch: 3,
                instance_task,
                board,
            },
            own: 0..0,
            frames: 0..1,
            refusals,
            #[cfg(feature = "test-hooks")]
            refuse: None,
        }
    }

    /// A reader refuses a row whose producer has not completed in this epoch
    /// (path 3, counted), and reads it once it has.
    #[test]
    fn a_reader_waits_for_the_producer() {
        let mut board = TaskBoard::new(vec![0, 1], vec![vec![1], Vec::new()]);
        board.reset(3);
        let cells = Cells::with_rows(vec![1.0, 2.0], 1, vec![0, 1]);
        let instance_task = [0_u32, 1];
        let refusals = AtomicU64::new(0);
        let reader = task_reader(&cells, &board, &instance_task, &refusals);
        assert!(reader.row(0).is_none());
        assert!(refusals.load(Ordering::Relaxed) > 0);
        assert_eq!(reader.row(1).as_deref(), Some(&[2.0][..]));
        assert_eq!(board.claim(3), Next::Task(0));
        board.complete(0, 3, true);
        assert_eq!(reader.row(0).as_deref(), Some(&[1.0][..]));
        assert_eq!(board.claim(3), Next::Task(1));
        assert_eq!(board.claim(3), Next::AllClaimed);
        assert!(!board.failed());
    }

    /// AC-TASK: a failed task fails the job and is never marked done, so its
    /// successor's reads of its rows are refused even after its completion.
    #[test]
    fn a_failed_task_is_never_read() {
        let mut board = TaskBoard::new(vec![0, 1], vec![vec![1], Vec::new()]);
        board.reset(3);
        let cells = Cells::with_rows(vec![1.0, 2.0], 1, vec![0, 1]);
        let instance_task = [0_u32, 1];
        let refusals = AtomicU64::new(0);
        let reader = task_reader(&cells, &board, &instance_task, &refusals);
        assert_eq!(board.claim(3), Next::Task(0));
        board.complete(0, 3, false);
        assert!(board.failed());
        assert!(reader.row(0).is_none(), "a failed producer's row");
        assert!(refusals.load(Ordering::Relaxed) > 0);
        assert!(board.drained());
    }

    /// Path 11: a claim that advances `head` and finds its slot empty settles
    /// itself as a failed completion, so a drain never waits for it. The
    /// board is reset whole (the slot's epoch published), then only `ready`
    /// is cut short.
    #[test]
    fn a_claim_with_an_empty_slot_settles_itself() {
        let mut board = TaskBoard::new(vec![0], vec![Vec::new()]);
        board.reset(1);
        board.ready = Box::new([]);
        assert_eq!(board.claim(1), Next::NotYet);
        assert!(board.failed());
        assert_eq!(board.head.load(Ordering::Relaxed), 1);
        assert_eq!(board.completed.load(Ordering::Relaxed), 1);
        assert!(board.drained());
        assert_eq!(board.claim(1), Next::AllClaimed);
    }

    /// A job whose first claim finds an empty slot ends, on the caller and
    /// a worker, with the job failed (an invariant) and nothing left joined:
    /// the drain waits only for the claims made. Under a watchdog.
    #[cfg(feature = "test-hooks")]
    #[test]
    fn a_job_with_a_missing_slot_ends() {
        let (done, outcome) = std::sync::mpsc::channel();
        std::thread::spawn(move || {
            let tasks = 4;
            let mut cells = PlanCells {
                lanes: Cells::new((0..2).map(|_| Lane::default()).collect()),
                outs: Cells::new((0..tasks).map(|_| Out::default()).collect()),
                ..PlanCells::default()
            };
            cells.board = TaskBoard::new(vec![0; tasks], vec![Vec::new(); tasks]);
            cells.inject(Armed::MissingSlot);
            let stats = PoolStats::default();
            let body = |_: &mut Access<'_>, _: usize| {};
            let (gate, mut opener) = Gate::new();
            let job = Job::new(&mut cells, 1, &body, &stats);
            let over = std::sync::atomic::AtomicBool::new(false);
            std::thread::scope(|s| {
                s.spawn(|| {
                    let mut seen = 0;
                    while !over.load(Ordering::SeqCst)
                        && !gate.join(&mut seen, |job| job.participate(1))
                    {
                        std::hint::spin_loop();
                    }
                });
                opener.serve(&job, || {}, |job| job.participate(0));
                over.store(true, Ordering::SeqCst);
            });
            let _ = done.send(job.failure());
        });
        let failure = outcome
            .recv_timeout(std::time::Duration::from_secs(10))
            .expect("the job hung");
        assert_eq!(failure, Some(FailureReason::InvariantViolation));
    }
}
