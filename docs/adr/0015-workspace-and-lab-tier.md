# ADR 0015: Workspace layout and the lab tier

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Clear module boundaries help AI agents. Experimental code must not compromise the real-time core (G10).

## Decision
- A Cargo workspace with `signal-*` crates (inception proposal §7), each created only when its milestone needs it.
- Experimental primitives live in `lab/` and compile in only with the `lab` feature.
- **Promotion to the standard library requires:** passing the contract suite, benchmarks, and a recorded human verdict.
- Test projects (fixtures) live in `projects/`; experiments in `experiments/`.

## Alternatives considered
A single crate with modules: simpler, but weaker boundaries.

## Consequences
If boundaries prove too fine in M1, `signal-core`, `signal-project`, and `signal-graph` may be merged and split again later.
