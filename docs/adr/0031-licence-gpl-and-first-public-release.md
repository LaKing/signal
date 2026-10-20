# ADR 0031: Licence GPL-3.0-or-later and the first public release (v0.2.0)

- **Status:** Accepted (owner's decisions of 2026-10-20: "let us reach a state that can be released on GitHub, that is the next goal, a GitHub release"; licence GPL-3.0-or-later; a public repository with its history unchanged; v0.2.0 as a pre-release; no Developer ID, so the app stays signed ad hoc; the internal development record stays, with a note). Supersedes ADR 0014. Release mechanics by the operator, delegated.
- **Date:** 2026-10-20

## Context
ADR 0014 kept the code "all rights reserved, unpublished" as the reversible choice, to be revisited before any public release. The owner now wants Signal published on GitHub with a downloadable release. The repository records its own development: ADRs with the owner's words, roadmap decisions with their IDs, devpanel's runs and seats, experiments with human verdicts.

## Decision
1. **Licence.** Signal is **GPL-3.0-or-later**. The root `LICENSE` holds the GNU GPL version 3 text unchanged; every crate declares `license = "GPL-3.0-or-later"` through the workspace; the README says so. Crates stay `publish = false` (no crates.io release now).
2. **Dependencies stay permissive.** New dependencies are MIT, Apache-2.0, BSD, ISC, Zlib, Unicode or similar; MPL-2.0 with a note (all GPL-3.0 compatible). GPL dependencies are not added, so that the libraries could be relicensed more permissively later if the owner wishes. A release ships a third-party notice generated from `Cargo.lock` with each linked crate's licence texts (Apache-2.0 notices and MPL-2.0 source notes included).
3. **Public repository, history as it is.** Only `main` and the `v*` tags are pushed; devpanel's branches and refs (`refs/devpanel/*`, `devpanel/*`) and stashes stay local. The development record (devpanel, decision IDs, the owner's quotes and verdicts, hardware notes, `AGENTS.md`) stays as it is; the README explains how Signal is developed (AI agents under devpanel, an operator, the owner's recorded verdicts).
4. **The release.** A GitHub **pre-release v0.2.0** (`Cargo.toml` version 0.2.0, tag `v0.2.0`; `scripts/version.sh` then counts from it). Assets, built by one script that runs the same locally and on GitHub Actions (on a `v*` tag, macOS 15, Apple Silicon, full history):
   - `Signal-<version>-macos-arm64.zip`: `Signal.app` (ad-hoc signed, hardened runtime, as today) with `LICENSE` and the third-party notice inside;
   - `signal-<version>-macos-arm64.tar.gz`: the `signal` CLI with the same two files;
   - `Signal-<version>-projects.zip`: the example projects;
   - `SHA256SUMS`.
   The release notes say what works, what does not yet, and how to open an app that is not notarized (right-click → Open, or `xattr -dr com.apple.quarantine Signal.app`). Developer ID signing and notarization are a later step that needs the owner's Apple Developer account.
5. **Public documents.** `README.md` (what Signal is, status, screenshots, install, build, licence, how it is developed), `CHANGELOG.md` (from 0.1 to 0.2.0, by milestone), `CONTRIBUTING.md` (checks, the hard rules, the ADR process), `SECURITY.md` (the local UI server and its token, `signal mcp`'s audio guards, how to report).

## Alternatives considered
- **MIT OR Apache-2.0**, **AGPL-3.0**, **binary-only**: offered; the owner chose GPL-3.0-or-later.
- **Rewriting history** to drop session links or squash to one commit: offered; the owner kept the history.

## Consequences
- ADR 0014 is superseded; AGENTS.md rule 7 cites this ADR.
- The first push and the release itself are operator steps with the owner's repository; the workflow can only be proven on GitHub, so the script it runs is proven locally first.
