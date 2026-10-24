# M3 tanh aliasing verdict: ADAA versus oversampling

The owner's verdict on the M3 "ADAA versus oversampling" experiment (`README.md` here; follow-up issue `2026-10-11-follow-up-bring-std-tanh-shaper-aliasing-to-60-db-on-off-div`). ADR 0015: a lab candidate is promoted to `std` only with a recorded human verdict. The format follows the earlier verdicts (`docs/testing-and-evaluation.md`, "Human verdicts") and is provisional until Q11 is decided.

| | |
|---|---|
| Verdict | "Nem hallok külömbséget. Ez technikai részletnek tűnik, rád bízom" (I hear no difference. This seems a technical detail, I leave it to you) |
| Candidate chosen (or none) | `lab.tanh_os8_adaa1`, promoted as a **new** type `std.tanh_shaper_os8` beside `std.tanh_shaper`, which stays as it is. V6 applies to the new type. Chosen by the operator, the owner having delegated the choice |
| Given by | Owner (PO), relayed verbatim by the operator; the choice of candidate is the operator's (an AI agent, Claude, acting on the owner's delegation), not a human listening verdict |
| Date | 2026-10-23 |
| Owner decision | Delegated to the operator in the same words |
| Was the package decision-useful (M3 exit condition)? | "igen" (yes), asked by the operator on 2026-10-23 whether the package helped the decision, even though what it showed was that no difference is heard |

## The operator's choice, and why

The owner heard no difference, so the choice rests on the costs the README lists, which are technical:
- **`std.tanh_shaper` is not replaced.** It runs inside the tape echo's feedback loop (`sat`), where ≈ 74 frames of delay would lengthen every repeat; it would shift the kick's onset that V3 and V8 assume; its output would no longer stay within [−1, 1]; and six goldens and two pinned library renders would change for a difference not heard.
- **`lab.tanh_os8_adaa1` becomes `std.tanh_shaper_os8`.** It is the only candidate that clears 60 dB at 96 and at 48 kHz with margin, and V6's 60 dB threshold stays where it is (owner decision sq-4aece389); V6 is retargeted to the type that meets it. A project chooses it where aliasing matters and the delay does not (a master limiter, a hard-driven voice), and the delay and the overshoot to 1.27 are documented on the type.
- **The minimum-phase variant is not built now.** Nothing needs a low-delay oversampled shaper yet (AGENTS.md rule 8); it stays a proposal in the README.
- The other four candidates stay in `lab/` as the experiment's record.

## The renders judged

The package made by `experiments/m3-tanh-aliasing/render.sh` on 2026-10-23 (`renders/m3-tanh-aliasing/`, opened for the owner in Finder): sweep, bass, kick and studio through all six shapers. Their `audio_fnv1a64` values are the ones in `README.md`, "The renders", and `renders/m3-tanh-aliasing/index.tsv`; the run reproduced every one. Which files the owner played was not said.
