# ADR 0010: Algorithms are compiled Rust; no scripting or DSL yet

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
A custom language is the highest-cost, highest-regret item. Agents are fluent in Rust.

## Decision
- DSP and generative algorithms are Rust primitives behind the module contract.
- Composites contain no logic: no conditionals, loops, or expressions.
- A scripting layer (rhai, wasmtime) or a DSL is reconsidered only with evidence from real projects.

## Alternatives considered
Rhai scripting from M1.

## Consequences
New behaviour requires a primitive (rebuild) or a composite of existing primitives.
