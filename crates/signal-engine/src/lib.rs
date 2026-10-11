//! Signal engine: the per-frame executor (ADR 0016 frame phases, event order,
//! parameter bases, ramps, suspension), fault guards, and drivers. The offline
//! driver is the reference (ADR 0012). Real-time code follows docs/realtime-rules.md.
//!
//! Planned for M1 (offline) and M2 (real time).
