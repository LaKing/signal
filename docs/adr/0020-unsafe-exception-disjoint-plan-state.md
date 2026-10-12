# ADR 0020: Unsafe exception: disjoint access to a plan's state

- **Status:** Accepted (operator decision sq-4e0e7cd8, delegated by the owner, 2026-10-12: option A, with conditions (1)–(4) below). Extends ADR 0019 §4.
- **Date:** 2026-10-12

## Context
ADR 0019 §2 runs a block's units on a fixed pool of worker threads that take ready tasks as their dependencies complete, with no allocation, lock or blocking system call during a block. The engine keeps a plan's state in flat buffers sized when the plan is built (`docs/semantics.md` §13): outputs, inputs and parameters per block frame, slots, primitives, event stores. A unit writes only its own instances' rows and reads the rows of the instances it depends on.

In safe Rust, threads that live across blocks can share such buffers and write disjoint parts of them only through a lock, through atomics (impossible for the `&mut [f64]` slices and primitives the module contract hands out), or by moving ownership every block. `unsafe_code` is denied workspace-wide (AGENTS.md rule 9). The alternatives were put to the operator:

- **B.** Safe Rust with uncontended `try_lock` per unit and atomics for outputs: reads "no locks" as "no blocking locks", and re-lays out every buffer.
- **C.** Safe Rust with tasks statically assigned to threads, each owning its instances, and data crossing threads through queues: weakens "take ready tasks", balances worse, and is the largest restructure.

The operator chose A, this ADR, under four conditions: (1) this ADR, accepted, with the module listed in AGENTS.md rule 9 and ADR 0019 §4 and `#![allow(unsafe_code)]` in that file only; (2) a minimal API that hands out a `&mut` to an element or a slice only against proof of exclusivity from the schedule, never a general raw-pointer escape hatch, with a `SAFETY:` comment on every `unsafe` block tied to the ordering (Release on completion, Acquire before reading); (3) a checked mode with an atomic claim per element in tests and debug builds; (4) a test that shows `unsafe` only in the audited modules.

## Decision
- Exactly one more module may use `unsafe`: `crates/signal-engine/src/shared.rs`, with `#![allow(unsafe_code)]` scoped to that file. `unsafe_code` stays denied everywhere else.
- **Storage.** `Cells<T>` holds one buffer as `UnsafeCell`s. Without a job, `get_mut(&mut self)` is the whole slice, and `get(&self)` is the whole slice shared, only for `T: Sync`.
- **Views.** `PlanCells` holds every per-instance buffer of a plan. A task's `Access` hands out one `CallView` of one instance's call (block frames `a..b`) at a time: the instance's own parts mutably (slot, primitive, parameters, held outputs, sinks and emitted events, the thread's lane, the task's log) and, of the per-frame block buffers (signal inputs, effective parameter values, outputs), a `Window` of its own rows at the call's frames; for parameter values and outputs also the frame before the call, `a − 1`, which the holds of a suspended or faulted instance read. A window's accessors reach only those elements. The view reads other instances' outputs and events through `Reader`s that refuse the viewed instance's own rows (the view holds them mutably; a state-output instance reading itself reads its window). Each view borrows the `Access` mutably, so the compiler rules out two views of one access at once. A task's views must be of instances the task owns (checked in every build).
- **Sequential segments use no views.** A segment on the calling thread alone (one thread, or a segment too small or too short to gain) takes the buffers whole with `get_mut` and indexes them by instance, as M2.6-1 did: the views' construction cost a measured 14–23 % at *k* = 1 (`docs/benchmarks.md`). Both kinds of segment run the same bodies (`gather`, `commit`, the fades and holds), so they sum and order alike; the determinism matrix compares them at every *k*.
- **Reads.** A `Reader` hands out frames of a row of another instance, inside the call's frames, only if its owner is the reader's own task or a task whose completion it observes: the producer stores its completion with Release, the reader loads it with Acquire before the read.
- **The frame before a call.** Claiming frame `a − 1` of the instance's own rows conflicts with no one: if it precedes the segment, an earlier, complete segment wrote it; otherwise this instance's earlier call in the same task did, and other tasks read this instance's rows only after this task's completion, and only at their own call's frames.
- **Invariants.**
  - A `&Cells` and a live view never coexist: views exist only inside an `Access` or a `Job`, and both hold the plan's cells mutably for their whole life; a job's task bodies see only their views, never a `&Cells` or `&PlanCells`.
  - Every shared read (`get`, `Reader`) requires `T: Sync`. Exclusive access requires only `T: Send`, as for `Mutex<T>` (`unsafe impl<T: Send> Sync for Cells<T>`). The primitives (`Send`, not necessarily `Sync`) are reached only through `get_mut` and the view's `&mut`.
  - The doctests of `shared.rs` prove the compile-time half: two views of one access (of one instance or of two) fail with E0499; `get` and `Reader` of a non-`Sync` element fail with E0277; each beside a sibling that compiles.
- **Publication (the job's lifetime).** A `Job` is published through a `Gate`: one atomic word holding a 48-bit epoch, an `open` bit and the number of joined workers, beside the job pointer. `Job` is `Sync`, checked at compile time (`assert_sync`), since workers share it.
  - The caller writes the pointer, opens the gate for a new epoch with a SeqCst swap (which includes the Release), runs tasks itself, closes the gate and waits until no worker is joined (Acquire). The close and the wait are a drop guard made right after the open, so they also run when the caller unwinds (a panic in a task on the calling thread, or in the wake-up); only then may it return, unwind past, republish or drop what the job borrows.
  - A task's completion is a drop guard made as soon as the task is claimed, before the test hook and the task's body run, so a task whose hook or body panics is still completed and no thread waits for it.
  - A worker joins with `compare_exchange(word, word + 1, AcqRel, Relaxed)`, which succeeds only while the gate is open for that epoch; its Acquire pairs with the open, so the pointer read after it is synchronised. It reads the pointer only after joining, and leaves with a Release decrement (in a drop guard, also on unwinding) that pairs with the caller's Acquire wait.
  - A worker that loaded the word and paused fails its join once the gate has closed, and never reads the pointer. If the epoch wrapped and the word matched again, the gate is open for a live job, which the worker then joins legitimately. A joined worker is counted, so the caller cannot return while it is in the job. Hence no worker holds any reference to a plan between segments, and a plan swap needs no wait.
- **Waking.** Workers spin about 50 µs on the gate, then store their `parked` flag (SeqCst), re-read the gate (SeqCst) and park. The caller opens the gate (SeqCst) and then reads the flags (SeqCst): in the single order of these operations, a worker either sees the open gate or is seen parked and woken, so no wake is lost (`docs/realtime-rules.md` "Worker threads").
- **Checked mode** (debug builds, and so every `cargo test`): each buffer carries an atomic claim per element. A view claims each element it holds for writing once, when it is made (a window: its ports × its frames; its accessors claim nothing), and a reader claims each element of the frames it reads for reading. A conflicting claim is refused, so no reference is made, and counted, as is any access outside a window or a refused read; `signal_engine::exclusivity()` reports the elements claimed and the refusals. The determinism matrix and the stress test assert that it ran and found nothing.
- **Scope check.** `crates/signal-engine/tests/unsafe_scope.rs` reads every `.rs` file under `crates/` and `lab/` and fails if `unsafe` or `allow(unsafe_code)` appears outside `alloc_counter.rs`, `rt_thread.rs` and `shared.rs`.
- Every `unsafe` block carries a `SAFETY:` comment.

## Alternatives considered
- **B, uncontended locks**, and **C, static lanes**: not chosen (Context).
- **A scoped thread pool per block** (`std::thread::scope`): safe, but it spawns threads every block, which ADR 0019 §2 forbids.
- **Element-by-element atomics for outputs**: cannot carry the slices, events and primitives the module contract hands out.

## Consequences
- The scheduler (ADR 0019 §2, M2.6-2) runs on the existing buffer layout, so the block engine's bit-identity argument carries over.
- Miri is not available offline; soundness rests on the borrow structure the compiler enforces, the ownership and completion checks, the checked mode in every test, and review of this module.
- Any further `unsafe` still needs its own ADR.
