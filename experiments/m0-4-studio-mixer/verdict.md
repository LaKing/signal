# M0.4 studio mixer: the golden decision

The owner's decision on ADR 0051 §13 (phase 4): the studio's `bus` (`std.mix` at gains 0.45, 0.45, 0.55) is replaced by a **Mixer** instrument on `lib.mixer`, and the goldens that play the studio are written again (`docs/testing-and-evaluation.md`, "Updating a golden").

| | |
|---|---|
| Decision | "igen" (yes), to the operator's question whether the Mixer goes into the studio, changing its golden by at most −147 dB |
| Given by | Owner (PO), relayed verbatim by the operator (an AI agent, Claude) |
| Date | 2026-11-14 |
| Listening verdict | "Meghallgattam, szól, működik, tökjó." (I listened to it: it plays, it works, it's great), 2026-11-14, after the change was pushed (c3e7d16). The golden decision itself was taken on the measurement below, before listening |

## What changed, and why the sound moves

- `bus` is `lib.mixer` with `a_level = -6.93575`, `b_level = -6.93575`, `c_level = -5.192746` (the old gains in dB, 20·log10(0.45) and 20·log10(0.55)); channel d is the spare. `std.gain`'s 10^(dB/20) does not give back 0.45 and 0.55 exactly, so the sum differs in its last bits.
- Each mono source feeds both sides of its channel; each side has its own `std.dc_block` and `std.tanh_shaper` (`dc_r`, `limiter_r`) into `out.r`. At balance 0 the two sides are equal, sample for sample.
- The panel has a MIXER card (a level fader, balance knob and mute toggle per channel, the master level) and `ui/plane.toml` lists it before the master card.

## Measured

A 16 s render (`signal render projects/studio --frames 768000 --threads 1`, release): largest difference from the previous studio 1.49e-8 (−156 dBFS, −143 dB below the render's 0.22 peak; the question to the owner said −147 dB), left equal to right in every frame, the log unchanged (`log_fnv1a64` the same in all three goldens).

| Golden | `audio_fnv1a64` before | after |
|---|---|---|
| `tests/golden/studio` | `ae83d794a40b6941` | `4acad25ddbd56c51` |
| `tests/golden/two-studios` (includes `../studio`) | `b06a1b70320c1059` | `46ac6481c0965e69` |
| `tests/golden/two-decks` (`deck_b` loads `../studio`) | `b06a1b70320c1059` | `46ac6481c0965e69` |

The studio golden also lists `library_composite_hashes` (`lib.mixer`) now.
