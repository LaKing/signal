# ADR 0006: Versioned graphs and live edits

- **Status:** Accepted
- **Date:** 2026-10-11

## Context
Modules must be loaded, unloaded, and reconnected on the fly without dropouts, and live sessions must replay bit-exactly.

## Decision
- Composites are flattened into an execution **plan**. Every graph change produces a new plan version.
- **Edits** (add/remove instance, connect/disconnect, set parameter) are commands stamped with a requested frame. The control thread builds the plan and calls `prepare` on new instances. The audio thread swaps plans at the first frame ≥ the request once the plan is ready. Surviving instances move into the new plan with their state. Old plans are freed off the audio thread.
- The **effective frame** of every edit is logged, so replay uses the log and stays exact.
- Added or removed instances get a deterministic engine-applied fade (default 5 ms), which modules may override (D16).
- Live edits are issued by watching project files and diffing them into edit commands, plus CLI commands. The GUI uses the same command interface (D17).

## Alternatives considered
In-place mutation of the running graph: harder to make real-time safe and deterministic.

## Consequences
- Edit preparation latency is visible: an edit applies a few milliseconds after it is requested.
- The command log is the single record of what happened in a session.

**2026-11-18, the watch reads what a load reads; an edit too large for the queue is invalid** (operator, after an outside review of `5317b11`).
- `ProjectWatcher` walked the whole folder every 100 ms on the control thread: 5,000 unrelated files under `past-renders/` made a poll about 23 ms (the reviewer's measurement). It now stats the top-level files, `composites/`, `patterns/`, `interfaces/` and `ui/`, the files the project names (sample WAVs, capture render files) and the projects included inside the folder, nothing else. A sample's WAV under `ui/` is no presentation change (`ProjectWatcher::presentation_only`), so its edit reloads (`crates/signal-project/tests/live.rs`, `the_watcher_follows_what_the_project_reads`).
- An edit needing more commands than the whole command queue holds was refused like a full queue, so the host reloaded and replanned it every poll. It is now `EditError::Invalid` ("split it"), which the watch does not retry; only a queue that draining can free is retried (`an_edit_larger_than_the_command_queue_sends_nothing`, `crates/signal-project/tests/write.rs` `an_edit_larger_than_the_queue_is_invalid_not_retried`).
