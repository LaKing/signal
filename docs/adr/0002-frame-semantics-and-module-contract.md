# ADR 0002: Frame-functional semantics and the k-frame module contract

- **Status:** Accepted. Refined by ADR 0016: scheduling, feedback execution. Amended by ADR 0020 (M2.7-3): after a module's panic in a call of several frames, the output depends on the partition (`docs/semantics.md` §14).
- **Date:** 2026-10-11

## Context
The PO proposed a master clock defining calculation frames, with everything computed per frame by pulling from outputs. The open question was semantics versus execution strategy.

## Decision
- **Semantics (D-EXEC-1, approved):** every instance computes `(inputs[n], state[n-1]) -> (outputs[n], state[n])` per frame *n*. Every feedback loop contains at least a one-frame delay; loops without a delay are rejected at load. Every instance advances every frame, and suspension is explicit.
- **Contract (D-EXEC-2, approved):** `process` handles *k* ≥ 1 consecutive frames per call, and output must be identical for every partition of the timeline into *k*-sized calls. Events arrive with frame offsets inside the *k*-block. `prepare` is the only place a module may allocate.
- **Scheduling (D-EXEC-3, deferred):** the engine starts per-frame (*k* = 1). Evaluation order is computed whenever the graph changes and covers every non-suspended instance (ADR 0016 §1). Block execution is added only if benchmarks (V13, M4) require it, and it must match the per-frame engine bit for bit.

## Alternatives considered
- Recursive pull at every frame: same results as a precomputed order, at higher cost.
- Block execution from day one: unproven need; makes feedback harder.
- Lazy evaluation of unobserved modules: rejected, because a module's time would stop when no one observes it.

## Consequences
- Output is independent of device buffer sizes by definition.
- The per-frame engine is the permanent reference oracle.
- Every primitive must pass the *k*-partition-invariance contract test.
