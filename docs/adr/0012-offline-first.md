# ADR 0012: Offline-first delivery

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Offline rendering is deterministic, debuggable, and testable without hardware.

## Decision
- The offline driver is the reference implementation.
- The vertical slice is delivered as M1 (offline) and M2 (real time, live edits, replay).

## Alternatives considered
Building both drivers in one milestone.

## Consequences
The first audible result needs no audio hardware; the real-time path must match offline bit for bit (V9).
