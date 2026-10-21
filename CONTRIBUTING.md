# Contributing to Signal

Thank you for your interest. Signal is developed in the open, mostly by AI agents working under a human operator and the owner's decisions; see "How Signal is developed" in the [README](README.md). Issues and pull requests from people are welcome. Read [`AGENTS.md`](AGENTS.md) first: it is the working guide for agents and people alike.

## Licence of contributions

Signal is licensed under the **GNU General Public License, version 3 or later** (`GPL-3.0-or-later`, [`LICENSE`](LICENSE); ADR 0031). By contributing, you agree that your contribution is licensed under the same terms. Dependencies are a separate matter: they must stay permissive (below).

## Setup

You need a Mac with Apple Silicon and [Homebrew](https://brew.sh). Then, in the repository:

```sh
./install.sh        # rustup (Homebrew), the pinned toolchain, cargo-nextest, cargo fetch --locked, then the checks
./start.sh          # build Signal.app (release) and open it
```

`./install.sh` fetches every dependency, so later builds work offline (`--offline`). Homebrew's rustup is keg-only: if `cargo` is not on your `PATH`, prefix commands with `PATH=/opt/homebrew/opt/rustup/bin:$PATH`. The toolchain is pinned in `rust-toolchain.toml` because golden renders depend on it (ADR 0007, 0008). Change it only as a reviewed decision.

### The target directory

`.cargo/config.toml` sets cargo's target directory to the absolute path `/tmp/signal-project-target`, outside the repository. The development tooling (devpanel) delivers every untracked file in a run's worktree, git-ignored ones included, so a `target/` inside the repository would break that. Keep the setting. To build somewhere else, override it:

```sh
CARGO_TARGET_DIR=~/signal-target cargo test --workspace --offline
scripts/bundle-app.sh --target-dir ~/signal-target     # the scripts take --target-dir too
scripts/release.sh --target-dir ~/signal-target --out ~/signal-release
```

CI keeps the default (`/tmp` is writable on the runners). Its lab job uses a directory of its own, `CARGO_TARGET_DIR=/tmp/signal-project-target-lab`.

## The checks

CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml)) runs all of these, and a pull request should pass them locally first:

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked --offline -- -D warnings                          # default features
cargo clippy --workspace --all-targets --locked --offline --features signal-cli/lab -- -D warnings # with the lab tier
cargo test --workspace --locked --offline                    # debug (CI: cargo nextest run --workspace)
cargo test --workspace --locked --offline --release          # release: the checked mode is compiled out
cargo test --workspace --locked --offline --features signal-cli/lab
node --test ui/                                              # the web UI's logic (also run inside cargo test)
```

CI also runs the scheduler's loom model tests (`RUSTFLAGS="--cfg loom" cargo test -p signal-engine --lib --release model::`). Tests that need a tool that is missing (`node`, `codesign`) skip with a message rather than fail. CI runs nextest with the `ci` profile of [`.config/nextest.toml`](.config/nextest.toml): every failure is reported, and the three tests that measure the device's real-time scheduling (`semaphore_workers_are_reached_by_every_wake_and_stop_cleanly`, `time_constraint_workers_play_the_offline_render`, `a_run_without_a_stall_reports_no_hang`) are left out, since a hosted runner's virtual machine cannot give it, and one that times the host by a margin it sometimes misses is retried; run them on a Mac with `cargo test`.

The tests include `THIRD-PARTY-NOTICES.md`: it must match `Cargo.lock`, and every linked crate's licence must be allowed. After a dependency change, run `node scripts/third-party-notices.mjs` and commit the result.

## The hard rules, in short

[`AGENTS.md`](AGENTS.md) states them in full:

1. **No domain concepts in the core.** `signal-core`, `signal-graph` and `signal-engine` know frames, ports, events and addresses only. Tempo, MIDI and the like live in library modules; output policy lives in `signal-output`.
2. **Real-time rules.** No allocation, locks, I/O or panics in `process` or the executor ([`docs/realtime-rules.md`](docs/realtime-rules.md)). Primitive calls must be bounded in time; anything that could block goes behind a latent boundary.
3. **Determinism.** No wall-clock time or unseeded randomness in semantics. Output is identical for any block size and any thread count.
4. **Text is the source of truth.** A project means only what its files say; the GUI and tools edit files and hold no hidden state.
5. **Experimental work goes in `lab/`**, behind the `lab` feature. Promotion needs the contract suite, benchmarks and a recorded human verdict.
6. **Metrics never decide quality.** Present renders and metrics; a human records the verdict.
7. **Dependencies** come only from the root `[workspace.dependencies]`. Their licences must be permissive (MIT, Apache-2.0, BSD, ISC, Zlib, Unicode); MPL-2.0 only with a note; never GPL (ADR 0031). Adding a crate is an operator step: edit the root `Cargo.toml`, then run `./install.sh`.
8. **Don't build ahead of the roadmap** ([`docs/roadmap.md`](docs/roadmap.md)).
9. **`unsafe` is denied** workspace-wide, except in three audited modules, each with its own ADR.

Use the terminology of [`docs/00-vision.md`](docs/00-vision.md) §3a: Project, Module, Primitive, Composite, Instance, Port, Connection, Parameter, Preset, Library, Setup.

## Decisions: the ADR process

Design decisions are recorded as Architecture Decision Records in [`docs/adr/`](docs/adr/README.md): context, decision, alternatives and consequences, with who decided and when. An accepted ADR is never rewritten and never silently contradicted. To change a decision, propose a new ADR that supersedes the old one, mark the old one superseded, and update the index. Changes that touch a decision cite its ADR in the code and in the pull request.

## Determinism and golden renders

Determinism is a promise to the user. The same project, seed and toolchain must give the same output, bit for bit, on any number of threads and with any block size ([`docs/semantics.md`](docs/semantics.md); ADR 0002, 0008, 0019). The test suite checks it.

`tests/golden/*/manifest.json` hold the audio and log hashes of reference renders of the example projects ([`docs/testing-and-evaluation.md`](docs/testing-and-evaluation.md), "Golden renders"). A golden changes only for an intentional change of sound or of the project files, and then only with:

1. a written note of what changed and why the sound is meant to change, reviewed with the change; and
2. a listening verdict from the owner, recorded in the repository.

A golden that changes without both is a regression, whatever the metrics say. Do not edit `projects/*` or a golden to make a test pass.

## Pull requests

- Keep each change small and focused, with tests. Say what you ran and what it showed; label claims as fact, measurement or assumption.
- Update the documents your change touches (`docs/`, `CHANGELOG.md` under the coming version).
- Versions are the owner's decision (`scripts/version.sh`). Do not change the workspace version in a pull request.
