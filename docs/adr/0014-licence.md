# ADR 0014: Licence: provisional, all rights reserved

- **Status:** Superseded by [ADR 0031](0031-licence-gpl-and-first-public-release.md) (2026-10-20). Was: accepted (provisional, chosen by the agent on the PO's delegation)
- **Date:** 2026-10-11

## Context
The PO delegated the licence choice "for now" (Q2). All researched runtime dependencies are MIT/Apache-2.0, except MPL-2.0 for triple_buffer, audio_thread_priority, and symphonia, none of which is currently planned.

## Decision
- The code is **all rights reserved, unpublished** for now. Crates set `publish = false` and carry no licence field.
- Rationale: this is the only reversible option. Opening the code later (e.g. MIT OR Apache-2.0, or GPL) is always possible; withdrawing an open licence is not.
- New dependencies must be permissive (MIT, Apache-2.0, BSD, ISC, Zlib, or similar). MPL-2.0 is allowed with a note. No GPL dependencies, so every future licence option stays open.

## Alternatives considered
- MIT OR Apache-2.0 now (the Rust ecosystem default).
- GPL-3.0 now.

## Consequences
Revisit before any public release or outside contribution.
