# M2.11 verdict: the studio demo, live listening

The owner's listening verdict on the M2.11 studio demo (`docs/roadmap.md` M2.11, ADR 0028). The format is provisional until Q11 is decided (`docs/testing-and-evaluation.md`, "Human verdicts").

| | |
|---|---|
| Verdict | "Nagyon jó!" ("Very good!") |
| Given by | Owner (PO), relayed by the operator |
| Date | 2026-10-17 |
| Owner decision | given in the operator's session after listening, not through devpanel |

## What was heard

`projects/studio` at commit `a58acc7`, played live in `signal ui` (CoreAudio, the Mac's default output) from an unmodified copy of the project, with the panel open in the browser, for several minutes. The owner could edit the patterns and turn the panel's controls while it played; the verdict covers the project as committed, with its default settings as the starting point. It is not a render, so there is no render hash of exactly what was heard; the identifiers below are those of the project's golden, which renders the same project files at its defaults.

| Field | Value |
|---|---|
| Project | `projects/studio` (commit `a58acc7`) |
| Golden | `tests/golden/studio/manifest.json` |
| Seed | 11 |
| Sample rate | 48000 Hz |
| Frames | 5760000 (120 s) |
| `audio_fnv1a64` | `ae83d794a40b6941` |
| `log_fnv1a64` | `c4920d6570c2a66b` |
| `project_file_hashes` | as listed in the golden manifest (`project.toml` `8dc10070eec67261`, `composites/moog_voice.toml` `f064466de222ca04`, `composites/kick909.toml` `cce33f9694d23e0c`, …) |

## The ladder golden

`std.ladder` (M2.11-2, commit `e790596`) was heard here as the Moog voice's filter, not as its own golden render (`tests/golden/ladder/`, a 1 s saw sweep, `audio_fnv1a64` `53ad8224b921bd11`). The operator records this verdict as covering the ladder's sound in use; the ladder fixture render itself was not listened to separately.
