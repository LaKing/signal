# Roadmap

Milestones are quality-gated, not date-gated. Each one ends with something audible, visible, or measurable, and with a human verdict. Acceptance IDs (V1…) refer to `01-inception-proposal.md` §8.4.

| M | Status | Deliverable | Exit condition |
|---|---|---|---|
| **M0 Foundations** | **In progress**: hosted CI not yet run (no remote); ADR 0016 awaiting approval | Approved vision and ADRs; git repo; workspace skeleton; pinned toolchain; AGENTS.md; real-time rules; CI (fmt, clippy, tests) | CI green on the skeleton; PO accepts M0 |
| **M1 Core + offline** | — | Core types, module contract and contract suite, graph and flattening, TOML project loading and schema, slice primitives, offline driver, `signal render / modules / schema`, test project `projects/kick` | V1–V8 pass; golden render; **PO listening verdict (V14)** |
| **M2 Real time + live edits** | — | CoreAudio driver, plan swap, edit commands, `--watch`, session log and `signal replay`, fault guards, benchmarks | V9–V11, V13; 30-min soak (V12). **Vertical slice complete.** |
| **M2.5 Signal.app shell** | — | `.app` bundle with permissions; **interface setup dialog** (devices, I/O, sound card) stored as machine Setup; **metering**; GUI toolkit ADR | PO configures devices and watches meters on a running project; setup round-trips to files |
| **M3 Evaluation loop** | — | `signal compare`, listening packages, `experiments/` workflow; first lab experiment (ADAA vs oversampling), run by an agent | PO finds the comparison package decision-useful |
| **M4 Second domain: analysis** | — | Audio input (recorded and replayed), rate divisors, vector ports, first auto-converters, FFT analyser, test project `tone-analyser`; D-EXEC-3 decided from benchmarks | PO judges the measurements credible |
| **M5 Stateful and generative** | — | Deterministic async helpers, sampler, stateful sequencing, hierarchical musical time; ≥ 20-min generative test project | Long-form listening review |
| **M6 External I/O** | — | MIDI/OSC input, Art-Net output, clock-domain adapters; test project: audio-reactive DMX ("blue on bass") | Live session with external control, replayed offline |
| Later | — | Control panels (GUI b), visual output, external sync, plugin export, visual patching (GUI c, nice-to-have), scripting evaluation | — |
